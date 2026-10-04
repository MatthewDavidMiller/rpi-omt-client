/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * AArch64 NEON inverse DCT: the kernel the appliance runs. NEON is mandatory
 * in ARMv8-A, so the architecture selects it with no runtime detection. It is
 * a lane-for-lane translation of idct_scalar.c and must agree with it bit for
 * bit; tests/c/test_vmx.c and the committed conformance vectors check that.
 */
#if defined(__aarch64__)
#include <arm_neon.h>

#include "vmx/internal.h"

/* _mm_madd_epi16: multiply 16-bit lanes and add adjacent 32-bit products. */
static inline int32x4_t madd(int16x8_t a, int16x8_t b) {
    int32x4_t low = vmull_s16(vget_low_s16(a), vget_low_s16(b));
    int32x4_t high = vmull_high_s16(a, b);
    return vpaddq_s32(low, high);
}

/* _mm_mulhi_epi16 against a broadcast constant. */
static inline int16x8_t mulhi_by(int16x8_t a, int16_t scale) {
    int16x8_t broadcast = vdupq_n_s16(scale);
    int32x4_t low = vmull_s16(vget_low_s16(a), vget_low_s16(broadcast));
    int32x4_t high = vmull_high_s16(a, broadcast);
    return vcombine_s16(vshrn_n_s32(low, 16), vshrn_n_s32(high, 16));
}

/* One row butterfly. Unzipping into even and odd lanes reaches the
 * reference's four broadcast pairs in two instructions. */
static inline int16x8_t row_pass(int16x8_t input, const int16_t tab[32]) {
    int32x4_t even = vreinterpretq_s32_s16(vuzp1q_s16(input, input));
    int32x4_t odd = vreinterpretq_s32_s16(vuzp2q_s16(input, input));
    int16x8_t pair0 = vreinterpretq_s16_s32(vdupq_laneq_s32(even, 0));
    int16x8_t pair2 = vreinterpretq_s16_s32(vdupq_laneq_s32(even, 1));
    int16x8_t pair1 = vreinterpretq_s16_s32(vdupq_laneq_s32(odd, 0));
    int16x8_t pair3 = vreinterpretq_s16_s32(vdupq_laneq_s32(odd, 1));

    int32x4_t t0 = madd(pair0, vld1q_s16(tab));
    int32x4_t t2 = madd(pair2, vld1q_s16(tab + 8));
    int32x4_t t1 = madd(pair1, vld1q_s16(tab + 16));
    int32x4_t t3 = madd(pair3, vld1q_s16(tab + 24));

    int32x4_t even_sum = vaddq_s32(vaddq_s32(t0, vdupq_n_s32(VMX_IRND_INV_ROW)), t2);
    int32x4_t odd_sum = vaddq_s32(t3, t1);
    int32x4_t sum = vshrq_n_s32(vaddq_s32(odd_sum, even_sum), VMX_SHIFT_INV_ROW);
    int32x4_t difference = vshrq_n_s32(vsubq_s32(even_sum, odd_sum), VMX_SHIFT_INV_ROW);
    /* The reference reverses the high half before packing. */
    int32x4_t reversed = vrev64q_s32(difference);
    reversed = vextq_s32(reversed, reversed, 2);
    return vcombine_s16(vqmovn_s32(sum), vqmovn_s32(reversed));
}

static inline void store_row(uint8_t *dst, int16x8_t value) { vst1_u8(dst, vqmovun_s16(value)); }

void vmx_idct_neon(const int16_t block[64], const uint16_t matrix[64], uint8_t *dst, size_t stride,
                   int16_t add_value) {
    /* The inverse zig-zag is a permutation, so it stays scalar; the
     * dequantising multiply that follows is eight lanes at a time. */
    int16_t permuted[64];
    for (int i = 0; i < 64; i++) permuted[i] = block[vmx_zigzag_inverse[i]];
    int16x8_t rows[8];
    for (int i = 0; i < 8; i++)
        rows[i] = vshrq_n_s16(vmulq_s16(vld1q_s16(permuted + 8 * i),
                                        vreinterpretq_s16_u16(vld1q_u16(matrix + 8 * i))),
                              4);

    int16x8_t row0 = row_pass(rows[0], vmx_tab_i_04);
    int16x8_t row1 = row_pass(rows[1], vmx_tab_i_17);
    int16x8_t row2 = row_pass(rows[2], vmx_tab_i_26);
    int16x8_t row3 = row_pass(rows[3], vmx_tab_i_35);
    int16x8_t row4 = row_pass(rows[4], vmx_tab_i_04);
    int16x8_t row5 = row_pass(rows[5], vmx_tab_i_35);
    int16x8_t row6 = row_pass(rows[6], vmx_tab_i_26);
    int16x8_t row7 = row_pass(rows[7], vmx_tab_i_17);

    int16x8_t one = vdupq_n_s16(1);
    int16x8_t round_col = vdupq_n_s16(VMX_IRND_INV_COL);
    int16x8_t round_corr = vdupq_n_s16(VMX_IRND_INV_CORR);
    int16x8_t bias = vdupq_n_s16(add_value);

    int16x8_t r2 = row5;
    int16x8_t r3 = row3;
    int16x8_t r0 = mulhi_by(row5, VMX_TG_3_16);
    int16x8_t r1 = mulhi_by(r3, VMX_TG_3_16);
    int16x8_t r4 = mulhi_by(row7, VMX_TG_1_16);
    r0 = vqaddq_s16(r0, r2);
    int16x8_t r5 = mulhi_by(row1, VMX_TG_1_16);
    r1 = vqaddq_s16(r1, r3);
    int16x8_t r7 = row6;
    r0 = vqaddq_s16(r0, r3);
    r2 = vqsubq_s16(r2, r1);
    r7 = mulhi_by(r7, VMX_TG_2_16);
    r1 = r0;
    r3 = mulhi_by(row2, VMX_TG_2_16);
    r5 = vqsubq_s16(r5, row7);
    r4 = vqaddq_s16(r4, row1);
    r0 = vqaddq_s16(r0, r4);
    r0 = vqaddq_s16(r0, one);
    r4 = vqsubq_s16(r4, r1);
    int16x8_t r6 = r5;
    r5 = vqsubq_s16(r5, r2);
    r5 = vqaddq_s16(r5, one);
    r6 = vqaddq_s16(r6, r2);

    int16x8_t temp7 = r0;
    r1 = r4;
    r4 = vqaddq_s16(r4, r5);
    r2 = mulhi_by(r4, VMX_COS_4_16);
    int16x8_t temp3 = r6;
    r1 = vqsubq_s16(r1, r5);
    r7 = vqaddq_s16(r7, row2);
    r3 = vqsubq_s16(r3, row6);
    r6 = row0;
    r0 = mulhi_by(r1, VMX_COS_4_16);
    r5 = row4;
    r5 = vqaddq_s16(r5, r6);
    r6 = vqsubq_s16(r6, row4);
    r4 = vqaddq_s16(r4, r2);
    r4 = vorrq_s16(r4, one);
    r0 = vqaddq_s16(r0, r1);
    r0 = vorrq_s16(r0, one);

    r2 = r5;
    r5 = vqaddq_s16(r5, r7);
    r1 = r6;
    r5 = vqaddq_s16(r5, round_col);
    r2 = vqsubq_s16(r2, r7);
    r7 = temp7;
    r6 = vqaddq_s16(r6, r3);
    r6 = vqaddq_s16(r6, round_col);
    r7 = vqaddq_s16(r7, r5);
    r7 = vshrq_n_s16(r7, VMX_SHIFT_INV_COL);
    r1 = vqsubq_s16(r1, r3);
    r1 = vqaddq_s16(r1, round_corr);
    r3 = r6;
    r2 = vqaddq_s16(r2, round_corr);
    r6 = vqaddq_s16(r6, r4);

    r7 = vqaddq_s16(r7, bias);
    store_row(dst, r7);
    r6 = vshrq_n_s16(r6, VMX_SHIFT_INV_COL);
    r6 = vqaddq_s16(r6, bias);
    store_row(dst + stride, r6);

    r7 = r1;
    r1 = vqaddq_s16(vshrq_n_s16(vqaddq_s16(r1, r0), VMX_SHIFT_INV_COL), bias);
    store_row(dst + 2 * stride, r1);
    r6 = temp3;
    r7 = vshrq_n_s16(vqsubq_s16(r7, r0), VMX_SHIFT_INV_COL);
    r5 = vqaddq_s16(vshrq_n_s16(vqsubq_s16(r5, temp7), VMX_SHIFT_INV_COL), bias);
    store_row(dst + 7 * stride, r5);
    r3 = vqsubq_s16(r3, r4);
    r6 = vqaddq_s16(r6, r2);
    r2 = vqsubq_s16(r2, temp3);
    r6 = vshrq_n_s16(r6, VMX_SHIFT_INV_COL);
    r2 = vshrq_n_s16(r2, VMX_SHIFT_INV_COL);
    r6 = vqaddq_s16(r6, bias);
    store_row(dst + 3 * stride, r6);

    r3 = vshrq_n_s16(r3, VMX_SHIFT_INV_COL);
    r2 = vqaddq_s16(r2, bias);
    store_row(dst + 4 * stride, r2);
    r7 = vqaddq_s16(r7, bias);
    store_row(dst + 5 * stride, r7);
    r3 = vqaddq_s16(r3, bias);
    store_row(dst + 6 * stride, r3);
}
#else
/* ISO C forbids an empty translation unit. */
typedef int vmx_idct_neon_unused;
#endif
