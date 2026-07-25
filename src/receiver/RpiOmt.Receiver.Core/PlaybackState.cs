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

/// <summary>
/// Decides which status projections are worth committing to disk.
/// </summary>
/// <remarks>
/// Every decoded video and audio frame drives a projection, but publishing is a
/// write + fsync + rename. Status lives on a tmpfs in the shipped image, yet
/// an unthrottled publish at 1080p60 is still well over a hundred fsync cycles
/// a second -- a synchronous stall inside the presentation loop to restate a
/// record whose only changing field is its timestamp.
///
/// So a projection identical to the last published one is written only once per
/// heartbeat. Any change to the state, detail, or connector is published
/// immediately, because the dashboard must show transitions at once.
/// The heartbeat stays well inside OMT_PLAYBACK_STATUS_STALE_SECONDS, whose
/// minimum accepted value is one second, so the consumer never reads the record
/// as stale purely because of this throttle.
///
/// Callers must serialize access: this type is not thread-safe on its own.
/// </remarks>
public sealed class StatusPublishPolicy(TimeSpan? heartbeat = null, Func<long>? clock = null)
{
    public static readonly TimeSpan DefaultHeartbeat = TimeSpan.FromMilliseconds(500);

    private readonly TimeSpan _heartbeat = heartbeat ?? DefaultHeartbeat;
    private readonly Func<long> _clock = clock ?? System.Diagnostics.Stopwatch.GetTimestamp;
    private PlaybackProjection? _published;
    private string _connector = "";
    private long _publishedAt;

    /// <summary>
    /// Reports whether <paramref name="projection"/> should be written now, and
    /// records it as published when the answer is yes.
    /// </summary>
    /// <param name="force">
    /// Publish regardless of the heartbeat. The caller still goes through this
    /// method rather than around it, so a forced write also restarts the
    /// interval instead of leaving the policy describing an older record.
    /// </param>
    public bool ShouldPublish(
        PlaybackProjection projection,
        string connector,
        bool force = false)
    {
        long now = _clock();
        bool changed = _published is null ||
            _published != projection ||
            !string.Equals(_connector, connector, StringComparison.Ordinal);
        if (!force &&
            !changed &&
            System.Diagnostics.Stopwatch.GetElapsedTime(_publishedAt, now) < _heartbeat)
        {
            return false;
        }

        _published = projection;
        _connector = connector;
        _publishedAt = now;
        return true;
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
