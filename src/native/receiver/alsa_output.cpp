// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#include "alsa_output.hpp"

#include "omt/omt_wire.h"

#include <cerrno>
#include <cstring>

namespace omt::native {

AlsaOutput::~AlsaOutput()
{
    close();
}

void AlsaOutput::close()
{
    if (pcm_ != nullptr) {
        (void)snd_pcm_drop(pcm_);
        (void)snd_pcm_close(pcm_);
        pcm_ = nullptr;
    }
    sample_rate_ = 0;
    channels_ = 0;
    device_.clear();
    interleaved_.clear();
}

bool AlsaOutput::configure(
    const std::string& device,
    int sample_rate,
    int channels,
    std::string& error)
{
    close();
    int result = snd_pcm_open(&pcm_, device.c_str(), SND_PCM_STREAM_PLAYBACK, SND_PCM_NONBLOCK);
    if (result < 0) {
        error = std::string("Unable to open audio device: ") + snd_strerror(result);
        pcm_ = nullptr;
        return false;
    }
    result = snd_pcm_set_params(
        pcm_,
        SND_PCM_FORMAT_FLOAT_LE,
        SND_PCM_ACCESS_RW_INTERLEAVED,
        static_cast<unsigned int>(channels),
        static_cast<unsigned int>(sample_rate),
        1,
        80'000u);
    if (result < 0) {
        error = std::string("Unable to configure audio device: ") + snd_strerror(result);
        close();
        return false;
    }
    sample_rate_ = sample_rate;
    channels_ = channels;
    device_ = device;
    return true;
}

bool AlsaOutput::write(const Frame& frame, const std::string& device, std::string& error)
{
    if (frame.header.type != OMT_FRAME_AUDIO) {
        error = "not an OMT audio frame";
        return false;
    }
    if (pcm_ == nullptr || sample_rate_ != frame.audio.sample_rate || channels_ != frame.audio.channels ||
        device_ != device) {
        if (!configure(device, frame.audio.sample_rate, frame.audio.channels, error)) {
            return false;
        }
    }
    std::size_t channels = static_cast<std::size_t>(frame.audio.channels);
    std::size_t samples = static_cast<std::size_t>(frame.audio.samples_per_channel);
    interleaved_.resize(channels * samples);
    std::size_t source_offset = OMT_WIRE_AUDIO_HEADER_SIZE;
    for (std::size_t channel = 0u; channel < channels; ++channel) {
        bool active = (frame.audio.active_channels & (1u << channel)) != 0u;
        for (std::size_t sample = 0u; sample < samples; ++sample) {
            float value = 0.0F;
            if (active) {
                if (source_offset + sizeof(float) > frame.payload.size() - frame.header.metadata_length) {
                    error = "truncated OMT audio frame";
                    return false;
                }
                std::memcpy(&value, frame.payload.data() + source_offset, sizeof(value));
                source_offset += sizeof(value);
            }
            interleaved_[sample * channels + channel] = value;
        }
    }
    std::size_t offset = 0u;
    int wait_timeouts = 0;
    while (offset < samples) {
        const auto remaining = static_cast<snd_pcm_uframes_t>(samples - offset);
        snd_pcm_sframes_t written = snd_pcm_writei(
            pcm_, interleaved_.data() + offset * channels, remaining);
        if (written > 0) {
            offset += static_cast<std::size_t>(written);
            wait_timeouts = 0;
            continue;
        }
        if (written == 0 || written == -EAGAIN) {
            const int ready = snd_pcm_wait(pcm_, 100);
            if (ready > 0) {
                wait_timeouts = 0;
                continue;
            }
            if (ready == 0 && ++wait_timeouts < 10) continue;
            if (ready == 0) {
                error = "Audio device remained unavailable for one second";
                return false;
            }
            written = ready;
        }
        const int recovered = snd_pcm_recover(pcm_, static_cast<int>(written), 1);
        if (recovered >= 0) continue;
        error = std::string("Unable to write audio: ") + snd_strerror(recovered);
        return false;
    }
    return true;
}

} // namespace omt::native
