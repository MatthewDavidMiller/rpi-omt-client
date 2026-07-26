using System.Text;
using System.Text.Json;
using System.Globalization;
using RpiOmt.Receiver.Core;
using Xunit;

namespace RpiOmt.Receiver.Core.Tests;

public sealed class CoreTests
{
    [Fact]
    public void CliParsesEveryCommand()
    {
        Assert.IsType<VersionCommand>(ReceiverCli.Parse(["--version"]));
        Assert.Equal(
            new DiscoverCommand(1_500),
            ReceiverCli.Parse(["discover", "--json"]));
        Assert.Equal(
            new DiscoverCommand(0),
            ReceiverCli.Parse(["discover", "--wait-ms", "0", "--json"]));
        Assert.Equal(
            new ProbeCommand("omt://host:6400", 5),
            ReceiverCli.Parse(
                ["probe", "--target", "omt://host:6400", "--timeout-ms", "5", "--json"]));
        Assert.Equal(
            new ProbeCommand("--camera", 3_000),
            ReceiverCli.Parse(["probe", "--target", "--camera", "--json"]));
        Assert.Equal(
            new PlayCommand("Camera", "HDMI-A-2", "/tmp/status", 3),
            ReceiverCli.Parse(
                [
                    "play",
                    "--target",
                    "Camera",
                    "--connector",
                    "HDMI-A-2",
                    "--status-file",
                    "/tmp/status",
                    "--retry-seconds",
                    "3",
                ]));
        Assert.Equal(
            new PlayCommand("Camera", "auto", "/tmp/status", 2),
            ReceiverCli.Parse(
                ["play", "--target", "Camera", "--status-file", "/tmp/status"]));
        Assert.Equal(
            new PlayCommand("--camera", "auto", "/tmp/status", 2),
            ReceiverCli.Parse(
                ["play", "--target", "--camera", "--status-file", "/tmp/status"]));
        Assert.Equal(
            "HDMI-A-1",
            Assert.IsType<PlayCommand>(
                ReceiverCli.Parse(
                    [
                        "play",
                        "--target",
                        "Camera",
                        "--connector",
                        "HDMI-A-1",
                        "--status-file",
                        "/tmp/status",
                    ])).Connector);
    }

    [Theory]
    [InlineData()]
    [InlineData("unknown")]
    [InlineData("discover")]
    [InlineData("discover", "--json", "--json")]
    [InlineData("discover", "--json", "--target", "Camera")]
    [InlineData("discover", "unexpected", "--json")]
    [InlineData("discover", "--wait-ms", "--json")]
    [InlineData("discover", "--wait-ms")]
    [InlineData("discover", "--wait-ms", "-1", "--json")]
    [InlineData("discover", "--wait-ms", "60001", "--json")]
    [InlineData("probe", "--json")]
    [InlineData("probe", "--target", "", "--json")]
    [InlineData("probe", "--target", "bad\nname", "--json")]
    [InlineData("probe", "--target", "Camera", "--timeout-ms", "0", "--json")]
    [InlineData("probe", "--target", "Camera", "--json", "--connector", "auto")]
    [InlineData("play", "--target", "Camera", "--status-file", "/tmp/x", "--json")]
    [InlineData("play", "--target", "Camera", "--status-file", "/tmp/x", "--connector", "bad")]
    [InlineData("play", "--target", "Camera", "--status-file", "/tmp/x", "--retry-seconds", "31")]
    [InlineData("--version", "extra")]
    public void CliRejectsMalformedOrInappropriateOptions(params string[] values)
    {
        Assert.Throws<ArgumentException>(() => ReceiverCli.Parse(values));
    }

    [Fact]
    public void SharedValidationVectorsAreEnforced()
    {
        using JsonDocument document = LoadVectors("omt-target-vectors.json");
        foreach (JsonElement vector in document.RootElement.GetProperty("source_names")
                     .EnumerateArray())
        {
            Assert.Equal(
                vector.GetProperty("valid").GetBoolean(),
                TargetValidator.IsValidDiscoveredName(
                    vector.GetProperty("value").GetString()!));
        }
        foreach (JsonElement vector in document.RootElement.GetProperty("direct_targets")
                     .EnumerateArray())
        {
            Assert.Equal(
                vector.GetProperty("valid").GetBoolean(),
                TargetValidator.IsValidDirectTarget(
                    vector.GetProperty("value").GetString()!));
        }
    }

    [Fact]
    public void ValidationRejectsInvalidUnicodeAndLongHosts()
    {
        Assert.False(TargetValidator.IsValidDiscoveredName("\ud800"));
        Assert.False(TargetValidator.IsValidDirectTarget(""));
        Assert.False(TargetValidator.IsValidDirectTarget($"omt://{new string('a', 506)}:1"));
        Assert.False(TargetValidator.IsValidDirectTarget("omt://é:1"));
        Assert.False(TargetValidator.IsValidDirectTarget("omt://host:\n"));
        Assert.False(TargetValidator.IsValidDirectTarget("omt://"));
        Assert.False(TargetValidator.IsValidDirectTarget("omt://[:1"));
        Assert.False(TargetValidator.IsValidDirectTarget("omt://[]:1"));
        Assert.False(TargetValidator.IsValidDirectTarget("omt://[::1]1"));
        Assert.False(TargetValidator.IsValidDirectTarget("omt://[::1]:"));
        Assert.False(TargetValidator.IsValidDirectTarget("omt://host:x"));
        Assert.False(TargetValidator.IsValidDirectTarget("omt://host_name:1"));
        Assert.True(TargetValidator.IsValidDirectTarget("omt://CAMERA-1:0001"));
        Assert.False(TargetValidator.IsValidDirectTarget(
            $"omt://{"a".PadRight(254, 'a')}:1"));
        Assert.False(TargetValidator.IsValidDirectTarget("omt://[192.0.2.1]:1"));
        Assert.Throws<ArgumentException>(() => TargetValidator.Validate("omt://host"));
        Assert.Throws<ArgumentException>(() => TargetValidator.Validate("\n"));
        TargetValidator.Validate("Camera 😀");
    }

    [Theory]
    [InlineData(1, 1, 1, true)]
    [InlineData(1920, 1080, 60, true)]
    [InlineData(0, 1080, 60, false)]
    [InlineData(1921, 1080, 60, false)]
    [InlineData(1920, 0, 60, false)]
    [InlineData(1920, 1081, 60, false)]
    [InlineData(1920, 1080, 0, false)]
    [InlineData(1920, 1080, 60.01, false)]
    [InlineData(1920, 1080, double.NaN, false)]
    [InlineData(1920, 1080, double.PositiveInfinity, false)]
    public void FormatPolicyIsBounded(
        int width,
        int height,
        double rate,
        bool expected)
    {
        Assert.Equal(expected, FormatPolicy.IsSupported(width, height, rate));
    }

    [Fact]
    public void SanitizationIsScalarAwareAndBounded()
    {
        string value = $"  hello\n😀{new string('x', 600)}  ";
        string sanitized = StatusSanitizer.Sanitize(value);
        Assert.DoesNotContain('\n', sanitized);
        Assert.Contains("😀", sanitized);
        Assert.True(sanitized.Length <= 512);
        Assert.Equal("space allowed", StatusSanitizer.Sanitize(" space allowed "));
        Assert.Equal(
            new string('x', 510) + "😀",
            StatusSanitizer.Sanitize(new string('x', 510) + "😀"));
        Assert.Equal(
            new string('x', 511),
            StatusSanitizer.Sanitize(new string('x', 511) + "😀"));
    }

    [Fact]
    public void VideoStateRemainsAuthoritativeAcrossAudioOrderings()
    {
        PlaybackStateModel model = new();
        Assert.Equal("stopped", model.Snapshot().State);

        PlaybackProjection audioFirst = model.AudioRunning();
        Assert.Equal("stopped", audioFirst.State);
        Assert.Equal("running", audioFirst.AudioState);

        Assert.Equal("starting", model.VideoStarting().State);
        Assert.Equal("waiting-for-discovery", model.WaitingForDiscovery("no bus").State);
        Assert.Equal("waiting-for-hdmi", model.WaitingForHdmi("no display").State);
        Assert.Equal("retrying", model.VideoRetrying("no video").State);
        Assert.Equal("unsupported-format", model.UnsupportedFormat("4k").State);

        PlaybackProjection running = model.VideoRunning("video");
        Assert.Equal("running", running.State);
        Assert.Equal("running", running.VideoState);

        PlaybackProjection degraded = model.AudioFailed("audio\nfailed");
        Assert.Equal("degraded", degraded.State);
        Assert.Equal("running", degraded.VideoState);
        Assert.Equal("failed", degraded.AudioState);
        Assert.Equal("audiofailed", degraded.Detail);
        model.AudioFailed("\n");
        Assert.Equal(
            "Video is playing but audio is unavailable.",
            model.VideoRunning("video").Detail);
        PlaybackProjection audioStopped = model.AudioStopped();
        Assert.Equal("running", audioStopped.State);
        Assert.Equal("stopped", audioStopped.AudioState);
        Assert.Equal("video", audioStopped.Detail);

        Assert.Equal("retrying", model.VideoRetrying("video lost").State);
        Assert.Equal("retrying", model.AudioRunning().State);
        Assert.Equal("running", model.VideoRunning("video restored").State);
        Assert.Equal("stopped", model.Stopped().State);
    }

    [Fact]
    public void StatusSerializationUsesTheSchemaBoundary()
    {
        DateTimeOffset now = DateTimeOffset.Parse(
            "2026-01-01T00:00:00Z",
            CultureInfo.InvariantCulture);
        byte[] encoded = StatusSerializer.Serialize(
            new PlaybackStatusDocument(
                1,
                "running",
                "running",
                "running",
                "Camera",
                "ok",
                "HDMI-A-1",
                "/dev/dri/card1",
                "plughw:0",
                now));
        string json = Encoding.UTF8.GetString(encoded);
        Assert.Contains("\"schema\":1", json);
        Assert.Contains("\"video_state\":\"running\"", json);
        Assert.Contains("\"updated_at\":\"2026-01-01T00:00:00+00:00\"", json);
    }

    [Fact]
    public void SerializedFieldNamesMatchTheSharedConsumerContract()
    {
        // src/omt_client/services/playback.py requires set(document) == STATUS_FIELDS
        // exactly. A field added or renamed here alone would make Python reject every
        // status record and pin the dashboard to "Playback status stale".
        using JsonDocument vectors = LoadVectors("playback-status-vectors.json");
        string[] expected = vectors.RootElement.GetProperty("fields")
            .EnumerateArray()
            .Select(field => field.GetString()!)
            .Order(StringComparer.Ordinal)
            .ToArray();

        byte[] encoded = StatusSerializer.Serialize(
            new PlaybackStatusDocument(
                vectors.RootElement.GetProperty("schema").GetInt32(),
                "running",
                "running",
                "running",
                "Camera",
                "ok",
                "HDMI-A-1",
                "/dev/dri/card1",
                "plughw:0",
                DateTimeOffset.UtcNow));
        using JsonDocument produced = JsonDocument.Parse(encoded);
        string[] actual = produced.RootElement.EnumerateObject()
            .Select(property => property.Name)
            .Order(StringComparer.Ordinal)
            .ToArray();

        Assert.Equal(expected, actual);
    }

    [Fact]
    public void ProducedProjectionsMatchTheSharedConsumerContract()
    {
        using JsonDocument vectors = LoadVectors("playback-status-vectors.json");

        foreach (JsonElement vector in vectors.RootElement.GetProperty("projections")
                     .EnumerateArray())
        {
            PlaybackStateModel model = new();
            PlaybackProjection projection = model.Snapshot();
            foreach (JsonElement step in vector.GetProperty("events").EnumerateArray())
            {
                projection = Apply(model, step.GetString()!);
            }

            Assert.Equal(vector.GetProperty("state").GetString(), projection.State);
            Assert.Equal(vector.GetProperty("video_state").GetString(), projection.VideoState);
            Assert.Equal(vector.GetProperty("audio_state").GetString(), projection.AudioState);
            Assert.Equal(projection, model.Snapshot());
        }
    }

    private static PlaybackProjection Apply(PlaybackStateModel model, string playbackEvent) =>
        playbackEvent switch
        {
            "VideoStarting" => model.VideoStarting(),
            "WaitingForDiscovery" => model.WaitingForDiscovery("no bus"),
            "WaitingForHdmi" => model.WaitingForHdmi("no display"),
            "VideoRetrying" => model.VideoRetrying("retrying"),
            "UnsupportedFormat" => model.UnsupportedFormat("unsupported"),
            "VideoRunning" => model.VideoRunning("playing"),
            "AudioRunning" => model.AudioRunning(),
            "AudioFailed" => model.AudioFailed("audio unavailable"),
            "AudioStopped" => model.AudioStopped(),
            "Stopped" => model.Stopped(),
            _ => throw new ArgumentException($"Unknown vector event: {playbackEvent}"),
        };

    private static JsonDocument LoadVectors(string fileName) =>
        JsonDocument.Parse(
            File.ReadAllText(Path.Combine(AppContext.BaseDirectory, fileName)));
}

public sealed class StatusPublishPolicyTests
{
    private long _now;

    /// <summary>
    /// Converts a duration to Stopwatch ticks with integer arithmetic only.
    /// Accumulating fractional seconds instead would let a step land a tick
    /// under a heartbeat boundary and make these assertions measure rounding.
    /// </summary>
    private static long Ticks(TimeSpan span) =>
        System.Diagnostics.Stopwatch.Frequency * span.Ticks / TimeSpan.TicksPerSecond;

    private void Advance(TimeSpan span) => _now += Ticks(span);

    private StatusPublishPolicy Policy(TimeSpan? heartbeat = null) =>
        new(heartbeat, () => _now);

    [Fact]
    public void AnUnchangedProjectionIsWrittenOncePerHeartbeatRatherThanPerFrame()
    {
        // The real driver is the decode loop and the audio worker, each
        // publishing an identical projection per frame. Every write is an fsync
        // and a rename on the SD-card-backed config volume.
        TimeSpan heartbeat = TimeSpan.FromMilliseconds(500);
        TimeSpan frameInterval = TimeSpan.FromMilliseconds(10);
        StatusPublishPolicy policy = Policy(heartbeat);
        PlaybackProjection playing = new("running", "running", "running", "Playing OMT video.");

        Assert.True(policy.ShouldPublish(playing, "HDMI-A-1"));

        int written = 0;
        for (int frame = 0; frame < 100; frame++)
        {
            Advance(frameInterval);
            if (policy.ShouldPublish(playing, "HDMI-A-1"))
            {
                written++;
            }
        }

        // One second of identical frames: two heartbeats, not a hundred writes.
        Assert.Equal(2, written);
    }

    [Fact]
    public void EveryChangeIsPublishedImmediatelyWithoutWaitingForTheHeartbeat()
    {
        StatusPublishPolicy policy = Policy(TimeSpan.FromSeconds(30));
        PlaybackProjection playing = new("running", "running", "running", "Playing OMT video.");
        Assert.True(policy.ShouldPublish(playing, "HDMI-A-1"));

        // A detail-only change still matters: it is the text the operator reads.
        PlaybackProjection interlaced = playing with { Detail = "Playing interlaced input." };
        Assert.True(policy.ShouldPublish(interlaced, "HDMI-A-1"));

        PlaybackProjection degraded =
            new("degraded", "running", "failed", "Audio unavailable.");
        Assert.True(policy.ShouldPublish(degraded, "HDMI-A-1"));

        // The same projection on a different output is a different fact.
        Assert.True(policy.ShouldPublish(degraded, "HDMI-A-2"));
        Assert.False(policy.ShouldPublish(degraded, "HDMI-A-2"));
    }

    [Fact]
    public void TheHeartbeatIsMeasuredFromTheLastWriteAndStaysInsideTheStaleWindow()
    {
        // OMT_PLAYBACK_STATUS_STALE_SECONDS accepts a minimum of 1, so the
        // default heartbeat has to leave room under even that setting or the
        // dashboard reads a throttled record as "Playback status stale".
        Assert.True(StatusPublishPolicy.DefaultHeartbeat < TimeSpan.FromSeconds(1));

        StatusPublishPolicy policy = Policy();
        TimeSpan half = StatusPublishPolicy.DefaultHeartbeat / 2;
        PlaybackProjection playing = new("running", "running", "running", "Playing OMT video.");
        Assert.True(policy.ShouldPublish(playing, "HDMI-A-1"));

        Advance(half);
        Assert.False(policy.ShouldPublish(playing, "HDMI-A-1"));

        // Suppressed calls must not restart the interval, or a busy loop would
        // hold the record just under the threshold forever and never refresh it.
        Advance(half);
        Assert.True(policy.ShouldPublish(playing, "HDMI-A-1"));
    }

    [Fact]
    public void AForcedPublishOverridesTheHeartbeatAndRestartsTheInterval()
    {
        // The shutdown path forces its terminal record so the file an operator
        // is left looking at carries a current timestamp. Going through the
        // policy rather than around it keeps that write inside the interval
        // bookkeeping, so the next heartbeat is measured from the real one.
        StatusPublishPolicy policy = Policy(TimeSpan.FromSeconds(10));
        PlaybackProjection stopped = new("stopped", "stopped", "stopped", "Playback stopped.");
        Assert.True(policy.ShouldPublish(stopped, "none"));
        Assert.False(policy.ShouldPublish(stopped, "none"));

        Advance(TimeSpan.FromSeconds(4));
        Assert.True(policy.ShouldPublish(stopped, "none", force: true));

        // Six more seconds is ten since the throttled call but only six since
        // the forced write, so the heartbeat must not be due yet.
        Advance(TimeSpan.FromSeconds(6));
        Assert.False(policy.ShouldPublish(stopped, "none"));

        Advance(TimeSpan.FromSeconds(4));
        Assert.True(policy.ShouldPublish(stopped, "none"));
    }
}

public sealed class RuntimePrimitiveTests : IDisposable
{
    private readonly string _root = Directory.CreateTempSubdirectory("omt-runtime").FullName;

    [Fact]
    public void InterruptibleWaitSlicesDelaysAndOffersEveryHeartbeat()
    {
        List<int> slices = [];
        int heartbeats = 0;

        InterruptibleWait.Run(
            250,
            () => true,
            () => heartbeats++,
            slices.Add);

        Assert.Equal([100, 100, 50], slices);
        Assert.Equal(3, heartbeats);

        bool running = true;
        InterruptibleWait.Run(
            500,
            () => running,
            delay: _ => running = false);
        Assert.Throws<ArgumentOutOfRangeException>(
            () => InterruptibleWait.Run(-1, () => true));
        Assert.Throws<ArgumentNullException>(
            () => InterruptibleWait.Run(1, null!));
    }

    [Fact]
    public void AtomicPublisherUsesAPrivateUniqueStageAndReplacesSymlinks()
    {
        string target = Path.Combine(_root, "status.json");
        string victim = Path.Combine(_root, "victim");
        File.WriteAllText(victim, "keep");
        File.CreateSymbolicLink(target, victim);

        AtomicFilePublisher.Replace(target, "first"u8);
        AtomicFilePublisher.Replace(target, "second"u8);

        Assert.Equal("second", File.ReadAllText(target));
        Assert.Equal("keep", File.ReadAllText(victim));
        Assert.Empty(Directory.GetFiles(_root, ".status.json.*"));
        if (OperatingSystem.IsLinux())
        {
            Assert.Equal(
                UnixFileMode.UserRead | UnixFileMode.UserWrite,
                File.GetUnixFileMode(target));
        }

        Assert.Throws<ArgumentException>(
            () => AtomicFilePublisher.Replace("", "x"u8));
        Assert.Throws<ArgumentException>(
            () => AtomicFilePublisher.Replace("status.json", "x"u8));
    }

    public void Dispose() => Directory.Delete(_root, true);
}

public sealed class HdmiConnectorTests : IDisposable
{
    private readonly string _root = Directory.CreateTempSubdirectory("omt-drm").FullName;

    private string DrmRoot => Path.Combine(_root, "sys");

    private string DeviceRoot => Path.Combine(_root, "dev");

    private HdmiConnectorLocator Locator => new(DrmRoot, DeviceRoot);

    [Fact]
    public void AutoPrefersTheFirstConnectedOutputAndItsAlsaDevice()
    {
        Publish("card1", "HDMI-A-1", "connected", "32");
        Publish("card1", "HDMI-A-2", "connected", "40");

        HdmiConnector connector = Assert.IsType<HdmiConnector>(Locator.Find("auto"));
        Assert.Equal("HDMI-A-1", connector.Name);
        Assert.Equal(32u, connector.ConnectorId);
        Assert.Equal(Path.Combine(DeviceRoot, "card1"), connector.DevicePath);
        Assert.Equal("plughw:CARD=vc4hdmi0,DEV=0", connector.AlsaDevice);
        Assert.True(connector.IsConnected());

        Assert.Equal(
            "plughw:CARD=vc4hdmi1,DEV=0",
            Assert.IsType<HdmiConnector>(Locator.Find("HDMI-A-2")).AlsaDevice);
    }

    [Fact]
    public void AutoFallsThroughToTheSecondOutputWhenTheFirstIsNotUsable()
    {
        Publish("card1", "HDMI-A-1", "disconnected", "32");
        Publish("card1", "HDMI-A-2", "connected", "40");

        Assert.Equal("HDMI-A-2", Assert.IsType<HdmiConnector>(Locator.Find("auto")).Name);
        Assert.Null(Locator.Find("HDMI-A-1"));
    }

    [Theory]
    [InlineData("connected", "0")]
    [InlineData("connected", "")]
    [InlineData("connected", "-1")]
    [InlineData("connected", "not-a-number")]
    [InlineData("unknown", "32")]
    public void UnusableSysfsAttributesSelectNothing(string status, string connectorId)
    {
        Publish("card1", "HDMI-A-1", status, connectorId);
        Assert.Null(Locator.Find("auto"));
    }

    [Fact]
    public void AnEmptySysfsAttributeReadsAsAbsent()
    {
        // sysfs can hand back an empty read while a hotplug is settling.
        Publish("card1", "HDMI-A-1", "connected", "32");
        File.WriteAllText(Path.Combine(DrmRoot, "card1-HDMI-A-1", "status"), "");
        Assert.Null(Locator.Find("auto"));
    }

    [Fact]
    public void AConnectorWithoutItsCardDeviceIsSkipped()
    {
        Publish("card1", "HDMI-A-1", "connected", "32", withCardDevice: false);
        Assert.Null(Locator.Find("auto"));
    }

    [Fact]
    public void AMissingDrmTreeIsNoDisplayRatherThanAFailure()
    {
        // The play loop treats null as "waiting for HDMI" and retries. An
        // exception here would instead terminate playback on any Pi that has
        // not bound its DRM driver yet.
        Assert.Null(Locator.Find("auto"));
        Assert.Null(new HdmiConnectorLocator("/nonexistent/drm", DeviceRoot).Find("auto"));
    }

    [Fact]
    public void AnUnreadableDrmTreeIsAlsoNoDisplayRatherThanAFailure()
    {
        Publish("card1", "HDMI-A-1", "connected", "32");
        if (!OperatingSystem.IsLinux() || Environment.IsPrivilegedProcess)
        {
            // The appliance is Linux-only, and mode bits do not apply to root,
            // so on either there is nothing to observe here.
            return;
        }

        string connector = Path.Combine(DrmRoot, "card1-HDMI-A-1");
        File.SetUnixFileMode(Path.Combine(connector, "status"), UnixFileMode.None);
        Assert.Null(Locator.Find("auto"));

        File.SetUnixFileMode(DrmRoot, UnixFileMode.None);
        try
        {
            Assert.Null(Locator.Find("auto"));
        }
        finally
        {
            File.SetUnixFileMode(
                DrmRoot,
                UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute);
        }
    }

    [Fact]
    public void AHotUnpluggedConnectorStopsReportingItselfAsConnected()
    {
        Publish("card1", "HDMI-A-1", "connected", "32");
        HdmiConnector connector = Assert.IsType<HdmiConnector>(Locator.Find("auto"));

        File.WriteAllText(Path.Combine(connector.SysfsPath, "status"), "disconnected\n");
        Assert.False(connector.IsConnected());

        File.WriteAllText(Path.Combine(connector.SysfsPath, "status"), "connected\n");
        File.WriteAllText(Path.Combine(connector.SysfsPath, "connector_id"), "40\n");
        Assert.False(connector.IsConnected());

        Directory.Delete(connector.SysfsPath, true);
        Assert.False(connector.IsConnected());
    }

    public void Dispose() => Directory.Delete(_root, true);

    private void Publish(
        string card,
        string name,
        string status,
        string connectorId,
        bool withCardDevice = true)
    {
        string path = Path.Combine(DrmRoot, $"{card}-{name}");
        Directory.CreateDirectory(path);
        File.WriteAllText(Path.Combine(path, "status"), status + "\n");
        File.WriteAllText(Path.Combine(path, "connector_id"), connectorId + "\n");
        Directory.CreateDirectory(DeviceRoot);
        if (withCardDevice)
        {
            File.WriteAllText(Path.Combine(DeviceRoot, card), "");
        }
    }
}
