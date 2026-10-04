/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "receiver/handoff.h"

#include <math.h>
#include <stdlib.h>
#include <string.h>

void omt_queue_init(omt_queue *q, omt_queue_kind kind, size_t byte_cap) {
    memset(q, 0, sizeof(*q));
    q->kind = kind;
    q->byte_cap = byte_cap;
}

void omt_queue_free(omt_queue *q) {
    for (size_t i = 0; i < q->count; i++) omt_frame_free(&q->frames[(q->head + i) % q->slots]);
    free(q->frames);
    for (size_t i = 0; i < q->spare_count; i++) free(q->spare[i].data);
    memset(q, 0, sizeof(*q));
}

void omt_queue_take_spare(omt_queue *q, uint8_t **data, size_t *cap) {
    if (q->spare_count == 0) {
        *data = NULL;
        *cap = 0;
        return;
    }
    q->spare_count--;
    *data = q->spare[q->spare_count].data;
    *cap = q->spare[q->spare_count].cap;
}

void omt_queue_recycle(omt_queue *q, uint8_t *data, size_t cap) {
    if (data && cap > 0 && q->spare_count < OMT_SPARE_BUFFERS) {
        q->spare[q->spare_count].data = data;
        q->spare[q->spare_count].cap = cap;
        q->spare_count++;
        return;
    }
    free(data);
}

static void drop_oldest(omt_queue *q) {
    if (q->count == 0) return;
    omt_frame *f = &q->frames[q->head];
    q->bytes -= f->len < q->bytes ? f->len : q->bytes;
    omt_queue_recycle(q, f->payload, f->cap);
    memset(f, 0, sizeof(*f));
    q->head = (q->head + 1) % q->slots;
    q->count--;
}

bool omt_queue_pop(omt_queue *q, omt_frame *out) {
    if (q->count == 0) return false;
    *out = q->frames[q->head];
    memset(&q->frames[q->head], 0, sizeof(omt_frame));
    q->bytes -= out->len < q->bytes ? out->len : q->bytes;
    q->head = (q->head + 1) % q->slots;
    q->count--;
    return true;
}

static bool grow(omt_queue *q) {
    if (q->count < q->slots) return true;
    size_t slots = q->slots ? q->slots * 2 : 8;
    omt_frame *frames = calloc(slots, sizeof(omt_frame));
    if (!frames) return false;
    for (size_t i = 0; i < q->count; i++) frames[i] = q->frames[(q->head + i) % q->slots];
    free(q->frames);
    q->frames = frames;
    q->slots = slots;
    q->head = 0;
    return true;
}

/* The queued audio's duration, from the latest frame's interval. */
static bool audio_backlog_exceeded(const omt_queue *q) {
    if (!q->has_interval || q->count <= 1) return false;
    uint64_t total;
    return !omt_mul(q->interval_ns, (uint64_t)q->count, &total) || total > OMT_AUDIO_BACKLOG_NS;
}

uint64_t omt_queue_push(omt_queue *q, omt_frame *frame) {
    bool wanted = q->kind == OMT_QUEUE_VIDEO ? frame->has_video : frame->has_audio;
    if (!wanted || frame->len > q->byte_cap) {
        omt_queue_recycle(q, frame->payload, frame->cap);
        memset(frame, 0, sizeof(*frame));
        return 0;
    }
    uint64_t dropped = 0;
    /* Video is a single slot: the newer frame always wins. */
    if (q->kind == OMT_QUEUE_VIDEO) {
        while (q->count > 0) {
            drop_oldest(q);
            dropped++;
        }
    } else {
        uint64_t interval;
        if (omt_audio_interval_ns(&frame->audio, &interval)) {
            q->interval_ns = interval;
            q->has_interval = true;
        }
    }
    if (!grow(q)) {
        omt_queue_recycle(q, frame->payload, frame->cap);
        memset(frame, 0, sizeof(*frame));
        return dropped;
    }
    q->bytes += frame->len;
    q->frames[(q->head + q->count) % q->slots] = *frame;
    q->count++;
    memset(frame, 0, sizeof(*frame));
    while (q->count > 1 && (q->bytes > q->byte_cap || audio_backlog_exceeded(q))) {
        drop_oldest(q);
        dropped++;
    }
    return dropped;
}

/* Duration::try_from_secs_f64 rounds to the nearest nanosecond; so does this. */
bool omt_audio_interval_ns(const omt_audio_header *h, uint64_t *out) {
    if (h->samples_per_channel <= 0 || h->sample_rate <= 0) return false;
    double seconds = (double)h->samples_per_channel / (double)h->sample_rate;
    if (!isfinite(seconds) || seconds <= 0.0 || seconds > 1.8e10) return false;
    *out = (uint64_t)llround(seconds * 1e9);
    return true;
}
