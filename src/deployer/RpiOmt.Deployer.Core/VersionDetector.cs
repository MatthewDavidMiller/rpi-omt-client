using System.Text.RegularExpressions;

namespace RpiOmt.Deployer.Core;

public sealed partial class VersionDetector(ICommandRunner commandRunner)
{
    private static readonly TimeSpan GitTimeout = TimeSpan.FromSeconds(10);
    private const long MaximumProjectMetadataBytes = 64 * 1024;

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

        var projectVersion = ReadProjectVersion(projectRoot);
        if (projectVersion is not null)
        {
            return projectVersion;
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

    private static string? ReadProjectVersion(string projectRoot)
    {
        var projectFile = Path.Combine(Path.GetFullPath(projectRoot), "pyproject.toml");

        try
        {
            var projectFileInfo = new FileInfo(projectFile);
            if (!projectFileInfo.Exists || projectFileInfo.Length > MaximumProjectMetadataBytes)
            {
                return null;
            }

            var inProjectSection = false;
            foreach (var line in File.ReadLines(projectFile))
            {
                var trimmedLine = line.Trim();
                if (trimmedLine.StartsWith('['))
                {
                    inProjectSection = trimmedLine.Equals("[project]", StringComparison.Ordinal);
                    continue;
                }

                if (!inProjectSection)
                {
                    continue;
                }

                var match = ProjectVersionPattern().Match(trimmedLine);
                if (!match.Success)
                {
                    continue;
                }

                var version = match.Groups[1].Success
                    ? match.Groups[1].Value
                    : match.Groups[2].Value;
                if (!Shell.VersionPattern().IsMatch(version))
                {
                    return null;
                }

                return version.StartsWith('v') ? version : $"v{version}";
            }
        }
        catch (Exception exception) when (exception is IOException or UnauthorizedAccessException)
        {
            return null;
        }

        return null;
    }

    [GeneratedRegex(
        "^version\\s*=\\s*(?:\"([^\"]+)\"|'([^']+)')\\s*(?:#.*)?$",
        RegexOptions.CultureInvariant)]
    private static partial Regex ProjectVersionPattern();
}
