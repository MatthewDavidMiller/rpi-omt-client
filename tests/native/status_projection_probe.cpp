// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#include "playback_status.hpp"

#include <filesystem>
#include <fstream>
#include <iostream>
#include <string_view>
#include <unistd.h>

int main(int argc, char** argv)
{
    const auto directory = std::filesystem::temp_directory_path() /
                           ("omt-status-vector-" + std::to_string(static_cast<long>(::getpid())));
    std::filesystem::create_directory(directory);
    const auto path = directory / "status.json";
    omt::native::PlaybackStatus status(path.string(), "Camera");
    status.heartbeat(nullptr);
    for (int index = 1; index < argc; ++index) {
        const std::string_view event(argv[index]);
        if (event == "AudioRunning") status.audio_running("audio", nullptr);
        else if (event == "VideoStarting") status.video_starting("starting", nullptr);
        else if (event == "WaitingForDiscovery") status.waiting_for_discovery("discovery", nullptr);
        else if (event == "WaitingForHdmi") status.waiting_for_hdmi("hdmi", nullptr);
        else if (event == "VideoRetrying") status.video_retrying("retrying", nullptr);
        else if (event == "UnsupportedFormat") status.unsupported_format("unsupported", nullptr);
        else if (event == "VideoRunning") status.video_running("video", nullptr);
        else if (event == "AudioFailed") status.audio_failed("audio failed", nullptr);
        else if (event == "AudioStopped") status.audio_stopped(nullptr);
        else if (event == "Stopped") status.stopped("stopped", nullptr);
        else return 2;
    }
    std::ifstream input(path, std::ios::binary);
    std::cout << input.rdbuf();
    input.close();
    std::filesystem::remove_all(directory);
    return std::cout ? 0 : 1;
}
