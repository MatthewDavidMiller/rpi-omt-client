// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// AArch64 NEON inverse DCT: the kernel the appliance actually runs.
//
// NEON is mandatory in ARMv8-A, so this needs no runtime detection — the
// architecture selects it. It is a lane-for-lane translation of the portable
// kernel next door and must agree with it bit for bit; `differential` in the
// parent module and the committed conformance vectors both check that.
//
// Unsafe surface: inside a `#[target_feature(enable = "neon")]` function the
// arithmetic intrinsics are safe to call, so the only unsafe operations left
// are the three pointer-based load/store primitives at the top of this file.
// Each moves exactly eight lanes to or from a fixed-size array, so the bound
// is visible in the type.
#![allow(unsafe_code)]

use crate::tables::{
    COS_4_16, IRND_INV_COL, IRND_INV_CORR, IRND_INV_ROW, SHIFT_INV_COL, SHIFT_INV_ROW, TAB_I_04,
    TAB_I_17, TAB_I_26, TAB_I_35, TG_1_16, TG_2_16, TG_3_16, ZIGZAG_INVERSE,
};
use std::arch::aarch64::{
    int16x8_t, int32x4_t, vaddq_s32, vcombine_s16, vdupq_laneq_s32, vdupq_n_s16, vdupq_n_s32,
    vextq_s32, vget_low_s16, vld1q_s16, vld1q_u16, vmull_high_s16, vmull_s16, vmulq_s16, vorrq_s16,
    vpaddq_s32, vqaddq_s16, vqmovn_s32, vqmovun_s16, vqsubq_s16, vreinterpretq_s16_s32,
    vreinterpretq_s16_u16, vreinterpretq_s32_s16, vrev64q_s32, vshrn_n_s32, vshrq_n_s16,
    vshrq_n_s32, vst1_u8, vsubq_s32, vuzp1q_s16, vuzp2q_s16,
};

/// Loads eight 16-bit lanes.
#[inline]
fn load(values: &[i16; 8]) -> int16x8_t {
    // SAFETY: `vld1q_s16` reads exactly eight `i16`, which is the whole of the
    // fixed-size array behind this reference. NEON loads are unaligned-safe.
    unsafe { vld1q_s16(values.as_ptr()) }
}

/// Loads eight 16-bit lanes of the dequantisation matrix.
#[inline]
fn load_unsigned(values: &[u16; 8]) -> int16x8_t {
    // SAFETY: as `load`, over the identically sized unsigned array. The
    // reinterpret is the bit-pattern reuse the reference multiply relies on.
    unsafe { vreinterpretq_s16_u16(vld1q_u16(values.as_ptr())) }
}

/// Stores eight saturated bytes.
#[inline]
fn store_bytes(vector: std::arch::aarch64::uint8x8_t, out: &mut [u8; 8]) {
    // SAFETY: `vst1_u8` writes exactly eight `u8`, which is the whole of the
    // fixed-size array behind this reference.
    unsafe { vst1_u8(out.as_mut_ptr(), vector) }
}

/// Reads eight consecutive coefficients out of a transform table.
#[inline]
fn table(tab: &[i16; 32], offset: usize) -> int16x8_t {
    let mut quad = [0_i16; 8];
    quad.copy_from_slice(&tab[offset..offset + 8]);
    load(&quad)
}

/// `_mm_madd_epi16`: multiply 16-bit lanes and add adjacent 32-bit products.
#[target_feature(enable = "neon")]
fn madd(a: int16x8_t, b: int16x8_t) -> int32x4_t {
    let low = vmull_s16(vget_low_s16(a), vget_low_s16(b));
    let high = vmull_high_s16(a, b);
    // A pairwise add over the two halves is exactly the adjacent-product sum.
    vpaddq_s32(low, high)
}

/// `_mm_mulhi_epi16` against a broadcast constant.
#[target_feature(enable = "neon")]
fn mulhi_by(a: int16x8_t, scale: i16) -> int16x8_t {
    let broadcast = vdupq_n_s16(scale);
    let low = vmull_s16(vget_low_s16(a), vget_low_s16(broadcast));
    let high = vmull_high_s16(a, broadcast);
    // Narrowing after a 16-bit shift keeps the high half of each product.
    vcombine_s16(vshrn_n_s32::<16>(low), vshrn_n_s32::<16>(high))
}

/// One row butterfly.
///
/// The reference shuffles the row into `[x0,x2,x1,x3,x4,x6,x5,x7]` so each
/// `madd` sees one broadcast pair. Unzipping into even and odd lanes reaches
/// the same four pairs in two instructions.
#[target_feature(enable = "neon")]
fn row_pass(input: int16x8_t, tab: &[i16; 32]) -> int16x8_t {
    let even = vreinterpretq_s32_s16(vuzp1q_s16(input, input));
    let odd = vreinterpretq_s32_s16(vuzp2q_s16(input, input));
    let pair0 = vreinterpretq_s16_s32(vdupq_laneq_s32::<0>(even));
    let pair2 = vreinterpretq_s16_s32(vdupq_laneq_s32::<1>(even));
    let pair1 = vreinterpretq_s16_s32(vdupq_laneq_s32::<0>(odd));
    let pair3 = vreinterpretq_s16_s32(vdupq_laneq_s32::<1>(odd));

    let t0 = madd(pair0, table(tab, 0));
    let t2 = madd(pair2, table(tab, 8));
    let t1 = madd(pair1, table(tab, 16));
    let t3 = madd(pair3, table(tab, 24));

    let even_sum = vaddq_s32(vaddq_s32(t0, vdupq_n_s32(i32::from(IRND_INV_ROW))), t2);
    let odd_sum = vaddq_s32(t3, t1);
    let sum = vshrq_n_s32::<SHIFT_INV_ROW>(vaddq_s32(odd_sum, even_sum));
    let difference = vshrq_n_s32::<SHIFT_INV_ROW>(vsubq_s32(even_sum, odd_sum));
    // The reference reverses the high half before packing.
    let reversed = vrev64q_s32(difference);
    let reversed = vextq_s32::<2>(reversed, reversed);
    vcombine_s16(vqmovn_s32(sum), vqmovn_s32(reversed))
}

/// Applies the inverse zig-zag, dequantises, and inverse transforms one 8x8
/// block, writing eight saturated rows of eight bytes at `stride` intervals.
#[target_feature(enable = "neon")]
#[allow(clippy::similar_names)]
fn transform(
    block: &[i16; 64],
    matrix: &[u16; 64],
    destination: &mut [u8],
    stride: usize,
    add_value: i16,
) {
    // The inverse zig-zag is a permutation, so it stays scalar; the
    // dequantising multiply that follows is eight lanes at a time.
    let mut permuted = [0_i16; 64];
    for (index, slot) in permuted.iter_mut().enumerate() {
        *slot = block[usize::from(ZIGZAG_INVERSE[index])];
    }

    let mut rows = [vdupq_n_s16(0); 8];
    for (index, row) in rows.iter_mut().enumerate() {
        let mut coefficients = [0_i16; 8];
        coefficients.copy_from_slice(&permuted[index * 8..index * 8 + 8]);
        let mut scale = [0_u16; 8];
        scale.copy_from_slice(&matrix[index * 8..index * 8 + 8]);
        *row = vshrq_n_s16::<4>(vmulq_s16(load(&coefficients), load_unsigned(&scale)));
    }

    let row0 = row_pass(rows[0], &TAB_I_04);
    let row1 = row_pass(rows[1], &TAB_I_17);
    let row2 = row_pass(rows[2], &TAB_I_26);
    let row3 = row_pass(rows[3], &TAB_I_35);
    let row4 = row_pass(rows[4], &TAB_I_04);
    let row5 = row_pass(rows[5], &TAB_I_35);
    let row6 = row_pass(rows[6], &TAB_I_26);
    let row7 = row_pass(rows[7], &TAB_I_17);

    let one = vdupq_n_s16(1);
    let round_col = vdupq_n_s16(IRND_INV_COL);
    let round_corr = vdupq_n_s16(IRND_INV_CORR);
    let bias = vdupq_n_s16(add_value);

    let mut r2 = row5;
    let mut r3 = row3;
    let mut r0 = mulhi_by(row5, TG_3_16);
    let mut r1 = mulhi_by(r3, TG_3_16);
    let mut r4 = mulhi_by(row7, TG_1_16);
    r0 = vqaddq_s16(r0, r2);
    let mut r5 = mulhi_by(row1, TG_1_16);
    r1 = vqaddq_s16(r1, r3);
    let mut r7 = row6;
    r0 = vqaddq_s16(r0, r3);
    r2 = vqsubq_s16(r2, r1);
    r7 = mulhi_by(r7, TG_2_16);
    r1 = r0;
    r3 = mulhi_by(row2, TG_2_16);
    r5 = vqsubq_s16(r5, row7);
    r4 = vqaddq_s16(r4, row1);
    r0 = vqaddq_s16(r0, r4);
    r0 = vqaddq_s16(r0, one);
    r4 = vqsubq_s16(r4, r1);
    let mut r6 = r5;
    r5 = vqsubq_s16(r5, r2);
    r5 = vqaddq_s16(r5, one);
    r6 = vqaddq_s16(r6, r2);

    let temp7 = r0;
    r1 = r4;
    r4 = vqaddq_s16(r4, r5);
    r2 = mulhi_by(r4, COS_4_16);
    let temp3 = r6;
    r1 = vqsubq_s16(r1, r5);
    r7 = vqaddq_s16(r7, row2);
    r3 = vqsubq_s16(r3, row6);
    r6 = row0;
    r0 = mulhi_by(r1, COS_4_16);
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
    r7 = vshrq_n_s16::<SHIFT_INV_COL>(r7);
    r1 = vqsubq_s16(r1, r3);
    r1 = vqaddq_s16(r1, round_corr);
    r3 = r6;
    r2 = vqaddq_s16(r2, round_corr);
    r6 = vqaddq_s16(r6, r4);

    r7 = vqaddq_s16(r7, bias);
    store_row(destination, 0, r7);
    r6 = vshrq_n_s16::<SHIFT_INV_COL>(r6);
    r6 = vqaddq_s16(r6, bias);
    store_row(destination, stride, r6);

    r7 = r1;
    r1 = vqaddq_s16(vshrq_n_s16::<SHIFT_INV_COL>(vqaddq_s16(r1, r0)), bias);
    store_row(destination, 2 * stride, r1);
    r6 = temp3;
    r7 = vshrq_n_s16::<SHIFT_INV_COL>(vqsubq_s16(r7, r0));
    r5 = vqaddq_s16(vshrq_n_s16::<SHIFT_INV_COL>(vqsubq_s16(r5, temp7)), bias);
    store_row(destination, 7 * stride, r5);
    r3 = vqsubq_s16(r3, r4);
    r6 = vqaddq_s16(r6, r2);
    r2 = vqsubq_s16(r2, temp3);
    r6 = vshrq_n_s16::<SHIFT_INV_COL>(r6);
    r2 = vshrq_n_s16::<SHIFT_INV_COL>(r2);
    r6 = vqaddq_s16(r6, bias);
    store_row(destination, 3 * stride, r6);

    r3 = vshrq_n_s16::<SHIFT_INV_COL>(r3);
    r2 = vqaddq_s16(r2, bias);
    store_row(destination, 4 * stride, r2);
    r7 = vqaddq_s16(r7, bias);
    store_row(destination, 5 * stride, r7);
    r3 = vqaddq_s16(r3, bias);
    store_row(destination, 6 * stride, r3);
}

/// `_mm_packus_epi16` for one output row, bounds-checked against the plane.
#[target_feature(enable = "neon")]
fn store_row(destination: &mut [u8], offset: usize, value: int16x8_t) {
    let mut bytes = [0_u8; 8];
    store_bytes(vqmovun_s16(value), &mut bytes);
    if let Some(row) = destination.get_mut(offset..offset + 8) {
        row.copy_from_slice(&bytes);
    }
}

/// Safe entry point, taking ordinary slices.
pub fn zig_dequantize_idct(
    block: &[i16; 64],
    matrix: &[u16; 64],
    destination: &mut [u8],
    stride: usize,
    add_value: i16,
) {
    // SAFETY: `transform` requires only the `neon` target feature, which is
    // mandatory in ARMv8-A and therefore present on every `aarch64` target
    // this module is compiled for. There is no CPU reachable here that lacks
    // it, so no runtime check can fail. Every other precondition is carried by
    // the argument types: the block and matrix are fixed-size arrays, and the
    // destination is a slice whose bounds `store_row` checks per row.
    unsafe { transform(block, matrix, destination, stride, add_value) };
}
