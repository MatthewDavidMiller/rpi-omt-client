using System.Security.Cryptography;
using System.Text;
using RpiOmt.Deployer.Core;

namespace RpiOmt.Deployer.Tests;

public sealed class ValidationTests
{
    [Theory]
    [InlineData("pi.local", true)]
    [InlineData("192.168.1.20", true)]
    [InlineData("pi-5", true)]
    [InlineData("", false)]
    [InlineData("-pi.local", false)]
    [InlineData("pi..local", false)]
    [InlineData("pi local", false)]
    [InlineData("pi\nlocal", false)]
    public void HostValidationIsStrict(string value, bool expected) =>
        Assert.Equal(expected, InputValidation.IsValidHost(value));

    [Theory]
    [InlineData("/opt/omt-client", true)]
    [InlineData("/srv/omt.v2_client", true)]
    [InlineData("/", false)]
    [InlineData("relative", false)]
    [InlineData("/opt/", false)]
    [InlineData("/opt//omt", false)]
    [InlineData("/opt/../root", false)]
    [InlineData("/opt/omt client", false)]
    public void RemoteDirectoryValidationRejectsAmbiguity(string value, bool expected) =>
        Assert.Equal(expected, InputValidation.IsValidRemoteDirectory(value));

    [Fact]
    public void WifiValidationHonorsByteAndPskBoundaries()
    {
        Assert.Null(InputValidation.WifiSsidError(new string('x', 32)));
        Assert.NotNull(InputValidation.WifiSsidError(new string('é', 17)));
        Assert.Contains("control", InputValidation.WifiSsidError("bad\nssid"), StringComparison.OrdinalIgnoreCase);
        Assert.Null(InputValidation.WifiPasswordError("12345678"));
        Assert.Null(InputValidation.WifiPasswordError(new string('a', 64)));
        Assert.NotNull(InputValidation.WifiPasswordError("short"));
        Assert.NotNull(InputValidation.WifiPasswordError(new string('z', 64)));
        Assert.NotNull(InputValidation.WifiPasswordError("password\n"));
    }

    [Fact]
    public void ConnectionValidationSeparatesAuthenticationSecrets()
    {
        var missingPassword = InputValidation.ValidateConnection(new PiConnection("pi.local", "pi"));
        Assert.Contains(missingPassword, error => error.Contains("password", StringComparison.Ordinal));
        var control = InputValidation.ValidateConnection(new PiConnection("pi.local", "pi", Password: "bad\n"));
        Assert.Contains(control, error => error.Contains("control", StringComparison.Ordinal));
        var badUser = InputValidation.ValidateConnection(new PiConnection("pi.local", "pi user", Password: "secret"));
        Assert.Contains(badUser, error => error.Contains("username", StringComparison.Ordinal));
        var badPort = InputValidation.ValidateConnection(new PiConnection("pi.local", "pi", 0, Password: "secret"));
        Assert.Contains(badPort, error => error.Contains("port", StringComparison.Ordinal));
        var missingKey = InputValidation.ValidateConnection(new PiConnection("pi.local", "pi", AuthMethod: AuthMethod.Key, KeyPath: "/missing"));
        Assert.Contains(missingKey, error => error.Contains("key", StringComparison.Ordinal));
        Assert.Empty(InputValidation.ValidateConnection(new PiConnection("pi.local", "pi", Password: "secret")));
    }

    [Fact]
    public void OptionsValidationChecksProjectAndRemoteDirectory()
    {
        var errors = InputValidation.ValidateOptions(new DeployOptions("/missing", "/"));
        Assert.Equal(2, errors.Count);
        Assert.Empty(InputValidation.ValidateOptions(new DeployOptions("/missing"), requireProject: false));
    }

    [Fact]
    public void ValidationCoversLengthComponentsControlCharactersAndKeySuccess()
    {
        Assert.False(InputValidation.IsValidHost(new string('a', 254)));
        Assert.False(InputValidation.IsValidHost($"{new string('a', 64)}.local"));
        Assert.True(InputValidation.ContainsControlCharacter("delete\u007f"));
        Assert.False(InputValidation.ContainsControlCharacter("printable"));
        Assert.False(InputValidation.IsValidRemoteDirectory(string.Empty));
        Assert.False(InputValidation.IsValidRemoteDirectory("/opt/."));
        Assert.False(InputValidation.IsValidRemoteDirectory("/opt/.."));
        Assert.NotNull(InputValidation.WifiSsidError(string.Empty));

        var key = Path.GetTempFileName();
        try
        {
            var connection = new PiConnection(
                "pi.local",
                "pi",
                65535,
                AuthMethod.Key,
                KeyPath: key,
                KeyPassphrase: "bad\n",
                SudoPassword: "bad\u007f");
            var errors = InputValidation.ValidateConnection(connection);
            Assert.DoesNotContain(errors, error => error.Contains("does not exist", StringComparison.Ordinal));
            Assert.Equal(2, errors.Count(error => error.Contains("control", StringComparison.Ordinal)));
        }
        finally
        {
            File.Delete(key);
        }
    }
}

public sealed class ActionControllerTests
{
    [Fact]
    public void LifecycleEnforcesMutualExclusionAndCancellationPolicy()
    {
        var controller = new ActionController();
        Assert.False(controller.IsActive);
        Assert.False(controller.RequestCancellation());
        Assert.True(controller.Start());
        Assert.False(controller.Start());
        controller.Progress(new ProgressEventArgs("upload", stage: "upload"));
        Assert.Equal("upload", controller.Stage);
        Assert.True(controller.RequestCancellation());
        controller.Progress(new ProgressEventArgs("cleanup", stage: "cleanup"));
        Assert.Equal(OperationState.Cancelling, controller.State);
        controller.Finish(OperationState.Cancelled);
        Assert.False(controller.IsActive);
        Assert.Equal("complete", controller.Stage);
    }

    [Fact]
    public void NonCancellableAndTerminalContractsFailClosed()
    {
        var controller = new ActionController();
        controller.Start();
        controller.Progress(new ProgressEventArgs("installer", stage: "installer", cancellable: false));
        Assert.False(controller.RequestCancellation());
        Assert.Throws<ArgumentOutOfRangeException>(() => controller.Finish(OperationState.Running));
        controller.Finish(OperationState.Succeeded);
    }

    [Fact]
    public void EmptyProgressStageRetainsTheCurrentStage()
    {
        var controller = new ActionController();
        Assert.True(controller.Start());
        controller.Progress(new ProgressEventArgs("still starting"));
        Assert.Equal("starting", controller.Stage);
        controller.Finish(OperationState.Failed);
    }
}

public sealed class ArtifactTests : IDisposable
{
    private readonly string _directory = Path.Combine(Path.GetTempPath(), $"rpi-omt-artifacts-{Guid.NewGuid():N}");

    public ArtifactTests() => Directory.CreateDirectory(_directory);

    [Fact]
    public void ManifestMustBeBoundedUniqueAsciiAndSafe()
    {
        var path = FilePath("manifest");
        File.WriteAllText(path, "version=2\none\ndeploy/two\n", Encoding.ASCII);
        Assert.Equal(["one", "deploy/two"], ArtifactSnapshots.LoadManifest(path));
        File.WriteAllText(path, "version=2\none\none\n", Encoding.ASCII);
        Assert.Throws<ArtifactException>(() => ArtifactSnapshots.LoadManifest(path));
        File.WriteAllText(path, "version=2\n../unsafe\n", Encoding.ASCII);
        Assert.Throws<ArtifactException>(() => ArtifactSnapshots.LoadManifest(path));
        File.WriteAllBytes(path, [.. "version=2\n"u8, 0xc3, 0xa9, (byte)'\n']);
        Assert.Throws<ArtifactException>(() => ArtifactSnapshots.LoadManifest(path));
        File.WriteAllBytes(path, new byte[32769]);
        Assert.Throws<ArtifactException>(() => ArtifactSnapshots.LoadManifest(path));
        File.WriteAllText(
            path,
            "version=2\n" + string.Join('\n', Enumerable.Range(0, 129).Select(index => $"f{index}")),
            Encoding.ASCII);
        Assert.Throws<ArtifactException>(() => ArtifactSnapshots.LoadManifest(path));
        foreach (var unsafePath in new[] { "/absolute", "a//b", "a/./b", "a/../b", "a/", ".", "..", "with space" })
        {
            File.WriteAllText(path, $"version=2\n{unsafePath}\n", Encoding.ASCII);
            Assert.Throws<ArtifactException>(() => ArtifactSnapshots.LoadManifest(path));
        }

        File.WriteAllText(path, "version=1\none\n", Encoding.ASCII);
        Assert.Throws<ArtifactException>(() => ArtifactSnapshots.LoadManifest(path));
    }

    [Fact]
    public void ManifestRejectsMissingEmptyAndSymlink()
    {
        var path = FilePath("missing");
        Assert.Throws<ArtifactException>(() => ArtifactSnapshots.LoadManifest(path));
        Assert.Throws<ArtifactException>(() => ArtifactSnapshots.LoadManifest(_directory));
        File.WriteAllText(path, string.Empty);
        Assert.Throws<ArtifactException>(() => ArtifactSnapshots.LoadManifest(path));
        var target = FilePath("target");
        File.WriteAllText(target, "version=2\none\n");
        File.Delete(path);
        File.CreateSymbolicLink(path, target);
        Assert.Throws<ArtifactException>(() => ArtifactSnapshots.LoadManifest(path));
    }

    [Theory]
    [InlineData("nested/file.txt", true)]
    [InlineData("", false)]
    [InlineData("/absolute", false)]
    [InlineData("trailing/", false)]
    [InlineData("double//slash", false)]
    [InlineData("bad space", false)]
    [InlineData("./file", false)]
    [InlineData("../file", false)]
    [InlineData("nested/./file", false)]
    [InlineData("nested/../file", false)]
    public void ManifestRelativePathValidationCoversEveryBoundary(
        string value,
        bool expected) =>
        Assert.Equal(expected, ArtifactSnapshots.IsSafeRelativePath(value));

    [Fact]
    public void ManifestRelativePathRejectsExcessiveLength() =>
        Assert.False(ArtifactSnapshots.IsSafeRelativePath(new string('a', 241)));

    [Fact]
    public async Task SnapshotUsesSha256AndDetectsChanges()
    {
        var path = FilePath("artifact");
        await File.WriteAllTextAsync(path, "before", TestContext.Current.CancellationToken);
        var snapshot = await ArtifactSnapshots.CaptureAsync(path, CancellationToken.None);
        Assert.Equal(Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes("before"))), snapshot.Sha256);
        Assert.True(await snapshot.IsUnchangedAsync(TestContext.Current.CancellationToken));
        var wrongDigest = snapshot with { Sha256 = new string('0', 64) };
        Assert.False(await wrongDigest.IsUnchangedAsync(TestContext.Current.CancellationToken));
        await File.WriteAllTextAsync(path, "after!", TestContext.Current.CancellationToken);
        Assert.False(await snapshot.IsUnchangedAsync(TestContext.Current.CancellationToken));
        File.Delete(path);
        Assert.False(await snapshot.IsUnchangedAsync(TestContext.Current.CancellationToken));
        await Assert.ThrowsAsync<ArtifactException>(() => ArtifactSnapshots.CaptureAsync(path, CancellationToken.None));
    }

    [Fact]
    public async Task SnapshotRejectsConcurrentMutation()
    {
        var path = FilePath("large-artifact");
        await using (var file = File.Create(path))
        {
            file.SetLength(128 * 1024 * 1024);
        }

        using var stop = new CancellationTokenSource();
        var mutator = Task.Run(async () =>
        {
            while (!stop.IsCancellationRequested)
            {
                File.SetLastWriteTimeUtc(path, DateTime.UtcNow);
                await Task.Yield();
            }
        }, TestContext.Current.CancellationToken);
        try
        {
            await Assert.ThrowsAsync<ArtifactException>(() => ArtifactSnapshots.CaptureAsync(path, TestContext.Current.CancellationToken));
        }
        finally
        {
            stop.Cancel();
            await mutator;
        }
    }

    public void Dispose() => Directory.Delete(_directory, recursive: true);

    private string FilePath(string name) => Path.Combine(_directory, name);
}

public sealed class UtilityTests
{
    [Fact]
    public void ShellQuoteAndRedactionProtectValues()
    {
        Assert.Equal("'simple'", Shell.Quote("simple"));
        Assert.Equal("'it'\"'\"'s'", Shell.Quote("it's"));
        var redactor = new SecretRedactor(["short", "short-and-long"]);
        Assert.Equal("<redacted> <redacted>", redactor.Redact("short-and-long short"));
        var result = redactor.Redact(new CommandResult("short", 1, "short-and-long", "short"));
        Assert.DoesNotContain("short", result.Command, StringComparison.Ordinal);
        Assert.DoesNotContain("short", result.StandardOutput, StringComparison.Ordinal);
    }

    [Fact]
    public void BoundedBufferReportsTruncation()
    {
        var buffer = new BoundedTextBuffer(4);
        buffer.Append("abcdef");
        Assert.True(buffer.Truncated);
        Assert.Contains("retained 4 of 6", buffer.ToString(), StringComparison.Ordinal);
        var exact = new BoundedTextBuffer(4);
        exact.Append("test");
        Assert.False(exact.Truncated);
        Assert.Equal("test", exact.ToString());
        var unicode = new BoundedTextBuffer(2);
        unicode.Append("éx");
        Assert.True(unicode.Truncated);
    }

    [Fact]
    public void BoundedBufferKeepsWholeScalarsAtItsLimit()
    {
        // The limit must never split a scalar: half a sequence decodes to
        // U+FFFD, so an operator would read corruption instead of output.
        var exact = new BoundedTextBuffer(2);
        exact.Append("é");
        Assert.False(exact.Truncated);
        Assert.Equal("é", exact.ToString());

        var split = new BoundedTextBuffer(2);
        split.Append("aé");
        Assert.True(split.Truncated);
        Assert.StartsWith("a\n[output truncated: retained 1 of 3 bytes]", split.ToString(), StringComparison.Ordinal);
        Assert.DoesNotContain("�", split.ToString(), StringComparison.Ordinal);

        var full = new BoundedTextBuffer(0);
        full.Append("a");
        Assert.True(full.Truncated);
        Assert.StartsWith("\n[output truncated: retained 0 of 1 bytes]", full.ToString(), StringComparison.Ordinal);
    }

    [Fact]
    public void BoundedBufferRejoinsScalarsSplitAcrossChunks()
    {
        // Remote reads land on arbitrary byte boundaries. Decoding each chunk
        // alone would turn every straddling character into U+FFFD.
        var bytes = Encoding.UTF8.GetBytes("héllo");
        var buffer = new BoundedTextBuffer(64);
        foreach (var index in Enumerable.Range(0, bytes.Length))
        {
            buffer.Append(bytes.AsSpan(index, 1));
        }

        Assert.False(buffer.Truncated);
        Assert.Equal("héllo", buffer.ToString());
    }
}

public sealed class ProcessRunnerTests
{
    [Fact]
    public async Task RunnerCapturesStreamsAndCallbacks()
    {
        var lines = new List<string>();
        var result = await new ProcessCommandRunner().RunAsync(
            ["/bin/sh", "-c", "printf 'out\\n'; printf 'err\\n' >&2"],
            null,
            lines.Add,
            TimeSpan.FromSeconds(2),
            CancellationToken.None);
        Assert.True(result.IsSuccess);
        Assert.Contains("out", result.StandardOutput, StringComparison.Ordinal);
        Assert.Contains("err", result.StandardError, StringComparison.Ordinal);
        Assert.Contains("out", lines);
        Assert.Contains("err", lines);
    }

    [Fact]
    public async Task RunnerBoundsOutputAndTerminatesOnTimeoutAndCancellation()
    {
        var bounded = await new ProcessCommandRunner().RunAsync(
            ["/bin/sh", "-c", $"head -c {ProcessCommandRunner.MaximumOutputBytes + 1} /dev/zero | tr '\\0' x"],
            null,
            null,
            TimeSpan.FromSeconds(10),
            CancellationToken.None);
        Assert.Contains("output truncated", bounded.StandardOutput, StringComparison.Ordinal);
        await Assert.ThrowsAsync<TimeoutException>(() => new ProcessCommandRunner().RunAsync(
            ["/bin/sh", "-c", "sleep 5"], null, null, TimeSpan.FromMilliseconds(50), CancellationToken.None));
        using var cancellation = new CancellationTokenSource(TimeSpan.FromMilliseconds(50));
        await Assert.ThrowsAnyAsync<OperationCanceledException>(() => new ProcessCommandRunner().RunAsync(
            ["/bin/sh", "-c", "sleep 5"], null, null, TimeSpan.FromSeconds(5), cancellation.Token));
    }

    [Fact]
    public async Task RunnerRejectsMissingAndEmptyCommands()
    {
        await Assert.ThrowsAsync<ArgumentException>(() => new ProcessCommandRunner().RunAsync(
            [], null, null, TimeSpan.FromSeconds(1), CancellationToken.None));
        await Assert.ThrowsAsync<FileNotFoundException>(() => new ProcessCommandRunner().RunAsync(
            ["definitely-not-a-real-command-rpi-omt"], null, null, TimeSpan.FromSeconds(1), CancellationToken.None));
        Assert.True(ProcessCommandRunner.IsExecutableAvailable("sh"));
        Assert.False(ProcessCommandRunner.IsExecutableAvailable("definitely-not-a-real-command-rpi-omt"));
    }
}
