/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The portable inverse DCT, and the definition every other kernel is
 * measured against: the NEON kernel must agree with it bit for bit, and on
 * targets without a hand-written kernel this is what runs.
 *
 * The reference is SSE2. Roughly a third of its instructions are shuffles
 * that only line data up in lanes; the row pass folds them into its indexing
 * and keeps only the arithmetic. The column pass is element-wise across eight
 * lanes, so it stays in that shape, which lets the compiler keep each vector
 * in a register and vectorise the lane loops. Operation order, saturations,
 * rounding corrections, and truncations match the reference exactly;
 * tests/vectors/vmx proves that on every run.
 */
#include "vmx/internal.h"

typedef struct {
    int16_t v[8];
} i16x8;

#define LANES for (int lane = 0; lane < 8; lane++)

static OMT_ALWAYS_INLINE i16x8 adds(i16x8 a, i16x8 b) {
    i16x8 o = {{0}};
    LANES o.v[lane] = vmx_adds(a.v[lane], b.v[lane]);
    return o;
}
static OMT_ALWAYS_INLINE i16x8 subs(i16x8 a, i16x8 b) {
    i16x8 o = {{0}};
    LANES o.v[lane] = vmx_subs(a.v[lane], b.v[lane]);
    return o;
}
static OMT_ALWAYS_INLINE i16x8 mulhi_by(i16x8 a, int16_t scale) {
    i16x8 o = {{0}};
    LANES o.v[lane] = vmx_mulhi(a.v[lane], scale);
    return o;
}
static OMT_ALWAYS_INLINE i16x8 adds_constant(i16x8 a, int16_t value) {
    i16x8 o = {{0}};
    LANES o.v[lane] = vmx_adds(a.v[lane], value);
    return o;
}
static OMT_ALWAYS_INLINE i16x8 shift_right(i16x8 a) {
    i16x8 o = {{0}};
    LANES o.v[lane] = (int16_t)(a.v[lane] >> VMX_SHIFT_INV_COL);
    return o;
}
static OMT_ALWAYS_INLINE i16x8 or_correction(i16x8 a) {
    i16x8 o = {{0}};
    LANES o.v[lane] = (int16_t)(a.v[lane] | 1);
    return o;
}
static OMT_ALWAYS_INLINE void store_row(uint8_t *dst, i16x8 value) {
    LANES dst[lane] = vmx_sat8(value.v[lane]);
}

/* _mm_madd_epi16 over one output lane: two products added with wraparound. */
static OMT_ALWAYS_INLINE int32_t product(int32_t first, int16_t first_scale, int32_t second,
                                         int16_t second_scale) {
    return vmx_wrap_add32(first * first_scale, second * second_scale);
}

/* One row butterfly. The reference shuffles the row into
 * [x0,x2,x1,x3,x4,x6,x5,x7] and broadcasts each 32-bit lane, so every madd
 * sees one fixed pair against four coefficient pairs; naming the pairs removes
 * the shuffles. */
static OMT_ALWAYS_INLINE i16x8 row_pass(i16x8 in, const int16_t tab[32]) {
    int32_t x0 = in.v[0], x1 = in.v[1], x2 = in.v[2], x3 = in.v[3];
    int32_t x4 = in.v[4], x5 = in.v[5], x6 = in.v[6], x7 = in.v[7];
    i16x8 out;
    for (int index = 0; index < 4; index++) {
        int pair = index * 2;
        int32_t t0 = product(x0, tab[pair], x2, tab[pair + 1]);
        int32_t t2 = product(x4, tab[8 + pair], x6, tab[9 + pair]);
        int32_t t1 = product(x1, tab[16 + pair], x3, tab[17 + pair]);
        int32_t t3 = product(x5, tab[24 + pair], x7, tab[25 + pair]);
        int32_t even = vmx_wrap_add32(vmx_wrap_add32(t0, VMX_IRND_INV_ROW), t2);
        int32_t odd = vmx_wrap_add32(t3, t1);
        /* The reference reverses the high half before packing, which puts the
         * differences at the mirrored positions. */
        out.v[index] = vmx_sat16(vmx_wrap_add32(odd, even) >> VMX_SHIFT_INV_ROW);
        out.v[7 - index] = vmx_sat16(vmx_wrap_sub32(even, odd) >> VMX_SHIFT_INV_ROW);
    }
    return out;
}

/* The column butterfly over all eight columns at once. Register names follow
 * the reference kernel so the two can be diffed line by line. */
static OMT_ALWAYS_INLINE void column_pass(const i16x8 rows[8], i16x8 bias, uint8_t *dst,
                                          size_t stride) {
    i16x8 row0 = rows[0], row1 = rows[1], row2 = rows[2], row3 = rows[3];
    i16x8 row4 = rows[4], row5 = rows[5], row6 = rows[6], row7 = rows[7];

    i16x8 r2 = row5;
    i16x8 r3 = row3;
    i16x8 r0 = mulhi_by(row5, VMX_TG_3_16);
    i16x8 r1 = mulhi_by(r3, VMX_TG_3_16);
    i16x8 r4 = mulhi_by(row7, VMX_TG_1_16);
    r0 = adds(r0, r2);
    i16x8 r5 = mulhi_by(row1, VMX_TG_1_16);
    r1 = adds(r1, r3);
    i16x8 r7 = row6;
    r0 = adds(r0, r3);
    r2 = subs(r2, r1);
    r7 = mulhi_by(r7, VMX_TG_2_16);
    r1 = r0;
    r3 = mulhi_by(row2, VMX_TG_2_16);
    r5 = subs(r5, row7);
    r4 = adds(r4, row1);
    r0 = adds(r0, r4);
    r0 = adds_constant(r0, 1);
    r4 = subs(r4, r1);
    i16x8 r6 = r5;
    r5 = subs(r5, r2);
    r5 = adds_constant(r5, 1);
    r6 = adds(r6, r2);

    i16x8 temp7 = r0;
    r1 = r4;
    r4 = adds(r4, r5);
    r2 = mulhi_by(r4, VMX_COS_4_16);
    i16x8 temp3 = r6;
    r1 = subs(r1, r5);
    r7 = adds(r7, row2);
    r3 = subs(r3, row6);
    r6 = row0;
    r0 = mulhi_by(r1, VMX_COS_4_16);
    r5 = row4;
    r5 = adds(r5, r6);
    r6 = subs(r6, row4);
    r4 = adds(r4, r2);
    r4 = or_correction(r4);
    r0 = adds(r0, r1);
    r0 = or_correction(r0);

    r2 = r5;
    r5 = adds(r5, r7);
    r1 = r6;
    r5 = adds_constant(r5, VMX_IRND_INV_COL);
    r2 = subs(r2, r7);
    r7 = temp7;
    r6 = adds(r6, r3);
    r6 = adds_constant(r6, VMX_IRND_INV_COL);
    r7 = adds(r7, r5);
    r7 = shift_right(r7);
    r1 = subs(r1, r3);
    r1 = adds_constant(r1, VMX_IRND_INV_CORR);
    r3 = r6;
    r2 = adds_constant(r2, VMX_IRND_INV_CORR);
    r6 = adds(r6, r4);

    r7 = adds(r7, bias);
    store_row(dst, r7);
    r6 = shift_right(r6);
    r6 = adds(r6, bias);
    store_row(dst + stride, r6);

    r7 = r1;
    r1 = adds(shift_right(adds(r1, r0)), bias);
    store_row(dst + 2 * stride, r1);
    r6 = temp3;
    r7 = shift_right(subs(r7, r0));
    r5 = adds(shift_right(subs(r5, temp7)), bias);
    store_row(dst + 7 * stride, r5);
    r3 = subs(r3, r4);
    r6 = adds(r6, r2);
    r2 = subs(r2, temp3);
    r6 = shift_right(r6);
    r2 = shift_right(r2);
    r6 = adds(r6, bias);
    store_row(dst + 3 * stride, r6);

    r3 = shift_right(r3);
    r2 = adds(r2, bias);
    store_row(dst + 4 * stride, r2);
    r7 = adds(r7, bias);
    store_row(dst + 5 * stride, r7);
    r3 = adds(r3, bias);
    store_row(dst + 6 * stride, r3);
}

void vmx_idct_scalar(const int16_t block[64], const uint16_t matrix[64], uint8_t *dst,
                     size_t stride, int16_t add_value, unsigned nonzero_rows) {
    /* The portable kernel is the definition: it always runs every row. */
    (void)nonzero_rows;
    /* Dequantise. The block is already in row-major order (see
     * vmx_zigzag_natural). The reference multiplies in 16 bits, keeping the
     * wraparound, then shifts down by four. */
    i16x8 rows[8];
    for (int index = 0; index < 64; index++)
        rows[index / 8].v[index % 8] =
            (int16_t)(vmx_wrap_mul16(block[index], (int16_t)matrix[index]) >> 4);
    i16x8 transformed[8] = {
        row_pass(rows[0], vmx_tab_i_04), row_pass(rows[1], vmx_tab_i_17),
        row_pass(rows[2], vmx_tab_i_26), row_pass(rows[3], vmx_tab_i_35),
        row_pass(rows[4], vmx_tab_i_04), row_pass(rows[5], vmx_tab_i_35),
        row_pass(rows[6], vmx_tab_i_26), row_pass(rows[7], vmx_tab_i_17),
    };
    i16x8 bias;
    LANES bias.v[lane] = add_value;
    column_pass(transformed, bias, dst, stride);
}

/* The flat block the reference codec emits when a block carries only DC. */
void vmx_broadcast_dc(int16_t dc, uint8_t *dst, size_t stride, int16_t add_value) {
    int16_t level = vmx_wrap_add16((int16_t)(vmx_wrap_add16(dc, 4) >> 3), add_value);
    uint8_t byte = vmx_sat8(level);
    for (size_t row = 0; row < 8; row++) memset(dst + row * stride, byte, 8);
}
