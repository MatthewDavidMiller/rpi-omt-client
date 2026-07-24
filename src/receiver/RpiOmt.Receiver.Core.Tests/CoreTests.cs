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
        using JsonDocument document = JsonDocument.Parse(
            File.ReadAllText(
                Path.Combine(AppContext.BaseDirectory, "omt-target-vectors.json")));
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
        using JsonDocument vectors = LoadPlaybackVectors();
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
        using JsonDocument vectors = LoadPlaybackVectors();
        HashSet<string> receiverStates = ReadStringSet(vectors, "receiver_states");
        HashSet<string> videoStates = ReadStringSet(vectors, "video_states");
        HashSet<string> audioStates = ReadStringSet(vectors, "audio_states");

        foreach (JsonElement vector in vectors.RootElement.GetProperty("projections")
                     .EnumerateArray())
        {
            string name = vector.GetProperty("name").GetString()!;
            PlaybackStateModel model = new();
            PlaybackProjection projection = model.Snapshot();
            foreach (JsonElement step in vector.GetProperty("events").EnumerateArray())
            {
                projection = Apply(model, step.GetString()!);
            }

            Assert.Equal(vector.GetProperty("state").GetString(), projection.State);
            Assert.Equal(vector.GetProperty("video_state").GetString(), projection.VideoState);
            Assert.Equal(vector.GetProperty("audio_state").GetString(), projection.AudioState);
            Assert.True(receiverStates.Contains(projection.State), name);
            Assert.True(videoStates.Contains(projection.VideoState), name);
            Assert.True(audioStates.Contains(projection.AudioState), name);
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

    private static HashSet<string> ReadStringSet(JsonDocument vectors, string property) =>
        vectors.RootElement.GetProperty(property)
            .EnumerateArray()
            .Select(value => value.GetString()!)
            .ToHashSet(StringComparer.Ordinal);

    private static JsonDocument LoadPlaybackVectors() =>
        JsonDocument.Parse(
            File.ReadAllText(
                Path.Combine(AppContext.BaseDirectory, "playback-status-vectors.json")));
}
