//! VMX_DecodePlaneInternal128.
//!
//! The reference kernel also carries a 16 KiB twelve-bit lookahead table
//! that resolves a value code, and any zero code following it, in one step,
//! and deliberately over-reads a trailing zero code into the next plane,
//! which REWINDOVERREAD then gives back. This port resolves one code per
//! lookup instead (`AC_LOOKAHEAD`), and only when the whole code is already
//! in the window, so it never over-reads and needs no rewind. A code the
//! table cannot resolve, or one at the edge of the window, takes the
//! bit-by-bit path, which is the reference's manual path and the definition
//! the table is held to.
//!
//! The reference truncates its unsigned bitstream intermediates into shorts,
//! which is the behaviour the casts below reproduce.

use crate::ac_lookahead::AC_LOOKAHEAD;
use crate::bits::Reader;
use crate::idct::{broadcast_dc, idct};
use crate::tables::{ROWS_USED, SLICE_HEIGHT, ZIGZAG_NATURAL};

/// Upper bound on Golomb symbols read for one 8x8 block. Every iteration
/// consumes at least one bit, so this only ever fires on damaged input.
const MAX_SYMBOLS_PER_BLOCK: u32 = 4096;
pub(crate) const LOOKAHEAD_BITS: i32 = 12;

/// GetIntFrom2MagSign(value - 1) in the reference codec's unsigned 64-bit
/// arithmetic, truncated to a short.
#[inline(always)]
pub(crate) fn mag_sign(value: u64) -> i16 {
    let input = value.wrapping_sub(1);
    let parity = input & 1;
    let adjusted = input.wrapping_add(parity);
    let result = (adjusted >> 1).wrapping_sub(adjusted.wrapping_mul(parity));
    result as u16 as i16
}

/// VMX_ShiftSignedShort.
#[inline(always)]
pub(crate) fn shift_signed(value: i16, shift: i32) -> i16 {
    if shift <= 0 {
        return value;
    }
    if shift >= 16 {
        return 0;
    }
    (u32::from(value as u16) << shift) as u16 as i16
}

/// Decodes one plane of one slice into `dst`, which holds at least
/// `stride * 16` bytes. `bias` is 128 for luma, 0 for chroma. Returns false
/// when the stream was rejected as damaged. The readers carry on into the
/// slice's next plane.
///
/// Out of line, with the readers decoded from local copies that are written
/// back on every path: three planes inlined into the slice decoder left too
/// little register room, and readers reached through the caller's pointers
/// were spilled and reloaded around every coefficient.
#[inline(never)]
pub(crate) fn decode_plane(
    dc: &mut Reader<'_>,
    ac: &mut Reader<'_>,
    stride: usize,
    bias: i16,
    matrix: &[u16; 64],
    dc_shift: i32,
    dst: &mut [u8],
) -> bool {
    let (mut local_dc, mut local_ac) = (dc.clone(), ac.clone());
    let ok = decode_plane_inner(&mut local_dc, &mut local_ac, stride, bias, matrix, dc_shift, dst);
    *dc = local_dc;
    *ac = local_ac;
    ok
}

#[inline(always)]
fn decode_plane_inner(
    dc: &mut Reader<'_>,
    ac: &mut Reader<'_>,
    stride: usize,
    bias: i16,
    matrix: &[u16; 64],
    dc_shift: i32,
    dst: &mut [u8],
) -> bool {
    if stride == 0 || !stride.is_multiple_of(8) || dst.len() / SLICE_HEIGHT < stride {
        return false;
    }
    let mut pending: u32 = 0;
    let mut dc_prediction: i16 = 0;
    // All zero between blocks: after a transform only the rows the stream
    // could have written are cleared (every nonzero coefficient lies in the
    // first ROWS_USED[last] rows), and a DC-only block touches only the DC
    // term, so the block is never zeroed whole. The C decoder cleared all
    // 128 bytes per block, which on flat pictures -- nearly every block
    // DC-only -- was a large share of the plane's cost.
    let mut block = [0_i16; 64];

    for row in (0..SLICE_HEIGHT).step_by(8) {
        for column in (0..stride).step_by(8) {
            let decoded_terms = pending < 64;
            let mut guard = 0_u32;
            let mut last = 0_u32; // the highest zig-zag position written
            while pending < 64 {
                guard += 1;
                if guard > MAX_SYMBOLS_PER_BLOCK {
                    return false;
                }
                // With at least LOOKAHEAD_BITS unread, a code the table
                // resolves is wholly inside the window, so consuming its
                // length leaves the reader exactly where the bit-by-bit path
                // would, and none of that path's corrupt cases can arise.
                let entry = if ac.bits_left >= LOOKAHEAD_BITS {
                    let index = (ac.window << (64 - ac.bits_left)) >> (64 - LOOKAHEAD_BITS);
                    AC_LOOKAHEAD[index as usize & 4095]
                } else {
                    0
                };
                if entry != 0 {
                    // No branch on the code's kind, which detail makes
                    // unpredictable: a run of zeros carries the coefficient
                    // 0 and stores it into a position that is already 0.
                    let coefficient = entry as u16 as i16;
                    ac.bits_left -= ((entry >> 16) & 0xFF) as i32;
                    block[usize::from(ZIGZAG_NATURAL[pending as usize & 63] & 63)] = coefficient;
                    last = if coefficient != 0 { pending } else { last };
                    pending += entry >> 24;
                } else if ac.take(1) == 1 {
                    if ac.take(1) == 1 {
                        pending += 1;
                    } else {
                        let width = ac.zeros_bare() + 2;
                        let run = ac.take(width);
                        pending = (u64::from(pending) + run).min(u64::from(u32::MAX)) as u32;
                    }
                } else {
                    let width = ac.zeros_bare() + 2;
                    let value = ac.take(width);
                    if pending < 64 {
                        last = pending;
                        block[usize::from(ZIGZAG_NATURAL[pending as usize] & 63)] = mag_sign(value);
                    }
                    pending += 1;
                }
                ac.reload();
                if ac.corrupt {
                    return false;
                }
            }
            pending -= 64;

            if dc.bit() == 1 {
                let _ = dc.bit();
            } else {
                let width = dc.zeros() + 2;
                let value = dc.bits(width);
                block[0] = shift_signed(mag_sign(value), dc_shift);
            }
            if dc.corrupt {
                return false;
            }

            block[0] = block[0].wrapping_add(dc_prediction);
            dc_prediction = block[0];

            let target = &mut dst[row * stride + column..];
            if decoded_terms {
                let rows = ROWS_USED[last as usize & 63];
                idct(&block, matrix, target, stride, bias, rows);
                block[..usize::from(rows) * 8].fill(0);
            } else {
                broadcast_dc(block[0], target, stride, bias);
                block[0] = 0;
            }
        }
    }

    dc.align();
    ac.align();
    !(dc.corrupt || ac.corrupt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::Stream;
    use crate::idct::idct_scalar;
    use crate::tables::{QUALITY_COUNT, decode_matrix};

    #[test]
    fn plane_helpers_match_the_reference() {
        assert_eq!([1, 2, 3, 4, 5].map(mag_sign), [0, -1, 1, -2, 2]);
        assert_eq!(shift_signed(-3, 0), -3);
        assert_eq!(shift_signed(-3, 16), 0);
        assert_eq!(shift_signed(1, 3), 8);
        assert_eq!(shift_signed(-1, 1), -2);
    }

    fn xorshift(s: &mut u64) -> u64 {
        *s ^= *s << 13;
        *s ^= *s >> 7;
        *s ^= *s << 17;
        *s
    }

    /// The decoder's AC loop before the lookahead table: one bit field at a
    /// time, as the reference's manual path reads them.
    fn reference_plane(
        dc: &mut Reader<'_>,
        ac: &mut Reader<'_>,
        stride: usize,
        bias: i16,
        matrix: &[u16; 64],
        dst: &mut [u8],
    ) -> bool {
        let mut pending: u32 = 0;
        let mut dc_prediction: i16 = 0;
        for row in (0..SLICE_HEIGHT).step_by(8) {
            for column in (0..stride).step_by(8) {
                let mut block = [0_i16; 64];
                let decoded_terms = pending < 64;
                let mut guard = 0;
                while pending < 64 {
                    guard += 1;
                    if guard > 4096 {
                        return false;
                    }
                    if ac.take(1) == 1 {
                        if ac.take(1) == 1 {
                            pending += 1;
                        } else {
                            let width = ac.zeros_bare() + 2;
                            let next = u64::from(pending) + ac.take(width);
                            pending = next.min(u64::from(u32::MAX)) as u32;
                        }
                    } else {
                        let width = ac.zeros_bare() + 2;
                        let value = ac.take(width);
                        if pending < 64 {
                            block[usize::from(ZIGZAG_NATURAL[pending as usize])] = mag_sign(value);
                        }
                        pending += 1;
                    }
                    ac.reload();
                    if ac.corrupt {
                        return false;
                    }
                }
                pending -= 64;
                if dc.bit() == 1 {
                    let _ = dc.bit();
                } else {
                    let width = dc.zeros() + 2;
                    block[0] = mag_sign(dc.bits(width));
                }
                if dc.corrupt {
                    return false;
                }
                block[0] = block[0].wrapping_add(dc_prediction);
                dc_prediction = block[0];
                let target = &mut dst[row * stride + column..];
                if decoded_terms {
                    idct_scalar(&block, matrix, target, stride, bias);
                } else {
                    broadcast_dc(block[0], target, stride, bias);
                }
            }
        }
        dc.align();
        ac.align();
        !(dc.corrupt || ac.corrupt)
    }

    /// The table must leave the decoder in exactly the state the bit-by-bit
    /// path does -- the same blocks, the same reader position, the same
    /// verdict on a damaged stream.
    #[test]
    #[cfg_attr(miri, ignore = "exhaustive; covered under ASan and on AArch64")]
    fn lookahead_matches_the_bit_by_bit_path() {
        const STRIDE: usize = 64;
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        for round in 0..3000 {
            // Uniform bytes are mostly damaged streams; biased ones are dense
            // with the short codes the table resolves; a run of zero bytes
            // holds codes too long for it.
            let mut dc = [0_u8; 512];
            let mut ac = [0_u8; 2048];
            let ac_len = 1 + (xorshift(&mut state) % ac.len() as u64) as usize;
            for b in &mut dc {
                *b = xorshift(&mut state) as u8;
            }
            for b in &mut ac[..ac_len] {
                let r = xorshift(&mut state);
                *b = match round % 3 {
                    0 => r as u8,
                    1 => (r | (r >> 8) | (r >> 16)) as u8,
                    _ => {
                        if r.is_multiple_of(7) {
                            0
                        } else {
                            (r | (r >> 8)) as u8
                        }
                    }
                };
            }
            let matrix = decode_matrix(round % QUALITY_COUNT);
            let mut dc_stream = Stream::new(4096);
            let mut ac_stream = Stream::new(4096);
            assert!(dc_stream.load(&dc) && ac_stream.load(&ac[..ac_len]));
            let (mut fast_dc, mut fast_ac) = (dc_stream.reader(), ac_stream.reader());
            let (mut slow_dc, mut slow_ac) = (dc_stream.reader(), ac_stream.reader());
            let mut expected = [0_u8; STRIDE * SLICE_HEIGHT];
            let mut actual = [0_u8; STRIDE * SLICE_HEIGHT];
            let want =
                reference_plane(&mut slow_dc, &mut slow_ac, STRIDE, 128, &matrix, &mut expected);
            let got =
                decode_plane(&mut fast_dc, &mut fast_ac, STRIDE, 128, &matrix, 0, &mut actual);
            assert_eq!(want, got, "round {round} verdict");
            if want && got {
                assert_eq!(expected, actual, "round {round} pixels");
                assert_eq!(
                    (fast_ac.position, fast_ac.bits_left, fast_dc.position),
                    (slow_ac.position, slow_ac.bits_left, slow_dc.position),
                    "round {round} reader"
                );
            }
        }
    }
}
