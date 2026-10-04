/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "common/buf.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

void omt_buf_init(omt_buf *b, size_t limit) {
    memset(b, 0, sizeof(*b));
    b->limit = limit;
}

void omt_buf_free(omt_buf *b) {
    free(b->data);
    size_t limit = b->limit;
    memset(b, 0, sizeof(*b));
    b->limit = limit;
}

void omt_buf_free_secret(omt_buf *b) {
    if (b->data) {
        volatile uint8_t *p = b->data;
        for (size_t i = 0; i < b->cap; i++) p[i] = 0;
    }
    omt_buf_free(b);
}

void omt_buf_clear(omt_buf *b) {
    b->len = 0;
    b->failed = false;
    if (b->data) b->data[0] = 0;
}

bool omt_buf_reserve(omt_buf *b, size_t extra) {
    if (b->failed) return false;
    size_t need;
    if (!omt_add(b->len, extra, &need) || need > b->limit) {
        b->failed = true;
        return false;
    }
    /* One byte past the contents always holds the terminator. */
    if (need + 1 <= b->cap) return true;
    size_t cap = b->cap ? b->cap : 64;
    while (cap < need + 1) {
        if (cap > SIZE_MAX / 2) {
            cap = need + 1;
            break;
        }
        cap *= 2;
    }
    if (cap > b->limit + 1) cap = b->limit + 1;
    uint8_t *grown = realloc(b->data, cap);
    if (!grown) {
        b->failed = true;
        return false;
    }
    b->data = grown;
    b->cap = cap;
    return true;
}

void omt_buf_append(omt_buf *b, const void *data, size_t len) {
    if (!omt_buf_reserve(b, len)) return;
    if (len) memcpy(b->data + b->len, data, len);
    b->len += len;
    b->data[b->len] = 0;
}

void omt_buf_putc(omt_buf *b, uint8_t c) { omt_buf_append(b, &c, 1); }

void omt_buf_puts(omt_buf *b, const char *s) { omt_buf_append(b, s, strlen(s)); }

void omt_buf_vprintf(omt_buf *b, const char *fmt, va_list args) {
    if (b->failed) return;
    va_list copy;
    va_copy(copy, args);
    int n = vsnprintf(NULL, 0, fmt, copy);
    va_end(copy);
    if (n < 0) {
        b->failed = true;
        return;
    }
    if (!omt_buf_reserve(b, (size_t)n)) return;
    vsnprintf((char *)b->data + b->len, (size_t)n + 1, fmt, args);
    b->len += (size_t)n;
}

void omt_buf_printf(omt_buf *b, const char *fmt, ...) {
    va_list args;
    va_start(args, fmt);
    omt_buf_vprintf(b, fmt, args);
    va_end(args);
}

void omt_buf_hex(omt_buf *b, const uint8_t *data, size_t len) {
    static const char digits[] = "0123456789abcdef";
    size_t width;
    if (!omt_mul(len, (size_t)2, &width) || !omt_buf_reserve(b, width)) {
        b->failed = true;
        return;
    }
    for (size_t i = 0; i < len; i++) {
        b->data[b->len++] = (uint8_t)digits[data[i] >> 4];
        b->data[b->len++] = (uint8_t)digits[data[i] & 15];
    }
    b->data[b->len] = 0;
}

void omt_buf_consume(omt_buf *b, size_t n) {
    if (n >= b->len) {
        omt_buf_clear(b);
        return;
    }
    memmove(b->data, b->data + n, b->len - n);
    b->len -= n;
    b->data[b->len] = 0;
}

char *omt_buf_take(omt_buf *b, size_t *len) {
    if (b->failed) {
        omt_buf_free(b);
        return NULL;
    }
    if (!b->data && !omt_buf_reserve(b, 0)) return NULL;
    if (!b->data) {
        b->data = malloc(1);
        if (!b->data) return NULL;
        b->data[0] = 0;
    }
    char *out = (char *)b->data;
    if (len) *len = b->len;
    size_t limit = b->limit;
    memset(b, 0, sizeof(*b));
    b->limit = limit;
    return out;
}

bool omt_span_skip(omt_span *s, size_t n) {
    if (n > s->len) return false;
    s->p += n;
    s->len -= n;
    return true;
}

bool omt_span_take(omt_span *s, size_t n, omt_span *out) {
    if (n > s->len) return false;
    out->p = s->p;
    out->len = n;
    s->p += n;
    s->len -= n;
    return true;
}

bool omt_span_u8(omt_span *s, uint8_t *v) {
    if (s->len < 1) return false;
    *v = s->p[0];
    return omt_span_skip(s, 1);
}

bool omt_span_be32(omt_span *s, uint32_t *v) {
    if (s->len < 4) return false;
    *v = omt_be32(s->p);
    return omt_span_skip(s, 4);
}

bool omt_span_eq(omt_span s, const char *text) {
    size_t n = strlen(text);
    return n == s.len && (n == 0 || memcmp(s.p, text, n) == 0);
}

bool omt_strlcpy(char *dst, const char *src, size_t size) {
    if (size == 0) return false;
    size_t n = strlen(src);
    if (n >= size) {
        memcpy(dst, src, size - 1);
        dst[size - 1] = 0;
        return false;
    }
    memcpy(dst, src, n + 1);
    return true;
}

bool omt_snprintf(char *dst, size_t size, const char *fmt, ...) {
    va_list args;
    va_start(args, fmt);
    int n = vsnprintf(dst, size, fmt, args);
    va_end(args);
    return n >= 0 && (size_t)n < size;
}

bool omt_parse_u64(const char *s, size_t len, uint64_t max, uint64_t *out) {
    if (len == 0) return false;
    uint64_t value = 0;
    for (size_t i = 0; i < len; i++) {
        if (s[i] < '0' || s[i] > '9') return false;
        if (!omt_mul(value, (uint64_t)10, &value) ||
            !omt_add(value, (uint64_t)(s[i] - '0'), &value))
            return false;
        if (value > max) return false;
    }
    *out = value;
    return true;
}

bool omt_has_prefix(const char *s, const char *prefix) {
    return strncmp(s, prefix, strlen(prefix)) == 0;
}

static char ascii_lower(char c) { return (c >= 'A' && c <= 'Z') ? (char)(c + 32) : c; }

bool omt_ascii_ieq(const char *a, size_t alen, const char *b) {
    size_t blen = strlen(b);
    if (alen != blen) return false;
    for (size_t i = 0; i < alen; i++)
        if (ascii_lower(a[i]) != ascii_lower(b[i])) return false;
    return true;
}
