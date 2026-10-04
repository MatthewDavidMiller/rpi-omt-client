/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "common/json.h"

#include <math.h>
#include <stdlib.h>
#include <string.h>

#define JSON_MAX_DEPTH 128

struct omt_json_block {
    omt_json_block *next;
    size_t size;
    size_t used;
    max_align_t data[];
};

static void *arena_alloc(omt_json_doc *doc, size_t size) {
    size = (size + 15u) & ~(size_t)15u;
    omt_json_block *block = doc->blocks;
    if (!block || block->size - block->used < size) {
        size_t want = size > 4096 ? size : 4096;
        size_t total;
        if (!omt_add(doc->total, want, &total) || total > doc->limit) return NULL;
        block = malloc(sizeof(*block) + want);
        if (!block) return NULL;
        block->next = doc->blocks;
        block->size = want;
        block->used = 0;
        doc->blocks = block;
        doc->total = total;
    }
    void *p = (uint8_t *)block->data + block->used;
    block->used += size;
    return p;
}

static void free_blocks(omt_json_doc *doc, bool wipe) {
    omt_json_block *block = doc->blocks;
    while (block) {
        omt_json_block *next = block->next;
        if (wipe) {
            volatile uint8_t *p = (volatile uint8_t *)block->data;
            for (size_t i = 0; i < block->size; i++) p[i] = 0;
        }
        free(block);
        block = next;
    }
    doc->blocks = NULL;
    doc->total = 0;
}

void omt_json_doc_free(omt_json_doc *doc) { free_blocks(doc, false); }

void omt_json_doc_free_secret(omt_json_doc *doc) { free_blocks(doc, true); }

typedef struct {
    omt_json_doc *doc;
    const char *s;
    size_t len;
    size_t pos;
    int depth;
} parser;

static bool fail(parser *p, const char *message) {
    if (!p->doc->error[0]) omt_strlcpy(p->doc->error, message, sizeof(p->doc->error));
    return false;
}

static void skip_ws(parser *p) {
    while (p->pos < p->len) {
        char c = p->s[p->pos];
        if (c != ' ' && c != '\t' && c != '\n' && c != '\r') break;
        p->pos++;
    }
}

static bool literal(parser *p, const char *word) {
    size_t n = strlen(word);
    if (p->len - p->pos < n || memcmp(p->s + p->pos, word, n) != 0)
        return fail(p, "invalid JSON literal");
    p->pos += n;
    return true;
}

static int hex_digit(char c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    if (c >= 'A' && c <= 'F') return c - 'A' + 10;
    return -1;
}

static bool read_hex4(parser *p, uint32_t *out) {
    if (p->len - p->pos < 4) return fail(p, "truncated JSON escape");
    uint32_t v = 0;
    for (int i = 0; i < 4; i++) {
        int d = hex_digit(p->s[p->pos + (size_t)i]);
        if (d < 0) return fail(p, "invalid JSON escape");
        v = (v << 4) | (uint32_t)d;
    }
    p->pos += 4;
    *out = v;
    return true;
}

static bool parse_string(parser *p, char **out, size_t *out_len) {
    /* Caller has consumed the opening quote. */
    omt_buf text;
    omt_buf_init(&text, p->len);
    for (;;) {
        if (p->pos >= p->len) {
            omt_buf_free(&text);
            return fail(p, "unterminated JSON string");
        }
        unsigned char c = (unsigned char)p->s[p->pos];
        if (c == '"') {
            p->pos++;
            break;
        }
        if (c < 0x20) {
            omt_buf_free(&text);
            return fail(p, "control character in JSON string");
        }
        if (c != '\\') {
            /* Copy the run of plain bytes; UTF-8 validity was checked up front. */
            size_t start = p->pos;
            while (p->pos < p->len) {
                unsigned char d = (unsigned char)p->s[p->pos];
                if (d == '"' || d == '\\' || d < 0x20) break;
                p->pos++;
            }
            omt_buf_append(&text, p->s + start, p->pos - start);
            continue;
        }
        p->pos++;
        if (p->pos >= p->len) {
            omt_buf_free(&text);
            return fail(p, "truncated JSON escape");
        }
        char e = p->s[p->pos++];
        switch (e) {
        case '"': omt_buf_putc(&text, '"'); break;
        case '\\': omt_buf_putc(&text, '\\'); break;
        case '/': omt_buf_putc(&text, '/'); break;
        case 'b': omt_buf_putc(&text, '\b'); break;
        case 'f': omt_buf_putc(&text, '\f'); break;
        case 'n': omt_buf_putc(&text, '\n'); break;
        case 'r': omt_buf_putc(&text, '\r'); break;
        case 't': omt_buf_putc(&text, '\t'); break;
        case 'u': {
            uint32_t cp;
            if (!read_hex4(p, &cp)) {
                omt_buf_free(&text);
                return false;
            }
            if (cp >= 0xDC00 && cp <= 0xDFFF) {
                omt_buf_free(&text);
                return fail(p, "lone JSON surrogate");
            }
            if (cp >= 0xD800 && cp <= 0xDBFF) {
                uint32_t low;
                if (p->len - p->pos < 2 || p->s[p->pos] != '\\' || p->s[p->pos + 1] != 'u') {
                    omt_buf_free(&text);
                    return fail(p, "lone JSON surrogate");
                }
                p->pos += 2;
                if (!read_hex4(p, &low)) {
                    omt_buf_free(&text);
                    return false;
                }
                if (low < 0xDC00 || low > 0xDFFF) {
                    omt_buf_free(&text);
                    return fail(p, "lone JSON surrogate");
                }
                cp = 0x10000 + ((cp - 0xD800) << 10) + (low - 0xDC00);
            }
            omt_utf8_put(&text, cp);
            break;
        }
        default: omt_buf_free(&text); return fail(p, "invalid JSON escape");
        }
    }
    if (text.failed) {
        omt_buf_free(&text);
        return fail(p, "JSON string too large");
    }
    char *copy = arena_alloc(p->doc, text.len + 1);
    if (!copy) {
        omt_buf_free(&text);
        return fail(p, "JSON document too large");
    }
    if (text.len) memcpy(copy, text.data, text.len);
    copy[text.len] = 0;
    *out = copy;
    *out_len = text.len;
    omt_buf_free(&text);
    return true;
}

static bool parse_number(parser *p, omt_json *v) {
    size_t start = p->pos;
    v->type = OMT_JSON_NUMBER;
    v->is_integer = true;
    if (p->s[p->pos] == '-') {
        v->negative = true;
        p->pos++;
    }
    if (p->pos >= p->len) return fail(p, "invalid JSON number");
    if (p->s[p->pos] == '0') {
        p->pos++;
        if (p->pos < p->len && p->s[p->pos] >= '0' && p->s[p->pos] <= '9')
            return fail(p, "invalid JSON number");
    } else if (p->s[p->pos] >= '1' && p->s[p->pos] <= '9') {
        while (p->pos < p->len && p->s[p->pos] >= '0' && p->s[p->pos] <= '9') p->pos++;
    } else {
        return fail(p, "invalid JSON number");
    }
    size_t int_end = p->pos;
    if (p->pos < p->len && p->s[p->pos] == '.') {
        v->is_integer = false;
        p->pos++;
        size_t digits = p->pos;
        while (p->pos < p->len && p->s[p->pos] >= '0' && p->s[p->pos] <= '9') p->pos++;
        if (p->pos == digits) return fail(p, "invalid JSON number");
    }
    if (p->pos < p->len && (p->s[p->pos] == 'e' || p->s[p->pos] == 'E')) {
        v->is_integer = false;
        p->pos++;
        if (p->pos < p->len && (p->s[p->pos] == '+' || p->s[p->pos] == '-')) p->pos++;
        size_t digits = p->pos;
        while (p->pos < p->len && p->s[p->pos] >= '0' && p->s[p->pos] <= '9') p->pos++;
        if (p->pos == digits) return fail(p, "invalid JSON number");
    }
    size_t digits_start = start + (v->negative ? 1u : 0u);
    if (v->is_integer) {
        uint64_t m = 0;
        for (size_t i = digits_start; i < int_end; i++) {
            if (!omt_mul(m, (uint64_t)10, &m) || !omt_add(m, (uint64_t)(p->s[i] - '0'), &m)) {
                v->magnitude_overflow = true;
                break;
            }
        }
        v->magnitude = m;
    }
    char text[64];
    size_t n = p->pos - start;
    if (n >= sizeof(text)) {
        /* Longer literals are legal JSON but nothing this project reads needs
         * them; strtod on a bounded copy keeps the parse allocation-free. */
        if (v->is_integer) {
            v->number = v->negative ? -HUGE_VAL : HUGE_VAL;
            v->magnitude_overflow = true;
            return true;
        }
        return fail(p, "JSON number too long");
    }
    memcpy(text, p->s + start, n);
    text[n] = 0;
    v->number = strtod(text, NULL);
    if (!__builtin_isfinite(v->number) && !v->is_integer) return fail(p, "non-finite JSON number");
    return true;
}

static omt_json *parse_value(parser *p);

static bool key_seen(omt_json *object, const char *key, size_t len) {
    for (size_t i = 0; i < object->count; i++)
        if (object->key_lens[i] == len && memcmp(object->keys[i], key, len) == 0) return true;
    return false;
}

/* Grows a member array geometrically inside the arena. */
static bool grow(parser *p, omt_json *v, size_t *cap, bool object) {
    if (v->count < *cap) return true;
    size_t next = *cap ? *cap * 2 : 4;
    size_t bytes;
    if (!omt_mul(next, sizeof(omt_json *), &bytes)) return fail(p, "JSON document too large");
    omt_json **items = arena_alloc(p->doc, bytes);
    if (!items) return fail(p, "JSON document too large");
    if (v->count) memcpy(items, v->items, v->count * sizeof(omt_json *));
    v->items = items;
    if (object) {
        char **keys = arena_alloc(p->doc, next * sizeof(char *));
        size_t *lens = arena_alloc(p->doc, next * sizeof(size_t));
        if (!keys || !lens) return fail(p, "JSON document too large");
        if (v->count) {
            memcpy(keys, v->keys, v->count * sizeof(char *));
            memcpy(lens, v->key_lens, v->count * sizeof(size_t));
        }
        v->keys = keys;
        v->key_lens = lens;
    }
    *cap = next;
    return true;
}

static omt_json *parse_value(parser *p) {
    skip_ws(p);
    if (p->pos >= p->len) {
        fail(p, "unexpected end of JSON");
        return NULL;
    }
    omt_json *v = arena_alloc(p->doc, sizeof(*v));
    if (!v) {
        fail(p, "JSON document too large");
        return NULL;
    }
    memset(v, 0, sizeof(*v));
    char c = p->s[p->pos];
    switch (c) {
    case 'n':
        if (!literal(p, "null")) return NULL;
        v->type = OMT_JSON_NULL;
        return v;
    case 't':
        if (!literal(p, "true")) return NULL;
        v->type = OMT_JSON_BOOL;
        v->boolean = true;
        return v;
    case 'f':
        if (!literal(p, "false")) return NULL;
        v->type = OMT_JSON_BOOL;
        return v;
    case '"':
        p->pos++;
        v->type = OMT_JSON_STRING;
        if (!parse_string(p, &v->string, &v->string_len)) return NULL;
        return v;
    case '[':
    case '{': {
        bool object = c == '{';
        if (++p->depth > JSON_MAX_DEPTH) {
            fail(p, "JSON nested too deeply");
            return NULL;
        }
        p->pos++;
        v->type = object ? OMT_JSON_OBJECT : OMT_JSON_ARRAY;
        size_t cap = 0;
        skip_ws(p);
        if (p->pos < p->len && p->s[p->pos] == (object ? '}' : ']')) {
            p->pos++;
            p->depth--;
            return v;
        }
        for (;;) {
            if (!grow(p, v, &cap, object)) return NULL;
            if (object) {
                skip_ws(p);
                if (p->pos >= p->len || p->s[p->pos] != '"') {
                    fail(p, "expected JSON object key");
                    return NULL;
                }
                p->pos++;
                char *key;
                size_t key_len;
                if (!parse_string(p, &key, &key_len)) return NULL;
                if (key_seen(v, key, key_len)) {
                    fail(p, "duplicate JSON key");
                    return NULL;
                }
                skip_ws(p);
                if (p->pos >= p->len || p->s[p->pos] != ':') {
                    fail(p, "expected ':' in JSON object");
                    return NULL;
                }
                p->pos++;
                v->keys[v->count] = key;
                v->key_lens[v->count] = key_len;
            }
            omt_json *item = parse_value(p);
            if (!item) return NULL;
            v->items[v->count++] = item;
            skip_ws(p);
            if (p->pos >= p->len) {
                fail(p, "unexpected end of JSON");
                return NULL;
            }
            char sep = p->s[p->pos++];
            if (sep == ',') continue;
            if (sep == (object ? '}' : ']')) break;
            fail(p, "expected ',' in JSON container");
            return NULL;
        }
        p->depth--;
        return v;
    }
    default:
        if (c == '-' || (c >= '0' && c <= '9')) {
            if (!parse_number(p, v)) return NULL;
            return v;
        }
        fail(p, "unexpected JSON character");
        return NULL;
    }
}

omt_json *omt_json_parse(omt_json_doc *doc, const char *text, size_t len, size_t arena_limit) {
    memset(doc, 0, sizeof(*doc));
    if (arena_limit == 0) {
        if (!omt_mul(len, (size_t)64, &arena_limit) ||
            !omt_add(arena_limit, (size_t)65536, &arena_limit))
            arena_limit = SIZE_MAX;
    }
    doc->limit = arena_limit;
    if (!omt_utf8_valid(text, len)) {
        omt_strlcpy(doc->error, "JSON is not valid UTF-8", sizeof(doc->error));
        return NULL;
    }
    parser p = {doc, text, len, 0, 0};
    omt_json *root = parse_value(&p);
    if (root) {
        skip_ws(&p);
        if (p.pos != len) {
            fail(&p, "trailing characters after JSON");
            root = NULL;
        }
    }
    if (!root) omt_json_doc_free(doc);
    return root;
}

const omt_json *omt_json_get(const omt_json *object, const char *key) {
    if (!object || object->type != OMT_JSON_OBJECT) return NULL;
    size_t n = strlen(key);
    for (size_t i = 0; i < object->count; i++)
        if (object->key_lens[i] == n && memcmp(object->keys[i], key, n) == 0)
            return object->items[i];
    return NULL;
}

bool omt_json_only_keys(const omt_json *object, const char *const *allowed, size_t count) {
    if (!object || object->type != OMT_JSON_OBJECT) return false;
    for (size_t i = 0; i < object->count; i++) {
        bool known = false;
        for (size_t j = 0; j < count && !known; j++)
            known = strlen(allowed[j]) == object->key_lens[i] &&
                    memcmp(allowed[j], object->keys[i], object->key_lens[i]) == 0;
        if (!known) return false;
    }
    return true;
}

bool omt_json_as_u64(const omt_json *value, uint64_t *out) {
    if (!value || value->type != OMT_JSON_NUMBER || !value->is_integer || value->magnitude_overflow)
        return false;
    if (value->negative && value->magnitude != 0) return false;
    *out = value->magnitude;
    return true;
}

bool omt_json_as_i64(const omt_json *value, int64_t *out) {
    if (!value || value->type != OMT_JSON_NUMBER || !value->is_integer || value->magnitude_overflow)
        return false;
    if (value->negative) {
        if (value->magnitude > (uint64_t)INT64_MAX + 1u) return false;
        *out =
            value->magnitude == (uint64_t)INT64_MAX + 1u ? INT64_MIN : -(int64_t)value->magnitude;
    } else {
        if (value->magnitude > (uint64_t)INT64_MAX) return false;
        *out = (int64_t)value->magnitude;
    }
    return true;
}

const char *omt_json_as_str(const omt_json *value) {
    if (!value || value->type != OMT_JSON_STRING) return NULL;
    if (strlen(value->string) != value->string_len) return NULL;
    return value->string;
}

void omt_json_write_string(omt_buf *out, const char *s, size_t len) {
    static const char hex[] = "0123456789abcdef";
    omt_buf_putc(out, '"');
    size_t run = 0;
    for (size_t i = 0; i < len; i++) {
        unsigned char c = (unsigned char)s[i];
        const char *escape = NULL;
        char unicode[7];
        switch (c) {
        case '"': escape = "\\\""; break;
        case '\\': escape = "\\\\"; break;
        case '\b': escape = "\\b"; break;
        case '\f': escape = "\\f"; break;
        case '\n': escape = "\\n"; break;
        case '\r': escape = "\\r"; break;
        case '\t': escape = "\\t"; break;
        default:
            if (c < 0x20) {
                unicode[0] = '\\';
                unicode[1] = 'u';
                unicode[2] = '0';
                unicode[3] = '0';
                unicode[4] = hex[c >> 4];
                unicode[5] = hex[c & 15];
                unicode[6] = 0;
                escape = unicode;
            }
        }
        if (escape) {
            omt_buf_append(out, s + i - run, run);
            run = 0;
            omt_buf_puts(out, escape);
        } else {
            run++;
        }
    }
    omt_buf_append(out, s + len - run, run);
    omt_buf_putc(out, '"');
}

void omt_json_write_f64(omt_buf *out, double value) {
    if (!__builtin_isfinite(value)) {
        omt_buf_puts(out, "null");
        return;
    }
    if (value == 0) {
        omt_buf_puts(out, __builtin_signbit(value) ? "-0.0" : "0.0");
        return;
    }
    /* Shortest digit string that round-trips, found by widening precision. */
    char sci[40];
    int precision = 1;
    for (; precision <= 17; precision++) {
        snprintf(sci, sizeof(sci), "%.*e", precision - 1, value);
        if (strtod(sci, NULL) == value) break;
    }
    /* sci is [-]d[.ddd]e[+-]XX; split it into digits and a decimal exponent. */
    char digits[24];
    size_t nd = 0;
    const char *p = sci;
    bool negative = false;
    if (*p == '-') {
        negative = true;
        p++;
    }
    for (; *p && *p != 'e'; p++)
        if (*p != '.' && nd < sizeof(digits) - 1) digits[nd++] = *p;
    while (nd > 1 && digits[nd - 1] == '0') nd--;
    digits[nd] = 0;
    long exponent = strtol(p + 1, NULL, 10);
    /* Lay the digits out the way ryu's format64 does, which serde_json uses:
     * plain decimal for exponents in (-5, 16], scientific otherwise. */
    long kk = exponent + 1; /* position of the decimal point */
    if (negative) omt_buf_putc(out, '-');
    if ((long)nd <= kk && kk <= 16) {
        omt_buf_append(out, digits, nd);
        for (long i = (long)nd; i < kk; i++) omt_buf_putc(out, '0');
        omt_buf_puts(out, ".0");
    } else if (0 < kk && kk <= 16) {
        omt_buf_append(out, digits, (size_t)kk);
        omt_buf_putc(out, '.');
        omt_buf_append(out, digits + kk, nd - (size_t)kk);
    } else if (-5 < kk && kk <= 0) {
        omt_buf_puts(out, "0.");
        for (long i = kk; i < 0; i++) omt_buf_putc(out, '0');
        omt_buf_append(out, digits, nd);
    } else {
        omt_buf_putc(out, (uint8_t)digits[0]);
        if (nd > 1) {
            omt_buf_putc(out, '.');
            omt_buf_append(out, digits + 1, nd - 1);
        }
        omt_buf_printf(out, "e%ld", kk - 1);
    }
}

bool omt_utf8_valid(const char *str, size_t len) {
    const unsigned char *s = (const unsigned char *)str;
    size_t i = 0;
    while (i < len) {
        unsigned char c = s[i];
        if (c < 0x80) {
            i++;
            continue;
        }
        size_t n;
        uint32_t cp, min;
        if ((c & 0xE0) == 0xC0) {
            n = 2;
            cp = c & 0x1Fu;
            min = 0x80;
        } else if ((c & 0xF0) == 0xE0) {
            n = 3;
            cp = c & 0x0Fu;
            min = 0x800;
        } else if ((c & 0xF8) == 0xF0) {
            n = 4;
            cp = c & 0x07u;
            min = 0x10000;
        } else {
            return false;
        }
        if (len - i < n) return false;
        for (size_t k = 1; k < n; k++) {
            if ((s[i + k] & 0xC0) != 0x80) return false;
            cp = (cp << 6) | (s[i + k] & 0x3Fu);
        }
        if (cp < min || cp > 0x10FFFF || (cp >= 0xD800 && cp <= 0xDFFF)) return false;
        i += n;
    }
    return true;
}

uint32_t omt_utf8_next(const char *str, size_t len, size_t *i) {
    const unsigned char *s = (const unsigned char *)str;
    unsigned char c = s[*i];
    if (c < 0x80) {
        (*i)++;
        return c;
    }
    size_t n = (c & 0xE0) == 0xC0 ? 2 : (c & 0xF0) == 0xE0 ? 3 : 4;
    uint32_t cp = c & (n == 2 ? 0x1Fu : n == 3 ? 0x0Fu : 0x07u);
    for (size_t k = 1; k < n && *i + k < len; k++) cp = (cp << 6) | (s[*i + k] & 0x3Fu);
    /* A sequence cut short by the end of the input ends there: stepping past
     * len would hand every caller's `i < len` loop an index beyond the data. */
    *i = len - *i < n ? len : *i + n;
    return cp;
}

void omt_utf8_put(omt_buf *out, uint32_t cp) {
    uint8_t b[4];
    size_t n;
    if (cp < 0x80) {
        b[0] = (uint8_t)cp;
        n = 1;
    } else if (cp < 0x800) {
        b[0] = (uint8_t)(0xC0 | (cp >> 6));
        b[1] = (uint8_t)(0x80 | (cp & 0x3F));
        n = 2;
    } else if (cp < 0x10000) {
        b[0] = (uint8_t)(0xE0 | (cp >> 12));
        b[1] = (uint8_t)(0x80 | ((cp >> 6) & 0x3F));
        b[2] = (uint8_t)(0x80 | (cp & 0x3F));
        n = 3;
    } else {
        b[0] = (uint8_t)(0xF0 | (cp >> 18));
        b[1] = (uint8_t)(0x80 | ((cp >> 12) & 0x3F));
        b[2] = (uint8_t)(0x80 | ((cp >> 6) & 0x3F));
        b[3] = (uint8_t)(0x80 | (cp & 0x3F));
        n = 4;
    }
    omt_buf_append(out, b, n);
}

void omt_utf8_lossy(omt_buf *out, const char *str, size_t len) {
    const unsigned char *s = (const unsigned char *)str;
    size_t i = 0;
    while (i < len) {
        unsigned char c = s[i];
        if (c < 0x80) {
            omt_buf_putc(out, c);
            i++;
            continue;
        }
        size_t need;
        unsigned char lo = 0x80, hi = 0xBF;
        if (c >= 0xC2 && c <= 0xDF) {
            need = 1;
        } else if (c >= 0xE0 && c <= 0xEF) {
            need = 2;
            if (c == 0xE0) lo = 0xA0;
            if (c == 0xED) hi = 0x9F;
        } else if (c >= 0xF0 && c <= 0xF4) {
            need = 3;
            if (c == 0xF0) lo = 0x90;
            if (c == 0xF4) hi = 0x8F;
        } else {
            omt_buf_puts(out, "\xEF\xBF\xBD");
            i++;
            continue;
        }
        size_t k = 1;
        for (; k <= need; k++) {
            if (i + k >= len) break;
            unsigned char d = s[i + k];
            unsigned char min = k == 1 ? lo : 0x80, max = k == 1 ? hi : 0xBF;
            if (d < min || d > max) break;
        }
        if (k == need + 1) {
            omt_buf_append(out, s + i, need + 1);
            i += need + 1;
        } else {
            omt_buf_puts(out, "\xEF\xBF\xBD");
            i += k;
        }
    }
}

void omt_fmt_f64_display(omt_buf *out, double value) {
    if (__builtin_isnan(value)) {
        omt_buf_puts(out, "NaN");
        return;
    }
    if (__builtin_isinf(value)) {
        omt_buf_puts(out, value < 0 ? "-inf" : "inf");
        return;
    }
    if (value == 0) {
        omt_buf_puts(out, __builtin_signbit(value) ? "-0" : "0");
        return;
    }
    char sci[40];
    int precision = 1;
    for (; precision <= 17; precision++) {
        snprintf(sci, sizeof(sci), "%.*e", precision - 1, value);
        if (strtod(sci, NULL) == value) break;
    }
    char digits[24];
    size_t nd = 0;
    const char *p = sci;
    if (*p == '-') {
        omt_buf_putc(out, '-');
        p++;
    }
    for (; *p && *p != 'e'; p++)
        if (*p != '.' && nd < sizeof(digits) - 1) digits[nd++] = *p;
    while (nd > 1 && digits[nd - 1] == '0') nd--;
    long kk = strtol(p + 1, NULL, 10) + 1;
    if (kk <= 0) {
        omt_buf_puts(out, "0.");
        for (long i = kk; i < 0; i++) omt_buf_putc(out, '0');
        omt_buf_append(out, digits, nd);
    } else if ((long)nd <= kk) {
        omt_buf_append(out, digits, nd);
        for (long i = (long)nd; i < kk; i++) omt_buf_putc(out, '0');
    } else {
        omt_buf_append(out, digits, (size_t)kk);
        omt_buf_putc(out, '.');
        omt_buf_append(out, digits + kk, nd - (size_t)kk);
    }
}

bool omt_unicode_is_whitespace(uint32_t cp) {
    return (cp >= 0x09 && cp <= 0x0D) || cp == 0x20 || cp == 0x85 || cp == 0xA0 || cp == 0x1680 ||
           (cp >= 0x2000 && cp <= 0x200A) || cp == 0x2028 || cp == 0x2029 || cp == 0x202F ||
           cp == 0x205F || cp == 0x3000;
}

omt_span omt_utf8_trim(const char *s, size_t len) {
    size_t start = 0;
    while (start < len) {
        size_t next = start;
        uint32_t cp = omt_utf8_next(s, len, &next);
        if (!omt_unicode_is_whitespace(cp)) break;
        start = next;
    }
    size_t end = len;
    while (end > start) {
        /* Step back to the start of the previous code point. */
        size_t prev = end - 1;
        while (prev > start && ((unsigned char)s[prev] & 0xC0) == 0x80) prev--;
        size_t probe = prev;
        uint32_t cp = omt_utf8_next(s, len, &probe);
        if (!omt_unicode_is_whitespace(cp)) break;
        end = prev;
    }
    return omt_span_of(s + start, end - start);
}
