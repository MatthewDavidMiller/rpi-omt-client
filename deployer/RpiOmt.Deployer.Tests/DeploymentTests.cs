using System.Security.Cryptography;
using RpiOmt.Deployer.Core;

namespace RpiOmt.Deployer.Tests;

public sealed class VersionAndKnownHostTests
{
    [Fact]
    public async Task VersionDetectionUsesOverrideExactTagDirectoryAndUnknown()
    {
        var runner = new ScriptedRunner();
        var detector = new VersionDetector(runner);
        Assert.Equal("v9.1", await detector.DetectAsync("/src/project", new Dictionary<string, string?> { ["RPI_OMT_CLIENT_VERSION"] = " v9.1 " }, CancellationToken.None));
        runner.Results.Enqueue(new CommandResult("git", 0, "true\n"));
        runner.Results.Enqueue(new CommandResult("git", 0, "v1.2.3\n"));
        Assert.Equal("v1.2.3", await detector.DetectAsync("/src/project", new Dictionary<string, string?>(), CancellationToken.None));
        runner.Results.Enqueue(new CommandResult("git", 1));
        Assert.Equal("v0.2", await detector.DetectAsync("/tmp/rpi-omt-client-v0.2", new Dictionary<string, string?>(), CancellationToken.None));
        runner.Results.Enqueue(new CommandResult("git", 1));
        Assert.Equal("unknown", await detector.DetectAsync("/tmp/rpi-omt-client-main", new Dictionary<string, string?>(), CancellationToken.None));
    }

    [Fact]
    public async Task KnownHostsLookupUsesOpenSshOutputAndRejectsUnknown()
    {
        var knownHosts = Path.GetTempFileName();
        try
        {
            var key = new byte[] { 1, 2, 3 };
            var runner = new ScriptedRunner();
            runner.Results.Enqueue(new CommandResult("ssh-keygen", 0, $"# Host found\nhashed ssh-ed25519 {Convert.ToBase64String(key)}\n"));
            var keys = await new KnownHostsVerifier(runner).LoadAsync("pi.local", 2222, knownHosts, CancellationToken.None);
            Assert.True(KnownHostsVerifier.Matches(keys, "ssh-ed25519", key));
            Assert.False(KnownHostsVerifier.Matches(keys, "ssh-rsa", key));
            Assert.Contains("[pi.local]:2222", runner.Calls.Single(), StringComparison.Ordinal);

            runner.Results.Enqueue(new CommandResult("ssh-keygen", 1));
            await Assert.ThrowsAsync<DeploymentException>(() => new KnownHostsVerifier(runner).LoadAsync("new.local", 22, knownHosts, CancellationToken.None));
            runner.Results.Enqueue(new CommandResult("ssh-keygen", 2, StandardError: "bad file"));
            await Assert.ThrowsAsync<CommandException>(() => new KnownHostsVerifier(runner).LoadAsync("bad.local", 22, knownHosts, CancellationToken.None));
        }
        finally
        {
            File.Delete(knownHosts);
        }
    }

    [Fact]
    public async Task KnownHostsRejectsMissingAndMalformedFilesAndVersionHandlesRunnerFailure()
    {
        var runner = new ScriptedRunner();
        await Assert.ThrowsAsync<DeploymentException>(() => new KnownHostsVerifier(runner).LoadAsync(
            "pi.local", 22, "/definitely/missing/known_hosts", TestContext.Current.CancellationToken));

        var knownHosts = Path.GetTempFileName();
        try
        {
            runner.Results.Enqueue(new CommandResult("ssh-keygen", 0, "comment-only\ninvalid ssh-ed25519 !!!\n"));
            await Assert.ThrowsAsync<DeploymentException>(() => new KnownHostsVerifier(runner).LoadAsync(
                "pi.local", 22, knownHosts, TestContext.Current.CancellationToken));
            Assert.Contains("ssh-keygen -F pi.local", runner.Calls[^1], StringComparison.Ordinal);
        }
        finally
        {
            File.Delete(knownHosts);
        }

        runner.Exception = new IOException("git unavailable");
        var version = await new VersionDetector(runner).DetectAsync(
            "/tmp/rpi-omt-client-2.3.4",
            new Dictionary<string, string?>(),
            TestContext.Current.CancellationToken);
        Assert.Equal("2.3.4", version);

        var archiveWithoutVersion = await new VersionDetector(new ScriptedRunner()).DetectAsync(
            "/tmp/rpi-omt-client-main",
            new Dictionary<string, string?>(),
            TestContext.Current.CancellationToken);
        Assert.Equal("unknown", archiveWithoutVersion);

        var originalPath = Environment.GetEnvironmentVariable("PATH");
        var existingKnownHosts = Path.GetTempFileName();
        try
        {
            Environment.SetEnvironmentVariable("PATH", string.Empty);
            await Assert.ThrowsAsync<FileNotFoundException>(() => new KnownHostsVerifier(new ScriptedRunner()).LoadAsync(
                "pi.local", 22, existingKnownHosts, TestContext.Current.CancellationToken));
        }
        finally
        {
            Environment.SetEnvironmentVariable("PATH", originalPath);
            File.Delete(existingKnownHosts);
        }
    }
}

public sealed class DeploymentOperationTests : IDisposable
{
    private readonly string _project = Path.Combine(Path.GetTempPath(), $"rpi-omt-deploy-{Guid.NewGuid():N}");

    public DeploymentOperationTests()
    {
        Directory.CreateDirectory(_project);
        File.WriteAllText(Path.Combine(_project, "deploy-artifacts.txt"), Manifest);
        foreach (var name in new[] { "omt-client-arm64.tar.gz", "docker-compose.yml", "install.sh", "uninstall.sh", "host-debug.sh", "host-reboot.sh", "LICENSE", "THIRD_PARTY_NOTICES.txt", "THIRD_PARTY_SOURCE.md", "deploy-transaction.sh" })
        {
            File.WriteAllText(Path.Combine(_project, name), name);
        }
    }

    [Fact]
    public async Task DeployChecksArchitectureBeforeWritesAndVerifiesEveryUpload()
    {
        var remote = new FakeRemote();
        var operations = Create(remote);
        var progress = new List<ProgressEventArgs>();
        operations.Progress += (_, value) => progress.Add(value);
        await operations.DeployAsync(Snapshot(), CancellationToken.None);
        Assert.StartsWith("uname -m", remote.Commands[0].Command, StringComparison.Ordinal);
        Assert.Equal(11, remote.Uploads.Count);
        Assert.Equal(11, remote.Commands.Count(call => call.Command.StartsWith("sha256sum", StringComparison.Ordinal)));
        Assert.Contains(remote.Commands, call => call.Command.Contains(" deploy ", StringComparison.Ordinal) || call.Command.Contains(" promote ", StringComparison.Ordinal));
        var installer = remote.Commands.Single(call => call.Command.Contains("printf", StringComparison.Ordinal));
        Assert.False(installer.TokenCanBeCancelled);
        Assert.Contains(progress, value => value.Stage == "installer" && !value.Cancellable);
        Assert.DoesNotContain("sudo-secret", string.Join('\n', progress.Select(value => value.Message)), StringComparison.Ordinal);
    }

    [Fact]
    public async Task WrongArchitectureFailsBeforeRemoteWrites()
    {
        var remote = new FakeRemote { Architecture = "x86_64" };
        var exception = await Assert.ThrowsAsync<DeploymentException>(() => Create(remote).DeployAsync(Snapshot(), CancellationToken.None));
        Assert.Contains("aarch64", exception.Message, StringComparison.Ordinal);
        Assert.Empty(remote.Uploads);
        Assert.Single(remote.Commands);
    }

    [Fact]
    public async Task ChecksumFailureCleansStagedUploadsAndKeepsInstallerUnrun()
    {
        var remote = new FakeRemote { WrongChecksum = true };
        await Assert.ThrowsAsync<DeploymentException>(() => Create(remote).DeployAsync(Snapshot(), CancellationToken.None));
        Assert.Contains(remote.Commands, call => call.Command.StartsWith("rm -f --", StringComparison.Ordinal));
        Assert.DoesNotContain(remote.Commands, call => call.Command.Contains("printf", StringComparison.Ordinal));
    }

    [Fact]
    public async Task WifiKeepsSshSudoAndPskSeparateAndCannotBeCancelledMidMutation()
    {
        var remote = new FakeRemote();
        var operations = Create(remote);
        var progress = new List<ProgressEventArgs>();
        operations.Progress += (_, value) => progress.Add(value);
        var connection = new PiConnection("pi.local", "pi", Password: "ssh-secret", SudoPassword: "sudo-secret");
        await operations.ApplyWifiAsync(connection, new WifiSettings("Studio WiFi", "wifi-secret"), CancellationToken.None);
        var call = Assert.Single(remote.Commands);
        Assert.DoesNotContain("wifi-secret", call.Command, StringComparison.Ordinal);
        Assert.DoesNotContain("sudo-secret", call.Command, StringComparison.Ordinal);
        Assert.StartsWith("sudo-secret\n", call.Input, StringComparison.Ordinal);
        Assert.EndsWith("wifi-secret\n", call.Input, StringComparison.Ordinal);
        Assert.False(call.TokenCanBeCancelled);
        Assert.Contains(progress, value => value.Stage == "wifi-mutation" && !value.Cancellable);
    }

    [Fact]
    public async Task WifiSupportsPasswordlessSudoSaveOnlyAndRejectsInvalidInput()
    {
        var remote = new FakeRemote();
        var operations = Create(remote);
        var key = Path.Combine(_project, "id_ed25519");
        File.WriteAllText(key, "test key");
        var connection = new PiConnection("pi.local", "pi", AuthMethod: AuthMethod.Key, KeyPath: key);
        await operations.ApplyWifiAsync(connection, new WifiSettings("Future Venue", "wifi-secret", Connect: false), CancellationToken.None);
        var call = Assert.Single(remote.Commands);
        Assert.StartsWith("sudo -n -v", call.Command, StringComparison.Ordinal);
        Assert.DoesNotContain("wifi rescan", call.Command, StringComparison.Ordinal);
        Assert.DoesNotContain("connection up", call.Command, StringComparison.Ordinal);
        Assert.DoesNotContain("sudo-secret", call.Input, StringComparison.Ordinal);

        await Assert.ThrowsAsync<DeploymentException>(() => operations.ApplyWifiAsync(
            connection, new WifiSettings(string.Empty, "wifi-secret"), CancellationToken.None));
        await Assert.ThrowsAsync<DeploymentException>(() => operations.ApplyWifiAsync(
            connection, new WifiSettings("Studio", "short"), CancellationToken.None));
        using var cancelled = new CancellationTokenSource();
        cancelled.Cancel();
        await Assert.ThrowsAnyAsync<OperationCanceledException>(() => operations.ApplyWifiAsync(
            connection, new WifiSettings("Studio", "wifi-secret"), cancelled.Token));
    }

    [Fact]
    public async Task ManageCommandsValidateDirectoryAndReturnOutput()
    {
        var remote = new FakeRemote { GenericOutput = "service output\n" };
        var operations = Create(remote);
        var connection = new PiConnection("pi.local", "pi", Password: "ssh-secret");
        Assert.Equal("service output\n", await operations.FetchStatusAsync(connection, "/opt/omt-client", CancellationToken.None));
        Assert.Equal("service output\n", await operations.FetchLogsAsync(connection, "/opt/omt-client", CancellationToken.None));
        Assert.Equal("service output\n", await operations.RestartServiceAsync(connection, "/opt/omt-client", CancellationToken.None));
        await Assert.ThrowsAsync<DeploymentException>(() => operations.FetchStatusAsync(connection, "/bad/../dir", CancellationToken.None));
    }

    [Fact]
    public async Task TestConnectionRejectsRemoteCommandFailure()
    {
        var remote = new FakeRemote { FailArchitectureCommand = true };
        await Assert.ThrowsAsync<CommandException>(() => Create(remote).TestConnectionAsync(
            new PiConnection("pi.local", "pi", Password: "ssh-secret"), CancellationToken.None));
    }

    [Fact]
    public async Task DeploymentValidationRejectsUnsafeOrIncompleteProjects()
    {
        var remote = new FakeRemote();
        var operations = Create(remote);
        var invalidConnection = Snapshot() with { Connection = new PiConnection("bad host", "pi", Password: "secret") };
        await Assert.ThrowsAsync<DeploymentException>(() => operations.DeployAsync(invalidConnection, CancellationToken.None));

        var manifest = Path.Combine(_project, "deploy-artifacts.txt");
        File.WriteAllText(manifest, "one\ntwo\n");
        await Assert.ThrowsAsync<DeploymentException>(() => operations.DeployAsync(Snapshot(), CancellationToken.None));
        File.WriteAllText(manifest, Manifest);
        File.Delete(Path.Combine(_project, "host-debug.sh"));
        await Assert.ThrowsAsync<DeploymentException>(() => operations.DeployAsync(Snapshot(), CancellationToken.None));
    }

    [Fact]
    public async Task PromotionInstallerAndChecksumCommandFailuresAreReportedAndCleaned()
    {
        foreach (var failingFragment in new[] { "sha256sum", " promote ", "printf" })
        {
            var remote = new FakeRemote { FailCommandContaining = failingFragment };
            var operations = Create(remote);
            await Assert.ThrowsAsync<CommandException>(() => operations.DeployAsync(Snapshot(), CancellationToken.None));
            if (failingFragment != "printf")
            {
                Assert.Contains(remote.Commands, call => call.Command.StartsWith("rm -f --", StringComparison.Ordinal));
            }
        }
    }

    [Fact]
    public async Task MalformedDigestUploadFailureAndMutationFailClosed()
    {
        var malformed = new FakeRemote { ChecksumOverride = "not-a-digest" };
        await Assert.ThrowsAsync<DeploymentException>(() => Create(malformed).DeployAsync(Snapshot(), CancellationToken.None));
        Assert.Contains(malformed.Commands, call => call.Command.StartsWith("rm -f --", StringComparison.Ordinal));

        var uploadFailure = new FakeRemote { ThrowDuringUpload = true, ThrowDuringCleanup = true };
        await Assert.ThrowsAsync<IOException>(() => Create(uploadFailure).DeployAsync(Snapshot(), CancellationToken.None));

        var mutation = new FakeRemote { MutateFirstUpload = true };
        await Assert.ThrowsAsync<DeploymentException>(() => Create(mutation).DeployAsync(Snapshot(), CancellationToken.None));
    }

    [Fact]
    public async Task PasswordlessKeyDeploymentUsesNonInteractiveSudoAndHandlesEmptyArchitecture()
    {
        var key = Path.Combine(_project, "key");
        File.WriteAllText(key, "test");
        var snapshot = Snapshot() with
        {
            Connection = new PiConnection("pi.local", "pi", AuthMethod: AuthMethod.Key, KeyPath: key),
        };
        var remote = new FakeRemote { ReportZeroUploadTotal = true };
        await Create(remote).DeployAsync(snapshot, CancellationToken.None);
        Assert.Contains(remote.Commands, call => call.Command.StartsWith("sudo -n install", StringComparison.Ordinal));
        Assert.Contains(remote.Commands, call => call.Command.Contains("&& sudo -n sh", StringComparison.Ordinal));

        var emptyArchitecture = new FakeRemote { Architecture = string.Empty };
        var exception = await Assert.ThrowsAsync<DeploymentException>(() => Create(emptyArchitecture).DeployAsync(Snapshot(), CancellationToken.None));
        Assert.Contains("unrecognized", exception.Message, StringComparison.Ordinal);
    }

    [Fact]
    public async Task PasswordAuthenticationFallsBackToSshPasswordForSudo()
    {
        var remote = new FakeRemote { ReportGranularProgress = true };
        var snapshot = Snapshot() with
        {
            Connection = new PiConnection("pi.local", "pi", Password: "ssh-only-secret"),
        };
        await Create(remote).DeployAsync(snapshot, CancellationToken.None);
        var prepare = remote.Commands.Single(call => call.Command.Contains("install -d", StringComparison.Ordinal));
        Assert.StartsWith("ssh-only-secret\n", prepare.Input, StringComparison.Ordinal);
    }

    [Fact]
    public async Task MissingTarEntryEmptyDigestAndCleanupFailureAreClosedFailures()
    {
        var manifest = Path.Combine(_project, "deploy-artifacts.txt");
        File.WriteAllText(manifest, Manifest.Replace("omt-client-arm64.tar.gz", "replacement.tar", StringComparison.Ordinal));
        File.WriteAllText(Path.Combine(_project, "replacement.tar"), "replacement");
        await Assert.ThrowsAsync<DeploymentException>(() => Create(new FakeRemote()).DeployAsync(Snapshot(), CancellationToken.None));

        File.WriteAllText(manifest, Manifest);
        var emptyDigest = new FakeRemote { EmptyChecksumOutput = true, FailCleanup = true };
        await Assert.ThrowsAsync<DeploymentException>(() => Create(emptyDigest).DeployAsync(Snapshot(), CancellationToken.None));
    }

    [Fact]
    public async Task BuildRequestFailsEarlyWhenDockerIsMissingAndInvalidConnectionFailsManageAction()
    {
        var build = Snapshot() with { Options = new DeployOptions(_project, BuildImage: true) };
        var originalPath = Environment.GetEnvironmentVariable("PATH");
        try
        {
            Environment.SetEnvironmentVariable("PATH", string.Empty);
            await Assert.ThrowsAsync<FileNotFoundException>(() => Create(new FakeRemote()).DeployAsync(build, CancellationToken.None));
        }
        finally
        {
            Environment.SetEnvironmentVariable("PATH", originalPath);
        }

        await Assert.ThrowsAsync<DeploymentException>(() => Create(new FakeRemote()).TestConnectionAsync(
            new PiConnection("bad host", "pi", Password: "secret"), CancellationToken.None));
    }

    [Fact]
    public void CommandExceptionSelectsStderrStdoutAndFallbackMessages()
    {
        Assert.Equal("stderr", new CommandException(new CommandResult("cmd", 1, "stdout", "stderr")).Message);
        Assert.Equal("stdout", new CommandException(new CommandResult("cmd", 1, "stdout")).Message);
        Assert.Contains("cmd", new CommandException(new CommandResult("cmd", 1)).Message, StringComparison.Ordinal);
    }

    public void Dispose() => Directory.Delete(_project, recursive: true);

    private const string Manifest =
        "omt-client-arm64.tar.gz\n" +
        "docker-compose.yml\n" +
        "install.sh\n" +
        "uninstall.sh\n" +
        "host-debug.sh\n" +
        "host-reboot.sh\n" +
        "LICENSE\n" +
        "THIRD_PARTY_NOTICES.txt\n" +
        "THIRD_PARTY_SOURCE.md\n";

    private static DeploymentOperations Create(FakeRemote remote) =>
        new(new ScriptedRunner(), new SingleRemoteFactory(remote), new ArtifactSnapshotProvider());

    private ActionSnapshot Snapshot() => new(
        new PiConnection("pi.local", "pi", Password: "ssh-secret", SudoPassword: "sudo-secret"),
        new DeployOptions(_project, BuildImage: false),
        new WifiSettings("Studio", "wifi-secret"));
}

internal sealed class ScriptedRunner : ICommandRunner
{
    public Queue<CommandResult> Results { get; } = new();
    public List<string> Calls { get; } = [];
    public Exception? Exception { get; set; }

    public Task<CommandResult> RunAsync(IReadOnlyList<string> arguments, string? workingDirectory, Action<string>? onOutput, TimeSpan timeout, CancellationToken cancellationToken)
    {
        Calls.Add(string.Join(' ', arguments));
        if (Exception is not null)
        {
            throw Exception;
        }

        return Task.FromResult(Results.Count > 0 ? Results.Dequeue() : new CommandResult(Calls[^1], 1));
    }
}

internal sealed class SingleRemoteFactory(FakeRemote remote) : IRemoteClientFactory
{
    public IRemoteClient Create() => remote;
}

internal sealed record RemoteCall(string Command, string Input, bool TokenCanBeCancelled);

internal sealed class FakeRemote : IRemoteClient
{
    private readonly Dictionary<string, string> _uploads = new(StringComparer.Ordinal);
    public string Architecture { get; init; } = "aarch64";
    public string GenericOutput { get; init; } = string.Empty;
    public bool WrongChecksum { get; init; }
    public bool FailArchitectureCommand { get; init; }
    public string? FailCommandContaining { get; init; }
    public string? ChecksumOverride { get; init; }
    public bool ThrowDuringUpload { get; init; }
    public bool ThrowDuringCleanup { get; init; }
    public bool MutateFirstUpload { get; init; }
    public bool ReportZeroUploadTotal { get; init; }
    public bool ReportGranularProgress { get; init; }
    public bool EmptyChecksumOutput { get; init; }
    public bool FailCleanup { get; init; }
    public List<RemoteCall> Commands { get; } = [];
    public List<(string Local, string Remote)> Uploads { get; } = [];

    public Task ConnectAsync(PiConnection connection, CancellationToken cancellationToken) => Task.CompletedTask;

    public Task<CommandResult> RunAsync(string command, string input, Action<string>? onOutput, TimeSpan timeout, CancellationToken cancellationToken)
    {
        Commands.Add(new RemoteCall(command, input, cancellationToken.CanBeCanceled));
        if (ThrowDuringCleanup && command.StartsWith("rm -f --", StringComparison.Ordinal))
        {
            throw new IOException("cleanup failed");
        }

        if (command.StartsWith("uname -m", StringComparison.Ordinal))
        {
            return Task.FromResult(FailArchitectureCommand
                ? new CommandResult(command, 1, StandardError: "failed")
                : new CommandResult(command, 0, Architecture.Length == 0 ? "\n" : $"{Architecture}\npi\n"));
        }

        if (command.StartsWith("sha256sum", StringComparison.Ordinal))
        {
            var staged = _uploads.Keys.Single(path => command.Contains(path, StringComparison.Ordinal));
            var digest = ChecksumOverride ?? (WrongChecksum ? new string('0', 64) : Convert.ToHexStringLower(SHA256.HashData(File.ReadAllBytes(_uploads[staged]))));
            if (string.Equals(FailCommandContaining, "sha256sum", StringComparison.Ordinal))
            {
                return Task.FromResult(new CommandResult(command, 1, StandardError: "checksum failed"));
            }

            return Task.FromResult(new CommandResult(command, 0, EmptyChecksumOutput ? string.Empty : $"{digest}  {staged}\n"));
        }

        if (FailCleanup && command.StartsWith("rm -f --", StringComparison.Ordinal))
        {
            return Task.FromResult(new CommandResult(command, 1, StandardError: "cleanup failed"));
        }

        if (!string.IsNullOrEmpty(FailCommandContaining) && command.Contains(FailCommandContaining, StringComparison.Ordinal))
        {
            return Task.FromResult(new CommandResult(command, 1, StandardError: "remote failed"));
        }

        onOutput?.Invoke(GenericOutput);
        return Task.FromResult(new CommandResult(command, 0, GenericOutput));
    }

    public Task UploadAsync(string localPath, string remotePath, TimeSpan timeout, Action<long, long>? onProgress, CancellationToken cancellationToken)
    {
        if (ThrowDuringUpload)
        {
            throw new IOException("upload failed");
        }

        Uploads.Add((localPath, remotePath));
        _uploads.Add(remotePath, localPath);
        var length = new FileInfo(localPath).Length;
        if (ReportGranularProgress)
        {
            onProgress?.Invoke(1, 100);
            onProgress?.Invoke(5, 100);
        }
        else
        {
            onProgress?.Invoke(ReportZeroUploadTotal ? 0 : length, ReportZeroUploadTotal ? 0 : length);
        }
        if (MutateFirstUpload && Uploads.Count == 1)
        {
            File.AppendAllText(localPath, "changed");
        }

        return Task.CompletedTask;
    }

    public ValueTask DisposeAsync() => ValueTask.CompletedTask;
}
