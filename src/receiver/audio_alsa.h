/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * HDMI audio through ALSA. OMT sends planar float samples with a bitmask of
 * active channels; inactive channels are silence and absent from the body, so
 * the interleaver writes zeros for them without advancing the source cursor.
 */
#ifndef OMT_RECEIVER_AUDIO_ALSA_H
#define OMT_RECEIVER_AUDIO_ALSA_H

#include "receiver/channel.h"

/* Ring capacity, not latency: what the device may hold, so a burst off the
 * network can be written in one go. */
#define OMT_AUDIO_BUFFER_US 240000u
#define OMT_AUDIO_PERIOD_US 20000u
/* Audio queued before the device may start: the cushion against one late
 * frame becoming an audible gap. */
#define OMT_AUDIO_PREFILL_MS 100u
/* How full the worker keeps the ring; between the start threshold and the
 * ring size. */
#define OMT_AUDIO_RING_TARGET_MS 160u
#define OMT_AUDIO_WRITE_TIMEOUT_MS 1000u

typedef struct omt_audio_output omt_audio_output;

omt_audio_output *omt_audio_new(void);
void omt_audio_free(omt_audio_output *a);
uint64_t omt_audio_underruns(const omt_audio_output *a);
/* Whether the ring holds less than the fill target (or cannot say). */
bool omt_audio_room(omt_audio_output *a);
/* Writes one frame, reopening the device if the format or device changed. */
OMT_NODISCARD bool omt_audio_write(omt_audio_output *a, const omt_frame *frame, const char *device,
                                   omt_err *err);

/* Pure helpers, exposed for the tests. */
long omt_audio_prefill_frames(int32_t sample_rate, long buffer_frames, long period_frames);
long omt_audio_target_frames(int32_t sample_rate, long buffer_frames, long start);
OMT_NODISCARD bool omt_audio_interleave(const uint8_t *body, size_t body_len, uint32_t active,
                                        size_t samples, size_t channels, float *out, size_t out_len,
                                        omt_err *err);

#endif
