//! SSH data types (RFC 4251 section 5). Every reader checks the remaining
//! length before it reads and returns `None` instead of reading past the
//! end.

use omt_common::buf::Cursor;
use omt_sys::evp::safe::Bn;

/// Appending SSH-encoded values.
pub trait Put {
    fn put_u8(&mut self, v: u8);
    fn put_bool(&mut self, v: bool);
    fn put_u32(&mut self, v: u32);
    fn put_u64(&mut self, v: u64);
    /// A length-prefixed string. Lengths beyond u32 cannot occur: every
    /// packet is capped far below that before it is sent.
    fn put_string(&mut self, data: &[u8]);
    /// An mpint from an unsigned big-endian magnitude.
    fn put_mpint(&mut self, be: &[u8]);
}

impl Put for Vec<u8> {
    fn put_u8(&mut self, v: u8) {
        self.push(v);
    }

    fn put_bool(&mut self, v: bool) {
        self.push(u8::from(v));
    }

    fn put_u32(&mut self, v: u32) {
        self.extend_from_slice(&v.to_be_bytes());
    }

    fn put_u64(&mut self, v: u64) {
        self.extend_from_slice(&v.to_be_bytes());
    }

    fn put_string(&mut self, data: &[u8]) {
        self.put_u32(u32::try_from(data.len()).unwrap_or(u32::MAX));
        self.extend_from_slice(data);
    }

    fn put_mpint(&mut self, be: &[u8]) {
        let start = be.iter().position(|&b| b != 0).unwrap_or(be.len());
        let be = &be[start..];
        let pad = be.first().is_some_and(|&b| b & 0x80 != 0);
        self.put_u32(u32::try_from(be.len() + usize::from(pad)).unwrap_or(u32::MAX));
        if pad {
            self.push(0);
        }
        self.extend_from_slice(be);
    }
}

/// An mpint from a non-negative big number.
pub fn put_bn(out: &mut Vec<u8>, bn: &Bn) -> bool {
    if bn.is_negative() {
        return false;
    }
    let mut raw = bn.to_bytes();
    out.put_mpint(&raw);
    omt_common::buf::wipe_vec(&mut raw);
    true
}

pub fn u8(c: &mut Cursor<'_>) -> Option<u8> {
    c.u8()
}

pub fn boolean(c: &mut Cursor<'_>) -> Option<bool> {
    c.u8().map(|v| v != 0)
}

pub fn u32(c: &mut Cursor<'_>) -> Option<u32> {
    c.be32()
}

pub fn u64(c: &mut Cursor<'_>) -> Option<u64> {
    c.be64()
}

pub fn string<'a>(c: &mut Cursor<'a>) -> Option<&'a [u8]> {
    let len = usize::try_from(c.be32()?).ok()?;
    c.take(len)
}

/// A non-negative mpint as its magnitude, leading zero bytes removed (as
/// OpenSSH trims rather than refuses them).
pub fn mpint<'a>(c: &mut Cursor<'a>) -> Option<&'a [u8]> {
    let raw = string(c)?;
    if raw.first().is_some_and(|&b| b & 0x80 != 0) {
        return None; // negative
    }
    let start = raw.iter().position(|&b| b != 0).unwrap_or(raw.len());
    Some(&raw[start..])
}

/// A non-negative mpint of at most 2048 bytes as a big number.
pub fn bn(c: &mut Cursor<'_>, secret: bool) -> Option<Bn> {
    let mag = mpint(c)?;
    if mag.len() > 2048 {
        return None;
    }
    Bn::from_bytes(mag, secret)
}

/// Whether `name` is an entry of a comma-separated name-list.
pub fn namelist_has(list: &[u8], name: &str) -> bool {
    list.split(|&b| b == b',').any(|entry| entry == name.as_bytes())
}

/// The first entry of a name-list.
pub fn first_name(list: &[u8]) -> &[u8] {
    list.split(|&b| b == b',').next().unwrap_or(list)
}

fn b64_value(c: u8) -> Option<u32> {
    match c {
        b'A'..=b'Z' => Some(u32::from(c - b'A')),
        b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
        b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Strict base64 (RFC 4648 with padding); whitespace is skipped on request.
pub fn base64_decode(text: &[u8], skip_space: bool) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut acc, mut bits, mut pad, mut symbols) = (0_u32, 0_u32, 0_usize, 0_usize);
    for &c in text {
        if skip_space && matches!(c, b' ' | b'\t' | b'\r' | b'\n') {
            continue;
        }
        if c == b'=' {
            pad += 1;
            symbols += 1;
            continue;
        }
        let v = b64_value(c)?;
        if pad > 0 {
            return None;
        }
        symbols += 1;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    (pad <= 2 && symbols % 4 == 0 && acc == 0).then_some(out)
}

/// Whether `needle` occurs in `haystack` as a complete byte sequence.
pub fn contains_bytes(haystack: &[u8], needle: &str) -> bool {
    let n = needle.as_bytes();
    !n.is_empty() && n.len() <= haystack.len() && haystack.windows(n.len()).any(|w| w == n)
}
