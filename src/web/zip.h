/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * An in-memory ZIP writer for the support bundle: Stored and Deflate members,
 * no ZIP64, no encryption. The Deflate encoder is a greedy LZ77 over a 32 KiB
 * window with fixed Huffman codes -- encoding only, so the appliance never
 * parses an attacker's compressed stream.
 */
#ifndef OMT_WEB_ZIP_H
#define OMT_WEB_ZIP_H

#include "common/base.h"
#include "common/buf.h"

typedef enum { OMT_ZIP_STORED = 0, OMT_ZIP_DEFLATED = 8 } omt_zip_method;

#define OMT_ZIP_MAX_MEMBERS 64

typedef struct {
    char name[128];
    uint32_t crc;
    uint32_t compressed;
    uint32_t size;
    uint32_t offset;
    uint16_t method;
} omt_zip_member;

typedef struct {
    omt_buf out;
    omt_zip_member members[OMT_ZIP_MAX_MEMBERS];
    size_t count;
    bool failed;
} omt_zip;

void omt_zip_init(omt_zip *z, size_t limit);
void omt_zip_add(omt_zip *z, const char *name, const void *data, size_t len, omt_zip_method method);
/* Writes the central directory. False if any member or the archive failed;
 * on success the archive bytes are in z->out. */
OMT_NODISCARD bool omt_zip_finish(omt_zip *z);
void omt_zip_free(omt_zip *z);

uint32_t omt_crc32(const void *data, size_t len);
/* Raw Deflate (RFC 1951) of `data`, appended to `out`. */
void omt_deflate(const uint8_t *data, size_t len, omt_buf *out);

#endif
