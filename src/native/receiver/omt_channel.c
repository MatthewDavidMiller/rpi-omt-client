// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#define _GNU_SOURCE
#include "omt_channel.h"

#include <errno.h>
#include <netdb.h>
#include <netinet/tcp.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

struct omt_channel {
    int socket_fd;
};

static bool wait_fd(int fd, short events, omt_deadline deadline, char *error, size_t capacity)
{
    while (omt_remaining_milliseconds(deadline) > 0) {
        struct pollfd descriptor = {fd, events, 0};
        int result = poll(&descriptor, 1, omt_remaining_milliseconds(deadline));
        if (result > 0) {
            if ((descriptor.revents & (POLLERR | POLLHUP | POLLNVAL)) != 0) {
                omt_set_error(error, capacity, "OMT socket closed");
                return false;
            }
            return (descriptor.revents & events) != 0;
        }
        if (result == 0) {
            break;
        }
        if (errno != EINTR) {
            omt_set_error(error, capacity, strerror(errno));
            return false;
        }
    }
    omt_set_error(error, capacity, "OMT socket deadline expired");
    return false;
}

omt_channel *omt_channel_create(void)
{
    omt_channel *channel = calloc(1u, sizeof(*channel));
    if (channel != NULL) {
        channel->socket_fd = -1;
    }
    return channel;
}

void omt_channel_close(omt_channel *channel)
{
    if (channel != NULL && channel->socket_fd >= 0) {
        (void)shutdown(channel->socket_fd, SHUT_RDWR);
        (void)close(channel->socket_fd);
        channel->socket_fd = -1;
    }
}

void omt_channel_destroy(omt_channel *channel)
{
    omt_channel_close(channel);
    free(channel);
}

bool omt_channel_connected(const omt_channel *channel)
{
    return channel != NULL && channel->socket_fd >= 0;
}

static bool transfer_exact(omt_channel *channel, uint8_t *data, size_t length, bool writing,
                           omt_deadline deadline, char *error, size_t capacity)
{
    size_t offset = 0u;
    while (offset < length) {
        if (!wait_fd(channel->socket_fd, writing ? POLLOUT : POLLIN, deadline, error, capacity)) {
            if (!writing && (offset != 0u || strcmp(error, "OMT socket deadline expired") != 0)) {
                omt_channel_close(channel);
            }
            return false;
        }
        ssize_t count = writing
            ? send(channel->socket_fd, data + offset, length - offset, MSG_NOSIGNAL)
            : recv(channel->socket_fd, data + offset, length - offset, 0);
        if (count > 0) {
            offset += (size_t)count;
            continue;
        }
        if (count < 0 && (errno == EINTR || errno == EAGAIN || errno == EWOULDBLOCK)) {
            continue;
        }
        omt_set_error(error, capacity, count == 0 ? "OMT source disconnected" : strerror(errno));
        omt_channel_close(channel);
        return false;
    }
    return true;
}

static bool send_subscription(omt_channel *channel, omt_frame_type type, omt_deadline deadline,
                              char *error, size_t capacity)
{
    const char *xml = type == OMT_FRAME_VIDEO ? "<OMTSubscribe Video=\"true\" />"
                    : type == OMT_FRAME_AUDIO ? "<OMTSubscribe Audio=\"true\" />"
                                              : "<OMTSubscribe Metadata=\"true\" />";
    uint8_t frame[128] = {0};
    char copy[64] = {0};
    size_t written = 0u;
    if (!omt_copy_string(copy, sizeof(copy), xml) ||
        !omt_wire_build_metadata(copy, 0, frame, sizeof(frame), &written)) {
        omt_set_error(error, capacity, "failed to build OMT subscription");
        return false;
    }
    return transfer_exact(channel, frame, written, true, deadline, error, capacity);
}

bool omt_channel_connect(omt_channel *channel, const omt_endpoint *endpoint,
                         omt_frame_type subscription, omt_deadline deadline,
                         char *error, size_t capacity)
{
    omt_channel_close(channel);
    struct addrinfo hints;
    memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    hints.ai_protocol = IPPROTO_TCP;
    char port[6];
    (void)snprintf(port, sizeof(port), "%u", (unsigned int)endpoint->port);
    struct addrinfo *addresses = NULL;
    int lookup = getaddrinfo(endpoint->host, port, &hints, &addresses);
    if (lookup != 0) {
        omt_set_error(error, capacity, gai_strerror(lookup));
        return false;
    }
    for (struct addrinfo *candidate = addresses; candidate != NULL; candidate = candidate->ai_next) {
        int fd = socket(candidate->ai_family, SOCK_STREAM | SOCK_CLOEXEC | SOCK_NONBLOCK, IPPROTO_TCP);
        if (fd < 0) {
            continue;
        }
        int one = 1;
        (void)setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &one, sizeof(one));
        (void)setsockopt(fd, SOL_SOCKET, SO_KEEPALIVE, &one, sizeof(one));
        int receive_buffer = subscription == OMT_FRAME_VIDEO ? 1024 * 1024
                           : subscription == OMT_FRAME_AUDIO ? 256 * 1024 : 128 * 1024;
        (void)setsockopt(fd, SOL_SOCKET, SO_RCVBUF, &receive_buffer, sizeof(receive_buffer));
        int result = connect(fd, candidate->ai_addr, candidate->ai_addrlen);
        channel->socket_fd = fd;
        if (result == 0 || (errno == EINPROGRESS && wait_fd(fd, POLLOUT, deadline, error, capacity))) {
            int socket_error = 0;
            socklen_t size = sizeof(socket_error);
            if (getsockopt(fd, SOL_SOCKET, SO_ERROR, &socket_error, &size) == 0 && socket_error == 0) {
                break;
            }
        }
        omt_channel_close(channel);
    }
    freeaddrinfo(addresses);
    if (channel->socket_fd < 0) {
        if (error[0] == '\0') {
            omt_set_error(error, capacity, "unable to connect to OMT source");
        }
        return false;
    }
    if (subscription == OMT_FRAME_VIDEO &&
        !send_subscription(channel, OMT_FRAME_METADATA, deadline, error, capacity)) {
        omt_channel_close(channel);
        return false;
    }
    if (!send_subscription(channel, subscription, deadline, error, capacity)) {
        omt_channel_close(channel);
        return false;
    }
    return true;
}

bool omt_channel_receive(omt_channel *channel, omt_frame *frame, omt_deadline deadline,
                         char *error, size_t capacity)
{
    uint8_t fixed[OMT_WIRE_HEADER_SIZE] = {0};
    char parse_error[160] = {0};
    if (!transfer_exact(channel, fixed, sizeof(fixed), false, deadline, error, capacity)) {
        return false;
    }
    if (!omt_wire_parse_header(fixed, sizeof(fixed), &frame->header, parse_error, sizeof(parse_error))) {
        omt_set_error(error, capacity, parse_error);
        omt_channel_close(channel);
        return false;
    }
    size_t required = frame->header.data_length;
    if (required > frame->payload_capacity) {
        uint8_t *replacement = realloc(frame->payload, required == 0u ? 1u : required);
        if (replacement == NULL) {
            omt_set_error(error, capacity, "unable to allocate bounded OMT frame");
            omt_channel_close(channel);
            return false;
        }
        frame->payload = replacement;
        frame->payload_capacity = required;
    }
    if (!transfer_exact(channel, frame->payload, required, false, deadline, error, capacity)) {
        omt_channel_close(channel);
        return false;
    }
    bool parsed = true;
    if (frame->header.type == OMT_FRAME_VIDEO) {
        parsed = omt_wire_parse_video_header(&frame->header, frame->payload, required, &frame->video,
                                             parse_error, sizeof(parse_error));
    } else if (frame->header.type == OMT_FRAME_AUDIO) {
        parsed = omt_wire_parse_audio_header(&frame->header, frame->payload, required, &frame->audio,
                                             parse_error, sizeof(parse_error));
    }
    if (!parsed) {
        omt_set_error(error, capacity, parse_error);
        omt_channel_close(channel);
    }
    return parsed;
}
