/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Aspect-preserving resample for displays whose mode list carries no entry at
 * the incoming video's size. The picture is fitted into the mode and centred;
 * the bars are black. The filter is nearest-neighbour with pixel-centre
 * sampling -- the only one the Pi 4's decode budget leaves room for.
 */
#ifndef OMT_RECEIVER_SCALE_H
#define OMT_RECEIVER_SCALE_H

#include "common/base.h"
#include "common/err.h"

typedef struct {
    size_t x, y, width, height;
} omt_placement;

/* Fits `source` inside `mode` without changing its aspect ratio, centred.
 * False for a degenerate source or mode. */
bool omt_placement_fit(size_t src_w, size_t src_h, size_t mode_w, size_t mode_h,
                       omt_placement *out);
bool omt_placement_fills(const omt_placement *p, size_t mode_w, size_t mode_h);

typedef struct {
    omt_placement placement;
    size_t source_stride;
    size_t source_row_bytes;
    size_t *rows;    /* byte offset of each destination row's source row */
    size_t *columns; /* source pixel index of each destination column */
    uint8_t *row;    /* one gathered destination row in cached memory */
} omt_scaler;

OMT_NODISCARD bool omt_scaler_init(omt_scaler *s, size_t src_w, size_t src_h, size_t source_stride,
                                   omt_placement placement, omt_err *err);
void omt_scaler_free(omt_scaler *s);
/* Resamples one decoded BGRX frame into the (write-combined) destination. */
OMT_NODISCARD bool omt_scaler_render(omt_scaler *s, const uint8_t *source, size_t source_len,
                                     uint8_t *dst, size_t dst_len, size_t dst_stride, omt_err *err);
size_t omt_scale_sample(size_t index, size_t destination, size_t source);

#endif
