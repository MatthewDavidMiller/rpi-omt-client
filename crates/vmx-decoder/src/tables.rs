// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// Constants transcribed from the Open Media Transport VMX reference codec
// (MIT, see third_party/omt/PROVENANCE.md). Only the decode-side tables are
// carried over; the encoder's reciprocal and forward-DCT tables are dropped
// along with the encoder itself.

pub const SLICE_HEIGHT: usize = 16;
pub const QUALITY_COUNT: usize = 25;
pub const DECODE_MATRIX_COUNT: usize = 64;

/// Row-pass accumulator precision (`BITS_INV_ACC` in the reference codec).
const BITS_INV_ACC: i16 = 5;
pub const SHIFT_INV_ROW: i32 = 16 - BITS_INV_ACC as i32;
pub const SHIFT_INV_COL: i32 = 1 + BITS_INV_ACC as i32;
pub const IRND_INV_ROW: i16 = 1024 * (6 - BITS_INV_ACC);
pub const IRND_INV_COL: i16 = 16 * (BITS_INV_ACC - 3);
pub const IRND_INV_CORR: i16 = (16 * (BITS_INV_ACC - 3)) - 1;

pub const QUANTIZATION_MATRIX: [u16; 64] = [
    16, 16, 19, 22, 26, 27, 29, 34, //
    16, 16, 22, 24, 27, 29, 34, 37, //
    19, 22, 26, 27, 29, 34, 34, 38, //
    22, 22, 26, 27, 29, 34, 37, 40, //
    22, 26, 27, 29, 32, 35, 40, 48, //
    26, 27, 29, 32, 35, 40, 48, 58, //
    26, 27, 29, 34, 38, 46, 56, 69, //
    27, 29, 35, 38, 46, 56, 69, 83, //
];

pub const ZIGZAG_INVERSE: [u8; 64] = [
    0, 1, 5, 6, 14, 15, 27, 28, //
    2, 4, 7, 13, 16, 26, 29, 42, //
    3, 8, 12, 17, 25, 30, 41, 43, //
    9, 11, 18, 24, 31, 40, 44, 53, //
    10, 19, 23, 32, 39, 45, 52, 54, //
    20, 22, 33, 38, 46, 51, 55, 60, //
    21, 34, 37, 47, 50, 56, 59, 61, //
    35, 36, 48, 49, 57, 58, 62, 63, //
];

pub const QUALITY: [i32; QUALITY_COUNT] = [
    1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 14, 16, 18, 20, 22, 24, 28, 32, 36, 40, 44, 48, 52, 56, 64,
];

pub const TAB_I_04: [i16; 32] = [
    16384, 21407, 16384, 8867, //
    16384, -8867, 16384, -21407, //
    16384, 8867, -16384, -21407, //
    -16384, 21407, 16384, -8867, //
    22725, 19266, 19266, -4520, //
    12873, -22725, 4520, -12873, //
    12873, 4520, -22725, -12873, //
    4520, 19266, 19266, -22725, //
];
pub const TAB_I_17: [i16; 32] = [
    22725, 29692, 22725, 12299, //
    22725, -12299, 22725, -29692, //
    22725, 12299, -22725, -29692, //
    -22725, 29692, 22725, -12299, //
    31521, 26722, 26722, -6270, //
    17855, -31521, 6270, -17855, //
    17855, 6270, -31521, -17855, //
    6270, 26722, 26722, -31521, //
];
pub const TAB_I_26: [i16; 32] = [
    21407, 27969, 21407, 11585, //
    21407, -11585, 21407, -27969, //
    21407, 11585, -21407, -27969, //
    -21407, 27969, 21407, -11585, //
    29692, 25172, 25172, -5906, //
    16819, -29692, 5906, -16819, //
    16819, 5906, -29692, -16819, //
    5906, 25172, 25172, -29692, //
];
pub const TAB_I_35: [i16; 32] = [
    19266, 25172, 19266, 10426, //
    19266, -10426, 19266, -25172, //
    19266, 10426, -19266, -25172, //
    -19266, 25172, 19266, -10426, //
    26722, 22654, 22654, -5315, //
    15137, -26722, 5315, -15137, //
    15137, 5315, -26722, -15137, //
    5315, 22654, 22654, -26722, //
];

pub const TG_1_16: i16 = 13036;
pub const TG_2_16: i16 = 27146;
pub const TG_3_16: i16 = -21746;
pub const COS_4_16: i16 = -19195;

/// `Y, R, GU, GV, B` fixed-point YUV-to-RGB coefficients.
pub const YUV_RGB_709: [i16; 5] = [19077, 29372, 3494, 8731, 17305];
pub const YUV_RGB_601: [i16; 5] = [19077, 26149, 6419, 13320, 16525];

/// Builds the dequantisation matrix the reference codec derives for a quality
/// preset. The DC term is never scaled.
#[must_use]
pub fn decode_matrix(index: usize) -> [u16; DECODE_MATRIX_COUNT] {
    let scale = QUALITY[index.min(QUALITY_COUNT - 1)];
    let mut matrix = [0_u16; DECODE_MATRIX_COUNT];
    for (position, (slot, value)) in matrix.iter_mut().zip(QUANTIZATION_MATRIX).enumerate() {
        // The DC term is never scaled. The largest scaled term is 83 * 64, so
        // the reference codec's unsigned-short store never truncates.
        *slot = if position == 0 {
            value
        } else {
            u16::try_from(i32::from(value) * scale).unwrap_or(u16::MAX)
        };
    }
    matrix
}

/// Mirrors `VMX_SetQualityInternal`: the first preset whose step covers the
/// requested loss wins, and an out-of-range request keeps preset zero.
#[must_use]
pub fn quality_index(quality: i32) -> usize {
    QUALITY
        .iter()
        .position(|step| *step >= (100 - quality))
        .unwrap_or(0)
}
