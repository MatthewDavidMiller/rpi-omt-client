/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * _omt._tcp browsing through the host's Avahi daemon over D-Bus, without the
 * Avahi client libraries. Only the two Avahi interfaces the browse needs are
 * called, every reply is re-validated against the shared target grammar, and
 * the whole browse -- method calls included -- is bounded by the caller's
 * deadline: a daemon that stops answering ends the browse on the timer.
 */
#include <stdlib.h>
#include <string.h>

#include "common/proc.h"
#include "receiver/dbus.h"
#include "receiver/discovery.h"

#define SERVICE "org.freedesktop.Avahi"
#define SERVER_INTERFACE "org.freedesktop.Avahi.Server"
#define BROWSER_INTERFACE "org.freedesktop.Avahi.ServiceBrowser"
#define RESOLVER_INTERFACE "org.freedesktop.Avahi.ServiceResolver"
#define SERVICE_TYPE "_omt._tcp"
/* AVAHI_IF_UNSPEC and AVAHI_PROTO_UNSPEC. */
#define UNSPEC (-1)
/* Cap on concurrently resolving services, bounding a flooded network. */
#define MAX_RESOLVERS 256
/* Cap on remembered removals so a flapping network cannot grow the set. */
#define MAX_REMOVED 256
/* How long `available` waits for the bus. */
#define AVAILABLE_TIMEOUT_MS 2000u

bool omt_mdns_available(void) {
    omt_dbus bus;
    omt_err err;
    uint64_t deadline = omt_now_ms() + AVAILABLE_TIMEOUT_MS;
    if (!omt_dbus_open_system(&bus, deadline, &err)) return false;
    omt_dbus_arg arg = {.type = 's', .v.s = SERVICE};
    omt_dbus_message reply;
    bool owned = false;
    if (omt_dbus_call(&bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus",
                      "NameHasOwner", &arg, 1, deadline, &reply, &err)) {
        omt_dbus_reader r;
        omt_dbus_reader_init(&r, &reply);
        owned = strcmp(reply.signature, "b") == 0 && omt_dbus_read_bool(&r) && !r.failed;
        omt_dbus_message_free(&reply);
    }
    omt_dbus_close(&bus);
    return owned;
}

typedef struct {
    char *paths[MAX_RESOLVERS];
    size_t count;
} path_list;

typedef struct {
    char names[MAX_REMOVED][OMT_SOURCE_NAME_MAX_BYTES + 1];
    size_t count;
} removed_set;

static bool removed_contains(const removed_set *r, const char *name) {
    for (size_t i = 0; i < r->count; i++)
        if (strcmp(r->names[i], name) == 0) return true;
    return false;
}

static bool is_signal(const omt_dbus_message *m, const char *interface, const char *member) {
    return m->type == OMT_DBUS_SIGNAL && m->member && m->interface &&
           strcmp(m->member, member) == 0 && strcmp(m->interface, interface) == 0;
}

/* ItemNew and ItemRemove carry `iisssu`: interface, protocol, name, type,
 * domain, flags. */
typedef struct {
    int32_t interface;
    int32_t protocol;
    const char *name;
    const char *type;
    const char *domain;
} browser_item;

static bool read_browser_item(const omt_dbus_message *m, browser_item *out) {
    if (strcmp(m->signature, "iisssu") != 0) return false;
    omt_dbus_reader r;
    omt_dbus_reader_init(&r, m);
    out->interface = omt_dbus_read_i32(&r);
    out->protocol = omt_dbus_read_i32(&r);
    out->name = omt_dbus_read_string(&r);
    out->type = omt_dbus_read_string(&r);
    out->domain = omt_dbus_read_string(&r);
    (void)omt_dbus_read_u32(&r);
    return !r.failed;
}

/* Found carries `iissssisqaayu`: the browser item's five leading fields,
 * then host, address protocol, address, port, the TXT records, and flags. */
static bool resolved_source(const omt_dbus_message *m, omt_source *out) {
    if (strcmp(m->signature, "iissssisqaayu") != 0) return false;
    omt_dbus_reader r;
    omt_dbus_reader_init(&r, m);
    (void)omt_dbus_read_i32(&r);
    (void)omt_dbus_read_i32(&r);
    const char *name = omt_dbus_read_string(&r);
    (void)omt_dbus_read_string(&r);
    (void)omt_dbus_read_string(&r);
    (void)omt_dbus_read_string(&r);
    (void)omt_dbus_read_i32(&r);
    const char *address = omt_dbus_read_string(&r);
    uint16_t port = omt_dbus_read_u16(&r);
    omt_dbus_skip_aay(&r);
    (void)omt_dbus_read_u32(&r);
    if (r.failed || r.pos != r.end) return false;
    if (!omt_is_valid_source_name(name, strlen(name))) return false;
    if (!omt_endpoint_from_parts(address, port, &out->endpoint)) return false;
    omt_strlcpy(out->name, name, sizeof(out->name));
    return true;
}

static char *reply_path(omt_dbus_message *reply) {
    char *copy = NULL;
    if (strcmp(reply->signature, "o") == 0) {
        omt_dbus_reader r;
        omt_dbus_reader_init(&r, reply);
        const char *path = omt_dbus_read_string(&r);
        if (!r.failed && path) copy = strdup(path);
    }
    omt_dbus_message_free(reply);
    return copy;
}

void omt_mdns_browse(uint64_t deadline_ms, size_t capacity, omt_source_set *out) {
    omt_sources_clear(out);
    omt_dbus bus;
    omt_err err;
    if (!omt_dbus_open_system(&bus, deadline_ms, &err)) return;
    omt_dbus_arg browse_args[] = {
        {.type = 'i', .v.i = UNSPEC},
        {.type = 'i', .v.i = UNSPEC},
        {.type = 's', .v.s = SERVICE_TYPE},
        {.type = 's', .v.s = ""},
        {.type = 'u', .v.u = 0},
    };
    omt_dbus_message reply;
    if (!omt_dbus_call(&bus, SERVICE, "/", SERVER_INTERFACE, "ServiceBrowserNew", browse_args, 5,
                       deadline_ms, &reply, &err)) {
        omt_dbus_close(&bus);
        return;
    }
    char *browser = reply_path(&reply);
    if (!browser) {
        omt_dbus_close(&bus);
        return;
    }
    path_list *resolvers = calloc(1, sizeof(path_list));
    removed_set *removed = calloc(1, sizeof(removed_set));
    while (resolvers && removed) {
        omt_dbus_message m;
        /* The timer fired, the bus closed, or a message failed to decode. */
        if (!omt_dbus_next(&bus, deadline_ms, &m, &err)) break;
        browser_item item;
        if (is_signal(&m, BROWSER_INTERFACE, "ItemNew")) {
            if (resolvers->count < MAX_RESOLVERS && read_browser_item(&m, &item) &&
                strcmp(item.type, SERVICE_TYPE) == 0) {
                omt_dbus_arg args[] = {
                    {.type = 'i', .v.i = item.interface},
                    {.type = 'i', .v.i = item.protocol},
                    {.type = 's', .v.s = item.name},
                    {.type = 's', .v.s = item.type},
                    {.type = 's', .v.s = item.domain},
                    {.type = 'i', .v.i = UNSPEC},
                    {.type = 'u', .v.u = 0},
                };
                omt_dbus_message resolved;
                if (omt_dbus_call(&bus, SERVICE, "/", SERVER_INTERFACE, "ServiceResolverNew", args,
                                  7, deadline_ms, &resolved, &err)) {
                    char *path = reply_path(&resolved);
                    if (path) resolvers->paths[resolvers->count++] = path;
                }
            }
        } else if (is_signal(&m, BROWSER_INTERFACE, "ItemRemove")) {
            if (read_browser_item(&m, &item)) {
                omt_sources_remove(out, item.name);
                if (removed->count < MAX_REMOVED &&
                    strlen(item.name) <= OMT_SOURCE_NAME_MAX_BYTES &&
                    !removed_contains(removed, item.name))
                    omt_strlcpy(removed->names[removed->count++], item.name,
                                sizeof(removed->names[0]));
            }
        } else if (is_signal(&m, RESOLVER_INTERFACE, "Found")) {
            omt_source source;
            if (resolved_source(&m, &source) && !removed_contains(removed, source.name) &&
                out->count < capacity)
                (void)omt_sources_put(out, source.name, &source.endpoint);
        }
        omt_dbus_message_free(&m);
    }
    /* Avahi keeps browser and resolver objects alive until freed. The frees
     * expect no reply, so teardown never waits on the daemon. */
    uint64_t teardown = omt_now_ms() + 500;
    for (size_t i = 0; resolvers && i < resolvers->count; i++) {
        omt_dbus_send_no_reply(&bus, SERVICE, resolvers->paths[i], RESOLVER_INTERFACE, "Free",
                               teardown, &err);
        free(resolvers->paths[i]);
    }
    omt_dbus_send_no_reply(&bus, SERVICE, browser, BROWSER_INTERFACE, "Free", teardown, &err);
    free(browser);
    free(resolvers);
    free(removed);
    omt_dbus_close(&bus);
}
