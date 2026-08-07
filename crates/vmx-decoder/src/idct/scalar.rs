// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// The portable reference implementation of the inverse DCT.
//
// This is the definition every other path is measured against: the NEON kernel
// beside it is required to agree with this one bit for bit, and on targets
// without a hand-written kernel this is what runs.
//
// The reference is written in SSE2 intrinsics. Roughly a third of those
// instructions are shuffles that exist only to line data up in lanes; they
// carry no arithmetic, so the row pass below folds every one of them into its
// indexing and keeps only the operations that compute something. The column
// pass needs no shuffles at all: it is element-wise across the eight lanes, so
// it stays in that shape, which is what lets a compiler keep each vector in a
// register and auto-vectorise the lane loops.
//
// The operation order, saturations, rounding corrections and truncations all
// match the reference, so the result is bit-exact with it. `tests/vectors/vmx`
// proves that on every run.
#![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
// On targets with a hand-written kernel this transform is not on the hot path,
// but it stays compiled: the differential test checks the two against each
// other, and it is the written definition of what the kernel must produce.
#![cfg_attr(target_arch = "aarch64", allow(dead_code))]
// The lane helpers below are single expressions whose whole purpose is to be
// folded into the butterfly around them. Leaving the decision to the inliner
// measured 21% slower on a 1080p frame, so it is taken here instead.
#![allow(clippy::inline_always)]

use crate::tables::{
    COS_4_16, IRND_INV_COL, IRND_INV_CORR, IRND_INV_ROW, SHIFT_INV_COL, SHIFT_INV_ROW, TAB_I_04,
    TAB_I_17, TAB_I_26, TAB_I_35, TG_1_16, TG_2_16, TG_3_16, ZIGZAG_INVERSE,
};

/// Eight packed 16-bit lanes, the width the reference kernel works in.
type I16x8 = [i16; 8];

/// `one_corr_128`, the reference kernel's rounding correction.
const ONE_CORR: i16 = 1;

/// `_mm_madd_epi16` over one output lane: two products, added with wraparound.
#[inline(always)]
fn product(first: i32, first_scale: i16, second: i32, second_scale: i16) -> i32 {
    (first * i32::from(first_scale)).wrapping_add(second * i32::from(second_scale))
}

/// `_mm_packs_epi32`.
#[inline(always)]
fn saturate16(value: i32) -> i16 {
    value.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

/// `_mm_packus_epi16`.
#[inline(always)]
fn saturate8(value: i16) -> u8 {
    value.clamp(0, 255) as u8
}

/// `_mm_mulhi_epi16`: the high half of the signed 16-bit product.
#[inline(always)]
fn mulhi(a: i16, b: i16) -> i16 {
    ((i32::from(a) * i32::from(b)) >> 16) as i16
}

// Element-wise helpers. None of them move data between lanes.

#[inline(always)]
fn adds(a: I16x8, b: I16x8) -> I16x8 {
    let mut out = [0_i16; 8];
    for lane in 0..8 {
        out[lane] = a[lane].saturating_add(b[lane]);
    }
    out
}

#[inline(always)]
fn subs(a: I16x8, b: I16x8) -> I16x8 {
    let mut out = [0_i16; 8];
    for lane in 0..8 {
        out[lane] = a[lane].saturating_sub(b[lane]);
    }
    out
}

/// `_mm_mulhi_epi16` against a broadcast constant.
#[inline(always)]
fn mulhi_by(a: I16x8, scale: i16) -> I16x8 {
    let mut out = [0_i16; 8];
    for lane in 0..8 {
        out[lane] = mulhi(a[lane], scale);
    }
    out
}

/// `_mm_adds_epi16` against a broadcast constant.
#[inline(always)]
fn adds_constant(a: I16x8, value: i16) -> I16x8 {
    let mut out = [0_i16; 8];
    for lane in 0..8 {
        out[lane] = a[lane].saturating_add(value);
    }
    out
}

/// `_mm_srai_epi16` by the column-pass shift.
#[inline(always)]
fn shift_right(a: I16x8) -> I16x8 {
    let mut out = [0_i16; 8];
    for lane in 0..8 {
        out[lane] = a[lane] >> SHIFT_INV_COL;
    }
    out
}

/// `_mm_or_si128` against the rounding correction.
#[inline(always)]
fn or_correction(a: I16x8) -> I16x8 {
    let mut out = [0_i16; 8];
    for lane in 0..8 {
        out[lane] = a[lane] | ONE_CORR;
    }
    out
}

/// `_mm_packus_epi16` for one output row.
#[inline(always)]
fn store_row(destination: &mut [u8], offset: usize, value: I16x8) {
    if let Some(row) = destination.get_mut(offset..offset + 8) {
        for lane in 0..8 {
            row[lane] = saturate8(value[lane]);
        }
    }
}

/// One row butterfly.
///
/// The reference shuffles the row into `[x0,x2,x1,x3,x4,x6,x5,x7]` and then
/// broadcasts each 32-bit lane, so every `madd` sees one fixed pair of inputs
/// against four coefficient pairs. Naming those pairs removes the shuffles.
#[inline(always)]
fn row_pass(input: I16x8, tab: &[i16; 32]) -> I16x8 {
    let x0 = i32::from(input[0]);
    let x1 = i32::from(input[1]);
    let x2 = i32::from(input[2]);
    let x3 = i32::from(input[3]);
    let x4 = i32::from(input[4]);
    let x5 = i32::from(input[5]);
    let x6 = i32::from(input[6]);
    let x7 = i32::from(input[7]);

    let mut out = [0_i16; 8];
    for index in 0..4 {
        let pair = index * 2;
        let t0 = product(x0, tab[pair], x2, tab[pair + 1]);
        let t2 = product(x4, tab[8 + pair], x6, tab[9 + pair]);
        let t1 = product(x1, tab[16 + pair], x3, tab[17 + pair]);
        let t3 = product(x5, tab[24 + pair], x7, tab[25 + pair]);
        let even = t0.wrapping_add(i32::from(IRND_INV_ROW)).wrapping_add(t2);
        let odd = t3.wrapping_add(t1);
        // The reference reverses the high half before packing, which puts the
        // differences at the mirrored positions.
        out[index] = saturate16(odd.wrapping_add(even) >> SHIFT_INV_ROW);
        out[7 - index] = saturate16(even.wrapping_sub(odd) >> SHIFT_INV_ROW);
    }
    out
}

/// The column butterfly, over all eight columns at once.
///
/// Register names follow the reference kernel so the two can be diffed line by
/// line.
#[allow(clippy::similar_names)]
#[inline(always)]
fn column_pass(rows: &[I16x8; 8], bias: I16x8, destination: &mut [u8], stride: usize) {
    let (row0, row1, row2, row3) = (rows[0], rows[1], rows[2], rows[3]);
    let (row4, row5, row6, row7) = (rows[4], rows[5], rows[6], rows[7]);

    let mut r2 = row5;
    let mut r3 = row3;
    let mut r0 = mulhi_by(row5, TG_3_16);
    let mut r1 = mulhi_by(r3, TG_3_16);
    let mut r4 = mulhi_by(row7, TG_1_16);
    r0 = adds(r0, r2);
    let mut r5 = mulhi_by(row1, TG_1_16);
    r1 = adds(r1, r3);
    let mut r7 = row6;
    r0 = adds(r0, r3);
    r2 = subs(r2, r1);
    r7 = mulhi_by(r7, TG_2_16);
    r1 = r0;
    r3 = mulhi_by(row2, TG_2_16);
    r5 = subs(r5, row7);
    r4 = adds(r4, row1);
    r0 = adds(r0, r4);
    r0 = adds_constant(r0, ONE_CORR);
    r4 = subs(r4, r1);
    let mut r6 = r5;
    r5 = subs(r5, r2);
    r5 = adds_constant(r5, ONE_CORR);
    r6 = adds(r6, r2);

    let temp7 = r0;
    r1 = r4;
    r4 = adds(r4, r5);
    r2 = mulhi_by(r4, COS_4_16);
    let temp3 = r6;
    r1 = subs(r1, r5);
    r7 = adds(r7, row2);
    r3 = subs(r3, row6);
    r6 = row0;
    r0 = mulhi_by(r1, COS_4_16);
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
    r5 = adds_constant(r5, IRND_INV_COL);
    r2 = subs(r2, r7);
    r7 = temp7;
    r6 = adds(r6, r3);
    r6 = adds_constant(r6, IRND_INV_COL);
    r7 = adds(r7, r5);
    r7 = shift_right(r7);
    r1 = subs(r1, r3);
    r1 = adds_constant(r1, IRND_INV_CORR);
    r3 = r6;
    r2 = adds_constant(r2, IRND_INV_CORR);
    r6 = adds(r6, r4);

    r7 = adds(r7, bias);
    store_row(destination, 0, r7);
    r6 = shift_right(r6);
    r6 = adds(r6, bias);
    store_row(destination, stride, r6);

    r7 = r1;
    r1 = adds(shift_right(adds(r1, r0)), bias);
    store_row(destination, 2 * stride, r1);
    r6 = temp3;
    r7 = shift_right(subs(r7, r0));
    r5 = adds(shift_right(subs(r5, temp7)), bias);
    store_row(destination, 7 * stride, r5);
    r3 = subs(r3, r4);
    r6 = adds(r6, r2);
    r2 = subs(r2, temp3);
    r6 = shift_right(r6);
    r2 = shift_right(r2);
    r6 = adds(r6, bias);
    store_row(destination, 3 * stride, r6);

    r3 = shift_right(r3);
    r2 = adds(r2, bias);
    store_row(destination, 4 * stride, r2);
    r7 = adds(r7, bias);
    store_row(destination, 5 * stride, r7);
    r3 = adds(r3, bias);
    store_row(destination, 6 * stride, r3);
}

/// Applies the inverse zig-zag, dequantises, and inverse transforms one 8x8
/// block, writing eight saturated rows of eight bytes at `stride` intervals.
pub fn zig_dequantize_idct(
    block: &[i16; 64],
    matrix: &[u16; 64],
    destination: &mut [u8],
    stride: usize,
    add_value: i16,
) {
    // Inverse zig-zag and dequantise in one pass. The reference multiplies in
    // 16 bits, keeping the wraparound, then shifts down by four.
    let mut rows = [[0_i16; 8]; 8];
    for (index, slot) in rows.iter_mut().flatten().enumerate() {
        let coefficient = block[usize::from(ZIGZAG_INVERSE[index])];
        *slot = coefficient.wrapping_mul(matrix[index].cast_signed()) >> 4;
    }

    let transformed = [
        row_pass(rows[0], &TAB_I_04),
        row_pass(rows[1], &TAB_I_17),
        row_pass(rows[2], &TAB_I_26),
        row_pass(rows[3], &TAB_I_35),
        row_pass(rows[4], &TAB_I_04),
        row_pass(rows[5], &TAB_I_35),
        row_pass(rows[6], &TAB_I_26),
        row_pass(rows[7], &TAB_I_17),
    ];
    column_pass(&transformed, [add_value; 8], destination, stride);
}

/// Writes the flat block the reference codec emits when a block carries only a
/// DC term.
pub fn broadcast_dc(dc: i16, destination: &mut [u8], stride: usize, add_value: i16) {
    let level = (dc.wrapping_add(4) >> 3).wrapping_add(add_value);
    let byte = saturate8(level);
    for row in 0..8 {
        let start = row * stride;
        if let Some(slice) = destination.get_mut(start..start + 8) {
            slice.fill(byte);
        }
    }
}
