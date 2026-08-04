// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#include "drm_output.hpp"

#include "omt/omt_wire.h"

#include <drm.h>
#include <drm_fourcc.h>
#include <xf86drm.h>

#include <algorithm>
#include <array>
#include <cerrno>
#include <cmath>
#include <cstring>
#include <fcntl.h>
#include <filesystem>
#include <fstream>
#include <poll.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>

namespace omt::native {
namespace {

#if defined(__GLIBC__)
using IoctlRequest = unsigned long;
#else
// musl follows the POSIX `int request` prototype while glibc exposes the
// Linux kernel request word as unsigned long.
using IoctlRequest = int;
#endif

constexpr IoctlRequest ioctl_request(unsigned long request) noexcept
{
    return static_cast<IoctlRequest>(request);
}

std::string read_line(const std::filesystem::path& path)
{
    std::ifstream input(path);
    std::string value;
    if (!input || !std::getline(input, value)) {
        return {};
    }
    while (!value.empty() && (value.back() == '\r' || value.back() == '\n' || value.back() == ' ')) {
        value.pop_back();
    }
    return value;
}

double refresh_rate(const drmModeModeInfo& mode)
{
    if (mode.htotal == 0u || mode.vtotal == 0u) {
        return 0.0;
    }
    return static_cast<double>(mode.clock) * 1000.0 /
           (static_cast<double>(mode.htotal) * static_cast<double>(mode.vtotal));
}

bool close_enough(double left, double right)
{
    return std::abs(left - right) < 0.02;
}

} // namespace

std::optional<Connector> find_connector(std::string_view preference)
{
    std::array<std::string_view, 2> automatic{"HDMI-A-1", "HDMI-A-2"};
    std::size_t count = preference == "auto" ? automatic.size() : 1u;
    for (std::size_t index = 0u; index < count; ++index) {
        std::string_view name = preference == "auto" ? automatic[index] : preference;
        std::error_code error;
        std::filesystem::directory_iterator entries("/sys/class/drm", error);
        if (error) {
            return std::nullopt;
        }
        std::vector<std::filesystem::path> matches;
        for (const auto& entry : entries) {
            std::string filename = entry.path().filename().string();
            std::string suffix = "-" + std::string(name);
            if (filename.starts_with("card") && filename.ends_with(suffix)) {
                matches.push_back(entry.path());
            }
        }
        std::sort(matches.begin(), matches.end());
        for (const auto& path : matches) {
            if (read_line(path / "status") != "connected") {
                continue;
            }
            std::string id_text = read_line(path / "connector_id");
            char* end = nullptr;
            errno = 0;
            unsigned long id = std::strtoul(id_text.c_str(), &end, 10);
            if (errno != 0 || end == id_text.c_str() || *end != '\0' || id == 0ul || id > UINT32_MAX) {
                continue;
            }
            std::string filename = path.filename().string();
            std::string card = filename.substr(0u, filename.size() - name.size() - 1u);
            std::string device = "/dev/dri/" + card;
            if (!std::filesystem::exists(device, error) || error) {
                continue;
            }
            std::string alsa = name == "HDMI-A-1"
                ? "plughw:CARD=vc4hdmi0,DEV=0"
                : "plughw:CARD=vc4hdmi1,DEV=0";
            return Connector{std::string(name), device, path.string(), static_cast<std::uint32_t>(id), alsa};
        }
    }
    return std::nullopt;
}

bool connector_is_connected(const Connector& connector)
{
    return read_line(std::filesystem::path(connector.sysfs_path) / "status") == "connected" &&
           read_line(std::filesystem::path(connector.sysfs_path) / "connector_id") ==
               std::to_string(connector.connector_id);
}

DrmOutput::DrmOutput(const Connector& connector) : connector_(connector)
{
    fd_ = ::open(connector.device_path.c_str(), O_RDWR | O_CLOEXEC | O_NOFOLLOW);
    if (fd_ < 0) {
        error_ = std::string("Failed to open DRM device: ") + std::strerror(errno);
        return;
    }
    std::uint64_t dumb = 0u;
    if (drmGetCap(fd_, DRM_CAP_DUMB_BUFFER, &dumb) != 0 || dumb == 0u) {
        error_ = "DRM device does not support dumb buffers";
        (void)::close(fd_);
        fd_ = -1;
    }
}

DrmOutput::~DrmOutput()
{
    if (codec_ != nullptr) {
        VMX_Destroy(codec_);
    }
    for (Buffer& buffer : buffers_) {
        destroy_buffer(buffer);
    }
    if (fd_ >= 0) {
        (void)::close(fd_);
    }
}

void DrmOutput::destroy_buffer(Buffer& buffer)
{
    if (buffer.mapping != nullptr) {
        (void)::munmap(buffer.mapping, buffer.size);
    }
    if (fd_ >= 0 && buffer.framebuffer != 0u) {
        (void)drmModeRmFB(fd_, buffer.framebuffer);
    }
    if (fd_ >= 0 && buffer.handle != 0u) {
        drm_mode_destroy_dumb destroy{};
        destroy.handle = buffer.handle;
        (void)::ioctl(fd_, ioctl_request(DRM_IOCTL_MODE_DESTROY_DUMB), &destroy);
    }
    buffer = Buffer{};
}

bool DrmOutput::create_buffer(Buffer& buffer)
{
    drm_mode_create_dumb create{};
    create.width = mode_.hdisplay;
    create.height = mode_.vdisplay;
    create.bpp = 32u;
    if (::ioctl(fd_, ioctl_request(DRM_IOCTL_MODE_CREATE_DUMB), &create) != 0) {
        error_ = std::string("Unable to create DRM buffer: ") + std::strerror(errno);
        return false;
    }
    buffer.handle = create.handle;
    buffer.pitch = create.pitch;
    buffer.size = create.size;
    std::array<std::uint32_t, 4> handles{buffer.handle, 0u, 0u, 0u};
    std::array<std::uint32_t, 4> pitches{buffer.pitch, 0u, 0u, 0u};
    std::array<std::uint32_t, 4> offsets{};
    if (drmModeAddFB2(
            fd_, create.width, create.height, DRM_FORMAT_XRGB8888,
            handles.data(), pitches.data(), offsets.data(), &buffer.framebuffer, 0u) != 0) {
        error_ = std::string("Unable to register DRM framebuffer: ") + std::strerror(errno);
        destroy_buffer(buffer);
        return false;
    }
    drm_mode_map_dumb mapping{};
    mapping.handle = buffer.handle;
    if (::ioctl(fd_, ioctl_request(DRM_IOCTL_MODE_MAP_DUMB), &mapping) != 0) {
        error_ = std::string("Unable to map DRM buffer: ") + std::strerror(errno);
        destroy_buffer(buffer);
        return false;
    }
    if (mapping.offset > static_cast<std::uint64_t>(INT64_MAX)) {
        error_ = "DRM mapping offset is out of range";
        destroy_buffer(buffer);
        return false;
    }
    void* address = ::mmap(
        nullptr, buffer.size, PROT_READ | PROT_WRITE, MAP_SHARED, fd_,
        static_cast<off_t>(mapping.offset));
    if (address == MAP_FAILED) {
        error_ = std::string("Unable to map DRM buffer memory: ") + std::strerror(errno);
        destroy_buffer(buffer);
        return false;
    }
    buffer.mapping = static_cast<std::uint8_t*>(address);
    std::memset(buffer.mapping, 0, buffer.size);
    return true;
}

bool DrmOutput::configure(const omt_video_header& header)
{
    drmModeConnector* connector = drmModeGetConnector(fd_, connector_.connector_id);
    if (connector == nullptr || connector->connection != DRM_MODE_CONNECTED || connector->encoder_id == 0u) {
        error_ = "Selected HDMI connector is unavailable";
        if (connector != nullptr) {
            drmModeFreeConnector(connector);
        }
        return false;
    }
    drmModeEncoder* encoder = drmModeGetEncoder(fd_, connector->encoder_id);
    if (encoder == nullptr || encoder->crtc_id == 0u) {
        error_ = "Selected HDMI encoder is unavailable";
        if (encoder != nullptr) {
            drmModeFreeEncoder(encoder);
        }
        drmModeFreeConnector(connector);
        return false;
    }
    double requested = static_cast<double>(header.frame_rate_n) / static_cast<double>(header.frame_rate_d);
    const drmModeModeInfo* selected = nullptr;
    for (int pass = 0; pass < 3 && selected == nullptr; ++pass) {
        for (int index = 0; index < connector->count_modes; ++index) {
            const drmModeModeInfo& candidate = connector->modes[index];
            if (candidate.hdisplay != header.width || candidate.vdisplay != header.height ||
                (candidate.flags & DRM_MODE_FLAG_INTERLACE) != 0u) {
                continue;
            }
            double rate = refresh_rate(candidate);
            bool match = pass == 0 ? close_enough(rate, requested)
                         : pass == 1 ? close_enough(rate, std::round(requested))
                                     : close_enough(rate, 60.0);
            if (match) {
                selected = &candidate;
                break;
            }
        }
    }
    if (selected == nullptr) {
        error_ = "Display has no mode for the OMT video format";
        drmModeFreeEncoder(encoder);
        drmModeFreeConnector(connector);
        return false;
    }
    for (Buffer& buffer : buffers_) {
        destroy_buffer(buffer);
    }
    if (codec_ != nullptr) {
        VMX_Destroy(codec_);
        codec_ = nullptr;
    }
    mode_ = *selected;
    crtc_id_ = encoder->crtc_id;
    drmModeFreeEncoder(encoder);
    drmModeFreeConnector(connector);
    for (Buffer& buffer : buffers_) {
        if (!create_buffer(buffer)) {
            return false;
        }
    }
    std::uint32_t connector_id = connector_.connector_id;
    if (drmModeSetCrtc(
            fd_, crtc_id_, buffers_[0].framebuffer, 0u, 0u,
            &connector_id, 1, &mode_) != 0) {
        error_ = std::string("Unable to set DRM mode: ") + std::strerror(errno);
        return false;
    }
    VMX_SIZE dimensions{header.width, header.height};
    codec_ = VMX_Create(dimensions, VMX_PROFILE_OMT_SQ, static_cast<VMX_COLORSPACE>(header.color_space));
    if (codec_ == nullptr) {
        error_ = "Unable to create VMX decoder";
        return false;
    }
    width_ = header.width;
    height_ = header.height;
    frame_rate_n_ = header.frame_rate_n;
    frame_rate_d_ = header.frame_rate_d;
    color_space_ = header.color_space;
    front_ = 0u;
    configured_ = true;
    return true;
}

void DrmOutput::page_flip(int, unsigned int, unsigned int, unsigned int, void* data)
{
    auto* output = static_cast<DrmOutput*>(data);
    if (output != nullptr) {
        output->flip_complete_ = true;
    }
}

bool DrmOutput::wait_for_flip()
{
    drmEventContext context{};
    context.version = DRM_EVENT_CONTEXT_VERSION;
    context.page_flip_handler = page_flip;
    Deadline deadline = deadline_after(std::chrono::milliseconds(500));
    while (!flip_complete_ && remaining_milliseconds(deadline) > 0) {
        pollfd descriptor{fd_, POLLIN, 0};
        int result = ::poll(&descriptor, 1, remaining_milliseconds(deadline));
        if (result > 0 && (descriptor.revents & POLLIN) != 0) {
            if (drmHandleEvent(fd_, &context) != 0) {
                error_ = "Unable to handle DRM page-flip event";
                return false;
            }
        } else if (result < 0 && errno != EINTR) {
            error_ = std::string("Unable to wait for DRM page flip: ") + std::strerror(errno);
            return false;
        }
    }
    if (!flip_complete_) {
        error_ = "DRM page flip timed out";
        return false;
    }
    return true;
}

bool DrmOutput::present(Frame& frame)
{
    if (frame.header.type != OMT_FRAME_VIDEO || frame.video.codec != OMT_CODEC_VMX1) {
        error_ = "Unsupported video frame";
        return false;
    }
    if (!configured_ || width_ != frame.video.width || height_ != frame.video.height ||
        frame_rate_n_ != frame.video.frame_rate_n || frame_rate_d_ != frame.video.frame_rate_d ||
        color_space_ != frame.video.color_space) {
        if (!configure(frame.video)) {
            return false;
        }
    }
    std::size_t next = (front_ + 1u) % buffers_.size();
    std::size_t payload_offset = OMT_WIRE_VIDEO_HEADER_SIZE;
    if (frame.payload.size() < payload_offset + frame.header.metadata_length) {
        error_ = "Truncated VMX frame";
        return false;
    }
    std::size_t compressed_length = frame.payload.size() - payload_offset - frame.header.metadata_length;
    if (compressed_length > static_cast<std::size_t>(INT32_MAX)) {
        error_ = "VMX frame is too large";
        return false;
    }
    VMX_ERR loaded = VMX_LoadFrom(
        codec_, frame.payload.data() + payload_offset, static_cast<int>(compressed_length));
    if (loaded != VMX_ERR_OK ||
        VMX_DecodeBGRX(codec_, buffers_[next].mapping, static_cast<int>(buffers_[next].pitch)) != VMX_ERR_OK) {
        error_ = "VMX decoder rejected the frame";
        return false;
    }
    flip_complete_ = false;
    if (drmModePageFlip(
            fd_, crtc_id_, buffers_[next].framebuffer,
            DRM_MODE_PAGE_FLIP_EVENT, this) != 0) {
        error_ = std::string("Unable to queue DRM page flip: ") + std::strerror(errno);
        return false;
    }
    if (!wait_for_flip()) {
        return false;
    }
    front_ = next;
    return true;
}

} // namespace omt::native
