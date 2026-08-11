// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// AArch64 NEON YUV 4:2:2 to BGRX: the output stage the appliance actually runs.
//
// NEON is mandatory in ARMv8-A, so this needs no runtime detection — the
// architecture selects it. It is a lane-for-lane translation of the portable
// kernel next door and must agree with it bit for bit; `kernels_agree_bit_for_bit`
// and `kernels_agree_at_the_saturating_extremes` in the parent module check
// that, and the committed conformance vectors pin the result to the reference
// decoder through the full decode.
//
// Sixteen pixels are converted per step, which is the width at which the four
// output byte lanes can be handed to `vst4q_u8`. Interleaving B, G, R and the
// constant X in the store unit is the point of the kernel: the portable
// version's four one-byte stores per pixel are what made this stage cost more
// than the entropy decode it follows.
//
// Unsafe surface: inside a `#[target_feature(enable = "neon")]` function the
// arithmetic intrinsics are safe to call, so the only unsafe operations left
// are the three pointer-based primitives at the top of this file. Each moves a
// fixed number of lanes to or from a slice this file has already length-checked
// against that number, and the check is next to the call.
#![allow(unsafe_code)]

use super::Rows;
use std::arch::aarch64::{
    int16x8_t, uint8x8_t, uint8x16_t, uint8x16x4_t, vcombine_s16, vcombine_u8, vdupq_n_s16,
    vdupq_n_u8, vget_high_u8, vget_low_s16, vget_low_u8, vld1_u8, vld1q_u8, vmovl_high_s16,
    vmovl_s16, vmovl_u8, vmulq_s32, vqaddq_s16, vqmovun_s16, vqsubq_s16, vqsubq_u8,
    vreinterpretq_s16_u16, vshlq_n_s16, vshrn_n_s32, vshrq_n_s16, vst4q_u8, vsubq_s16, vzip1q_s16,
    vzip2q_s16,
};

/// Pixels converted per vector step.
const STEP: usize = 16;

/// Loads eight bytes.
#[inline]
fn load8(values: &[u8]) -> uint8x8_t {
    debug_assert!(values.len() >= 8);
    // SAFETY: `vld1_u8` reads exactly eight bytes, and the assertion above is
    // the caller's guarantee that the slice holds at least that many.
    unsafe { vld1_u8(values.as_ptr()) }
}

/// Loads sixteen bytes.
#[inline]
fn load16(values: &[u8]) -> uint8x16_t {
    debug_assert!(values.len() >= 16);
    // SAFETY: `vld1q_u8` reads exactly sixteen bytes, and the assertion above
    // is the caller's guarantee that the slice holds at least that many.
    unsafe { vld1q_u8(values.as_ptr()) }
}

/// Stores sixteen four-byte pixels, interleaving the four channel vectors.
#[inline]
fn store_pixels(out: &mut [u8], pixels: uint8x16x4_t) {
    debug_assert!(out.len() >= STEP * 4);
    // SAFETY: `vst4q_u8` writes exactly four interleaved 16-byte lanes, which
    // is 64 bytes, and the assertion above is the caller's guarantee that the
    // slice holds at least that many.
    unsafe { vst4q_u8(out.as_mut_ptr(), pixels) }
}

/// `(a * b) >> 16` per lane, the portable kernel's `mulhi`.
///
/// The reference multiply is a full 32-bit product narrowed back to 16 bits,
/// not NEON's doubling `vqdmulhq_s16`, so it is built from the widening
/// multiply and a narrowing shift the way the inverse DCT builds its own.
#[inline]
fn mulhi(a: int16x8_t, b: int16x8_t) -> int16x8_t {
    // SAFETY: every intrinsic here is pure lane arithmetic on values already
    // held in registers; none reads or writes memory.
    unsafe {
        let product_low = vmulq_s32(vmovl_s16(vget_low_s16(a)), vmovl_s16(vget_low_s16(b)));
        let product_high = vmulq_s32(vmovl_high_s16(a), vmovl_high_s16(b));
        vcombine_s16(
            vshrn_n_s32::<16>(product_low),
            vshrn_n_s32::<16>(product_high),
        )
    }
}

/// `(value + 8) >> 4` saturated into bytes, the portable kernel's `round`.
#[inline]
fn round(value: int16x8_t) -> uint8x8_t {
    // SAFETY: pure lane arithmetic on register values. `vqmovun_s16` is the
    // saturating unsigned narrow, which is exactly `.clamp(0, 255) as u8`.
    unsafe { vqmovun_s16(vshrq_n_s16::<4>(vqaddq_s16(value, vdupq_n_s16(8)))) }
}

/// Widens eight unsigned bytes into eight signed 16-bit lanes.
#[inline]
fn widen(values: uint8x8_t) -> int16x8_t {
    // SAFETY: pure lane arithmetic on register values.
    unsafe { vreinterpretq_s16_u16(vmovl_u8(values)) }
}

/// `VMX_YUV4224ToBGRA` for one row.
pub(crate) fn bgra_row(source: &Rows<'_>, out: &mut [u8], coefficients: &[i16; 5]) {
    // SAFETY: NEON is part of the AArch64 baseline this module is compiled
    // for, so the feature the callee requires is always present.
    unsafe { convert(source, out, coefficients) }
}

#[target_feature(enable = "neon")]
unsafe fn convert(source: &Rows<'_>, out: &mut [u8], coefficients: &[i16; 5]) {
    let luma_coefficient = vdupq_n_s16(coefficients[0]);
    let red_coefficient = vdupq_n_s16(coefficients[1]);
    let green_blue_coefficient = vdupq_n_s16(coefficients[2]);
    let green_red_coefficient = vdupq_n_s16(coefficients[3]);
    let blue_coefficient = vdupq_n_s16(coefficients[4]);
    let bias = vdupq_n_s16(128);
    let opaque = vdupq_n_u8(0xFF);

    // `Rows` is already narrowed to the visible width, and chroma is half of
    // it, so one bound governs how many whole vector steps this row has.
    let steps = source.luma.len() / STEP;
    for step in 0..steps {
        let pixel = step * STEP;
        let pair = pixel / 2;

        // Sixteen luma samples, floored at the reference's 16 before widening:
        // the portable kernel's `saturating_sub` on a byte is this instruction.
        let luma = vqsubq_u8(load16(&source.luma[pixel..]), vdupq_n_u8(16));
        let luma_low = vshlq_n_s16::<6>(widen(vget_low_u8(luma)));
        let luma_high = vshlq_n_s16::<6>(widen(vget_high_u8(luma)));
        let scaled_low = mulhi(luma_low, luma_coefficient);
        let scaled_high = mulhi(luma_high, luma_coefficient);

        // Eight chroma pairs, each serving two of the sixteen pixels.
        let blue_difference = vsubq_s16(widen(load8(&source.blue[pair..])), bias);
        let red_difference = vsubq_s16(widen(load8(&source.red[pair..])), bias);
        let chroma_red = mulhi(vshlq_n_s16::<6>(red_difference), red_coefficient);
        let chroma_blue = mulhi(vshlq_n_s16::<7>(blue_difference), blue_coefficient);
        let green_from_blue = mulhi(vshlq_n_s16::<6>(blue_difference), green_blue_coefficient);
        let green_from_red = mulhi(vshlq_n_s16::<6>(red_difference), green_red_coefficient);

        // Duplicating each chroma lane is what turns eight pairs into sixteen
        // pixels; `vzip1`/`vzip2` of a vector with itself is that duplication.
        let blue_channel = vcombine_u8(
            round(vqaddq_s16(vzip1q_s16(chroma_blue, chroma_blue), scaled_low)),
            round(vqaddq_s16(
                vzip2q_s16(chroma_blue, chroma_blue),
                scaled_high,
            )),
        );
        let red_channel = vcombine_u8(
            round(vqaddq_s16(vzip1q_s16(chroma_red, chroma_red), scaled_low)),
            round(vqaddq_s16(vzip2q_s16(chroma_red, chroma_red), scaled_high)),
        );
        // The two green terms stay separate for the same reason they do in the
        // portable kernel: each subtraction saturates on its own.
        let green_channel = vcombine_u8(
            round(vqsubq_s16(
                vqsubq_s16(scaled_low, vzip1q_s16(green_from_blue, green_from_blue)),
                vzip1q_s16(green_from_red, green_from_red),
            )),
            round(vqsubq_s16(
                vqsubq_s16(scaled_high, vzip2q_s16(green_from_blue, green_from_blue)),
                vzip2q_s16(green_from_red, green_from_red),
            )),
        );

        store_pixels(
            &mut out[pixel * 4..],
            uint8x16x4_t(blue_channel, green_channel, red_channel, opaque),
        );
    }

    // A width that is not a whole number of steps finishes on the portable
    // kernel rather than on a masked vector: the tail is at most fifteen
    // pixels of a row and one definition of the arithmetic is worth more here
    // than the instructions it would save.
    let done = steps * STEP;
    if done < source.luma.len() {
        let tail = Rows {
            luma: &source.luma[done..],
            blue: &source.blue[done / 2..],
            red: &source.red[done / 2..],
        };
        super::scalar::bgra_row(&tail, &mut out[done * 4..], coefficients);
    }
}
