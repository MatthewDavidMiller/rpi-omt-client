/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The VMX decoder on attacker-chosen frames. The first byte picks one of a few
 * geometries so the slice-count check is passed often enough for the entropy
 * decoder to be reached. Each frame is also decoded through a placement,
 * shrunk or enlarged into a letterboxed destination as a scaled display mode
 * would have it.
 */
#include <stdlib.h>

#include "vmx/vmx.h"

int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size);

static uint32_t *samples(size_t destination, size_t source) {
    uint32_t *t = malloc(destination * sizeof(uint32_t));
    for (size_t i = 0; t && i < destination; i++) {
        size_t c = ((2 * i + 1) * source) / (2 * destination);
        t[i] = (uint32_t)(c < source ? c : source - 1);
    }
    return t;
}

static void placed(vmx_decoder *d, const size_t shape[2], bool enlarge) {
    size_t width = enlarge ? shape[0] * 3 / 2 : shape[0] / 2 + 1;
    size_t height = enlarge ? shape[1] * 3 / 2 : shape[1] / 2 + 1;
    size_t stride = (width + 8) * 4, len = stride * (height + 3);
    uint32_t *columns = samples(width, shape[0]), *rows = samples(height, shape[1]);
    uint8_t *out = malloc(len);
    if (columns && rows && out) {
        vmx_placement p = {4, 2, width, height, columns, rows};
        (void)vmx_decode_bgrx_placed(d, out, len, stride, &p);
    }
    free(out);
    free(columns);
    free(rows);
}

int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) {
    static const size_t shapes[][2] = {{16, 16}, {64, 32}, {128, 80}, {320, 176}};
    if (size < 1) return 0;
    const size_t *shape = shapes[data[0] % 4];
    vmx_decoder *d = NULL;
    if (vmx_decoder_new(shape[0], shape[1], VMX_BT709, 1 + (data[0] >> 6) % 3, &d) != VMX_OK)
        return 0;
    size_t len = shape[0] * shape[1] * 4;
    uint8_t *out = malloc(len);
    if (out && vmx_decoder_load(d, data + 1, size - 1) == VMX_OK) {
        (void)vmx_decode_bgrx(d, out, len, shape[0] * 4);
        if (vmx_decoder_load(d, data + 1, size - 1) == VMX_OK)
            (void)vmx_decode_uyvy(d, out, len, shape[0] * 2);
        if (vmx_decoder_load(d, data + 1, size - 1) == VMX_OK)
            placed(d, shape, (data[0] & 0x10) != 0);
    }
    free(out);
    vmx_decoder_free(d);
    return 0;
}
