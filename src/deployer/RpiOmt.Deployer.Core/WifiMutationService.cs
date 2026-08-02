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
            "the remote wpa_supplicant change finishes.",
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
        string ssidHex = Convert.ToHexStringLower(System.Text.Encoding.UTF8.GetBytes(settings.Ssid));
        // InputValidation has already proved that a 64-character value is hex.
        string psk = settings.Password.Length == 64
            ? settings.Password.ToLowerInvariant()
            : Convert.ToHexStringLower(Rfc2898DeriveBytes.Pbkdf2(
                System.Text.Encoding.UTF8.GetBytes(settings.Password),
                System.Text.Encoding.UTF8.GetBytes(settings.Ssid),
                4096,
                HashAlgorithmName.SHA1,
                32));
        string sudoPassword = DeploymentGuards.SudoPassword(connection);
        string input = string.IsNullOrEmpty(sudoPassword)
            ? $"{marker}\n{psk}\n"
            : $"{sudoPassword}\n{marker}\n{psk}\n";
        string sudo = string.IsNullOrEmpty(sudoPassword)
            ? "sudo -n -v"
            : "sudo -S -p '' -v";
        string command =
            $"{sudo} && sudo -n sh -eu -c {Shell.Quote(script)} sh " +
            $"{Shell.Quote(ssidHex)} {(settings.Connect ? "yes" : "no")} " +
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
        string scan = connect ? "wpa_cli -i wlan0 scan >/dev/null || true\n" : string.Empty;
        string activate = connect
            ? "wpa_cli -i wlan0 select_network \"$network_id\" >/dev/null\n" +
              "wpa_cli -i wlan0 reassociate >/dev/null\n"
            : string.Empty;
        return "marker=$3\nfound_marker=no\n" +
            "while IFS= read -r line; do\n" +
            "  if [ \"$line\" = \"$marker\" ]; then found_marker=yes; break; fi\n" +
            "done\n" +
            "if [ \"$found_marker\" != yes ]; then " +
            "echo \"Wi-Fi password marker not found\" >&2; exit 11; fi\n" +
            "if ! IFS= read -r wifi_password; then " +
            "echo \"Wi-Fi password not provided\" >&2; exit 11; fi\n" +
            "ssid_hex=$1\nactivate=$2\n" +
            "command -v wpa_cli >/dev/null 2>&1 || { echo 'wpa_cli is unavailable' >&2; exit 12; }\n" +
            "wpa_cli -i wlan0 ping | grep -Fxq PONG || { echo 'wpa_supplicant is unavailable on wlan0' >&2; exit 12; }\n" +
            scan +
            "network_id=\n" +
            "for candidate in $(wpa_cli -i wlan0 list_networks | awk 'NR > 2 {print $1}'); do\n" +
            "  current=$(wpa_cli -i wlan0 get_network \"$candidate\" ssid 2>/dev/null || true)\n" +
            "  if [ \"$current\" = \"$ssid_hex\" ]; then network_id=$candidate; break; fi\n" +
            "done\n" +
            "if [ -z \"$network_id\" ]; then network_id=$(wpa_cli -i wlan0 add_network); fi\n" +
            "case \"$network_id\" in ''|*[!0-9]*) echo 'Unable to allocate Wi-Fi profile' >&2; exit 13;; esac\n" +
            "wpa_cli -i wlan0 set_network \"$network_id\" ssid \"$ssid_hex\" | grep -Fxq OK\n" +
            "wpa_cli -i wlan0 set_network \"$network_id\" key_mgmt WPA-PSK | grep -Fxq OK\n" +
            "wpa_cli -i wlan0 set_network \"$network_id\" psk \"$wifi_password\" | grep -Fxq OK\n" +
            "wpa_cli -i wlan0 enable_network \"$network_id\" | grep -Fxq OK\n" +
            "wpa_cli -i wlan0 save_config | grep -Fxq OK\n" + activate;
    }
}
