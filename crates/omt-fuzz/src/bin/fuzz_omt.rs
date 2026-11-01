//! The OMT wire headers and the target grammar.

#![no_main]

use omt_protocol::{
    HEADER_SIZE, is_valid_source_name, parse_audio_header, parse_direct_target, parse_frame_header,
    parse_video_header,
};

/// # Safety
///
/// Called by libFuzzer with `size` readable bytes at `data`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn LLVMFuzzerTestOneInput(data: *const u8, size: usize) -> i32 {
    // SAFETY: libFuzzer's contract for this call.
    let input = unsafe { omt_fuzz::input(data, size) };
    if let Ok(h) = parse_frame_header(input)
        && input.len() > HEADER_SIZE
    {
        let _ = parse_video_header(&h, &input[HEADER_SIZE..]);
        let _ = parse_audio_header(&h, &input[HEADER_SIZE..]);
    }
    if let Ok(text) = std::str::from_utf8(input) {
        let _ = parse_direct_target(text);
        let _ = is_valid_source_name(text);
    }
    0
}
