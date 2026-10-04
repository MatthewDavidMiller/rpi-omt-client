/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Frame handoff from the socket to the display and the audio device.
 *
 * There is no playout buffer. An OMT sender keeps none, so frames held here
 * would only add latency the picture cannot win back. TCP reads stay greedy
 * (OMT: never stall the accept path), and what they deliver is handed on as
 * soon as the consumer can take it:
 *
 *   * Video keeps only the newest frame. The session decodes and flips it as
 *     soon as it arrives; a frame a newer one replaces before the display
 *     took it is dropped and counted.
 *   * Audio cannot skip ahead, so it queues in arrival order for the ALSA
 *     ring, which paces it. A backlog past OMT_AUDIO_BACKLOG_NS -- a sender
 *     running faster than the sink -- sheds its oldest frames so latency
 *     cannot creep.
 */
#ifndef OMT_RECEIVER_HANDOFF_H
#define OMT_RECEIVER_HANDOFF_H

#include "receiver/channel.h"

/* The most compressed media a queue may hold: one video frame, or the audio
 * backlog the trim below allows several times over. */
#define OMT_VIDEO_BYTE_CAP ((size_t)OMT_VIDEO_MAX_SIZE + 4096u)
#define OMT_AUDIO_BYTE_CAP ((size_t)8 * 1024 * 1024)
/* Queued audio beyond this is a sender ahead of the sink. */
#define OMT_AUDIO_BACKLOG_NS 100000000ull
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
    size_t byte_cap;
    bool has_interval;
    uint64_t interval_ns;
    omt_queue_kind kind;
    omt_spare spare[OMT_SPARE_BUFFERS];
    size_t spare_count;
} omt_queue;

void omt_queue_init(omt_queue *q, omt_queue_kind kind, size_t byte_cap);
void omt_queue_free(omt_queue *q);
static inline bool omt_queue_empty(const omt_queue *q) { return q->count == 0; }
/* A recycled payload buffer for the next received frame, or NULL/0. */
void omt_queue_take_spare(omt_queue *q, uint8_t **data, size_t *cap);
void omt_queue_recycle(omt_queue *q, uint8_t *data, size_t cap);
/* Takes ownership of `frame`. Metadata is ignored. Returns how many older
 * frames were dropped to make way for it; a frame that could not be stored
 * is recycled. */
uint64_t omt_queue_push(omt_queue *q, omt_frame *frame);
/* Moves the oldest frame into `out`; false when empty. */
bool omt_queue_pop(omt_queue *q, omt_frame *out);

bool omt_audio_interval_ns(const omt_audio_header *h, uint64_t *out);

#endif
