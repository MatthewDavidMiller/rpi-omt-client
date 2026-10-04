/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Source discovery. A configured central discovery server takes precedence
 * over mDNS, as the appliance's settings contract specifies; with no server
 * configured the receiver browses _omt._tcp through the host's Avahi daemon.
 */
#ifndef OMT_RECEIVER_DISCOVERY_H
#define OMT_RECEIVER_DISCOVERY_H

#include "receiver/channel.h"

/* The receiver never tracks more sources than the dashboard can present. */
#define OMT_MAX_SOURCES 256

typedef struct {
    char name[OMT_SOURCE_NAME_MAX_BYTES + 1];
    omt_endpoint endpoint;
} omt_source;

/* A bounded, name-keyed set of sources; insertion replaces an existing name. */
typedef struct {
    omt_source items[OMT_MAX_SOURCES];
    size_t count;
} omt_source_set;

void omt_sources_clear(omt_source_set *s);
/* Inserts or replaces; returns false when full and the name is new. */
bool omt_sources_put(omt_source_set *s, const char *name, const omt_endpoint *ep);
void omt_sources_remove(omt_source_set *s, const char *name);
void omt_sources_sort(omt_source_set *s);

/* The configured central discovery server's target, if storage names one. */
bool omt_configured_server(char out[OMT_TARGET_MAX_BYTES + 1]);
bool omt_discovery_transport_available(void);
/* Browses every configured transport for `wait_ms`; sorted by name. */
void omt_discover_sources(uint64_t wait_ms, omt_source_set *out);
/* Turns a target into an endpoint, discovering it by name if needed. */
bool omt_discover_resolve(const char *target, uint64_t wait_ms, omt_endpoint *out);
/* Re-validates a discovered address through the shared target grammar, and
 * refuses literals no connect can use (unscoped link-local IPv6 and so on). */
bool omt_endpoint_from_parts(const char *address, uint16_t port, omt_endpoint *out);

/* One OMTAddress announcement read in a single pass. */
typedef struct {
    char name[OMT_SOURCE_NAME_MAX_BYTES + 1];
    bool has_endpoint;
    omt_endpoint endpoint;
    bool removed;
} omt_announcement;
bool omt_announcement_read(const char *doc, size_t len, omt_announcement *out);

/* Avahi over D-Bus (mdns_dbus.c). */
bool omt_mdns_available(void);
void omt_mdns_browse(uint64_t deadline_ms, size_t capacity, omt_source_set *out);

#endif
