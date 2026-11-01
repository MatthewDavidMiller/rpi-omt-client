//! Everything the SSH client parses from a server or a file: known_hosts,
//! private keys, public key and signature blobs, EXT_INFO, and the channel
//! messages a session dispatches. The first byte picks the parser.

#![no_main]

use omt_common::buf::Cursor;
use omt_deploy::ssh::channel::dispatch_detached;
use omt_deploy::ssh::keys::{PrivateKey, PublicKey};
use omt_deploy::ssh::known_hosts;
use omt_deploy::ssh::wire;

const BLOB: [u8; 51] = [
    0, 0, 0, 11, b's', b's', b'h', b'-', b'e', b'd', b'2', b'5', b'5', b'1', b'9', 0, 0, 0, 32, 1,
    2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27,
    28, 29, 30, 31, 32,
];

fn public_key(data: &[u8]) {
    let mut r = Cursor::new(data);
    let Some(blob) = wire::string(&mut r) else { return };
    if let Ok(key) = PublicKey::parse(blob) {
        for alg in ["ssh-ed25519", "ecdsa-sha2-nistp256", "rsa-sha2-512"] {
            let _ = key.verify(alg, r.rest(), b"h");
        }
    }
}

fn channel_messages(data: &[u8]) {
    // Messages are length-prefixed in the input.
    let mut r = Cursor::new(data);
    let mut messages = Vec::new();
    while let Some(m) = wire::string(&mut r) {
        if m.is_empty() {
            break;
        }
        messages.push(m);
    }
    dispatch_detached(&messages);
}

fn readers(data: &[u8]) {
    let _ = wire::base64_decode(data, true);
    let mut r = Cursor::new(data);
    while r.remaining() > 0 {
        if wire::bn(&mut r, false).is_none() && wire::string(&mut r).is_none() {
            break;
        }
    }
}

/// # Safety
///
/// Called by libFuzzer with `size` readable bytes at `data`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn LLVMFuzzerTestOneInput(data: *const u8, size: usize) -> i32 {
    // SAFETY: libFuzzer's contract for this call.
    let input = unsafe { omt_fuzz::input(data, size) };
    let Some((&selector, rest)) = input.split_first() else { return 0 };
    match selector % 5 {
        0 => {
            let mut types = String::new();
            known_hosts::parse(rest, "pi.local", 2222, &BLOB, Some(&mut types));
        }
        1 => {
            let _ = PrivateKey::parse(rest, b"");
        }
        2 => public_key(rest),
        3 => channel_messages(rest),
        _ => readers(rest),
    }
    0
}
