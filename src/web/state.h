/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The operator's saved choices: the OMT target and the video ceiling
 * override. Each lives in a small strict-JSON file that is read
 * with a size cap and replaced atomically.
 */
#ifndef OMT_WEB_STATE_H
#define OMT_WEB_STATE_H

#include "common/base.h"
#include "common/buf.h"
#include "common/err.h"
#include "protocol/omt.h"

typedef struct {
    bool direct;
    char value[OMT_TARGET_MAX_BYTES + 1];
} omt_source_target;

/* 1 with a target, 0 when none is saved, -1 with `err` when it is invalid. */
int omt_read_source(const char *path, omt_source_target *out, omt_err *err);
/* Saves, or removes the file when `target` is NULL. */
OMT_NODISCARD bool omt_save_source(const char *path, const omt_source_target *target, omt_err *err);

/* Validates WIDTHxHEIGHT@FPS[,...] with the Web form's rules; the message
 * names the offending part. */
OMT_NODISCARD bool omt_parse_video_ceiling(const char *value, omt_err *err);
void omt_describe_video_ceiling(const char *value, omt_buf *out);
/* 1 with an override in `out`, 0 when none is saved, -1 on an invalid file. */
int omt_read_video_ceiling(const char *path, omt_buf *out, omt_err *err);
OMT_NODISCARD bool omt_effective_video_ceiling(const char *path, const char *board_default,
                                               omt_buf *out, omt_err *err);
OMT_NODISCARD bool omt_save_video_ceiling(const char *path, const char *ceiling, omt_err *err);
uint64_t omt_pixel_rate(const char *value);

#endif
