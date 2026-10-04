/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Constants transcribed from the Open Media Transport VMX reference codec
 * (MIT, see third_party/omt/PROVENANCE.md). Only the decode-side tables are
 * carried over; the encoder's reciprocal and forward-DCT tables are dropped
 * along with the encoder itself.
 */
#include "vmx/internal.h"

/* Implementation-defined, not undefined: this assertion is the check that
 * the implementation defines it as arithmetic. */
// cppcheck-suppress shiftNegativeLHS
_Static_assert((-1 >> 1) == -1, "the kernels need an arithmetic right shift");

const uint16_t vmx_quantization_matrix[64] = {
    16, 16, 19, 22, 26, 27, 29, 34, 16, 16, 22, 24, 27, 29, 34, 37, 19, 22, 26, 27, 29, 34,
    34, 38, 22, 22, 26, 27, 29, 34, 37, 40, 22, 26, 27, 29, 32, 35, 40, 48, 26, 27, 29, 32,
    35, 40, 48, 58, 26, 27, 29, 34, 38, 46, 56, 69, 27, 29, 35, 38, 46, 56, 69, 83,
};

const int32_t vmx_quality[VMX_QUALITY_COUNT] = {1,  2,  3,  4,  5,  6,  7,  8,  10, 12, 14, 16, 18,
                                                20, 22, 24, 28, 32, 36, 40, 44, 48, 52, 56, 64};

/* Y, R, GU, GV, B fixed-point YUV-to-RGB coefficients. */
const int16_t vmx_yuv_rgb_709[5] = {19077, 29372, 3494, 8731, 17305};
const int16_t vmx_yuv_rgb_601[5] = {19077, 26149, 6419, 13320, 16525};

void vmx_decode_matrix(size_t index, uint16_t out[64]) {
    int32_t scale = vmx_quality[index < VMX_QUALITY_COUNT ? index : VMX_QUALITY_COUNT - 1];
    /* The DC term is never scaled. The largest scaled term is 83 * 64, so the
     * reference codec's unsigned-short store never truncates. */
    for (size_t i = 0; i < 64; i++)
        out[i] = i == 0 ? vmx_quantization_matrix[i]
                        : (uint16_t)((int32_t)vmx_quantization_matrix[i] * scale);
}

/* Mirrors VMX_SetQualityInternal: the first preset whose step covers the
 * requested loss wins, and an out-of-range request keeps preset zero. */
size_t vmx_quality_index(int32_t quality) {
    for (size_t i = 0; i < VMX_QUALITY_COUNT; i++)
        if (vmx_quality[i] >= 100 - quality) return i;
    return 0;
}
