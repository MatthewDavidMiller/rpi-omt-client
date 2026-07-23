// Copyright (c) 2026 Matthew David Miller. All rights reserved.
// Playback code is derived from the MIT-licensed Open Media Transport
// omtplayer project. See THIRD_PARTY_NOTICES.txt.

using System.Diagnostics;
using System.Globalization;
using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Text.Json;
using libomtnet;
using omtplayer.drm;

namespace RpiOmt.Receiver;

internal static class Program
{
    private const int MaximumWaitMilliseconds = 60_000;
    private const string DefaultVersion = "unknown";

    private static volatile bool _running = true;

    public static int Main(string[] args)
    {
        Console.CancelKeyPress += (_, eventArgs) =>
        {
            eventArgs.Cancel = true;
            _running = false;
        };
        AppDomain.CurrentDomain.ProcessExit += (_, _) => _running = false;

        try
        {
            if (args.Length == 1 && args[0] == "--version")
            {
                Console.WriteLine(BuildVersion());
                return 0;
            }
            if (args.Length == 0)
            {
                return Usage();
            }

            return args[0] switch
            {
                "discover" => Discover(ParseOptions(args[1..])),
                "probe" => Probe(ParseOptions(args[1..])),
                "play" => Play(ParseOptions(args[1..])),
                _ => Usage(),
            };
        }
        catch (ArgumentException exception)
        {
            Console.Error.WriteLine(exception.Message);
            return 2;
        }
        catch (Exception exception)
        {
            Console.Error.WriteLine(exception);
            return 1;
        }
    }

    private static int Discover(Dictionary<string, string> options)
    {
        EnsureJsonOption(options);
        int waitMs = IntegerOption(options, "--wait-ms", 1_500, 0, MaximumWaitMilliseconds);
        if (!AvahiBusAvailable())
        {
            Console.WriteLine("[]");
            return 0;
        }
        using OMTDiscovery discovery = OMTDiscovery.GetInstance();
        Wait(waitMs);
        string[] names = discovery.GetAddresses()
            .Where(TargetValidator.IsValidDiscoveredName)
            .Distinct(StringComparer.Ordinal)
            .Order(StringComparer.Ordinal)
            .ToArray();

        using Utf8JsonWriter writer = new(Console.OpenStandardOutput(), JsonOptions);
        writer.WriteStartArray();
        foreach (string name in names)
        {
            writer.WriteStartObject();
            writer.WriteString("name", name);
            writer.WriteString("target", name);
            writer.WriteString("kind", "discovered");
            writer.WriteEndObject();
        }
        writer.WriteEndArray();
        writer.Flush();
        Console.WriteLine();
        return 0;
    }

    private static int Probe(Dictionary<string, string> options)
    {
        EnsureJsonOption(options);
        string target = RequiredOption(options, "--target");
        TargetValidator.Validate(target);
        int timeoutMs = IntegerOption(
            options, "--timeout-ms", 3_000, 1, MaximumWaitMilliseconds);
        Stopwatch timer = Stopwatch.StartNew();
        OMTMediaFrame frame = new();
        bool video = false;
        bool audio = false;
        int width = 0;
        int height = 0;
        double frameRate = 0;
        int channels = 0;
        int sampleRate = 0;
        string error = "";

        if (!AvahiBusAvailable())
        {
            error = "The Avahi system bus is unavailable.";
        }
        else
        {
            try
            {
                using OMTReceive receiver = new(
                    target,
                    OMTFrameType.Video | OMTFrameType.Audio,
                    OMTPreferredVideoFormat.BGRA,
                    OMTReceiveFlags.None);
                while (timer.ElapsedMilliseconds < timeoutMs && !(video && audio))
                {
                    int slice = Math.Min(100, timeoutMs - (int)timer.ElapsedMilliseconds);
                    if (!video && receiver.Receive(OMTFrameType.Video, slice, ref frame))
                    {
                        video = true;
                        width = frame.Width;
                        height = frame.Height;
                        frameRate = frame.FrameRate;
                    }
                    if (!audio && receiver.Receive(OMTFrameType.Audio, 1, ref frame))
                    {
                        audio = true;
                        channels = frame.Channels;
                        sampleRate = frame.SampleRate;
                    }
                }
            }
            catch (Exception exception)
            {
                error = exception.Message;
            }
        }

        using Utf8JsonWriter writer = new(Console.OpenStandardOutput(), JsonOptions);
        writer.WriteStartObject();
        writer.WriteBoolean("ok", video || audio);
        writer.WriteString("target", target);
        writer.WriteBoolean("video", video);
        writer.WriteBoolean("audio", audio);
        writer.WriteNumber("width", width);
        writer.WriteNumber("height", height);
        writer.WriteNumber("frame_rate", frameRate);
        writer.WriteNumber("channels", channels);
        writer.WriteNumber("sample_rate", sampleRate);
        writer.WriteString("error", error);
        writer.WriteEndObject();
        writer.Flush();
        Console.WriteLine();
        return video || audio ? 0 : 3;
    }

    private static int Play(Dictionary<string, string> options)
    {
        string target = RequiredOption(options, "--target");
        TargetValidator.Validate(target);
        string connectorPreference = options.GetValueOrDefault("--connector", "auto");
        if (connectorPreference is not ("auto" or "HDMI-A-1" or "HDMI-A-2"))
        {
            throw new ArgumentException(
                "--connector must be auto, HDMI-A-1, or HDMI-A-2.");
        }
        string statusFile = RequiredOption(options, "--status-file");
        PlaybackStatus status = new(statusFile, target);
        int retrySeconds = IntegerOption(options, "--retry-seconds", 2, 1, 30);

        while (_running)
        {
            if (!AvahiBusAvailable())
            {
                status.Publish("waiting-for-discovery", "waiting-for-discovery", "stopped",
                    "The Avahi system bus is unavailable.", null);
                Wait(1_000);
                continue;
            }
            ConnectorSelection? selection = ConnectorSelection.Find(connectorPreference);
            if (selection is null)
            {
                status.Publish("waiting-for-hdmi", "waiting-for-hdmi", "stopped",
                    "No supported HDMI display is connected.", null);
                Wait(1_000);
                continue;
            }

            try
            {
                RunSession(target, selection, status);
            }
            catch (Exception exception)
            {
                status.Publish("retrying", "retrying", status.AudioState,
                    PlaybackStatus.Sanitize(exception.Message), selection);
                Wait(retrySeconds * 1_000);
            }
        }

        status.Publish("stopped", "stopped", "stopped",
            "Playback stopped.", null);
        return 0;
    }

    private static bool AvahiBusAvailable()
    {
        string address = Environment.GetEnvironmentVariable("DBUS_SYSTEM_BUS_ADDRESS")
            ?? "unix:path=/run/dbus/system_bus_socket";
        const string prefix = "unix:path=";
        string? path = address
            .Split(';', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
            .FirstOrDefault(candidate => candidate.StartsWith(prefix, StringComparison.Ordinal));
        if (path is null)
        {
            return false;
        }
        path = path[prefix.Length..].Split(',', 2)[0];
        if (string.IsNullOrEmpty(path) || path.Length > 107)
        {
            return false;
        }
        try
        {
            using Socket socket = new(
                AddressFamily.Unix, SocketType.Stream, ProtocolType.Unspecified);
            socket.Connect(new UnixDomainSocketEndPoint(path));
            return true;
        }
        catch (SocketException)
        {
            return false;
        }
        catch (ArgumentException)
        {
            return false;
        }
    }

    private static void RunSession(
        string target,
        ConnectorSelection selection,
        PlaybackStatus status)
    {
        using DRMDevice device = new(selection.DevicePath);
        DRMConnector connector = device.GetConnectorById(selection.ConnectorId)
            ?? throw new InvalidOperationException("Selected HDMI connector is unavailable.");
        device.StartEvents();
        using OMTReceive receiver = new(
            target,
            OMTFrameType.Video | OMTFrameType.Audio,
            OMTPreferredVideoFormat.BGRA,
            OMTReceiveFlags.None);
        using AudioWorker audio = new(receiver, selection.AlsaDevice, status, selection);
        audio.Start();

        OMTMediaFrame frame = new();
        DRMPresenter? presenter = null;
        int width = 0;
        int height = 0;
        double frameRate = 0;
        bool interlaced = false;
        long lastFrame = Stopwatch.GetTimestamp();

        try
        {
            status.Publish("starting", "starting", "starting",
                "Waiting for OMT media.", selection);
            while (_running && selection.IsConnected())
            {
                if (!receiver.Receive(OMTFrameType.Video, 500, ref frame))
                {
                    if (Stopwatch.GetElapsedTime(lastFrame).TotalSeconds >= 5)
                    {
                        status.Publish("retrying", "retrying", status.AudioState,
                            "Waiting for video frames.", selection);
                    }
                    continue;
                }
                lastFrame = Stopwatch.GetTimestamp();

                bool nextInterlaced = frame.Flags.HasFlag(OMTVideoFlags.Interlaced);
                if (frame.Width > 1920 || frame.Height > 1080 || frame.FrameRate > 60.01)
                {
                    status.Publish("unsupported-format", "unsupported-format",
                        status.AudioState,
                        $"Unsupported video format {frame.Width}x{frame.Height} " +
                        $"{frame.FrameRate.ToString("0.##", CultureInfo.InvariantCulture)}fps.",
                        selection);
                    continue;
                }

                if (presenter is null || width != frame.Width || height != frame.Height ||
                    Math.Abs(frameRate - frame.FrameRate) > 0.001 ||
                    interlaced != nextInterlaced)
                {
                    if (presenter is not null)
                    {
                        device.SetPresenter(null);
                        presenter.Dispose();
                    }
                    width = frame.Width;
                    height = frame.Height;
                    frameRate = frame.FrameRate;
                    interlaced = nextInterlaced;
                    DRMMode? mode = connector.FindNearestMode(
                        width, height, frameRate, false);
                    if (mode is null)
                    {
                        presenter = null;
                        status.Publish("unsupported-format", "unsupported-format",
                            status.AudioState,
                            $"Display has no mode for {width}x{height}.", selection);
                        continue;
                    }
                    presenter = new DRMPresenter(device, connector, mode, 3);
                    device.SetPresenter(presenter);
                }

                presenter.Enqueue(frame.Data, frame.Stride);
                status.Publish(
                    status.AudioState == "failed" ? "degraded" : "running",
                    "running",
                    status.AudioState,
                    interlaced
                        ? "Playing interlaced input progressively without deinterlacing."
                        : "Playing OMT video.",
                    selection);
            }
        }
        finally
        {
            audio.Stop();
            if (presenter is not null)
            {
                device.SetPresenter(null);
                presenter.Dispose();
            }
        }
    }

    private static Dictionary<string, string> ParseOptions(string[] args)
    {
        Dictionary<string, string> options = new(StringComparer.Ordinal);
        for (int index = 0; index < args.Length; index++)
        {
            string key = args[index];
            if (!key.StartsWith("--", StringComparison.Ordinal))
            {
                throw new ArgumentException($"Unexpected argument: {key}");
            }
            if (key == "--json")
            {
                if (!options.TryAdd(key, "true"))
                {
                    throw new ArgumentException($"Duplicate option: {key}");
                }
                continue;
            }
            if (++index >= args.Length)
            {
                throw new ArgumentException($"Missing value for {key}.");
            }
            if (!options.TryAdd(key, args[index]))
            {
                throw new ArgumentException($"Duplicate option: {key}");
            }
        }
        return options;
    }

    private static string RequiredOption(
        IReadOnlyDictionary<string, string> options, string name) =>
        options.TryGetValue(name, out string? value) && value.Length > 0
            ? value
            : throw new ArgumentException($"{name} is required.");

    private static int IntegerOption(
        IReadOnlyDictionary<string, string> options,
        string name,
        int defaultValue,
        int minimum,
        int maximum)
    {
        if (!options.TryGetValue(name, out string? raw))
        {
            return defaultValue;
        }
        if (!int.TryParse(raw, NumberStyles.None, CultureInfo.InvariantCulture, out int value) ||
            value < minimum || value > maximum)
        {
            throw new ArgumentException(
                $"{name} must be between {minimum} and {maximum}.");
        }
        return value;
    }

    private static void EnsureJsonOption(
        IReadOnlyDictionary<string, string> options)
    {
        if (!options.ContainsKey("--json"))
        {
            throw new ArgumentException("--json is required.");
        }
    }

    private static void Wait(int milliseconds)
    {
        int remaining = milliseconds;
        while (_running && remaining > 0)
        {
            int slice = Math.Min(remaining, 100);
            Thread.Sleep(slice);
            remaining -= slice;
        }
    }

    private static string BuildVersion()
    {
        string? informational = typeof(Program).Assembly
            .GetCustomAttributes(false)
            .OfType<System.Reflection.AssemblyInformationalVersionAttribute>()
            .FirstOrDefault()?.InformationalVersion;
        return string.IsNullOrWhiteSpace(informational) ? DefaultVersion : informational;
    }

    private static int Usage()
    {
        Console.Error.WriteLine(
            "Usage: omt-receiver --version | discover --wait-ms N --json | " +
            "probe --target TARGET --timeout-ms N --json | " +
            "play --target TARGET --connector auto|HDMI-A-1|HDMI-A-2 " +
            "--status-file PATH");
        return 2;
    }

    private static readonly JsonWriterOptions JsonOptions = new()
    {
        Indented = false,
        SkipValidation = false,
    };
}

internal static class TargetValidator
{
    public static void Validate(string value)
    {
        if (value.StartsWith("omt://", StringComparison.Ordinal))
        {
            ValidateUrl(value);
            return;
        }
        if (!IsValidDiscoveredName(value))
        {
            throw new ArgumentException("Invalid OMT source name.");
        }
    }

    public static bool IsValidDiscoveredName(string value)
    {
        if (string.IsNullOrWhiteSpace(value) ||
            !value.IsNormalized(NormalizationForm.FormC) ||
            Encoding.UTF8.GetByteCount(value) > 63)
        {
            return false;
        }
        return value.All(character =>
        {
            UnicodeCategory category = CharUnicodeInfo.GetUnicodeCategory(character);
            return category is not (
                UnicodeCategory.Control or
                UnicodeCategory.Format or
                UnicodeCategory.LineSeparator or
                UnicodeCategory.ParagraphSeparator or
                UnicodeCategory.Surrogate);
        });
    }

    private static void ValidateUrl(string value)
    {
        if (!Uri.TryCreate(value, UriKind.Absolute, out Uri? uri) ||
            uri.Scheme != "omt" || uri.Port is < 1 or > 65535 ||
            string.IsNullOrEmpty(uri.Host) || !string.IsNullOrEmpty(uri.UserInfo) ||
            uri.AbsolutePath != "/" || !string.IsNullOrEmpty(uri.Query) ||
            !string.IsNullOrEmpty(uri.Fragment) ||
            uri.Host.Any(character => character > 0x7f || char.IsControl(character)))
        {
            throw new ArgumentException("Invalid OMT direct target.");
        }
        if (value.EndsWith("/", StringComparison.Ordinal))
        {
            throw new ArgumentException("OMT direct targets must not contain a path.");
        }
    }
}

internal sealed record ConnectorSelection(
    string Name,
    string DevicePath,
    string SysfsPath,
    uint ConnectorId,
    string AlsaDevice)
{
    public static ConnectorSelection? Find(string preference)
    {
        IEnumerable<string> names = preference == "auto"
            ? ["HDMI-A-1", "HDMI-A-2"]
            : [preference];
        foreach (string name in names)
        {
            foreach (string path in Directory.EnumerateDirectories(
                         "/sys/class/drm", $"card*-{name}").Order(StringComparer.Ordinal))
            {
                string status = ReadOneLine(Path.Combine(path, "status"));
                string connectorId = ReadOneLine(Path.Combine(path, "connector_id"));
                if (status != "connected" ||
                    !uint.TryParse(connectorId, NumberStyles.None,
                        CultureInfo.InvariantCulture, out uint id) || id == 0)
                {
                    continue;
                }
                string card = Path.GetFileName(path)[..^($"-{name}".Length)];
                string device = $"/dev/dri/{card}";
                if (!File.Exists(device))
                {
                    continue;
                }
                string alsa = name == "HDMI-A-1"
                    ? "plughw:CARD=vc4hdmi0,DEV=0"
                    : "plughw:CARD=vc4hdmi1,DEV=0";
                return new(name, device, path, id, alsa);
            }
        }
        return null;
    }

    public bool IsConnected() =>
        ReadOneLine(Path.Combine(SysfsPath, "status")) == "connected" &&
        ReadOneLine(Path.Combine(SysfsPath, "connector_id")) ==
        ConnectorId.ToString(CultureInfo.InvariantCulture);

    private static string ReadOneLine(string path)
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

internal sealed class PlaybackStatus
{
    private readonly object _sync = new();
    private readonly string _path;
    private readonly string _target;
    private string _audioState = "stopped";

    public PlaybackStatus(string path, string target)
    {
        _path = path;
        _target = target;
    }

    public string AudioState
    {
        get
        {
            lock (_sync)
            {
                return _audioState;
            }
        }
    }

    public void Publish(
        string state,
        string video,
        string audio,
        string detail,
        ConnectorSelection? connector)
    {
        lock (_sync)
        {
            _audioState = audio;
            string directory = Path.GetDirectoryName(_path) ??
                throw new InvalidOperationException("Status path has no directory.");
            Directory.CreateDirectory(directory);
            string temporary = $"{_path}.tmp.{Environment.ProcessId}";
            using (FileStream stream = new(
                       temporary, FileMode.Create, FileAccess.Write, FileShare.None))
            using (Utf8JsonWriter writer = new(stream))
            {
                writer.WriteStartObject();
                writer.WriteNumber("schema", 1);
                writer.WriteString("state", state);
                writer.WriteString("video_state", video);
                writer.WriteString("audio_state", audio);
                writer.WriteString("target", _target);
                writer.WriteString("detail", Sanitize(detail));
                writer.WriteString("connector", connector?.Name ?? "none");
                writer.WriteString("drm_device", connector?.DevicePath ?? "none");
                writer.WriteString("alsa_device", connector?.AlsaDevice ?? "none");
                writer.WriteString("updated_at", DateTimeOffset.UtcNow);
                writer.WriteEndObject();
                writer.Flush();
                stream.Flush(true);
            }
            File.Move(temporary, _path, true);
            File.SetUnixFileMode(_path, UnixFileMode.UserRead | UnixFileMode.UserWrite);
        }
    }

    public static string Sanitize(string value)
    {
        string result = new(value.Where(character =>
            !char.IsControl(character) || character == ' ').Take(512).ToArray());
        return result.Trim();
    }
}

internal sealed class AudioWorker : IDisposable
{
    private readonly OMTReceive _receiver;
    private readonly string _device;
    private readonly PlaybackStatus _status;
    private readonly ConnectorSelection _selection;
    private Thread? _thread;
    private volatile bool _running;

    public AudioWorker(
        OMTReceive receiver,
        string device,
        PlaybackStatus status,
        ConnectorSelection selection)
    {
        _receiver = receiver;
        _device = device;
        _status = status;
        _selection = selection;
    }

    public void Start()
    {
        _running = true;
        _thread = new Thread(Run) { IsBackground = true };
        _thread.Start();
    }

    public void Stop()
    {
        _running = false;
        _thread?.Join(1_000);
        _thread = null;
    }

    private void Run()
    {
        ALSAPlayer? player = null;
        OMTMediaFrame frame = new();
        try
        {
            while (_running)
            {
                if (!_receiver.Receive(OMTFrameType.Audio, 100, ref frame))
                {
                    continue;
                }
                if (player is null || player.Channels != frame.Channels ||
                    player.SampleRate != frame.SampleRate)
                {
                    player?.Dispose();
                    player = new ALSAPlayer(
                        _device,
                        (uint)frame.SampleRate,
                        (uint)frame.Channels,
                        ALSAPlayer.DEFAULT_LATENCY_MS);
                }
                if (player.GetBufferAvailable() >= frame.SamplesPerChannel)
                {
                    player.WritePlanar(frame.Data, (uint)frame.SamplesPerChannel);
                }
                _status.Publish("running", "running", "running",
                    "Playing OMT video and audio.", _selection);
            }
        }
        catch (Exception exception)
        {
            _status.Publish("degraded", "running", "failed",
                $"Audio unavailable: {PlaybackStatus.Sanitize(exception.Message)}",
                _selection);
        }
        finally
        {
            player?.Dispose();
        }
    }

    public void Dispose() => Stop();
}
