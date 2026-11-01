//! The D-Bus message parser and body reader the Avahi browse runs on every
//! message from the bus.

#![no_main]

use omt_receiver::dbus::{Message, message_length};

/// # Safety
///
/// Called by libFuzzer with `size` readable bytes at `data`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn LLVMFuzzerTestOneInput(data: *const u8, size: usize) -> i32 {
    // SAFETY: libFuzzer's contract for this call.
    let input = unsafe { omt_fuzz::input(data, size) };
    let Some(total) = message_length(input).filter(|&t| t <= input.len()) else { return 0 };
    let Some(m) = Message::parse(input[..total].to_vec()) else { return 0 };
    let mut r = m.reader();
    let _ = (r.i32(), r.i32(), r.string(), r.string(), r.u16());
    r.skip_aay();
    let _ = r.bool();
    let _ = (m.path(), m.interface(), m.member(), m.signature());
    0
}
