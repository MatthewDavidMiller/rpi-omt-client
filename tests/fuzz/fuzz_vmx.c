/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The VMX decoder on attacker-chosen frames. The first byte picks one of a few
 * geometries so the slice-count check is passed often enough for the entropy
 * decoder to be reached.
 */
#include <stdlib.h>

#include "vmx/vmx.h"

int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size);

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
    }
    free(out);
    vmx_decoder_free(d);
    return 0;
}
