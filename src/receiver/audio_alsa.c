/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "receiver/audio_alsa.h"

#include <alsa/asoundlib.h>
#include <errno.h>
#include <stdlib.h>
#include <string.h>

#include "common/proc.h"

struct omt_audio_output {
    snd_pcm_t *pcm;
    char device[96];
    int32_t sample_rate;
    int32_t channels;
    float *interleaved;
    size_t interleaved_cap;
    snd_pcm_uframes_t buffer_frames;
    snd_pcm_uframes_t target_frames;
    /* Underruns recovered this session. Every one is a gap the operator
     * heard, so it is counted and survives a reopen. */
    uint64_t underruns;
};

static void alsa_error(omt_err *err, const char *prefix, const char *function, int code) {
    omt_err_set(err, "%s: ALSA function '%s' failed with error '%s'", prefix, function,
                snd_strerror(code));
}

omt_audio_output *omt_audio_new(void) { return calloc(1, sizeof(omt_audio_output)); }

static void close_pcm(omt_audio_output *a) {
    if (a->pcm) {
        snd_pcm_drop(a->pcm);
        snd_pcm_close(a->pcm);
        a->pcm = NULL;
    }
    a->device[0] = 0;
    a->sample_rate = 0;
    a->channels = 0;
}

void omt_audio_free(omt_audio_output *a) {
    if (!a) return;
    close_pcm(a);
    free(a->interleaved);
    free(a);
}

uint64_t omt_audio_underruns(const omt_audio_output *a) { return a->underruns; }

bool omt_audio_room(omt_audio_output *a) {
    if (!a->pcm) return true;
    snd_pcm_sframes_t avail = snd_pcm_avail_update(a->pcm);
    /* An underrun or suspend reads as room: the write path recovers it. */
    if (avail < 0) return true;
    snd_pcm_uframes_t used = (snd_pcm_uframes_t)avail >= a->buffer_frames
                                 ? 0
                                 : a->buffer_frames - (snd_pcm_uframes_t)avail;
    return used < a->target_frames;
}

static bool configure(omt_audio_output *a, const char *device, int32_t rate, int32_t channels,
                      omt_err *err) {
    close_pcm(a);
    snd_pcm_t *pcm = NULL;
    int rc = snd_pcm_open(&pcm, device, SND_PCM_STREAM_PLAYBACK, SND_PCM_NONBLOCK);
    if (rc < 0) {
        alsa_error(err, "Unable to open audio device", "snd_pcm_open", rc);
        return false;
    }
    /* The _malloc forms, because the _alloca macros expand to alloca. */
    snd_pcm_hw_params_t *hw = NULL;
    snd_pcm_sw_params_t *sw = NULL;
    if ((rc = snd_pcm_hw_params_malloc(&hw)) < 0 || (rc = snd_pcm_sw_params_malloc(&sw)) < 0) {
        snd_pcm_hw_params_free(hw);
        alsa_error(err, "Unable to configure audio device", "snd_pcm_hw_params_malloc", rc);
        snd_pcm_close(pcm);
        return false;
    }
    const char *step = "snd_pcm_hw_params_any";
    unsigned int urate = (unsigned)rate, buffer_time = OMT_AUDIO_BUFFER_US,
                 period_time = OMT_AUDIO_PERIOD_US;
    int dir = 0;
    snd_pcm_uframes_t buffer_frames = 0, period_frames = 0;
    if ((rc = snd_pcm_hw_params_any(pcm, hw)) < 0) goto hw_failed;
    step = "snd_pcm_hw_params_set_channels";
    if ((rc = snd_pcm_hw_params_set_channels(pcm, hw, (unsigned)channels)) < 0) goto hw_failed;
    step = "snd_pcm_hw_params_set_rate_near";
    if ((rc = snd_pcm_hw_params_set_rate_near(pcm, hw, &urate, &dir)) < 0) goto hw_failed;
    step = "snd_pcm_hw_params_set_format";
    if ((rc = snd_pcm_hw_params_set_format(pcm, hw, SND_PCM_FORMAT_FLOAT_LE)) < 0) goto hw_failed;
    step = "snd_pcm_hw_params_set_access";
    if ((rc = snd_pcm_hw_params_set_access(pcm, hw, SND_PCM_ACCESS_RW_INTERLEAVED)) < 0)
        goto hw_failed;
    step = "snd_pcm_hw_params_set_buffer_time_near";
    dir = 0;
    if ((rc = snd_pcm_hw_params_set_buffer_time_near(pcm, hw, &buffer_time, &dir)) < 0)
        goto hw_failed;
    step = "snd_pcm_hw_params_set_period_time_near";
    dir = 0;
    if ((rc = snd_pcm_hw_params_set_period_time_near(pcm, hw, &period_time, &dir)) < 0)
        goto hw_failed;
    step = "snd_pcm_hw_params";
    if ((rc = snd_pcm_hw_params(pcm, hw)) < 0) goto hw_failed;
    /* The device refines both sizes; the thresholds use what it chose. */
    step = "snd_pcm_hw_params_get_buffer_size";
    if ((rc = snd_pcm_hw_params_get_buffer_size(hw, &buffer_frames)) < 0) goto hw_failed;
    step = "snd_pcm_hw_params_get_period_size";
    if ((rc = snd_pcm_hw_params_get_period_size(hw, &period_frames, &dir)) < 0) goto hw_failed;

    long start = omt_audio_prefill_frames(rate, (long)buffer_frames, (long)period_frames);
    step = "snd_pcm_sw_params_current";
    if ((rc = snd_pcm_sw_params_current(pcm, sw)) < 0) goto sw_failed;
    step = "snd_pcm_sw_params_set_avail_min";
    if ((rc = snd_pcm_sw_params_set_avail_min(pcm, sw, period_frames)) < 0) goto sw_failed;
    step = "snd_pcm_sw_params_set_start_threshold";
    if ((rc = snd_pcm_sw_params_set_start_threshold(pcm, sw, (snd_pcm_uframes_t)start)) < 0)
        goto sw_failed;
    /* The ring running dry stays an error the writer is told about, so an
     * underrun is counted and recovered rather than passing as silence. */
    step = "snd_pcm_sw_params_set_stop_threshold";
    if ((rc = snd_pcm_sw_params_set_stop_threshold(pcm, sw, buffer_frames)) < 0) goto sw_failed;
    step = "snd_pcm_sw_params";
    if ((rc = snd_pcm_sw_params(pcm, sw)) < 0) goto sw_failed;

    a->pcm = pcm;
    a->buffer_frames = buffer_frames;
    a->target_frames = (snd_pcm_uframes_t)omt_audio_target_frames(rate, (long)buffer_frames, start);
    omt_strlcpy(a->device, device, sizeof(a->device));
    a->sample_rate = rate;
    a->channels = channels;
    snd_pcm_hw_params_free(hw);
    snd_pcm_sw_params_free(sw);
    return true;
hw_failed:
    alsa_error(err, "Unable to configure audio device", step, rc);
    goto failed;
sw_failed:
    alsa_error(err, "Unable to configure audio timing", step, rc);
failed:
    snd_pcm_hw_params_free(hw);
    snd_pcm_sw_params_free(sw);
    snd_pcm_close(pcm);
    return false;
}

/* Waits for the device to take more samples, giving up once one write has
 * made no progress for the write timeout. */
static bool stall(omt_audio_output *a, uint64_t *since, omt_err *err) {
    if (*since == 0) *since = omt_now_ms();
    if (omt_now_ms() - *since > OMT_AUDIO_WRITE_TIMEOUT_MS) {
        omt_err_set(err, "Audio device remained unavailable for one second");
        return false;
    }
    (void)snd_pcm_wait(a->pcm, 100);
    return true;
}

static bool play(omt_audio_output *a, size_t samples, size_t channels, omt_err *err) {
    size_t offset = 0;
    uint64_t stalled_since = 0;
    /* One frame needing several recoveries is still one gap, counted once. */
    bool recovered = false;
    while (offset < samples) {
        snd_pcm_sframes_t written =
            snd_pcm_writei(a->pcm, a->interleaved + offset * channels, samples - offset);
        if (written > 0) {
            offset += (size_t)written;
            stalled_since = 0;
        } else if (written == 0 || written == -EAGAIN) {
            /* A full ring is the device asking for time, not a fault. */
            if (!stall(a, &stalled_since, err)) return false;
        } else {
            /* Recover from an underrun or a suspend once; a second failure
             * means the sink is gone. Recovery re-prepares the device, so the
             * start threshold rebuilds the cushion before it plays. */
            int rc = snd_pcm_recover(a->pcm, (int)written, 1);
            if (rc < 0) {
                omt_err_set(err,
                            "Unable to write audio: ALSA function 'snd_pcm_recover' failed with "
                            "error '%s'",
                            snd_strerror(rc));
                return false;
            }
            recovered = true;
            if (!stall(a, &stalled_since, err)) return false;
        }
    }
    if (recovered && a->underruns < UINT64_MAX) a->underruns++;
    return true;
}

bool omt_audio_write(omt_audio_output *a, const omt_frame *frame, const char *device,
                     omt_err *err) {
    if (!frame->has_audio) {
        omt_err_set(err, "not an OMT audio frame");
        return false;
    }
    const omt_audio_header *h = &frame->audio;
    if (!a->pcm || a->sample_rate != h->sample_rate || a->channels != h->channels ||
        strcmp(a->device, device) != 0) {
        if (!configure(a, device, h->sample_rate, h->channels, err)) return false;
    }
    size_t channels = (size_t)h->channels, samples = (size_t)h->samples_per_channel;
    size_t required = samples * channels; /* both bounded by the protocol */
    if (required > a->interleaved_cap) {
        float *grown = realloc(a->interleaved, required * sizeof(float));
        if (!grown) {
            omt_err_set(err, "Unable to allocate bounded audio buffer");
            return false;
        }
        a->interleaved = grown;
        a->interleaved_cap = required;
    }
    const uint8_t *body;
    size_t body_len;
    if (!omt_frame_media(frame, OMT_AUDIO_HEADER_SIZE, &body, &body_len)) {
        omt_err_set(err, "truncated OMT audio frame");
        return false;
    }
    if (!omt_audio_interleave(body, body_len, h->active_channels, samples, channels, a->interleaved,
                              a->interleaved_cap, err))
        return false;
    return play(a, samples, channels, err);
}
