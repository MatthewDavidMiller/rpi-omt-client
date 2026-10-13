//! Constants transcribed from the Open Media Transport VMX reference codec
//! (MIT, see third_party/omt/PROVENANCE.md). Only the decode-side tables are
//! carried over.

pub(crate) const SLICE_HEIGHT: usize = 16;
pub(crate) const QUALITY_COUNT: usize = 25;
const BITS_INV_ACC: i32 = 5;
pub(crate) const SHIFT_INV_ROW: i32 = 16 - BITS_INV_ACC;
pub(crate) const SHIFT_INV_COL: i32 = 1 + BITS_INV_ACC;
pub(crate) const IRND_INV_ROW: i32 = 1024 * (6 - BITS_INV_ACC);
pub(crate) const IRND_INV_COL: i16 = 16 * (BITS_INV_ACC as i16 - 3);
pub(crate) const IRND_INV_CORR: i16 = 16 * (BITS_INV_ACC as i16 - 3) - 1;
pub(crate) const TG_1_16: i16 = 13036;
pub(crate) const TG_2_16: i16 = 27146;
pub(crate) const TG_3_16: i16 = -21746;
pub(crate) const COS_4_16: i16 = -19195;

pub(crate) const QUANTIZATION_MATRIX: [u16; 64] = [
    16, 16, 19, 22, 26, 27, 29, 34, 16, 16, 22, 24, 27, 29, 34, 37, 19, 22, 26, 27, 29, 34, 34, 38,
    22, 22, 26, 27, 29, 34, 37, 40, 22, 26, 27, 29, 32, 35, 40, 48, 26, 27, 29, 32, 35, 40, 48, 58,
    26, 27, 29, 34, 38, 46, 56, 69, 27, 29, 35, 38, 46, 56, 69, 83,
];

pub(crate) const QUALITY: [i32; QUALITY_COUNT] =
    [1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 14, 16, 18, 20, 22, 24, 28, 32, 36, 40, 44, 48, 52, 56, 64];

/// Y, R, GU, GV, B fixed-point YUV-to-RGB coefficients.
pub(crate) const YUV_RGB_709: [i16; 5] = [19077, 29372, 3494, 8731, 17305];
pub(crate) const YUV_RGB_601: [i16; 5] = [19077, 26149, 6419, 13320, 16525];

/// The row-major position of each coefficient, indexed by its place in the
/// stream's zig-zag order. The entropy decoder stores every coefficient
/// where the transform reads it, so neither kernel permutes the block.
pub(crate) const ZIGZAG_NATURAL: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// How many leading rows of a block can hold a coefficient once the stream
/// has written zig-zag position p: rows below that count are certainly
/// zero. The inverse transform skips their row passes, whose output for a
/// zero row is exactly zero.
pub(crate) const ROWS_USED: [u8; 64] = [
    1, 1, 2, 3, 3, 3, 3, 3, 3, 4, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 6, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8,
];

pub(crate) const TAB_I_04: [i16; 32] = [
    16384, 21407, 16384, 8867, 16384, -8867, 16384, -21407, 16384, 8867, -16384, -21407, -16384,
    21407, 16384, -8867, 22725, 19266, 19266, -4520, 12873, -22725, 4520, -12873, 12873, 4520,
    -22725, -12873, 4520, 19266, 19266, -22725,
];
pub(crate) const TAB_I_17: [i16; 32] = [
    22725, 29692, 22725, 12299, 22725, -12299, 22725, -29692, 22725, 12299, -22725, -29692, -22725,
    29692, 22725, -12299, 31521, 26722, 26722, -6270, 17855, -31521, 6270, -17855, 17855, 6270,
    -31521, -17855, 6270, 26722, 26722, -31521,
];
pub(crate) const TAB_I_26: [i16; 32] = [
    21407, 27969, 21407, 11585, 21407, -11585, 21407, -27969, 21407, 11585, -21407, -27969, -21407,
    27969, 21407, -11585, 29692, 25172, 25172, -5906, 16819, -29692, 5906, -16819, 16819, 5906,
    -29692, -16819, 5906, 25172, 25172, -29692,
];
pub(crate) const TAB_I_35: [i16; 32] = [
    19266, 25172, 19266, 10426, 19266, -10426, 19266, -25172, 19266, 10426, -19266, -25172, -19266,
    25172, 19266, -10426, 26722, 22654, 22654, -5315, 15137, -26722, 5315, -15137, 15137, 5315,
    -26722, -15137, 5315, 22654, 22654, -26722,
];

/// The dequantisation matrix for a quality preset. The DC term is never
/// scaled; the largest scaled term is 83 * 64, so the reference codec's
/// unsigned-short store never truncates.
pub(crate) fn decode_matrix(index: usize) -> [u16; 64] {
    let scale = QUALITY[index.min(QUALITY_COUNT - 1)];
    let mut out = QUANTIZATION_MATRIX;
    for v in out.iter_mut().skip(1) {
        *v = (i32::from(*v) * scale) as u16;
    }
    out
}

/// Mirrors VMX_SetQualityInternal: the first preset whose step covers the
/// requested loss wins, and an out-of-range request keeps preset zero.
pub(crate) fn quality_index(quality: i32) -> usize {
    QUALITY.iter().position(|&q| q >= 100 - quality).unwrap_or(0)
}
