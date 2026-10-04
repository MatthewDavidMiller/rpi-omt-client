/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The pure half of the audio path, kept out of audio_alsa.c so the suites can
 * exercise it without linking ALSA.
 */
#include <string.h>

#include "receiver/audio_alsa.h"

static long clamp(long v, long lo, long hi) { return v < lo ? lo : v > hi ? hi : v; }

/* The start threshold: the prefill at this rate, never less than one period
 * and never more than the ring, because a threshold outside that range is one
 * the device can never reach. */
long omt_audio_prefill_frames(int32_t sample_rate, long buffer_frames, long period_frames) {
    long wanted = sample_rate > 0 ? (long)sample_rate * (long)OMT_AUDIO_PREFILL_MS / 1000 : 0;
    long floor = period_frames > 1 ? period_frames : 1;
    long ceiling = buffer_frames > floor ? buffer_frames : floor;
    return clamp(wanted, floor, ceiling);
}

long omt_audio_target_frames(int32_t sample_rate, long buffer_frames, long start) {
    long wanted = sample_rate > 0 ? (long)sample_rate * (long)OMT_AUDIO_RING_TARGET_MS / 1000 : 0;
    long ceiling = buffer_frames > start ? buffer_frames : start;
    return clamp(wanted, start, ceiling);
}

bool omt_audio_interleave(const uint8_t *body, size_t body_len, uint32_t active, size_t samples,
                          size_t channels, float *out, size_t out_len, omt_err *err) {
    size_t cursor = 0;
    for (size_t channel = 0; channel < channels; channel++) {
        /* Past the mask width there is no bit to test: a channel that cannot
         * be proven active is silence. */
        bool on = channel < 32 && (active & (1u << channel)) != 0;
        for (size_t sample = 0; sample < samples; sample++) {
            float value = 0.0f;
            if (on) {
                if (body_len - cursor < 4) {
                    omt_err_set(err, "truncated OMT audio frame");
                    return false;
                }
                uint32_t bits = omt_le32(body + cursor);
                memcpy(&value, &bits, sizeof(value));
                cursor += 4;
            }
            size_t slot = sample * channels + channel;
            if (slot >= out_len) {
                omt_err_set(err, "OMT audio output buffer is too short");
                return false;
            }
            out[slot] = value;
        }
    }
    return true;
}
