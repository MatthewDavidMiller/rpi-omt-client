// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// Scalar ports of the reference codec's planar output stages. Both kernels are
// pixel-independent, so writing them per pixel reproduces the vector results
// exactly while dropping the reference codec's aligned staging buffer, which
// only existed to make its 64-byte stores legal.
//
// Both walk a row at a time through iterator pairs rather than indexing. Each
// row is bounds-checked once, on the way in; the inner loop then carries no
// checks and has the fixed 2:1 luma-to-chroma shape the format guarantees,
// which is what a compiler needs to vectorise it.
//
// The rounding stage saturates to bytes exactly as `_mm_packus_epi16` does.
#![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]

/// One slice's worth of decoded 4:2:2 planes.
pub struct Planes<'a> {
    pub luma: &'a [u8],
    pub luma_stride: usize,
    pub blue: &'a [u8],
    pub red: &'a [u8],
    pub chroma_stride: usize,
    /// Opaque for BGRX output; unused by the UYVY path.
    pub alpha: &'a [u8],
    pub alpha_stride: usize,
}

/// The destination rectangle for one slice.
#[derive(Clone, Copy)]
pub struct Target {
    pub stride: usize,
    pub width: usize,
    pub height: usize,
}

/// The source rows for one output row, already narrowed to the visible width.
struct Rows<'a> {
    luma: &'a [u8],
    blue: &'a [u8],
    red: &'a [u8],
    alpha: &'a [u8],
}

impl<'a> Planes<'a> {
    /// Narrows every plane to one row, or `None` if any is short.
    fn row(&self, row: usize, width: usize) -> Option<Rows<'a>> {
        let pairs = width / 2;
        Some(Rows {
            luma: self.luma.get(row * self.luma_stride..)?.get(..width)?,
            blue: self.blue.get(row * self.chroma_stride..)?.get(..pairs)?,
            red: self.red.get(row * self.chroma_stride..)?.get(..pairs)?,
            alpha: self.alpha.get(row * self.alpha_stride..)?.get(..width)?,
        })
    }
}

/// `VMX_PlanarToUYVY`: interleave one slice of 4:2:2 planes into packed UYVY.
pub fn planar_to_uyvy(planes: &Planes<'_>, target: Target, destination: &mut [u8]) {
    for row in 0..target.height {
        let Some(source) = planes.row(row, target.width) else {
            return;
        };
        let Some(out) = destination
            .get_mut(row * target.stride..)
            .and_then(|rest| rest.get_mut(..target.width * 2))
        else {
            return;
        };
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
}

/// `VMX_YUV4224ToBGRA`: convert one slice of 4:2:2 planes into packed BGRA.
///
/// The alpha plane supplies the fourth byte of each pixel; the receiver passes
/// an opaque plane so the result is the XRGB8888 the scanout expects.
pub fn yuv422_to_bgra(
    planes: &Planes<'_>,
    target: Target,
    destination: &mut [u8],
    coefficients: &[i16; 5],
) {
    for row in 0..target.height {
        let Some(source) = planes.row(row, target.width) else {
            return;
        };
        let Some(out) = destination
            .get_mut(row * target.stride..)
            .and_then(|rest| rest.get_mut(..target.width * 4))
        else {
            return;
        };
        // Two pixels share one chroma sample, so they are converted together.
        for (pixels, (((luma, alpha), &blue), &red)) in out.chunks_exact_mut(8).zip(
            source
                .luma
                .chunks_exact(2)
                .zip(source.alpha.chunks_exact(2))
                .zip(source.blue)
                .zip(source.red),
        ) {
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
                pixel[3] = alpha[index];
            }
        }
    }
}

fn mulhi(a: i16, b: i16) -> i16 {
    ((i32::from(a) * i32::from(b)) >> 16) as i16
}

fn round(value: i16) -> u8 {
    (value.saturating_add(8) >> 4).clamp(0, 255) as u8
}
