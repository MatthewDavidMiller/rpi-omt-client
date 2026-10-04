/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "web/zip.h"

#include <stdlib.h>
#include <string.h>

uint32_t omt_crc32(const void *data, size_t len) {
    static uint32_t table[256];
    static bool ready;
    if (!ready) {
        for (uint32_t i = 0; i < 256; i++) {
            uint32_t c = i;
            for (int k = 0; k < 8; k++) c = c & 1 ? 0xEDB88320u ^ (c >> 1) : c >> 1;
            table[i] = c;
        }
        ready = true;
    }
    const uint8_t *p = data;
    uint32_t crc = 0xFFFFFFFFu;
    for (size_t i = 0; i < len; i++) crc = table[(crc ^ p[i]) & 0xFF] ^ (crc >> 8);
    return crc ^ 0xFFFFFFFFu;
}

/* ------------------------------------------------------------- deflate */

typedef struct {
    omt_buf *out;
    uint32_t bits;
    int count;
} bit_writer;

static void put_bits(bit_writer *w, uint32_t value, int n) {
    w->bits |= value << w->count;
    w->count += n;
    while (w->count >= 8) {
        omt_buf_putc(w->out, (uint8_t)w->bits);
        w->bits >>= 8;
        w->count -= 8;
    }
}

/* Huffman codes are sent most-significant bit first. */
static void put_code(bit_writer *w, uint32_t code, int n) {
    uint32_t reversed = 0;
    for (int i = 0; i < n; i++) reversed |= ((code >> i) & 1u) << (n - 1 - i);
    put_bits(w, reversed, n);
}

/* The fixed literal/length code of RFC 1951 section 3.2.6. */
static void put_literal(bit_writer *w, unsigned symbol) {
    if (symbol <= 143)
        put_code(w, 0x30 + symbol, 8);
    else if (symbol <= 255)
        put_code(w, 0x190 + (symbol - 144), 9);
    else if (symbol <= 279)
        put_code(w, symbol - 256, 7);
    else
        put_code(w, 0xC0 + (symbol - 280), 8);
}

static const uint16_t length_base[29] = {3,  4,  5,  6,   7,   8,   9,   10,  11, 13,
                                         15, 17, 19, 23,  27,  31,  35,  43,  51, 59,
                                         67, 83, 99, 115, 131, 163, 195, 227, 258};
static const uint8_t length_extra[29] = {0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2,
                                         2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0};
static const uint16_t dist_base[30] = {
    1,   2,   3,   4,   5,   7,    9,    13,   17,   25,   33,   49,   65,    97,    129,
    193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577};
static const uint8_t dist_extra[30] = {0, 0, 0, 0, 1, 1, 2, 2,  3,  3,  4,  4,  5,  5,  6,
                                       6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13};

static void put_match(bit_writer *w, unsigned length, unsigned distance) {
    unsigned l = 28;
    while (length_base[l] > length) l--;
    put_literal(w, 257 + l);
    if (length_extra[l]) put_bits(w, length - length_base[l], length_extra[l]);
    unsigned d = 29;
    while (dist_base[d] > distance) d--;
    put_code(w, d, 5);
    if (dist_extra[d]) put_bits(w, distance - dist_base[d], dist_extra[d]);
}

#define WINDOW 32768u
#define HASH_BITS 15
#define HASH_SIZE (1u << HASH_BITS)
#define MAX_CHAIN 64
#define MIN_MATCH 3
#define MAX_MATCH 258

static uint32_t hash3(const uint8_t *p) {
    return ((uint32_t)p[0] << 16 ^ (uint32_t)p[1] << 8 ^ p[2]) * 2654435761u >> (32 - HASH_BITS);
}

void omt_deflate(const uint8_t *data, size_t len, omt_buf *out) {
    bit_writer w = {out, 0, 0};
    /* One fixed-Huffman block for the whole member: BFINAL=1, BTYPE=01. */
    put_bits(&w, 1, 1);
    put_bits(&w, 1, 2);
    int32_t *head = malloc(HASH_SIZE * sizeof(int32_t));
    int32_t *prev = malloc(WINDOW * sizeof(int32_t));
    if (!head || !prev) {
        free(head);
        free(prev);
        out->failed = true;
        return;
    }
    for (size_t i = 0; i < HASH_SIZE; i++) head[i] = -1;
    size_t i = 0;
    while (i < len) {
        unsigned best_len = 0, best_dist = 0;
        if (len - i >= MIN_MATCH) {
            uint32_t h = hash3(data + i);
            int32_t candidate = head[h];
            int chain = MAX_CHAIN;
            size_t limit = omt_min_size(MAX_MATCH, len - i);
            while (candidate >= 0 && chain-- > 0 && i - (size_t)candidate <= WINDOW - 1) {
                const uint8_t *a = data + candidate, *b = data + i;
                unsigned n = 0;
                while (n < limit && a[n] == b[n]) n++;
                if (n > best_len) {
                    best_len = n;
                    best_dist = (unsigned)(i - (size_t)candidate);
                    if (n == limit) break;
                }
                int32_t next = prev[(size_t)candidate % WINDOW];
                if (next >= candidate) break;
                candidate = next;
            }
        }
        size_t advance = best_len >= MIN_MATCH ? best_len : 1;
        if (best_len >= MIN_MATCH)
            put_match(&w, best_len, best_dist);
        else
            put_literal(&w, data[i]);
        /* Index every position consumed, so later matches can reach it. */
        for (size_t k = 0; k < advance; k++, i++) {
            if (len - i >= MIN_MATCH) {
                uint32_t h = hash3(data + i);
                prev[i % WINDOW] = head[h];
                head[h] = (int32_t)i;
            }
        }
    }
    put_literal(&w, 256);
    if (w.count) put_bits(&w, 0, 8 - w.count);
    free(head);
    free(prev);
}

/* ----------------------------------------------------------------- zip */

void omt_zip_init(omt_zip *z, size_t limit) {
    memset(z, 0, sizeof(*z));
    omt_buf_init(&z->out, limit);
}

void omt_zip_free(omt_zip *z) { omt_buf_free(&z->out); }

static void le16(omt_buf *b, uint16_t v) {
    uint8_t p[2];
    omt_put_le16(p, v);
    omt_buf_append(b, p, 2);
}

static void le32(omt_buf *b, uint32_t v) {
    uint8_t p[4];
    omt_put_le32(p, v);
    omt_buf_append(b, p, 4);
}

/* 1980-01-01 00:00, the timestamp the Rust writer's defaults produced. */
#define DOS_TIME 0x0000u
#define DOS_DATE 0x0021u

void omt_zip_add(omt_zip *z, const char *name, const void *data, size_t len,
                 omt_zip_method method) {
    size_t name_len = strlen(name);
    if (z->failed || z->count == OMT_ZIP_MAX_MEMBERS || name_len >= sizeof(z->members[0].name) ||
        len > UINT32_MAX || z->out.len > UINT32_MAX) {
        z->failed = true;
        return;
    }
    omt_buf body;
    omt_buf_init(&body, len + len / 8 + 64);
    if (method == OMT_ZIP_DEFLATED)
        omt_deflate(data, len, &body);
    else
        omt_buf_append(&body, data, len);
    if (body.failed || body.len > UINT32_MAX) {
        omt_buf_free(&body);
        z->failed = true;
        return;
    }
    omt_zip_member *m = &z->members[z->count++];
    memcpy(m->name, name, name_len + 1);
    m->crc = omt_crc32(data, len);
    m->compressed = (uint32_t)body.len;
    m->size = (uint32_t)len;
    m->offset = (uint32_t)z->out.len;
    m->method = (uint16_t)method;
    le32(&z->out, 0x04034b50u);
    le16(&z->out, 20);
    le16(&z->out, 0x0800); /* UTF-8 names */
    le16(&z->out, m->method);
    le16(&z->out, DOS_TIME);
    le16(&z->out, DOS_DATE);
    le32(&z->out, m->crc);
    le32(&z->out, m->compressed);
    le32(&z->out, m->size);
    le16(&z->out, (uint16_t)name_len);
    le16(&z->out, 0);
    omt_buf_append(&z->out, name, name_len);
    omt_buf_append(&z->out, body.data ? body.data : (const uint8_t *)"", body.len);
    omt_buf_free(&body);
    if (z->out.failed) z->failed = true;
}

bool omt_zip_finish(omt_zip *z) {
    if (z->failed || z->out.len > UINT32_MAX) return false;
    uint32_t directory = (uint32_t)z->out.len;
    for (size_t i = 0; i < z->count; i++) {
        size_t name_len = strlen(z->members[i].name);
        le32(&z->out, 0x02014b50u);
        le16(&z->out, 0x0314); /* made by Unix, spec 2.0 */
        le16(&z->out, 20);
        le16(&z->out, 0x0800);
        le16(&z->out, z->members[i].method);
        le16(&z->out, DOS_TIME);
        le16(&z->out, DOS_DATE);
        le32(&z->out, z->members[i].crc);
        le32(&z->out, z->members[i].compressed);
        le32(&z->out, z->members[i].size);
        le16(&z->out, (uint16_t)name_len);
        le16(&z->out, 0);
        le16(&z->out, 0);
        le16(&z->out, 0);
        le16(&z->out, 0);
        le32(&z->out, 0100644u << 16); /* regular file, rw-r--r-- */
        le32(&z->out, z->members[i].offset);
        omt_buf_append(&z->out, z->members[i].name, name_len);
    }
    uint32_t size = (uint32_t)(z->out.len - directory);
    le32(&z->out, 0x06054b50u);
    le16(&z->out, 0);
    le16(&z->out, 0);
    le16(&z->out, (uint16_t)z->count);
    le16(&z->out, (uint16_t)z->count);
    le32(&z->out, size);
    le32(&z->out, directory);
    le16(&z->out, 0);
    return !z->out.failed;
}
