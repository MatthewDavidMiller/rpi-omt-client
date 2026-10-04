/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * A growable byte buffer with a hard ceiling.
 *
 * Every buffer is created with the most it may ever hold. An append that would
 * pass it, or an allocation that fails, marks the buffer failed and leaves its
 * contents as they were; every later append is a no-op. Callers build a whole
 * document and check `failed` once, instead of checking every append and
 * risking the one that was missed. The contents are always NUL-terminated
 * past `len`, so a text buffer can be handed to a C string API directly.
 */
#ifndef OMT_BUF_H
#define OMT_BUF_H

#include <stdarg.h>

#include "common/base.h"

typedef struct {
    uint8_t *data;
    size_t len;
    size_t cap;
    size_t limit;
    bool failed;
} omt_buf;

void omt_buf_init(omt_buf *b, size_t limit);
void omt_buf_free(omt_buf *b);
/* Zeroes the contents before freeing them, for buffers that held secrets. */
void omt_buf_free_secret(omt_buf *b);
void omt_buf_clear(omt_buf *b);
bool omt_buf_reserve(omt_buf *b, size_t extra);
void omt_buf_append(omt_buf *b, const void *data, size_t len);
void omt_buf_putc(omt_buf *b, uint8_t c);
void omt_buf_puts(omt_buf *b, const char *s);
void omt_buf_printf(omt_buf *b, const char *fmt, ...) OMT_PRINTF(2, 3);
void omt_buf_vprintf(omt_buf *b, const char *fmt, va_list args) OMT_PRINTF(2, 0);
/* Appends `len` bytes as lowercase hex. */
void omt_buf_hex(omt_buf *b, const uint8_t *data, size_t len);
/* Removes leading bytes, keeping the rest. */
void omt_buf_consume(omt_buf *b, size_t n);
static inline const char *omt_buf_cstr(const omt_buf *b) {
    return b->data ? (const char *)b->data : "";
}
/* Transfers ownership of the contents to the caller; the buffer is reset.
 * Returns NULL for a failed buffer. */
OMT_NODISCARD char *omt_buf_take(omt_buf *b, size_t *len);

/* A read-only view with a cursor. Every read checks the remaining length and
 * returns false rather than reading past the end. */
typedef struct {
    const uint8_t *p;
    size_t len;
} omt_span;

static inline omt_span omt_span_of(const void *p, size_t len) {
    omt_span s = {(const uint8_t *)p, len};
    return s;
}
OMT_NODISCARD bool omt_span_skip(omt_span *s, size_t n);
OMT_NODISCARD bool omt_span_take(omt_span *s, size_t n, omt_span *out);
OMT_NODISCARD bool omt_span_u8(omt_span *s, uint8_t *v);
OMT_NODISCARD bool omt_span_be32(omt_span *s, uint32_t *v);
bool omt_span_eq(omt_span s, const char *text);

/* Bounded C string helpers that replace the poisoned libc ones. Each returns
 * false when the destination is too small, leaving it NUL-terminated. */
bool omt_strlcpy(char *dst, const char *src, size_t size);
bool omt_snprintf(char *dst, size_t size, const char *fmt, ...) OMT_PRINTF(3, 4);
/* Parses an unsigned decimal of only ASCII digits, rejecting empty input,
 * signs, whitespace, and values above `max`. */
bool omt_parse_u64(const char *s, size_t len, uint64_t max, uint64_t *out);
bool omt_has_prefix(const char *s, const char *prefix);
bool omt_ascii_ieq(const char *a, size_t alen, const char *b);

#endif
