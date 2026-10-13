//! x86-64 inverse DCT in SSE2, the instruction set the reference codec was
//! written in.
//!
//! It computes exactly what `idct::idct_scalar` computes and must agree with
//! it bit for bit; the agreement tests in tests/vmx.rs hold it to that on
//! every x86-64 run. SSE2 is part of the x86-64 baseline, so no runtime
//! detection is needed; the functions still carry `target_feature(sse2)`
//! because that is what makes the intrinsics callable without `unsafe`.
//!
//! Row pass. The scalar row butterfly is four two-term dot products per
//! half; `_mm_madd_epi16` is exactly that, two products added with 32-bit
//! wraparound. Broadcasting each (x0, x2), (x4, x6), (x1, x3), (x5, x7) pair
//! against the eight-entry slices of the row table gives the four sums for
//! every output at once. The differences are reversed before packing, as the
//! reference does, which puts them at the mirrored positions.
//!
//! Column pass. A line-for-line transcription of `idct::column_pass`, with
//! its register names. Every helper there is one SSE2 instruction here:
//! saturating add and subtract, the high half of the product, the arithmetic
//! shift, the OR, and the unsigned saturating pack for the store.

use std::arch::x86_64::{
    __m128i, _mm_add_epi32, _mm_adds_epi16, _mm_madd_epi16, _mm_mulhi_epi16, _mm_mullo_epi16,
    _mm_or_si128, _mm_packs_epi32, _mm_packus_epi16, _mm_set1_epi16, _mm_set1_epi32,
    _mm_setzero_si128, _mm_shuffle_epi32, _mm_srai_epi16, _mm_srai_epi32, _mm_sub_epi32,
    _mm_subs_epi16,
};

use crate::tables::{
    COS_4_16, IRND_INV_COL, IRND_INV_CORR, IRND_INV_ROW, TAB_I_04, TAB_I_17, TAB_I_26, TAB_I_35,
    TG_1_16, TG_2_16, TG_3_16,
};

#[inline]
#[target_feature(enable = "sse2")]
fn load8(v: &[i16; 8]) -> __m128i {
    // SAFETY: `v` is 16 readable bytes; the load has no alignment
    // requirement.
    unsafe { std::arch::x86_64::_mm_loadu_si128(v.as_ptr().cast()) }
}

#[inline]
#[target_feature(enable = "sse2")]
fn row(block: &[i16; 64], index: usize) -> &[i16; 8] {
    block[index * 8..index * 8 + 8].try_into().unwrap_or(&[0; 8])
}

#[inline]
#[target_feature(enable = "sse2")]
fn table(tab: &[i16; 32], part: usize) -> &[i16; 8] {
    tab[part * 8..part * 8 + 8].try_into().unwrap_or(&[0; 8])
}

/// A 32-bit lane holding the pair (a, b) as madd reads it: a in the low half.
#[inline]
#[target_feature(enable = "sse2")]
fn pair(a: i16, b: i16) -> __m128i {
    _mm_set1_epi32(i32::from(a as u16) | (i32::from(b as u16) << 16))
}

/// One row: dequantised in, the row pass's eight outputs out.
#[inline]
#[target_feature(enable = "sse2")]
fn row_pass(x: &[i16; 8], tab: &[i16; 32]) -> __m128i {
    let t0 = _mm_madd_epi16(pair(x[0], x[2]), load8(table(tab, 0)));
    let t2 = _mm_madd_epi16(pair(x[4], x[6]), load8(table(tab, 1)));
    let t1 = _mm_madd_epi16(pair(x[1], x[3]), load8(table(tab, 2)));
    let t3 = _mm_madd_epi16(pair(x[5], x[7]), load8(table(tab, 3)));
    let even = _mm_add_epi32(_mm_add_epi32(t0, _mm_set1_epi32(IRND_INV_ROW)), t2);
    let odd = _mm_add_epi32(t3, t1);
    let sums = _mm_srai_epi32::<11>(_mm_add_epi32(odd, even));
    let differences = _mm_srai_epi32::<11>(_mm_sub_epi32(even, odd));
    // Reverse the differences: out[7 - i] takes difference i.
    _mm_packs_epi32(sums, _mm_shuffle_epi32::<0x1B>(differences))
}

#[inline]
#[target_feature(enable = "sse2")]
fn store(dst: &mut [u8], at: usize, v: __m128i) {
    let packed = _mm_packus_epi16(v, _mm_setzero_si128());
    let mut bytes = [0_u8; 16];
    // SAFETY: `bytes` is 16 writable bytes; the store has no alignment
    // requirement.
    unsafe { std::arch::x86_64::_mm_storeu_si128(bytes.as_mut_ptr().cast(), packed) };
    dst[at..at + 8].copy_from_slice(&bytes[..8]);
}

#[inline]
#[target_feature(enable = "sse2")]
fn shift(v: __m128i) -> __m128i {
    _mm_srai_epi16::<6>(v)
}

#[inline]
#[target_feature(enable = "sse2")]
fn mulhi(a: __m128i, c: i16) -> __m128i {
    _mm_mulhi_epi16(a, _mm_set1_epi16(c))
}

#[inline]
#[target_feature(enable = "sse2")]
fn adds(a: __m128i, b: __m128i) -> __m128i {
    _mm_adds_epi16(a, b)
}

#[inline]
#[target_feature(enable = "sse2")]
fn subs(a: __m128i, b: __m128i) -> __m128i {
    _mm_subs_epi16(a, b)
}

#[inline]
#[target_feature(enable = "sse2")]
fn addc(a: __m128i, c: i16) -> __m128i {
    _mm_adds_epi16(a, _mm_set1_epi16(c))
}

#[inline]
#[target_feature(enable = "sse2")]
fn corr(a: __m128i) -> __m128i {
    _mm_or_si128(a, _mm_set1_epi16(1))
}

/// The SSE2 transform. `dst` holds the block's top-left byte at 0 and rows
/// `stride` apart.
#[inline]
#[target_feature(enable = "sse2")]
#[allow(clippy::many_single_char_names)]
pub(crate) fn idct(
    block: &[i16; 64],
    matrix: &[u16; 64],
    dst: &mut [u8],
    stride: usize,
    add_value: i16,
) {
    // Dequantise in 16 bits, keeping the wraparound, then shift down by four.
    let mut rows = [[0_i16; 8]; 8];
    for (r, out) in rows.iter_mut().enumerate() {
        let q: [i16; 8] = std::array::from_fn(|i| matrix[r * 8 + i] as i16);
        let v = _mm_srai_epi16::<4>(_mm_mullo_epi16(load8(row(block, r)), load8(&q)));
        // SAFETY: `out` is 16 writable bytes.
        unsafe { std::arch::x86_64::_mm_storeu_si128(out.as_mut_ptr().cast(), v) };
    }
    let row0 = row_pass(&rows[0], &TAB_I_04);
    let row1 = row_pass(&rows[1], &TAB_I_17);
    let row2 = row_pass(&rows[2], &TAB_I_26);
    let row3 = row_pass(&rows[3], &TAB_I_35);
    let row4 = row_pass(&rows[4], &TAB_I_04);
    let row5 = row_pass(&rows[5], &TAB_I_35);
    let row6 = row_pass(&rows[6], &TAB_I_26);
    let row7 = row_pass(&rows[7], &TAB_I_17);

    let bias = _mm_set1_epi16(add_value);

    let mut r2 = row5;
    let mut r3 = row3;
    let mut r0 = mulhi(row5, TG_3_16);
    let mut r1 = mulhi(r3, TG_3_16);
    let mut r4 = mulhi(row7, TG_1_16);
    r0 = adds(r0, r2);
    let mut r5 = mulhi(row1, TG_1_16);
    r1 = adds(r1, r3);
    let mut r7 = row6;
    r0 = adds(r0, r3);
    r2 = subs(r2, r1);
    r7 = mulhi(r7, TG_2_16);
    r1 = r0;
    r3 = mulhi(row2, TG_2_16);
    r5 = subs(r5, row7);
    r4 = adds(r4, row1);
    r0 = adds(r0, r4);
    r0 = addc(r0, 1);
    r4 = subs(r4, r1);
    let mut r6 = r5;
    r5 = subs(r5, r2);
    r5 = addc(r5, 1);
    r6 = adds(r6, r2);

    let temp7 = r0;
    r1 = r4;
    r4 = adds(r4, r5);
    r2 = mulhi(r4, COS_4_16);
    let temp3 = r6;
    r1 = subs(r1, r5);
    r7 = adds(r7, row2);
    r3 = subs(r3, row6);
    r6 = row0;
    r0 = mulhi(r1, COS_4_16);
    r5 = row4;
    r5 = adds(r5, r6);
    r6 = subs(r6, row4);
    r4 = adds(r4, r2);
    r4 = corr(r4);
    r0 = adds(r0, r1);
    r0 = corr(r0);

    r2 = r5;
    r5 = adds(r5, r7);
    r1 = r6;
    r5 = addc(r5, IRND_INV_COL);
    r2 = subs(r2, r7);
    r7 = temp7;
    r6 = adds(r6, r3);
    r6 = addc(r6, IRND_INV_COL);
    r7 = adds(r7, r5);
    r7 = shift(r7);
    r1 = subs(r1, r3);
    r1 = addc(r1, IRND_INV_CORR);
    r3 = r6;
    r2 = addc(r2, IRND_INV_CORR);
    r6 = adds(r6, r4);

    r7 = adds(r7, bias);
    store(dst, 0, r7);
    r6 = shift(r6);
    r6 = adds(r6, bias);
    store(dst, stride, r6);

    r7 = r1;
    r1 = adds(shift(adds(r1, r0)), bias);
    store(dst, 2 * stride, r1);
    r6 = temp3;
    r7 = shift(subs(r7, r0));
    r5 = adds(shift(subs(r5, temp7)), bias);
    store(dst, 7 * stride, r5);
    r3 = subs(r3, r4);
    r6 = adds(r6, r2);
    r2 = subs(r2, temp3);
    r6 = shift(r6);
    r2 = shift(r2);
    r6 = adds(r6, bias);
    store(dst, 3 * stride, r6);

    r3 = shift(r3);
    r2 = adds(r2, bias);
    store(dst, 4 * stride, r2);
    r7 = adds(r7, bias);
    store(dst, 5 * stride, r7);
    r3 = adds(r3, bias);
    store(dst, 6 * stride, r3);
}
