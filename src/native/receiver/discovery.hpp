// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#pragma once

#include "native_types.hpp"

#include <chrono>
#include <optional>
#include <string_view>
#include <vector>

namespace omt::native {

std::vector<Source> discover_sources(std::chrono::milliseconds wait);
std::optional<Endpoint> resolve_target(std::string_view target, std::chrono::milliseconds wait);
bool avahi_bus_available();
bool discovery_transport_available();

} // namespace omt::native
