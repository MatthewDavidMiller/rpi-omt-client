// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#ifndef OMT_RECEIVER_DISCOVERY_H
#define OMT_RECEIVER_DISCOVERY_H

#include "native_types.h"

size_t omt_discover_sources(omt_source *sources, size_t capacity, int wait_ms);
bool omt_resolve_target(const char *target, int wait_ms, omt_endpoint *endpoint);
bool omt_discovery_transport_available(void);

#endif
