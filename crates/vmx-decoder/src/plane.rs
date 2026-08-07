// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// `VMX_DecodePlaneInternal128` ported to safe Rust.
//
// The reference kernel also carries a 16 KiB twelve-bit lookahead table that
// resolves a value code, and any zero code following it, in one step. That
// table is a decode-speed optimisation over the manual path reproduced here:
// it consumes the same bits and yields the same coefficients, and its only
// observable difference — deliberately over-reading a trailing zero code into
// the next plane, which `REWINDOVERREAD` then gives back — cannot arise
// without it. The scalar path is therefore bit-exact and needs no rewind.
//
// The reference codec truncates its unsigned bitstream intermediates into
// shorts, which is the behaviour these casts reproduce.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

use crate::bitstream::BitReader;
use crate::idct::{broadcast_dc, zig_dequantize_idct};
use crate::tables::SLICE_HEIGHT;

/// Upper bound on Golomb symbols read for one 8x8 block. Every iteration
/// consumes at least one bit, so this only ever fires on damaged input.
const MAX_SYMBOLS_PER_BLOCK: u32 = 4096;

pub struct PlaneShape {
    pub stride: usize,
    /// 128 for luma and alpha, 0 for chroma, matching the reference codec's
    /// per-plane output bias.
    pub bias: i16,
}

/// Decode state that persists across the planes of one slice.
pub struct SliceStreams {
    pub dc: BitReader,
    pub ac: BitReader,
}

/// Decodes one plane of one slice into `destination`, which must be at least
/// `stride * SLICE_HEIGHT` bytes.
///
/// Returns `false` if the stream was rejected as damaged.
pub fn decode_plane(
    streams: &mut SliceStreams,
    shape: &PlaneShape,
    matrix: &[u16; 64],
    dc_shift: i32,
    destination: &mut [u8],
) -> bool {
    let stride = shape.stride;
    if stride == 0 || !stride.is_multiple_of(8) || destination.len() < stride * SLICE_HEIGHT {
        return false;
    }

    let mut block = [0_i16; 64];
    let mut pending: u32 = 0;
    let mut dc_prediction: i16 = 0;

    for row in (0..SLICE_HEIGHT).step_by(8) {
        for column in (0..stride).step_by(8) {
            block.fill(0);
            let decoded_terms = pending < 64;
            let mut guard = 0_u32;
            while pending < 64 {
                guard += 1;
                if guard > MAX_SYMBOLS_PER_BLOCK {
                    return false;
                }
                if streams.ac.bit_bare() == 1 {
                    if streams.ac.bit_bare() == 1 {
                        pending += 1;
                    } else {
                        let width = streams.ac.zeros_bare() + 2;
                        let run = streams.ac.bits_bare(width);
                        pending = pending.saturating_add(u32::try_from(run).unwrap_or(u32::MAX));
                    }
                } else {
                    let width = streams.ac.zeros_bare() + 2;
                    let value = streams.ac.bits_bare(width);
                    let slot = usize::try_from(pending).unwrap_or(0);
                    if let Some(target) = block.get_mut(slot) {
                        *target = mag_sign(value);
                    }
                    pending += 1;
                }
                streams.ac.reload();
                if streams.ac.corrupt() {
                    return false;
                }
            }
            pending -= 64;

            if streams.dc.bit() == 1 {
                let _parity = streams.dc.bit();
            } else {
                let width = streams.dc.zeros() + 2;
                let value = streams.dc.bits(width);
                block[0] = shift_signed(mag_sign(value), dc_shift);
            }
            if streams.dc.corrupt() {
                return false;
            }

            block[0] = block[0].wrapping_add(dc_prediction);
            dc_prediction = block[0];

            let offset = row * stride + column;
            let Some(target) = destination.get_mut(offset..) else {
                return false;
            };
            if decoded_terms {
                zig_dequantize_idct(&block, matrix, target, stride, shape.bias);
            } else {
                broadcast_dc(block[0], target, stride, shape.bias);
            }
        }
    }

    streams.dc.align();
    streams.ac.align();
    !(streams.dc.corrupt() || streams.ac.corrupt())
}

/// `GetIntFrom2MagSign(value - 1)` evaluated in the reference codec's unsigned
/// 64-bit arithmetic before truncation to a short.
fn mag_sign(value: u64) -> i16 {
    let input = value.wrapping_sub(1);
    let parity = input & 1;
    let adjusted = input.wrapping_add(parity);
    let result = (adjusted >> 1).wrapping_sub(adjusted.wrapping_mul(parity));
    result as u16 as i16
}

/// `VMX_ShiftSignedShort`.
fn shift_signed(value: i16, shift: i32) -> i16 {
    if shift <= 0 {
        return value;
    }
    if shift >= 16 {
        return 0;
    }
    ((value as u16) << shift) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mag_sign_matches_the_reference_mapping() {
        assert_eq!(mag_sign(1), 0);
        assert_eq!(mag_sign(2), -1);
        assert_eq!(mag_sign(3), 1);
        assert_eq!(mag_sign(4), -2);
        assert_eq!(mag_sign(5), 2);
    }

    #[test]
    fn dc_shift_matches_the_reference_bounds() {
        assert_eq!(shift_signed(-3, 0), -3);
        assert_eq!(shift_signed(-3, 16), 0);
        assert_eq!(shift_signed(1, 3), 8);
        assert_eq!(shift_signed(-1, 1), -2);
    }
}
