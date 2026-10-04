/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "common/ipaddr.h"

#include <stdio.h>
#include <string.h>

bool omt_parse_ipv4(const char *s, size_t len, uint8_t out[4]) {
    size_t i = 0;
    for (int part = 0; part < 4; part++) {
        if (part) {
            if (i >= len || s[i] != '.') return false;
            i++;
        }
        size_t start = i;
        unsigned value = 0;
        while (i < len && s[i] >= '0' && s[i] <= '9' && i - start < 3) {
            value = value * 10 + (unsigned)(s[i] - '0');
            i++;
        }
        size_t digits = i - start;
        if (digits == 0 || value > 255 || (digits > 1 && s[start] == '0')) return false;
        out[part] = (uint8_t)value;
    }
    return i == len;
}

static int hexval(char c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    if (c >= 'A' && c <= 'F') return c - 'A' + 10;
    return -1;
}

/* Parses up to `limit` groups separated by ':' from s[*i..len). Stops before
 * a "::" or at the end. A trailing dotted quad counts as two groups when
 * `allow_v4` is set. Returns the number of groups, or -1 on error. */
static int read_groups(const char *s, size_t len, size_t *i, uint16_t *groups, int limit,
                       bool allow_v4) {
    int count = 0;
    while (count < limit) {
        size_t start = *i;
        /* Try an embedded IPv4 tail: it must run to the end of the input. */
        if (allow_v4 && count + 2 <= limit) {
            uint8_t v4[4];
            if (memchr(s + start, '.', len - start) && omt_parse_ipv4(s + start, len - start, v4)) {
                groups[count++] = (uint16_t)((v4[0] << 8) | v4[1]);
                groups[count++] = (uint16_t)((v4[2] << 8) | v4[3]);
                *i = len;
                return count;
            }
        }
        unsigned value = 0;
        size_t j = start;
        while (j < len && j - start < 4 && hexval(s[j]) >= 0) {
            value = (value << 4) | (unsigned)hexval(s[j]);
            j++;
        }
        if (j == start) return count == 0 ? 0 : -1;
        groups[count++] = (uint16_t)value;
        *i = j;
        if (j >= len) return count;
        if (s[j] != ':') return -1;
        if (j + 1 < len && s[j + 1] == ':') return count; /* leave "::" to the caller */
        if (j + 1 >= len) return -1;                      /* trailing single ':' */
        *i = j + 1;
    }
    return count;
}

bool omt_parse_ipv6(const char *s, size_t len, uint8_t out[16]) {
    uint16_t head[8] = {0}, tail[8] = {0};
    size_t i = 0;
    int nhead = read_groups(s, len, &i, head, 8, true);
    if (nhead < 0) return false;
    if (nhead == 8) {
        if (i != len) return false;
        for (int k = 0; k < 8; k++) {
            out[2 * k] = (uint8_t)(head[k] >> 8);
            out[2 * k + 1] = (uint8_t)head[k];
        }
        return true;
    }
    if (len - i < 2 || s[i] != ':' || s[i + 1] != ':') return false;
    i += 2;
    /* "::" stands for at least one group, so at most seven are explicit. */
    int limit = 7 - nhead;
    int ntail = 0;
    if (i < len) {
        ntail = read_groups(s, len, &i, tail, limit, true);
        if (ntail <= 0 || i != len) return false;
    }
    memset(out, 0, 16);
    for (int k = 0; k < nhead; k++) {
        out[2 * k] = (uint8_t)(head[k] >> 8);
        out[2 * k + 1] = (uint8_t)head[k];
    }
    for (int k = 0; k < ntail; k++) {
        int at = 8 - ntail + k;
        out[2 * at] = (uint8_t)(tail[k] >> 8);
        out[2 * at + 1] = (uint8_t)tail[k];
    }
    return true;
}

void omt_format_ipv6(const uint8_t a[16], char out[46]) {
    uint16_t g[8];
    for (int i = 0; i < 8; i++) g[i] = (uint16_t)(a[2 * i] << 8 | a[2 * i + 1]);
    if (!g[0] && !g[1] && !g[2] && !g[3] && !g[4] && g[5] == 0xFFFF) {
        snprintf(out, 46, "::ffff:%u.%u.%u.%u", a[12], a[13], a[14], a[15]);
        return;
    }
    int best = -1, best_len = 0;
    for (int i = 0; i < 8;) {
        if (g[i]) {
            i++;
            continue;
        }
        int start = i;
        while (i < 8 && !g[i]) i++;
        if (i - start > best_len) {
            best = start;
            best_len = i - start;
        }
    }
    if (best_len < 2) best = -1;
    size_t n = 0;
    for (int i = 0; i < 8; i++) {
        if (i == best) {
            n += (size_t)snprintf(out + n, 46 - n, "::");
            i += best_len - 1;
            continue;
        }
        if (i && !(best >= 0 && i == best + best_len)) n += (size_t)snprintf(out + n, 46 - n, ":");
        n += (size_t)snprintf(out + n, 46 - n, "%x", g[i]);
    }
}
