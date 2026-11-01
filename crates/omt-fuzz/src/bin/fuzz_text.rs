//! The text parsers: strict JSON, the bounded XML reader, NFC, and lossy
//! UTF-8 decoding.

#![no_main]

use omt_common::{json, nfc, xml};

/// # Safety
///
/// Called by libFuzzer with `size` readable bytes at `data`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn LLVMFuzzerTestOneInput(data: *const u8, size: usize) -> i32 {
    // SAFETY: libFuzzer's contract for this call.
    let input = unsafe { omt_fuzz::input(data, size) };
    let _ = json::parse(input);
    let _ = xml::unique_texts(input, &["Name", "Removed", "IPAddress", "Port"]);
    let _ = xml::root_is(input, "Settings");
    if let Ok(text) = std::str::from_utf8(input) {
        let _ = omt_receiver::discovery::read_announcement(text.as_bytes());
        let _ = nfc::normalize(text);
        let _ = nfc::is_normalized(text, 1024);
    }
    let _ = String::from_utf8_lossy(input);
    0
}
