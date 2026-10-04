/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "receiver/jitter.h"

#include <math.h>
#include <stdlib.h>
#include <string.h>

void omt_queue_init(omt_queue *q, omt_queue_kind kind, uint64_t delay_ns, size_t byte_cap) {
    memset(q, 0, sizeof(*q));
    q->kind = kind;
    q->delay_ns = delay_ns;
    q->byte_cap = byte_cap;
}

void omt_queue_free(omt_queue *q) {
    for (size_t i = 0; i < q->count; i++) omt_frame_free(&q->frames[(q->head + i) % q->slots]);
    free(q->frames);
    for (size_t i = 0; i < q->spare_count; i++) free(q->spare[i].data);
    memset(q, 0, sizeof(*q));
}

uint64_t omt_queue_duration_ns(const omt_queue *q) {
    if (!q->has_interval) return 0;
    uint64_t total;
    if (!omt_mul(q->interval_ns, (uint64_t)q->count, &total)) return UINT64_MAX;
    return total;
}

bool omt_queue_filled(const omt_queue *q) {
    if (q->count == 0) return false;
    if (q->delay_ns == 0) return true;
    return omt_queue_duration_ns(q) >= q->delay_ns || q->bytes >= q->byte_cap;
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

static uint64_t drop_while_longer_than(omt_queue *q, uint64_t limit) {
    uint64_t dropped = 0;
    while (omt_queue_duration_ns(q) > limit && q->count > 1) {
        drop_oldest(q);
        dropped++;
    }
    return dropped;
}

static uint64_t saturating_add(uint64_t a, uint64_t b) {
    return a > UINT64_MAX - b ? UINT64_MAX : a + b;
}

static bool grow(omt_queue *q) {
    if (q->count < q->slots) return true;
    size_t slots = q->slots ? q->slots * 2 : 16;
    omt_frame *frames = calloc(slots, sizeof(omt_frame));
    if (!frames) return false;
    for (size_t i = 0; i < q->count; i++) frames[i] = q->frames[(q->head + i) % q->slots];
    free(q->frames);
    q->frames = frames;
    q->slots = slots;
    q->head = 0;
    return true;
}

static bool interval_of(const omt_frame *f, uint64_t *out) {
    if (f->has_video) return omt_video_interval_ns(&f->video, out);
    if (f->has_audio) return omt_audio_interval_ns(&f->audio, out);
    return false;
}

uint64_t omt_queue_push(omt_queue *q, omt_frame *frame, bool playing) {
    if (!frame->has_video && !frame->has_audio) {
        omt_queue_recycle(q, frame->payload, frame->cap);
        memset(frame, 0, sizeof(*frame));
        return 0;
    }
    uint64_t interval;
    if (interval_of(frame, &interval)) {
        q->interval_ns = interval;
        q->has_interval = true;
    }
    if (frame->len > q->byte_cap || !grow(q)) {
        omt_queue_recycle(q, frame->payload, frame->cap);
        memset(frame, 0, sizeof(*frame));
        return 0;
    }
    q->bytes += frame->len;
    q->frames[(q->head + q->count) % q->slots] = *frame;
    q->count++;
    memset(frame, 0, sizeof(*frame));

    uint64_t dropped = 0;
    while (q->bytes > q->byte_cap && q->count > 0) {
        drop_oldest(q);
        dropped++;
    }
    dropped = saturating_add(
        dropped, drop_while_longer_than(q, saturating_add(q->delay_ns, OMT_TIME_SLACK_NS)));
    if (playing) {
        /* Keeps a running session live when the sender is faster than
         * playout: video holds delay 0 at one frame and anything else at the
         * delay; audio only sheds past its slack. */
        if (q->kind == OMT_QUEUE_AUDIO) {
            dropped = saturating_add(
                dropped,
                drop_while_longer_than(q, saturating_add(q->delay_ns, OMT_AUDIO_SLACK_NS)));
        } else if (q->delay_ns == 0) {
            while (q->count > 1) {
                drop_oldest(q);
                dropped++;
            }
        } else {
            dropped = saturating_add(dropped, drop_while_longer_than(q, q->delay_ns));
        }
    }
    return dropped;
}

void omt_queue_keep_latest_only(omt_queue *q) {
    if (q->kind == OMT_QUEUE_AUDIO) return;
    while (q->count > 1) drop_oldest(q);
}

/* Duration::try_from_secs_f64 rounds to the nearest nanosecond; so does this. */
static bool ratio_ns(int32_t numerator, int32_t denominator, uint64_t *out) {
    if (numerator <= 0 || denominator <= 0) return false;
    double seconds = (double)numerator / (double)denominator;
    if (!isfinite(seconds) || seconds <= 0.0 || seconds > 1.8e10) return false;
    double ns = seconds * 1e9;
    *out = (uint64_t)llround(ns);
    return true;
}

bool omt_video_interval_ns(const omt_video_header *h, uint64_t *out) {
    return ratio_ns(h->frame_rate_d, h->frame_rate_n, out);
}

bool omt_audio_interval_ns(const omt_audio_header *h, uint64_t *out) {
    return ratio_ns(h->samples_per_channel, h->sample_rate, out);
}

void omt_gate_init(omt_fill_gate *g) {
    atomic_init(&g->video, false);
    atomic_init(&g->audio, false);
    atomic_init(&g->audio_missing, false);
}
void omt_gate_set_video(omt_fill_gate *g, bool ready) {
    atomic_store_explicit(&g->video, ready, memory_order_relaxed);
}
void omt_gate_set_audio(omt_fill_gate *g, bool ready) {
    atomic_store_explicit(&g->audio, ready, memory_order_relaxed);
}
void omt_gate_set_audio_missing(omt_fill_gate *g) {
    atomic_store_explicit(&g->audio_missing, true, memory_order_relaxed);
}
bool omt_gate_video_ready(omt_fill_gate *g) {
    return atomic_load_explicit(&g->video, memory_order_relaxed);
}
bool omt_gate_audio_ready(omt_fill_gate *g) {
    return atomic_load_explicit(&g->audio, memory_order_relaxed) ||
           atomic_load_explicit(&g->audio_missing, memory_order_relaxed);
}
