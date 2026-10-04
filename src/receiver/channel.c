/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "receiver/channel.h"

#include <errno.h>
#include <fcntl.h>
#include <netdb.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

#include "common/buf.h"
#include "common/proc.h"

void omt_frame_free(omt_frame *f) {
    free(f->payload);
    memset(f, 0, sizeof(*f));
}

bool omt_frame_media(const omt_frame *f, size_t extended_header, const uint8_t **out, size_t *len) {
    if (f->header.metadata_length > f->len) return false;
    size_t end = f->len - f->header.metadata_length;
    if (extended_header > end) return false;
    *out = f->payload + extended_header;
    *len = end - extended_header;
    return true;
}

uint64_t omt_body_deadline(uint64_t slice_ms) {
    uint64_t floor = omt_now_ms() + OMT_BODY_BUDGET_MS;
    return slice_ms > floor ? slice_ms : floor;
}

void omt_channel_init(omt_channel *c) {
    memset(c, 0, sizeof(*c));
    c->fd = -1;
}

void omt_channel_close(omt_channel *c) {
    if (c->fd >= 0) {
        shutdown(c->fd, SHUT_RDWR);
        close(c->fd);
        c->fd = -1;
    }
}

void omt_channel_free(omt_channel *c) {
    omt_channel_close(c);
    omt_frame_free(&c->frame);
}

size_t omt_endpoint_resolve(const omt_endpoint *ep, struct sockaddr_storage *out, size_t max,
                            omt_err *err) {
    char host[OMT_HOST_MAX_BYTES];
    omt_strlcpy(host, ep->host, sizeof(host));
    char *zone = strchr(host, '%');
    if (zone) *zone = 0;
    char port[8];
    snprintf(port, sizeof(port), "%u", (unsigned)ep->port);
    struct addrinfo hints = {0};
    hints.ai_socktype = SOCK_STREAM;
    hints.ai_flags = AI_NUMERICSERV;
    struct addrinfo *list = NULL;
    int rc = getaddrinfo(host, port, &hints, &list);
    if (rc != 0) {
        omt_err_set(err, "failed to lookup address information: %s", gai_strerror(rc));
        return 0;
    }
    size_t n = 0;
    for (struct addrinfo *a = list; a && n < max; a = a->ai_next) {
        if (a->ai_addrlen > sizeof(struct sockaddr_storage)) continue;
        memset(&out[n], 0, sizeof(out[n]));
        memcpy(&out[n], a->ai_addr, a->ai_addrlen);
        n++;
    }
    freeaddrinfo(list);
    return n;
}

static socklen_t addr_len(const struct sockaddr_storage *a) {
    return a->ss_family == AF_INET6 ? (socklen_t)sizeof(struct sockaddr_in6)
                                    : (socklen_t)sizeof(struct sockaddr_in);
}

/* A non-blocking connect bounded by `timeout_ms`. */
static int connect_timeout(const struct sockaddr_storage *addr, uint64_t timeout_ms, omt_err *err) {
    int fd = socket(addr->ss_family, SOCK_STREAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0);
    if (fd < 0) {
        omt_err_os(err, errno);
        return -1;
    }
    /* The receive buffer is set before connect so TCP window scaling is
     * negotiated for it. Linux may double the request internally. */
    int size = OMT_RECV_BUFFER;
    if (setsockopt(fd, SOL_SOCKET, SO_RCVBUF, &size, sizeof(size)) != 0) {
        fprintf(stderr, "unable to set OMT receive buffer to %d bytes: %s\n", size,
                strerror(errno));
    }
    if (connect(fd, (const struct sockaddr *)addr, addr_len(addr)) != 0) {
        if (errno != EINPROGRESS) {
            omt_err_os(err, errno);
            close(fd);
            return -1;
        }
        struct pollfd p = {fd, POLLOUT, 0};
        int rc;
        do {
            rc = poll(&p, 1, (int)omt_min_size(timeout_ms, INT32_MAX));
        } while (rc < 0 && errno == EINTR);
        if (rc == 0) {
            omt_err_set(err, "connection timed out");
            close(fd);
            return -1;
        }
        int so_error = 0;
        socklen_t len = sizeof(so_error);
        if (rc < 0 || getsockopt(fd, SOL_SOCKET, SO_ERROR, &so_error, &len) != 0 || so_error) {
            omt_err_os(err, rc < 0 ? errno : so_error ? so_error : errno);
            close(fd);
            return -1;
        }
    }
    int one = 1;
    if (setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &one, sizeof(one)) != 0) {
        omt_err_os(err, errno);
        close(fd);
        return -1;
    }
    return fd;
}

static bool write_all_deadline(int fd, const uint8_t *data, size_t len, uint64_t deadline_ms,
                               omt_err *err) {
    while (len) {
        ssize_t n = send(fd, data, len, MSG_NOSIGNAL);
        if (n > 0) {
            data += n;
            len -= (size_t)n;
            continue;
        }
        if (n < 0 && errno == EINTR) continue;
        if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) {
            /* A write timeout of zero would mean "block forever" to a socket;
             * the shortest real bound is one millisecond. */
            uint64_t left = omt_remaining_ms(deadline_ms);
            struct pollfd p = {fd, POLLOUT, 0};
            int rc = poll(&p, 1, (int)(left ? omt_min_size(left, INT32_MAX) : 1));
            if (rc == 0) {
                omt_err_set(err, "Resource temporarily unavailable (os error %d)", EAGAIN);
                return false;
            }
            continue;
        }
        omt_err_os(err, n < 0 ? errno : EPIPE);
        return false;
    }
    return true;
}

static bool subscribe(omt_channel *c, omt_frame_type type, uint64_t deadline_ms, omt_err *err) {
    const char *xml = type == OMT_FRAME_VIDEO   ? "<OMTSubscribe Video=\"true\" />"
                      : type == OMT_FRAME_AUDIO ? "<OMTSubscribe Audio=\"true\" />"
                                                : "<OMTSubscribe Metadata=\"true\" />";
    omt_buf b;
    omt_buf_init(&b, 256);
    omt_proto_error pe;
    if (!omt_build_metadata(xml, strlen(xml), 0, &b, &pe)) {
        omt_proto_error_format(&pe, err ? err->msg : (char[256]){0}, 256);
        omt_buf_free(&b);
        return false;
    }
    bool ok = c->fd >= 0 && write_all_deadline(c->fd, b.data, b.len, deadline_ms, err);
    if (c->fd < 0) omt_err_set(err, "OMT socket closed");
    omt_buf_free(&b);
    if (!ok) omt_channel_close(c);
    return ok;
}

bool omt_channel_connect(omt_channel *c, const omt_endpoint *ep, omt_frame_type subscription,
                         uint64_t deadline_ms, omt_err *err) {
    omt_channel_close(c);
    struct sockaddr_storage addrs[OMT_MAX_ADDRESSES];
    size_t n = omt_endpoint_resolve(ep, addrs, OMT_MAX_ADDRESSES, err);
    if (n == 0 && err && !err->msg[0]) omt_err_set(err, "unable to connect to the OMT source");
    if (n == 0) return false;
    omt_err last;
    omt_err_set(&last, "unable to connect to the OMT source");
    for (size_t i = 0; i < n; i++) {
        uint64_t left = omt_remaining_ms(deadline_ms);
        if (left == 0) break;
        int fd = connect_timeout(&addrs[i], left, &last);
        if (fd >= 0) {
            c->fd = fd;
            break;
        }
    }
    if (c->fd < 0) {
        if (err) *err = last;
        return false;
    }
    if (subscription == OMT_FRAME_VIDEO && !subscribe(c, OMT_FRAME_METADATA, deadline_ms, err))
        return false;
    return subscribe(c, subscription, deadline_ms, err);
}

/* `resumable` marks a read that has consumed none of the frame, so the slice
 * can expire without invalidating the connection. From the first byte on, the
 * body budget applies instead. */
static omt_recv_status read_exact(omt_channel *c, uint8_t *target, size_t len, uint64_t slice_ms,
                                  bool resumable, omt_err *err) {
    size_t filled = 0;
    uint64_t deadline = resumable ? slice_ms : omt_body_deadline(slice_ms);
    while (filled < len) {
        uint64_t left = omt_remaining_ms(deadline);
        if (left == 0) goto expired;
        if (c->fd < 0) {
            omt_err_set(err, "OMT socket closed");
            return OMT_RECV_ERROR;
        }
        struct pollfd p = {c->fd, POLLIN, 0};
        int rc = poll(&p, 1, (int)omt_min_size(left, INT32_MAX));
        if (rc < 0) {
            if (errno == EINTR) {
                if (filled != 0) goto expired;
                continue;
            }
            omt_err_os(err, errno);
            return OMT_RECV_ERROR;
        }
        if (rc == 0) continue;
        ssize_t n = recv(c->fd, target + filled, len - filled, 0);
        if (n == 0) {
            omt_err_set(err, "OMT source disconnected");
            return OMT_RECV_ERROR;
        }
        if (n < 0) {
            if (errno == EINTR || errno == EAGAIN || errno == EWOULDBLOCK) continue;
            omt_err_os(err, errno);
            return OMT_RECV_ERROR;
        }
        /* This read is now committed to a frame, so the polling slice stops
         * bounding it. */
        if (filled == 0) deadline = omt_body_deadline(slice_ms);
        filled += (size_t)n;
    }
    return OMT_RECV_OK;
expired:
    if (resumable && filled == 0) {
        omt_err_set(err, "OMT socket deadline expired");
        return OMT_RECV_WOULD_BLOCK;
    }
    omt_err_set(err, "OMT frame was truncated by a timeout");
    return OMT_RECV_TIMED_OUT;
}

/* Grows the payload buffer in place. Only bytes it has never held need to be
 * zeroed, and the read overwrites the rest. */
static bool ensure_payload(omt_frame *f, size_t required) {
    if (required > f->cap) {
        uint8_t *grown = realloc(f->payload, required);
        if (!grown) return false;
        memset(grown + f->cap, 0, required - f->cap);
        f->payload = grown;
        f->cap = required;
    }
    f->len = required;
    return true;
}

static omt_recv_status receive_inner(omt_channel *c, uint64_t deadline_ms, omt_err *err) {
    uint8_t fixed[OMT_HEADER_SIZE];
    omt_recv_status st = read_exact(c, fixed, sizeof(fixed), deadline_ms, true, err);
    if (st != OMT_RECV_OK) return st;
    omt_frame_header header;
    omt_proto_error pe;
    if (!omt_parse_frame_header(fixed, sizeof(fixed), &header, &pe)) {
        if (err) omt_proto_error_format(&pe, err->msg, sizeof(err->msg));
        return OMT_RECV_ERROR;
    }
    if (!ensure_payload(&c->frame, header.data_length)) {
        omt_err_set(err, "unable to allocate the bounded OMT frame");
        return OMT_RECV_ERROR;
    }
    /* The fixed header committed this connection to a frame. */
    st = read_exact(c, c->frame.payload, c->frame.len, deadline_ms, false, err);
    if (st != OMT_RECV_OK) return st == OMT_RECV_WOULD_BLOCK ? OMT_RECV_TIMED_OUT : st;
    c->frame.has_video = false;
    c->frame.has_audio = false;
    if (header.frame_type == OMT_FRAME_VIDEO) {
        if (!omt_parse_video_header(&header, c->frame.payload, c->frame.len, &c->frame.video,
                                    &pe)) {
            if (err) omt_proto_error_format(&pe, err->msg, sizeof(err->msg));
            return OMT_RECV_ERROR;
        }
        c->frame.has_video = true;
    } else if (header.frame_type == OMT_FRAME_AUDIO) {
        if (!omt_parse_audio_header(&header, c->frame.payload, c->frame.len, &c->frame.audio,
                                    &pe)) {
            if (err) omt_proto_error_format(&pe, err->msg, sizeof(err->msg));
            return OMT_RECV_ERROR;
        }
        c->frame.has_audio = true;
    }
    c->frame.header = header;
    return OMT_RECV_OK;
}

omt_recv_status omt_channel_receive(omt_channel *c, uint64_t deadline_ms, omt_err *err) {
    omt_recv_status st = receive_inner(c, deadline_ms, err);
    /* A deadline with nothing consumed leaves the channel usable; anything
     * else means the stream is no longer trustworthy. */
    if (st != OMT_RECV_OK && st != OMT_RECV_WOULD_BLOCK) omt_channel_close(c);
    return st;
}

void omt_channel_take_frame(omt_channel *c, omt_frame *out, uint8_t *replacement,
                            size_t replacement_cap) {
    *out = c->frame;
    memset(&c->frame, 0, sizeof(c->frame));
    c->frame.payload = replacement;
    c->frame.cap = replacement ? replacement_cap : 0;
}
