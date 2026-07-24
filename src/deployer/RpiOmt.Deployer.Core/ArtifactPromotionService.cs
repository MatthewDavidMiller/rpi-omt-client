using System.Security.Cryptography;
using System.Text.RegularExpressions;

namespace RpiOmt.Deployer.Core;

internal sealed partial class ArtifactPromotionService(
    IArtifactSnapshotProvider snapshotProvider,
    ProgressRedactionService progress)
{
    private static readonly TimeSpan RemoteTimeout = TimeSpan.FromSeconds(60);
    private static readonly TimeSpan UploadTimeout = TimeSpan.FromMinutes(30);

    public static void ValidateProjectArtifacts(DeployOptions options)
    {
        DeploymentGuards.RequireRegularFile(options.ManifestPath);
        DeploymentGuards.RequireRegularFile(options.TransactionScriptPath);
        IReadOnlyList<string> names = ArtifactSnapshots.LoadManifest(options.ManifestPath);
        if (!names.Contains("deploy/transaction.sh", StringComparer.Ordinal) ||
            !names.Contains("deploy/manifest-v2.txt", StringComparer.Ordinal))
        {
            throw new DeploymentException(
                "Manifest v2 must include deploy/transaction.sh and deploy/manifest-v2.txt.");
        }

        foreach (string name in names)
        {
            if (options.BuildImage &&
                string.Equals(name, options.TarballName, StringComparison.Ordinal))
            {
                continue;
            }

            DeploymentGuards.RequireRegularFile(Path.Combine(options.ProjectRoot, name));
        }
    }

    public async Task UploadAndPromoteAsync(
        IRemoteClient remote,
        DeployOptions options,
        CancellationToken cancellationToken)
    {
        IReadOnlyList<string> manifestNames =
            ArtifactSnapshots.LoadManifest(options.ManifestPath);
        if (!manifestNames.Contains(options.TarballName, StringComparer.Ordinal))
        {
            throw new DeploymentException(
                $"Deployment artifact manifest does not include {options.TarballName}.");
        }

        if (!manifestNames.Contains("deploy/transaction.sh", StringComparer.Ordinal) ||
            !manifestNames.Contains("deploy/manifest-v2.txt", StringComparer.Ordinal))
        {
            throw new DeploymentException(
                "Manifest v2 must include deploy/transaction.sh and deploy/manifest-v2.txt.");
        }

        (string Name, string LocalPath)[] artifacts = manifestNames
            .Select(name => (
                Name: name,
                LocalPath: Path.Combine(options.ProjectRoot, name)))
            .ToArray();
        progress.SetStage(
            "local-verify",
            "Hashing stable local deployment artifacts...");
        var snapshots =
            new Dictionary<string, ArtifactSnapshot>(StringComparer.Ordinal);
        foreach ((string name, string localPath) in artifacts)
        {
            snapshots.Add(
                name,
                await snapshotProvider.CaptureAsync(localPath, cancellationToken)
                    .ConfigureAwait(false));
        }

        string token = RandomNumberGenerator.GetHexString(24).ToLowerInvariant();
        string remoteDirectory = options.RemoteDirectory.TrimEnd('/');
        string remoteStage = $"{remoteDirectory}/.deploy-staging/{token}";
        Transfer[] transfers = artifacts.Select(artifact => new Transfer(
            artifact.Name,
            artifact.LocalPath,
            $"{remoteStage}/{artifact.Name}")).ToArray();
        await RecoverAndPrepareStageAsync(
            remote,
            remoteDirectory,
            remoteStage,
            manifestNames,
            cancellationToken).ConfigureAwait(false);
        progress.SetStage("upload", "Uploading verified deployment artifacts...");
        try
        {
            foreach (Transfer transfer in transfers)
            {
                cancellationToken.ThrowIfCancellationRequested();
                progress.Emit($"Uploading {transfer.Name}...");
                int lastPercentage = -1;
                await remote.UploadAsync(
                    transfer.LocalPath,
                    transfer.StagedPath,
                    UploadTimeout,
                    (current, total) =>
                    {
                        cancellationToken.ThrowIfCancellationRequested();
                        int percentage = total == 0
                            ? 100
                            : checked((int)(current * 100 / total));
                        if (percentage == 100 || percentage >= lastPercentage + 5)
                        {
                            lastPercentage = percentage;
                            progress.Emit($"Uploading {transfer.Name}: {percentage}%");
                        }
                    },
                    cancellationToken).ConfigureAwait(false);
                ArtifactSnapshot snapshot = snapshots[transfer.Name];
                if (!await snapshot.IsUnchangedAsync(cancellationToken)
                    .ConfigureAwait(false))
                {
                    throw new DeploymentException(
                        $"Local deployment artifact changed during upload: {transfer.Name}.");
                }

                CommandResult checksum = progress.Redact(await remote.RunAsync(
                    $"sha256sum -- {Shell.Quote(transfer.StagedPath)}",
                    string.Empty,
                    null,
                    RemoteTimeout,
                    cancellationToken).ConfigureAwait(false));
                DeploymentGuards.RequireSuccess(checksum);
                string digest = checksum.StandardOutput
                    .Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries)
                    .FirstOrDefault()?.ToLowerInvariant() ?? string.Empty;
                if (!DigestPattern().IsMatch(digest) ||
                    !string.Equals(
                        digest,
                        snapshot.Sha256,
                        StringComparison.Ordinal))
                {
                    throw new DeploymentException(
                        $"Checksum mismatch after uploading {transfer.Name}.");
                }

                progress.Emit($"Verified SHA-256 for {transfer.Name}.");
            }
        }
        catch
        {
            await CleanupUploadsAsync(remote, transfers).ConfigureAwait(false);
            throw;
        }

        progress.SetStage("promotion", "Activating uploaded deployment files...");
        cancellationToken.ThrowIfCancellationRequested();
        string helper = transfers.Single(
            transfer => transfer.Name == "deploy/transaction.sh").StagedPath;
        string manifest = transfers.Single(
            transfer => transfer.Name == "deploy/manifest-v2.txt").StagedPath;
        string promotionCommand =
            $"bash {Shell.Quote(helper)} promote {Shell.Quote(remoteDirectory)} " +
            $"{Shell.Quote(token)} {Shell.Quote(manifest)}";
        try
        {
            CommandResult promotion = progress.Redact(await remote.RunAsync(
                promotionCommand,
                string.Empty,
                null,
                RemoteTimeout,
                cancellationToken).ConfigureAwait(false));
            DeploymentGuards.RequireSuccess(promotion);
        }
        catch
        {
            await CleanupUploadsAsync(remote, transfers).ConfigureAwait(false);
            throw;
        }
    }

    private async Task RecoverAndPrepareStageAsync(
        IRemoteClient remote,
        string remoteDirectory,
        string remoteStage,
        IReadOnlyList<string> manifestNames,
        CancellationToken cancellationToken)
    {
        progress.SetStage(
            "recovery",
            "Recovering any interrupted deployment transaction...");
        string v1Helper = $"{remoteDirectory}/deploy-transaction.sh";
        string v1Manifest = $"{remoteDirectory}/deploy-artifacts.txt";
        string v2Helper = $"{remoteDirectory}/deploy/transaction.sh";
        string recoveryCommand =
            $"if [ -x {Shell.Quote(v1Helper)} ] && [ -f {Shell.Quote(v1Manifest)} ]; then " +
            $"{Shell.Quote(v1Helper)} recover {Shell.Quote(remoteDirectory)} " +
            $"{Shell.Quote(v1Manifest)}; fi; " +
            $"if [ -x {Shell.Quote(v2Helper)} ]; then " +
            $"{Shell.Quote(v2Helper)} recover {Shell.Quote(remoteDirectory)}; fi";
        CommandResult recovery = progress.Redact(await remote.RunAsync(
            recoveryCommand,
            string.Empty,
            null,
            RemoteTimeout,
            cancellationToken).ConfigureAwait(false));
        DeploymentGuards.RequireSuccess(recovery);

        IEnumerable<string> directories = manifestNames
            .Select(name => name.LastIndexOf('/') is var separator && separator >= 0
                ? $"{remoteStage}/{name[..separator]}"
                : remoteStage)
            .Append(remoteStage)
            .Distinct(StringComparer.Ordinal)
            .OrderBy(path => path.Length)
            .Select(Shell.Quote);
        CommandResult preparation = progress.Redact(await remote.RunAsync(
            $"mkdir -p -- {string.Join(' ', directories)}",
            string.Empty,
            null,
            RemoteTimeout,
            cancellationToken).ConfigureAwait(false));
        DeploymentGuards.RequireSuccess(preparation);
    }

    private async Task CleanupUploadsAsync(
        IRemoteClient remote,
        IReadOnlyList<Transfer> transfers)
    {
        try
        {
            string stage = transfers[0].StagedPath;
            int suffixIndex = stage.IndexOf(
                "/.deploy-staging/",
                StringComparison.Ordinal);
            string relative = stage[
                (suffixIndex + "/.deploy-staging/".Length)..];
            int tokenEnd = relative.IndexOf('/');
            string stageRoot =
                stage[..(suffixIndex + "/.deploy-staging/".Length + tokenEnd)];
            CommandResult result = progress.Redact(await remote.RunAsync(
                $"if [ -d {Shell.Quote(stageRoot)} ] && " +
                $"[ ! -L {Shell.Quote(stageRoot)} ]; then " +
                $"find -P {Shell.Quote(stageRoot)} -xdev -depth -delete; fi",
                string.Empty,
                null,
                RemoteTimeout,
                CancellationToken.None).ConfigureAwait(false));
            if (!result.IsSuccess)
            {
                progress.Emit(
                    "Warning: remote staged deployment files could not be removed.",
                    "warning");
            }
        }
        catch (Exception)
        {
            progress.Emit(
                "Warning: remote staged deployment cleanup could not be completed.",
                "warning");
        }
    }

    private sealed record Transfer(
        string Name,
        string LocalPath,
        string StagedPath);

    [GeneratedRegex("^[0-9a-f]{64}$", RegexOptions.CultureInvariant)]
    private static partial Regex DigestPattern();
}
