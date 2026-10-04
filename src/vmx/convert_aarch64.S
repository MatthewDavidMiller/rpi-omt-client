/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * AArch64 YUV 4:2:2 to BGRX: the output stage the appliance runs.
 *
 * void vmx_bgra_row_neon(const uint8_t *luma, const uint8_t *blue,
 *                        const uint8_t *red, size_t width, uint8_t *out,
 *                        const int16_t c[5]);
 *
 * It computes exactly what vmx_bgra_row_scalar computes and must agree with
 * it bit for bit; tests/c/test_vmx.c checks every (Y, U, V) under both
 * coefficient sets. Sixteen pixels per step, the width at which st4 can
 * interleave B, G, R and the constant X in the store unit. A tail of fewer
 * than sixteen pixels finishes on the scalar kernel.
 *
 * Every multiply is the scalar (x * 2^k * c) >> 16 with x * 2^k in 16 bits.
 * sqdmulh computes (2 * a * c) >> 16, so passing x * 2^(k-1) gives the same
 * product with one instruction instead of a widening multiply and a narrow;
 * it cannot saturate, because c is positive.
 *
 * The scalar rounding is sat8(sat16(v + 8) >> 4). sqrshrun #4 is
 * sat8((v + 8) >> 4) without the inner saturation, and the two differ only
 * when v + 8 exceeds INT16_MAX. With the BT.601 and BT.709 coefficients every
 * channel stays within about +/-9000 for any input, so they are the same
 * function on every input this kernel is given -- which is what the
 * exhaustive test pins.
 *
 * Only v0-v7 and v16-v31 are used, so no callee-saved register is touched.
 */
#if defined(__aarch64__)
#include "vmx/aarch64.inc"

VMX_FUNCTION(vmx_bgra_row_neon)
        ld1     {v7.4h}, [x5]                   // c[0..3]: Y, R, GU, GV
        add     x9, x5, #8
        ld1     {v7.h}[4], [x9]                 // c[4]: B
        movi    v19.16b, #0xFF                  // X
        movi    v20.16b, #16
        movi    v21.8b, #0x80
        lsr     x8, x3, #4
        cbz     x8, 2f
1:
        ld1     {v0.16b}, [x0], #16
        ld1     {v1.8b}, [x1], #8
        ld1     {v2.8b}, [x2], #8
        uqsub   v0.16b, v0.16b, v20.16b         // Y floored at 16, minus 16
        eor     v1.8b, v1.8b, v21.8b            // U - 128 as a signed byte
        eor     v2.8b, v2.8b, v21.8b            // V - 128
        ushll   v3.8h, v0.8b, #5
        ushll2  v4.8h, v0.16b, #5
        sshll   v5.8h, v1.8b, #5
        sshll   v6.8h, v1.8b, #6
        sshll   v22.8h, v2.8b, #5
        sqdmulh v3.8h, v3.8h, v7.h[0]           // scaled luma, pixels 0..7
        sqdmulh v4.8h, v4.8h, v7.h[0]           // pixels 8..15
        sqdmulh v23.8h, v22.8h, v7.h[1]         // chroma red
        sqdmulh v6.8h, v6.8h, v7.h[4]           // chroma blue
        sqdmulh v5.8h, v5.8h, v7.h[2]           // green from blue
        sqdmulh v22.8h, v22.8h, v7.h[3]         // green from red
        /* Zipping a vector with itself turns eight pairs into sixteen pixels. */
        zip1    v24.8h, v6.8h, v6.8h
        zip2    v25.8h, v6.8h, v6.8h
        zip1    v26.8h, v5.8h, v5.8h
        zip2    v27.8h, v5.8h, v5.8h
        zip1    v28.8h, v22.8h, v22.8h
        zip2    v29.8h, v22.8h, v22.8h
        zip1    v30.8h, v23.8h, v23.8h
        zip2    v31.8h, v23.8h, v23.8h
        sqadd   v24.8h, v24.8h, v3.8h           // B
        sqadd   v25.8h, v25.8h, v4.8h
        sqsub   v26.8h, v3.8h, v26.8h           // G: the two terms one at a time
        sqsub   v27.8h, v4.8h, v27.8h
        sqsub   v26.8h, v26.8h, v28.8h
        sqsub   v27.8h, v27.8h, v29.8h
        sqadd   v30.8h, v30.8h, v3.8h           // R
        sqadd   v31.8h, v31.8h, v4.8h
        sqrshrun  v16.8b, v24.8h, #4
        sqrshrun2 v16.16b, v25.8h, #4
        sqrshrun  v17.8b, v26.8h, #4
        sqrshrun2 v17.16b, v27.8h, #4
        sqrshrun  v18.8b, v30.8h, #4
        sqrshrun2 v18.16b, v31.8h, #4
        st4     {v16.16b, v17.16b, v18.16b, v19.16b}, [x4], #64
        subs    x8, x8, #1
        b.ne    1b
2:
        /* The pointers have advanced past the vector part; at most fifteen
         * pixels remain, and the scalar kernel takes the same arguments. */
        ands    x3, x3, #15
        b.ne    vmx_bgra_row_scalar
        ret
VMX_END(vmx_bgra_row_neon)

#endif
