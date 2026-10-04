/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * SSH data types (RFC 4251 section 5). Every reader checks the remaining
 * length before it reads and returns false instead of reading past the end.
 */
#include <string.h>

#include "deploy/ssh/ssh_internal.h"

void sb_u8(omt_buf *b, uint8_t v) { omt_buf_putc(b, v); }
void sb_bool(omt_buf *b, bool v) { omt_buf_putc(b, v ? 1 : 0); }

void sb_u32(omt_buf *b, uint32_t v) {
    uint8_t raw[4];
    omt_put_be32(raw, v);
    omt_buf_append(b, raw, 4);
}

void sb_u64(omt_buf *b, uint64_t v) {
    sb_u32(b, (uint32_t)(v >> 32));
    sb_u32(b, (uint32_t)v);
}

void sb_string(omt_buf *b, const void *data, size_t len) {
    if (len > UINT32_MAX) {
        b->failed = true;
        return;
    }
    sb_u32(b, (uint32_t)len);
    omt_buf_append(b, data, len);
}

void sb_cstring(omt_buf *b, const char *s) { sb_string(b, s, strlen(s)); }

void sb_mpint_bytes(omt_buf *b, const uint8_t *be, size_t len) {
    while (len > 0 && be[0] == 0) {
        be++;
        len--;
    }
    bool pad = len > 0 && (be[0] & 0x80);
    sb_u32(b, (uint32_t)(len + (pad ? 1 : 0)));
    if (pad) omt_buf_putc(b, 0);
    omt_buf_append(b, be, len);
}

bool sb_mpint_bn(omt_buf *b, const BIGNUM *bn) {
    int n = BN_num_bytes(bn);
    if (n < 0 || BN_is_negative(bn)) return false;
    uint8_t stack[1100];
    uint8_t *raw = (size_t)n <= sizeof(stack) ? stack : OPENSSL_malloc((size_t)n);
    if (!raw) return false;
    BN_bn2bin(bn, raw);
    sb_mpint_bytes(b, raw, (size_t)n);
    OPENSSL_cleanse(raw, (size_t)n);
    if (raw != stack) OPENSSL_free(raw);
    return !b->failed;
}

bool sr_u8(omt_span *s, uint8_t *v) { return omt_span_u8(s, v); }

bool sr_bool(omt_span *s, bool *v) {
    uint8_t raw;
    if (!omt_span_u8(s, &raw)) return false;
    *v = raw != 0;
    return true;
}

bool sr_u32(omt_span *s, uint32_t *v) { return omt_span_be32(s, v); }

bool sr_u64(omt_span *s, uint64_t *v) {
    uint32_t hi, lo;
    if (!omt_span_be32(s, &hi) || !omt_span_be32(s, &lo)) return false;
    *v = ((uint64_t)hi << 32) | lo;
    return true;
}

bool sr_string(omt_span *s, omt_span *out) {
    uint32_t len;
    return omt_span_be32(s, &len) && omt_span_take(s, len, out);
}

bool sr_mpint_bytes(omt_span *s, omt_span *out) {
    omt_span raw;
    if (!sr_string(s, &raw)) return false;
    if (raw.len > 0 && (raw.p[0] & 0x80)) return false; /* negative */
    /* Leading zeros are trimmed rather than refused, as OpenSSH does. */
    while (raw.len > 0 && raw.p[0] == 0) {
        raw.p++;
        raw.len--;
    }
    *out = raw;
    return true;
}

bool sr_mpint_bn(omt_span *s, BIGNUM **out) {
    omt_span mag;
    if (!sr_mpint_bytes(s, &mag) || mag.len > 2048) return false;
    *out = BN_bin2bn(mag.p, (int)mag.len, NULL);
    return *out != NULL;
}

bool span_is(omt_span s, const char *text) {
    size_t n = strlen(text);
    return s.len == n && memcmp(s.p, text, n) == 0;
}

bool namelist_has(omt_span list, const char *name) {
    size_t n = strlen(name);
    size_t start = 0;
    for (size_t i = 0; i <= list.len; i++) {
        if (i == list.len || list.p[i] == ',') {
            if (i - start == n && memcmp(list.p + start, name, n) == 0) return true;
            start = i + 1;
        }
    }
    return false;
}
