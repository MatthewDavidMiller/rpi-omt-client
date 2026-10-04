/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Aspect-preserving resample for displays whose mode list carries no entry at
 * the incoming video's size. The picture is fitted into the mode and centred;
 * the bars are black. The filter is nearest-neighbour with pixel-centre
 * sampling -- the only one the Pi 4's decode budget leaves room for.
 *
 * This file only decides where each destination pixel comes from. The
 * resample itself runs inside the VMX decode (vmx_decode_bgrx_placed), on
 * every worker, straight into the scanout buffer.
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
    uint32_t *rows;    /* source row of each destination row, non-decreasing */
    uint32_t *columns; /* source pixel of each destination column */
} omt_scaler;

OMT_NODISCARD bool omt_scaler_init(omt_scaler *s, size_t src_w, size_t src_h,
                                   omt_placement placement, omt_err *err);
void omt_scaler_free(omt_scaler *s);
size_t omt_scale_sample(size_t index, size_t destination, size_t source);

#endif
