/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The receiver's only network entry point. Every frame is bounded by the
 * protocol module before a byte of payload is allocated, and a parse failure
 * closes the connection rather than resynchronising on attacker-chosen bytes.
 */
#ifndef OMT_RECEIVER_CHANNEL_H
#define OMT_RECEIVER_CHANNEL_H

#include "common/base.h"
#include "common/err.h"
#include "protocol/omt.h"

/* Maximum addresses tried for one endpoint, bounding a hostile DNS answer. */
#define OMT_MAX_ADDRESSES 16
/* How long a frame that has started arriving may take to finish. The caller's
 * deadline is a polling slice, not a frame budget: it bounds only the wait for
 * a frame to begin. Six seconds is above the 3.5 s Wi-Fi stalls measured
 * against vMix and under the SIGTERM grace in control-omt.sh. */
#define OMT_BODY_BUDGET_MS 6000u
/* Kernel receive buffer matching libomtnet NETWORK_RECEIVE_BUFFER. */
#define OMT_RECV_BUFFER (8 * 1024 * 1024)

typedef struct {
    char host[OMT_HOST_MAX_BYTES];
    uint16_t port;
} omt_endpoint;

/* A received frame with its parsed, validated headers. */
typedef struct {
    omt_frame_header header;
    bool has_video;
    bool has_audio;
    omt_video_header video;
    omt_audio_header audio;
    uint8_t *payload;
    size_t len;
    size_t cap;
} omt_frame;

void omt_frame_free(omt_frame *f);
/* The compressed media body, excluding the extended header and any trailing
 * per-frame metadata. Returns false when the two overlap. */
bool omt_frame_media(const omt_frame *f, size_t extended_header, const uint8_t **out, size_t *len);

typedef enum {
    OMT_RECV_OK = 0,
    /* The slice expired with nothing consumed; the channel stays open. */
    OMT_RECV_WOULD_BLOCK,
    /* A frame in flight did not finish inside the body budget. */
    OMT_RECV_TIMED_OUT,
    OMT_RECV_ERROR,
} omt_recv_status;

typedef struct {
    int fd;
    omt_frame frame;
} omt_channel;

void omt_channel_init(omt_channel *c);
void omt_channel_close(omt_channel *c);
void omt_channel_free(omt_channel *c);
static inline bool omt_channel_connected(const omt_channel *c) { return c->fd >= 0; }

/* Connects and subscribes. A video subscription also asks for metadata,
 * matching the sender handshake the reference implementation expects. */
OMT_NODISCARD bool omt_channel_connect(omt_channel *c, const omt_endpoint *ep,
                                       omt_frame_type subscription, uint64_t deadline_ms,
                                       omt_err *err);
/* Reads the next frame into c->frame, closing the channel on anything but a
 * would-block. */
omt_recv_status omt_channel_receive(omt_channel *c, uint64_t deadline_ms, omt_err *err);
/* Swaps the received frame out so a queue can own its payload, leaving
 * `replacement` (a recycled buffer, possibly empty) to read the next one into. */
void omt_channel_take_frame(omt_channel *c, omt_frame *out, uint8_t *replacement,
                            size_t replacement_cap);

/* The deadline a frame already in flight finishes on: the slice, raised to at
 * least the body budget from now. */
uint64_t omt_body_deadline(uint64_t slice_ms);

/* Resolves to at most OMT_MAX_ADDRESSES socket addresses. A scoped IPv6
 * literal keeps its zone for display but not for lookup. */
struct sockaddr_storage;
size_t omt_endpoint_resolve(const omt_endpoint *ep, struct sockaddr_storage *out, size_t max,
                            omt_err *err);

#endif
