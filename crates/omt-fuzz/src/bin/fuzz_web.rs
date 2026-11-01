//! Everything an unauthenticated client or a damaged file can hand the Web
//! frontend: HTTP request heads, form bodies, cookie headers, the OMT
//! settings XML, saved-state records, and stored password hashes.

#![no_main]

use omt_web::app::{Form, cookie_value};
use omt_web::auth::Password;
use omt_web::diagnostics::parse_record;
use omt_web::http::{Parse, Request, parse_head};
use omt_web::network::{normalize_server, parse_server, update_settings_xml};
use omt_web::playback::{parse_discovered, status_record_valid};

/// # Safety
///
/// Called by libFuzzer with `size` readable bytes at `data`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn LLVMFuzzerTestOneInput(data: *const u8, size: usize) -> i32 {
    // SAFETY: libFuzzer's contract for this call.
    let input = unsafe { omt_fuzz::input(data, size) };
    if let Parse::Complete(head) = parse_head(input) {
        let req = Request::new(&head, input, "127.0.0.1");
        let _ = (req.header("cookie"), req.header("expect"));
    }
    let _ = Form::parse(input);
    let _ = parse_server(input);
    let _ = update_settings_xml(input, "omt://example.com:6399");
    // Password parsing only: verification would spend the fuzzer's time in
    // PBKDF2 and scrypt rather than in parsing.
    let _ = Password::parse(input);
    if let Ok(text) = std::str::from_utf8(input) {
        let _ = cookie_value(text, "__Host-omt_session", 127);
        let _ = normalize_server(text);
        let _ = parse_discovered(text);
        let _ = status_record_valid(text, "Camera", 5);
        let _ = parse_record(text, &["version", "request_id", "status"], true);
    }
    0
}
