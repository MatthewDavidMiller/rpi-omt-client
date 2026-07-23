using System.Collections.ObjectModel;
using System.Text;
using System.Text.RegularExpressions;

namespace RpiOmt.Deployer.Core;

public enum AuthMethod
{
    Password,
    Key,
}

public enum OperationState
{
    Idle,
    Running,
    Cancelling,
    Succeeded,
    Failed,
    Cancelled,
}

public sealed record PiConnection(
    string Host,
    string Username,
    int Port = 22,
    AuthMethod AuthMethod = AuthMethod.Password,
    string Password = "",
    string? KeyPath = null,
    string KeyPassphrase = "",
    string SudoPassword = "");

public sealed record DeployOptions(
    string ProjectRoot,
    string RemoteDirectory = "/opt/omt-client",
    string ImageName = "omt-client",
    string TarballName = "omt-client-arm64.tar.gz",
    bool BuildImage = true)
{
    public string TarballPath => Path.Combine(ProjectRoot, TarballName);

    public string ManifestPath => Path.Combine(ProjectRoot, "deploy-artifacts.txt");

    public string TransactionScriptPath => Path.Combine(ProjectRoot, "deploy-transaction.sh");
}

public sealed record WifiSettings(string Ssid, string Password, bool Connect = true);

public sealed record CommandResult(string Command, int ExitCode, string StandardOutput = "", string StandardError = "")
{
    public bool IsSuccess => ExitCode == 0;
}

public sealed class ProgressEventArgs(
    string message,
    string level = "info",
    string stage = "",
    bool cancellable = true) : EventArgs
{
    public string Message { get; } = message;

    public string Level { get; } = level;

    public string Stage { get; } = stage;

    public bool Cancellable { get; } = cancellable;
}

public sealed record ActionSnapshot(PiConnection Connection, DeployOptions Options, WifiSettings Wifi);

public static partial class InputValidation
{
    private static readonly Regex RemoteDirectoryRegex = RemoteDirectoryPattern();
    private static readonly Regex HostRegex = HostPattern();
    private static readonly Regex UsernameRegex = UsernamePattern();
    private static readonly Regex HexPskRegex = HexPskPattern();
    private static readonly Regex PrintablePskRegex = PrintablePskPattern();

    public static bool ContainsControlCharacter(string value) =>
        value.Any(character => character <= '\u001f' || character == '\u007f');

    public static bool IsValidHost(string value)
    {
        if (string.IsNullOrWhiteSpace(value) || value.Length > 253 || ContainsControlCharacter(value))
        {
            return false;
        }

        return HostRegex.IsMatch(value) && value.Split('.').All(label => label.Length is > 0 and <= 63);
    }

    public static bool IsValidRemoteDirectory(string value)
    {
        if (string.IsNullOrEmpty(value) || value is "/" || !RemoteDirectoryRegex.IsMatch(value) ||
            value.EndsWith('/') || value.Contains("//", StringComparison.Ordinal))
        {
            return false;
        }

        return value.Split('/').Skip(1).All(component => component is not ("" or "." or ".."));
    }

    public static string? WifiSsidError(string value)
    {
        if (string.IsNullOrEmpty(value))
        {
            return "Wi-Fi SSID is required.";
        }

        if (ContainsControlCharacter(value))
        {
            return "Wi-Fi SSID must not contain control characters.";
        }

        return Encoding.UTF8.GetByteCount(value) <= 32
            ? null
            : "Wi-Fi SSID must be 32 UTF-8 bytes or fewer.";
    }

    public static string? WifiPasswordError(string value)
    {
        if (ContainsControlCharacter(value))
        {
            return "Wi-Fi password must not contain control characters.";
        }

        return HexPskRegex.IsMatch(value) || PrintablePskRegex.IsMatch(value)
            ? null
            : "Wi-Fi password must be 8 to 63 printable ASCII characters or a 64-digit hex PSK.";
    }

    public static IReadOnlyList<string> ValidateConnection(PiConnection connection)
    {
        var errors = new List<string>();
        if (!IsValidHost(connection.Host))
        {
            errors.Add("Pi host must be a valid IPv4 address or DNS host name.");
        }

        if (!UsernameRegex.IsMatch(connection.Username))
        {
            errors.Add("SSH username must use only letters, digits, dots, underscores, or hyphens.");
        }

        if (connection.Port is < 1 or > 65535)
        {
            errors.Add("SSH port must be between 1 and 65535.");
        }

        if (connection.AuthMethod == AuthMethod.Password && string.IsNullOrEmpty(connection.Password))
        {
            errors.Add("SSH password is required for password auth.");
        }

        if (connection.AuthMethod == AuthMethod.Key &&
            (string.IsNullOrEmpty(connection.KeyPath) || !File.Exists(connection.KeyPath)))
        {
            errors.Add("SSH key file does not exist.");
        }

        AddControlError(errors, "SSH password", connection.Password);
        AddControlError(errors, "Key passphrase", connection.KeyPassphrase);
        AddControlError(errors, "Sudo password", connection.SudoPassword);
        return new ReadOnlyCollection<string>(errors);
    }

    public static IReadOnlyList<string> ValidateOptions(DeployOptions options, bool requireProject = true)
    {
        var errors = new List<string>();
        if (requireProject && !Directory.Exists(options.ProjectRoot))
        {
            errors.Add("Project root does not exist.");
        }

        if (!IsValidRemoteDirectory(options.RemoteDirectory))
        {
            errors.Add("Remote install directory must be a normalized absolute path using only letters, digits, dots, underscores, hyphens, and slashes.");
        }

        return new ReadOnlyCollection<string>(errors);
    }

    private static void AddControlError(List<string> errors, string label, string value)
    {
        if (!string.IsNullOrEmpty(value) && ContainsControlCharacter(value))
        {
            errors.Add($"{label} must not contain control characters.");
        }
    }

    [GeneratedRegex("^/[A-Za-z0-9._/-]+$", RegexOptions.CultureInvariant)]
    private static partial Regex RemoteDirectoryPattern();

    [GeneratedRegex("^(?=.{1,253}$)(?:[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?)(?:\\.(?:[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?))*$", RegexOptions.CultureInvariant)]
    private static partial Regex HostPattern();

    [GeneratedRegex("^[A-Za-z0-9._-]+$", RegexOptions.CultureInvariant)]
    private static partial Regex UsernamePattern();

    [GeneratedRegex("^[0-9A-Fa-f]{64}$", RegexOptions.CultureInvariant)]
    private static partial Regex HexPskPattern();

    [GeneratedRegex("^[ -~]{8,63}$", RegexOptions.CultureInvariant)]
    private static partial Regex PrintablePskPattern();
}
