// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#pragma once

#include "native_types.hpp"

#include <alsa/asoundlib.h>

#include <string>
#include <vector>

namespace omt::native {

class AlsaOutput final {
public:
    AlsaOutput() = default;
    ~AlsaOutput();
    AlsaOutput(const AlsaOutput&) = delete;
    AlsaOutput& operator=(const AlsaOutput&) = delete;

    bool write(const Frame& frame, const std::string& device, std::string& error);
    void close();

private:
    bool configure(const std::string& device, int sample_rate, int channels, std::string& error);

    snd_pcm_t* pcm_{};
    int sample_rate_{};
    int channels_{};
    std::string device_;
    std::vector<float> interleaved_;
};

} // namespace omt::native
