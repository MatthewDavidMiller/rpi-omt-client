//! The portable inverse DCT, and the definition every other kernel is
//! measured against: the AArch64 assembly and the x86-64 SSE2 kernel must
//! agree with it bit for bit, and on any other target this is what runs.
//!
//! The reference is SSE2. Roughly a third of its instructions are shuffles
//! that only line data up in lanes; the row pass folds them into its
//! indexing and keeps only the arithmetic. The column pass is element-wise
//! across eight lanes, so it stays in that shape, which lets the compiler
//! keep each vector in a register and vectorise the lane loops. Operation
//! order, saturations, rounding corrections, and truncations match the
//! reference exactly; tests/vectors/vmx proves that on every run.

use crate::tables::{
    COS_4_16, IRND_INV_COL, IRND_INV_CORR, IRND_INV_ROW, SHIFT_INV_COL, SHIFT_INV_ROW, TAB_I_04,
    TAB_I_17, TAB_I_26, TAB_I_35, TG_1_16, TG_2_16, TG_3_16,
};

type V = [i16; 8];

#[inline(always)]
fn map2(a: V, b: V, f: impl Fn(i16, i16) -> i16) -> V {
    let mut o = [0; 8];
    for lane in 0..8 {
        o[lane] = f(a[lane], b[lane]);
    }
    o
}

#[inline(always)]
fn map1(a: V, f: impl Fn(i16) -> i16) -> V {
    let mut o = [0; 8];
    for lane in 0..8 {
        o[lane] = f(a[lane]);
    }
    o
}

/// _mm_mulhi_epi16: the high half of the signed 32-bit product.
#[inline(always)]
pub(crate) fn mulhi(a: i16, b: i16) -> i16 {
    ((i32::from(a) * i32::from(b)) >> 16) as i16
}

#[inline(always)]
pub(crate) fn sat8(v: i16) -> u8 {
    v.clamp(0, 255) as u8
}

#[inline(always)]
fn adds(a: V, b: V) -> V {
    map2(a, b, i16::saturating_add)
}

#[inline(always)]
fn subs(a: V, b: V) -> V {
    map2(a, b, i16::saturating_sub)
}

#[inline(always)]
fn mulhi_by(a: V, scale: i16) -> V {
    map1(a, |x| mulhi(x, scale))
}

#[inline(always)]
fn adds_constant(a: V, value: i16) -> V {
    map1(a, |x| x.saturating_add(value))
}

#[inline(always)]
fn shift_right(a: V) -> V {
    map1(a, |x| x >> SHIFT_INV_COL)
}

#[inline(always)]
fn or_correction(a: V) -> V {
    map1(a, |x| x | 1)
}

#[inline(always)]
fn store_row(dst: &mut [u8], at: usize, value: V) {
    let mut bytes = [0_u8; 8];
    for lane in 0..8 {
        bytes[lane] = sat8(value[lane]);
    }
    dst[at..at + 8].copy_from_slice(&bytes);
}

/// _mm_madd_epi16 over one output lane: two products added with wraparound.
#[inline(always)]
fn product(first: i32, first_scale: i16, second: i32, second_scale: i16) -> i32 {
    (first * i32::from(first_scale)).wrapping_add(second * i32::from(second_scale))
}

/// One row butterfly. The reference shuffles the row into
/// [x0,x2,x1,x3,x4,x6,x5,x7] and broadcasts each 32-bit lane, so every madd
/// sees one fixed pair against four coefficient pairs; naming the pairs
/// removes the shuffles.
#[inline(always)]
fn row_pass(input: V, tab: &[i16; 32]) -> V {
    let x = input.map(i32::from);
    let mut out = [0_i16; 8];
    for index in 0..4 {
        let pair = index * 2;
        let t0 = product(x[0], tab[pair], x[2], tab[pair + 1]);
        let t2 = product(x[4], tab[8 + pair], x[6], tab[9 + pair]);
        let t1 = product(x[1], tab[16 + pair], x[3], tab[17 + pair]);
        let t3 = product(x[5], tab[24 + pair], x[7], tab[25 + pair]);
        let even = t0.wrapping_add(IRND_INV_ROW).wrapping_add(t2);
        let odd = t3.wrapping_add(t1);
        // The reference reverses the high half before packing, which puts
        // the differences at the mirrored positions.
        out[index] = sat16(odd.wrapping_add(even) >> SHIFT_INV_ROW);
        out[7 - index] = sat16(even.wrapping_sub(odd) >> SHIFT_INV_ROW);
    }
    out
}

#[inline(always)]
fn sat16(v: i32) -> i16 {
    v.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

/// The column butterfly over all eight columns at once. Register names
/// follow the reference kernel so the two can be diffed line by line.
#[inline(always)]
#[allow(clippy::many_single_char_names)]
fn column_pass(rows: &[V; 8], bias: V, dst: &mut [u8], stride: usize) {
    let [row0, row1, row2, row3, row4, row5, row6, row7] = *rows;

    let mut r2 = row5;
    let mut r3 = row3;
    let mut r0 = mulhi_by(row5, TG_3_16);
    let mut r1 = mulhi_by(r3, TG_3_16);
    let mut r4 = mulhi_by(row7, TG_1_16);
    r0 = adds(r0, r2);
    let mut r5 = mulhi_by(row1, TG_1_16);
    r1 = adds(r1, r3);
    let mut r7 = row6;
    r0 = adds(r0, r3);
    r2 = subs(r2, r1);
    r7 = mulhi_by(r7, TG_2_16);
    r1 = r0;
    r3 = mulhi_by(row2, TG_2_16);
    r5 = subs(r5, row7);
    r4 = adds(r4, row1);
    r0 = adds(r0, r4);
    r0 = adds_constant(r0, 1);
    r4 = subs(r4, r1);
    let mut r6 = r5;
    r5 = subs(r5, r2);
    r5 = adds_constant(r5, 1);
    r6 = adds(r6, r2);

    let temp7 = r0;
    r1 = r4;
    r4 = adds(r4, r5);
    r2 = mulhi_by(r4, COS_4_16);
    let temp3 = r6;
    r1 = subs(r1, r5);
    r7 = adds(r7, row2);
    r3 = subs(r3, row6);
    r6 = row0;
    r0 = mulhi_by(r1, COS_4_16);
    r5 = row4;
    r5 = adds(r5, r6);
    r6 = subs(r6, row4);
    r4 = adds(r4, r2);
    r4 = or_correction(r4);
    r0 = adds(r0, r1);
    r0 = or_correction(r0);

    r2 = r5;
    r5 = adds(r5, r7);
    r1 = r6;
    r5 = adds_constant(r5, IRND_INV_COL);
    r2 = subs(r2, r7);
    r7 = temp7;
    r6 = adds(r6, r3);
    r6 = adds_constant(r6, IRND_INV_COL);
    r7 = adds(r7, r5);
    r7 = shift_right(r7);
    r1 = subs(r1, r3);
    r1 = adds_constant(r1, IRND_INV_CORR);
    r3 = r6;
    r2 = adds_constant(r2, IRND_INV_CORR);
    r6 = adds(r6, r4);

    r7 = adds(r7, bias);
    store_row(dst, 0, r7);
    r6 = shift_right(r6);
    r6 = adds(r6, bias);
    store_row(dst, stride, r6);

    r7 = r1;
    r1 = adds(shift_right(adds(r1, r0)), bias);
    store_row(dst, 2 * stride, r1);
    r6 = temp3;
    r7 = shift_right(subs(r7, r0));
    r5 = adds(shift_right(subs(r5, temp7)), bias);
    store_row(dst, 7 * stride, r5);
    r3 = subs(r3, r4);
    r6 = adds(r6, r2);
    r2 = subs(r2, temp3);
    r6 = shift_right(r6);
    r2 = shift_right(r2);
    r6 = adds(r6, bias);
    store_row(dst, 3 * stride, r6);

    r3 = shift_right(r3);
    r2 = adds(r2, bias);
    store_row(dst, 4 * stride, r2);
    r7 = adds(r7, bias);
    store_row(dst, 5 * stride, r7);
    r3 = adds(r3, bias);
    store_row(dst, 6 * stride, r3);
}

/// The portable kernel: it always runs every row, whatever `rows` says.
/// `dst` holds the 8x8 block's top-left byte at 0 and rows `stride` apart.
#[inline(always)]
pub(crate) fn idct_scalar(
    block: &[i16; 64],
    matrix: &[u16; 64],
    dst: &mut [u8],
    stride: usize,
    add_value: i16,
) {
    // Dequantise. The block is already in row-major order. The reference
    // multiplies in 16 bits, keeping the wraparound, then shifts down by
    // four.
    let mut rows = [[0_i16; 8]; 8];
    for (index, (&b, &m)) in block.iter().zip(matrix.iter()).enumerate() {
        rows[index / 8][index % 8] = b.wrapping_mul(m as i16) >> 4;
    }
    let transformed = [
        row_pass(rows[0], &TAB_I_04),
        row_pass(rows[1], &TAB_I_17),
        row_pass(rows[2], &TAB_I_26),
        row_pass(rows[3], &TAB_I_35),
        row_pass(rows[4], &TAB_I_04),
        row_pass(rows[5], &TAB_I_35),
        row_pass(rows[6], &TAB_I_26),
        row_pass(rows[7], &TAB_I_17),
    ];
    column_pass(&transformed, [add_value; 8], dst, stride);
}

/// The flat block the reference codec emits when a block carries only DC.
///
/// The block's extent is checked once; the eight row stores then go out
/// unchecked. On a flat picture nearly every block takes this path, and a
/// bounds check per row was most of the difference between this decoder's
/// instruction count and the C decoder's.
#[inline(always)]
pub(crate) fn broadcast_dc(dc: i16, dst: &mut [u8], stride: usize, add_value: i16) {
    let level = (dc.wrapping_add(4) >> 3).wrapping_add(add_value);
    let row_bytes = [sat8(level); 8];
    assert!(stride >= 8 && dst.len() >= 7 * stride + 8, "DC block destination too small");
    let base = dst.as_mut_ptr();
    for row in 0..8 {
        // SAFETY: row * stride + 8 <= 7 * stride + 8 <= dst.len() (checked
        // above), so each eight-byte store lies inside `dst`.
        unsafe { base.wrapping_add(row * stride).cast::<[u8; 8]>().write_unaligned(row_bytes) };
    }
}

/// The transform the decoder runs: the AArch64 kernel there, the portable
/// one elsewhere. `rows` (1..=8) is how many leading rows may be nonzero; a
/// kernel may use it to skip work, never to change a result.
#[inline(always)]
pub(crate) fn idct(
    block: &[i16; 64],
    matrix: &[u16; 64],
    dst: &mut [u8],
    stride: usize,
    add_value: i16,
    rows: u8,
) {
    #[cfg(target_arch = "aarch64")]
    crate::asm::idct(block, matrix, dst, stride, add_value, rows);
    #[cfg(target_arch = "x86_64")]
    {
        let _ = rows;
        // SAFETY: SSE2 is part of the x86-64 baseline every build targets.
        unsafe { crate::idct_sse2::idct(block, matrix, dst, stride, add_value) };
    }
    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        let _ = rows;
        idct_scalar(block, matrix, dst, stride, add_value);
    }
}
