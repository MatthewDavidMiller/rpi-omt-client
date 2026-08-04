// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#include "playback_status.hpp"

#include <algorithm>
#include <array>
#include <atomic>
#include <cerrno>
#include <cstdio>
#include <cstring>
#include <ctime>
#include <fcntl.h>
#include <filesystem>
#include <string_view>
#include <sys/stat.h>
#include <unistd.h>

namespace omt::native {
namespace {

constexpr auto heartbeat_interval = std::chrono::milliseconds(500);
constexpr std::size_t detail_limit = 2048u;
std::atomic_uint64_t stage_counter{};

void append_json_string(std::string& output, std::string_view value)
{
    output.push_back('"');
    constexpr char hex[] = "0123456789abcdef";
    for (char raw : value) {
        unsigned char ch = static_cast<unsigned char>(raw);
        switch (ch) {
        case '"': output += "\\\""; break;
        case '\\': output += "\\\\"; break;
        case '\b': output += "\\b"; break;
        case '\f': output += "\\f"; break;
        case '\n': output += "\\n"; break;
        case '\r': output += "\\r"; break;
        case '\t': output += "\\t"; break;
        default:
            if (ch < 0x20u) {
                output += "\\u00";
                output.push_back(hex[ch >> 4u]);
                output.push_back(hex[ch & 0x0fu]);
            } else {
                output.push_back(static_cast<char>(ch));
            }
            break;
        }
    }
    output.push_back('"');
}

std::string utc_timestamp()
{
    timespec now{};
    (void)::clock_gettime(CLOCK_REALTIME, &now);
    std::tm utc{};
    (void)::gmtime_r(&now.tv_sec, &utc);
    std::array<char, 40> output{};
    (void)std::snprintf(
        output.data(), output.size(),
        "%04d-%02d-%02dT%02d:%02d:%02d.%03ldZ",
        utc.tm_year + 1900, utc.tm_mon + 1, utc.tm_mday,
        utc.tm_hour, utc.tm_min, utc.tm_sec, now.tv_nsec / 1'000'000L);
    return output.data();
}

bool write_all(int fd, std::string_view value)
{
    std::size_t offset = 0u;
    while (offset < value.size()) {
        ssize_t written = ::write(fd, value.data() + offset, value.size() - offset);
        if (written > 0) {
            offset += static_cast<std::size_t>(written);
        } else if (written < 0 && errno == EINTR) {
            continue;
        } else {
            return false;
        }
    }
    return true;
}

void atomic_replace(const std::string& path, const std::string& content)
{
    std::filesystem::path destination(path);
    std::error_code error;
    std::filesystem::create_directories(destination.parent_path(), error);
    if (error) {
        return;
    }
    std::array<char, 80> suffix{};
    (void)std::snprintf(
        suffix.data(), suffix.size(), ".omt-status.%ld.%016llx",
        static_cast<long>(::getpid()),
        static_cast<unsigned long long>(stage_counter.fetch_add(1u, std::memory_order_relaxed)));
    std::filesystem::path stage = destination.parent_path() / suffix.data();
    int fd = ::open(stage.c_str(), O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW, 0600);
    if (fd < 0) {
        return;
    }
    bool committed = write_all(fd, content) && ::fsync(fd) == 0;
    if (::close(fd) != 0) {
        committed = false;
    }
    if (committed && ::rename(stage.c_str(), destination.c_str()) == 0) {
        int directory = ::open(destination.parent_path().c_str(), O_RDONLY | O_DIRECTORY | O_CLOEXEC);
        if (directory >= 0) {
            (void)::fsync(directory);
            (void)::close(directory);
        }
        return;
    }
    (void)::unlink(stage.c_str());
}

} // namespace

std::string sanitize_status_detail(std::string_view value)
{
    std::string result;
    result.reserve(std::min(value.size(), detail_limit));
    for (std::size_t index = 0u; index < value.size() && result.size() < detail_limit;) {
        unsigned char first = static_cast<unsigned char>(value[index]);
        std::size_t scalar_length = 1u;
        std::uint32_t scalar = first;
        std::uint32_t minimum = 0u;
        if (first < 0x80u) {
            scalar_length = 1u;
        } else if ((first & 0xe0u) == 0xc0u) {
            scalar_length = 2u;
            scalar = first & 0x1fu;
            minimum = 0x80u;
        } else if ((first & 0xf0u) == 0xe0u) {
            scalar_length = 3u;
            scalar = first & 0x0fu;
            minimum = 0x800u;
        } else if ((first & 0xf8u) == 0xf0u) {
            scalar_length = 4u;
            scalar = first & 0x07u;
            minimum = 0x10000u;
        } else {
            ++index;
            continue;
        }
        bool valid = index + scalar_length <= value.size();
        for (std::size_t offset = 1u; valid && offset < scalar_length; ++offset) {
            const auto next = static_cast<unsigned char>(value[index + offset]);
            valid = (next & 0xc0u) == 0x80u;
            scalar = (scalar << 6u) | (next & 0x3fu);
        }
        valid = valid && (scalar_length == 1u || scalar >= minimum) && scalar <= 0x10ffffu &&
                !(scalar >= 0xd800u && scalar <= 0xdfffu);
        if (!valid) {
            ++index;
            continue;
        }
        if (result.size() + scalar_length > detail_limit) break;
        if (scalar >= 0x20u && scalar != 0x7fu) {
            result.append(value.substr(index, scalar_length));
        }
        index += scalar_length;
    }
    std::size_t begin = result.find_first_not_of(" \t\r\n");
    if (begin == std::string::npos) {
        return {};
    }
    std::size_t end = result.find_last_not_of(" \t\r\n");
    return result.substr(begin, end - begin + 1u);
}

PlaybackStatus::PlaybackStatus(std::string path, std::string target)
    : path_(std::move(path)), target_(std::move(target))
{
}

void PlaybackStatus::set_video(
    std::string_view state,
    std::string_view detail,
    const Connector* connector)
{
    std::lock_guard lock(mutex_);
    if (video_state_ != state || video_detail_ != detail) {
        video_state_ = state;
        video_detail_ = sanitize_status_detail(detail);
    }
    publish_locked(connector, false);
}

void PlaybackStatus::video_starting(std::string_view detail, const Connector* connector)
{
    set_video("starting", detail, connector);
}

void PlaybackStatus::waiting_for_discovery(std::string_view detail, const Connector* connector)
{
    set_video("waiting-for-discovery", detail, connector);
}

void PlaybackStatus::waiting_for_hdmi(std::string_view detail, const Connector* connector)
{
    set_video("waiting-for-hdmi", detail, connector);
}

void PlaybackStatus::video_retrying(std::string_view detail, const Connector* connector)
{
    set_video("retrying", detail, connector);
}

void PlaybackStatus::unsupported_format(std::string_view detail, const Connector* connector)
{
    set_video("unsupported-format", detail, connector);
}

void PlaybackStatus::video_running(std::string_view detail, const Connector* connector)
{
    set_video("running", detail, connector);
}

void PlaybackStatus::audio_running(std::string_view detail, const Connector* connector)
{
    std::lock_guard lock(mutex_);
    if (audio_state_ != "running" || audio_detail_ != detail) {
        audio_state_ = "running";
        audio_detail_ = sanitize_status_detail(detail);
    }
    publish_locked(connector, false);
}

void PlaybackStatus::audio_failed(std::string_view detail, const Connector* connector)
{
    std::lock_guard lock(mutex_);
    if (audio_state_ != "failed" || audio_detail_ != detail) {
        audio_state_ = "failed";
        audio_detail_ = sanitize_status_detail(detail);
    }
    publish_locked(connector, false);
}

void PlaybackStatus::audio_stopped(const Connector* connector)
{
    std::lock_guard lock(mutex_);
    if (audio_state_ != "stopped" || !audio_detail_.empty()) {
        audio_state_ = "stopped";
        audio_detail_.clear();
    }
    publish_locked(connector, false);
}

void PlaybackStatus::heartbeat(const Connector* connector)
{
    std::lock_guard lock(mutex_);
    publish_locked(connector, false);
}

void PlaybackStatus::stopped(std::string_view detail, const Connector* connector)
{
    std::lock_guard lock(mutex_);
    video_state_ = "stopped";
    audio_state_ = "stopped";
    video_detail_ = sanitize_status_detail(detail);
    audio_detail_.clear();
    publish_locked(connector, true);
}

void PlaybackStatus::publish_locked(const Connector* connector, bool force)
{
    std::string_view state = video_state_;
    std::string_view video = video_state_;
    std::string_view audio = audio_state_;
    std::string_view detail = video_detail_;
    if (video_state_ == "running" && audio_state_ == "failed") {
        state = "degraded";
        detail = audio_detail_.empty()
                     ? std::string_view{"Video is playing but audio is unavailable."}
                     : std::string_view{audio_detail_};
    }
    std::string_view connector_name = connector == nullptr ? std::string_view{"none"}
                                                            : std::string_view{connector->name};
    Clock::time_point now = Clock::now();
    bool changed = state != published_state_ || video != published_video_ || audio != published_audio_ ||
                   detail != published_detail_ || connector_name != published_connector_;
    if (!force && !changed && published_at_ != Clock::time_point{} && now - published_at_ < heartbeat_interval) {
        return;
    }
    published_state_.assign(state);
    published_video_.assign(video);
    published_audio_.assign(audio);
    published_detail_.assign(detail);
    published_connector_.assign(connector_name);
    published_at_ = now;

    std::string document;
    document.reserve(1024u + detail.size());
    document += "{\"schema\":1,\"state\":";
    append_json_string(document, state);
    document += ",\"video_state\":";
    append_json_string(document, video);
    document += ",\"audio_state\":";
    append_json_string(document, audio);
    document += ",\"target\":";
    append_json_string(document, target_);
    document += ",\"detail\":";
    append_json_string(document, detail);
    document += ",\"connector\":";
    append_json_string(document, connector_name);
    document += ",\"drm_device\":";
    append_json_string(document, connector == nullptr ? "none" : connector->device_path);
    document += ",\"alsa_device\":";
    append_json_string(document, connector == nullptr ? "none" : connector->alsa_device);
    document += ",\"updated_at\":";
    append_json_string(document, utc_timestamp());
    document += '}';
    atomic_replace(path_, document);
}

} // namespace omt::native
