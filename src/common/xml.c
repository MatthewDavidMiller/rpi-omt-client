/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "common/xml.h"

#include <string.h>

void omt_xml_reader_init(omt_xml_reader *r, const char *doc, size_t len) {
    memset(r, 0, sizeof(*r));
    r->doc = doc;
    r->len = len;
}

static bool starts_with(const omt_xml_reader *r, const char *s) {
    size_t n = strlen(s);
    return r->len - r->pos >= n && memcmp(r->doc + r->pos, s, n) == 0;
}

static bool starts_with_ci(const omt_xml_reader *r, const char *s) {
    size_t n = strlen(s);
    return r->len - r->pos >= n && omt_ascii_ieq(r->doc + r->pos, n, s);
}

/* Finds `terminator` at or after `from`; returns its offset or len. */
static size_t find(const omt_xml_reader *r, size_t from, const char *terminator) {
    size_t n = strlen(terminator);
    if (r->len < n) return r->len;
    for (size_t i = from; i + n <= r->len; i++)
        if (memcmp(r->doc + i, terminator, n) == 0) return i;
    return r->len;
}

static bool is_space(char c) { return c == ' ' || c == '\t' || c == '\n' || c == '\r'; }

static omt_span span_at(const omt_xml_reader *r, size_t from, size_t to) {
    return omt_span_of(r->doc + from, to - from);
}

static bool span_equal(omt_span a, omt_span b) {
    return a.len == b.len && (a.len == 0 || memcmp(a.p, b.p, a.len) == 0);
}

omt_xml_status omt_xml_next(omt_xml_reader *r, omt_xml_event *ev) {
    memset(ev, 0, sizeof(*ev));
    if (r->pos >= r->len) {
        ev->type = OMT_XML_EV_EOF;
        return OMT_XML_OK;
    }
    char c = r->doc[r->pos];
    if (c == '&') {
        size_t start = r->pos + 1;
        size_t i = start;
        while (i < r->len && r->doc[i] != ';' && r->doc[i] != '&' && r->doc[i] != '<' &&
               !is_space(r->doc[i]))
            i++;
        if (i >= r->len || r->doc[i] != ';' || i == start) return OMT_XML_MALFORMED;
        ev->type = OMT_XML_EV_REF;
        ev->name = span_at(r, start, i);
        r->pos = i + 1;
        return OMT_XML_OK;
    }
    if (c != '<') {
        size_t start = r->pos;
        while (r->pos < r->len && r->doc[r->pos] != '<' && r->doc[r->pos] != '&') r->pos++;
        ev->type = OMT_XML_EV_TEXT;
        ev->content = span_at(r, start, r->pos);
        return OMT_XML_OK;
    }
    if (starts_with(r, "<?")) {
        size_t body = r->pos + 2;
        size_t end = find(r, body, "?>");
        if (end >= r->len) return OMT_XML_MALFORMED;
        bool decl = end - body >= 3 && memcmp(r->doc + body, "xml", 3) == 0 &&
                    (end - body == 3 || is_space(r->doc[body + 3]));
        ev->type = decl ? OMT_XML_EV_DECL : OMT_XML_EV_PI;
        ev->content = span_at(r, body, end);
        r->pos = end + 2;
        return OMT_XML_OK;
    }
    if (starts_with(r, "<!--")) {
        size_t body = r->pos + 4;
        size_t end = find(r, body, "-->");
        if (end >= r->len) return OMT_XML_MALFORMED;
        ev->type = OMT_XML_EV_COMMENT;
        ev->content = span_at(r, body, end);
        r->pos = end + 3;
        return OMT_XML_OK;
    }
    if (starts_with(r, "<![CDATA[")) {
        size_t body = r->pos + 9;
        size_t end = find(r, body, "]]>");
        if (end >= r->len) return OMT_XML_MALFORMED;
        ev->type = OMT_XML_EV_CDATA;
        ev->content = span_at(r, body, end);
        r->pos = end + 3;
        return OMT_XML_OK;
    }
    if (starts_with_ci(r, "<!DOCTYPE")) {
        /* Skip to the matching '>', honouring an internal subset in [...]. */
        size_t i = r->pos + 9;
        int brackets = 0;
        char quote = 0;
        for (; i < r->len; i++) {
            char d = r->doc[i];
            if (quote) {
                if (d == quote) quote = 0;
            } else if (d == '"' || d == '\'') {
                quote = d;
            } else if (d == '[') {
                brackets++;
            } else if (d == ']') {
                brackets--;
            } else if (d == '>' && brackets <= 0) {
                break;
            }
        }
        if (i >= r->len) return OMT_XML_MALFORMED;
        ev->type = OMT_XML_EV_DOCTYPE;
        ev->content = span_at(r, r->pos + 9, i);
        r->pos = i + 1;
        return OMT_XML_OK;
    }
    if (starts_with(r, "<!")) return OMT_XML_MALFORMED;

    bool closing = starts_with(r, "</");
    size_t name_start = r->pos + (closing ? 2 : 1);
    size_t i = name_start;
    while (i < r->len && !is_space(r->doc[i]) && r->doc[i] != '>' && r->doc[i] != '/') i++;
    size_t name_end = i;
    /* Find the end of the tag, skipping '>' inside quoted attribute values. */
    char quote = 0;
    for (; i < r->len; i++) {
        char d = r->doc[i];
        if (quote) {
            if (d == quote) quote = 0;
        } else if (d == '"' || d == '\'') {
            quote = d;
        } else if (d == '>') {
            break;
        }
    }
    if (i >= r->len) return OMT_XML_MALFORMED;
    omt_span name = span_at(r, name_start, name_end);
    if (name.len == 0) return OMT_XML_MALFORMED;
    if (closing) {
        for (size_t k = name_end; k < i; k++)
            if (!is_space(r->doc[k])) return OMT_XML_MALFORMED;
        if (r->depth == 0 || !span_equal(r->open[r->depth - 1], name)) return OMT_XML_MALFORMED;
        r->depth--;
        ev->type = OMT_XML_EV_END;
        ev->name = name;
        r->pos = i + 1;
        return OMT_XML_OK;
    }
    bool empty = i > name_end && r->doc[i - 1] == '/';
    ev->name = name;
    ev->content = span_at(r, name_end, empty ? i - 1 : i);
    r->pos = i + 1;
    if (empty) {
        ev->type = OMT_XML_EV_EMPTY;
        return OMT_XML_OK;
    }
    if (r->depth >= OMT_XML_STACK) return OMT_XML_UNSUPPORTED;
    r->open[r->depth++] = name;
    ev->type = OMT_XML_EV_START;
    return OMT_XML_OK;
}

int omt_xml_predefined_entity(omt_span name) {
    if (omt_span_eq(name, "amp")) return '&';
    if (omt_span_eq(name, "lt")) return '<';
    if (omt_span_eq(name, "gt")) return '>';
    if (omt_span_eq(name, "quot")) return '"';
    if (omt_span_eq(name, "apos")) return '\'';
    return -1;
}

omt_xml_status omt_xml_unique_texts(const char *doc, size_t len, const char *const *tags,
                                    size_t count, omt_buf *out, bool *found) {
    for (size_t i = 0; i < count; i++) {
        found[i] = false;
        omt_buf_init(&out[i], OMT_XML_MAX_DOCUMENT);
    }
    if (len > OMT_XML_MAX_DOCUMENT) return OMT_XML_UNSUPPORTED;
    omt_xml_reader r;
    omt_xml_reader_init(&r, doc, len);
    size_t depth = 0;
    size_t capturing = SIZE_MAX;
    omt_xml_status status = OMT_XML_OK;
    for (;;) {
        omt_xml_event ev;
        status = omt_xml_next(&r, &ev);
        if (status != OMT_XML_OK) break;
        if (ev.type == OMT_XML_EV_EOF) break;
        if (ev.type == OMT_XML_EV_DOCTYPE || ev.type == OMT_XML_EV_PI) {
            status = OMT_XML_UNSUPPORTED;
            break;
        }
        if (ev.type == OMT_XML_EV_START) {
            if (++depth > OMT_XML_MAX_DEPTH) {
                status = OMT_XML_UNSUPPORTED;
                break;
            }
            for (size_t i = 0; i < count; i++) {
                if (!omt_span_eq(ev.name, tags[i])) continue;
                if (found[i] || capturing != SIZE_MAX) {
                    status = OMT_XML_DUPLICATE;
                    goto done;
                }
                capturing = i;
                omt_buf_clear(&out[i]);
                break;
            }
        } else if (ev.type == OMT_XML_EV_TEXT && capturing != SIZE_MAX) {
            omt_buf_append(&out[capturing], ev.content.p, ev.content.len);
            if (out[capturing].failed) {
                status = OMT_XML_UNSUPPORTED;
                break;
            }
        } else if (ev.type == OMT_XML_EV_REF && capturing != SIZE_MAX) {
            int ch = omt_xml_predefined_entity(ev.name);
            if (ch < 0) {
                status = OMT_XML_MALFORMED;
                break;
            }
            omt_buf_putc(&out[capturing], (uint8_t)ch);
            if (out[capturing].failed) {
                status = OMT_XML_UNSUPPORTED;
                break;
            }
        } else if (ev.type == OMT_XML_EV_END) {
            if (depth) depth--;
            if (capturing != SIZE_MAX && omt_span_eq(ev.name, tags[capturing])) {
                found[capturing] = true;
                capturing = SIZE_MAX;
            }
        }
    }
done:
    if (status != OMT_XML_OK) {
        for (size_t i = 0; i < count; i++) {
            found[i] = false;
            omt_buf_free(&out[i]);
        }
        return status;
    }
    /* An element still open at the end of input was never completed. */
    if (capturing != SIZE_MAX) omt_buf_free(&out[capturing]);
    return OMT_XML_OK;
}

omt_xml_status omt_xml_unique_text(const char *doc, size_t len, const char *tag, omt_buf *out) {
    bool found = false;
    const char *tags[1] = {tag};
    omt_xml_status status = omt_xml_unique_texts(doc, len, tags, 1, out, &found);
    if (status != OMT_XML_OK) return status;
    if (!found) {
        omt_buf_free(out);
        return OMT_XML_NOT_FOUND;
    }
    return OMT_XML_OK;
}

bool omt_xml_element_is(const omt_buf *text, bool found, const char *value) {
    return found && omt_ascii_ieq(omt_buf_cstr(text), text->len, value);
}

omt_xml_status omt_xml_root_is(const char *doc, size_t len, const char *tag) {
    omt_xml_reader r;
    omt_xml_reader_init(&r, doc, len);
    for (;;) {
        omt_xml_event ev;
        omt_xml_status status = omt_xml_next(&r, &ev);
        if (status != OMT_XML_OK) return status == OMT_XML_UNSUPPORTED ? status : OMT_XML_MALFORMED;
        switch (ev.type) {
        case OMT_XML_EV_DOCTYPE: return OMT_XML_UNSUPPORTED;
        case OMT_XML_EV_START:
        case OMT_XML_EV_EMPTY: return omt_span_eq(ev.name, tag) ? OMT_XML_OK : OMT_XML_UNSUPPORTED;
        case OMT_XML_EV_EOF: return OMT_XML_NOT_FOUND;
        default: break;
        }
    }
}
