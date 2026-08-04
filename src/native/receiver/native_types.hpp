// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#pragma once

#include "omt/omt_wire.h"

#include <array>
#include <chrono>
#include <cstddef>
#include <cstdint>
#include <string>
#include <vector>

namespace omt::native {

struct Endpoint {
    std::string host;
    std::uint16_t port{};
};

struct Source {
    std::string name;
    Endpoint endpoint;
};

struct Frame {
    omt_frame_header header{};
    omt_video_header video{};
    omt_audio_header audio{};
    std::vector<std::uint8_t> payload;
};

struct Connector {
    std::string name;
    std::string device_path;
    std::string sysfs_path;
    std::uint32_t connector_id{};
    std::string alsa_device;
};

using Clock = std::chrono::steady_clock;
using Deadline = Clock::time_point;

inline Deadline deadline_after(std::chrono::milliseconds duration)
{
    return Clock::now() + duration;
}

inline int remaining_milliseconds(Deadline deadline)
{
    auto remaining = std::chrono::duration_cast<std::chrono::milliseconds>(deadline - Clock::now());
    if (remaining.count() <= 0) {
        return 0;
    }
    if (remaining.count() > 60'000) {
        return 60'000;
    }
    return static_cast<int>(remaining.count());
}

} // namespace omt::native
