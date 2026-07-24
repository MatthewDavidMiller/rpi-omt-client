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

    public static void RequireAarch64(CommandResult result)
    {
        string architecture = result.StandardOutput
            .Split('\n', StringSplitOptions.RemoveEmptyEntries)
            .Select(line => line.Trim())
            .FirstOrDefault() ?? string.Empty;
        if (!string.Equals(architecture, "aarch64", StringComparison.Ordinal))
        {
            throw new DeploymentException(
                "The remote host must be a 64-bit ARM Raspberry Pi (aarch64); detected " +
                $"{(architecture.Length == 0 ? "unrecognized output" : architecture)}.");
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
