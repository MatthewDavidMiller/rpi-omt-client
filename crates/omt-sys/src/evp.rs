//! OpenSSL 3 libcrypto beyond digests: symmetric ciphers, MACs, asymmetric
//! keys (key agreement, KEM decapsulation, signatures), big numbers, and the
//! parameter builder that assembles keys from their components. The SSH
//! client is built from these.

use std::ffi::{c_char, c_int, c_long, c_uint, c_void};

use crate::openssl::{ENGINE, EVP_MD_CTX};

opaque!(
    EVP_CIPHER,
    EVP_CIPHER_CTX,
    EVP_MAC,
    EVP_MAC_CTX,
    EVP_PKEY,
    EVP_PKEY_CTX,
    OSSL_LIB_CTX,
    OSSL_PARAM_BLD,
    BIGNUM,
    BN_CTX,
    BN_MONT_CTX,
    ECDSA_SIG,
    BIO,
);

/// One element of an OpenSSL parameter array.
#[repr(C)]
#[derive(Debug)]
pub struct OSSL_PARAM {
    pub key: *const c_char,
    pub data_type: c_uint,
    pub data: *mut c_void,
    pub data_size: usize,
    pub return_size: usize,
}

pub const EVP_CTRL_GCM_SET_IVLEN: c_int = 0x9;
pub const EVP_CTRL_GCM_GET_TAG: c_int = 0x10;
pub const EVP_CTRL_GCM_SET_TAG: c_int = 0x11;

pub const EVP_PKEY_RSA: c_int = 6;
pub const EVP_PKEY_EC: c_int = 408;
pub const EVP_PKEY_X25519: c_int = 1034;
pub const EVP_PKEY_ED25519: c_int = 1087;

/// Key-selection bits for `EVP_PKEY_fromdata`.
pub const EVP_PKEY_PUBLIC_KEY: c_int = 0x86;
pub const EVP_PKEY_KEYPAIR: c_int = 0x87;

pub const BN_FLG_CONSTTIME: c_int = 0x04;
pub const BN_RAND_TOP_ONE: c_int = 0;
pub const BN_RAND_BOTTOM_ANY: c_int = 0;

pub type pem_password_cb =
    unsafe extern "C" fn(buf: *mut c_char, size: c_int, rwflag: c_int, user: *mut c_void) -> c_int;

unsafe extern "C" {
    // Ciphers.
    pub fn EVP_aes_128_ctr() -> *const EVP_CIPHER;
    pub fn EVP_aes_192_ctr() -> *const EVP_CIPHER;
    pub fn EVP_aes_256_ctr() -> *const EVP_CIPHER;
    pub fn EVP_aes_128_gcm() -> *const EVP_CIPHER;
    pub fn EVP_aes_256_gcm() -> *const EVP_CIPHER;
    pub fn EVP_chacha20() -> *const EVP_CIPHER;
    pub fn EVP_CIPHER_fetch(
        ctx: *mut OSSL_LIB_CTX,
        algorithm: *const c_char,
        properties: *const c_char,
    ) -> *mut EVP_CIPHER;
    pub fn EVP_CIPHER_free(cipher: *mut EVP_CIPHER);
    pub fn EVP_CIPHER_CTX_new() -> *mut EVP_CIPHER_CTX;
    pub fn EVP_CIPHER_CTX_free(ctx: *mut EVP_CIPHER_CTX);
    pub fn EVP_CipherInit_ex(
        ctx: *mut EVP_CIPHER_CTX,
        cipher: *const EVP_CIPHER,
        engine: *mut ENGINE,
        key: *const u8,
        iv: *const u8,
        enc: c_int,
    ) -> c_int;
    pub fn EVP_CipherUpdate(
        ctx: *mut EVP_CIPHER_CTX,
        out: *mut u8,
        outl: *mut c_int,
        input: *const u8,
        inl: c_int,
    ) -> c_int;
    pub fn EVP_CipherFinal_ex(ctx: *mut EVP_CIPHER_CTX, out: *mut u8, outl: *mut c_int) -> c_int;
    pub fn EVP_CIPHER_CTX_ctrl(
        ctx: *mut EVP_CIPHER_CTX,
        kind: c_int,
        arg: c_int,
        ptr: *mut c_void,
    ) -> c_int;
    pub fn EVP_CIPHER_CTX_set_padding(ctx: *mut EVP_CIPHER_CTX, pad: c_int) -> c_int;
    pub fn EVP_CIPHER_CTX_get_block_size(ctx: *const EVP_CIPHER_CTX) -> c_int;

    // MACs.
    pub fn EVP_MAC_fetch(
        ctx: *mut OSSL_LIB_CTX,
        algorithm: *const c_char,
        properties: *const c_char,
    ) -> *mut EVP_MAC;
    pub fn EVP_MAC_free(mac: *mut EVP_MAC);
    pub fn EVP_MAC_CTX_new(mac: *mut EVP_MAC) -> *mut EVP_MAC_CTX;
    pub fn EVP_MAC_CTX_free(ctx: *mut EVP_MAC_CTX);
    pub fn EVP_MAC_init(
        ctx: *mut EVP_MAC_CTX,
        key: *const u8,
        keylen: usize,
        params: *const OSSL_PARAM,
    ) -> c_int;
    pub fn EVP_MAC_update(ctx: *mut EVP_MAC_CTX, data: *const u8, datalen: usize) -> c_int;
    pub fn EVP_MAC_final(
        ctx: *mut EVP_MAC_CTX,
        out: *mut u8,
        outl: *mut usize,
        outsize: usize,
    ) -> c_int;
    pub fn OSSL_PARAM_construct_utf8_string(
        key: *const c_char,
        buf: *mut c_char,
        bsize: usize,
    ) -> OSSL_PARAM;
    pub fn OSSL_PARAM_construct_end() -> OSSL_PARAM;

    // Asymmetric keys.
    pub fn EVP_PKEY_free(pkey: *mut EVP_PKEY);
    pub fn EVP_PKEY_new_raw_public_key(
        kind: c_int,
        engine: *mut ENGINE,
        key: *const u8,
        len: usize,
    ) -> *mut EVP_PKEY;
    pub fn EVP_PKEY_new_raw_private_key(
        kind: c_int,
        engine: *mut ENGINE,
        key: *const u8,
        len: usize,
    ) -> *mut EVP_PKEY;
    pub fn EVP_PKEY_get_raw_public_key(
        pkey: *const EVP_PKEY,
        out: *mut u8,
        len: *mut usize,
    ) -> c_int;
    pub fn EVP_PKEY_get_octet_string_param(
        pkey: *const EVP_PKEY,
        key_name: *const c_char,
        buf: *mut u8,
        max_buf_sz: usize,
        out_len: *mut usize,
    ) -> c_int;
    pub fn EVP_PKEY_get_utf8_string_param(
        pkey: *const EVP_PKEY,
        key_name: *const c_char,
        out: *mut c_char,
        max_buf_sz: usize,
        out_len: *mut usize,
    ) -> c_int;
    pub fn EVP_PKEY_set_utf8_string_param(
        pkey: *mut EVP_PKEY,
        key_name: *const c_char,
        value: *const c_char,
    ) -> c_int;
    pub fn EVP_PKEY_get_bn_param(
        pkey: *const EVP_PKEY,
        key_name: *const c_char,
        bn: *mut *mut BIGNUM,
    ) -> c_int;
    pub fn EVP_PKEY_get_base_id(pkey: *const EVP_PKEY) -> c_int;
    pub fn EVP_PKEY_get_size(pkey: *const EVP_PKEY) -> c_int;
    pub fn EVP_PKEY_CTX_new(pkey: *mut EVP_PKEY, engine: *mut ENGINE) -> *mut EVP_PKEY_CTX;
    pub fn EVP_PKEY_CTX_new_from_name(
        ctx: *mut OSSL_LIB_CTX,
        name: *const c_char,
        propq: *const c_char,
    ) -> *mut EVP_PKEY_CTX;
    pub fn EVP_PKEY_CTX_new_from_pkey(
        ctx: *mut OSSL_LIB_CTX,
        pkey: *mut EVP_PKEY,
        propq: *const c_char,
    ) -> *mut EVP_PKEY_CTX;
    pub fn EVP_PKEY_CTX_free(ctx: *mut EVP_PKEY_CTX);
    pub fn EVP_PKEY_keygen_init(ctx: *mut EVP_PKEY_CTX) -> c_int;
    pub fn EVP_PKEY_generate(ctx: *mut EVP_PKEY_CTX, pkey: *mut *mut EVP_PKEY) -> c_int;
    pub fn EVP_PKEY_derive_init(ctx: *mut EVP_PKEY_CTX) -> c_int;
    pub fn EVP_PKEY_derive_set_peer(ctx: *mut EVP_PKEY_CTX, peer: *mut EVP_PKEY) -> c_int;
    pub fn EVP_PKEY_derive(ctx: *mut EVP_PKEY_CTX, key: *mut u8, keylen: *mut usize) -> c_int;
    pub fn EVP_PKEY_decapsulate_init(ctx: *mut EVP_PKEY_CTX, params: *const OSSL_PARAM) -> c_int;
    pub fn EVP_PKEY_decapsulate(
        ctx: *mut EVP_PKEY_CTX,
        unwrapped: *mut u8,
        unwrappedlen: *mut usize,
        wrapped: *const u8,
        wrappedlen: usize,
    ) -> c_int;
    pub fn EVP_PKEY_fromdata_init(ctx: *mut EVP_PKEY_CTX) -> c_int;
    pub fn EVP_PKEY_fromdata(
        ctx: *mut EVP_PKEY_CTX,
        pkey: *mut *mut EVP_PKEY,
        selection: c_int,
        params: *mut OSSL_PARAM,
    ) -> c_int;
    pub fn EVP_PKEY_public_check(ctx: *mut EVP_PKEY_CTX) -> c_int;
    pub fn EVP_DigestSignInit_ex(
        ctx: *mut EVP_MD_CTX,
        pctx: *mut *mut EVP_PKEY_CTX,
        mdname: *const c_char,
        libctx: *mut OSSL_LIB_CTX,
        props: *const c_char,
        pkey: *mut EVP_PKEY,
        params: *const OSSL_PARAM,
    ) -> c_int;
    pub fn EVP_DigestSign(
        ctx: *mut EVP_MD_CTX,
        sig: *mut u8,
        siglen: *mut usize,
        tbs: *const u8,
        tbslen: usize,
    ) -> c_int;
    pub fn EVP_DigestVerifyInit_ex(
        ctx: *mut EVP_MD_CTX,
        pctx: *mut *mut EVP_PKEY_CTX,
        mdname: *const c_char,
        libctx: *mut OSSL_LIB_CTX,
        props: *const c_char,
        pkey: *mut EVP_PKEY,
        params: *const OSSL_PARAM,
    ) -> c_int;
    pub fn EVP_DigestVerify(
        ctx: *mut EVP_MD_CTX,
        sig: *const u8,
        siglen: usize,
        tbs: *const u8,
        tbslen: usize,
    ) -> c_int;

    // The parameter builder.
    pub fn OSSL_PARAM_BLD_new() -> *mut OSSL_PARAM_BLD;
    pub fn OSSL_PARAM_BLD_free(bld: *mut OSSL_PARAM_BLD);
    pub fn OSSL_PARAM_BLD_push_utf8_string(
        bld: *mut OSSL_PARAM_BLD,
        key: *const c_char,
        buf: *const c_char,
        bsize: usize,
    ) -> c_int;
    pub fn OSSL_PARAM_BLD_push_octet_string(
        bld: *mut OSSL_PARAM_BLD,
        key: *const c_char,
        buf: *const c_void,
        bsize: usize,
    ) -> c_int;
    pub fn OSSL_PARAM_BLD_push_BN(
        bld: *mut OSSL_PARAM_BLD,
        key: *const c_char,
        bn: *const BIGNUM,
    ) -> c_int;
    pub fn OSSL_PARAM_BLD_to_param(bld: *mut OSSL_PARAM_BLD) -> *mut OSSL_PARAM;
    pub fn OSSL_PARAM_free(params: *mut OSSL_PARAM);

    // Big numbers.
    pub fn BN_new() -> *mut BIGNUM;
    pub fn BN_secure_new() -> *mut BIGNUM;
    pub fn BN_free(a: *mut BIGNUM);
    pub fn BN_clear_free(a: *mut BIGNUM);
    pub fn BN_dup(a: *const BIGNUM) -> *mut BIGNUM;
    pub fn BN_bin2bn(s: *const u8, len: c_int, ret: *mut BIGNUM) -> *mut BIGNUM;
    pub fn BN_bn2bin(a: *const BIGNUM, to: *mut u8) -> c_int;
    pub fn BN_num_bits(a: *const BIGNUM) -> c_int;
    pub fn BN_is_negative(a: *const BIGNUM) -> c_int;
    pub fn BN_is_odd(a: *const BIGNUM) -> c_int;
    pub fn BN_cmp(a: *const BIGNUM, b: *const BIGNUM) -> c_int;
    pub fn BN_value_one() -> *const BIGNUM;
    pub fn BN_sub_word(a: *mut BIGNUM, w: u64) -> c_int;
    pub fn BN_set_word(a: *mut BIGNUM, w: u64) -> c_int;
    pub fn BN_sub(r: *mut BIGNUM, a: *const BIGNUM, b: *const BIGNUM) -> c_int;
    pub fn BN_div(
        dv: *mut BIGNUM,
        rem: *mut BIGNUM,
        m: *const BIGNUM,
        d: *const BIGNUM,
        ctx: *mut BN_CTX,
    ) -> c_int;
    pub fn BN_set_flags(b: *mut BIGNUM, n: c_int);
    pub fn BN_priv_rand_ex(
        rnd: *mut BIGNUM,
        bits: c_int,
        top: c_int,
        bottom: c_int,
        strength: c_uint,
        ctx: *mut BN_CTX,
    ) -> c_int;
    pub fn BN_mod_exp_mont_consttime(
        rr: *mut BIGNUM,
        a: *const BIGNUM,
        p: *const BIGNUM,
        m: *const BIGNUM,
        ctx: *mut BN_CTX,
        in_mont: *mut BN_MONT_CTX,
    ) -> c_int;
    pub fn BN_CTX_new() -> *mut BN_CTX;
    pub fn BN_CTX_free(ctx: *mut BN_CTX);
    pub fn BN_get_rfc3526_prime_2048(bn: *mut BIGNUM) -> *mut BIGNUM;
    pub fn BN_get_rfc3526_prime_3072(bn: *mut BIGNUM) -> *mut BIGNUM;
    pub fn BN_get_rfc3526_prime_4096(bn: *mut BIGNUM) -> *mut BIGNUM;
    pub fn BN_get_rfc3526_prime_6144(bn: *mut BIGNUM) -> *mut BIGNUM;
    pub fn BN_get_rfc3526_prime_8192(bn: *mut BIGNUM) -> *mut BIGNUM;

    // ECDSA signature encoding.
    pub fn ECDSA_SIG_new() -> *mut ECDSA_SIG;
    pub fn ECDSA_SIG_free(sig: *mut ECDSA_SIG);
    pub fn ECDSA_SIG_set0(sig: *mut ECDSA_SIG, r: *mut BIGNUM, s: *mut BIGNUM) -> c_int;
    pub fn ECDSA_SIG_get0_r(sig: *const ECDSA_SIG) -> *const BIGNUM;
    pub fn ECDSA_SIG_get0_s(sig: *const ECDSA_SIG) -> *const BIGNUM;
    pub fn i2d_ECDSA_SIG(sig: *const ECDSA_SIG, pp: *mut *mut u8) -> c_int;
    pub fn d2i_ECDSA_SIG(
        sig: *mut *mut ECDSA_SIG,
        pp: *mut *const u8,
        len: c_long,
    ) -> *mut ECDSA_SIG;
    pub fn OBJ_txt2nid(s: *const c_char) -> c_int;
    pub fn EC_curve_nist2nid(name: *const c_char) -> c_int;

    // PEM private keys.
    pub fn BIO_new_mem_buf(buf: *const c_void, len: c_int) -> *mut BIO;
    pub fn BIO_free(bio: *mut BIO) -> c_int;
    pub fn PEM_read_bio_PrivateKey(
        bio: *mut BIO,
        x: *mut *mut EVP_PKEY,
        cb: Option<pem_password_cb>,
        user: *mut c_void,
    ) -> *mut EVP_PKEY;

    pub fn RAND_bytes(buf: *mut u8, num: c_int) -> c_int;
    pub fn CRYPTO_free(ptr: *mut c_void, file: *const c_char, line: c_int);
}

/// Owned OpenSSL objects with the operations the SSH client performs on
/// them. Every constructor returns `None` (and every operation `false` or
/// `None`) when OpenSSL reports a failure; nothing here aborts.
pub mod safe {
    #[allow(clippy::wildcard_imports)]
    use super::*;
    use crate::openssl::{EVP_MD_CTX_free, EVP_MD_CTX_new};
    use std::cmp::Ordering;
    use std::ffi::CStr;
    use std::marker::PhantomData;
    use std::ptr::{self, NonNull};

    fn c_len(len: usize) -> Option<c_int> {
        c_int::try_from(len).ok()
    }

    /// Fills `buf` from OpenSSL's CSPRNG.
    pub fn random_bytes(buf: &mut [u8]) -> bool {
        let Some(n) = c_len(buf.len()) else { return false };
        // SAFETY: `buf` is writable for `n` bytes.
        unsafe { RAND_bytes(buf.as_mut_ptr(), n) == 1 }
    }

    // ----------------------------------------------------------- ciphers

    /// The fixed ciphers the SSH transport runs.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Cipher {
        Aes128Ctr,
        Aes192Ctr,
        Aes256Ctr,
        Aes128Gcm,
        Aes256Gcm,
        ChaCha20,
    }

    impl Cipher {
        fn evp(self) -> *const EVP_CIPHER {
            let getter: unsafe extern "C" fn() -> *const EVP_CIPHER = match self {
                Cipher::Aes128Ctr => EVP_aes_128_ctr,
                Cipher::Aes192Ctr => EVP_aes_192_ctr,
                Cipher::Aes256Ctr => EVP_aes_256_ctr,
                Cipher::Aes128Gcm => EVP_aes_128_gcm,
                Cipher::Aes256Gcm => EVP_aes_256_gcm,
                Cipher::ChaCha20 => EVP_chacha20,
            };
            // SAFETY: the getters take no arguments and return static
            // objects.
            unsafe { getter() }
        }
    }

    /// A cipher implementation fetched by name ("AES-256-CBC", ...).
    #[derive(Debug)]
    pub struct FetchedCipher(NonNull<EVP_CIPHER>);

    impl FetchedCipher {
        pub fn fetch(name: &CStr) -> Option<Self> {
            // SAFETY: the name is NUL-terminated; NULL selects the default
            // library context and properties.
            let c = unsafe { EVP_CIPHER_fetch(ptr::null_mut(), name.as_ptr(), ptr::null()) };
            NonNull::new(c).map(Self)
        }
    }

    impl Drop for FetchedCipher {
        fn drop(&mut self) {
            // SAFETY: fetched by EVP_CIPHER_fetch and freed once.
            unsafe { EVP_CIPHER_free(self.0.as_ptr()) };
        }
    }

    #[derive(Debug)]
    pub struct CipherCtx(NonNull<EVP_CIPHER_CTX>);

    impl Drop for CipherCtx {
        fn drop(&mut self) {
            // SAFETY: created by EVP_CIPHER_CTX_new and freed once; the free
            // also clears the key schedule.
            unsafe { EVP_CIPHER_CTX_free(self.0.as_ptr()) };
        }
    }

    /// Which way a context runs; `Keep` re-initialises without changing it.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Direction {
        Decrypt = 0,
        Encrypt = 1,
        Keep = -1,
    }

    fn opt_ptr(s: Option<&[u8]>) -> *const u8 {
        s.map_or(ptr::null(), <[u8]>::as_ptr)
    }

    impl CipherCtx {
        pub fn new() -> Option<Self> {
            // SAFETY: no preconditions.
            NonNull::new(unsafe { EVP_CIPHER_CTX_new() }).map(Self)
        }

        fn ptr(&self) -> *mut EVP_CIPHER_CTX {
            self.0.as_ptr()
        }

        /// Selects (or keeps, with `None`) the cipher and sets the key and
        /// IV, which must be at least as long as the cipher's own.
        fn init_raw(
            &mut self,
            cipher: *const EVP_CIPHER,
            key: Option<&[u8]>,
            iv: Option<&[u8]>,
            dir: Direction,
        ) -> bool {
            // A key or IV shorter than 32 and 16 bytes could be over-read by
            // the cipher; the SSH ciphers never need more.
            if key.is_some_and(|k| k.len() < 32) || iv.is_some_and(|v| v.len() < 16) {
                return false;
            }
            // SAFETY: the context is live; key and IV are NULL or readable
            // for the longest key (32) and IV (16) any of these ciphers use.
            unsafe {
                EVP_CipherInit_ex(
                    self.ptr(),
                    cipher,
                    ptr::null_mut(),
                    opt_ptr(key),
                    opt_ptr(iv),
                    dir as c_int,
                ) == 1
            }
        }

        /// Starts `cipher`; the key and IV slices are padded copies, at
        /// least 32 and 16 bytes long.
        pub fn init(
            &mut self,
            cipher: Option<Cipher>,
            key: Option<&[u8]>,
            iv: Option<&[u8]>,
            dir: Direction,
        ) -> bool {
            self.init_raw(cipher.map_or(ptr::null(), Cipher::evp), key, iv, dir)
        }

        pub fn init_fetched(
            &mut self,
            cipher: Option<&FetchedCipher>,
            key: Option<&[u8]>,
            iv: Option<&[u8]>,
            dir: Direction,
        ) -> bool {
            self.init_raw(cipher.map_or(ptr::null(), |c| c.0.as_ptr()), key, iv, dir)
        }

        fn ctrl(&mut self, kind: c_int, arg: c_int, data: *mut c_void) -> bool {
            // SAFETY: the context is live; each caller passes a buffer of
            // the size its control documents.
            unsafe { EVP_CIPHER_CTX_ctrl(self.ptr(), kind, arg, data) == 1 }
        }

        pub fn gcm_set_ivlen(&mut self, len: c_int) -> bool {
            self.ctrl(EVP_CTRL_GCM_SET_IVLEN, len, ptr::null_mut())
        }

        pub fn gcm_get_tag(&mut self, tag: &mut [u8; 16]) -> bool {
            self.ctrl(EVP_CTRL_GCM_GET_TAG, 16, tag.as_mut_ptr().cast())
        }

        pub fn gcm_set_tag(&mut self, tag: &[u8; 16]) -> bool {
            let mut copy = *tag;
            self.ctrl(EVP_CTRL_GCM_SET_TAG, 16, copy.as_mut_ptr().cast())
        }

        pub fn set_padding(&mut self, on: bool) -> bool {
            // SAFETY: the context is live.
            unsafe { EVP_CIPHER_CTX_set_padding(self.ptr(), c_int::from(on)) == 1 }
        }

        fn block_size(&self) -> usize {
            // SAFETY: the context is live.
            usize::try_from(unsafe { EVP_CIPHER_CTX_get_block_size(self.ptr()) }).unwrap_or(0)
        }

        /// Feeds additional authenticated data to an AEAD cipher.
        pub fn aad(&mut self, data: &[u8]) -> bool {
            let Some(n) = c_len(data.len()) else { return false };
            let mut out_len = 0;
            // SAFETY: a NULL output marks the input as AAD; `data` is
            // readable for `n` bytes.
            unsafe {
                EVP_CipherUpdate(self.ptr(), ptr::null_mut(), &raw mut out_len, data.as_ptr(), n)
                    == 1
            }
        }

        /// Encrypts or decrypts `data` in place. Only stream modes (block
        /// size 1: CTR, GCM, ChaCha20) produce exactly as much as they take,
        /// so anything else is refused.
        pub fn update_in_place(&mut self, data: &mut [u8]) -> bool {
            if self.block_size() != 1 {
                return false;
            }
            for chunk in data.chunks_mut(1 << 30) {
                let Some(n) = c_len(chunk.len()) else { return false };
                let mut out_len = 0;
                // SAFETY: a stream cipher writes exactly `n` bytes; in-place
                // operation is supported, and `chunk` is valid for both.
                let ok = unsafe {
                    EVP_CipherUpdate(
                        self.ptr(),
                        chunk.as_mut_ptr(),
                        &raw mut out_len,
                        chunk.as_ptr(),
                        n,
                    )
                };
                if ok != 1 || out_len != n {
                    return false;
                }
            }
            true
        }

        /// Processes `input` into `out`, which must hold a block more than
        /// the input; returns the bytes written.
        pub fn update(&mut self, input: &[u8], out: &mut [u8]) -> Option<usize> {
            let block = self.block_size().max(1);
            if out.len() < input.len().checked_add(block)? {
                return None;
            }
            let n = c_len(input.len())?;
            let mut out_len = 0;
            // SAFETY: `out` holds the input plus a block, the most a cipher
            // update can produce; `input` is readable for `n` bytes.
            let ok = unsafe {
                EVP_CipherUpdate(self.ptr(), out.as_mut_ptr(), &raw mut out_len, input.as_ptr(), n)
            };
            if ok == 1 { usize::try_from(out_len).ok() } else { None }
        }

        /// Finishes the operation (for GCM decryption, checks the tag);
        /// returns any final bytes written to `out`.
        pub fn finalize(&mut self, out: &mut [u8]) -> Option<usize> {
            let mut scratch = [0_u8; 64];
            let mut out_len = 0;
            // SAFETY: `scratch` is larger than any block a final call writes.
            let ok =
                unsafe { EVP_CipherFinal_ex(self.ptr(), scratch.as_mut_ptr(), &raw mut out_len) };
            let n = usize::try_from(out_len).ok()?;
            if ok != 1 || n > out.len() {
                return None;
            }
            out[..n].copy_from_slice(&scratch[..n]);
            Some(n)
        }
    }

    // -------------------------------------------------------------- MACs

    /// A MAC implementation ("HMAC", "POLY1305").
    #[derive(Debug)]
    pub struct Mac(NonNull<EVP_MAC>);

    impl Drop for Mac {
        fn drop(&mut self) {
            // SAFETY: fetched by EVP_MAC_fetch and freed once.
            unsafe { EVP_MAC_free(self.0.as_ptr()) };
        }
    }

    impl Mac {
        pub fn fetch(name: &CStr) -> Option<Self> {
            // SAFETY: the name is NUL-terminated; NULL selects the defaults.
            let m = unsafe { EVP_MAC_fetch(ptr::null_mut(), name.as_ptr(), ptr::null()) };
            NonNull::new(m).map(Self)
        }

        /// One MAC over the concatenation of `parts`, written to `out`;
        /// `digest` names the hash for HMAC. True only when exactly
        /// `out.len()` bytes were produced.
        pub fn compute(
            &self,
            digest: Option<&CStr>,
            key: &[u8],
            parts: &[&[u8]],
            out: &mut [u8],
        ) -> bool {
            // SAFETY: the MAC is live.
            let Some(ctx) = NonNull::new(unsafe { EVP_MAC_CTX_new(self.0.as_ptr()) }) else {
                return false;
            };
            let ctx = MacCtx(ctx);
            let mut name = digest.map(|d| d.to_bytes_with_nul().to_vec());
            let params = name.as_mut().map(|n| {
                // SAFETY: the key is a static NUL-terminated string and `n`
                // a NUL-terminated buffer that outlives `params`; a zero
                // size asks OpenSSL to measure it.
                let digest = unsafe {
                    OSSL_PARAM_construct_utf8_string(c"digest".as_ptr(), n.as_mut_ptr().cast(), 0)
                };
                // SAFETY: no preconditions.
                let end = unsafe { OSSL_PARAM_construct_end() };
                [digest, end]
            });
            let params_ptr = params.as_ref().map_or(ptr::null(), |p| p.as_ptr());
            // SAFETY: the context is live, the key readable for its length,
            // and the parameter array NULL or terminated.
            if unsafe { EVP_MAC_init(ctx.0.as_ptr(), key.as_ptr(), key.len(), params_ptr) } != 1 {
                return false;
            }
            for part in parts {
                // SAFETY: the context is live and `part` readable.
                if unsafe { EVP_MAC_update(ctx.0.as_ptr(), part.as_ptr(), part.len()) } != 1 {
                    return false;
                }
            }
            let mut produced = 0;
            // SAFETY: `out` is writable for its length, which is passed as
            // the output size.
            let ok = unsafe {
                EVP_MAC_final(ctx.0.as_ptr(), out.as_mut_ptr(), &raw mut produced, out.len())
            };
            ok == 1 && produced == out.len()
        }
    }

    struct MacCtx(NonNull<EVP_MAC_CTX>);

    impl Drop for MacCtx {
        fn drop(&mut self) {
            // SAFETY: created by EVP_MAC_CTX_new and freed once.
            unsafe { EVP_MAC_CTX_free(self.0.as_ptr()) };
        }
    }

    // -------------------------------------------------------- big numbers

    #[derive(Debug)]
    pub struct Bn {
        ptr: NonNull<BIGNUM>,
        /// Secret values are zeroed when freed.
        secret: bool,
    }

    impl Drop for Bn {
        fn drop(&mut self) {
            let free: unsafe extern "C" fn(*mut BIGNUM) =
                if self.secret { BN_clear_free } else { BN_free };
            // SAFETY: the number is owned and freed once.
            unsafe { free(self.ptr.as_ptr()) };
        }
    }

    impl Bn {
        fn wrap(ptr: *mut BIGNUM, secret: bool) -> Option<Self> {
            NonNull::new(ptr).map(|ptr| Self { ptr, secret })
        }

        pub fn as_ptr(&self) -> *const BIGNUM {
            self.ptr.as_ptr()
        }

        fn mut_ptr(&mut self) -> *mut BIGNUM {
            self.ptr.as_ptr()
        }

        pub fn new() -> Option<Self> {
            // SAFETY: no preconditions.
            Self::wrap(unsafe { BN_new() }, false)
        }

        /// A number for secret values: allocated from the secure heap when
        /// one is configured, and zeroed when freed.
        pub fn new_secret() -> Option<Self> {
            // SAFETY: no preconditions.
            Self::wrap(unsafe { BN_secure_new() }, true)
        }

        pub fn from_word(w: u64) -> Option<Self> {
            let mut n = Self::new()?;
            // SAFETY: the number is live.
            (unsafe { BN_set_word(n.mut_ptr(), w) } == 1).then_some(n)
        }

        /// From an unsigned big-endian magnitude.
        pub fn from_bytes(be: &[u8], secret: bool) -> Option<Self> {
            let n = c_len(be.len())?;
            let mut out = if secret { Self::new_secret()? } else { Self::new()? };
            // SAFETY: `be` is readable for `n` bytes; the result is written
            // into the live `out`.
            let r = unsafe { BN_bin2bn(be.as_ptr(), n, out.mut_ptr()) };
            (!r.is_null()).then_some(out)
        }

        /// The unsigned big-endian magnitude, without leading zeros.
        pub fn to_bytes(&self) -> Vec<u8> {
            let mut out = vec![0_u8; self.num_bits().div_ceil(8)];
            // SAFETY: `out` holds BN_num_bytes, exactly what BN_bn2bin
            // writes.
            let n = unsafe { BN_bn2bin(self.as_ptr(), out.as_mut_ptr()) };
            out.truncate(usize::try_from(n).unwrap_or(0));
            out
        }

        pub fn num_bits(&self) -> usize {
            // SAFETY: the number is live.
            usize::try_from(unsafe { BN_num_bits(self.as_ptr()) }).unwrap_or(0)
        }

        pub fn is_negative(&self) -> bool {
            // SAFETY: the number is live.
            unsafe { BN_is_negative(self.as_ptr()) != 0 }
        }

        pub fn is_odd(&self) -> bool {
            // SAFETY: the number is live.
            unsafe { BN_is_odd(self.as_ptr()) != 0 }
        }

        pub fn compare(&self, other: &Bn) -> Ordering {
            // SAFETY: both numbers are live.
            unsafe { BN_cmp(self.as_ptr(), other.as_ptr()) }.cmp(&0)
        }

        /// Compared with one.
        pub fn cmp_one(&self) -> Ordering {
            // SAFETY: BN_value_one returns a static number.
            let one = unsafe { BN_value_one() };
            // SAFETY: both numbers are live.
            unsafe { BN_cmp(self.as_ptr(), one) }.cmp(&0)
        }

        pub fn dup(&self) -> Option<Self> {
            // SAFETY: the number is live.
            Self::wrap(unsafe { BN_dup(self.as_ptr()) }, self.secret)
        }

        pub fn sub_word(&mut self, w: u64) -> bool {
            // SAFETY: the number is live.
            unsafe { BN_sub_word(self.mut_ptr(), w) == 1 }
        }

        /// The RFC 3526 MODP prime of `bits` (2048, 3072, 4096, 6144, 8192).
        pub fn rfc3526_prime(bits: u32) -> Option<Self> {
            let getter: unsafe extern "C" fn(*mut BIGNUM) -> *mut BIGNUM = match bits {
                2048 => BN_get_rfc3526_prime_2048,
                3072 => BN_get_rfc3526_prime_3072,
                4096 => BN_get_rfc3526_prime_4096,
                6144 => BN_get_rfc3526_prime_6144,
                8192 => BN_get_rfc3526_prime_8192,
                _ => return None,
            };
            // SAFETY: a NULL argument allocates a new number.
            Self::wrap(unsafe { getter(ptr::null_mut()) }, false)
        }

        /// A secret random number of `bits` bits with the top bit set,
        /// marked for constant-time arithmetic.
        pub fn private_random(bits: c_int) -> Option<Self> {
            let mut n = Self::new_secret()?;
            // SAFETY: the number is live; NULL selects the default context.
            let ok = unsafe {
                BN_priv_rand_ex(
                    n.mut_ptr(),
                    bits,
                    BN_RAND_TOP_ONE,
                    BN_RAND_BOTTOM_ANY,
                    0,
                    ptr::null_mut(),
                )
            };
            if ok != 1 {
                return None;
            }
            // SAFETY: the number is live.
            unsafe { BN_set_flags(n.mut_ptr(), BN_FLG_CONSTTIME) };
            Some(n)
        }

        /// `a ^ p mod m` in constant time, into a new secret number.
        pub fn mod_exp_consttime(a: &Bn, p: &Bn, m: &Bn, ctx: &mut BnCtx) -> Option<Self> {
            let mut r = Self::new_secret()?;
            // SAFETY: all numbers and the context are live; NULL asks for a
            // fresh Montgomery context.
            let ok = unsafe {
                BN_mod_exp_mont_consttime(
                    r.mut_ptr(),
                    a.as_ptr(),
                    p.as_ptr(),
                    m.as_ptr(),
                    ctx.0.as_ptr(),
                    ptr::null_mut(),
                )
            };
            (ok == 1).then_some(r)
        }

        /// `a - b`, into a new secret number.
        pub fn sub(a: &Bn, b: &Bn) -> Option<Self> {
            let mut r = Self::new_secret()?;
            // SAFETY: all numbers are live.
            (unsafe { BN_sub(r.mut_ptr(), a.as_ptr(), b.as_ptr()) } == 1).then_some(r)
        }

        /// `a mod m`, into a new secret number.
        pub fn rem(a: &Bn, m: &Bn, ctx: &mut BnCtx) -> Option<Self> {
            let mut r = Self::new_secret()?;
            // SAFETY: all numbers and the context are live; a NULL quotient
            // is not computed.
            let ok = unsafe {
                BN_div(ptr::null_mut(), r.mut_ptr(), a.as_ptr(), m.as_ptr(), ctx.0.as_ptr())
            };
            (ok == 1).then_some(r)
        }

        /// One, as a fresh number.
        pub fn one() -> Option<Self> {
            Self::from_word(1)
        }
    }

    #[derive(Debug)]
    pub struct BnCtx(NonNull<BN_CTX>);

    impl BnCtx {
        pub fn new() -> Option<Self> {
            // SAFETY: no preconditions.
            NonNull::new(unsafe { BN_CTX_new() }).map(Self)
        }
    }

    impl Drop for BnCtx {
        fn drop(&mut self) {
            // SAFETY: created by BN_CTX_new and freed once.
            unsafe { BN_CTX_free(self.0.as_ptr()) };
        }
    }

    // --------------------------------------------------- parameter builder

    /// Collects key components; borrowed data must outlive the builder,
    /// which OpenSSL reads only when the parameters are built.
    #[derive(Debug)]
    pub struct ParamBld<'a> {
        ptr: NonNull<OSSL_PARAM_BLD>,
        ok: bool,
        _borrows: PhantomData<&'a [u8]>,
    }

    impl Drop for ParamBld<'_> {
        fn drop(&mut self) {
            // SAFETY: created by OSSL_PARAM_BLD_new and freed once.
            unsafe { OSSL_PARAM_BLD_free(self.ptr.as_ptr()) };
        }
    }

    impl<'a> ParamBld<'a> {
        pub fn new() -> Option<Self> {
            // SAFETY: no preconditions.
            let ptr = NonNull::new(unsafe { OSSL_PARAM_BLD_new() })?;
            Some(Self { ptr, ok: true, _borrows: PhantomData })
        }

        pub fn utf8(&mut self, key: &'static CStr, value: &'a CStr) {
            // SAFETY: both strings are NUL-terminated; `value` outlives the
            // builder; a zero size asks OpenSSL to measure it.
            self.ok &= unsafe {
                OSSL_PARAM_BLD_push_utf8_string(self.ptr.as_ptr(), key.as_ptr(), value.as_ptr(), 0)
                    == 1
            };
        }

        pub fn octets(&mut self, key: &'static CStr, value: &'a [u8]) {
            // SAFETY: the key is NUL-terminated and `value` is readable for
            // its length for the builder's lifetime.
            self.ok &= unsafe {
                OSSL_PARAM_BLD_push_octet_string(
                    self.ptr.as_ptr(),
                    key.as_ptr(),
                    value.as_ptr().cast(),
                    value.len(),
                ) == 1
            };
        }

        pub fn bn(&mut self, key: &'static CStr, value: &'a Bn) {
            // SAFETY: the key is NUL-terminated and the number outlives the
            // builder.
            self.ok &= unsafe {
                OSSL_PARAM_BLD_push_BN(self.ptr.as_ptr(), key.as_ptr(), value.as_ptr()) == 1
            };
        }
    }

    // ------------------------------------------------------ asymmetric keys

    struct PkeyCtx(NonNull<EVP_PKEY_CTX>);

    impl Drop for PkeyCtx {
        fn drop(&mut self) {
            // SAFETY: created by one of the EVP_PKEY_CTX constructors and
            // freed once.
            unsafe { EVP_PKEY_CTX_free(self.0.as_ptr()) };
        }
    }

    impl PkeyCtx {
        fn by_name(name: &CStr) -> Option<Self> {
            // SAFETY: the name is NUL-terminated; NULLs select the defaults.
            let c =
                unsafe { EVP_PKEY_CTX_new_from_name(ptr::null_mut(), name.as_ptr(), ptr::null()) };
            NonNull::new(c).map(Self)
        }

        fn ptr(&self) -> *mut EVP_PKEY_CTX {
            self.0.as_ptr()
        }
    }

    struct DigestCtx(NonNull<EVP_MD_CTX>);

    impl Drop for DigestCtx {
        fn drop(&mut self) {
            // SAFETY: created by EVP_MD_CTX_new and freed once; the signing
            // or verifying key context it owns goes with it.
            unsafe { EVP_MD_CTX_free(self.0.as_ptr()) };
        }
    }

    impl DigestCtx {
        fn new() -> Option<Self> {
            // SAFETY: no preconditions.
            NonNull::new(unsafe { EVP_MD_CTX_new() }).map(Self)
        }
    }

    #[derive(Debug)]
    pub struct Pkey(NonNull<EVP_PKEY>);

    impl Drop for Pkey {
        fn drop(&mut self) {
            // SAFETY: the key is owned (one reference) and released once.
            unsafe { EVP_PKEY_free(self.0.as_ptr()) };
        }
    }

    fn md_name(digest: Option<&CStr>) -> *const c_char {
        digest.map_or(ptr::null(), CStr::as_ptr)
    }

    /// Whether this OpenSSL provides the named key algorithm.
    pub fn pkey_algorithm_available(name: &CStr) -> bool {
        PkeyCtx::by_name(name).is_some()
    }

    impl Pkey {
        fn ptr(&self) -> *mut EVP_PKEY {
            self.0.as_ptr()
        }

        /// A fresh key pair of the named algorithm ("X25519", "ML-KEM-768").
        pub fn generate(name: &CStr) -> Option<Self> {
            let ctx = PkeyCtx::by_name(name)?;
            let mut out: *mut EVP_PKEY = ptr::null_mut();
            // SAFETY: the context is live.
            if unsafe { EVP_PKEY_keygen_init(ctx.ptr()) } != 1 {
                return None;
            }
            // SAFETY: the context is live and `out` receives an owned key.
            if unsafe { EVP_PKEY_generate(ctx.ptr(), &raw mut out) } != 1 {
                return None;
            }
            NonNull::new(out).map(Self)
        }

        pub fn from_raw_public(kind: c_int, key: &[u8]) -> Option<Self> {
            // SAFETY: `key` is readable for its length.
            let p = unsafe {
                EVP_PKEY_new_raw_public_key(kind, ptr::null_mut(), key.as_ptr(), key.len())
            };
            NonNull::new(p).map(Self)
        }

        pub fn from_raw_private(kind: c_int, key: &[u8]) -> Option<Self> {
            // SAFETY: `key` is readable for its length.
            let p = unsafe {
                EVP_PKEY_new_raw_private_key(kind, ptr::null_mut(), key.as_ptr(), key.len())
            };
            NonNull::new(p).map(Self)
        }

        /// A key of the named type from its components.
        pub fn from_data(name: &CStr, selection: c_int, bld: &ParamBld<'_>) -> Option<Self> {
            struct Params(NonNull<OSSL_PARAM>);
            impl Drop for Params {
                fn drop(&mut self) {
                    // SAFETY: built by OSSL_PARAM_BLD_to_param, freed once.
                    unsafe { OSSL_PARAM_free(self.0.as_ptr()) };
                }
            }
            if !bld.ok {
                return None;
            }
            // SAFETY: the builder is live and every borrowed component is
            // still valid (the builder's lifetime guarantees it).
            let params =
                Params(NonNull::new(unsafe { OSSL_PARAM_BLD_to_param(bld.ptr.as_ptr()) })?);
            let ctx = PkeyCtx::by_name(name)?;
            let mut out: *mut EVP_PKEY = ptr::null_mut();
            // SAFETY: the context is live.
            if unsafe { EVP_PKEY_fromdata_init(ctx.ptr()) } != 1 {
                return None;
            }
            // SAFETY: the context and parameter array are live; `out`
            // receives an owned key.
            let ok =
                unsafe { EVP_PKEY_fromdata(ctx.ptr(), &raw mut out, selection, params.0.as_ptr()) };
            if ok == 1 { NonNull::new(out).map(Self) } else { None }
        }

        /// A PEM private key (PKCS#1, SEC1, PKCS#8), decrypted with the
        /// passphrase when it is encrypted.
        pub fn read_pem_private(data: &[u8], passphrase: &[u8]) -> Option<Self> {
            struct Bio(NonNull<BIO>);
            impl Drop for Bio {
                fn drop(&mut self) {
                    // SAFETY: created by BIO_new_mem_buf and freed once.
                    unsafe { BIO_free(self.0.as_ptr()) };
                }
            }
            unsafe extern "C" fn callback(
                buf: *mut c_char,
                size: c_int,
                _rwflag: c_int,
                user: *mut c_void,
            ) -> c_int {
                // SAFETY: `user` is the slice reference passed below, alive
                // for the read call.
                let pass = unsafe { &*user.cast::<&[u8]>() };
                let Ok(cap) = usize::try_from(size) else { return 0 };
                if pass.is_empty() || pass.len() > cap {
                    return 0;
                }
                // SAFETY: OpenSSL's buffer holds `size` bytes, at least the
                // passphrase's length.
                unsafe { ptr::copy_nonoverlapping(pass.as_ptr(), buf.cast::<u8>(), pass.len()) };
                c_int::try_from(pass.len()).unwrap_or(0)
            }
            let n = c_len(data.len())?;
            // SAFETY: the buffer is read-only and outlives the BIO.
            let bio = Bio(NonNull::new(unsafe { BIO_new_mem_buf(data.as_ptr().cast(), n) })?);
            let mut pass: &[u8] = passphrase;
            // SAFETY: the BIO is live; the callback reads `pass` through the
            // pointer only during this call.
            let key = unsafe {
                PEM_read_bio_PrivateKey(
                    bio.0.as_ptr(),
                    ptr::null_mut(),
                    Some(callback),
                    (&raw mut pass).cast(),
                )
            };
            NonNull::new(key).map(Self)
        }

        pub fn base_id(&self) -> c_int {
            // SAFETY: the key is live.
            unsafe { EVP_PKEY_get_base_id(self.ptr()) }
        }

        /// The largest signature, in bytes (the modulus for RSA).
        pub fn size(&self) -> usize {
            // SAFETY: the key is live.
            usize::try_from(unsafe { EVP_PKEY_get_size(self.ptr()) }).unwrap_or(0)
        }

        pub fn raw_public(&self) -> Option<Vec<u8>> {
            let mut buf = vec![0_u8; 1600];
            let mut len = buf.len();
            // SAFETY: `buf` is writable for `len` bytes and the name is a
            // static NUL-terminated string.
            let encoded = unsafe {
                EVP_PKEY_get_octet_string_param(
                    self.ptr(),
                    c"encoded-pub-key".as_ptr(),
                    buf.as_mut_ptr(),
                    buf.len(),
                    &raw mut len,
                )
            } == 1;
            if !encoded {
                len = buf.len();
                // SAFETY: as above.
                let raw = unsafe {
                    EVP_PKEY_get_raw_public_key(self.ptr(), buf.as_mut_ptr(), &raw mut len)
                };
                if raw != 1 {
                    return None;
                }
            }
            buf.truncate(len);
            Some(buf)
        }

        pub fn octet_param(&self, name: &CStr) -> Option<Vec<u8>> {
            let mut buf = vec![0_u8; 1024];
            let mut len = 0;
            // SAFETY: `buf` is writable for its length; the name is
            // NUL-terminated.
            let ok = unsafe {
                EVP_PKEY_get_octet_string_param(
                    self.ptr(),
                    name.as_ptr(),
                    buf.as_mut_ptr(),
                    buf.len(),
                    &raw mut len,
                )
            };
            if ok != 1 || len > buf.len() {
                return None;
            }
            buf.truncate(len);
            Some(buf)
        }

        pub fn utf8_param(&self, name: &CStr) -> Option<String> {
            let mut buf = [0_u8; 64];
            let mut len = 0;
            // SAFETY: `buf` is writable for its length (OpenSSL keeps room
            // for the terminator); the name is NUL-terminated.
            let ok = unsafe {
                EVP_PKEY_get_utf8_string_param(
                    self.ptr(),
                    name.as_ptr(),
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                    &raw mut len,
                )
            };
            if ok != 1 || len >= buf.len() {
                return None;
            }
            String::from_utf8(buf[..len].to_vec()).ok()
        }

        pub fn set_utf8_param(&self, name: &CStr, value: &CStr) -> bool {
            // SAFETY: the key is live and both strings NUL-terminated.
            unsafe {
                EVP_PKEY_set_utf8_string_param(self.ptr(), name.as_ptr(), value.as_ptr()) == 1
            }
        }

        pub fn bn_param(&self, name: &CStr) -> Option<Bn> {
            let mut out: *mut BIGNUM = ptr::null_mut();
            // SAFETY: the key is live and `out` receives an owned number.
            let ok = unsafe { EVP_PKEY_get_bn_param(self.ptr(), name.as_ptr(), &raw mut out) };
            if ok == 1 { Bn::wrap(out, false) } else { None }
        }

        pub fn public_check(&self) -> bool {
            // SAFETY: the key is live; NULLs select the defaults.
            let c = unsafe { EVP_PKEY_CTX_new_from_pkey(ptr::null_mut(), self.ptr(), ptr::null()) };
            let Some(ctx) = NonNull::new(c).map(PkeyCtx) else { return false };
            // SAFETY: the context is live.
            unsafe { EVP_PKEY_public_check(ctx.ptr()) == 1 }
        }

        /// Key agreement with `peer`; true when exactly `out.len()` bytes
        /// of shared secret were written.
        pub fn derive(&self, peer: &Pkey, out: &mut [u8]) -> bool {
            // SAFETY: the key is live; the context takes its own reference.
            let c = unsafe { EVP_PKEY_CTX_new(self.ptr(), ptr::null_mut()) };
            let Some(ctx) = NonNull::new(c).map(PkeyCtx) else { return false };
            let mut len = out.len();
            // SAFETY: the context is live.
            if unsafe { EVP_PKEY_derive_init(ctx.ptr()) } != 1 {
                return false;
            }
            // SAFETY: the context and peer are live.
            if unsafe { EVP_PKEY_derive_set_peer(ctx.ptr(), peer.ptr()) } != 1 {
                return false;
            }
            // SAFETY: `out` is writable for `len` bytes, which bounds what
            // derive writes.
            let ok = unsafe { EVP_PKEY_derive(ctx.ptr(), out.as_mut_ptr(), &raw mut len) };
            ok == 1 && len == out.len()
        }

        /// KEM decapsulation; true when exactly `out.len()` bytes of shared
        /// secret were recovered.
        pub fn decapsulate(&self, wrapped: &[u8], out: &mut [u8]) -> bool {
            // SAFETY: the key is live; the context takes its own reference.
            let c = unsafe { EVP_PKEY_CTX_new(self.ptr(), ptr::null_mut()) };
            let Some(ctx) = NonNull::new(c).map(PkeyCtx) else { return false };
            let mut len = out.len();
            // SAFETY: the context is live.
            if unsafe { EVP_PKEY_decapsulate_init(ctx.ptr(), ptr::null()) } != 1 {
                return false;
            }
            // SAFETY: the context is live; `out` is writable for `len`
            // bytes and `wrapped` readable for its length.
            let ok = unsafe {
                EVP_PKEY_decapsulate(
                    ctx.ptr(),
                    out.as_mut_ptr(),
                    &raw mut len,
                    wrapped.as_ptr(),
                    wrapped.len(),
                ) == 1
            };
            ok && len == out.len()
        }

        /// One-shot verification; `digest` is `None` for Ed25519.
        pub fn verify(&self, digest: Option<&CStr>, sig: &[u8], data: &[u8]) -> bool {
            let Some(md) = DigestCtx::new() else { return false };
            // SAFETY: the digest context and key are live; NULLs select the
            // defaults.
            let init = unsafe {
                EVP_DigestVerifyInit_ex(
                    md.0.as_ptr(),
                    ptr::null_mut(),
                    md_name(digest),
                    ptr::null_mut(),
                    ptr::null(),
                    self.ptr(),
                    ptr::null(),
                )
            };
            if init != 1 {
                return false;
            }
            // SAFETY: signature and data are readable for their lengths.
            unsafe {
                EVP_DigestVerify(md.0.as_ptr(), sig.as_ptr(), sig.len(), data.as_ptr(), data.len())
                    == 1
            }
        }

        /// One-shot signature; `digest` is `None` for Ed25519.
        pub fn sign(&self, digest: Option<&CStr>, data: &[u8]) -> Option<Vec<u8>> {
            let md = DigestCtx::new()?;
            let mut len = 0;
            // SAFETY: the digest context and key are live; NULLs select the
            // defaults.
            let init = unsafe {
                EVP_DigestSignInit_ex(
                    md.0.as_ptr(),
                    ptr::null_mut(),
                    md_name(digest),
                    ptr::null_mut(),
                    ptr::null(),
                    self.ptr(),
                    ptr::null(),
                )
            };
            // SAFETY: a NULL signature buffer asks for the maximum length.
            let sized = init == 1
                && unsafe {
                    EVP_DigestSign(
                        md.0.as_ptr(),
                        ptr::null_mut(),
                        &raw mut len,
                        data.as_ptr(),
                        data.len(),
                    )
                } == 1;
            if !sized {
                return None;
            }
            let mut sig = vec![0_u8; len];
            // SAFETY: `sig` holds the maximum length just reported.
            let ok = unsafe {
                EVP_DigestSign(
                    md.0.as_ptr(),
                    sig.as_mut_ptr(),
                    &raw mut len,
                    data.as_ptr(),
                    data.len(),
                )
            };
            if ok != 1 || len > sig.len() {
                return None;
            }
            sig.truncate(len);
            Some(sig)
        }
    }

    // ---------------------------------------------------------- ECDSA sigs

    struct EcdsaSig(NonNull<ECDSA_SIG>);

    impl Drop for EcdsaSig {
        fn drop(&mut self) {
            // SAFETY: owned and freed once, with the numbers it holds.
            unsafe { ECDSA_SIG_free(self.0.as_ptr()) };
        }
    }

    /// DER for an ECDSA signature given its r and s.
    pub fn ecdsa_der_from_rs(r: Bn, s: Bn) -> Option<Vec<u8>> {
        // SAFETY: no preconditions.
        let sig = EcdsaSig(NonNull::new(unsafe { ECDSA_SIG_new() })?);
        // SAFETY: set0 takes ownership of both numbers on success, so they
        // are forgotten here only when it succeeds.
        if unsafe { ECDSA_SIG_set0(sig.0.as_ptr(), r.ptr.as_ptr(), s.ptr.as_ptr()) } != 1 {
            return None;
        }
        std::mem::forget(r);
        std::mem::forget(s);
        let mut der: *mut u8 = ptr::null_mut();
        // SAFETY: a NULL `*pp` makes OpenSSL allocate the output, which is
        // copied and released below.
        let n = unsafe { i2d_ECDSA_SIG(sig.0.as_ptr(), &raw mut der) };
        let len = usize::try_from(n).ok().filter(|&n| n > 0)?;
        if der.is_null() {
            return None;
        }
        // SAFETY: OpenSSL allocated `len` bytes at `der`.
        let out = unsafe { std::slice::from_raw_parts(der, len) }.to_vec();
        // SAFETY: `der` came from OpenSSL's allocator and is freed once.
        unsafe { CRYPTO_free(der.cast(), c"evp.rs".as_ptr(), 0) };
        Some(out)
    }

    /// The r and s magnitudes of a DER ECDSA signature.
    pub fn ecdsa_rs_from_der(der: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
        let len = c_long::try_from(der.len()).ok()?;
        let mut p = der.as_ptr();
        // SAFETY: `p` points at `len` readable bytes; a NULL first argument
        // allocates a fresh signature.
        let sig =
            EcdsaSig(NonNull::new(unsafe { d2i_ECDSA_SIG(ptr::null_mut(), &raw mut p, len) })?);
        let bytes = |bn: *const BIGNUM| {
            if bn.is_null() {
                return None;
            }
            // SAFETY: `bn` is owned by the live signature.
            let bits = usize::try_from(unsafe { BN_num_bits(bn) }).ok()?;
            let mut out = vec![0_u8; bits.div_ceil(8)];
            // SAFETY: `out` holds BN_num_bytes.
            let n = unsafe { BN_bn2bin(bn, out.as_mut_ptr()) };
            out.truncate(usize::try_from(n).ok()?);
            Some(out)
        };
        // SAFETY: the signature is live; the getter returns a borrowed number.
        let r = unsafe { ECDSA_SIG_get0_r(sig.0.as_ptr()) };
        // SAFETY: as above.
        let s = unsafe { ECDSA_SIG_get0_s(sig.0.as_ptr()) };
        Some((bytes(r)?, bytes(s)?))
    }

    /// Whether OpenSSL's group name `group` is the NIST curve `nist`
    /// ("P-256"), under either of its names.
    pub fn same_curve(group: &CStr, nist: &CStr) -> bool {
        // SAFETY: the string is NUL-terminated.
        let a = unsafe { OBJ_txt2nid(group.as_ptr()) };
        // SAFETY: the string is NUL-terminated.
        let b = unsafe { EC_curve_nist2nid(nist.as_ptr()) };
        group == nist || (a != 0 && a == b)
    }
}
