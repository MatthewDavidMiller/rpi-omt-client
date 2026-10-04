/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Compressed A/V playout queue. TCP reads stay greedy (OMT: never stall the
 * accept path); HDMI and ALSA consume this queue on a paced clock. vMix drops
 * in-flight extras during a stall, so the queue is a pre-roll cushion of frames
 * already accepted, not a catch-up reel.
 */
#ifndef OMT_RECEIVER_JITTER_H
#define OMT_RECEIVER_JITTER_H

#include <stdatomic.h>

#include "receiver/channel.h"

/* Payload ceiling for queued compressed video. */
#define OMT_VIDEO_BYTE_CAP ((size_t)256 * 1024 * 1024)
#define OMT_AUDIO_BYTE_CAP ((size_t)8 * 1024 * 1024)
/* How far past the configured delay a queue may grow before oldest frames are
 * dropped. */
#define OMT_TIME_SLACK_NS 500000000ull
/* How far past the delay a *playing* audio queue may grow: every audio frame
 * shed is a gap the operator hears, so audio is only trimmed past this. */
#define OMT_AUDIO_SLACK_NS 100000000ull
/* Payload buffers kept for reuse. */
#define OMT_SPARE_BUFFERS 4

typedef enum { OMT_QUEUE_VIDEO, OMT_QUEUE_AUDIO } omt_queue_kind;

typedef struct {
    uint8_t *data;
    size_t cap;
} omt_spare;

typedef struct {
    omt_frame *frames; /* ring buffer */
    size_t head;
    size_t count;
    size_t slots;
    size_t bytes;
    uint64_t delay_ns;
    size_t byte_cap;
    bool has_interval;
    uint64_t interval_ns;
    omt_queue_kind kind;
    omt_spare spare[OMT_SPARE_BUFFERS];
    size_t spare_count;
} omt_queue;

void omt_queue_init(omt_queue *q, omt_queue_kind kind, uint64_t delay_ns, size_t byte_cap);
void omt_queue_free(omt_queue *q);
static inline bool omt_queue_empty(const omt_queue *q) { return q->count == 0; }
/* Queued media time from the current interval, or zero before a timed frame. */
uint64_t omt_queue_duration_ns(const omt_queue *q);
/* Ready to start HDMI or ALSA: delay 0 needs one frame; otherwise the
 * configured delay, or the byte cap when a fat stream fills RAM first. */
bool omt_queue_filled(const omt_queue *q);
/* A recycled payload buffer for the next received frame, or NULL/0. */
void omt_queue_take_spare(omt_queue *q, uint8_t **data, size_t *cap);
void omt_queue_recycle(omt_queue *q, uint8_t *data, size_t cap);
/* Takes ownership of `frame`. Metadata is ignored so it cannot occupy the
 * budget. Returns how many older frames were dropped; a frame that could not
 * be stored is recycled. */
uint64_t omt_queue_push(omt_queue *q, omt_frame *frame, bool playing);
/* Moves the oldest frame into `out`; false when empty. */
bool omt_queue_pop(omt_queue *q, omt_frame *out);
/* Drops every queued video frame except the newest. Audio keeps its burst. */
void omt_queue_keep_latest_only(omt_queue *q);

bool omt_video_interval_ns(const omt_video_header *h, uint64_t *out);
bool omt_audio_interval_ns(const omt_audio_header *h, uint64_t *out);

/* Handshake so HDMI and ALSA start together after pre-roll, without blocking
 * video forever when the sender has no audio. */
typedef struct {
    atomic_bool video;
    atomic_bool audio;
    atomic_bool audio_missing;
} omt_fill_gate;

void omt_gate_init(omt_fill_gate *g);
void omt_gate_set_video(omt_fill_gate *g, bool ready);
void omt_gate_set_audio(omt_fill_gate *g, bool ready);
void omt_gate_set_audio_missing(omt_fill_gate *g);
bool omt_gate_video_ready(omt_fill_gate *g);
bool omt_gate_audio_ready(omt_fill_gate *g);

#endif
