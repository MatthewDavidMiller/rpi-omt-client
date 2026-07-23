using System.Diagnostics;
using System.Text;
using System.ComponentModel;
using RpiOmt.Deployer.Core;

namespace RpiOmt.Deployer.IntegrationTests;

public sealed class PiUserlandTests : IDisposable
{
    private readonly string _testRoot = Path.Combine(Path.GetTempPath(), $"rpi-omt-pi-userland-{Guid.NewGuid():N}");

    public PiUserlandTests()
    {
        if (OperatingSystem.IsWindows())
        {
            throw new PlatformNotSupportedException("Pi userland container tests run on Linux.");
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
        WriteExecutable("nmcli", """
            #!/bin/sh
            set -eu
            echo "nmcli:$*" >> /tmp/pi-test/nmcli.log
            if [ "$1" = "dev" ] && [ "$2" = "wifi" ] && [ "$3" = "rescan" ]; then exit 0; fi
            if [ "$1" = "-t" ] && [ "$2" = "--escape" ] && [ "$3" = "no" ] && [ "$4" = "-f" ]; then
              if [ "$5" = "SSID" ]; then printf '%s\n' " Studio WiFi " "Studio WiFi"; exit 0; fi
              if [ "$5" = "NAME" ]; then exit 0; fi
            fi
            if [ "$1" = "connection" ] && [ "$2" = "add" ]; then echo "ssid:${10}" >> /tmp/pi-test/nmcli.log; exit 0; fi
            if [ "$1" = "connection" ] && [ "$2" = "modify" ]; then
              previous=""
              for argument in "$@"; do
                if [ "$previous" = "wifi-sec.psk" ]; then echo "wifi_password:$argument" >> /tmp/pi-test/nmcli.log; exit 0; fi
                previous="$argument"
              done
            fi
            if [ "$1" = "connection" ] && [ "$2" = "up" ]; then echo "up:$3" >> /tmp/pi-test/nmcli.log; exit 0; fi
            echo "unexpected nmcli arguments: $*" >&2
            exit 64
            """);
    }

    [Theory]
    [InlineData(true)]
    [InlineData(false)]
    public async Task WifiMutationRunsSafelyInPiUserland(bool connect)
    {
        var engine = await ResolveContainerEngineAsync(TestContext.Current.CancellationToken);
        var image = Environment.GetEnvironmentVariable("PI_OS_TEST_IMAGE") ?? "debian:bookworm-slim";
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
        var nmcliLog = await File.ReadAllTextAsync(Path.Combine(_testRoot, "nmcli.log"), TestContext.Current.CancellationToken);
        Assert.Contains("sudo_password:sudo-password", sudoLog, StringComparison.Ordinal);
        Assert.Contains("wifi_password:wifi-password", nmcliLog, StringComparison.Ordinal);
        Assert.DoesNotContain("wifi_password:sudo-password", nmcliLog, StringComparison.Ordinal);
        if (connect)
        {
            Assert.Contains("nmcli:dev wifi rescan ifname wlan0", nmcliLog, StringComparison.Ordinal);
            Assert.Contains("up: Studio WiFi ", nmcliLog, StringComparison.Ordinal);
        }
        else
        {
            Assert.DoesNotContain("wifi rescan", nmcliLog, StringComparison.Ordinal);
            Assert.DoesNotContain("connection up", nmcliLog, StringComparison.Ordinal);
        }
    }

    public void Dispose() => Directory.Delete(_testRoot, recursive: true);

    private void WriteExecutable(string name, string text)
    {
        if (!OperatingSystem.IsLinux())
        {
            throw new PlatformNotSupportedException("Pi userland container tests run on Linux.");
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

        Assert.Fail("Neither Docker nor Podman is available for Pi userland integration tests.");
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
