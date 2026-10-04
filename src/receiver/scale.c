/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "receiver/scale.h"

#include <stdlib.h>
#include <string.h>

bool omt_placement_fit(size_t sw, size_t sh, size_t mw, size_t mh, omt_placement *out) {
    if (sw == 0 || sh == 0 || mw == 0 || mh == 0) return false;
    /* Cross-multiplied, so the limiting axis never depends on a rounded ratio. */
    unsigned __int128 source_ratio = (unsigned __int128)sw * mh;
    unsigned __int128 mode_ratio = (unsigned __int128)mw * sh;
    size_t width, height;
    if (source_ratio <= mode_ratio) {
        unsigned __int128 w = source_ratio / sh;
        width = w > mw ? mw : (size_t)w;
        height = mh;
    } else {
        unsigned __int128 h = mode_ratio / sw;
        width = mw;
        height = h > mh ? mh : (size_t)h;
    }
    if (width == 0) width = 1;
    if (height == 0) height = 1;
    out->x = (mw - width) / 2;
    out->y = (mh - height) / 2;
    out->width = width;
    out->height = height;
    return true;
}

bool omt_placement_fills(const omt_placement *p, size_t mw, size_t mh) {
    return p->x == 0 && p->y == 0 && p->width == mw && p->height == mh;
}

size_t omt_scale_sample(size_t index, size_t destination, size_t source) {
    unsigned __int128 numerator = ((unsigned __int128)2 * index + 1) * source;
    unsigned __int128 coordinate = numerator / ((unsigned __int128)2 * destination);
    size_t c = coordinate > SIZE_MAX ? 0 : (size_t)coordinate;
    return c < source - 1 ? c : source - 1;
}

void omt_scaler_free(omt_scaler *s) {
    free(s->rows);
    free(s->columns);
    free(s->row);
    memset(s, 0, sizeof(*s));
}

bool omt_scaler_init(omt_scaler *s, size_t sw, size_t sh, size_t source_stride, omt_placement p,
                     omt_err *err) {
    memset(s, 0, sizeof(*s));
    size_t source_row_bytes, total, row_bytes;
    if (!omt_mul(sw, (size_t)4, &source_row_bytes)) {
        omt_err_set(err, "Video frame is too wide to scale");
        return false;
    }
    if (sw == 0 || sh == 0 || p.width == 0 || p.height == 0 || source_stride < source_row_bytes ||
        !omt_mul(sh, source_stride, &total)) {
        omt_err_set(err, "Video frame geometry cannot be scaled");
        return false;
    }
    if (!omt_mul(p.width, (size_t)4, &row_bytes)) {
        omt_err_set(err, "Video mode is too wide to scale into");
        return false;
    }
    s->rows = calloc(p.height, sizeof(size_t));
    s->columns = calloc(p.width, sizeof(size_t));
    s->row = calloc(row_bytes, 1);
    if (!s->rows || !s->columns || !s->row) {
        omt_scaler_free(s);
        omt_err_set(err, "Unable to reserve the scaler tables");
        return false;
    }
    for (size_t y = 0; y < p.height; y++)
        s->rows[y] = omt_scale_sample(y, p.height, sh) * source_stride;
    for (size_t x = 0; x < p.width; x++) s->columns[x] = omt_scale_sample(x, p.width, sw);
    s->placement = p;
    s->source_stride = source_stride;
    s->source_row_bytes = source_row_bytes;
    return true;
}

bool omt_scaler_render(omt_scaler *s, const uint8_t *source, size_t source_len, uint8_t *dst,
                       size_t dst_len, size_t dst_stride, omt_err *err) {
    const omt_placement *p = &s->placement;
    size_t row_bytes = p->width * 4;
    size_t left = p->x * 4;
    size_t first, last_offset, required;
    /* The whole placed row has to end inside its own destination row. */
    if (dst_stride < left + row_bytes || !omt_mul(p->y, dst_stride, &first) ||
        !omt_add(first, left, &first) || !omt_mul(p->height - 1, dst_stride, &last_offset) ||
        !omt_add(last_offset, row_bytes, &required) || !omt_add(required, first, &required) ||
        required > dst_len) {
        omt_err_set(err, "Scaler buffer is too small");
        return false;
    }
    size_t gathered = SIZE_MAX;
    for (size_t y = 0; y < p->height; y++) {
        size_t src_offset = s->rows[y];
        if (gathered != src_offset) {
            if (src_offset > source_len || source_len - src_offset < s->source_row_bytes) {
                omt_err_set(err, "Scaler buffer is too small");
                return false;
            }
            const uint8_t *src_row = source + src_offset;
            for (size_t x = 0; x < p->width; x++)
                memcpy(s->row + 4 * x, src_row + 4 * s->columns[x], 4);
            gathered = src_offset;
        }
        /* One contiguous store into the write-combined mapping per row. */
        memcpy(dst + first + y * dst_stride, s->row, row_bytes);
    }
    return true;
}
