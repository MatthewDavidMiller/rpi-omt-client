// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
// Playback code is derived from the MIT-licensed Open Media Transport projects.
#include "alsa_output.hpp"
#include "discovery.hpp"
#include "drm_output.hpp"
#include "omt_channel.hpp"
#include "playback_status.hpp"

#include "omt/omt_wire.h"

#include <algorithm>
#include <atomic>
#include <charconv>
#include <chrono>
#include <csignal>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <map>
#include <optional>
#include <pthread.h>
#include <string>
#include <string_view>
#include <thread>
#include <vector>

#ifndef OMT_CLIENT_VERSION
#define OMT_CLIENT_VERSION "unknown"
#endif

namespace omt::native {
namespace {

std::atomic_bool running{true};
static_assert(std::atomic_bool::is_always_lock_free,
              "signal-driven shutdown requires a lock-free atomic flag");

void signal_handler(int)
{
    running.store(false, std::memory_order_relaxed);
}

void json_string(FILE* output, std::string_view value)
{
    (void)std::fputc('"', output);
    constexpr char hex[] = "0123456789abcdef";
    for (char raw : value) {
        unsigned char ch = static_cast<unsigned char>(raw);
        switch (ch) {
        case '"': (void)std::fputs("\\\"", output); break;
        case '\\': (void)std::fputs("\\\\", output); break;
        case '\b': (void)std::fputs("\\b", output); break;
        case '\f': (void)std::fputs("\\f", output); break;
        case '\n': (void)std::fputs("\\n", output); break;
        case '\r': (void)std::fputs("\\r", output); break;
        case '\t': (void)std::fputs("\\t", output); break;
        default:
            if (ch < 0x20u) {
                (void)std::fprintf(output, "\\u00%c%c", hex[ch >> 4u], hex[ch & 0x0fu]);
            } else {
                (void)std::fputc(ch, output);
            }
            break;
        }
    }
    (void)std::fputc('"', output);
}

struct Options {
    std::map<std::string, std::optional<std::string>, std::less<>> values;
};

bool parse_options(int argc, char** argv, int begin, Options& options, std::string& error)
{
    for (int index = begin; index < argc; ++index) {
        std::string key = argv[index];
        if (!key.starts_with("--")) {
            error = "Unexpected argument: " + key;
            return false;
        }
        bool flag = key == "--json";
        std::optional<std::string> value;
        if (!flag) {
            ++index;
            if (index >= argc) {
                error = "Missing value for " + key;
                return false;
            }
            value = argv[index];
        }
        if (!options.values.emplace(key, std::move(value)).second) {
            error = "Duplicate option: " + key;
            return false;
        }
    }
    return true;
}

bool allowed(const Options& options, std::initializer_list<std::string_view> names, std::string& error)
{
    for (const auto& [key, unused] : options.values) {
        (void)unused;
        if (std::find(names.begin(), names.end(), key) == names.end()) {
            error = "Option " + key + " is not valid for this command.";
            return false;
        }
    }
    return true;
}

bool flag_required(const Options& options, std::string_view name, std::string& error)
{
    auto found = options.values.find(name);
    if (found == options.values.end() || found->second.has_value()) {
        error = std::string(name) + " is required.";
        return false;
    }
    return true;
}

std::optional<std::string> required(
    const Options& options,
    std::string_view name,
    std::string& error)
{
    auto found = options.values.find(name);
    if (found == options.values.end() || !found->second.has_value() || found->second->empty()) {
        error = std::string(name) + " is required.";
        return std::nullopt;
    }
    return found->second;
}

std::optional<int> integer(
    const Options& options,
    std::string_view name,
    int default_value,
    int minimum,
    int maximum,
    std::string& error)
{
    auto found = options.values.find(name);
    if (found == options.values.end()) {
        return default_value;
    }
    if (!found->second.has_value()) {
        error = std::string(name) + " requires a value.";
        return std::nullopt;
    }
    int value = 0;
    const std::string& raw = *found->second;
    auto parsed = std::from_chars(raw.data(), raw.data() + raw.size(), value, 10);
    if (parsed.ec != std::errc{} || parsed.ptr != raw.data() + raw.size() ||
        value < minimum || value > maximum) {
        error = std::string(name) + " must be between " + std::to_string(minimum) +
                " and " + std::to_string(maximum) + ".";
        return std::nullopt;
    }
    return value;
}

bool valid_target(const std::string& target)
{
    omt_direct_target direct{};
    if (target.starts_with("omt://")) {
        return omt_parse_direct_target(target.c_str(), &direct);
    }
    return omt_is_valid_source_name_utf8(target.c_str());
}

bool direct_target(const std::string& target)
{
    omt_direct_target direct{};
    return omt_parse_direct_target(target.c_str(), &direct);
}

void interruptible_wait(int milliseconds, PlaybackStatus* status = nullptr, const Connector* connector = nullptr)
{
    int remaining = milliseconds;
    while (running.load(std::memory_order_relaxed) && remaining > 0) {
        int slice = std::min(100, remaining);
        std::this_thread::sleep_for(std::chrono::milliseconds(slice));
        remaining -= slice;
        if (status != nullptr) {
            status->heartbeat(connector);
        }
    }
}

int discover_command(const Options& options, std::string& error)
{
    if (!allowed(options, {"--wait-ms", "--json"}, error) ||
        !flag_required(options, "--json", error)) {
        return 2;
    }
    auto wait = integer(options, "--wait-ms", 1500, 0, 60'000, error);
    if (!wait.has_value()) {
        return 2;
    }
    std::vector<Source> sources = discover_sources(std::chrono::milliseconds(*wait));
    (void)std::fputc('[', stdout);
    for (std::size_t index = 0u; index < sources.size(); ++index) {
        if (index != 0u) {
            (void)std::fputc(',', stdout);
        }
        (void)std::fputs("{\"name\":", stdout);
        json_string(stdout, sources[index].name);
        (void)std::fputs(",\"target\":", stdout);
        json_string(stdout, sources[index].name);
        (void)std::fputs(",\"kind\":\"discovered\"}", stdout);
    }
    (void)std::fputs("]\n", stdout);
    return 0;
}

bool next_media_frame(
    OmtChannel& channel,
    omt_frame_type wanted,
    Frame& frame,
    Deadline deadline,
    std::string& error)
{
    while (remaining_milliseconds(deadline) > 0) {
        if (!channel.receive(frame, deadline, error)) {
            return false;
        }
        if (frame.header.type == wanted) {
            return true;
        }
    }
    error = "OMT media deadline expired";
    return false;
}

int probe_command(const Options& options, std::string& error)
{
    if (!allowed(options, {"--target", "--timeout-ms", "--json"}, error) ||
        !flag_required(options, "--json", error)) {
        return 2;
    }
    auto target = required(options, "--target", error);
    auto timeout = integer(options, "--timeout-ms", 3000, 1, 60'000, error);
    if (!target.has_value() || !timeout.has_value() || !valid_target(*target)) {
        if (error.empty()) {
            error = "Invalid OMT direct target.";
        }
        return 2;
    }
    std::optional<Endpoint> endpoint = resolve_target(*target, std::chrono::milliseconds(*timeout));
    Deadline deadline = deadline_after(std::chrono::milliseconds(*timeout));
    bool video = false;
    bool audio = false;
    int width = 0;
    int height = 0;
    double frame_rate = 0.0;
    int channels = 0;
    int sample_rate = 0;
    std::string probe_error;
    if (!endpoint.has_value()) {
        probe_error = "OMT target was not discovered.";
    } else {
        OmtChannel video_channel;
        OmtChannel audio_channel;
        std::string channel_error;
        bool video_connected = video_channel.connect(*endpoint, OMT_FRAME_VIDEO, deadline, channel_error);
        bool audio_connected = audio_channel.connect(*endpoint, OMT_FRAME_AUDIO, deadline, channel_error);
        Frame frame;
        while (remaining_milliseconds(deadline) > 0 && !(video && audio)) {
            Deadline slice = std::min(deadline, deadline_after(std::chrono::milliseconds(100)));
            if (!video && video_connected && next_media_frame(video_channel, OMT_FRAME_VIDEO, frame, slice, channel_error)) {
                video = true;
                width = frame.video.width;
                height = frame.video.height;
                frame_rate = static_cast<double>(frame.video.frame_rate_n) /
                             static_cast<double>(frame.video.frame_rate_d);
            }
            slice = std::min(deadline, deadline_after(std::chrono::milliseconds(video ? 100 : 1)));
            if (!audio && audio_connected && next_media_frame(audio_channel, OMT_FRAME_AUDIO, frame, slice, channel_error)) {
                audio = true;
                channels = frame.audio.channels;
                sample_rate = frame.audio.sample_rate;
            }
        }
        if (!(video || audio)) {
            probe_error = channel_error.empty() ? "No OMT media was received." : channel_error;
        }
    }
    (void)std::fputs("{\"ok\":", stdout);
    (void)std::fputs(video || audio ? "true" : "false", stdout);
    (void)std::fputs(",\"target\":", stdout);
    json_string(stdout, *target);
    (void)std::fprintf(
        stdout,
        ",\"video\":%s,\"audio\":%s,\"width\":%d,\"height\":%d,"
        "\"frame_rate\":%.8g,\"channels\":%d,\"sample_rate\":%d,\"error\":",
        video ? "true" : "false", audio ? "true" : "false", width, height,
        frame_rate, channels, sample_rate);
    json_string(stdout, sanitize_status_detail(probe_error));
    (void)std::fputs("}\n", stdout);
    return video || audio ? 0 : 3;
}

class AudioWorker final {
public:
    AudioWorker(Endpoint endpoint, const Connector& connector, PlaybackStatus& status)
        : endpoint_(std::move(endpoint)), connector_(connector), status_(status)
    {
    }

    bool start()
    {
        active_.store(true, std::memory_order_relaxed);
        pthread_attr_t attributes;
        if (pthread_attr_init(&attributes) != 0) {
            active_.store(false, std::memory_order_relaxed);
            status_.audio_failed("Audio unavailable: unable to initialize worker attributes.",
                                 &connector_);
            return false;
        }
        const long minimum_stack = PTHREAD_STACK_MIN;
        const std::size_t stack_size = std::max<std::size_t>(
            512U * 1024U,
            minimum_stack > 0 ? static_cast<std::size_t>(minimum_stack) : 0U);
        const int stack_result = pthread_attr_setstacksize(&attributes, stack_size);
        const int create_result = stack_result == 0
                                      ? pthread_create(&thread_, &attributes, &AudioWorker::entry, this)
                                      : stack_result;
        (void)pthread_attr_destroy(&attributes);
        if (create_result != 0) {
            active_.store(false, std::memory_order_relaxed);
            status_.audio_failed("Audio unavailable: unable to create bounded-stack worker.",
                                 &connector_);
            return false;
        }
        started_ = true;
        return true;
    }

    void stop()
    {
        active_.store(false, std::memory_order_relaxed);
        if (started_) {
            (void)pthread_join(thread_, nullptr);
            started_ = false;
        }
    }

    ~AudioWorker() { stop(); }

private:
    static void* entry(void* context)
    {
        static_cast<AudioWorker*>(context)->run();
        return nullptr;
    }

    void run()
    {
        while (active_.load(std::memory_order_relaxed) && running.load(std::memory_order_relaxed)) {
            OmtChannel channel;
            std::string error;
            if (channel.connect(
                    endpoint_, OMT_FRAME_AUDIO, deadline_after(std::chrono::seconds(3)), error)) {
                AlsaOutput output;
                Frame frame;
                while (active_.load(std::memory_order_relaxed) &&
                       running.load(std::memory_order_relaxed)) {
                    Deadline deadline = deadline_after(std::chrono::milliseconds(100));
                    if (!next_media_frame(channel, OMT_FRAME_AUDIO, frame, deadline, error)) {
                        if (channel.connected()) {
                            continue;
                        }
                        break;
                    }
                    if (!output.write(frame, connector_.alsa_device, error)) {
                        break;
                    }
                    status_.audio_running("Playing OMT video and audio.", &connector_);
                }
            }
            if (active_.load(std::memory_order_relaxed) && running.load(std::memory_order_relaxed)) {
                status_.audio_failed("Audio unavailable: " + sanitize_status_detail(error), &connector_);
                for (int attempt = 0; attempt < 10 && active_.load(std::memory_order_relaxed) &&
                                      running.load(std::memory_order_relaxed); ++attempt) {
                    std::this_thread::sleep_for(std::chrono::milliseconds(100));
                }
            }
        }
        status_.audio_stopped(&connector_);
    }

    Endpoint endpoint_;
    Connector connector_;
    PlaybackStatus& status_;
    std::atomic_bool active_{};
    pthread_t thread_{};
    bool started_{};
};

bool run_session(
    const std::string& target,
    const Connector& connector,
    PlaybackStatus& status,
    std::string& error)
{
    std::optional<Endpoint> endpoint = resolve_target(target, std::chrono::milliseconds(1500));
    if (!endpoint.has_value()) {
        error = "OMT target was not discovered.";
        return false;
    }
    DrmOutput output(connector);
    if (!output.ready()) {
        error = output.error();
        return false;
    }
    OmtChannel video;
    if (!video.connect(*endpoint, OMT_FRAME_VIDEO, deadline_after(std::chrono::seconds(3)), error)) {
        return false;
    }
    AudioWorker audio(*endpoint, connector, status);
    (void)audio.start();
    status.video_starting("Waiting for OMT media.", &connector);
    Frame frame;
    Clock::time_point last_frame = Clock::now();
    Clock::time_point last_connector_check{};
    while (running.load(std::memory_order_relaxed)) {
        Clock::time_point now = Clock::now();
        if (last_connector_check == Clock::time_point{} || now - last_connector_check >= std::chrono::milliseconds(500)) {
            last_connector_check = now;
            if (!connector_is_connected(connector)) {
                error = "HDMI display disconnected.";
                break;
            }
        }
        std::string receive_error;
        if (!next_media_frame(
                video, OMT_FRAME_VIDEO, frame,
                deadline_after(std::chrono::milliseconds(500)), receive_error)) {
            status.heartbeat(&connector);
            if (!video.connected()) {
                error = receive_error;
                break;
            }
            if (Clock::now() - last_frame >= std::chrono::seconds(5)) {
                status.video_retrying("Waiting for video frames.", &connector);
            }
            continue;
        }
        last_frame = Clock::now();
        if (!output.present(frame)) {
            std::string detail = output.error();
            if (detail.find("mode") != std::string::npos || detail.find("format") != std::string::npos) {
                status.unsupported_format(detail, &connector);
                continue;
            }
            error = detail;
            break;
        }
        bool interlaced = (frame.video.flags & 1u) != 0u;
        status.video_running(
            interlaced
                ? "Playing interlaced input progressively without deinterlacing."
                : "Playing OMT video.",
            &connector);
    }
    audio.stop();
    status.audio_stopped(&connector);
    return error.empty();
}

int play_command(const Options& options, std::string& error)
{
    if (!allowed(options, {"--target", "--connector", "--status-file", "--retry-seconds"}, error)) {
        return 2;
    }
    auto target = required(options, "--target", error);
    auto status_file = required(options, "--status-file", error);
    auto retry = integer(options, "--retry-seconds", 2, 1, 30, error);
    std::string connector_preference = "auto";
    auto connector_option = options.values.find("--connector");
    if (connector_option != options.values.end() && connector_option->second.has_value()) {
        connector_preference = *connector_option->second;
    }
    if (!target.has_value() || !status_file.has_value() || !retry.has_value() || !valid_target(*target) ||
        (connector_preference != "auto" && connector_preference != "HDMI-A-1" &&
         connector_preference != "HDMI-A-2")) {
        if (error.empty()) {
            error = "Invalid play options.";
        }
        return 2;
    }
    PlaybackStatus status(*status_file, *target);
    while (running.load(std::memory_order_relaxed)) {
        if (!direct_target(*target) && !discovery_transport_available()) {
            status.waiting_for_discovery("No configured OMT discovery transport is available.", nullptr);
            interruptible_wait(1000, &status, nullptr);
            continue;
        }
        std::optional<Connector> connector = find_connector(connector_preference);
        if (!connector.has_value()) {
            status.waiting_for_hdmi("No supported HDMI display is connected.", nullptr);
            interruptible_wait(1000, &status, nullptr);
            continue;
        }
        std::string session_error;
        if (!run_session(*target, *connector, status, session_error) && running.load(std::memory_order_relaxed)) {
            status.video_retrying(sanitize_status_detail(session_error), &*connector);
            interruptible_wait(*retry * 1000, &status, &*connector);
        }
    }
    status.stopped("Playback stopped.", nullptr);
    return 0;
}

int usage()
{
    (void)std::fputs(
        "Usage: omt-receiver --version | discover --wait-ms N --json | "
        "probe --target TARGET --timeout-ms N --json | "
        "play --target TARGET --connector auto|HDMI-A-1|HDMI-A-2 --status-file PATH\n",
        stderr);
    return 2;
}

} // namespace
} // namespace omt::native

int main(int argc, char** argv)
{
    using namespace omt::native;
    std::signal(SIGINT, signal_handler);
    std::signal(SIGTERM, signal_handler);
    if (argc == 2 && std::strcmp(argv[1], "--version") == 0) {
        (void)std::puts(OMT_CLIENT_VERSION);
        return 0;
    }
    if (argc < 2) {
        return usage();
    }
    Options options;
    std::string error;
    if (!parse_options(argc, argv, 2, options, error)) {
        (void)std::fprintf(stderr, "%s\n", error.c_str());
        return 2;
    }
    int result = 2;
    std::string command = argv[1];
    if (command == "discover") {
        result = discover_command(options, error);
    } else if (command == "probe") {
        result = probe_command(options, error);
    } else if (command == "play") {
        result = play_command(options, error);
    } else {
        return usage();
    }
    if (result == 2 && !error.empty()) {
        (void)std::fprintf(stderr, "%s\n", error.c_str());
    }
    return result;
}
