// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
// Playback code is derived from the MIT-licensed Open Media Transport
// omtplayer project. See THIRD_PARTY_NOTICES.txt.

using System.Diagnostics;
using System.Globalization;
using System.Net;
using System.Net.Sockets;
using System.Text.Json;
using libomtnet;
using omtplayer.drm;
using RpiOmt.Receiver.Core;

namespace RpiOmt.Receiver;

internal static class Program
{
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
            return ReceiverCli.Parse(args) switch
            {
                VersionCommand => PrintVersion(),
                DiscoverCommand command => Discover(command),
                ProbeCommand command => Probe(command),
                PlayCommand command => Play(command),
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

    private static int PrintVersion()
    {
        Console.WriteLine(BuildVersion());
        return 0;
    }

    private static int Discover(DiscoverCommand command)
    {
        if (!AvahiBusAvailable())
        {
            Console.WriteLine("[]");
            return 0;
        }
        using OMTDiscovery discovery = OMTDiscovery.GetInstance();
        Wait(command.WaitMilliseconds);
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

    private static int Probe(ProbeCommand command)
    {
        string target = command.Target;
        int timeoutMs = command.TimeoutMilliseconds;
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
                    // Audio is polled cheaply while the video receive above owns
                    // the slice, but once video has arrived nothing else blocks:
                    // a 1 ms wait would spin this loop a thousand times a second
                    // for the rest of the timeout against a video-only sender,
                    // burning a core inside the support-bundle budget.
                    if (!audio && receiver.Receive(
                            OMTFrameType.Audio, video ? slice : 1, ref frame))
                    {
                        audio = true;
                        channels = frame.Channels;
                        sampleRate = frame.SampleRate;
                    }
                }
            }
            catch (Exception exception)
            {
                error = StatusSanitizer.Sanitize(exception.Message);
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

    private static int Play(PlayCommand command)
    {
        string target = command.Target;
        string connectorPreference = command.Connector;
        PlaybackStatus status = new(command.StatusFile, target);
        HdmiConnectorLocator connectors = new();

        while (_running)
        {
            if (!AvahiBusAvailable())
            {
                status.WaitingForDiscovery(
                    "The Avahi system bus is unavailable.", null);
                Wait(1_000, () => status.Heartbeat(null));
                continue;
            }
            HdmiConnector? selection = connectors.Find(connectorPreference);
            if (selection is null)
            {
                status.WaitingForHdmi(
                    "No supported HDMI display is connected.", null);
                Wait(1_000, () => status.Heartbeat(null));
                continue;
            }

            try
            {
                RunSession(target, selection, status);
            }
            catch (Exception exception)
            {
                status.VideoRetrying(
                    StatusSanitizer.Sanitize(exception.Message), selection);
                Wait(
                    command.RetrySeconds * 1_000,
                    () => status.Heartbeat(selection));
            }
        }

        status.Stopped("Playback stopped.", null);
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
        HdmiConnector selection,
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
            status.VideoStarting("Waiting for OMT media.", selection);
            while (_running && selection.IsConnected())
            {
                if (!receiver.Receive(OMTFrameType.Video, 500, ref frame))
                {
                    status.Heartbeat(selection);
                    if (Stopwatch.GetElapsedTime(lastFrame).TotalSeconds >= 5)
                    {
                        status.VideoRetrying("Waiting for video frames.", selection);
                    }
                    continue;
                }
                lastFrame = Stopwatch.GetTimestamp();

                bool nextInterlaced = frame.Flags.HasFlag(OMTVideoFlags.Interlaced);
                if (!FormatPolicy.IsSupported(
                        frame.Width,
                        frame.Height,
                        frame.FrameRate))
                {
                    status.UnsupportedFormat(
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
                        // Drop the reference with the object. FindNearestMode and
                        // the DRMPresenter constructor below both allocate DRM
                        // resources and can throw; the finally block would then
                        // dispose this same presenter a second time.
                        presenter = null;
                    }
                    width = frame.Width;
                    height = frame.Height;
                    frameRate = frame.FrameRate;
                    interlaced = nextInterlaced;
                    DRMMode? mode = connector.FindNearestMode(
                        width, height, frameRate, false);
                    if (mode is null)
                    {
                        status.UnsupportedFormat(
                            $"Display has no mode for {width}x{height}.", selection);
                        continue;
                    }
                    presenter = new DRMPresenter(device, connector, mode, 3);
                    device.SetPresenter(presenter);
                }

                presenter.Enqueue(frame.Data, frame.Stride);
                status.VideoRunning(
                    interlaced
                        ? "Playing interlaced input progressively without deinterlacing."
                        : "Playing OMT video.",
                    selection);
            }
        }
        finally
        {
            audio.Stop();
            status.AudioStopped(selection);
            if (presenter is not null)
            {
                device.SetPresenter(null);
                presenter.Dispose();
            }
        }
    }

    private static void Wait(int milliseconds, Action? heartbeat = null) =>
        InterruptibleWait.Run(milliseconds, () => _running, heartbeat);

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

internal sealed class PlaybackStatus
{
    private readonly object _sync = new();
    private readonly string _path;
    private readonly string _target;
    private readonly PlaybackStateModel _model = new();
    private readonly StatusPublishPolicy _policy = new();

    public PlaybackStatus(string path, string target)
    {
        _path = path;
        _target = target;
    }

    public void VideoStarting(string detail, HdmiConnector? connector)
    {
        _model.VideoStarting(detail);
        Publish(connector);
    }

    public void WaitingForDiscovery(string detail, HdmiConnector? connector)
    {
        _model.WaitingForDiscovery(detail);
        Publish(connector);
    }

    public void WaitingForHdmi(string detail, HdmiConnector? connector)
    {
        _model.WaitingForHdmi(detail);
        Publish(connector);
    }

    public void VideoRetrying(string detail, HdmiConnector? connector)
    {
        _model.VideoRetrying(detail);
        Publish(connector);
    }

    public void UnsupportedFormat(string detail, HdmiConnector? connector)
    {
        _model.UnsupportedFormat(detail);
        Publish(connector);
    }

    public void VideoRunning(string detail, HdmiConnector? connector)
    {
        _model.VideoRunning(detail);
        Publish(connector);
    }

    public void AudioRunning(string detail, HdmiConnector? connector)
    {
        _model.AudioRunning(detail);
        Publish(connector);
    }

    public void AudioFailed(string detail, HdmiConnector? connector)
    {
        _model.AudioFailed(detail);
        Publish(connector);
    }

    public void AudioStopped(HdmiConnector? connector)
    {
        _model.AudioStopped();
        Publish(connector);
    }

    public void Heartbeat(HdmiConnector? connector) => Publish(connector);

    /// <summary>
    /// Publishes the terminal projection. This one bypasses the heartbeat
    /// throttle: it is the last record the process writes, so its timestamp has
    /// to be current or the dashboard reads a stale file after a clean exit.
    /// </summary>
    public void Stopped(string detail, HdmiConnector? connector)
    {
        _model.Stopped(detail);
        Publish(connector, force: true);
    }

    private void Publish(HdmiConnector? connector, bool force = false)
    {
        lock (_sync)
        {
            // Video and audio threads each mutate then publish. Snapshot under
            // this lock so an older projection cannot overwrite a newer one that
            // another thread already committed between the caller's mutate and
            // this critical section.
            PlaybackProjection current = _model.Snapshot();
            if (!_policy.ShouldPublish(current, connector?.Name ?? "none", force))
            {
                return;
            }

            PlaybackStatusDocument document = new(
                1,
                current.State,
                current.VideoState,
                current.AudioState,
                _target,
                current.Detail,
                connector?.Name ?? "none",
                connector?.DevicePath ?? "none",
                connector?.AlsaDevice ?? "none",
                DateTimeOffset.UtcNow);
            AtomicFilePublisher.Replace(_path, StatusSerializer.Serialize(document));
        }
    }
}

internal sealed class AudioWorker : IDisposable
{
    private readonly OMTReceive _receiver;
    private readonly string _device;
    private readonly PlaybackStatus _status;
    private readonly HdmiConnector _selection;
    private Thread? _thread;
    private volatile bool _running;

    public AudioWorker(
        OMTReceive receiver,
        string device,
        PlaybackStatus status,
        HdmiConnector selection)
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
                _status.AudioRunning("Playing OMT video and audio.", _selection);
            }
        }
        catch (Exception exception)
        {
            _status.AudioFailed(
                $"Audio unavailable: {StatusSanitizer.Sanitize(exception.Message)}",
                _selection);
        }
        finally
        {
            player?.Dispose();
        }
    }

    public void Dispose() => Stop();
}
