namespace RpiOmt.Deployer.Core;

public sealed class VersionDetector(ICommandRunner commandRunner)
{
    private static readonly TimeSpan GitTimeout = TimeSpan.FromSeconds(10);

    public async Task<string> DetectAsync(
        string projectRoot,
        IReadOnlyDictionary<string, string?>? environment,
        CancellationToken cancellationToken)
    {
        environment ??= Environment.GetEnvironmentVariables()
            .Cast<System.Collections.DictionaryEntry>()
            .ToDictionary(entry => (string)entry.Key, entry => entry.Value?.ToString(), StringComparer.Ordinal);
        if (environment.TryGetValue("RPI_OMT_CLIENT_VERSION", out var explicitVersion) &&
            !string.IsNullOrWhiteSpace(explicitVersion))
        {
            return explicitVersion.Trim();
        }

        try
        {
            var inside = await commandRunner.RunAsync(
                ["git", "-C", projectRoot, "rev-parse", "--is-inside-work-tree"],
                projectRoot,
                null,
                GitTimeout,
                cancellationToken).ConfigureAwait(false);
            if (inside.IsSuccess)
            {
                var tag = await commandRunner.RunAsync(
                    ["git", "-C", projectRoot, "describe", "--tags", "--exact-match"],
                    projectRoot,
                    null,
                    GitTimeout,
                    cancellationToken).ConfigureAwait(false);
                var value = tag.StandardOutput.Trim();
                if (tag.IsSuccess && value.Length > 0)
                {
                    return value;
                }
            }
        }
        catch (Exception exception) when (exception is IOException or TimeoutException)
        {
            // A source archive need not contain Git metadata.
        }

        var directoryName = new DirectoryInfo(Path.GetFullPath(projectRoot)).Name;
        var match = Shell.SourceDirectoryVersionPattern().Match(directoryName);
        return match.Success ? match.Groups[1].Value : "unknown";
    }
}
