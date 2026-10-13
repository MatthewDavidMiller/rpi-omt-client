//! The cryptographic primitives the Web frontend and the deployer use, all
//! from OpenSSL's libcrypto: the one third-party library the project links.
//! Nothing here implements a primitive; it only adapts OpenSSL's EVP
//! interfaces to safe Rust.
//!
//! The FFI calls live in omt-sys; this crate forbids `unsafe` and reaches
//! them through the safe wrappers in `omt_sys::openssl::safe`.

#![forbid(unsafe_code)]

use omt_sys::openssl::safe;

pub const SHA256_LEN: usize = 32;
pub const SHA512_LEN: usize = 64;

/// OpenSSL reported a failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CryptoError;

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cryptographic operation failed")
    }
}

impl std::error::Error for CryptoError {}

pub type Result<T> = std::result::Result<T, CryptoError>;

pub fn sha256(data: &[u8]) -> Result<[u8; SHA256_LEN]> {
    safe::digest(safe::Md::Sha256, data).ok_or(CryptoError)
}

pub fn sha512(data: &[u8]) -> Result<[u8; SHA512_LEN]> {
    safe::digest(safe::Md::Sha512, data).ok_or(CryptoError)
}

/// The lowercase hex SHA-256 of `data`.
pub fn sha256_hex(data: &[u8]) -> Result<String> {
    sha256(data).map(|d| hex(&d))
}

pub fn hmac_sha256(key: &[u8], data: &[u8]) -> Result<[u8; SHA256_LEN]> {
    safe::hmac(safe::Md::Sha256, key, data).ok_or(CryptoError)
}

pub fn hmac_sha256_hex(key: &[u8], data: &[u8]) -> Result<String> {
    hmac_sha256(key, data).map(|d| hex(&d))
}

pub fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32, out: &mut [u8]) -> Result<()> {
    safe::pbkdf2(safe::Md::Sha256, password, salt, iterations, out).then_some(()).ok_or(CryptoError)
}

pub fn pbkdf2_sha1(password: &[u8], salt: &[u8], iterations: u32, out: &mut [u8]) -> Result<()> {
    safe::pbkdf2(safe::Md::Sha1, password, salt, iterations, out).then_some(()).ok_or(CryptoError)
}

/// scrypt with cost `n` (a power of two). Parameters are checked against
/// the bounds the Rust scrypt crate enforced before any work is done:
/// r and p non-zero, r*p < 2^30, and an output of 10-64 bytes.
pub fn scrypt(password: &[u8], salt: &[u8], n: u64, r: u32, p: u32, out: &mut [u8]) -> Result<()> {
    if n < 2
        || !n.is_power_of_two()
        || r == 0
        || p == 0
        || u64::from(r) * u64::from(p) >= 1 << 30
        || !(10..=64).contains(&out.len())
    {
        return Err(CryptoError);
    }
    // 128 * r * N bytes of working memory, which OpenSSL also bounds. Allow
    // up to 1 GiB so the Werkzeug default (N=32768, r=8: 32 MiB) fits.
    let memory = 128_u64
        .checked_mul(u64::from(r))
        .and_then(|m| m.checked_mul(n))
        .and_then(|m| m.checked_mul(2))
        .and_then(|m| m.checked_add(128 * u64::from(r) * u64::from(p)))
        .ok_or(CryptoError)?;
    if memory > 1 << 30 {
        return Err(CryptoError);
    }
    safe::scrypt(password, salt, n, u64::from(r), u64::from(p), memory, out)
        .then_some(())
        .ok_or(CryptoError)
}

/// Constant-time comparison; lengths are compared first.
pub fn ct_equal(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && safe::memcmp_equal(a, b)
}

/// Wipes memory in a way the compiler cannot elide.
pub fn cleanse(buf: &mut [u8]) {
    safe::cleanse(buf);
}

/// Incremental SHA-256 for streams too large to buffer.
#[derive(Debug)]
pub struct Sha256(safe::MdCtx);

impl Sha256 {
    pub fn new() -> Result<Self> {
        safe::MdCtx::new(safe::Md::Sha256).map(Self).ok_or(CryptoError)
    }

    pub fn update(&mut self, data: &[u8]) -> Result<()> {
        self.0.update(data).then_some(()).ok_or(CryptoError)
    }

    pub fn finish(self) -> Result<[u8; SHA256_LEN]> {
        self.0.finish().ok_or(CryptoError)
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_digests() {
        assert_eq!(
            sha256_hex(b"abc").unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let mut s = Sha256::new().unwrap();
        s.update(b"a").unwrap();
        s.update(b"bc").unwrap();
        assert_eq!(s.finish().unwrap(), sha256(b"abc").unwrap());
        assert_eq!(hex(&sha512(b"").unwrap()[..4]), "cf83e135");
    }

    #[test]
    fn hmac_matches_rfc4231_case_2() {
        assert_eq!(
            hmac_sha256_hex(b"Jefe", b"what do ya want for nothing?").unwrap(),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn pbkdf2_matches_rfc7914() {
        let mut out = [0_u8; 64];
        pbkdf2_sha256(b"passwd", b"salt", 1, &mut out).unwrap();
        assert_eq!(hex(&out[..8]), "55ac046e56e3089f");
        assert!(pbkdf2_sha256(b"p", b"s", 0, &mut out).is_err());
    }

    #[test]
    fn scrypt_matches_rfc7914_and_bounds_parameters() {
        let mut out = [0_u8; 64];
        scrypt(b"password", b"NaCl", 1024, 8, 16, &mut out).unwrap();
        assert_eq!(hex(&out[..8]), "fdbabe1c9d347200");
        assert!(scrypt(b"p", b"s", 1000, 8, 1, &mut out).is_err());
        assert!(scrypt(b"p", b"s", 1 << 30, 8, 1, &mut out).is_err());
        assert!(scrypt(b"p", b"s", 1024, 8, 1, &mut out[..9]).is_err());
    }

    #[test]
    fn constant_time_equality() {
        assert!(ct_equal(b"abc", b"abc"));
        assert!(!ct_equal(b"abc", b"abd"));
        assert!(!ct_equal(b"abc", b"ab"));
        assert!(ct_equal(b"", b""));
    }
}
