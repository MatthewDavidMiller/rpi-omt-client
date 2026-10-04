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
    memset(s, 0, sizeof(*s));
}

bool omt_scaler_init(omt_scaler *s, size_t sw, size_t sh, omt_placement p, omt_err *err) {
    memset(s, 0, sizeof(*s));
    if (sw == 0 || sh == 0 || sw > UINT32_MAX || sh > UINT32_MAX || p.width == 0 || p.height == 0) {
        omt_err_set(err, "Video frame geometry cannot be scaled");
        return false;
    }
    s->rows = calloc(p.height, sizeof(uint32_t));
    s->columns = calloc(p.width, sizeof(uint32_t));
    if (!s->rows || !s->columns) {
        omt_scaler_free(s);
        omt_err_set(err, "Unable to reserve the scaler tables");
        return false;
    }
    for (size_t y = 0; y < p.height; y++) s->rows[y] = (uint32_t)omt_scale_sample(y, p.height, sh);
    for (size_t x = 0; x < p.width; x++) s->columns[x] = (uint32_t)omt_scale_sample(x, p.width, sw);
    s->placement = p;
    return true;
}
