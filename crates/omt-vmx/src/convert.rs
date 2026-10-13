//! Portable ports of the reference codec's planar output stages. Both
//! kernels are pixel-independent, so writing them per pixel reproduces the
//! vector results exactly while dropping the reference's aligned staging
//! buffer.
//!
//! This is also the definition the AArch64 kernel is checked against, so it
//! stays a plain transcription of the reference arithmetic rather than
//! something tuned. Rounding saturates to bytes as _mm_packus_epi16 does.

use crate::idct::{mulhi, sat8};

/// VMX_PlanarToUYVY for one row: each group of two pixels is U, Y, V, Y.
#[inline(always)]
pub(crate) fn uyvy_row(luma: &[u8], blue: &[u8], red: &[u8], width: usize, out: &mut [u8]) {
    let pairs = width / 2;
    let out = &mut out[..pairs * 4];
    for (pair, o) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        o[0] = blue[pair];
        o[1] = luma[2 * pair];
        o[2] = red[pair];
        o[3] = luma[2 * pair + 1];
    }
}

#[inline(always)]
fn round_channel(value: i16) -> u8 {
    sat8(value.saturating_add(8) >> 4)
}

/// VMX_YUV4224ToBGRA for one row. VMX1 carries no alpha, and the scanout
/// reads the buffer as XRGB8888, so the fourth byte is a constant.
#[inline(always)]
pub(crate) fn bgra_row_scalar(
    luma: &[u8],
    blue: &[u8],
    red: &[u8],
    width: usize,
    out: &mut [u8],
    c: &[i16; 5],
) {
    let pairs = width / 2;
    let (luma, blue, red, out) =
        (&luma[..pairs * 2], &blue[..pairs], &red[..pairs], &mut out[..pairs * 8]);
    for (pair, o) in out.as_chunks_mut::<8>().0.iter_mut().enumerate() {
        let blue_difference = i16::from(blue[pair]) - 128;
        let red_difference = i16::from(red[pair]) - 128;
        let chroma_red = mulhi(red_difference.wrapping_mul(64), c[1]);
        let chroma_blue = mulhi(blue_difference.wrapping_mul(128), c[4]);
        // The two green terms stay separate: the reference subtracts them
        // one after the other, and folding them would change the result
        // wherever either subtraction saturates.
        let green_from_blue = mulhi(blue_difference.wrapping_mul(64), c[2]);
        let green_from_red = mulhi(red_difference.wrapping_mul(64), c[3]);
        for k in 0..2 {
            let y = luma[2 * pair + k];
            let floored = i16::from(y.saturating_sub(16));
            let scaled = mulhi(floored.wrapping_mul(64), c[0]);
            let pixel = &mut o[4 * k..4 * k + 4];
            pixel[0] = round_channel(chroma_blue.saturating_add(scaled));
            pixel[1] = round_channel(
                scaled.saturating_sub(green_from_blue).saturating_sub(green_from_red),
            );
            pixel[2] = round_channel(chroma_red.saturating_add(scaled));
            pixel[3] = 0xFF;
        }
    }
}

/// The conversion the decoder runs: the AArch64 kernel there, the portable
/// one elsewhere.
#[inline(always)]
pub(crate) fn bgra_row(
    luma: &[u8],
    blue: &[u8],
    red: &[u8],
    width: usize,
    out: &mut [u8],
    c: &[i16; 5],
) {
    #[cfg(target_arch = "aarch64")]
    crate::asm::bgra_row(luma, blue, red, width, out, c);
    #[cfg(not(target_arch = "aarch64"))]
    bgra_row_scalar(luma, blue, red, width, out, c);
}
