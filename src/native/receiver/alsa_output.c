// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#include "alsa_output.h"

#include <alsa/asoundlib.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

struct omt_alsa_output {
    snd_pcm_t *pcm;
    int sample_rate;
    int channels;
    char device[OMT_ALSA_DEVICE_CAPACITY];
    float *interleaved;
    size_t interleaved_capacity;
};

omt_alsa_output *omt_alsa_output_create(void)
{
    return calloc(1u, sizeof(omt_alsa_output));
}

void omt_alsa_output_close(omt_alsa_output *output)
{
    if (output == NULL) return;
    if (output->pcm != NULL) {
        (void)snd_pcm_drop(output->pcm);
        (void)snd_pcm_close(output->pcm);
        output->pcm = NULL;
    }
    output->sample_rate = 0;
    output->channels = 0;
    output->device[0] = '\0';
}

void omt_alsa_output_destroy(omt_alsa_output *output)
{
    if (output != NULL) {
        omt_alsa_output_close(output);
        free(output->interleaved);
        free(output);
    }
}

static bool configure(omt_alsa_output *output, const char *device, int sample_rate, int channels,
                      char *error, size_t capacity)
{
    omt_alsa_output_close(output);
    int result = snd_pcm_open(&output->pcm, device, SND_PCM_STREAM_PLAYBACK, SND_PCM_NONBLOCK);
    if (result < 0) {
        (void)snprintf(error, capacity, "Unable to open audio device: %s", snd_strerror(result));
        output->pcm = NULL;
        return false;
    }
    result = snd_pcm_set_params(output->pcm, SND_PCM_FORMAT_FLOAT_LE,
        SND_PCM_ACCESS_RW_INTERLEAVED, (unsigned int)channels, (unsigned int)sample_rate, 1, 80000u);
    if (result < 0) {
        (void)snprintf(error, capacity, "Unable to configure audio device: %s", snd_strerror(result));
        omt_alsa_output_close(output);
        return false;
    }
    output->sample_rate = sample_rate;
    output->channels = channels;
    return omt_copy_string(output->device, sizeof(output->device), device);
}

bool omt_alsa_output_write(omt_alsa_output *output, const omt_frame *frame, const char *device,
                           char *error, size_t capacity)
{
    if (frame->header.type != OMT_FRAME_AUDIO) {
        omt_set_error(error, capacity, "not an OMT audio frame");
        return false;
    }
    if (output->pcm == NULL || output->sample_rate != frame->audio.sample_rate ||
        output->channels != frame->audio.channels || strcmp(output->device, device) != 0) {
        if (!configure(output, device, frame->audio.sample_rate, frame->audio.channels, error, capacity))
            return false;
    }
    size_t channels = (size_t)frame->audio.channels;
    size_t samples = (size_t)frame->audio.samples_per_channel;
    if (channels != 0u && samples > SIZE_MAX / channels) {
        omt_set_error(error, capacity, "OMT audio sample count is out of range");
        return false;
    }
    size_t required = channels * samples;
    if (required > output->interleaved_capacity) {
        float *replacement = realloc(output->interleaved, required * sizeof(float));
        if (replacement == NULL) {
            omt_set_error(error, capacity, "Unable to allocate bounded audio buffer");
            return false;
        }
        output->interleaved = replacement;
        output->interleaved_capacity = required;
    }
    size_t source_offset = OMT_WIRE_AUDIO_HEADER_SIZE;
    for (size_t channel = 0u; channel < channels; ++channel) {
        bool active = (frame->audio.active_channels & (1u << channel)) != 0u;
        for (size_t sample = 0u; sample < samples; ++sample) {
            float value = 0.0F;
            if (active) {
                if (source_offset + sizeof(float) >
                    (size_t)frame->header.data_length - frame->header.metadata_length) {
                    omt_set_error(error, capacity, "truncated OMT audio frame");
                    return false;
                }
                memcpy(&value, frame->payload + source_offset, sizeof(value));
                source_offset += sizeof(value);
            }
            output->interleaved[sample * channels + channel] = value;
        }
    }
    size_t offset = 0u;
    int wait_timeouts = 0;
    while (offset < samples) {
        snd_pcm_uframes_t remaining = (snd_pcm_uframes_t)(samples - offset);
        snd_pcm_sframes_t written = snd_pcm_writei(
            output->pcm, output->interleaved + offset * channels, remaining);
        if (written > 0) {
            offset += (size_t)written;
            wait_timeouts = 0;
            continue;
        }
        if (written == 0 || written == -EAGAIN) {
            int ready = snd_pcm_wait(output->pcm, 100);
            if (ready > 0) { wait_timeouts = 0; continue; }
            if (ready == 0 && ++wait_timeouts < 10) continue;
            if (ready == 0) {
                omt_set_error(error, capacity, "Audio device remained unavailable for one second");
                return false;
            }
            written = ready;
        }
        int recovered = snd_pcm_recover(output->pcm, (int)written, 1);
        if (recovered >= 0) continue;
        (void)snprintf(error, capacity, "Unable to write audio: %s", snd_strerror(recovered));
        return false;
    }
    return true;
}
