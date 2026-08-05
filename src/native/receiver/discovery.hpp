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

/// Report whether any OMT discovery transport is usable right now.
///
/// `play` asks this before it reports "no source discovered": without a
/// configured Discovery Server or a reachable Avahi bus there is nothing to
/// discover *with*, which is a different operator problem from a source that is
/// simply absent. Whether the answer came from the bus or the settings file is
/// an implementation detail of `discovery.cpp`.
bool discovery_transport_available();

} // namespace omt::native
