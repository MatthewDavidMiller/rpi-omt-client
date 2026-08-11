// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// Portable ports of the reference codec's planar output stages. Both kernels
// are pixel-independent, so writing them per pixel reproduces the vector
// results exactly while dropping the reference codec's aligned staging buffer,
// which only existed to make its 64-byte stores legal.
//
// Each kernel converts one row whose ends the caller has already narrowed, so
// the inner loop carries no bounds check and has the fixed 2:1 luma-to-chroma
// shape the format guarantees.
//
// This is also the definition the AArch64 kernel is checked against, so it
// stays a plain transcription of the reference arithmetic rather than
// something tuned: a differential test is only worth anything if the two sides
// were written independently.
//
// The rounding stage saturates to bytes exactly as `_mm_packus_epi16` does.
#![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]

use super::Rows;

/// `VMX_PlanarToUYVY` for one row.
pub(crate) fn uyvy_row(source: &Rows<'_>, out: &mut [u8]) {
    // One output group is two pixels: U, Y, V, Y.
    for (group, ((luma, &blue), &red)) in out
        .chunks_exact_mut(4)
        .zip(source.luma.chunks_exact(2).zip(source.blue).zip(source.red))
    {
        group[0] = blue;
        group[1] = luma[0];
        group[2] = red;
        group[3] = luma[1];
    }
}

/// `VMX_YUV4224ToBGRA` for one row.
pub(crate) fn bgra_row(source: &Rows<'_>, out: &mut [u8], coefficients: &[i16; 5]) {
    // Two pixels share one chroma sample, so they are converted together.
    for (pixels, ((luma, &blue), &red)) in out
        .chunks_exact_mut(8)
        .zip(source.luma.chunks_exact(2).zip(source.blue).zip(source.red))
    {
        let blue_difference = i16::from(blue) - 128;
        let red_difference = i16::from(red) - 128;
        let chroma_red = mulhi(red_difference << 6, coefficients[1]);
        let chroma_blue = mulhi(blue_difference << 7, coefficients[4]);
        // The two green terms stay separate: the reference subtracts them
        // one after the other, and folding them together would change the
        // result wherever either subtraction saturates.
        let green_from_blue = mulhi(blue_difference << 6, coefficients[2]);
        let green_from_red = mulhi(red_difference << 6, coefficients[3]);

        for (index, pixel) in pixels.chunks_exact_mut(4).enumerate() {
            let scaled = mulhi(
                i16::from(luma[index].saturating_sub(16)) << 6,
                coefficients[0],
            );
            pixel[0] = round(chroma_blue.saturating_add(scaled));
            pixel[1] = round(
                scaled
                    .saturating_sub(green_from_blue)
                    .saturating_sub(green_from_red),
            );
            pixel[2] = round(chroma_red.saturating_add(scaled));
            pixel[3] = 0xFF;
        }
    }
}

fn mulhi(a: i16, b: i16) -> i16 {
    ((i32::from(a) * i32::from(b)) >> 16) as i16
}

fn round(value: i16) -> u8 {
    (value.saturating_add(8) >> 4).clamp(0, 255) as u8
}
