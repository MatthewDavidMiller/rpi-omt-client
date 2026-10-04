/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The playback supervisor: the outer retry loop, the video session, and the
 * bounded-stack audio worker. It is the only place that decides which
 * playback state the status document reports.
 */
#ifndef OMT_RECEIVER_PLAY_H
#define OMT_RECEIVER_PLAY_H

#include <stdatomic.h>

#include "receiver_core/core.h"

#define OMT_AUDIO_STACK (128u * 1024u)
/* How long a session waits for its first frame before saying so. */
#define OMT_MEDIA_GRACE_MS 5000u
/* How long a connected socket that delivers nothing is tolerated before the
 * session is rebuilt: three grace periods, far past any burst gap. */
#define OMT_MEDIA_STALL_MS 15000u
#define OMT_CONNECTOR_POLL_MS 500u
/* Per-receive slice, which is also the heartbeat cadence while idle. */
#define OMT_RECEIVE_SLICE_MS 500u
#define OMT_CONNECT_TIMEOUT_MS 3000u
#define OMT_RESOLVE_TIMEOUT_MS 1500u
/* In-session video reconnects before the session is rebuilt. */
#define OMT_RECOVER_ATTEMPTS 3u
/* Added before each reconnect attempt after the first. */
#define OMT_RECOVER_BACKOFF_MS 250u
#define OMT_RECOVER_TIMEOUT_MS 1000u
/* After one queue has filled, how long to wait for the other. */
#define OMT_PEER_WAIT_MS 1000u
#define OMT_BUFFERED_STEP_MS 100u
/* The audio worker's wake interval while the ring is at its target. */
#define OMT_AUDIO_FEED_MS 20u
/* Receive slice used to copy frames already in the kernel buffer. */
#define OMT_DRAIN_SLICE_MS 1u

typedef struct {
    const char *target;
    const char *preference;
    uint64_t retry_ms;
    omt_video_ceiling ceiling;
    uint64_t playout_delay_ms;
} omt_play_options;

/* Runs until `stop` is raised, then reports a stopped document. */
void omt_play_run(const omt_play_options *options, omt_playback_status *status, atomic_bool *stop);

/* The running detail with this session's delay, buffer, and counts; rebuilt
 * only when something it names changes. */
typedef struct {
    uint64_t delay_ms;
    uint64_t buffered_steps;
    uint64_t reconnects;
    uint64_t skipped;
    uint64_t underruns;
    char base[256];
    char text[512];
    bool valid;
} omt_running_detail;

const char *omt_running_detail_get(omt_running_detail *d, const char *base, uint64_t delay_ms,
                                   uint64_t buffered_ms, uint64_t reconnects, uint64_t skipped,
                                   uint64_t underruns);
void omt_describe_running(const char *base, uint64_t delay_ms, uint64_t buffered_ms,
                          uint64_t reconnects, uint64_t skipped, uint64_t underruns, char *out,
                          size_t size);
void omt_describe_audio(uint64_t underruns, char *out, size_t size);

#endif
