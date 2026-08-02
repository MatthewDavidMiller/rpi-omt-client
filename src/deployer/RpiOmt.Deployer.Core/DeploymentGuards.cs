using System.Diagnostics.CodeAnalysis;

namespace RpiOmt.Deployer.Core;

internal static class DeploymentGuards
{
    public static void ValidateOrThrow(PiConnection connection, DeployOptions options)
    {
        string[] errors = InputValidation.ValidateConnection(connection)
            .Concat(InputValidation.ValidateOptions(options))
            .ToArray();
        if (errors.Length > 0)
        {
            throw new DeploymentException(string.Join(Environment.NewLine, errors));
        }
    }

    public static void ValidateConnectionOrThrow(PiConnection connection)
    {
        IReadOnlyList<string> errors = InputValidation.ValidateConnection(connection);
        if (errors.Count > 0)
        {
            throw new DeploymentException(string.Join(Environment.NewLine, errors));
        }
    }

    public static string SudoPassword(PiConnection connection) =>
        !string.IsNullOrEmpty(connection.SudoPassword)
            ? connection.SudoPassword
            : connection.AuthMethod == AuthMethod.Password
                ? connection.Password
                : string.Empty;

    public static void RequireRegularFile(string path)
    {
        if (!ArtifactSnapshots.IsRegularFile(path))
        {
            throw new DeploymentException($"Required file is missing: {path}");
        }
    }

    [ExcludeFromCodeCoverage(
        Justification = "Executable discovery branches are covered by process-adapter tests.")]
    public static void RequireExecutable(string name)
    {
        if (!ProcessCommandRunner.IsExecutableAvailable(name))
        {
            throw new FileNotFoundException(
                $"Required executable not found on PATH: {name}");
        }
    }

    public static void RequireAlpinePi5(CommandResult result)
    {
        string[] platform = result.StandardOutput
            .Split('\n', StringSplitOptions.RemoveEmptyEntries)
            .Select(line => line.Trim())
            .ToArray();
        string architecture = platform.ElementAtOrDefault(0) ?? string.Empty;
        string os = platform.ElementAtOrDefault(1) ?? string.Empty;
        string version = platform.ElementAtOrDefault(2) ?? string.Empty;
        string model = platform.ElementAtOrDefault(3) ?? string.Empty;
        if (!string.Equals(architecture, "aarch64", StringComparison.Ordinal) ||
            !string.Equals(os, "alpine", StringComparison.Ordinal) ||
            !version.StartsWith("3.23.", StringComparison.Ordinal) ||
            !model.StartsWith("Raspberry Pi 5", StringComparison.Ordinal))
        {
            throw new DeploymentException(
                "The remote host must be a Raspberry Pi 5 running Alpine Linux 3.23 " +
                $"aarch64; detected {(platform.Length == 0 ? "unrecognized output" : string.Join(", ", platform.Take(4)))}.");
        }
    }

    public static void RequireSuccess(CommandResult result)
    {
        if (!result.IsSuccess)
        {
            throw new CommandException(result);
        }
    }
}
