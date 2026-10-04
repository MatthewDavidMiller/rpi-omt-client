/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "web/network.h"

#include <stdio.h>
#include <string.h>

#include "common/fsio.h"
#include "common/ipaddr.h"
#include "common/json.h"
#include "common/xml.h"

#define SETTINGS_LIMIT (64 * 1024)
#define DEFAULT_PORT 6399

static bool is_alnum(char c) {
    return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9');
}

static bool canonical_host(const char *host, size_t len, char *out, size_t size) {
    if (len == 0) return false;
    for (size_t i = 0; i < len; i++)
        if ((unsigned char)host[i] >= 0x80) return false;
    uint8_t v4[4], v6[16];
    if (omt_parse_ipv4(host, len, v4))
        return omt_snprintf(out, size, "%u.%u.%u.%u", v4[0], v4[1], v4[2], v4[3]);
    if (omt_parse_ipv6(host, len, v6)) {
        char text[46];
        omt_format_ipv6(v6, text);
        return omt_snprintf(out, size, "[%s]", text);
    }
    if (len > 253 || host[0] == '.' || host[len - 1] == '.') return false;
    size_t start = 0;
    for (size_t i = 0; i <= len; i++) {
        if (i < len && host[i] != '.') {
            if (!is_alnum(host[i]) && host[i] != '-') return false;
            continue;
        }
        size_t n = i - start;
        if (n == 0 || n > 63 || host[start] == '-' || host[i - 1] == '-') return false;
        start = i + 1;
    }
    if (len >= size) return false;
    for (size_t i = 0; i < len; i++)
        out[i] = (host[i] >= 'A' && host[i] <= 'Z') ? (char)(host[i] + 32) : host[i];
    out[len] = 0;
    return true;
}

static bool parse_port(const char *s, size_t len, uint64_t *out) {
    if (len && s[0] == '+') {
        s++;
        len--;
    }
    return omt_parse_u64(s, len, 65535, out);
}

bool omt_normalize_server(const char *value, char out[600], omt_err *err) {
    omt_span t = omt_utf8_trim(value, strlen(value));
    const char *v = (const char *)t.p;
    size_t len = t.len;
    if (len == 0) {
        out[0] = 0;
        return true;
    }
    bool bad = len > 512;
    for (size_t i = 0; i < len && !bad; i++) {
        unsigned char c = (unsigned char)v[i];
        bad = c >= 0x80 || c < 32 || c == 127;
    }
    if (bad) {
        omt_err_set(err, "Discovery Server contains unsupported characters.");
        return false;
    }
    const char *auth = v;
    size_t alen = len;
    if (alen >= 6 && memcmp(auth, "omt://", 6) == 0) {
        auth += 6;
        alen -= 6;
    }
    for (size_t i = 0; i < alen; i++)
        if (auth[i] == '/' || auth[i] == '?' || auth[i] == '#' || auth[i] == '@') {
            omt_err_set(err, "Discovery Server must be a host or omt://host:port.");
            return false;
        }
    const char *host;
    size_t host_len;
    uint64_t port = DEFAULT_PORT;
    size_t colons = 0;
    for (size_t i = 0; i < alen; i++) colons += auth[i] == ':';
    if (alen && auth[0] == '[') {
        const char *close = memchr(auth + 1, ']', alen - 1);
        if (!close) {
            omt_err_set(err, "Discovery Server is invalid.");
            return false;
        }
        host = auth + 1;
        host_len = (size_t)(close - host);
        const char *suffix = close + 1;
        size_t slen = alen - (size_t)(suffix - auth);
        if (slen) {
            if (suffix[0] != ':' || !parse_port(suffix + 1, slen - 1, &port)) {
                omt_err_set(err, "Discovery Server is invalid.");
                return false;
            }
        }
    } else if (colons == 1) {
        const char *colon = memchr(auth, ':', alen);
        host = auth;
        host_len = (size_t)(colon - auth);
        if (!parse_port(colon + 1, alen - host_len - 1, &port)) {
            omt_err_set(err, "Discovery Server is invalid.");
            return false;
        }
    } else if (colons == 0) {
        host = auth;
        host_len = alen;
    } else {
        omt_err_set(err, "Discovery Server IPv6 addresses must be bracketed.");
        return false;
    }
    if (port == 0) {
        omt_err_set(err, "Discovery Server is invalid.");
        return false;
    }
    char canonical[300];
    if (!canonical_host(host, host_len, canonical, sizeof(canonical))) {
        omt_err_set(err, "Discovery Server host is invalid.");
        return false;
    }
    return omt_snprintf(out, 600, "omt://%s:%llu", canonical, (unsigned long long)port);
}

/* Walks the document with the strict rules: one <Settings> root, no doctype,
 * no entity references at all, at most one direct DiscoveryServer child. */
typedef struct {
    size_t content_start;
    size_t content_end;
    bool has_element; /* a <DiscoveryServer>...</DiscoveryServer> child */
    bool has_empty;   /* a <DiscoveryServer/> child */
    size_t empty_start;
    size_t empty_end;
    bool root_empty; /* <Settings/> */
    size_t root_start;
    size_t root_end;
    size_t settings_end_start; /* offset of </Settings> */
} settings_shape;

static bool walk(const uint8_t *xml, size_t len, omt_buf *raw, settings_shape *shape,
                 omt_err *err) {
    memset(shape, 0, sizeof(*shape));
    if (!omt_utf8_valid((const char *)xml, len)) {
        omt_err_set(err, "OMT settings XML is invalid: invalid UTF-8");
        return false;
    }
    omt_xml_reader r;
    omt_xml_reader_init(&r, (const char *)xml, len);
    size_t depth = 0, discovery_count = 0;
    bool root_seen = false, in_discovery = false;
    for (;;) {
        size_t start = r.pos;
        omt_xml_event ev;
        if (omt_xml_next(&r, &ev) != OMT_XML_OK) {
            omt_err_set(err, "OMT settings XML is invalid: ill-formed document");
            return false;
        }
        switch (ev.type) {
        case OMT_XML_EV_DOCTYPE:
        case OMT_XML_EV_REF:
            omt_err_set(err, "OMT settings XML must not declare a doctype or entities.");
            return false;
        case OMT_XML_EV_START:
            if (depth == 0 && root_seen) {
                omt_err_set(err, "OMT settings XML must contain exactly one root element.");
                return false;
            }
            depth++;
            if (depth == 1) {
                if (!omt_span_eq(ev.name, "Settings")) {
                    omt_err_set(err, "OMT settings root must be <Settings>.");
                    return false;
                }
                root_seen = true;
            } else if (depth == 2 && omt_span_eq(ev.name, "DiscoveryServer")) {
                discovery_count++;
                in_discovery = true;
                shape->has_element = true;
                shape->content_start = r.pos;
            }
            break;
        case OMT_XML_EV_EMPTY:
            if (depth == 0) {
                if (root_seen) {
                    omt_err_set(err, "OMT settings XML must contain exactly one root element.");
                    return false;
                }
                if (!omt_span_eq(ev.name, "Settings")) {
                    omt_err_set(err, "OMT settings root must be <Settings>.");
                    return false;
                }
                root_seen = true;
                shape->root_empty = true;
                shape->root_start = start;
                shape->root_end = r.pos;
            } else if (depth == 1 && omt_span_eq(ev.name, "DiscoveryServer")) {
                discovery_count++;
                shape->has_empty = true;
                shape->empty_start = start;
                shape->empty_end = r.pos;
            }
            break;
        case OMT_XML_EV_TEXT:
            if (in_discovery && depth == 2) omt_buf_append(raw, ev.content.p, ev.content.len);
            break;
        case OMT_XML_EV_END:
            if (depth == 2 && omt_span_eq(ev.name, "DiscoveryServer") && in_discovery) {
                in_discovery = false;
                shape->content_end = start;
            }
            if (depth == 1) shape->settings_end_start = start;
            if (depth) depth--;
            break;
        case OMT_XML_EV_EOF:
            if (!root_seen || depth != 0) {
                omt_err_set(err, "OMT settings XML is invalid.");
                return false;
            }
            if (discovery_count > 1) {
                omt_err_set(err, "OMT settings contain duplicate DiscoveryServer entries.");
                return false;
            }
            return true;
        default: break;
        }
    }
}

bool omt_parse_server(const uint8_t *xml, size_t len, char out[600], omt_err *err) {
    omt_buf raw;
    omt_buf_init(&raw, SETTINGS_LIMIT);
    settings_shape shape;
    bool ok =
        walk(xml, len, &raw, &shape, err) && omt_normalize_server(omt_buf_cstr(&raw), out, err);
    omt_buf_free(&raw);
    return ok;
}

void omt_read_network_configuration(const char *path, omt_network_configuration *out) {
    memset(out, 0, sizeof(*out));
    omt_buf xml;
    omt_err err;
    omt_read_result r = omt_read_bounded(path, SETTINGS_LIMIT, &xml, &err);
    if (r == OMT_READ_MISSING) return;
    if (r == OMT_READ_OK && omt_parse_server(xml.data, xml.len, out->discovery_server, &err)) {
        omt_buf_free(&xml);
        return;
    }
    out->discovery_server[0] = 0;
    omt_strlcpy(out->error, err.msg, sizeof(out->error));
    if (r == OMT_READ_OK) omt_buf_free(&xml);
}

bool omt_update_settings_xml(const uint8_t *xml, size_t len, const char *normalized, omt_buf *out,
                             bool *changed, omt_err *err) {
    char current[600];
    omt_err ignored;
    if (omt_parse_server(xml, len, current, &ignored) && strcmp(current, normalized) == 0) {
        *changed = false;
        return true;
    }
    /* A strict parse before any transform; then only the value is spliced. */
    if (!omt_parse_server(xml, len, current, err)) return false;
    omt_buf raw;
    omt_buf_init(&raw, SETTINGS_LIMIT);
    settings_shape shape;
    bool ok = walk(xml, len, &raw, &shape, err);
    omt_buf_free(&raw);
    if (!ok) return false;
    char element[700];
    snprintf(element, sizeof(element), "<DiscoveryServer>%s</DiscoveryServer>", normalized);
    if (shape.has_element) {
        omt_buf_append(out, xml, shape.content_start);
        omt_buf_puts(out, normalized);
        omt_buf_append(out, xml + shape.content_end, len - shape.content_end);
    } else if (shape.has_empty) {
        omt_buf_append(out, xml, shape.empty_start);
        omt_buf_puts(out, element);
        omt_buf_append(out, xml + shape.empty_end, len - shape.empty_end);
    } else if (shape.root_empty) {
        omt_buf_append(out, xml, shape.root_start);
        omt_buf_printf(out, "<Settings>\n  %s\n</Settings>", element);
        omt_buf_append(out, xml + shape.root_end, len - shape.root_end);
    } else {
        /* Indent like the line the closing tag starts, when it starts one. */
        size_t at = shape.settings_end_start;
        omt_buf_append(out, xml, at);
        bool line_start = at == 0 || xml[at - 1] == '\n';
        if (line_start) omt_buf_puts(out, "  ");
        omt_buf_puts(out, element);
        omt_buf_puts(out, line_start ? "\n" : "");
        omt_buf_append(out, xml + at, len - at);
    }
    if (out->len == 0 || out->data[out->len - 1] != '\n') omt_buf_putc(out, '\n');
    if (out->failed) {
        omt_err_set(err, "updated OMT settings exceed their size limit");
        return false;
    }
    *changed = true;
    return true;
}

int omt_save_network_configuration(const char *path, const char *value, omt_err *err) {
    char normalized[600];
    if (!omt_normalize_server(value, normalized, err)) return -1;
    omt_buf current;
    omt_read_result r = omt_read_bounded(path, SETTINGS_LIMIT, &current, err);
    if (r == OMT_READ_ERROR) return -1;
    if (r == OMT_READ_MISSING) {
        omt_buf_init(&current, SETTINGS_LIMIT);
        omt_buf_puts(&current, "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<Settings />\n");
    }
    omt_buf updated;
    omt_buf_init(&updated, SETTINGS_LIMIT);
    bool changed = false;
    bool ok =
        omt_update_settings_xml(current.data, current.len, normalized, &updated, &changed, err);
    if (ok && changed)
        ok = omt_atomic_replace(path, updated.data, updated.len, SETTINGS_LIMIT, err);
    omt_buf_free(&current);
    omt_buf_free(&updated);
    if (!ok) return -1;
    return changed ? 1 : 0;
}
