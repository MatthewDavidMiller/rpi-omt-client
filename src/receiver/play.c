/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "receiver/play.h"

#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "common/proc.h"
#include "receiver/audio_alsa.h"
#include "receiver/connector.h"
#include "receiver/discovery.h"
#include "receiver/handoff.h"
#include "receiver/video_drm.h"

#define DETAIL 1024

/* Logs the first status-publish failure in each contiguous outage, so a full
 * or briefly missing /run/omt is diagnosable without flooding stderr. */
static void note_status(bool *logged, int result, const omt_err *err) {
    if (result >= 0) {
        *logged = false;
    } else if (!*logged) {
        fprintf(stderr, "playback status publish failed: %s\n", err->msg);
        *logged = true;
    }
}

static void sleep_ms(uint64_t ms) {
    struct timespec ts = {(time_t)(ms / 1000), (long)(ms % 1000) * 1000000L};
    while (nanosleep(&ts, &ts) != 0) {}
}

static bool stopped(atomic_bool *stop) { return atomic_load_explicit(stop, memory_order_relaxed); }

/* Sleeps in short slices so a signal is noticed promptly and the status file
 * keeps its heartbeat while the receiver waits. */
static void wait_with_heartbeat(uint64_t total_ms, omt_playback_status *status,
                                const omt_hdmi *connector, atomic_bool *stop, bool *logged) {
    uint64_t deadline = omt_now_ms() + total_ms;
    while (!stopped(stop) && omt_now_ms() < deadline) {
        uint64_t left = omt_remaining_ms(deadline);
        sleep_ms(left < 100 ? left : 100);
        omt_connector described;
        if (connector)
            omt_hdmi_describe(connector, &described);
        else
            omt_connector_none(&described);
        omt_err err;
        note_status(logged, omt_status_heartbeat(status, &described, &err), &err);
    }
}

void omt_describe_running(const char *base, uint64_t reconnects, uint64_t skipped, uint64_t dropped,
                          char *out, size_t size) {
    omt_buf b;
    omt_buf_init(&b, size);
    omt_buf_puts(&b, base);
    if (reconnects > 0 && skipped > 0)
        omt_buf_printf(&b, " %llu video reconnect(s) and %llu skipped frame(s) in this session.",
                       (unsigned long long)reconnects, (unsigned long long)skipped);
    else if (reconnects > 0)
        omt_buf_printf(&b, " %llu video reconnect(s) in this session.",
                       (unsigned long long)reconnects);
    else if (skipped > 0)
        omt_buf_printf(&b, " %llu skipped frame(s) in this session.", (unsigned long long)skipped);
    if (dropped > 0)
        omt_buf_printf(&b, " %llu frame(s) replaced by a newer one before display in this session.",
                       (unsigned long long)dropped);
    snprintf(out, size, "%s", omt_buf_cstr(&b));
    omt_buf_free(&b);
}

const char *omt_running_detail_get(omt_running_detail *d, const char *base, uint64_t reconnects,
                                   uint64_t skipped, uint64_t dropped) {
    if (!d->valid || d->reconnects != reconnects || d->skipped != skipped ||
        d->dropped != dropped || strcmp(d->base, base) != 0) {
        d->reconnects = reconnects;
        d->skipped = skipped;
        d->dropped = dropped;
        snprintf(d->base, sizeof(d->base), "%s", base);
        omt_describe_running(base, reconnects, skipped, dropped, d->text, sizeof(d->text));
        d->valid = true;
    }
    return d->text;
}

void omt_describe_audio(uint64_t underruns, char *out, size_t size) {
    if (underruns == 0)
        snprintf(out, size, "Playing OMT video and audio.");
    else
        snprintf(out, size,
                 "Playing OMT video and audio. %llu audio underrun(s) in this session; the "
                 "sender's audio is arriving later than the display consumes it.",
                 (unsigned long long)underruns);
}

/* Hands the frame just received to the queue when it is the wanted kind,
 * giving the channel a recycled buffer to read the next one into. Frames the
 * queue drops to make way are added to `dropped`. */
static bool accept_frame(omt_channel *c, omt_queue *q, omt_frame_type kind, uint64_t *dropped) {
    if (c->frame.header.frame_type != kind) return false;
    uint8_t *spare;
    size_t cap;
    omt_queue_take_spare(q, &spare, &cap);
    omt_frame frame;
    omt_channel_take_frame(c, &frame, spare, cap);
    *dropped += omt_queue_push(q, &frame);
    return true;
}

/* Reads frames already in the kernel buffer, each on the drain slice. */
static int drain_ready(omt_channel *c, omt_queue *q, omt_frame_type kind, bool *received,
                       uint64_t *dropped, omt_err *err) {
    for (;;) {
        omt_recv_status st = omt_channel_receive(c, omt_now_ms() + OMT_DRAIN_SLICE_MS, err);
        if (st == OMT_RECV_OK)
            *received |= accept_frame(c, q, kind, dropped);
        else if (st == OMT_RECV_WOULD_BLOCK)
            return 0;
        else
            return -1;
    }
}

/* Copies every complete frame the socket will give without stalling the
 * accept path, then waits until `wait_until` for the next one. Returns -1 on
 * a channel error. */
static int drain(omt_channel *c, omt_queue *q, omt_frame_type kind, uint64_t wait_until,
                 bool *received, uint64_t *dropped, omt_err *err) {
    *received = false;
    if (drain_ready(c, q, kind, received, dropped, err) < 0) return -1;
    if (omt_remaining_ms(wait_until) == 0) return 0;
    omt_recv_status st = omt_channel_receive(c, wait_until, err);
    if (st == OMT_RECV_OK) {
        *received |= accept_frame(c, q, kind, dropped);
        return drain_ready(c, q, kind, received, dropped, err);
    }
    return st == OMT_RECV_WOULD_BLOCK ? 0 : -1;
}

/* ------------------------------------------------------------ audio worker */

typedef struct {
    omt_endpoint endpoint;
    char device[96];
    omt_connector connector;
    omt_playback_status *status;
    atomic_bool active;
    atomic_bool *stop;
    bool status_failed;
} audio_context;

static bool wanted(audio_context *ctx) {
    return atomic_load_explicit(&ctx->active, memory_order_relaxed) && !stopped(ctx->stop);
}

static void audio_status(audio_context *ctx, omt_audio_state state, const char *detail) {
    omt_err err;
    note_status(&ctx->status_failed,
                omt_status_audio(ctx->status, state, detail, &ctx->connector, &err), &err);
}

/* Audio runs independently of video: a failing sink degrades the session
 * rather than ending it, and the worker retries behind a backoff. */
static void *audio_loop(void *raw) {
    audio_context *ctx = raw;
    while (wanted(ctx)) {
        omt_channel channel;
        omt_channel_init(&channel);
        omt_audio_output *output = omt_audio_new();
        omt_err failure;
        failure.msg[0] = 0;
        omt_err err;
        if (!output) {
            omt_err_set(&failure, "out of memory");
        } else if (omt_channel_connect(&channel, &ctx->endpoint, OMT_FRAME_AUDIO,
                                       omt_now_ms() + OMT_CONNECT_TIMEOUT_MS, &err)) {
            char detail[512];
            omt_describe_audio(0, detail, sizeof(detail));
            uint64_t reported = 0;
            uint64_t stall_at = omt_now_ms() + OMT_MEDIA_STALL_MS;
            omt_queue queue;
            omt_queue_init(&queue, OMT_QUEUE_AUDIO, OMT_AUDIO_BYTE_CAP);
            uint64_t trimmed = 0;
            while (wanted(ctx)) {
                /* Paced by the device, not the wall clock: while audio is
                 * queued and the ring is full, wake once a period. */
                uint64_t wait_until = omt_now_ms() + (omt_queue_empty(&queue) ? OMT_RECEIVE_SLICE_MS
                                                                              : OMT_AUDIO_FEED_MS);
                bool received;
                if (drain(&channel, &queue, OMT_FRAME_AUDIO, wait_until, &received, &trimmed,
                          &err) < 0) {
                    if (!omt_channel_connected(&channel)) {
                        failure = err;
                        break;
                    }
                } else if (received) {
                    stall_at = omt_now_ms() + OMT_MEDIA_STALL_MS;
                }
                /* An empty queue is the steady state: what arrived is already
                 * in the ring. The underrun that matters is ALSA's. */
                bool written = false;
                while (omt_audio_room(output)) {
                    omt_frame frame;
                    if (!omt_queue_pop(&queue, &frame)) break;
                    bool ok = omt_audio_write(output, &frame, ctx->device, &err);
                    omt_queue_recycle(&queue, frame.payload, frame.cap);
                    if (!ok) {
                        failure = err;
                        break;
                    }
                    written = true;
                }
                if (failure.msg[0]) break;
                uint64_t underruns = omt_audio_underruns(output);
                if (underruns != reported) {
                    reported = underruns;
                    omt_describe_audio(underruns, detail, sizeof(detail));
                }
                if (written) audio_status(ctx, OMT_AUDIO_RUNNING, detail);
                if (omt_queue_empty(&queue) && omt_now_ms() >= stall_at) {
                    omt_err_set(&failure, "no audio frames for %u seconds on a connected socket",
                                OMT_MEDIA_STALL_MS / 1000);
                    break;
                }
            }
            omt_queue_free(&queue);
        } else {
            failure = err;
        }
        omt_audio_free(output);
        omt_channel_free(&channel);
        if (!wanted(ctx)) break;
        char sanitized[OMT_DETAIL_LIMIT + 1], detail[OMT_DETAIL_LIMIT + 64];
        omt_sanitize_detail(failure.msg, sanitized);
        snprintf(detail, sizeof(detail), "Audio unavailable: %s", sanitized);
        audio_status(ctx, OMT_AUDIO_FAILED, detail);
        /* Back off before reconnecting so a dead sink cannot spin a core. */
        for (int i = 0; i < 10; i++) {
            if (!wanted(ctx)) return NULL;
            sleep_ms(100);
        }
    }
    audio_status(ctx, OMT_AUDIO_STOPPED, "");
    return NULL;
}

typedef struct {
    audio_context *ctx;
    pthread_t thread;
    bool running;
} audio_worker;

static void audio_start(audio_worker *w, const omt_endpoint *ep, const omt_hdmi *connector,
                        omt_playback_status *status, atomic_bool *stop) {
    memset(w, 0, sizeof(*w));
    w->ctx = calloc(1, sizeof(audio_context));
    omt_connector described;
    omt_hdmi_describe(connector, &described);
    if (w->ctx) {
        w->ctx->endpoint = *ep;
        omt_strlcpy(w->ctx->device, connector->alsa_device, sizeof(w->ctx->device));
        w->ctx->connector = described;
        w->ctx->status = status;
        atomic_init(&w->ctx->active, true);
        w->ctx->stop = stop;
        pthread_attr_t attr;
        if (pthread_attr_init(&attr) == 0) {
            pthread_attr_setstacksize(&attr, OMT_AUDIO_STACK);
            /* Process signals stay with the main thread. */
            sigset_t all, previous;
            sigfillset(&all);
            pthread_sigmask(SIG_SETMASK, &all, &previous);
            w->running = pthread_create(&w->thread, &attr, audio_loop, w->ctx) == 0;
            pthread_sigmask(SIG_SETMASK, &previous, NULL);
            pthread_attr_destroy(&attr);
            if (w->running) pthread_setname_np(w->thread, "omt-audio");
        }
    }
    if (!w->running) {
        omt_err err;
        if (omt_status_audio(status, OMT_AUDIO_FAILED,
                             "Audio unavailable: unable to create bounded-stack worker.",
                             &described, &err) < 0)
            fprintf(stderr, "playback status publish failed: %s\n", err.msg);
        free(w->ctx);
        w->ctx = NULL;
    }
}

static void audio_stop(audio_worker *w) {
    if (!w->running) return;
    atomic_store_explicit(&w->ctx->active, false, memory_order_relaxed);
    pthread_join(w->thread, NULL);
    free(w->ctx);
    w->ctx = NULL;
    w->running = false;
}

/* ------------------------------------------------------------ video session */

/* Returns true on a clean end (stop raised), false with `failure` set. */
static bool session(const omt_play_options *o, const omt_hdmi *connector,
                    omt_playback_status *status, atomic_bool *stop, bool *logged,
                    omt_err *failure) {
    omt_endpoint endpoint;
    if (!omt_discover_resolve(o->target, OMT_RESOLVE_TIMEOUT_MS, &endpoint)) {
        omt_err_set(failure, "OMT target was not discovered.");
        return false;
    }
    omt_video_output output;
    if (!omt_video_open(&output, connector->card_path, connector->id, &o->ceiling, failure))
        return false;
    omt_channel video;
    omt_channel_init(&video);
    if (!omt_channel_connect(&video, &endpoint, OMT_FRAME_VIDEO,
                             omt_now_ms() + OMT_CONNECT_TIMEOUT_MS, failure)) {
        omt_channel_free(&video);
        omt_video_close(&output);
        return false;
    }
    audio_worker audio;
    audio_start(&audio, &endpoint, connector, status, stop);
    omt_connector described;
    omt_hdmi_describe(connector, &described);
    omt_err err;
    note_status(
        logged,
        omt_status_video(status, OMT_VIDEO_STARTING, "Waiting for OMT media.", &described, &err),
        &err);

    uint64_t last_frame = omt_now_ms() + OMT_MEDIA_GRACE_MS;
    uint64_t stall_at = omt_now_ms() + OMT_MEDIA_STALL_MS;
    uint64_t next_connector_check = omt_now_ms();
    bool failed = false;
    uint64_t reconnects = 0, skipped = 0, dropped = 0;
    uint32_t attempts = 0;
    omt_running_detail running;
    memset(&running, 0, sizeof(running));
    omt_queue queue;
    omt_queue_init(&queue, OMT_QUEUE_VIDEO, OMT_VIDEO_BYTE_CAP);
    char detail[DETAIL];

    /* No playout clock: each frame is decoded and flipped as soon as it
     * arrives, and the display's own refresh is the only pacing. While a
     * frame is being decoded, whatever arrives behind it waits in the
     * socket; the next drain keeps only the newest of those. */
    while (!stopped(stop)) {
        if (omt_now_ms() >= next_connector_check) {
            next_connector_check = omt_now_ms() + OMT_CONNECTOR_POLL_MS;
            if (!omt_hdmi_is_connected(connector)) {
                omt_err_set(failure, "HDMI display disconnected.");
                failed = true;
                break;
            }
        }
        bool received;
        if (drain(&video, &queue, OMT_FRAME_VIDEO, omt_now_ms() + OMT_RECEIVE_SLICE_MS, &received,
                  &dropped, &err) == 0) {
            if (received) {
                attempts = 0;
                last_frame = omt_now_ms() + OMT_MEDIA_GRACE_MS;
                stall_at = omt_now_ms() + OMT_MEDIA_STALL_MS;
            }
        } else {
            omt_err hb;
            note_status(logged, omt_status_heartbeat(status, &described, &hb), &hb);
            if (!omt_channel_connected(&video)) {
                if (attempts >= OMT_RECOVER_ATTEMPTS) {
                    omt_err_set(failure, "%s; %u in-session video reconnects did not hold.",
                                err.msg, attempts);
                    failed = true;
                    break;
                }
                char sanitized[OMT_DETAIL_LIMIT + 1];
                omt_sanitize_detail(err.msg, sanitized);
                omt_err st;
                note_status(
                    logged,
                    omt_status_video(status, OMT_VIDEO_RETRYING, sanitized, &described, &st), &st);
                wait_with_heartbeat((uint64_t)OMT_RECOVER_BACKOFF_MS * attempts, status, connector,
                                    stop, logged);
                if (stopped(stop)) break;
                attempts++;
                /* One reconnect to the endpoint this session already resolved:
                 * success keeps the mode, the picture, and the audio worker. */
                if (!omt_channel_connect(&video, &endpoint, OMT_FRAME_VIDEO,
                                         omt_now_ms() + OMT_RECOVER_TIMEOUT_MS, &err)) {
                    if (attempts >= OMT_RECOVER_ATTEMPTS) {
                        omt_err_set(failure, "%s; %u in-session video reconnects did not hold.",
                                    err.msg, attempts);
                        failed = true;
                        break;
                    }
                    continue;
                }
                reconnects++;
                last_frame = omt_now_ms() + OMT_MEDIA_GRACE_MS;
                stall_at = omt_now_ms() + OMT_MEDIA_STALL_MS;
                continue;
            }
        }

        omt_frame frame;
        if (omt_queue_pop(&queue, &frame)) {
            bool interlaced = frame.has_video && (frame.video.flags & 1u) != 0;
            omt_present outcome = omt_video_present(&output, &frame, detail, sizeof(detail));
            omt_queue_recycle(&queue, frame.payload, frame.cap);
            omt_err st;
            if (outcome == OMT_PRESENTED || outcome == OMT_PRESENT_SKIPPED) {
                if (outcome == OMT_PRESENT_SKIPPED) skipped++;
                const char *text = omt_running_detail_get(
                    &running, omt_video_presentation_detail(&output, interlaced), reconnects,
                    skipped, dropped);
                note_status(logged,
                            omt_status_video(status, OMT_VIDEO_RUNNING, text, &described, &st),
                            &st);
            } else if (outcome == OMT_PRESENT_UNSUPPORTED) {
                note_status(
                    logged,
                    omt_status_video(status, OMT_VIDEO_UNSUPPORTED_FORMAT, detail, &described, &st),
                    &st);
            } else {
                omt_err_set(failure, "%s", detail);
                failed = true;
                break;
            }
        } else if (omt_now_ms() >= stall_at) {
            omt_err_set(failure, "No video frames for %u seconds on a connected socket.",
                        OMT_MEDIA_STALL_MS / 1000);
            failed = true;
            break;
        } else if (omt_now_ms() >= last_frame) {
            omt_err st;
            note_status(logged,
                        omt_status_video(status, OMT_VIDEO_RETRYING, "Waiting for video frames.",
                                         &described, &st),
                        &st);
        }
    }

    audio_stop(&audio);
    omt_err st;
    note_status(logged, omt_status_audio(status, OMT_AUDIO_STOPPED, "", &described, &st), &st);
    omt_queue_free(&queue);
    omt_channel_free(&video);
    omt_video_close(&output);
    return !failed;
}

void omt_play_run(const omt_play_options *o, omt_playback_status *status, atomic_bool *stop) {
    bool direct = omt_has_prefix(o->target, "omt://");
    bool logged = false;
    omt_connector none;
    omt_connector_none(&none);
    while (!stopped(stop)) {
        omt_err err;
        if (!direct && !omt_discovery_transport_available()) {
            note_status(&logged,
                        omt_status_video(status, OMT_VIDEO_WAITING_FOR_DISCOVERY,
                                         "No configured OMT discovery transport is available.",
                                         &none, &err),
                        &err);
            wait_with_heartbeat(1000, status, NULL, stop, &logged);
            continue;
        }
        omt_hdmi connector;
        if (!omt_hdmi_find(o->preference, &connector)) {
            note_status(&logged,
                        omt_status_video(status, OMT_VIDEO_WAITING_FOR_HDMI,
                                         "No supported HDMI display is connected.", &none, &err),
                        &err);
            wait_with_heartbeat(1000, status, NULL, stop, &logged);
            continue;
        }
        omt_err failure;
        failure.msg[0] = 0;
        if (!session(o, &connector, status, stop, &logged, &failure) && !stopped(stop)) {
            char sanitized[OMT_DETAIL_LIMIT + 1];
            omt_sanitize_detail(failure.msg, sanitized);
            omt_connector described;
            omt_hdmi_describe(&connector, &described);
            note_status(&logged,
                        omt_status_video(status, OMT_VIDEO_RETRYING, sanitized, &described, &err),
                        &err);
            wait_with_heartbeat(o->retry_ms, status, &connector, stop, &logged);
        }
    }
    omt_err err;
    note_status(&logged, omt_status_stopped(status, "Playback stopped.", &err), &err);
}
