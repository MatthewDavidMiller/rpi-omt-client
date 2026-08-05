// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#pragma once

#include "native_types.hpp"

#include "vmxcodec.h"

#include <xf86drmMode.h>

#include <array>
#include <cstdint>
#include <optional>
#include <string>

namespace omt::native {

std::optional<Connector> find_connector(std::string_view preference);
bool connector_is_connected(const Connector& connector);

/// Why a frame did or did not reach the display.
///
/// `unsupported_format` is the one recoverable outcome: the stream is being
/// received correctly but this display cannot show it, so the caller reports it
/// and keeps the session. Anything else is a failure of the output itself and
/// ends the session. Classifying this here, rather than by searching `error()`
/// for words like "mode" or "format", keeps a genuine `drmModeSetCrtc` failure
/// -- whose message also contains "mode" -- from being retried once per frame.
enum class PresentOutcome { presented, unsupported_format, failed };

class DrmOutput final {
public:
    explicit DrmOutput(const Connector& connector);
    ~DrmOutput();
    DrmOutput(const DrmOutput&) = delete;
    DrmOutput& operator=(const DrmOutput&) = delete;

    [[nodiscard]] bool ready() const { return fd_ >= 0; }
    [[nodiscard]] const std::string& error() const { return error_; }
    [[nodiscard]] PresentOutcome present(Frame& frame);

private:
    struct Buffer {
        std::uint32_t handle{};
        std::uint32_t pitch{};
        std::uint64_t size{};
        std::uint32_t framebuffer{};
        std::uint8_t* mapping{};
    };

    PresentOutcome configure(const omt_video_header& header);
    bool create_buffer(Buffer& buffer);
    void destroy_buffer(Buffer& buffer);
    bool wait_for_flip();
    static void page_flip(
        int fd,
        unsigned int sequence,
        unsigned int seconds,
        unsigned int microseconds,
        void* data);

    Connector connector_;
    int fd_{-1};
    std::uint32_t crtc_id_{};
    drmModeModeInfo mode_{};
    std::array<Buffer, 3> buffers_{};
    std::size_t front_{};
    bool configured_{};
    bool flip_complete_{};
    int width_{};
    int height_{};
    int frame_rate_n_{};
    int frame_rate_d_{};
    int color_space_{};
    VMX_INSTANCE* codec_{};
    std::string error_;
};

} // namespace omt::native
