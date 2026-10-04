/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "web/state.h"

#include <stdio.h>
#include <string.h>

#include "common/fsio.h"
#include "common/json.h"

#define SOURCE_LIMIT 1024
#define CEILING_LIMIT 256
#define DELAY_LIMIT 64
#define DELAY_SCHEMA 2

/* Reads a strict JSON record; on any failure `err` carries the serde-style
 * reason with the caller's prefix. */
static int read_record(const char *path, size_t limit, const char *what, omt_json_doc *doc,
                       omt_json **root, omt_buf *raw, omt_err *err) {
    omt_read_result r = omt_read_bounded(path, limit, raw, err);
    if (r != OMT_READ_OK) return r == OMT_READ_MISSING ? 0 : -1;
    *root = omt_json_parse(doc, omt_buf_cstr(raw), raw->len, 0);
    if (!*root) {
        omt_err_set(err, "%s is invalid JSON: %s", what, doc->error);
        omt_buf_free(raw);
        return -1;
    }
    return 1;
}

int omt_read_source(const char *path, omt_source_target *out, omt_err *err) {
    omt_json_doc doc;
    omt_json *root = NULL;
    omt_buf raw;
    int r = read_record(path, SOURCE_LIMIT, "saved OMT target", &doc, &root, &raw, err);
    if (r <= 0) return r;
    const char *fields[] = {"schema", "kind", "name", "uri"};
    const omt_json *kind = omt_json_get(root, "kind");
    const omt_json *name = omt_json_get(root, "name");
    const omt_json *uri = omt_json_get(root, "uri");
    uint64_t schema;
    int result = -1;
    /* serde: every field typed, unknown fields refused, Option accepts null. */
    bool shaped = root->type == OMT_JSON_OBJECT && omt_json_only_keys(root, fields, 4) &&
                  omt_json_as_u64(omt_json_get(root, "schema"), &schema) && schema <= 255 &&
                  omt_json_as_str(kind) &&
                  (!name || name->type == OMT_JSON_NULL || omt_json_as_str(name)) &&
                  (!uri || uri->type == OMT_JSON_NULL || omt_json_as_str(uri));
    if (!shaped) {
        omt_err_set(err, "saved OMT target is invalid JSON: unexpected record shape");
    } else if (schema != 1) {
        omt_err_set(err, "saved OMT target has an invalid schema");
    } else {
        bool has_name = name && name->type == OMT_JSON_STRING;
        bool has_uri = uri && uri->type == OMT_JSON_STRING;
        const char *k = omt_json_as_str(kind);
        if (!strcmp(k, "discovered") && has_name && !has_uri &&
            omt_is_valid_source_name(name->string, name->string_len)) {
            out->direct = false;
            omt_strlcpy(out->value, name->string, sizeof(out->value));
            result = 1;
        } else if (!strcmp(k, "direct") && !has_name && has_uri) {
            omt_direct_target t;
            if (omt_parse_direct_target(uri->string, uri->string_len, &t, NULL)) {
                out->direct = true;
                omt_strlcpy(out->value, uri->string, sizeof(out->value));
                result = 1;
            }
        }
        if (result < 0) omt_err_set(err, "saved OMT target kind or value is invalid");
    }
    omt_json_doc_free(&doc);
    omt_buf_free(&raw);
    return result;
}

bool omt_save_source(const char *path, const omt_source_target *target, omt_err *err) {
    if (!target) return omt_remove_file_durable(path, err);
    size_t len = strlen(target->value);
    omt_direct_target t;
    bool valid = target->direct ? omt_parse_direct_target(target->value, len, &t, NULL)
                                : omt_is_valid_source_name(target->value, len);
    if (!valid) {
        omt_err_set(err, "invalid OMT target kind or value");
        return false;
    }
    omt_buf out;
    omt_buf_init(&out, SOURCE_LIMIT);
    omt_buf_printf(&out, "{\"schema\":1,\"kind\":\"%s\",\"%s\":",
                   target->direct ? "direct" : "discovered", target->direct ? "uri" : "name");
    omt_json_write_cstr(&out, target->value);
    omt_buf_puts(&out, "}\n");
    bool ok = !out.failed && omt_atomic_replace(path, out.data, out.len, SOURCE_LIMIT, err);
    if (out.failed) omt_err_set(err, "replacement exceeds its size limit");
    omt_buf_free(&out);
    return ok;
}

static bool all_digits(const char *s, size_t len) {
    for (size_t i = 0; i < len; i++)
        if (s[i] < '0' || s[i] > '9') return false;
    return true;
}

bool omt_parse_video_ceiling(const char *value, omt_err *err) {
    size_t shapes = 1;
    for (const char *p = value; *p; p++) shapes += *p == ',';
    if (shapes > 4) {
        omt_err_set(err, "A video limit must list between 1 and 4 resolutions.");
        return false;
    }
    const char *shape = value;
    for (;;) {
        const char *comma = strchr(shape, ',');
        size_t len = comma ? (size_t)(comma - shape) : strlen(shape);
        const char *at = memchr(shape, '@', len);
        const char *x = at ? memchr(shape, 'x', (size_t)(at - shape)) : NULL;
        if (!at || !x) {
            omt_err_set(err, "Invalid video limit: %.*s. Expected WIDTHxHEIGHT@FPS.", (int)len,
                        shape);
            return false;
        }
        size_t wl = (size_t)(x - shape), hl = (size_t)(at - x - 1),
               fl = len - (size_t)(at - shape) - 1;
        if (wl < 2 || wl > 4 || hl < 2 || hl > 4 || fl < 1 || fl > 3 || !all_digits(shape, wl) ||
            !all_digits(x + 1, hl) || !all_digits(at + 1, fl)) {
            omt_err_set(err, "Invalid video limit: %.*s. Expected WIDTHxHEIGHT@FPS.", (int)len,
                        shape);
            return false;
        }
        uint64_t w, h, f;
        (void)omt_parse_u64(shape, wl, UINT64_MAX, &w);
        (void)omt_parse_u64(x + 1, hl, UINT64_MAX, &h);
        (void)omt_parse_u64(at + 1, fl, UINT64_MAX, &f);
        /* fps parses as u8, so a three-digit rate past 255 is a parse error. */
        if (f > 255) {
            omt_err_set(err, "Invalid frame rate");
            return false;
        }
        if (w < 16 || w > 1920) {
            omt_err_set(err, "Width %llu is outside 16-1920.", (unsigned long long)w);
            return false;
        }
        if (h < 16 || h > 1080) {
            omt_err_set(err, "Height %llu is outside 16-1080.", (unsigned long long)h);
            return false;
        }
        if (f < 1 || f > 60) {
            omt_err_set(err, "Frame rate %llu is outside 1-60.", (unsigned long long)f);
            return false;
        }
        if (!comma) return true;
        shape = comma + 1;
    }
}

void omt_describe_video_ceiling(const char *value, omt_buf *out) {
    const char *shape = value;
    bool first = true;
    for (;;) {
        const char *comma = strchr(shape, ',');
        size_t len = comma ? (size_t)(comma - shape) : strlen(shape);
        if (!first) omt_buf_puts(out, ", or ");
        first = false;
        const char *at = memchr(shape, '@', len);
        if (!at) {
            omt_buf_append(out, shape, len);
        } else {
            size_t fl = len - (size_t)(at - shape) - 1;
            uint64_t f;
            omt_buf_append(out, shape, (size_t)(at - shape));
            omt_buf_puts(out, " at ");
            /* u8::from_str normalizes "060" to 60; anything else is kept. */
            const char *ft = at + 1;
            size_t fll = fl;
            if (fll && ft[0] == '+') {
                ft++;
                fll--;
            }
            if (omt_parse_u64(ft, fll, 255, &f))
                omt_buf_printf(out, "%llu", (unsigned long long)f);
            else
                omt_buf_append(out, at + 1, fl);
            omt_buf_puts(out, " fps");
        }
        if (!comma) return;
        shape = comma + 1;
    }
}

int omt_read_video_ceiling(const char *path, omt_buf *out, omt_err *err) {
    omt_json_doc doc;
    omt_json *root = NULL;
    omt_buf raw;
    int r = read_record(path, CEILING_LIMIT, "saved video limit", &doc, &root, &raw, err);
    if (r <= 0) return r;
    const char *fields[] = {"schema", "ceiling"};
    uint64_t schema;
    const char *ceiling = omt_json_as_str(omt_json_get(root, "ceiling"));
    int result = -1;
    if (!(root->type == OMT_JSON_OBJECT && omt_json_only_keys(root, fields, 2) && ceiling &&
          omt_json_as_u64(omt_json_get(root, "schema"), &schema) && schema <= 255))
        omt_err_set(err, "saved video limit is invalid JSON: unexpected record shape");
    else if (schema != 1)
        omt_err_set(err, "saved video limit has an invalid schema");
    else if (omt_parse_video_ceiling(ceiling, err)) {
        omt_buf_puts(out, ceiling);
        result = 1;
    }
    omt_json_doc_free(&doc);
    omt_buf_free(&raw);
    return result;
}

bool omt_effective_video_ceiling(const char *path, const char *board_default, omt_buf *out,
                                 omt_err *err) {
    if (!omt_parse_video_ceiling(board_default, err)) return false;
    int r = omt_read_video_ceiling(path, out, err);
    if (r < 0) return false;
    if (r == 0) omt_buf_puts(out, board_default);
    return true;
}

bool omt_save_video_ceiling(const char *path, const char *ceiling, omt_err *err) {
    if (!ceiling) return omt_remove_file_durable(path, err);
    if (!omt_parse_video_ceiling(ceiling, err)) return false;
    omt_buf out;
    omt_buf_init(&out, CEILING_LIMIT);
    omt_buf_puts(&out, "{\"schema\":1,\"ceiling\":");
    omt_json_write_cstr(&out, ceiling);
    omt_buf_puts(&out, "}\n");
    bool ok = !out.failed && omt_atomic_replace(path, out.data, out.len, CEILING_LIMIT, err);
    if (out.failed) omt_err_set(err, "replacement exceeds its size limit");
    omt_buf_free(&out);
    return ok;
}

uint64_t omt_pixel_rate(const char *value) {
    uint64_t best = 0;
    const char *shape = value;
    for (;;) {
        const char *comma = strchr(shape, ',');
        size_t len = comma ? (size_t)(comma - shape) : strlen(shape);
        const char *at = memchr(shape, '@', len);
        const char *x = at ? memchr(shape, 'x', (size_t)(at - shape)) : NULL;
        uint64_t w, h, f, rate;
        if (x && omt_parse_u64(shape, (size_t)(x - shape), UINT64_MAX, &w) &&
            omt_parse_u64(x + 1, (size_t)(at - x - 1), UINT64_MAX, &h) &&
            omt_parse_u64(at + 1, len - (size_t)(at - shape) - 1, UINT64_MAX, &f) &&
            omt_mul(w, h, &rate) && omt_mul(rate, f, &rate) && rate > best)
            best = rate;
        if (!comma) return best;
        shape = comma + 1;
    }
}

bool omt_parse_playout_delay(const char *value, uint64_t *out, omt_err *err) {
    omt_span t = omt_utf8_trim(value, strlen(value));
    if (t.len == 0 || omt_ascii_ieq((const char *)t.p, t.len, "auto")) {
        *out = OMT_DEFAULT_PLAYOUT_DELAY_MS;
        return true;
    }
    uint64_t v;
    if (!all_digits((const char *)t.p, t.len) ||
        !omt_parse_u64((const char *)t.p, t.len, UINT64_MAX, &v)) {
        omt_err_set(err,
                    "Invalid playout delay: %.*s. Expected a whole number of milliseconds from 0 "
                    "to %u.",
                    (int)t.len, (const char *)t.p, OMT_MAX_PLAYOUT_DELAY_MS);
        return false;
    }
    if (v > OMT_MAX_PLAYOUT_DELAY_MS) {
        omt_err_set(err, "Playout delay %llu ms is outside 0-%u.", (unsigned long long)v,
                    OMT_MAX_PLAYOUT_DELAY_MS);
        return false;
    }
    *out = v;
    return true;
}

int omt_read_playout_delay(const char *path, uint64_t *out, omt_err *err) {
    omt_json_doc doc;
    omt_json *root = NULL;
    omt_buf raw;
    int r = read_record(path, DELAY_LIMIT, "saved playout delay", &doc, &root, &raw, err);
    if (r <= 0) return r;
    int result = -1;
    uint64_t schema, ms;
    /* The schema is read alone first, so a schema-1 (seconds) file from
     * before milliseconds is recognised without its other field failing the
     * strict shape. It is treated as absent: the default moved to 0 at the
     * same time, and refusing it would block playback after an upgrade. */
    if (root->type != OMT_JSON_OBJECT || !omt_json_as_u64(omt_json_get(root, "schema"), &schema) ||
        schema > 255) {
        omt_err_set(err, "saved playout delay is invalid JSON: unexpected record shape");
    } else if (schema == 1) {
        result = 0;
    } else if (schema != DELAY_SCHEMA) {
        omt_err_set(err, "saved playout delay has an invalid schema");
    } else {
        const char *fields[] = {"schema", "milliseconds"};
        if (!omt_json_only_keys(root, fields, 2) ||
            !omt_json_as_u64(omt_json_get(root, "milliseconds"), &ms)) {
            omt_err_set(err, "saved playout delay is invalid JSON: unexpected record shape");
        } else if (ms > OMT_MAX_PLAYOUT_DELAY_MS) {
            omt_err_set(err, "Playout delay %llu ms is outside 0-%u.", (unsigned long long)ms,
                        OMT_MAX_PLAYOUT_DELAY_MS);
        } else {
            *out = ms;
            result = 1;
        }
    }
    omt_json_doc_free(&doc);
    omt_buf_free(&raw);
    return result;
}

bool omt_effective_playout_delay(const char *path, const char *def, uint64_t *out, omt_err *err) {
    uint64_t fallback;
    if (!omt_parse_playout_delay(def, &fallback, err)) return false;
    int r = omt_read_playout_delay(path, out, err);
    if (r < 0) return false;
    if (r == 0) *out = fallback;
    return true;
}

bool omt_save_playout_delay(const char *path, const uint64_t *milliseconds, omt_err *err) {
    if (!milliseconds) return omt_remove_file_durable(path, err);
    if (*milliseconds > OMT_MAX_PLAYOUT_DELAY_MS) {
        omt_err_set(err, "Playout delay %llu ms is outside 0-%u.",
                    (unsigned long long)*milliseconds, OMT_MAX_PLAYOUT_DELAY_MS);
        return false;
    }
    if (*milliseconds == OMT_DEFAULT_PLAYOUT_DELAY_MS) return omt_remove_file_durable(path, err);
    char text[64];
    snprintf(text, sizeof(text), "{\"schema\":%d,\"milliseconds\":%llu}\n", DELAY_SCHEMA,
             (unsigned long long)*milliseconds);
    return omt_atomic_replace(path, text, strlen(text), DELAY_LIMIT, err);
}
