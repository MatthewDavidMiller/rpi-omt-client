/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * AArch64 NEON YUV 4:2:2 to BGRX: the output stage the appliance runs. A
 * lane-for-lane translation of convert_scalar.c that must agree with it bit
 * for bit. Sixteen pixels are converted per step, the width at which the four
 * output byte lanes can be handed to vst4q_u8: interleaving B, G, R and the
 * constant X in the store unit is the point of the kernel.
 */
#if defined(__aarch64__)
#include <arm_neon.h>

#include "vmx/internal.h"

#define STEP 16u

/* (a * b) >> 16 per lane: a full 32-bit product narrowed back, not NEON's
 * doubling vqdmulhq_s16. */
static inline int16x8_t mulhi(int16x8_t a, int16x8_t b) {
    int32x4_t low = vmulq_s32(vmovl_s16(vget_low_s16(a)), vmovl_s16(vget_low_s16(b)));
    int32x4_t high = vmulq_s32(vmovl_high_s16(a), vmovl_high_s16(b));
    return vcombine_s16(vshrn_n_s32(low, 16), vshrn_n_s32(high, 16));
}

/* (value + 8) >> 4 saturated into bytes. */
static inline uint8x8_t round_channel(int16x8_t value) {
    return vqmovun_s16(vshrq_n_s16(vqaddq_s16(value, vdupq_n_s16(8)), 4));
}

static inline int16x8_t widen(uint8x8_t v) { return vreinterpretq_s16_u16(vmovl_u8(v)); }

void vmx_bgra_row_neon(const uint8_t *luma, const uint8_t *blue, const uint8_t *red, size_t width,
                       uint8_t *out, const int16_t c[5]) {
    int16x8_t luma_coefficient = vdupq_n_s16(c[0]);
    int16x8_t red_coefficient = vdupq_n_s16(c[1]);
    int16x8_t green_blue_coefficient = vdupq_n_s16(c[2]);
    int16x8_t green_red_coefficient = vdupq_n_s16(c[3]);
    int16x8_t blue_coefficient = vdupq_n_s16(c[4]);
    int16x8_t bias = vdupq_n_s16(128);
    uint8x16_t opaque = vdupq_n_u8(0xFF);

    size_t steps = width / STEP;
    for (size_t step = 0; step < steps; step++) {
        size_t pixel = step * STEP;
        size_t pair = pixel / 2;
        uint8x16_t y = vqsubq_u8(vld1q_u8(luma + pixel), vdupq_n_u8(16));
        int16x8_t scaled_low = mulhi(vshlq_n_s16(widen(vget_low_u8(y)), 6), luma_coefficient);
        int16x8_t scaled_high = mulhi(vshlq_n_s16(widen(vget_high_u8(y)), 6), luma_coefficient);

        int16x8_t blue_difference = vsubq_s16(widen(vld1_u8(blue + pair)), bias);
        int16x8_t red_difference = vsubq_s16(widen(vld1_u8(red + pair)), bias);
        int16x8_t chroma_red = mulhi(vshlq_n_s16(red_difference, 6), red_coefficient);
        int16x8_t chroma_blue = mulhi(vshlq_n_s16(blue_difference, 7), blue_coefficient);
        int16x8_t green_from_blue = mulhi(vshlq_n_s16(blue_difference, 6), green_blue_coefficient);
        int16x8_t green_from_red = mulhi(vshlq_n_s16(red_difference, 6), green_red_coefficient);

        /* Zipping a vector with itself duplicates each chroma lane, turning
         * eight pairs into sixteen pixels. */
        uint8x16x4_t pixels;
        pixels.val[0] = vcombine_u8(
            round_channel(vqaddq_s16(vzip1q_s16(chroma_blue, chroma_blue), scaled_low)),
            round_channel(vqaddq_s16(vzip2q_s16(chroma_blue, chroma_blue), scaled_high)));
        pixels.val[1] = vcombine_u8(
            round_channel(
                vqsubq_s16(vqsubq_s16(scaled_low, vzip1q_s16(green_from_blue, green_from_blue)),
                           vzip1q_s16(green_from_red, green_from_red))),
            round_channel(
                vqsubq_s16(vqsubq_s16(scaled_high, vzip2q_s16(green_from_blue, green_from_blue)),
                           vzip2q_s16(green_from_red, green_from_red))));
        pixels.val[2] =
            vcombine_u8(round_channel(vqaddq_s16(vzip1q_s16(chroma_red, chroma_red), scaled_low)),
                        round_channel(vqaddq_s16(vzip2q_s16(chroma_red, chroma_red), scaled_high)));
        pixels.val[3] = opaque;
        vst4q_u8(out + pixel * 4, pixels);
    }
    /* The tail, at most fifteen pixels, finishes on the portable kernel: one
     * definition of the arithmetic is worth more than a masked vector. */
    size_t done = steps * STEP;
    if (done < width)
        vmx_bgra_row_scalar(luma + done, blue + done / 2, red + done / 2, width - done,
                            out + done * 4, c);
}
#else
typedef int vmx_convert_neon_unused;
#endif
