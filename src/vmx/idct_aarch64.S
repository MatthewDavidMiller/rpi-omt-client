/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * AArch64 inverse DCT: the kernel the appliance runs.
 *
 * void vmx_idct_neon(const int16_t block[64], const uint16_t matrix[64],
 *                    uint8_t *dst, size_t stride, int16_t add_value);
 *
 * It computes exactly what idct_scalar.c computes and must agree with it bit
 * for bit; tests/c/test_vmx.c and the committed conformance vectors check
 * that under emulation on every gate run. The block arrives in row-major
 * order (plane.c stores it that way), so there is no zig-zag gather.
 *
 * Row pass. The scalar row butterfly is four two-term dot products per half,
 * summed with 32-bit wraparound. Wraparound addition is associative, so each
 * half is one smull and three smlal against a coefficient vector, multiplied
 * by one lane of the row. ld2 de-interleaves each 32-entry table into those
 * vectors as it loads them. Rows sharing a table (0/4, 1/7, 2/6, 3/5) are
 * done as a pair so the four accumulation chains overlap. The differences
 * land in lanes 4..7 in reverse column order; the column pass works lane by
 * lane, so that order is carried through it and undone by one tbl per pair of
 * output rows.
 *
 * Column pass. A line-for-line transcription of idct_scalar.c's column_pass,
 * with its register names in the comments. mulhi by an even constant c is
 * sqdmulh by c/2: the doubling restores the product exactly, and it cannot
 * saturate because c/2 is never -32768. COS_4_16 is odd, so that multiply
 * keeps the full 32-bit products and takes their high halves with uzp2.
 *
 * Only v0-v7 and v16-v31 are used, so no callee-saved register is touched.
 */
#if defined(__aarch64__)
#include "vmx/aarch64.inc"

/* One row pair through the row pass, in place: rows v\a and v\b, the table
 * de-interleaved into v16-v19, the rounding constant in v28. */
.macro rows a, b
        smull   v20.4s, v16.4h, v\a\().h[0]
        smull   v22.4s, v16.4h, v\b\().h[0]
        smull   v21.4s, v18.4h, v\a\().h[1]
        smull   v23.4s, v18.4h, v\b\().h[1]
        smlal   v20.4s, v17.4h, v\a\().h[2]
        smlal   v22.4s, v17.4h, v\b\().h[2]
        smlal   v21.4s, v19.4h, v\a\().h[3]
        smlal   v23.4s, v19.4h, v\b\().h[3]
        smlal2  v20.4s, v16.8h, v\a\().h[4]
        smlal2  v22.4s, v16.8h, v\b\().h[4]
        smlal2  v21.4s, v18.8h, v\a\().h[5]
        smlal2  v23.4s, v18.8h, v\b\().h[5]
        smlal2  v20.4s, v17.8h, v\a\().h[6]
        smlal2  v22.4s, v17.8h, v\b\().h[6]
        smlal2  v21.4s, v19.8h, v\a\().h[7]
        smlal2  v23.4s, v19.8h, v\b\().h[7]
        add     v20.4s, v20.4s, v28.4s          // even += IRND_INV_ROW
        add     v22.4s, v22.4s, v28.4s
        add     v24.4s, v20.4s, v21.4s          // even + odd
        sub     v25.4s, v20.4s, v21.4s          // even - odd
        add     v26.4s, v22.4s, v23.4s
        sub     v27.4s, v22.4s, v23.4s
        sqshrn  v\a\().4h, v24.4s, #11          // SHIFT_INV_ROW, saturated
        sqshrn  v\b\().4h, v26.4s, #11
        sqshrn2 v\a\().8h, v25.4s, #11
        sqshrn2 v\b\().8h, v27.4s, #11
.endm

/* d = (s * COS_4_16) >> 16, the exact high half; clobbers v1 and v3. */
.macro mulhi_cos4 d, s
        smull   v1.4s, v\s\().4h, v27.4h
        smull2  v3.4s, v\s\().8h, v27.8h
        uzp2    v\d\().8h, v1.8h, v3.8h
.endm

VMX_FUNCTION(vmx_idct_neon)
        adrp    x9, .Lidct_data
        add     x9, x9, :lo12:.Lidct_data

        /* Dequantise: a wrapping 16-bit multiply, then >> 4. */
        ld1     {v0.8h, v1.8h, v2.8h, v3.8h}, [x0], #64
        ld1     {v16.8h, v17.8h, v18.8h, v19.8h}, [x1], #64
        ld1     {v4.8h, v5.8h, v6.8h, v7.8h}, [x0]
        ld1     {v20.8h, v21.8h, v22.8h, v23.8h}, [x1]
        mul     v0.8h, v0.8h, v16.8h
        mul     v1.8h, v1.8h, v17.8h
        mul     v2.8h, v2.8h, v18.8h
        mul     v3.8h, v3.8h, v19.8h
        mul     v4.8h, v4.8h, v20.8h
        mul     v5.8h, v5.8h, v21.8h
        mul     v6.8h, v6.8h, v22.8h
        mul     v7.8h, v7.8h, v23.8h
        sshr    v0.8h, v0.8h, #4
        sshr    v1.8h, v1.8h, #4
        sshr    v2.8h, v2.8h, #4
        sshr    v3.8h, v3.8h, #4
        sshr    v4.8h, v4.8h, #4
        sshr    v5.8h, v5.8h, #4
        sshr    v6.8h, v6.8h, #4
        sshr    v7.8h, v7.8h, #4

        /* Row pass. */
        movi    v28.4s, #4, lsl #8              // IRND_INV_ROW = 1024
        ld2     {v16.8h, v17.8h}, [x9], #32     // vmx_tab_i_04
        ld2     {v18.8h, v19.8h}, [x9], #32
        rows    0, 4
        ld2     {v16.8h, v17.8h}, [x9], #32     // vmx_tab_i_17
        ld2     {v18.8h, v19.8h}, [x9], #32
        rows    1, 7
        ld2     {v16.8h, v17.8h}, [x9], #32     // vmx_tab_i_26
        ld2     {v18.8h, v19.8h}, [x9], #32
        rows    2, 6
        ld2     {v16.8h, v17.8h}, [x9], #32     // vmx_tab_i_35
        ld2     {v18.8h, v19.8h}, [x9], #32
        rows    3, 5

        /* Column pass. v24-v27: TG_1/2, TG_2/2, TG_3/2, COS_4. */
        ld1     {v24.8h, v25.8h, v26.8h, v27.8h}, [x9], #64
        movi    v28.8h, #1
        movi    v29.8h, #32                     // IRND_INV_COL
        movi    v30.8h, #31                     // IRND_INV_CORR
        dup     v31.8h, w4                      // add_value

        sqdmulh v16.8h, v5.8h, v26.8h           // r0 = mulhi(row5, TG_3)
        sqdmulh v17.8h, v3.8h, v26.8h           // r1 = mulhi(row3, TG_3)
        sqdmulh v18.8h, v7.8h, v24.8h           // r4 = mulhi(row7, TG_1)
        sqadd   v16.8h, v16.8h, v5.8h           // r0 += r2 (row5)
        sqdmulh v19.8h, v1.8h, v24.8h           // r5 = mulhi(row1, TG_1)
        sqadd   v17.8h, v17.8h, v3.8h           // r1 += r3 (row3)
        sqadd   v16.8h, v16.8h, v3.8h           // r0 += r3
        sqsub   v20.8h, v5.8h, v17.8h           // r2 = row5 - r1
        sqdmulh v21.8h, v6.8h, v25.8h           // r7 = mulhi(row6, TG_2)
        sqdmulh v22.8h, v2.8h, v25.8h           // r3 = mulhi(row2, TG_2); r1 = r0 (v16)
        sqsub   v19.8h, v19.8h, v7.8h           // r5 -= row7
        sqadd   v18.8h, v18.8h, v1.8h           // r4 += row1
        ld1     {v24.16b}, [x9]                 // the output lane order (TG_1 is done)
        sqadd   v23.8h, v16.8h, v18.8h          // r0 = r0 + r4
        sqadd   v23.8h, v23.8h, v28.8h          // r0 += 1
        sqsub   v18.8h, v18.8h, v16.8h          // r4 -= r1; r6 = r5 (v19)
        sqsub   v17.8h, v19.8h, v20.8h          // r5 = r5 - r2
        sqadd   v17.8h, v17.8h, v28.8h          // r5 += 1
        sqadd   v19.8h, v19.8h, v20.8h          // r6 += r2; temp7 = r0 (v23); r1 = r4 (v18)
        sqadd   v16.8h, v18.8h, v17.8h          // r4 = r4 + r5
        mulhi_cos4 20, 16                       // r2 = mulhi(r4, COS_4); temp3 = r6 (v19)
        sqsub   v18.8h, v18.8h, v17.8h          // r1 -= r5
        sqadd   v21.8h, v21.8h, v2.8h           // r7 += row2
        sqsub   v22.8h, v22.8h, v6.8h           // r3 -= row6
        mulhi_cos4 17, 18                       // r0 = mulhi(r1, COS_4)
        sqadd   v5.8h, v4.8h, v0.8h             // r5 = row4 + row0
        sqsub   v6.8h, v0.8h, v4.8h             // r6 = row0 - row4
        sqadd   v16.8h, v16.8h, v20.8h          // r4 += r2
        orr     v16.16b, v16.16b, v28.16b       // r4 |= 1
        sqadd   v17.8h, v17.8h, v18.8h          // r0 += r1
        orr     v17.16b, v17.16b, v28.16b       // r0 |= 1; r2 = r5 (v5)
        sqadd   v0.8h, v5.8h, v21.8h            // r5 = r5 + r7; r1 = r6 (v6)
        sqadd   v0.8h, v0.8h, v29.8h            // r5 += IRND_INV_COL
        sqsub   v2.8h, v5.8h, v21.8h            // r2 = r2 - r7; r7 = temp7 (v23)
        sqadd   v4.8h, v6.8h, v22.8h            // r6 = r6 + r3
        sqadd   v4.8h, v4.8h, v29.8h            // r6 += IRND_INV_COL
        sqadd   v7.8h, v23.8h, v0.8h            // r7 = r7 + r5
        sshr    v7.8h, v7.8h, #6                // r7 >>= SHIFT_INV_COL
        sqsub   v6.8h, v6.8h, v22.8h            // r1 -= r3
        sqadd   v6.8h, v6.8h, v30.8h            // r1 += IRND_INV_CORR; r3 = r6 (v4)
        sqadd   v2.8h, v2.8h, v30.8h            // r2 += IRND_INV_CORR
        sqadd   v5.8h, v4.8h, v16.8h            // r6 = r6 + r4

        sqadd   v7.8h, v7.8h, v31.8h            // output row 0
        sshr    v5.8h, v5.8h, #6
        sqadd   v5.8h, v5.8h, v31.8h            // output row 1
        sqadd   v1.8h, v6.8h, v17.8h            // r1 + r0; r7 = r1 (v6)
        sshr    v1.8h, v1.8h, #6
        sqadd   v1.8h, v1.8h, v31.8h            // output row 2
        sqsub   v3.8h, v6.8h, v17.8h            // r7 = r7 - r0; r6 = temp3 (v19)
        sshr    v3.8h, v3.8h, #6
        sqsub   v0.8h, v0.8h, v23.8h            // r5 - temp7
        sshr    v0.8h, v0.8h, #6
        sqadd   v0.8h, v0.8h, v31.8h            // output row 7
        sqsub   v4.8h, v4.8h, v16.8h            // r3 -= r4
        sqadd   v18.8h, v19.8h, v2.8h           // r6 = r6 + r2
        sqsub   v2.8h, v2.8h, v19.8h            // r2 -= temp3
        sshr    v18.8h, v18.8h, #6
        sshr    v2.8h, v2.8h, #6
        sqadd   v18.8h, v18.8h, v31.8h          // output row 3
        sshr    v4.8h, v4.8h, #6
        sqadd   v2.8h, v2.8h, v31.8h            // output row 4
        sqadd   v3.8h, v3.8h, v31.8h            // output row 5
        sqadd   v4.8h, v4.8h, v31.8h            // output row 6

        /* Saturate to bytes two rows at a time, restore the column order,
         * and store. */
        sqxtun  v16.8b, v7.8h
        sqxtun2 v16.16b, v5.8h
        sqxtun  v17.8b, v1.8h
        sqxtun2 v17.16b, v18.8h
        sqxtun  v20.8b, v2.8h
        sqxtun2 v20.16b, v3.8h
        sqxtun  v21.8b, v4.8h
        sqxtun2 v21.16b, v0.8h
        tbl     v16.16b, {v16.16b}, v24.16b
        tbl     v17.16b, {v17.16b}, v24.16b
        tbl     v20.16b, {v20.16b}, v24.16b
        tbl     v21.16b, {v21.16b}, v24.16b
        st1     {v16.d}[0], [x2], x3
        st1     {v16.d}[1], [x2], x3
        st1     {v17.d}[0], [x2], x3
        st1     {v17.d}[1], [x2], x3
        st1     {v20.d}[0], [x2], x3
        st1     {v20.d}[1], [x2], x3
        st1     {v21.d}[0], [x2], x3
        st1     {v21.d}[1], [x2]
        ret
VMX_END(vmx_idct_neon)

        .section .rodata
        .p2align 4
.Lidct_data:
        /* The row tables, in the order the row pass loads them. The values
         * are internal.h's; the agreement test fails on any difference. */
        /* vmx_tab_i_04 */
        .hword  16384, 21407, 16384, 8867, 16384, -8867, 16384, -21407
        .hword  16384, 8867, -16384, -21407, -16384, 21407, 16384, -8867
        .hword  22725, 19266, 19266, -4520, 12873, -22725, 4520, -12873
        .hword  12873, 4520, -22725, -12873, 4520, 19266, 19266, -22725
        /* vmx_tab_i_17 */
        .hword  22725, 29692, 22725, 12299, 22725, -12299, 22725, -29692
        .hword  22725, 12299, -22725, -29692, -22725, 29692, 22725, -12299
        .hword  31521, 26722, 26722, -6270, 17855, -31521, 6270, -17855
        .hword  17855, 6270, -31521, -17855, 6270, 26722, 26722, -31521
        /* vmx_tab_i_26 */
        .hword  21407, 27969, 21407, 11585, 21407, -11585, 21407, -27969
        .hword  21407, 11585, -21407, -27969, -21407, 27969, 21407, -11585
        .hword  29692, 25172, 25172, -5906, 16819, -29692, 5906, -16819
        .hword  16819, 5906, -29692, -16819, 5906, 25172, 25172, -29692
        /* vmx_tab_i_35 */
        .hword  19266, 25172, 19266, 10426, 19266, -10426, 19266, -25172
        .hword  19266, 10426, -19266, -25172, -19266, 25172, 19266, -10426
        .hword  26722, 22654, 22654, -5315, 15137, -26722, 5315, -15137
        .hword  15137, 5315, -26722, -15137, 5315, 22654, 22654, -26722
        /* Column multipliers: TG_1_16 / 2, TG_2_16 / 2, TG_3_16 / 2 for
         * sqdmulh, then COS_4_16 itself. */
        .hword  6518, 6518, 6518, 6518, 6518, 6518, 6518, 6518
        .hword  13573, 13573, 13573, 13573, 13573, 13573, 13573, 13573
        .hword  -10873, -10873, -10873, -10873, -10873, -10873, -10873, -10873
        .hword  -19195, -19195, -19195, -19195, -19195, -19195, -19195, -19195
        /* Output byte order for two rows: columns 4..7 come out reversed. */
        .byte   0, 1, 2, 3, 7, 6, 5, 4, 8, 9, 10, 11, 15, 14, 13, 12

#endif
