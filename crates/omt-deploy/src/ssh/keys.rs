//! Public keys and signature verification -- ssh-ed25519 (RFC 8709), ECDSA
//! over the NIST curves (RFC 5656), RSA with SHA-2 only (RFC 8332); ssh-rsa
//! signatures, which hash with SHA-1, are refused -- and user private keys:
//! OpenSSH's own format (openssh-key-v1, PROTOCOL.key), plain or
//! passphrase-protected with bcrypt_pbkdf, and the PEM formats (PKCS#1,
//! SEC1, PKCS#8) through OpenSSL's decoder.

use std::ffi::{CStr, CString};

use omt_common::buf::{Cursor, wipe_vec};
use omt_common::{Error, Result};
use omt_sys::evp::safe::{
    Bn, CipherCtx, Direction, FetchedCipher, ParamBld, Pkey, ecdsa_der_from_rs, ecdsa_rs_from_der,
    same_curve,
};
use omt_sys::evp::{
    EVP_PKEY_EC, EVP_PKEY_ED25519, EVP_PKEY_KEYPAIR, EVP_PKEY_PUBLIC_KEY, EVP_PKEY_RSA,
};
use omt_sys::openssl::safe::cleanse;

use super::bcrypt::bcrypt_pbkdf;
use super::wire::{self, Put, base64_decode};
use crate::sys;

pub const HOSTKEY_LIST: &str = "ssh-ed25519,ecdsa-sha2-nistp256,ecdsa-sha2-nistp384,\
                                ecdsa-sha2-nistp521,rsa-sha2-512,rsa-sha2-256";

/// The smallest RSA modulus accepted for a host or user key.
const RSA_MIN_BITS: usize = 2048;

struct Curve {
    id: &'static str,
    group: &'static CStr,
    digest: &'static CStr,
}

static CURVES: [Curve; 3] = [
    Curve { id: "nistp256", group: c"P-256", digest: c"SHA256" },
    Curve { id: "nistp384", group: c"P-384", digest: c"SHA384" },
    Curve { id: "nistp521", group: c"P-521", digest: c"SHA512" },
];

/// Whether host-key algorithm `alg` verifies keys of blob type `key_type`.
pub fn hostkey_alg_matches(alg: &str, key_type: &str) -> bool {
    if alg == "rsa-sha2-256" || alg == "rsa-sha2-512" {
        return key_type == "ssh-rsa";
    }
    alg == key_type
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Ed25519,
    Ec(&'static CStr),
    Rsa,
}

#[derive(Debug)]
pub struct PublicKey {
    pkey: Pkey,
    kind: Kind,
    /// The key type as named in the blob.
    pub name: String,
}

fn malformed() -> Error {
    Error::new("malformed public key")
}

impl PublicKey {
    /// Parses a public key blob of a supported type.
    pub fn parse(blob: &[u8]) -> Result<Self> {
        let mut r = Cursor::new(blob);
        let type_name = wire::string(&mut r).filter(|t| t.len() < 32).ok_or_else(malformed)?;
        let name = String::from_utf8_lossy(type_name).into_owned();
        if type_name == b"ssh-ed25519" {
            let pk = wire::string(&mut r).filter(|pk| pk.len() == 32).ok_or_else(malformed)?;
            if r.remaining() != 0 {
                return Err(malformed());
            }
            let pkey = Pkey::from_raw_public(EVP_PKEY_ED25519, pk).ok_or_else(malformed)?;
            return Ok(Self { pkey, kind: Kind::Ed25519, name });
        }
        for curve in &CURVES {
            if type_name != format!("ecdsa-sha2-{}", curve.id).as_bytes() {
                continue;
            }
            let id = wire::string(&mut r).ok_or_else(malformed)?;
            let q = wire::string(&mut r).ok_or_else(malformed)?;
            if id != curve.id.as_bytes() || r.remaining() != 0 || q.first() != Some(&0x04) {
                return Err(malformed());
            }
            let mut bld = ParamBld::new().ok_or_else(malformed)?;
            bld.utf8(c"group", curve.group);
            bld.octets(c"pub", q);
            let pkey = Pkey::from_data(c"EC", EVP_PKEY_PUBLIC_KEY, &bld).filter(Pkey::public_check);
            let pkey = pkey.ok_or_else(malformed)?;
            return Ok(Self { pkey, kind: Kind::Ec(curve.digest), name });
        }
        if type_name == b"ssh-rsa" {
            let e = wire::bn(&mut r, false).ok_or_else(malformed)?;
            let n = wire::bn(&mut r, false).ok_or_else(malformed)?;
            if r.remaining() != 0 {
                return Err(malformed());
            }
            if n.num_bits() < RSA_MIN_BITS || n.num_bits() > 16384 || !e.is_odd() {
                return Err(Error::new(format!("RSA keys must have at least {RSA_MIN_BITS} bits")));
            }
            let mut bld = ParamBld::new().ok_or_else(malformed)?;
            bld.bn(c"n", &n);
            bld.bn(c"e", &e);
            let pkey = Pkey::from_data(c"RSA", EVP_PKEY_PUBLIC_KEY, &bld).ok_or_else(malformed)?;
            return Ok(Self { pkey, kind: Kind::Rsa, name });
        }
        Err(Error::new(format!("unsupported public key type {name}")))
    }

    /// Verifies an SSH signature blob made with `alg` over `data`.
    pub fn verify(&self, alg: &str, sig: &[u8], data: &[u8]) -> Result<()> {
        let mut r = Cursor::new(sig);
        let parts = (wire::string(&mut r), wire::string(&mut r));
        let (Some(name), Some(body)) = parts else {
            return Err(Error::new("malformed or mismatched signature"));
        };
        if r.remaining() != 0 || name != alg.as_bytes() || !hostkey_alg_matches(alg, &self.name) {
            return Err(Error::new("malformed or mismatched signature"));
        }
        let ok = match self.kind {
            Kind::Ed25519 => body.len() == 64 && self.pkey.verify(None, body, data),
            Kind::Ec(digest) => {
                let mut b = Cursor::new(body);
                let r = wire::bn(&mut b, false);
                let s = wire::bn(&mut b, false);
                match (r, s, b.remaining()) {
                    (Some(r), Some(s), 0) => ecdsa_der_from_rs(r, s)
                        .is_some_and(|der| self.pkey.verify(Some(digest), &der, data)),
                    _ => false,
                }
            }
            Kind::Rsa => {
                let digest = if alg == "rsa-sha2-512" { c"SHA512" } else { c"SHA256" };
                let modulus = self.pkey.size();
                // A signature shorter than the modulus has lost leading zero
                // bytes; OpenSSH pads it back rather than refusing it.
                body.len() <= modulus && {
                    let mut padded = vec![0_u8; modulus];
                    padded[modulus - body.len()..].copy_from_slice(body);
                    self.pkey.verify(Some(digest), &padded, data)
                }
            }
        };
        if ok { Ok(()) } else { Err(Error::new("signature verification failed")) }
    }
}

// ------------------------------------------------------------- user keys

const KEY_FILE_LIMIT: usize = 1024 * 1024;
const BEGIN_OPENSSH: &str = "-----BEGIN OPENSSH PRIVATE KEY-----";
const END_OPENSSH: &str = "-----END OPENSSH PRIVATE KEY-----";
const AUTH_MAGIC: &[u8] = b"openssh-key-v1\0";

#[derive(Debug)]
pub struct PrivateKey {
    pkey: Pkey,
    /// ssh-ed25519, ecdsa-sha2-nistp256, ssh-rsa, ...
    pub type_name: String,
    /// The public key blob.
    pub blob: Vec<u8>,
    ec_digest: Option<&'static CStr>,
}

fn unreadable() -> Error {
    Error::new("the private key could not be read")
}

fn file_malformed() -> Error {
    Error::new("the private key file is malformed")
}

fn wrong_passphrase() -> Error {
    Error::new("incorrect passphrase supplied to decrypt private key")
}

/// Fills in the type and public blob of a loaded key.
fn describe(pkey: Pkey) -> Result<PrivateKey> {
    let mut blob = Vec::new();
    match pkey.base_id() {
        EVP_PKEY_ED25519 => {
            let pk = pkey.raw_public().filter(|pk| pk.len() == 32).ok_or_else(unreadable)?;
            blob.put_string(b"ssh-ed25519");
            blob.put_string(&pk);
            Ok(PrivateKey { pkey, type_name: "ssh-ed25519".into(), blob, ec_digest: None })
        }
        EVP_PKEY_EC => {
            let group = pkey.utf8_param(c"group").ok_or_else(unreadable)?;
            let group_c = CString::new(group.clone()).map_err(|_| unreadable())?;
            // OpenSSL may report a P-256 key as prime256v1 or secp384r1.
            let curve = CURVES
                .iter()
                .rev()
                .find(|c| same_curve(&group_c, c.group))
                .ok_or_else(|| Error::new(format!("unsupported ECDSA curve {group}")))?;
            pkey.set_utf8_param(c"point-format", c"uncompressed");
            let q = pkey
                .octet_param(c"encoded-pub-key")
                .filter(|q| !q.is_empty() && q.len() <= 200 && q[0] == 0x04)
                .ok_or_else(unreadable)?;
            let type_name = format!("ecdsa-sha2-{}", curve.id);
            blob.put_string(type_name.as_bytes());
            blob.put_string(curve.id.as_bytes());
            blob.put_string(&q);
            Ok(PrivateKey { pkey, type_name, blob, ec_digest: Some(curve.digest) })
        }
        EVP_PKEY_RSA => {
            let (Some(n), Some(e)) = (pkey.bn_param(c"n"), pkey.bn_param(c"e")) else {
                return Err(unreadable());
            };
            if n.num_bits() < RSA_MIN_BITS {
                return Err(Error::new("RSA keys must have at least 2048 bits"));
            }
            blob.put_string(b"ssh-rsa");
            if !wire::put_bn(&mut blob, &e) || !wire::put_bn(&mut blob, &n) {
                return Err(unreadable());
            }
            Ok(PrivateKey { pkey, type_name: "ssh-rsa".into(), blob, ec_digest: None })
        }
        _ => Err(Error::new("unsupported private key type")),
    }
}

struct KeyCipher {
    name: &'static str,
    evp: Option<&'static CStr>,
    key_len: usize,
    iv_len: usize,
    block: usize,
    tag_len: usize,
}

static KEY_CIPHERS: [KeyCipher; 9] = [
    KeyCipher { name: "none", evp: None, key_len: 0, iv_len: 0, block: 8, tag_len: 0 },
    KeyCipher {
        name: "aes128-ctr",
        evp: Some(c"AES-128-CTR"),
        key_len: 16,
        iv_len: 16,
        block: 16,
        tag_len: 0,
    },
    KeyCipher {
        name: "aes192-ctr",
        evp: Some(c"AES-192-CTR"),
        key_len: 24,
        iv_len: 16,
        block: 16,
        tag_len: 0,
    },
    KeyCipher {
        name: "aes256-ctr",
        evp: Some(c"AES-256-CTR"),
        key_len: 32,
        iv_len: 16,
        block: 16,
        tag_len: 0,
    },
    KeyCipher {
        name: "aes128-cbc",
        evp: Some(c"AES-128-CBC"),
        key_len: 16,
        iv_len: 16,
        block: 16,
        tag_len: 0,
    },
    KeyCipher {
        name: "aes192-cbc",
        evp: Some(c"AES-192-CBC"),
        key_len: 24,
        iv_len: 16,
        block: 16,
        tag_len: 0,
    },
    KeyCipher {
        name: "aes256-cbc",
        evp: Some(c"AES-256-CBC"),
        key_len: 32,
        iv_len: 16,
        block: 16,
        tag_len: 0,
    },
    KeyCipher {
        name: "aes128-gcm@openssh.com",
        evp: Some(c"AES-128-GCM"),
        key_len: 16,
        iv_len: 12,
        block: 16,
        tag_len: 16,
    },
    KeyCipher {
        name: "aes256-gcm@openssh.com",
        evp: Some(c"AES-256-GCM"),
        key_len: 32,
        iv_len: 12,
        block: 16,
        tag_len: 16,
    },
];

/// Decrypts an encrypted section with the derived key material `km`
/// (key, then IV).
fn decrypt_section(
    cipher: &KeyCipher,
    evp: &CStr,
    km: &[u8; 64],
    data: &[u8],
    tag: &[u8],
) -> Option<Vec<u8>> {
    let fetched = FetchedCipher::fetch(evp)?;
    let mut ctx = CipherCtx::new()?;
    ctx.init_fetched(Some(&fetched), None, None, Direction::Decrypt).then_some(())?;
    if cipher.tag_len != 0 {
        ctx.gcm_set_ivlen(i32::try_from(cipher.iv_len).ok()?).then_some(())?;
    }
    let mut out = vec![0_u8; data.len() + 64];
    let ok = ctx.init_fetched(None, Some(&km[..]), Some(&km[cipher.key_len..]), Direction::Keep)
        && ctx.set_padding(false);
    let mut n = if ok { ctx.update(data, &mut out) } else { None };
    if n.is_some() && cipher.tag_len != 0 {
        let tag16: Option<&[u8; 16]> = tag.try_into().ok();
        if !tag16.is_some_and(|t| ctx.gcm_set_tag(t)) {
            n = None;
        }
    }
    let total = n.and_then(|n| ctx.finalize(&mut out[n..]).map(|m| n + m));
    match total {
        Some(len) if len == data.len() => {
            out.truncate(len);
            Some(out)
        }
        _ => {
            wipe_vec(&mut out);
            None
        }
    }
}

fn load_private_fields(r: &mut Cursor<'_>) -> Result<Pkey> {
    let key_type = wire::string(r).ok_or_else(unreadable)?;
    if key_type == b"ssh-ed25519" {
        let pk = wire::string(r).ok_or_else(unreadable)?;
        let sk = wire::string(r).ok_or_else(unreadable)?;
        if pk.len() != 32 || sk.len() != 64 || sk[32..] != *pk {
            return Err(unreadable());
        }
        return Pkey::from_raw_private(EVP_PKEY_ED25519, &sk[..32]).ok_or_else(unreadable);
    }
    for curve in &CURVES {
        if key_type != format!("ecdsa-sha2-{}", curve.id).as_bytes() {
            continue;
        }
        let id = wire::string(r).ok_or_else(unreadable)?;
        let q = wire::string(r).ok_or_else(unreadable)?;
        let d = wire::bn(r, true).ok_or_else(unreadable)?;
        if id != curve.id.as_bytes() {
            return Err(unreadable());
        }
        let mut bld = ParamBld::new().ok_or_else(unreadable)?;
        bld.utf8(c"group", curve.group);
        bld.octets(c"pub", q);
        bld.bn(c"priv", &d);
        return Pkey::from_data(c"EC", EVP_PKEY_KEYPAIR, &bld).ok_or_else(unreadable);
    }
    if key_type == b"ssh-rsa" {
        let mut next = || wire::bn(r, true).ok_or_else(unreadable);
        let (n, e, d, iqmp, p, q) = (next()?, next()?, next()?, next()?, next()?, next()?);
        let mut ctx = omt_sys::evp::safe::BnCtx::new().ok_or_else(unreadable)?;
        let one = Bn::one().ok_or_else(unreadable)?;
        // The CRT exponents are not stored; derive them.
        let dmp1 = Bn::sub(&p, &one).and_then(|t| Bn::rem(&d, &t, &mut ctx));
        let dmq1 = Bn::sub(&q, &one).and_then(|t| Bn::rem(&d, &t, &mut ctx));
        let (Some(dmp1), Some(dmq1)) = (dmp1, dmq1) else { return Err(unreadable()) };
        let mut bld = ParamBld::new().ok_or_else(unreadable)?;
        bld.bn(c"n", &n);
        bld.bn(c"e", &e);
        bld.bn(c"d", &d);
        bld.bn(c"rsa-factor1", &p);
        bld.bn(c"rsa-factor2", &q);
        bld.bn(c"rsa-exponent1", &dmp1);
        bld.bn(c"rsa-exponent2", &dmq1);
        bld.bn(c"rsa-coefficient1", &iqmp);
        return Pkey::from_data(c"RSA", EVP_PKEY_KEYPAIR, &bld).ok_or_else(unreadable);
    }
    let shown = &key_type[..key_type.len().min(64)];
    Err(Error::new(format!("unsupported private key type {}", String::from_utf8_lossy(shown))))
}

fn parse_openssh(text: &str, passphrase: &[u8]) -> Result<PrivateKey> {
    let begin = text.find(BEGIN_OPENSSH).ok_or_else(file_malformed)?;
    let body_start = begin + BEGIN_OPENSSH.len();
    let end = text[body_start..].find(END_OPENSSH).ok_or_else(file_malformed)? + body_start;
    let mut raw =
        base64_decode(&text.as_bytes()[body_start..end], true).ok_or_else(file_malformed)?;
    let result = parse_openssh_raw(&raw, passphrase);
    wipe_vec(&mut raw);
    result
}

fn parse_openssh_raw(raw: &[u8], passphrase: &[u8]) -> Result<PrivateKey> {
    let rest = raw.strip_prefix(AUTH_MAGIC).ok_or_else(file_malformed)?;
    let mut r = Cursor::new(rest);
    let cname = wire::string(&mut r).ok_or_else(file_malformed)?;
    let kdf = wire::string(&mut r).ok_or_else(file_malformed)?;
    let kdfopts = wire::string(&mut r).ok_or_else(file_malformed)?;
    if wire::u32(&mut r) != Some(1) {
        return Err(file_malformed());
    }
    let public = wire::string(&mut r).ok_or_else(file_malformed)?;
    let enc = wire::string(&mut r).ok_or_else(file_malformed)?;
    let Some(cipher) = KEY_CIPHERS.iter().find(|c| c.name.as_bytes() == cname) else {
        let shown = &cname[..cname.len().min(64)];
        return Err(Error::new(format!(
            "the private key is encrypted with an unsupported cipher ({})",
            String::from_utf8_lossy(shown)
        )));
    };
    let tag = r.rest();
    if tag.len() != cipher.tag_len || enc.len() % cipher.block != 0 {
        return Err(file_malformed());
    }
    let mut plain = match cipher.evp {
        None => {
            if kdf != b"none" {
                return Err(file_malformed());
            }
            enc.to_vec()
        }
        Some(evp) => {
            if kdf != b"bcrypt" {
                return Err(Error::new("the private key uses an unsupported key derivation"));
            }
            if passphrase.is_empty() {
                return Err(Error::new("the private key is encrypted and no passphrase was given"));
            }
            let mut o = Cursor::new(kdfopts);
            let salt = wire::string(&mut o).ok_or_else(file_malformed)?;
            let rounds = wire::u32(&mut o).ok_or_else(file_malformed)?;
            if o.remaining() != 0 || rounds == 0 || rounds > 100_000 {
                return Err(file_malformed());
            }
            let mut km = [0_u8; 64];
            let derived =
                bcrypt_pbkdf(passphrase, salt, &mut km[..cipher.key_len + cipher.iv_len], rounds);
            let plain = if derived { decrypt_section(cipher, evp, &km, enc, tag) } else { None };
            cleanse(&mut km);
            if !derived {
                return Err(file_malformed());
            }
            plain.ok_or_else(wrong_passphrase)?
        }
    };
    let result = parse_private_section(&plain, public);
    wipe_vec(&mut plain);
    result
}

fn parse_private_section(plain: &[u8], public: &[u8]) -> Result<PrivateKey> {
    let mut s = Cursor::new(plain);
    let (Some(check1), Some(check2)) = (wire::u32(&mut s), wire::u32(&mut s)) else {
        return Err(file_malformed());
    };
    if check1 != check2 {
        return Err(wrong_passphrase());
    }
    let pkey = load_private_fields(&mut s)?;
    wire::string(&mut s).ok_or_else(file_malformed)?; // comment
    // Deterministic padding: 1, 2, 3, ...
    if s.rest().iter().enumerate().any(|(i, &b)| usize::from(b) != (i + 1) % 256) {
        return Err(file_malformed());
    }
    let key = describe(pkey)?;
    if key.blob != public {
        return Err(Error::new("the private key does not match its public half"));
    }
    Ok(key)
}

impl PrivateKey {
    pub fn parse(data: &[u8], passphrase: &[u8]) -> Result<Self> {
        if data.len() > KEY_FILE_LIMIT || data.contains(&0) {
            return Err(file_malformed());
        }
        let text = String::from_utf8_lossy(data);
        if text.contains(BEGIN_OPENSSH) {
            return parse_openssh(&text, passphrase);
        }
        let pkey = Pkey::read_pem_private(data, passphrase).ok_or_else(|| {
            Error::new(if passphrase.is_empty() {
                "the private key could not be read; if it is encrypted, supply key_passphrase"
            } else {
                "the private key could not be decrypted or read"
            })
        })?;
        describe(pkey)
    }

    pub fn load(path: &str, passphrase: &[u8]) -> Result<Self> {
        let mut data = sys::read_file(path, KEY_FILE_LIMIT)?;
        let key = Self::parse(&data, passphrase);
        wipe_vec(&mut data);
        key
    }

    /// An SSH signature blob for `alg` over `data`.
    pub fn sign(&self, alg: &str, data: &[u8]) -> Result<Vec<u8>> {
        let failed = || Error::new("signing with the private key failed");
        let mut sig = Vec::new();
        match self.pkey.base_id() {
            EVP_PKEY_ED25519 => {
                let raw =
                    self.pkey.sign(None, data).filter(|s| s.len() == 64).ok_or_else(failed)?;
                sig.put_string(b"ssh-ed25519");
                sig.put_string(&raw);
            }
            EVP_PKEY_EC => {
                let der = self.pkey.sign(self.ec_digest, data).ok_or_else(failed)?;
                let (r, s) = ecdsa_rs_from_der(&der).ok_or_else(failed)?;
                let mut inner = Vec::new();
                inner.put_mpint(&r);
                inner.put_mpint(&s);
                sig.put_string(self.type_name.as_bytes());
                sig.put_string(&inner);
            }
            EVP_PKEY_RSA => {
                let digest = if alg == "rsa-sha2-256" { c"SHA256" } else { c"SHA512" };
                let raw = self.pkey.sign(Some(digest), data).ok_or_else(failed)?;
                sig.put_string(alg.as_bytes());
                sig.put_string(&raw);
            }
            _ => return Err(failed()),
        }
        Ok(sig)
    }
}
