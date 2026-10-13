//! The AArch64 assembly kernels and their safe entry points.
//!
//! The instructions are src/vmx's, copied by tools/gen/gen_vmx_asm.py and
//! assembled here after the macros in asm/prelude.s. Each entry point checks
//! that every byte the kernel touches lies inside the slices it was given
//! before handing the kernel raw pointers.

use std::arch::global_asm;

// Raw: the kernels' register lists use braces, which are not format
// placeholders here.
global_asm!(
    concat!(
        include_str!("../asm/prelude.s"),
        include_str!("../asm/idct_aarch64.s"),
        include_str!("../asm/convert_aarch64.s"),
    ),
    options(raw)
);

unsafe extern "C" {
    fn vmx_idct_neon(
        block: *const i16,
        matrix: *const u16,
        dst: *mut u8,
        stride: usize,
        add_value: i16,
        rows: u32,
    );
    fn vmx_bgra_row_neon(
        luma: *const u8,
        blue: *const u8,
        red: *const u8,
        width: usize,
        out: *mut u8,
        c: *const i16,
    );
}

/// The kernel finishes a row narrower than sixteen pixels -- or the tail of
/// a wider one -- by tail-calling this, with its pointers advanced past the
/// part it converted.
///
/// # Safety
///
/// `luma` must be readable for `width` bytes, `blue` and `red` for
/// `width / 2`, `out` writable for `width * 4`, and `c` readable for five
/// coefficients; the regions must not overlap `out`.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn vmx_bgra_row_scalar(
    luma: *const u8,
    blue: *const u8,
    red: *const u8,
    width: usize,
    out: *mut u8,
    c: *const i16,
) {
    let pairs = width / 2;
    // SAFETY (each block below): the caller guarantees each region's extent
    // (see above); the only caller is vmx_bgra_row_neon, whose own caller
    // checked them.
    // SAFETY: see above.
    let luma = unsafe { std::slice::from_raw_parts(luma, pairs * 2) };
    // SAFETY: see above.
    let blue = unsafe { std::slice::from_raw_parts(blue, pairs) };
    // SAFETY: see above.
    let red = unsafe { std::slice::from_raw_parts(red, pairs) };
    // SAFETY: see above.
    let out = unsafe { std::slice::from_raw_parts_mut(out, pairs * 8) };
    // SAFETY: see above.
    let c = unsafe { &*c.cast::<[i16; 5]>() };
    crate::convert::bgra_row_scalar(luma, blue, red, pairs * 2, out, c);
}

#[inline(always)]
pub(crate) fn idct(
    block: &[i16; 64],
    matrix: &[u16; 64],
    dst: &mut [u8],
    stride: usize,
    add_value: i16,
    rows: u8,
) {
    // The kernel writes eight bytes on each of eight rows.
    assert!(stride >= 8 && dst.len() >= 7 * stride + 8, "IDCT destination too small");
    // SAFETY: block and matrix are 64 entries each, dst holds every byte the
    // kernel stores (checked above), and rows is 1..=8 by construction
    // (crate::tables::ROWS_USED); the kernel touches no other memory.
    unsafe {
        vmx_idct_neon(
            block.as_ptr(),
            matrix.as_ptr(),
            dst.as_mut_ptr(),
            stride,
            add_value,
            u32::from(rows),
        );
    }
}

#[inline(always)]
pub(crate) fn bgra_row(
    luma: &[u8],
    blue: &[u8],
    red: &[u8],
    width: usize,
    out: &mut [u8],
    c: &[i16; 5],
) {
    let pairs = width / 2;
    assert!(
        luma.len() >= pairs * 2
            && blue.len() >= pairs
            && red.len() >= pairs
            && out.len() >= pairs * 8,
        "conversion row out of bounds"
    );
    // SAFETY: each pointer is valid for the extent the kernel reads or
    // writes for `pairs * 2` pixels (checked above), and `c` holds five
    // coefficients.
    unsafe {
        vmx_bgra_row_neon(
            luma.as_ptr(),
            blue.as_ptr(),
            red.as_ptr(),
            pairs * 2,
            out.as_mut_ptr(),
            c.as_ptr(),
        );
    }
}
