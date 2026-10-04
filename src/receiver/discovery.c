/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "receiver/discovery.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "common/fsio.h"
#include "common/ipaddr.h"
#include "common/json.h"
#include "common/proc.h"
#include "common/xml.h"

void omt_sources_clear(omt_source_set *s) { s->count = 0; }

bool omt_sources_put(omt_source_set *s, const char *name, const omt_endpoint *ep) {
    for (size_t i = 0; i < s->count; i++)
        if (strcmp(s->items[i].name, name) == 0) {
            s->items[i].endpoint = *ep;
            return true;
        }
    if (s->count >= OMT_MAX_SOURCES) return false;
    omt_strlcpy(s->items[s->count].name, name, sizeof(s->items[s->count].name));
    s->items[s->count].endpoint = *ep;
    s->count++;
    return true;
}

void omt_sources_remove(omt_source_set *s, const char *name) {
    for (size_t i = 0; i < s->count; i++)
        if (strcmp(s->items[i].name, name) == 0) {
            s->items[i] = s->items[s->count - 1];
            s->count--;
            return;
        }
}

static int by_name(const void *a, const void *b) {
    return strcmp(((const omt_source *)a)->name, ((const omt_source *)b)->name);
}

void omt_sources_sort(omt_source_set *s) { qsort(s->items, s->count, sizeof(omt_source), by_name); }

bool omt_configured_server(char out[OMT_TARGET_MAX_BYTES + 1]) {
    const char *storage = getenv("OMT_STORAGE_PATH");
    char path[4096];
    if (!omt_snprintf(path, sizeof(path), "%s/settings.xml",
                      storage && *storage ? storage : "/etc/omt/omt"))
        return false;
    omt_buf doc;
    omt_err err;
    if (omt_read_text(path, OMT_XML_MAX_DOCUMENT, &doc, &err) != OMT_READ_OK) return false;
    omt_span trimmed = omt_utf8_trim(omt_buf_cstr(&doc), doc.len);
    bool ok = omt_xml_root_is((const char *)trimmed.p, trimmed.len, "Settings") == OMT_XML_OK;
    omt_buf server;
    if (ok)
        ok = omt_xml_unique_text(omt_buf_cstr(&doc), doc.len, "DiscoveryServer", &server) ==
             OMT_XML_OK;
    if (ok) {
        omt_direct_target t;
        ok = omt_parse_direct_target(omt_buf_cstr(&server), server.len, &t, NULL) &&
             omt_strlcpy(out, omt_buf_cstr(&server), OMT_TARGET_MAX_BYTES + 1);
        omt_buf_free(&server);
    }
    omt_buf_free(&doc);
    return ok;
}

bool omt_discovery_transport_available(void) {
    char server[OMT_TARGET_MAX_BYTES + 1];
    return omt_configured_server(server) || omt_mdns_available();
}

bool omt_endpoint_from_parts(const char *address, uint16_t port, omt_endpoint *out) {
    if (port == 0) return false;
    size_t len = strlen(address);
    uint8_t v4[4], v6[16];
    bool is_v4 = omt_parse_ipv4(address, len, v4);
    bool is_v6 = !is_v4 && omt_parse_ipv6(address, len, v6);
    if ((is_v4 && omt_is_undiallable_ipv4(v4)) || (is_v6 && omt_is_undiallable_ipv6(v6)))
        return false;
    char candidate[OMT_TARGET_MAX_BYTES + 64];
    if (!omt_snprintf(candidate, sizeof(candidate), is_v6 ? "omt://[%s]:%u" : "omt://%s:%u",
                      address, (unsigned)port))
        return false;
    omt_direct_target t;
    if (!omt_parse_direct_target(candidate, strlen(candidate), &t, NULL)) return false;
    omt_strlcpy(out->host, t.host, sizeof(out->host));
    out->port = t.port;
    return true;
}

bool omt_announcement_read(const char *doc, size_t len, omt_announcement *out) {
    memset(out, 0, sizeof(*out));
    const char *tags[] = {"Name", "Removed", "IPAddress", "Port"};
    omt_buf text[4];
    bool found[4];
    if (omt_xml_unique_texts(doc, len, tags, 4, text, found) != OMT_XML_OK) return false;
    bool ok = found[0] && omt_is_valid_source_name(omt_buf_cstr(&text[0]), text[0].len);
    if (ok) {
        omt_strlcpy(out->name, omt_buf_cstr(&text[0]), sizeof(out->name));
        out->removed = omt_xml_element_is(&text[1], found[1], "True");
        uint64_t port;
        /* u16::from_str: digits with an optional leading '+'. */
        const char *ptext = omt_buf_cstr(&text[3]);
        size_t plen = text[3].len;
        if (plen > 0 && ptext[0] == '+') {
            ptext++;
            plen--;
        }
        if (found[2] && found[3] && omt_parse_u64(ptext, plen, 65535, &port))
            out->has_endpoint =
                omt_endpoint_from_parts(omt_buf_cstr(&text[2]), (uint16_t)port, &out->endpoint);
    }
    for (int i = 0; i < 4; i++) omt_buf_free(&text[i]);
    return ok;
}

/* Subscribes to a central discovery server's metadata stream and collects
 * every announcement that arrives before the deadline, last writer wins. */
static void server_sources(const char *server, uint64_t deadline_ms, omt_source_set *out) {
    omt_direct_target t;
    if (!omt_parse_direct_target(server, strlen(server), &t, NULL)) return;
    omt_endpoint ep;
    omt_strlcpy(ep.host, t.host, sizeof(ep.host));
    ep.port = t.port;
    omt_channel c;
    omt_channel_init(&c);
    omt_err err;
    if (!omt_channel_connect(&c, &ep, OMT_FRAME_METADATA, deadline_ms, &err)) {
        omt_channel_free(&c);
        return;
    }
    while (omt_now_ms() < deadline_ms && out->count < OMT_MAX_SOURCES) {
        if (omt_channel_receive(&c, deadline_ms, &err) != OMT_RECV_OK) break;
        if (c.frame.header.frame_type != OMT_FRAME_METADATA || c.frame.len == 0) continue;
        if (!omt_utf8_valid((const char *)c.frame.payload, c.frame.len)) continue;
        omt_announcement a;
        if (!omt_announcement_read((const char *)c.frame.payload, c.frame.len, &a)) continue;
        if (a.removed) {
            omt_sources_remove(out, a.name);
            continue;
        }
        if (a.has_endpoint) (void)omt_sources_put(out, a.name, &a.endpoint);
    }
    omt_channel_free(&c);
}

void omt_discover_sources(uint64_t wait_ms, omt_source_set *out) {
    omt_sources_clear(out);
    uint64_t deadline = omt_now_ms() + wait_ms;
    char server[OMT_TARGET_MAX_BYTES + 1];
    if (omt_configured_server(server))
        server_sources(server, deadline, out);
    else
        omt_mdns_browse(deadline, OMT_MAX_SOURCES, out);
    omt_sources_sort(out);
}

bool omt_discover_resolve(const char *target, uint64_t wait_ms, omt_endpoint *out) {
    size_t len = strlen(target);
    omt_direct_target t;
    if (omt_parse_direct_target(target, len, &t, NULL)) {
        omt_strlcpy(out->host, t.host, sizeof(out->host));
        out->port = t.port;
        return true;
    }
    if (omt_has_prefix(target, "omt://") || !omt_is_valid_source_name(target, len)) return false;
    omt_source_set *set = malloc(sizeof(*set));
    if (!set) return false;
    omt_discover_sources(wait_ms, set);
    bool found = false;
    for (size_t i = 0; i < set->count && !found; i++)
        if (strcmp(set->items[i].name, target) == 0) {
            *out = set->items[i].endpoint;
            found = true;
        }
    free(set);
    return found;
}
