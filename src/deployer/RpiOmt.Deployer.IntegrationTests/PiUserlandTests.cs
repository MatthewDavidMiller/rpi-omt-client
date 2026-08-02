using System.Diagnostics;
using System.Text;
using System.ComponentModel;
using RpiOmt.Deployer.Core;

namespace RpiOmt.Deployer.IntegrationTests;

public sealed class PiUserlandTests : IDisposable
{
    private readonly string _testRoot = Path.Combine(Path.GetTempPath(), $"rpi-omt-alpine-userland-{Guid.NewGuid():N}");

    public PiUserlandTests()
    {
        if (OperatingSystem.IsWindows())
        {
            throw new PlatformNotSupportedException("Alpine userland container tests run on Linux.");
        }

        Directory.CreateDirectory(Path.Combine(_testRoot, "bin"));
        WriteExecutable("sudo", """
            #!/bin/sh
            set -eu
            echo "sudo:$*" >> /tmp/pi-test/sudo.log
            if [ "${1:-}" = "-n" ] && [ "${2:-}" = "-v" ]; then exit 0; fi
            if [ "${1:-}" = "-n" ]; then shift; fi
            if [ "${1:-}" = "-S" ]; then
              shift
              if [ "${1:-}" = "-p" ]; then shift 2; fi
              if [ "${1:-}" = "-v" ]; then IFS= read -r password; echo "sudo_password:$password" >> /tmp/pi-test/sudo.log; exit 0; fi
            fi
            exec "$@"
            """);
        WriteExecutable("wpa_cli", """
            #!/bin/sh
            set -eu
            echo "wpa_cli:$*" >> /tmp/pi-test/wpa_cli.log
            [ "$1" = "-i" ] && [ "$2" = "wlan0" ] || exit 64
            case "$3" in
              ping) echo PONG ;;
              scan|set_network|enable_network|save_config|select_network|reassociate) echo OK ;;
              list_networks) printf 'network id / ssid / bssid / flags\n' ;;
              add_network) echo 7 ;;
              get_network) echo FAIL ;;
              *) exit 64 ;;
            esac
            """);
    }

    [Theory]
    [InlineData(true)]
    [InlineData(false)]
    public async Task WifiMutationRunsSafelyInPiUserland(bool connect)
    {
        var engine = await ResolveContainerEngineAsync(TestContext.Current.CancellationToken);
        var image = Environment.GetEnvironmentVariable("ALPINE_TEST_IMAGE") ??
            "alpine:3.23.5@sha256:fd791d74b68913cbb027c6546007b3f0d3bc45125f797758156952bc2d6daf40";
        if (!await ImageExistsAsync(engine, image, TestContext.Current.CancellationToken))
        {
            Assert.Skip($"Container image is not available locally: {image}");
        }

        var remote = new ContainerRemote(engine, image, _testRoot);
        var operations = new DeploymentOperations(
            new ProcessCommandRunner(),
            new SingleRemoteFactory(remote),
            new ArtifactSnapshotProvider());
        await operations.ApplyWifiAsync(
            new PiConnection("pi.local", "pi", Password: "ssh-password", SudoPassword: "sudo-password"),
            new WifiSettings(connect ? " Studio WiFi " : "Future Venue", "wifi-password", connect),
            TestContext.Current.CancellationToken);

        var sudoLog = await File.ReadAllTextAsync(Path.Combine(_testRoot, "sudo.log"), TestContext.Current.CancellationToken);
        var wpaLog = await File.ReadAllTextAsync(Path.Combine(_testRoot, "wpa_cli.log"), TestContext.Current.CancellationToken);
        Assert.Contains("sudo_password:sudo-password", sudoLog, StringComparison.Ordinal);
        Assert.Contains("set_network 7 psk ", wpaLog, StringComparison.Ordinal);
        Assert.DoesNotContain("wifi-password", wpaLog, StringComparison.Ordinal);
        Assert.DoesNotContain("sudo-password", wpaLog, StringComparison.Ordinal);
        if (connect)
        {
            Assert.Contains("wpa_cli:-i wlan0 scan", wpaLog, StringComparison.Ordinal);
            Assert.Contains("wpa_cli:-i wlan0 select_network 7", wpaLog, StringComparison.Ordinal);
        }
        else
        {
            Assert.DoesNotContain("wlan0 scan", wpaLog, StringComparison.Ordinal);
            Assert.DoesNotContain("select_network", wpaLog, StringComparison.Ordinal);
        }
    }

    public void Dispose() => Directory.Delete(_testRoot, recursive: true);

    private void WriteExecutable(string name, string text)
    {
        if (!OperatingSystem.IsLinux())
        {
            throw new PlatformNotSupportedException("Alpine userland container tests run on Linux.");
        }

        var path = Path.Combine(_testRoot, "bin", name);
        File.WriteAllText(path, text.Replace("            ", string.Empty, StringComparison.Ordinal), Encoding.UTF8);
        File.SetUnixFileMode(path, UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute |
            UnixFileMode.GroupRead | UnixFileMode.GroupExecute | UnixFileMode.OtherRead | UnixFileMode.OtherExecute);
    }

    private static async Task<string> ResolveContainerEngineAsync(CancellationToken cancellationToken)
    {
        var requested = Environment.GetEnvironmentVariable("CONTAINER_ENGINE");
        var candidates = string.IsNullOrWhiteSpace(requested) ? new[] { "docker", "podman" } : new[] { requested };
        foreach (var candidate in candidates)
        {
            if (Path.GetFileName(candidate) is not ("docker" or "podman"))
            {
                Assert.Fail("CONTAINER_ENGINE must be docker or podman.");
            }

            if (await RunAsync(candidate, ["info"], string.Empty, cancellationToken) is { ExitCode: 0 })
            {
                return candidate;
            }
        }

        Assert.Fail("Neither Docker nor Podman is available for Alpine userland integration tests.");
        return string.Empty;
    }

    private static async Task<bool> ImageExistsAsync(string engine, string image, CancellationToken cancellationToken) =>
        await RunAsync(engine, ["image", "inspect", image], string.Empty, cancellationToken) is { ExitCode: 0 };

    private static async Task<CommandResult> RunAsync(
        string executable,
        IEnumerable<string> arguments,
        string input,
        CancellationToken cancellationToken)
    {
        var startInfo = new ProcessStartInfo
        {
            FileName = executable,
            RedirectStandardInput = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
        };
        foreach (var argument in arguments)
        {
            startInfo.ArgumentList.Add(argument);
        }

        Process? process;
        try
        {
            process = Process.Start(startInfo);
        }
        catch (Win32Exception)
        {
            return new CommandResult(executable, 127, StandardError: "not found");
        }

        using (process)
        {
            if (process is null)
            {
                return new CommandResult(executable, 127, StandardError: "not found");
            }

            await process.StandardInput.WriteAsync(input.AsMemory(), cancellationToken);
            process.StandardInput.Close();
            var stdout = process.StandardOutput.ReadToEndAsync(cancellationToken);
            var stderr = process.StandardError.ReadToEndAsync(cancellationToken);
            await process.WaitForExitAsync(cancellationToken);
            return new CommandResult(executable, process.ExitCode, await stdout, await stderr);
        }
    }

    private sealed class SingleRemoteFactory(ContainerRemote remote) : IRemoteClientFactory
    {
        public IRemoteClient Create() => remote;
    }

    private sealed class ContainerRemote(string engine, string image, string testRoot) : IRemoteClient
    {
        public Task ConnectAsync(PiConnection connection, CancellationToken cancellationToken) => Task.CompletedTask;

        public async Task<CommandResult> RunAsync(
            string command,
            string input,
            Action<string>? onOutput,
            TimeSpan timeout,
            CancellationToken cancellationToken)
        {
            var volume = $"{testRoot}:/tmp/pi-test" + (Path.GetFileName(engine) == "podman" ? ":Z" : string.Empty);
            var arguments = new[]
            {
                "run", "--rm", "-i", "-v", volume,
                "--env", "PATH=/tmp/pi-test/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
                "--entrypoint", "sh", image, "-c", command,
            };
            using var timeoutSource = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
            timeoutSource.CancelAfter(timeout);
            var result = await PiUserlandTests.RunAsync(engine, arguments, input, timeoutSource.Token);
            foreach (var line in (result.StandardOutput + result.StandardError).Split('\n', StringSplitOptions.RemoveEmptyEntries))
            {
                onOutput?.Invoke(line);
            }

            return result with { Command = command };
        }

        public Task UploadAsync(string localPath, string remotePath, TimeSpan timeout, Action<long, long>? onProgress, CancellationToken cancellationToken) =>
            throw new InvalidOperationException("Uploads are not used by Wi-Fi integration tests.");

        public ValueTask DisposeAsync() => ValueTask.CompletedTask;
    }
}
