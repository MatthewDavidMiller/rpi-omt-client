//! OpenSSL 3 (libcrypto and libssl) declarations.
//!
//! Every type is opaque: the project only ever holds OpenSSL objects by
//! pointer. Macros in the C headers -- `SSL_CTX_set_min_proto_version`,
//! `SSL_set_tlsext_host_name`, `OPENSSL_free`, and the like -- are not
//! symbols, so their expansions are written out here as functions over the
//! calls they expand to.

use std::ffi::{c_char, c_int, c_uint, c_void};

opaque!(EVP_MD, EVP_MD_CTX, ENGINE);

unsafe extern "C" {
    pub fn EVP_sha1() -> *const EVP_MD;
    pub fn EVP_sha256() -> *const EVP_MD;
    pub fn EVP_sha384() -> *const EVP_MD;
    pub fn EVP_sha512() -> *const EVP_MD;

    pub fn EVP_Digest(
        data: *const c_void,
        count: usize,
        md: *mut u8,
        size: *mut c_uint,
        kind: *const EVP_MD,
        engine: *mut ENGINE,
    ) -> c_int;
    pub fn EVP_MD_CTX_new() -> *mut EVP_MD_CTX;
    pub fn EVP_MD_CTX_free(ctx: *mut EVP_MD_CTX);
    pub fn EVP_DigestInit_ex(
        ctx: *mut EVP_MD_CTX,
        kind: *const EVP_MD,
        engine: *mut ENGINE,
    ) -> c_int;
    pub fn EVP_DigestUpdate(ctx: *mut EVP_MD_CTX, data: *const c_void, len: usize) -> c_int;
    pub fn EVP_DigestFinal_ex(ctx: *mut EVP_MD_CTX, md: *mut u8, size: *mut c_uint) -> c_int;

    pub fn HMAC(
        md: *const EVP_MD,
        key: *const c_void,
        key_len: c_int,
        data: *const u8,
        data_len: usize,
        out: *mut u8,
        out_len: *mut c_uint,
    ) -> *mut u8;
    pub fn PKCS5_PBKDF2_HMAC(
        pass: *const c_char,
        pass_len: c_int,
        salt: *const u8,
        salt_len: c_int,
        iterations: c_int,
        digest: *const EVP_MD,
        key_len: c_int,
        out: *mut u8,
    ) -> c_int;
    pub fn EVP_PBE_scrypt(
        pass: *const c_char,
        pass_len: usize,
        salt: *const u8,
        salt_len: usize,
        n: u64,
        r: u64,
        p: u64,
        max_mem: u64,
        key: *mut u8,
        key_len: usize,
    ) -> c_int;

    pub fn CRYPTO_memcmp(a: *const c_void, b: *const c_void, len: usize) -> c_int;
    pub fn OPENSSL_cleanse(ptr: *mut c_void, len: usize);
}

/// Safe wrappers over the declarations above. Each checks the lengths
/// OpenSSL's `int` parameters can carry and reports failure as `None` or
/// `false`.
pub mod safe {
    use super::{
        CRYPTO_memcmp, EVP_Digest, EVP_DigestFinal_ex, EVP_DigestInit_ex, EVP_DigestUpdate, EVP_MD,
        EVP_MD_CTX, EVP_MD_CTX_free, EVP_MD_CTX_new, EVP_PBE_scrypt, EVP_sha1, EVP_sha256,
        EVP_sha384, EVP_sha512, HMAC, OPENSSL_cleanse, PKCS5_PBKDF2_HMAC,
    };
    use std::ffi::{c_int, c_uint};
    use std::ptr;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Md {
        Sha1,
        Sha256,
        Sha384,
        Sha512,
    }

    impl Md {
        pub fn evp(self) -> *const EVP_MD {
            match self {
                // SAFETY: the digest getters take no arguments and return
                // static objects.
                Md::Sha1 => unsafe { EVP_sha1() },
                // SAFETY: as above.
                Md::Sha256 => unsafe { EVP_sha256() },
                // SAFETY: as above.
                Md::Sha384 => unsafe { EVP_sha384() },
                // SAFETY: as above.
                Md::Sha512 => unsafe { EVP_sha512() },
            }
        }
    }

    /// A one-shot digest of `N` bytes; `None` unless OpenSSL produced exactly
    /// `N`.
    pub fn digest<const N: usize>(md: Md, data: &[u8]) -> Option<[u8; N]> {
        let mut out = [0_u8; N];
        let mut n: c_uint = 0;
        // SAFETY: `data` is readable for its length, `out` holds N bytes,
        // which is at least the size of every digest requested through Md.
        let ok = unsafe {
            EVP_Digest(
                data.as_ptr().cast(),
                data.len(),
                out.as_mut_ptr(),
                &raw mut n,
                md.evp(),
                ptr::null_mut(),
            )
        };
        (ok == 1 && n as usize == N).then_some(out)
    }

    pub fn hmac<const N: usize>(md: Md, key: &[u8], data: &[u8]) -> Option<[u8; N]> {
        let key_len = c_int::try_from(key.len()).ok()?;
        let mut out = [0_u8; 64];
        let mut n: c_uint = 0;
        // SAFETY: key and data are readable for their lengths; `out` holds
        // 64 bytes, the largest digest Md names.
        let ret = unsafe {
            HMAC(
                md.evp(),
                key.as_ptr().cast(),
                key_len,
                data.as_ptr(),
                data.len(),
                out.as_mut_ptr(),
                &raw mut n,
            )
        };
        if ret.is_null() || n as usize != N {
            return None;
        }
        out[..N].try_into().ok()
    }

    pub fn pbkdf2(md: Md, password: &[u8], salt: &[u8], iterations: u32, out: &mut [u8]) -> bool {
        let (Ok(plen), Ok(slen), Ok(iter), Ok(olen)) = (
            c_int::try_from(password.len()),
            c_int::try_from(salt.len()),
            c_int::try_from(iterations),
            c_int::try_from(out.len()),
        ) else {
            return false;
        };
        if iter == 0 {
            return false;
        }
        // SAFETY: every buffer is valid for the length passed with it.
        let ok = unsafe {
            PKCS5_PBKDF2_HMAC(
                password.as_ptr().cast(),
                plen,
                salt.as_ptr(),
                slen,
                iter,
                md.evp(),
                olen,
                out.as_mut_ptr(),
            )
        };
        ok == 1
    }

    pub fn scrypt(
        password: &[u8],
        salt: &[u8],
        n: u64,
        r: u64,
        p: u64,
        max_mem: u64,
        out: &mut [u8],
    ) -> bool {
        // SAFETY: every buffer is valid for the length passed with it.
        let ok = unsafe {
            EVP_PBE_scrypt(
                password.as_ptr().cast(),
                password.len(),
                salt.as_ptr(),
                salt.len(),
                n,
                r,
                p,
                max_mem,
                out.as_mut_ptr(),
                out.len(),
            )
        };
        ok == 1
    }

    /// Constant-time equality of two slices of the same length.
    pub fn memcmp_equal(a: &[u8], b: &[u8]) -> bool {
        if a.len() != b.len() {
            return false;
        }
        if a.is_empty() {
            return true;
        }
        // SAFETY: both slices are readable for `a.len()` bytes.
        unsafe { CRYPTO_memcmp(a.as_ptr().cast(), b.as_ptr().cast(), a.len()) == 0 }
    }

    pub fn cleanse(buf: &mut [u8]) {
        if !buf.is_empty() {
            // SAFETY: `buf` is writable for its length.
            unsafe { OPENSSL_cleanse(buf.as_mut_ptr().cast(), buf.len()) }
        }
    }

    /// An owned digest context.
    #[derive(Debug)]
    pub struct MdCtx(*mut EVP_MD_CTX);

    // SAFETY: an EVP_MD_CTX is owned exclusively by this handle and OpenSSL
    // permits using a context from any one thread at a time.
    unsafe impl Send for MdCtx {}

    impl MdCtx {
        pub fn new(md: Md) -> Option<Self> {
            // SAFETY: EVP_MD_CTX_new has no preconditions.
            let ctx = unsafe { EVP_MD_CTX_new() };
            if ctx.is_null() {
                return None;
            }
            let this = Self(ctx);
            // SAFETY: `ctx` is a fresh, valid context.
            (unsafe { EVP_DigestInit_ex(this.0, md.evp(), ptr::null_mut()) } == 1).then_some(this)
        }

        pub fn update(&mut self, data: &[u8]) -> bool {
            // SAFETY: the context is valid and `data` readable for its length.
            unsafe { EVP_DigestUpdate(self.0, data.as_ptr().cast(), data.len()) == 1 }
        }

        pub fn finish<const N: usize>(self) -> Option<[u8; N]> {
            let mut out = [0_u8; 64];
            let mut n: c_uint = 0;
            // SAFETY: the context is valid and `out` holds the largest digest.
            let ok = unsafe { EVP_DigestFinal_ex(self.0, out.as_mut_ptr(), &raw mut n) };
            if ok != 1 || n as usize != N {
                return None;
            }
            out[..N].try_into().ok()
        }
    }

    impl Drop for MdCtx {
        fn drop(&mut self) {
            // SAFETY: the context was created by EVP_MD_CTX_new and is freed
            // once.
            unsafe { EVP_MD_CTX_free(self.0) }
        }
    }
}
