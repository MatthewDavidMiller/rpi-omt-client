// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#include "playback_status.hpp"

#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>
#include <unistd.h>

namespace {

void require(bool value, const char* message)
{
    if (!value) {
        std::cerr << "FAIL: " << message << '\n';
        std::exit(1);
    }
}

std::string read_file(const std::filesystem::path& path)
{
    std::ifstream input(path, std::ios::binary);
    return {std::istreambuf_iterator<char>(input), std::istreambuf_iterator<char>()};
}

} // namespace

int main()
{
    std::filesystem::path directory = std::filesystem::temp_directory_path() /
        ("omt-native-status-" + std::to_string(static_cast<long>(::getpid())));
    std::filesystem::create_directory(directory);
    std::filesystem::path status_path = directory / "playback-status.json";
    omt::native::Connector connector{
        "HDMI-A-1", "/dev/dri/card1", "/sys/class/drm/card1-HDMI-A-1", 32u,
        "plughw:CARD=vc4hdmi0,DEV=0"};
    omt::native::PlaybackStatus status(status_path.string(), "Camera");
    status.video_running("Playing OMT video.\n", &connector);
    status.audio_failed("Audio\nunavailable", &connector);
    std::string document = read_file(status_path);
    require(document.find("\"schema\":1") != std::string::npos, "schema is published");
    require(document.find("\"state\":\"degraded\"") != std::string::npos, "degraded state is projected");
    require(document.find("\"video_state\":\"running\"") != std::string::npos, "video remains running");
    require(document.find("\"audio_state\":\"failed\"") != std::string::npos, "audio failure is published");
    require(document.find("Audiounavailable") != std::string::npos, "detail controls are removed");
    require(document.find("\"connector\":\"HDMI-A-1\"") != std::string::npos, "connector is published");
    require(document.find("\"updated_at\":") != std::string::npos, "timestamp is published");
    status.stopped("Playback stopped.", nullptr);
    document = read_file(status_path);
    require(document.find("\"state\":\"stopped\"") != std::string::npos, "terminal state is forced");
    std::filesystem::remove_all(directory);
    std::cout << "native playback status contracts passed\n";
    return 0;
}
