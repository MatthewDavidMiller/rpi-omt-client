using System.Globalization;

namespace RpiOmt.Receiver.Core;

/// <summary>
/// One connected HDMI output and the devices that drive it.
/// </summary>
public sealed record HdmiConnector(
    string Name,
    string DevicePath,
    string SysfsPath,
    uint ConnectorId,
    string AlsaDevice)
{
    /// <summary>
    /// Reports whether this exact output is still connected. The connector id
    /// is re-checked as well, so a card renumbered by a hotplug event reads as
    /// disconnected rather than as this connector.
    /// </summary>
    public bool IsConnected() =>
        SysfsReader.ReadOneLine(Path.Combine(SysfsPath, "status")) == "connected" &&
        SysfsReader.ReadOneLine(Path.Combine(SysfsPath, "connector_id")) ==
        ConnectorId.ToString(CultureInfo.InvariantCulture);
}

/// <summary>
/// Bounds sysfs hotplug probes while preserving prompt disconnect detection.
/// </summary>
/// <remarks>
/// The video loop runs once per decoded frame. Reading both connector sysfs
/// attributes on every iteration would open and close roughly 120 files per
/// second at 60 fps even though hotplug state only needs sub-second precision.
/// Cache a positive result for one short interval; a negative result is still
/// returned as soon as the next scheduled probe runs and ends the session.
/// </remarks>
public sealed class HdmiConnectionMonitor
{
    public static readonly TimeSpan DefaultProbeInterval = TimeSpan.FromMilliseconds(500);

    private readonly Func<bool> _probe;
    private readonly TimeSpan _interval;
    private readonly Func<long> _clock;
    private bool _hasResult;
    private bool _connected;
    private long _checkedAt;

    public HdmiConnectionMonitor(
        Func<bool> probe,
        TimeSpan? interval = null,
        Func<long>? clock = null)
    {
        ArgumentNullException.ThrowIfNull(probe);
        _interval = interval ?? DefaultProbeInterval;
        if (_interval <= TimeSpan.Zero)
        {
            throw new ArgumentOutOfRangeException(nameof(interval));
        }

        _probe = probe;
        _clock = clock ?? System.Diagnostics.Stopwatch.GetTimestamp;
    }

    public bool IsConnected()
    {
        long now = _clock();
        if (_hasResult &&
            System.Diagnostics.Stopwatch.GetElapsedTime(_checkedAt, now) < _interval)
        {
            return _connected;
        }

        _connected = _probe();
        _checkedAt = now;
        _hasResult = true;
        return _connected;
    }
}

/// <summary>
/// Resolves an HDMI connector preference against the DRM sysfs tree. The roots
/// are injectable so the selection rules can be tested without Pi hardware.
/// </summary>
public sealed class HdmiConnectorLocator(
    string drmClassRoot = "/sys/class/drm",
    string deviceRoot = "/dev/dri")
{
    /// <summary>
    /// Returns the first usable connector for <paramref name="preference"/>
    /// ("auto", "HDMI-A-1", or "HDMI-A-2"), or null when none is ready.
    /// A missing or unreadable DRM tree is "none connected", not a failure:
    /// the play loop reports "waiting for HDMI" and retries.
    /// </summary>
    public HdmiConnector? Find(string preference)
    {
        IEnumerable<string> names = preference == "auto"
            ? ["HDMI-A-1", "HDMI-A-2"]
            : [preference];
        foreach (string name in names)
        {
            foreach (string path in CardDirectories(name))
            {
                HdmiConnector? connector = Select(name, path);
                if (connector is not null)
                {
                    return connector;
                }
            }
        }

        return null;
    }

    private HdmiConnector? Select(string name, string path)
    {
        string status = SysfsReader.ReadOneLine(Path.Combine(path, "status"));
        string connectorId = SysfsReader.ReadOneLine(Path.Combine(path, "connector_id"));
        if (status != "connected" ||
            !uint.TryParse(
                connectorId,
                NumberStyles.None,
                CultureInfo.InvariantCulture,
                out uint id) ||
            id == 0)
        {
            return null;
        }

        string card = Path.GetFileName(path)[..^($"-{name}".Length)];
        string device = Path.Combine(deviceRoot, card);
        if (!File.Exists(device))
        {
            return null;
        }

        string alsa = name == "HDMI-A-1"
            ? "plughw:CARD=vc4hdmi0,DEV=0"
            : "plughw:CARD=vc4hdmi1,DEV=0";
        return new(name, device, path, id, alsa);
    }

    private string[] CardDirectories(string name)
    {
        try
        {
            return Directory.EnumerateDirectories(drmClassRoot, $"card*-{name}")
                .Order(StringComparer.Ordinal)
                .ToArray();
        }
        catch (IOException)
        {
            return [];
        }
        catch (UnauthorizedAccessException)
        {
            return [];
        }
    }
}

internal static class SysfsReader
{
    /// <summary>
    /// Reads the first line of a sysfs attribute, treating any unreadable
    /// attribute as absent. These files disappear under hotplug, so a failure
    /// here is expected traffic rather than an error.
    /// </summary>
    public static string ReadOneLine(string path)
    {
        try
        {
            using StreamReader reader = new(path);
            return reader.ReadLine()?.Trim() ?? "";
        }
        catch (IOException)
        {
            return "";
        }
        catch (UnauthorizedAccessException)
        {
            return "";
        }
    }
}
