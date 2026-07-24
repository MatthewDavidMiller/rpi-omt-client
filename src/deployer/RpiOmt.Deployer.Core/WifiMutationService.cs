using System.Security.Cryptography;

namespace RpiOmt.Deployer.Core;

internal sealed class WifiMutationService(
    IRemoteClientFactory remoteClientFactory,
    ProgressRedactionService progress)
{
    private static readonly TimeSpan WifiTimeout = TimeSpan.FromSeconds(120);

    public async Task ApplyAsync(
        PiConnection connection,
        WifiSettings settings,
        CancellationToken cancellationToken)
    {
        DeploymentGuards.ValidateConnectionOrThrow(connection);
        string? wifiError =
            InputValidation.WifiSsidError(settings.Ssid) ??
            InputValidation.WifiPasswordError(settings.Password);
        if (wifiError is not null)
        {
            throw new DeploymentException(wifiError);
        }

        cancellationToken.ThrowIfCancellationRequested();
        progress.SetStage(
            "wifi-mutation",
            "Applying Wi-Fi settings; cancellation is disabled until " +
            "the remote NetworkManager change finishes.",
            false);
        progress.Emit(settings.Connect
            ? "Scanning for Wi-Fi networks before connecting..."
            : "Saving Wi-Fi profile without connecting.");
        if (settings.Connect)
        {
            progress.Emit(
                "SSH may disconnect if the Raspberry Pi switches networks.");
        }

        await using IRemoteClient remote = remoteClientFactory.Create();
        await remote.ConnectAsync(connection, cancellationToken).ConfigureAwait(false);
        string marker =
            $"__OMT_WIFI_PASSWORD_FOLLOWS_{RandomNumberGenerator.GetHexString(24)}__";
        string script = BuildScript(settings.Connect);
        string sudoPassword = DeploymentGuards.SudoPassword(connection);
        string input = string.IsNullOrEmpty(sudoPassword)
            ? $"{marker}\n{settings.Password}\n"
            : $"{sudoPassword}\n{marker}\n{settings.Password}\n";
        string sudo = string.IsNullOrEmpty(sudoPassword)
            ? "sudo -n -v"
            : "sudo -S -p '' -v";
        string command =
            $"{sudo} && sudo -n sh -eu -c {Shell.Quote(script)} sh " +
            $"{Shell.Quote(settings.Ssid)} {(settings.Connect ? "yes" : "no")} " +
            Shell.Quote(marker);
        CommandResult result = progress.Redact(await remote.RunAsync(
            command,
            input,
            line => progress.Emit(line),
            WifiTimeout,
            CancellationToken.None).ConfigureAwait(false));
        DeploymentGuards.RequireSuccess(result);
        progress.Emit(settings.Connect
            ? "Wi-Fi settings applied and connection requested."
            : "Wi-Fi settings saved.");
    }

    private static string BuildScript(bool connect)
    {
        string scan = connect
            ? "nmcli dev wifi rescan ifname wlan0 || nmcli dev wifi rescan || true\n" +
              "if ! nmcli -t --escape no -f SSID dev wifi list | " +
              "grep -Fx -- \"$ssid\" >/dev/null; then\n" +
              "  echo \"Wi-Fi SSID not found after scan: $ssid\" >&2\n" +
              "  exit 10\nfi\n"
            : string.Empty;
        string activate = connect
            ? "nmcli connection up \"$ssid\"\n"
            : string.Empty;
        return "marker=$3\nfound_marker=no\n" +
            "while IFS= read -r line; do\n" +
            "  if [ \"$line\" = \"$marker\" ]; then found_marker=yes; break; fi\n" +
            "done\n" +
            "if [ \"$found_marker\" != yes ]; then " +
            "echo \"Wi-Fi password marker not found\" >&2; exit 11; fi\n" +
            "if ! IFS= read -r wifi_password; then " +
            "echo \"Wi-Fi password not provided\" >&2; exit 11; fi\n" +
            "ssid=$1\nactivate=$2\n" + scan +
            "if nmcli -t --escape no -f NAME connection show | " +
            "grep -Fx -- \"$ssid\" >/dev/null; then\n" +
            "  nmcli connection modify \"$ssid\" " +
            "802-11-wireless.ssid \"$ssid\" wifi-sec.key-mgmt wpa-psk " +
            "wifi-sec.psk \"$wifi_password\" connection.autoconnect yes\n" +
            "else\n" +
            "  nmcli connection add type wifi ifname wlan0 " +
            "con-name \"$ssid\" ssid \"$ssid\"\n" +
            "  nmcli connection modify \"$ssid\" wifi-sec.key-mgmt wpa-psk " +
            "wifi-sec.psk \"$wifi_password\" connection.autoconnect yes\n" +
            "fi\n" + activate;
    }
}
