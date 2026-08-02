using System.Globalization;
using RpiOmt.Deployer.Core;

namespace RpiOmt.Deployer.App.ViewModels;

public sealed class ConnectionViewModel
{
    public string Host { get; set; } = string.Empty;
    public string Username { get; set; } = "admin";
    public string Port { get; set; } = "22";
    public string Password { get; set; } = string.Empty;
    public string KeyPath { get; set; } = string.Empty;
    public string KeyPassphrase { get; set; } = string.Empty;
    public string SudoPassword { get; set; } = string.Empty;
    public bool IsKeyAuthentication { get; set; }

    public PiConnection Capture()
    {
        if (Port.Length == 0 || !Port.All(character => character is >= '0' and <= '9'))
        {
            throw new FormatException();
        }

        var port = int.Parse(Port, NumberStyles.None, CultureInfo.InvariantCulture);
        return new PiConnection(
            Host.Trim(),
            Username.Trim(),
            port,
            IsKeyAuthentication ? AuthMethod.Key : AuthMethod.Password,
            IsKeyAuthentication ? string.Empty : Password,
            string.IsNullOrWhiteSpace(KeyPath) ? null : KeyPath,
            IsKeyAuthentication ? KeyPassphrase : string.Empty,
            SudoPassword);
    }
}

public sealed class DeploymentViewModel(string projectRoot)
{
    public string ProjectRoot { get; set; } = projectRoot;
    public string RemoteDirectory { get; set; } = "/opt/omt-client";
    public bool BuildImage { get; set; } = true;

    public DeployOptions Capture() =>
        new(ProjectRoot, RemoteDirectory.Trim(), BuildImage: BuildImage);
}

public sealed class WifiViewModel
{
    public string Ssid { get; set; } = string.Empty;
    public string Password { get; set; } = string.Empty;
    public bool Connect { get; set; } = true;

    public WifiSettings Capture() => new(Ssid, Password, Connect);
}

public sealed class OperationStateViewModel
{
    public ActionController Controller { get; } = new();
    public bool IsBusy { get; set; }
    public bool CanCancel { get; set; }
    public string Activity { get; set; } = "Idle · idle";
    public string ValidationMessage { get; set; } = string.Empty;
}
