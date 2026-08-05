// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#pragma once

#include "native_types.hpp"

#include <chrono>
#include <mutex>
#include <string>
#include <string_view>

namespace omt::native {

class PlaybackStatus final {
public:
    PlaybackStatus(std::string path, std::string target);

    void video_starting(std::string_view detail, const Connector* connector);
    void waiting_for_discovery(std::string_view detail, const Connector* connector);
    void waiting_for_hdmi(std::string_view detail, const Connector* connector);
    void video_retrying(std::string_view detail, const Connector* connector);
    void unsupported_format(std::string_view detail, const Connector* connector);
    void video_running(std::string_view detail, const Connector* connector);
    void audio_running(std::string_view detail, const Connector* connector);
    void audio_failed(std::string_view detail, const Connector* connector);
    void audio_stopped(const Connector* connector);
    void heartbeat(const Connector* connector);
    void stopped(std::string_view detail, const Connector* connector);

private:
    void set_video(std::string_view state, std::string_view detail, const Connector* connector);
    void set_audio(std::string_view state, std::string_view detail, const Connector* connector);
    void publish_locked(const Connector* connector, bool force);

    std::mutex mutex_;
    std::string path_;
    std::string target_;
    std::string video_state_{"stopped"};
    std::string audio_state_{"stopped"};
    std::string video_detail_{"Playback stopped."};
    std::string audio_detail_;
    std::string published_state_;
    std::string published_video_;
    std::string published_audio_;
    std::string published_detail_;
    std::string published_connector_;
    Clock::time_point published_at_{};
};

std::string sanitize_status_detail(std::string_view value);

} // namespace omt::native
