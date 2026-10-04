/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Portable ports of the reference codec's planar output stages. Both kernels
 * are pixel-independent, so writing them per pixel reproduces the vector
 * results exactly while dropping the reference's aligned staging buffer.
 *
 * This is also the definition the AArch64 kernel is checked against, so it
 * stays a plain transcription of the reference arithmetic rather than
 * something tuned. Rounding saturates to bytes as _mm_packus_epi16 does.
 */
#include "vmx/internal.h"

/* VMX_PlanarToUYVY for one row: each group of two pixels is U, Y, V, Y. */
void vmx_uyvy_row(const uint8_t *luma, const uint8_t *blue, const uint8_t *red, size_t width,
                  uint8_t *out) {
    for (size_t pair = 0; pair < width / 2; pair++) {
        out[4 * pair] = blue[pair];
        out[4 * pair + 1] = luma[2 * pair];
        out[4 * pair + 2] = red[pair];
        out[4 * pair + 3] = luma[2 * pair + 1];
    }
}

static OMT_ALWAYS_INLINE uint8_t round_channel(int16_t value) {
    return vmx_sat8((int16_t)(vmx_adds(value, 8) >> 4));
}

/* VMX_YUV4224ToBGRA for one row. VMX1 carries no alpha, and the scanout reads
 * the buffer as XRGB8888, so the fourth byte is a constant. */
void vmx_bgra_row_scalar(const uint8_t *luma, const uint8_t *blue, const uint8_t *red, size_t width,
                         uint8_t *out, const int16_t c[5]) {
    for (size_t pair = 0; pair < width / 2; pair++) {
        int16_t blue_difference = (int16_t)(blue[pair] - 128);
        int16_t red_difference = (int16_t)(red[pair] - 128);
        int16_t chroma_red = vmx_mulhi((int16_t)(red_difference * 64), c[1]);
        int16_t chroma_blue = vmx_mulhi((int16_t)(blue_difference * 128), c[4]);
        /* The two green terms stay separate: the reference subtracts them one
         * after the other, and folding them would change the result wherever
         * either subtraction saturates. */
        int16_t green_from_blue = vmx_mulhi((int16_t)(blue_difference * 64), c[2]);
        int16_t green_from_red = vmx_mulhi((int16_t)(red_difference * 64), c[3]);
        for (size_t k = 0; k < 2; k++) {
            uint8_t y = luma[2 * pair + k];
            int16_t floored = (int16_t)(y > 16 ? y - 16 : 0);
            int16_t scaled = vmx_mulhi((int16_t)(floored * 64), c[0]);
            uint8_t *pixel = out + 8 * pair + 4 * k;
            pixel[0] = round_channel(vmx_adds(chroma_blue, scaled));
            pixel[1] = round_channel(vmx_subs(vmx_subs(scaled, green_from_blue), green_from_red));
            pixel[2] = round_channel(vmx_adds(chroma_red, scaled));
            pixel[3] = 0xFF;
        }
    }
}
