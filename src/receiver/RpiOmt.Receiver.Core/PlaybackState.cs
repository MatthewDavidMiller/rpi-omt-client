using System.Text.Json;

namespace RpiOmt.Receiver.Core;

public sealed record PlaybackProjection(
    string State,
    string VideoState,
    string AudioState,
    string Detail);

public sealed class PlaybackStateModel
{
    private readonly object _sync = new();
    private string _videoState = "stopped";
    private string _audioState = "stopped";
    private string _videoDetail = "Playback stopped.";
    private string _audioDetail = "";

    public PlaybackProjection Snapshot()
    {
        lock (_sync)
        {
            return Project();
        }
    }

    public PlaybackProjection VideoStarting(string detail = "Waiting for OMT media.") =>
        SetVideo("starting", detail);

    public PlaybackProjection WaitingForDiscovery(string detail) =>
        SetVideo("waiting-for-discovery", detail);

    public PlaybackProjection WaitingForHdmi(string detail) =>
        SetVideo("waiting-for-hdmi", detail);

    public PlaybackProjection VideoRetrying(string detail) =>
        SetVideo("retrying", detail);

    public PlaybackProjection UnsupportedFormat(string detail) =>
        SetVideo("unsupported-format", detail);

    public PlaybackProjection VideoRunning(string detail) =>
        SetVideo("running", detail);

    public PlaybackProjection AudioRunning(string detail = "Playing OMT audio.")
    {
        lock (_sync)
        {
            _audioState = "running";
            _audioDetail = detail;
            return Project();
        }
    }

    public PlaybackProjection AudioFailed(string detail)
    {
        lock (_sync)
        {
            _audioState = "failed";
            _audioDetail = StatusSanitizer.Sanitize(detail);
            return Project();
        }
    }

    public PlaybackProjection AudioStopped()
    {
        lock (_sync)
        {
            _audioState = "stopped";
            _audioDetail = "";
            return Project();
        }
    }

    public PlaybackProjection Stopped(string detail = "Playback stopped.")
    {
        lock (_sync)
        {
            _videoState = "stopped";
            _audioState = "stopped";
            _videoDetail = detail;
            _audioDetail = "";
            return Project();
        }
    }

    private PlaybackProjection SetVideo(string state, string detail)
    {
        lock (_sync)
        {
            _videoState = state;
            _videoDetail = StatusSanitizer.Sanitize(detail);
            return Project();
        }
    }

    private PlaybackProjection Project()
    {
        if (_videoState == "running")
        {
            if (_audioState == "failed")
            {
                return new(
                    "degraded",
                    "running",
                    "failed",
                    string.IsNullOrEmpty(_audioDetail)
                        ? "Video is playing but audio is unavailable."
                        : _audioDetail);
            }
            return new("running", "running", _audioState, _videoDetail);
        }
        return new(_videoState, _videoState, _audioState, _videoDetail);
    }
}

public sealed record PlaybackStatusDocument(
    int Schema,
    string State,
    string VideoState,
    string AudioState,
    string Target,
    string Detail,
    string Connector,
    string DrmDevice,
    string AlsaDevice,
    DateTimeOffset UpdatedAt);

public static class StatusSerializer
{
    public static byte[] Serialize(PlaybackStatusDocument document) =>
        JsonSerializer.SerializeToUtf8Bytes(
            document,
            StatusJsonContext.Default.PlaybackStatusDocument);
}

[System.Text.Json.Serialization.JsonSerializable(typeof(PlaybackStatusDocument))]
[System.Text.Json.Serialization.JsonSourceGenerationOptions(
    PropertyNamingPolicy = System.Text.Json.Serialization.JsonKnownNamingPolicy.SnakeCaseLower)]
internal sealed partial class StatusJsonContext :
    System.Text.Json.Serialization.JsonSerializerContext;
