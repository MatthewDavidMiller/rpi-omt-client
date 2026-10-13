//! The packet ciphers: chacha20-poly1305@openssh.com, AES-GCM (RFC 5647 as
//! OpenSSH implements it), and AES-CTR with an HMAC-SHA2 tag, either over
//! the plaintext (RFC 6668) or over the ciphertext (the -etm@openssh.com
//! MACs). There is no CBC, no SHA-1, and no unauthenticated mode.

use std::ffi::CStr;

use omt_sys::evp::safe::{Cipher, CipherCtx, Direction, Mac};
use omt_sys::openssl::safe::{cleanse, memcmp_equal};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    ChaCha,
    Gcm,
    Ctr,
}

#[derive(Debug)]
pub struct CipherAlg {
    pub name: &'static str,
    pub kind: Kind,
    pub key_len: usize,
    pub iv_len: usize,
    pub block: usize,
}

#[derive(Debug)]
pub struct MacAlg {
    pub name: &'static str,
    pub digest: &'static CStr,
    pub key_len: usize,
    pub mac_len: usize,
    pub etm: bool,
}

pub static CIPHERS: [CipherAlg; 6] = [
    CipherAlg {
        name: "chacha20-poly1305@openssh.com",
        kind: Kind::ChaCha,
        key_len: 64,
        iv_len: 0,
        block: 8,
    },
    CipherAlg {
        name: "aes256-gcm@openssh.com",
        kind: Kind::Gcm,
        key_len: 32,
        iv_len: 12,
        block: 16,
    },
    CipherAlg {
        name: "aes128-gcm@openssh.com",
        kind: Kind::Gcm,
        key_len: 16,
        iv_len: 12,
        block: 16,
    },
    CipherAlg { name: "aes256-ctr", kind: Kind::Ctr, key_len: 32, iv_len: 16, block: 16 },
    CipherAlg { name: "aes192-ctr", kind: Kind::Ctr, key_len: 24, iv_len: 16, block: 16 },
    CipherAlg { name: "aes128-ctr", kind: Kind::Ctr, key_len: 16, iv_len: 16, block: 16 },
];

pub const CIPHER_LIST: &str = "chacha20-poly1305@openssh.com,aes256-gcm@openssh.com,\
                               aes128-gcm@openssh.com,aes256-ctr,aes192-ctr,aes128-ctr";

pub static MACS: [MacAlg; 4] = [
    MacAlg {
        name: "hmac-sha2-512-etm@openssh.com",
        digest: c"SHA512",
        key_len: 64,
        mac_len: 64,
        etm: true,
    },
    MacAlg {
        name: "hmac-sha2-256-etm@openssh.com",
        digest: c"SHA256",
        key_len: 32,
        mac_len: 32,
        etm: true,
    },
    MacAlg { name: "hmac-sha2-512", digest: c"SHA512", key_len: 64, mac_len: 64, etm: false },
    MacAlg { name: "hmac-sha2-256", digest: c"SHA256", key_len: 32, mac_len: 32, etm: false },
];

pub const MAC_LIST: &str =
    "hmac-sha2-512-etm@openssh.com,hmac-sha2-256-etm@openssh.com,hmac-sha2-512,hmac-sha2-256";

pub fn find_cipher(name: &[u8]) -> Option<&'static CipherAlg> {
    CIPHERS.iter().find(|c| c.name.as_bytes() == name)
}

pub fn find_mac(name: &[u8]) -> Option<&'static MacAlg> {
    MACS.iter().find(|m| m.name.as_bytes() == name)
}

/// One direction's packet protection; `none` before the first NEWKEYS.
#[derive(Debug)]
pub struct PacketCipher {
    alg: Option<&'static CipherAlg>,
    mac: Option<&'static MacAlg>,
    ctx: Option<CipherCtx>,
    mac_impl: Option<Mac>,
    key: [u8; 64],
    iv: [u8; 16],
    mac_key: [u8; 64],
}

impl Default for PacketCipher {
    fn default() -> Self {
        Self {
            alg: None,
            mac: None,
            ctx: None,
            mac_impl: None,
            key: [0; 64],
            iv: [0; 16],
            mac_key: [0; 64],
        }
    }
}

impl Drop for PacketCipher {
    fn drop(&mut self) {
        cleanse(&mut self.key);
        cleanse(&mut self.iv);
        cleanse(&mut self.mac_key);
    }
}

/// One ChaCha20 operation in OpenSSH's original-construction layout: a
/// 64-bit block counter followed by the 64-bit big-endian sequence number.
/// OpenSSL's 16-byte IV is a 32-bit little-endian counter and a 96-bit
/// nonce, so the counter's high word becomes the nonce's first four bytes.
fn chacha(key: &[u8], seq: u32, counter: u32, data: &mut [u8]) -> bool {
    let mut iv = [0_u8; 16];
    iv[..4].copy_from_slice(&counter.to_le_bytes());
    iv[12..].copy_from_slice(&seq.to_be_bytes()); // the high word is zero
    let Some(mut ctx) = CipherCtx::new() else { return false };
    ctx.init(Some(Cipher::ChaCha20), Some(key), Some(&iv), Direction::Encrypt)
        && ctx.update_in_place(data)
}

fn gcm_next_iv(iv: &mut [u8; 16]) {
    for i in (4..12).rev() {
        iv[i] = iv[i].wrapping_add(1);
        if iv[i] != 0 {
            break;
        }
    }
}

impl PacketCipher {
    pub fn init(
        alg: &'static CipherAlg,
        mac: Option<&'static MacAlg>,
        key: &[u8],
        iv: &[u8],
        mac_key: &[u8],
        encrypt: bool,
    ) -> Option<Self> {
        let mut c = Self::default();
        c.alg = Some(alg);
        c.mac = if alg.kind == Kind::Ctr { mac } else { None };
        c.key[..alg.key_len].copy_from_slice(key.get(..alg.key_len)?);
        c.iv[..alg.iv_len].copy_from_slice(iv.get(..alg.iv_len)?);
        if let Some(m) = c.mac {
            c.mac_key[..m.key_len].copy_from_slice(mac_key.get(..m.key_len)?);
        }
        let dir = if encrypt { Direction::Encrypt } else { Direction::Decrypt };
        match alg.kind {
            Kind::Ctr => {
                c.mac?;
                c.mac_impl = Some(Mac::fetch(c"HMAC")?);
                let cipher = match alg.key_len {
                    16 => Cipher::Aes128Ctr,
                    24 => Cipher::Aes192Ctr,
                    _ => Cipher::Aes256Ctr,
                };
                let mut ctx = CipherCtx::new()?;
                ctx.init(Some(cipher), Some(&c.key), Some(&c.iv), dir).then_some(())?;
                c.ctx = Some(ctx);
            }
            Kind::Gcm => {
                let cipher = if alg.key_len == 16 { Cipher::Aes128Gcm } else { Cipher::Aes256Gcm };
                let mut ctx = CipherCtx::new()?;
                (ctx.init(Some(cipher), None, None, dir)
                    && ctx.gcm_set_ivlen(12)
                    && ctx.init(None, Some(&c.key), None, Direction::Keep))
                .then_some(())?;
                c.ctx = Some(ctx);
            }
            Kind::ChaCha => c.mac_impl = Some(Mac::fetch(c"POLY1305")?),
        }
        Some(c)
    }

    /// Whether keys are in use (after the first NEWKEYS).
    pub fn active(&self) -> bool {
        self.alg.is_some()
    }

    pub fn block(&self) -> usize {
        self.alg.map_or(8, |a| a.block.max(8))
    }

    pub fn tag_len(&self) -> usize {
        match (self.alg, self.mac) {
            (None, _) => 0,
            (Some(a), Some(m)) if a.kind == Kind::Ctr => m.mac_len,
            _ => 16,
        }
    }

    /// Whether the length field is sent in the clear (as AAD or ETM).
    pub fn aead(&self) -> bool {
        match (self.alg, self.mac) {
            (None, _) => false,
            (Some(a), mac) => a.kind != Kind::Ctr || mac.is_some_and(|m| m.etm),
        }
    }

    /// How many bytes must arrive before the packet length can be read.
    pub fn first_len(&self) -> usize {
        match (self.alg, self.mac) {
            (Some(a), Some(m)) if a.kind == Kind::Ctr && !m.etm => a.block,
            _ => 4,
        }
    }

    /// HMAC over seq || data.
    fn hmac(&self, seq: u32, data: &[u8], out: &mut [u8]) -> bool {
        let (Some(mac), Some(imp)) = (self.mac, &self.mac_impl) else { return false };
        imp.compute(
            Some(mac.digest),
            &self.mac_key[..mac.key_len],
            &[&seq.to_be_bytes(), data],
            &mut out[..mac.mac_len],
        )
    }

    fn poly1305(&self, seq: u32, data: &[u8], tag: &mut [u8; 16]) -> bool {
        let Some(imp) = &self.mac_impl else { return false };
        let mut block = [0_u8; 64];
        let ok = chacha(&self.key[..32], seq, 0, &mut block)
            && imp.compute(None, &block[..32], &[data], tag);
        cleanse(&mut block);
        ok
    }

    fn crypt(&mut self, data: &mut [u8]) -> bool {
        self.ctx.as_mut().is_some_and(|c| c.update_in_place(data))
    }

    /// Seals `packet` (length field included) in place and writes the tag
    /// or MAC to `tag`, which holds at least `tag_len()` bytes.
    pub fn seal(&mut self, seq: u32, packet: &mut [u8], tag: &mut [u8]) -> bool {
        let Some(alg) = self.alg else { return true };
        if packet.len() < 4 || tag.len() < self.tag_len() {
            return false;
        }
        match alg.kind {
            Kind::Ctr => {
                if self.mac.is_some_and(|m| m.etm) {
                    self.crypt(&mut packet[4..]) && self.hmac(seq, packet, tag)
                } else {
                    self.hmac(seq, packet, tag) && self.crypt(packet)
                }
            }
            Kind::Gcm => {
                let iv = self.iv;
                let Some(ctx) = self.ctx.as_mut() else { return false };
                let mut tag16 = [0_u8; 16];
                let mut rest = [0_u8; 16];
                let (aad, body) = packet.split_at_mut(4);
                let ok = ctx.init(None, None, Some(&iv), Direction::Keep)
                    && ctx.aad(aad)
                    && ctx.update_in_place(body)
                    && ctx.finalize(&mut rest).is_some()
                    && ctx.gcm_get_tag(&mut tag16);
                gcm_next_iv(&mut self.iv);
                tag[..16].copy_from_slice(&tag16);
                ok
            }
            Kind::ChaCha => {
                let mut tag16 = [0_u8; 16];
                let ok = chacha(&self.key[32..], seq, 0, &mut packet[..4])
                    && chacha(&self.key[..32], seq, 1, &mut packet[4..])
                    && self.poly1305(seq, packet, &mut tag16);
                tag[..16].copy_from_slice(&tag16);
                ok
            }
        }
    }

    /// The packet length from the first `first_len()` bytes received; for
    /// CTR without ETM the first block is decrypted in place.
    pub fn length(&mut self, seq: u32, first: &mut [u8]) -> Option<u32> {
        match (self.alg, self.mac) {
            (Some(a), Some(m)) if a.kind == Kind::Ctr && !m.etm => {
                let block = a.block;
                self.crypt(first.get_mut(..block)?).then_some(())?;
            }
            (Some(a), _) if a.kind == Kind::ChaCha => {
                let mut plain: [u8; 4] = first.get(..4)?.try_into().ok()?;
                chacha(&self.key[32..], seq, 0, &mut plain).then_some(())?;
                return Some(u32::from_be_bytes(plain));
            }
            _ => {}
        }
        Some(u32::from_be_bytes(first.get(..4)?.try_into().ok()?))
    }

    /// Verifies and decrypts a whole packet (length field and body) in
    /// place against `tag`. Afterwards the body at `packet[4..]` is
    /// plaintext.
    pub fn open(&mut self, seq: u32, packet: &mut [u8], tag: &[u8]) -> bool {
        let Some(alg) = self.alg else { return true };
        if packet.len() < 4 || tag.len() < self.tag_len() {
            return false;
        }
        match alg.kind {
            Kind::Ctr => {
                let Some(mac) = self.mac else { return false };
                let mut expect = [0_u8; 64];
                if mac.etm {
                    self.hmac(seq, packet, &mut expect)
                        && memcmp_equal(&expect[..mac.mac_len], &tag[..mac.mac_len])
                        && self.crypt(&mut packet[4..])
                } else {
                    // The first block was decrypted to read the length.
                    let block = alg.block;
                    packet.len() >= block
                        && self.crypt(&mut packet[block..])
                        && self.hmac(seq, packet, &mut expect)
                        && memcmp_equal(&expect[..mac.mac_len], &tag[..mac.mac_len])
                }
            }
            Kind::Gcm => {
                let iv = self.iv;
                let Some(ctx) = self.ctx.as_mut() else { return false };
                let mut tag16 = [0_u8; 16];
                tag16.copy_from_slice(&tag[..16]);
                let mut rest = [0_u8; 16];
                let (aad, body) = packet.split_at_mut(4);
                let ok = ctx.init(None, None, Some(&iv), Direction::Keep)
                    && ctx.aad(aad)
                    && ctx.update_in_place(body)
                    && ctx.gcm_set_tag(&tag16)
                    && ctx.finalize(&mut rest).is_some();
                gcm_next_iv(&mut self.iv);
                ok
            }
            Kind::ChaCha => {
                let mut expect = [0_u8; 16];
                if !self.poly1305(seq, packet, &mut expect) || !memcmp_equal(&expect, &tag[..16]) {
                    return false;
                }
                chacha(&self.key[32..], seq, 0, &mut packet[..4])
                    && chacha(&self.key[..32], seq, 1, &mut packet[4..])
            }
        }
    }
}
