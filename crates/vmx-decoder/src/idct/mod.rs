// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// The inverse DCT, and the choice of which implementation runs.
//
// The selection is made at compile time rather than by runtime detection:
// NEON is mandatory in ARMv8-A, so on the appliance's target the hand-written
// kernel is always the right one, and anywhere else the portable kernel is.
// That keeps the dispatch free and leaves no untested branch in the hot path.

mod scalar;

#[cfg(target_arch = "aarch64")]
mod neon;

pub use scalar::broadcast_dc;

/// Applies the inverse zig-zag, dequantises, and inverse transforms one 8x8
/// block, writing eight saturated rows of eight bytes at `stride` intervals.
#[inline]
pub fn zig_dequantize_idct(
    block: &[i16; 64],
    matrix: &[u16; 64],
    destination: &mut [u8],
    stride: usize,
    add_value: i16,
) {
    #[cfg(target_arch = "aarch64")]
    neon::zig_dequantize_idct(block, matrix, destination, stride, add_value);
    #[cfg(not(target_arch = "aarch64"))]
    scalar::zig_dequantize_idct(block, matrix, destination, stride, add_value);
}

#[cfg(test)]
mod tests {
    use crate::tables::decode_matrix;

    /// Every hand-written kernel has to agree with the portable one on every
    /// byte. The committed vectors already pin the active path to the C
    /// reference; this pins the two Rust paths to each other over inputs the
    /// vectors do not reach, including coefficients that drive the butterfly
    /// into saturation.
    #[test]
    fn kernels_agree_bit_for_bit() {
        let mut state = 0x2545_F491_4F6C_DD1D_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };

        for round in 0..2000 {
            let mut block = [0_i16; 64];
            for (index, slot) in block.iter_mut().enumerate() {
                let raw = next();
                *slot = match round % 4 {
                    // Realistic sparse coefficients.
                    0 => i16::try_from(raw % 512).unwrap_or(0) - 256,
                    // Only a DC term.
                    1 if index == 0 => i16::try_from(raw % 4096).unwrap_or(0) - 2048,
                    1 => 0,
                    // Full-range values, which push the saturating steps.
                    2 => u16::try_from(raw & 0xFFFF).unwrap_or(0).cast_signed(),
                    _ => {
                        if raw % 8 == 0 {
                            i16::MAX
                        } else if raw % 8 == 1 {
                            i16::MIN
                        } else {
                            0
                        }
                    }
                };
            }
            let matrix = decode_matrix(round % 25);
            for bias in [0_i16, 128] {
                let mut expected = [0_u8; 64];
                super::scalar::zig_dequantize_idct(&block, &matrix, &mut expected, 8, bias);
                let mut actual = [0_u8; 64];
                super::zig_dequantize_idct(&block, &matrix, &mut actual, 8, bias);
                assert_eq!(
                    actual, expected,
                    "round {round}, bias {bias}: kernels disagree"
                );
            }
        }
    }
}
