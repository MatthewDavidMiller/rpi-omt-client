//! Shared plumbing for the libFuzzer targets in src/bin. Each target is the
//! Rust counterpart of a tests/fuzz harness and reads the same committed
//! corpus under tests/fuzz/corpus.

/// The input libFuzzer hands a target.
///
/// # Safety
///
/// `data` must point to `size` readable bytes that stay valid for the
/// returned lifetime -- what libFuzzer guarantees for one call of
/// `LLVMFuzzerTestOneInput`.
pub unsafe fn input<'a>(data: *const u8, size: usize) -> &'a [u8] {
    if data.is_null() || size == 0 {
        return &[];
    }
    // SAFETY: the caller's guarantee.
    unsafe { std::slice::from_raw_parts(data, size) }
}
