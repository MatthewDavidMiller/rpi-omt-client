// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#include "omt_channel.hpp"

#include "omt/omt_wire.h"

#include <algorithm>
#include <array>
#include <cerrno>
#include <cstring>
#include <fcntl.h>
#include <netdb.h>
#include <netinet/tcp.h>
#include <poll.h>
#include <sys/socket.h>
#include <unistd.h>

namespace omt::native {
namespace {

constexpr std::string_view subscribe_video = R"(<OMTSubscribe Video="true" />)";
constexpr std::string_view subscribe_audio = R"(<OMTSubscribe Audio="true" />)";
constexpr std::string_view subscribe_metadata = R"(<OMTSubscribe Metadata="true" />)";

bool wait_fd(int fd, short events, Deadline deadline, std::string& error)
{
    while (remaining_milliseconds(deadline) > 0) {
        pollfd descriptor{fd, events, 0};
        int result = ::poll(&descriptor, 1, remaining_milliseconds(deadline));
        if (result > 0) {
            if ((descriptor.revents & static_cast<short>(POLLERR | POLLHUP | POLLNVAL)) != 0) {
                error = "OMT socket closed";
                return false;
            }
            return (descriptor.revents & events) != 0;
        }
        if (result == 0) {
            break;
        }
        if (errno != EINTR) {
            error = std::strerror(errno);
            return false;
        }
    }
    error = "OMT socket deadline expired";
    return false;
}

} // namespace

OmtChannel::~OmtChannel()
{
    close();
}

void OmtChannel::close()
{
    if (socket_ >= 0) {
        (void)::shutdown(socket_, SHUT_RDWR);
        (void)::close(socket_);
        socket_ = -1;
    }
}

bool OmtChannel::connect(
    const Endpoint& endpoint,
    omt_frame_type subscription,
    Deadline deadline,
    std::string& error)
{
    close();
    addrinfo hints{};
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    hints.ai_protocol = IPPROTO_TCP;
    std::array<char, 6> port{};
    (void)std::snprintf(port.data(), port.size(), "%u", static_cast<unsigned int>(endpoint.port));
    addrinfo* addresses = nullptr;
    int lookup = ::getaddrinfo(endpoint.host.c_str(), port.data(), &hints, &addresses);
    if (lookup != 0) {
        error = ::gai_strerror(lookup);
        return false;
    }
    for (addrinfo* candidate = addresses; candidate != nullptr && socket_ < 0; candidate = candidate->ai_next) {
        int fd = ::socket(candidate->ai_family, SOCK_STREAM | SOCK_CLOEXEC | SOCK_NONBLOCK, IPPROTO_TCP);
        if (fd < 0) {
            continue;
        }
        int one = 1;
        (void)::setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &one, sizeof(one));
        (void)::setsockopt(fd, SOL_SOCKET, SO_KEEPALIVE, &one, sizeof(one));
        int receive_buffer = subscription == OMT_FRAME_VIDEO ? 1024 * 1024
                           : subscription == OMT_FRAME_AUDIO ? 256 * 1024
                                                             : 128 * 1024;
        (void)::setsockopt(fd, SOL_SOCKET, SO_RCVBUF, &receive_buffer, sizeof(receive_buffer));
        int result = ::connect(fd, candidate->ai_addr, candidate->ai_addrlen);
        if (result == 0 || (errno == EINPROGRESS && wait_fd(fd, POLLOUT, deadline, error))) {
            int socket_error = 0;
            socklen_t size = sizeof(socket_error);
            if (::getsockopt(fd, SOL_SOCKET, SO_ERROR, &socket_error, &size) == 0 && socket_error == 0) {
                socket_ = fd;
                break;
            }
        }
        (void)::close(fd);
    }
    ::freeaddrinfo(addresses);
    if (socket_ < 0) {
        if (error.empty()) {
            error = "unable to connect to OMT source";
        }
        return false;
    }
    if (subscription == OMT_FRAME_VIDEO && !send_subscription(OMT_FRAME_METADATA, deadline, error)) {
        close();
        return false;
    }
    if (!send_subscription(subscription, deadline, error)) {
        close();
        return false;
    }
    return true;
}

bool OmtChannel::send_subscription(omt_frame_type type, Deadline deadline, std::string& error)
{
    std::string_view xml;
    if (type == OMT_FRAME_VIDEO) {
        xml = subscribe_video;
    } else if (type == OMT_FRAME_AUDIO) {
        xml = subscribe_audio;
    } else {
        xml = subscribe_metadata;
    }
    std::array<std::uint8_t, 128> frame{};
    std::array<char, 64> copy{};
    if (xml.size() >= copy.size()) {
        error = "internal OMT subscription is too long";
        return false;
    }
    std::memcpy(copy.data(), xml.data(), xml.size());
    std::size_t written = 0;
    if (!omt_wire_build_metadata(copy.data(), 0, frame.data(), frame.size(), &written)) {
        error = "failed to build OMT subscription";
        return false;
    }
    return write_exact(frame.data(), written, deadline, error);
}

bool OmtChannel::read_exact(
    std::uint8_t* output,
    std::size_t length,
    Deadline deadline,
    std::string& error)
{
    std::size_t offset = 0u;
    while (offset < length) {
        if (!wait_fd(socket_, POLLIN, deadline, error)) {
            if (offset != 0u || error != "OMT socket deadline expired") {
                close();
            }
            return false;
        }
        ssize_t received = ::recv(socket_, output + offset, length - offset, 0);
        if (received > 0) {
            offset += static_cast<std::size_t>(received);
            continue;
        }
        if (received == 0) {
            error = "OMT source disconnected";
            close();
            return false;
        }
        if (errno != EINTR && errno != EAGAIN && errno != EWOULDBLOCK) {
            error = std::strerror(errno);
            close();
            return false;
        }
    }
    return true;
}

bool OmtChannel::write_exact(
    const std::uint8_t* input,
    std::size_t length,
    Deadline deadline,
    std::string& error)
{
    std::size_t offset = 0u;
    while (offset < length) {
        if (!wait_fd(socket_, POLLOUT, deadline, error)) {
            return false;
        }
        ssize_t sent = ::send(socket_, input + offset, length - offset, MSG_NOSIGNAL);
        if (sent > 0) {
            offset += static_cast<std::size_t>(sent);
            continue;
        }
        if (sent < 0 && (errno == EINTR || errno == EAGAIN || errno == EWOULDBLOCK)) {
            continue;
        }
        error = sent == 0 ? "OMT source disconnected" : std::strerror(errno);
        close();
        return false;
    }
    return true;
}

bool OmtChannel::receive(Frame& frame, Deadline deadline, std::string& error)
{
    std::array<std::uint8_t, OMT_WIRE_HEADER_SIZE> fixed{};
    if (!read_exact(fixed.data(), fixed.size(), deadline, error)) {
        return false;
    }
    std::array<char, 160> parse_error{};
    if (!omt_wire_parse_header(fixed.data(), fixed.size(), &frame.header, parse_error.data(), parse_error.size())) {
        error = parse_error.data();
        close();
        return false;
    }
    frame.payload.resize(frame.header.data_length);
    if (!read_exact(frame.payload.data(), frame.payload.size(), deadline, error)) {
        // The fixed header has already been consumed. Even a timeout before the
        // first payload byte would make a later receive start mid-frame.
        close();
        return false;
    }
    if (frame.header.type == OMT_FRAME_VIDEO &&
        !omt_wire_parse_video_header(
            &frame.header, frame.payload.data(), frame.payload.size(), &frame.video,
            parse_error.data(), parse_error.size())) {
        error = parse_error.data();
        close();
        return false;
    }
    if (frame.header.type == OMT_FRAME_AUDIO &&
        !omt_wire_parse_audio_header(
            &frame.header, frame.payload.data(), frame.payload.size(), &frame.audio,
            parse_error.data(), parse_error.size())) {
        error = parse_error.data();
        close();
        return false;
    }
    return true;
}

} // namespace omt::native
