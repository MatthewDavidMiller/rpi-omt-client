// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#pragma once

#include "native_types.hpp"

#include <string>

namespace omt::native {

class OmtChannel final {
public:
    OmtChannel() = default;
    ~OmtChannel();
    OmtChannel(const OmtChannel&) = delete;
    OmtChannel& operator=(const OmtChannel&) = delete;

    bool connect(const Endpoint& endpoint, omt_frame_type subscription, Deadline deadline, std::string& error);
    bool receive(Frame& frame, Deadline deadline, std::string& error);
    void close();
    [[nodiscard]] bool connected() const { return socket_ >= 0; }

private:
    bool send_subscription(omt_frame_type type, Deadline deadline, std::string& error);
    bool read_exact(std::uint8_t* output, std::size_t length, Deadline deadline, std::string& error);
    bool write_exact(const std::uint8_t* input, std::size_t length, Deadline deadline, std::string& error);

    int socket_{-1};
};

} // namespace omt::native
