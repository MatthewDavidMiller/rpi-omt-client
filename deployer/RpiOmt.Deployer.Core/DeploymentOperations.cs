using System.Formats.Tar;
using System.Diagnostics.CodeAnalysis;
using System.Security.Cryptography;
using System.Text;
using System.Text.RegularExpressions;

namespace RpiOmt.Deployer.Core;

public sealed partial class DeploymentOperations(
    ICommandRunner commandRunner,
    IRemoteClientFactory remoteClientFactory,
    IArtifactSnapshotProvider snapshotProvider) : IDeploymentOperations
{
    public const string Arm64CheckImage = "debian:bookworm-slim@sha256:4724b8cc51e33e398f0e2e15e18d5ec2851ff0c2280647e1310bc1642182655d";
    public const string BinfmtImage = "tonistiigi/binfmt@sha256:400a4873b838d1b89194d982c45e5fb3cda4593fbfd7e08a02e76b03b21166f0";
    private static readonly TimeSpan RemoteTimeout = TimeSpan.FromSeconds(60);
    private static readonly TimeSpan WifiTimeout = TimeSpan.FromSeconds(120);
    private static readonly TimeSpan InstallerTimeout = TimeSpan.FromMinutes(30);
    private static readonly TimeSpan EmulatorTimeout = TimeSpan.FromSeconds(120);
    private static readonly TimeSpan PrerequisiteTimeout = TimeSpan.FromMinutes(5);
    private static readonly TimeSpan BuildTimeout = TimeSpan.FromHours(1);
    private static readonly TimeSpan UploadTimeout = TimeSpan.FromMinutes(30);
    private SecretRedactor _redactor = new([]);
    private string _stage = "idle";
    private bool _cancellable = true;

    public event EventHandler<ProgressEventArgs>? Progress;

    public async Task DeployAsync(ActionSnapshot snapshot, CancellationToken cancellationToken)
    {
        var connection = snapshot.Connection;
        var options = snapshot.Options;
        _redactor = ConnectionRedactor(connection, snapshot.Wifi.Password);
        SetStage("validation", "Validating deployment inputs...");
        ValidateOrThrow(connection, options);
        ValidateProjectArtifacts(options);
        if (options.BuildImage)
        {
            await BuildImageAsync(options, cancellationToken).ConfigureAwait(false);
        }
        else
        {
            RequireRegularFile(options.TarballPath);
        }

        SetStage("connect", "Connecting to Raspberry Pi...");
        cancellationToken.ThrowIfCancellationRequested();
        await using var remote = remoteClientFactory.Create();
        await remote.ConnectAsync(connection, cancellationToken).ConfigureAwait(false);
        var architecture = Redact(await remote.RunAsync(
            "uname -m && hostname",
            string.Empty,
            null,
            RemoteTimeout,
            cancellationToken).ConfigureAwait(false));
        RequireSuccess(architecture);
        RequireAarch64(architecture);
        await PrepareRemoteDirectoryAsync(remote, connection, options, cancellationToken).ConfigureAwait(false);
        await UploadArtifactsAsync(remote, options, cancellationToken).ConfigureAwait(false);
        await RunInstallerAsync(remote, connection, options, cancellationToken).ConfigureAwait(false);
        SetStage("complete", "Deployment complete; use the installer URL shown above.");
    }

    [ExcludeFromCodeCoverage(Justification = "Exercises the privileged Docker/binfmt boundary and is covered by local integration tests.")]
    public async Task InstallPrerequisitesAsync(string projectRoot, CancellationToken cancellationToken)
    {
        _redactor = new SecretRedactor([]);
        SetStage("prerequisites", "Installing Docker ARM64 emulation support...");
        RequireExecutable("docker");
        var result = await commandRunner.RunAsync(
            ["docker", "run", "--privileged", "--rm", BinfmtImage, "--install", "arm64"],
            projectRoot,
            line => Emit(line),
            PrerequisiteTimeout,
            cancellationToken).ConfigureAwait(false);
        RequireSuccess(result);
        await VerifyEmulationAsync(projectRoot, cancellationToken).ConfigureAwait(false);
        Emit("Docker ARM64 emulation is ready.");
    }

    public async Task TestConnectionAsync(PiConnection connection, CancellationToken cancellationToken)
    {
        _redactor = ConnectionRedactor(connection);
        ValidateConnectionOrThrow(connection);
        SetStage("test-connection", "Testing SSH connection...");
        await using var remote = remoteClientFactory.Create();
        await remote.ConnectAsync(connection, cancellationToken).ConfigureAwait(false);
        var result = Redact(await remote.RunAsync(
            "uname -m && hostname",
            string.Empty,
            null,
            RemoteTimeout,
            cancellationToken).ConfigureAwait(false));
        RequireSuccess(result);
        RequireAarch64(result);
        Emit("SSH connection succeeded.");
    }

    public Task<string> FetchStatusAsync(PiConnection connection, string remoteDirectory, CancellationToken cancellationToken) =>
        RunRemoteTextAsync(connection, remoteDirectory, "docker compose ps", "Fetching container status...", cancellationToken);

    public Task<string> FetchLogsAsync(PiConnection connection, string remoteDirectory, CancellationToken cancellationToken) =>
        RunRemoteTextAsync(connection, remoteDirectory, "docker compose logs --tail=120", "Fetching recent logs...", cancellationToken);

    public Task<string> RestartServiceAsync(PiConnection connection, string remoteDirectory, CancellationToken cancellationToken) =>
        RunRemoteTextAsync(connection, remoteDirectory, "docker compose restart", "Restarting service...", cancellationToken);

    public async Task ApplyWifiAsync(PiConnection connection, WifiSettings settings, CancellationToken cancellationToken)
    {
        _redactor = ConnectionRedactor(connection, settings.Password);
        ValidateConnectionOrThrow(connection);
        var wifiError = InputValidation.WifiSsidError(settings.Ssid) ?? InputValidation.WifiPasswordError(settings.Password);
        if (wifiError is not null)
        {
            throw new DeploymentException(wifiError);
        }

        cancellationToken.ThrowIfCancellationRequested();
        SetStage(
            "wifi-mutation",
            "Applying Wi-Fi settings; cancellation is disabled until the remote NetworkManager change finishes.",
            false);
        Emit(settings.Connect
            ? "Scanning for Wi-Fi networks before connecting..."
            : "Saving Wi-Fi profile without connecting.");
        if (settings.Connect)
        {
            Emit("SSH may disconnect if the Raspberry Pi switches networks.");
        }

        await using var remote = remoteClientFactory.Create();
        await remote.ConnectAsync(connection, cancellationToken).ConfigureAwait(false);
        var marker = $"__OMT_WIFI_PASSWORD_FOLLOWS_{RandomNumberGenerator.GetHexString(24)}__";
        var script = BuildWifiScript(settings.Connect);
        var sudoPassword = SudoPassword(connection);
        var input = string.IsNullOrEmpty(sudoPassword)
            ? $"{marker}\n{settings.Password}\n"
            : $"{sudoPassword}\n{marker}\n{settings.Password}\n";
        var sudo = string.IsNullOrEmpty(sudoPassword) ? "sudo -n -v" : "sudo -S -p '' -v";
        var command = $"{sudo} && sudo -n sh -eu -c {Shell.Quote(script)} sh {Shell.Quote(settings.Ssid)} {(settings.Connect ? "yes" : "no")} {Shell.Quote(marker)}";
        var result = Redact(await remote.RunAsync(
            command,
            input,
            line => Emit(line),
            WifiTimeout,
            CancellationToken.None).ConfigureAwait(false));
        RequireSuccess(result);
        Emit(settings.Connect ? "Wi-Fi settings applied and connection requested." : "Wi-Fi settings saved.");
    }

    [ExcludeFromCodeCoverage(Justification = "Exercises Docker buildx and atomic archive publication through integration tests.")]
    private async Task BuildImageAsync(DeployOptions options, CancellationToken cancellationToken)
    {
        SetStage("build-preflight", "Checking local build prerequisites...");
        RequireExecutable("docker");
        await VerifyEmulationAsync(options.ProjectRoot, cancellationToken).ConfigureAwait(false);
        var version = await new VersionDetector(commandRunner).DetectAsync(options.ProjectRoot, null, cancellationToken).ConfigureAwait(false);
        SetStage("build", "Building ARM64 Docker image...");
        var stagedPath = Path.Combine(options.ProjectRoot, $".{options.TarballName}.{RandomNumberGenerator.GetHexString(16)}.tmp");
        try
        {
            var result = await commandRunner.RunAsync(
                [
                    "docker", "buildx", "build", "--platform", "linux/arm64", "--build-arg",
                    $"RPI_OMT_CLIENT_VERSION={version}", "--output", $"type=docker,dest={stagedPath}",
                    "-t", options.ImageName, ".",
                ],
                options.ProjectRoot,
                line => Emit(line),
                BuildTimeout,
                cancellationToken).ConfigureAwait(false);
            RequireSuccess(result);
            cancellationToken.ThrowIfCancellationRequested();
            VerifyTarArchive(stagedPath);
            await using (var flush = new FileStream(stagedPath, FileMode.Open, FileAccess.ReadWrite, FileShare.Read))
            {
                flush.Flush(flushToDisk: true);
            }

            File.Move(stagedPath, options.TarballPath, overwrite: true);
            Emit($"Published verified artifact: {options.TarballName}");
        }
        finally
        {
            if (File.Exists(stagedPath))
            {
                File.Delete(stagedPath);
            }
        }
    }

    [ExcludeFromCodeCoverage(Justification = "Exercises the external ARM64 container runtime.")]
    private async Task VerifyEmulationAsync(string projectRoot, CancellationToken cancellationToken)
    {
        SetStage("emulation-check", "Checking Docker ARM64 emulation...");
        var result = await commandRunner.RunAsync(
            ["docker", "run", "--rm", "--platform", "linux/arm64", "--entrypoint", "/bin/sh", Arm64CheckImage, "-c", "test \"$(uname -m)\" = \"aarch64\""],
            projectRoot,
            line => Emit(line),
            EmulatorTimeout,
            cancellationToken).ConfigureAwait(false);
        if (!result.IsSuccess)
        {
            throw new DeploymentException(
                "Docker ARM64 emulation is not ready. Use Install Prerequisites, ensure Docker Desktop's Linux engine is running, and retry.");
        }
    }

    private async Task PrepareRemoteDirectoryAsync(
        IRemoteClient remote,
        PiConnection connection,
        DeployOptions options,
        CancellationToken cancellationToken)
    {
        SetStage("remote-prepare", "Preparing the remote deployment directory...");
        var sudoPassword = SudoPassword(connection);
        var sudo = string.IsNullOrEmpty(sudoPassword) ? "sudo -n" : "sudo -S -p ''";
        var command = $"{sudo} install -d -m 755 -o \"$(id -u)\" -g \"$(id -g)\" {Shell.Quote(options.RemoteDirectory)}";
        var result = Redact(await remote.RunAsync(
            command,
            string.IsNullOrEmpty(sudoPassword) ? string.Empty : $"{sudoPassword}\n",
            null,
            RemoteTimeout,
            cancellationToken).ConfigureAwait(false));
        RequireSuccess(result);
    }

    private async Task UploadArtifactsAsync(IRemoteClient remote, DeployOptions options, CancellationToken cancellationToken)
    {
        var manifestNames = ArtifactSnapshots.LoadManifest(options.ManifestPath);
        if (!manifestNames.Contains(options.TarballName, StringComparer.Ordinal))
        {
            throw new DeploymentException($"Deployment artifact manifest does not include {options.TarballName}.");
        }

        var artifacts = manifestNames
            .Select(name => (Name: name, LocalPath: Path.Combine(options.ProjectRoot, name)))
            .Concat([
                (Name: "deploy-transaction.sh", LocalPath: options.TransactionScriptPath),
                (Name: "deploy-artifacts.txt", LocalPath: options.ManifestPath),
            ])
            .ToArray();
        SetStage("local-verify", "Hashing stable local deployment artifacts...");
        var snapshots = new Dictionary<string, ArtifactSnapshot>(StringComparer.Ordinal);
        foreach (var artifact in artifacts)
        {
            snapshots.Add(artifact.Name, await snapshotProvider.CaptureAsync(artifact.LocalPath, cancellationToken).ConfigureAwait(false));
        }

        var token = RandomNumberGenerator.GetHexString(24).ToLowerInvariant();
        var remoteDirectory = options.RemoteDirectory.TrimEnd('/');
        var transfers = artifacts.Select(artifact => new Transfer(
            artifact.Name,
            artifact.LocalPath,
            $"{remoteDirectory}/.{artifact.Name}.upload-{token}")).ToArray();
        SetStage("upload", "Uploading verified deployment artifacts...");
        try
        {
            foreach (var transfer in transfers)
            {
                cancellationToken.ThrowIfCancellationRequested();
                Emit($"Uploading {transfer.Name}...");
                var lastPercentage = -1;
                await remote.UploadAsync(
                    transfer.LocalPath,
                    transfer.StagedPath,
                    UploadTimeout,
                    (current, total) =>
                    {
                        cancellationToken.ThrowIfCancellationRequested();
                        var percentage = total == 0 ? 100 : checked((int)(current * 100 / total));
                        if (percentage == 100 || percentage >= lastPercentage + 5)
                        {
                            lastPercentage = percentage;
                            Emit($"Uploading {transfer.Name}: {percentage}%");
                        }
                    },
                    cancellationToken).ConfigureAwait(false);
                var snapshot = snapshots[transfer.Name];
                if (!await snapshot.IsUnchangedAsync(cancellationToken).ConfigureAwait(false))
                {
                    throw new DeploymentException($"Local deployment artifact changed during upload: {transfer.Name}.");
                }

                var checksum = Redact(await remote.RunAsync(
                    $"sha256sum -- {Shell.Quote(transfer.StagedPath)}",
                    string.Empty,
                    null,
                    RemoteTimeout,
                    cancellationToken).ConfigureAwait(false));
                RequireSuccess(checksum);
                var digest = checksum.StandardOutput.Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries).FirstOrDefault()?.ToLowerInvariant() ?? string.Empty;
                if (!DigestPattern().IsMatch(digest) || !string.Equals(digest, snapshot.Sha256, StringComparison.Ordinal))
                {
                    throw new DeploymentException($"Checksum mismatch after uploading {transfer.Name}.");
                }

                Emit($"Verified SHA-256 for {transfer.Name}.");
            }
        }
        catch
        {
            await CleanupUploadsAsync(remote, transfers).ConfigureAwait(false);
            throw;
        }

        SetStage("promotion", "Activating uploaded deployment files...");
        cancellationToken.ThrowIfCancellationRequested();
        var helper = transfers.Single(transfer => transfer.Name == "deploy-transaction.sh").StagedPath;
        var manifest = transfers.Single(transfer => transfer.Name == "deploy-artifacts.txt").StagedPath;
        var promotionCommand = $"bash {Shell.Quote(helper)} promote {Shell.Quote(remoteDirectory)} {Shell.Quote(token)} {Shell.Quote(manifest)}";
        try
        {
            var promotion = Redact(await remote.RunAsync(
                promotionCommand,
                string.Empty,
                null,
                RemoteTimeout,
                cancellationToken).ConfigureAwait(false));
            RequireSuccess(promotion);
        }
        catch
        {
            await CleanupUploadsAsync(remote, transfers).ConfigureAwait(false);
            throw;
        }
    }

    private async Task CleanupUploadsAsync(IRemoteClient remote, IEnumerable<Transfer> transfers)
    {
        try
        {
            var paths = string.Join(' ', transfers.Select(transfer => Shell.Quote(transfer.StagedPath)));
            var result = Redact(await remote.RunAsync(
                $"rm -f -- {paths}",
                string.Empty,
                null,
                RemoteTimeout,
                CancellationToken.None).ConfigureAwait(false));
            if (!result.IsSuccess)
            {
                Emit("Warning: remote staged deployment files could not be removed.", "warning");
            }
        }
        catch (Exception)
        {
            Emit("Warning: remote staged deployment cleanup could not be completed.", "warning");
        }
    }

    private async Task RunInstallerAsync(
        IRemoteClient remote,
        PiConnection connection,
        DeployOptions options,
        CancellationToken cancellationToken)
    {
        cancellationToken.ThrowIfCancellationRequested();
        var directory = options.RemoteDirectory.TrimEnd('/');
        var sudoPassword = SudoPassword(connection);
        var sudo = string.IsNullOrEmpty(sudoPassword) ? "sudo -n" : "sudo -S -p ''";
        var installCommand = $"printf 'n\\n' | {Shell.Quote($"{directory}/install.sh")}";
        var command = $"chmod +x {Shell.Quote($"{directory}/install.sh")} {Shell.Quote($"{directory}/uninstall.sh")} {Shell.Quote($"{directory}/host-debug.sh")} {Shell.Quote($"{directory}/host-reboot.sh")} && {sudo} sh -c {Shell.Quote(installCommand)}";
        SetStage(
            "installer",
            "Running the privileged remote installer; cancellation is disabled until it finishes.",
            false);
        var result = Redact(await remote.RunAsync(
            command,
            string.IsNullOrEmpty(sudoPassword) ? string.Empty : $"{sudoPassword}\n",
            line => Emit(line),
            InstallerTimeout,
            CancellationToken.None).ConfigureAwait(false));
        RequireSuccess(result);
    }

    private async Task<string> RunRemoteTextAsync(
        PiConnection connection,
        string remoteDirectory,
        string action,
        string message,
        CancellationToken cancellationToken)
    {
        _redactor = ConnectionRedactor(connection);
        ValidateConnectionOrThrow(connection);
        if (!InputValidation.IsValidRemoteDirectory(remoteDirectory))
        {
            throw new DeploymentException($"Remote install directory is not a normalized safe path: {remoteDirectory}");
        }

        SetStage("remote-read", message);
        await using var remote = remoteClientFactory.Create();
        await remote.ConnectAsync(connection, cancellationToken).ConfigureAwait(false);
        var result = Redact(await remote.RunAsync(
            $"cd {Shell.Quote(remoteDirectory)} && {action}",
            string.Empty,
            line => Emit(line),
            RemoteTimeout,
            cancellationToken).ConfigureAwait(false));
        RequireSuccess(result);
        return result.StandardOutput;
    }

    private static string BuildWifiScript(bool connect)
    {
        var scan = connect
            ? "nmcli dev wifi rescan ifname wlan0 || nmcli dev wifi rescan || true\n" +
              "if ! nmcli -t --escape no -f SSID dev wifi list | grep -Fx -- \"$ssid\" >/dev/null; then\n" +
              "  echo \"Wi-Fi SSID not found after scan: $ssid\" >&2\n  exit 10\nfi\n"
            : string.Empty;
        var activate = connect ? "nmcli connection up \"$ssid\"\n" : string.Empty;
        return "marker=$3\nfound_marker=no\n" +
            "while IFS= read -r line; do\n  if [ \"$line\" = \"$marker\" ]; then found_marker=yes; break; fi\ndone\n" +
            "if [ \"$found_marker\" != yes ]; then echo \"Wi-Fi password marker not found\" >&2; exit 11; fi\n" +
            "if ! IFS= read -r wifi_password; then echo \"Wi-Fi password not provided\" >&2; exit 11; fi\n" +
            "ssid=$1\nactivate=$2\n" + scan +
            "if nmcli -t --escape no -f NAME connection show | grep -Fx -- \"$ssid\" >/dev/null; then\n" +
            "  nmcli connection modify \"$ssid\" 802-11-wireless.ssid \"$ssid\" wifi-sec.key-mgmt wpa-psk wifi-sec.psk \"$wifi_password\" connection.autoconnect yes\n" +
            "else\n  nmcli connection add type wifi ifname wlan0 con-name \"$ssid\" ssid \"$ssid\"\n" +
            "  nmcli connection modify \"$ssid\" wifi-sec.key-mgmt wpa-psk wifi-sec.psk \"$wifi_password\" connection.autoconnect yes\nfi\n" + activate;
    }

    [ExcludeFromCodeCoverage(Justification = "Only receives Docker image archives and is covered by the local publish tier.")]
    private static void VerifyTarArchive(string path)
    {
        if (!ArtifactSnapshots.IsRegularFile(path) || new FileInfo(path).Length == 0)
        {
            throw new DeploymentException("Docker reported success but did not produce a non-empty regular ARM64 artifact.");
        }

        try
        {
            using var file = File.OpenRead(path);
            using var reader = new TarReader(file);
            if (reader.GetNextEntry() is null)
            {
                throw new DeploymentException("Docker produced an empty ARM64 image archive.");
            }
        }
        catch (InvalidDataException exception)
        {
            throw new DeploymentException($"Docker produced an invalid or incomplete ARM64 image archive: {exception.Message}");
        }
    }

    private static string SudoPassword(PiConnection connection) =>
        !string.IsNullOrEmpty(connection.SudoPassword)
            ? connection.SudoPassword
            : connection.AuthMethod == AuthMethod.Password ? connection.Password : string.Empty;

    private static SecretRedactor ConnectionRedactor(PiConnection connection, params string[] additional) =>
        new([connection.Password, connection.KeyPassphrase, connection.SudoPassword, .. additional]);

    private static void ValidateOrThrow(PiConnection connection, DeployOptions options)
    {
        var errors = InputValidation.ValidateConnection(connection)
            .Concat(InputValidation.ValidateOptions(options))
            .ToArray();
        if (errors.Length > 0)
        {
            throw new DeploymentException(string.Join(Environment.NewLine, errors));
        }
    }

    private static void ValidateConnectionOrThrow(PiConnection connection)
    {
        var errors = InputValidation.ValidateConnection(connection);
        if (errors.Count > 0)
        {
            throw new DeploymentException(string.Join(Environment.NewLine, errors));
        }
    }

    private static void ValidateProjectArtifacts(DeployOptions options)
    {
        RequireRegularFile(options.ManifestPath);
        RequireRegularFile(options.TransactionScriptPath);
        var names = ArtifactSnapshots.LoadManifest(options.ManifestPath);
        if (names.Count != 9)
        {
            throw new DeploymentException("Deployment artifact manifest must enumerate exactly nine capsule files.");
        }

        foreach (var name in names)
        {
            if (options.BuildImage && name == options.TarballName)
            {
                continue;
            }

            RequireRegularFile(Path.Combine(options.ProjectRoot, name));
        }
    }

    private static void RequireRegularFile(string path)
    {
        if (!ArtifactSnapshots.IsRegularFile(path))
        {
            throw new DeploymentException($"Required file is missing: {path}");
        }
    }

    [ExcludeFromCodeCoverage(Justification = "Executable discovery branches are exercised by the concrete process adapter tests.")]
    private static void RequireExecutable(string name)
    {
        if (!ProcessCommandRunner.IsExecutableAvailable(name))
        {
            throw new FileNotFoundException($"Required executable not found on PATH: {name}");
        }
    }

    private static void RequireAarch64(CommandResult result)
    {
        var architecture = result.StandardOutput.Split('\n', StringSplitOptions.RemoveEmptyEntries)
            .Select(line => line.Trim())
            .FirstOrDefault() ?? string.Empty;
        if (!string.Equals(architecture, "aarch64", StringComparison.Ordinal))
        {
            throw new DeploymentException(
                $"The remote host must be a 64-bit ARM Raspberry Pi (aarch64); detected {(architecture.Length == 0 ? "unrecognized output" : architecture)}.");
        }
    }

    private static void RequireSuccess(CommandResult result)
    {
        if (!result.IsSuccess)
        {
            throw new CommandException(result);
        }
    }

    private void SetStage(string stage, string message, bool cancellable = true)
    {
        _stage = stage;
        _cancellable = cancellable;
        Emit(message);
    }

    private void Emit(string message, string level = "info")
    {
        if (message.Length > 0)
        {
            Progress?.Invoke(this, new ProgressEventArgs(_redactor.Redact(message), level, _stage, _cancellable));
        }
    }

    private CommandResult Redact(CommandResult result) => _redactor.Redact(result);

    private sealed record Transfer(string Name, string LocalPath, string StagedPath);

    [GeneratedRegex("^[0-9a-f]{64}$", RegexOptions.CultureInvariant)]
    private static partial Regex DigestPattern();
}
