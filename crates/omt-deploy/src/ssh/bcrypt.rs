//! bcrypt_pbkdf, the key derivation OpenSSH uses for passphrase-protected
//! private keys, and the Blowfish it is built from. Both follow OpenBSD's
//! definitions (blf.c and bcrypt_pbkdf.c) exactly, because the output has to
//! be bit-identical to what ssh-keygen derived when it encrypted the key.

use omt_sys::openssl::safe::{Md, MdCtx, cleanse};

use super::blowfish_tables::{INIT_P, INIT_S};

/// Blowfish with the expensive key schedule (EksBlowfish); only what
/// bcrypt_pbkdf needs: no modes, no decryption.
pub struct Blowfish {
    s: [[u32; 256]; 4],
    p: [u32; 18],
}

impl std::fmt::Debug for Blowfish {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Blowfish")
    }
}

impl Drop for Blowfish {
    fn drop(&mut self) {
        // The state is derived from the passphrase; black_box keeps the
        // wipe from being optimised away as a dead store.
        self.p = [0; 18];
        self.s = [[0; 256]; 4];
        std::hint::black_box(&mut *self);
    }
}

/// Four bytes of a cyclically repeated stream, big-endian.
fn stream2word(data: &[u8], j: &mut usize) -> u32 {
    let mut word = 0_u32;
    for _ in 0..4 {
        if *j >= data.len() {
            *j = 0;
        }
        word = (word << 8) | u32::from(data[*j]);
        *j += 1;
    }
    word
}

impl Default for Blowfish {
    fn default() -> Self {
        Self::new()
    }
}

impl Blowfish {
    pub fn new() -> Self {
        Self { s: INIT_S, p: INIT_P }
    }

    fn feistel(&self, x: u32) -> u32 {
        let [a, b, c, d] = x.to_be_bytes();
        (self.s[0][usize::from(a)].wrapping_add(self.s[1][usize::from(b)])
            ^ self.s[2][usize::from(c)])
        .wrapping_add(self.s[3][usize::from(d)])
    }

    fn encipher(&self, xl: &mut u32, xr: &mut u32) {
        let (mut l, mut r) = (*xl, *xr);
        l ^= self.p[0];
        for i in (1..=16).step_by(2) {
            r ^= self.feistel(l) ^ self.p[i];
            l ^= self.feistel(r) ^ self.p[i + 1];
        }
        *xl = r ^ self.p[17];
        *xr = l;
    }

    pub fn expand0state(&mut self, key: &[u8]) {
        let mut j = 0;
        for i in 0..18 {
            self.p[i] ^= stream2word(key, &mut j);
        }
        let (mut l, mut r) = (0, 0);
        for i in (0..18).step_by(2) {
            self.encipher(&mut l, &mut r);
            self.p[i] = l;
            self.p[i + 1] = r;
        }
        for i in 0..4 {
            for k in (0..256).step_by(2) {
                self.encipher(&mut l, &mut r);
                self.s[i][k] = l;
                self.s[i][k + 1] = r;
            }
        }
    }

    pub fn expandstate(&mut self, data: &[u8], key: &[u8]) {
        let mut j = 0;
        for i in 0..18 {
            self.p[i] ^= stream2word(key, &mut j);
        }
        j = 0;
        let (mut l, mut r) = (0, 0);
        for i in (0..18).step_by(2) {
            l ^= stream2word(data, &mut j);
            r ^= stream2word(data, &mut j);
            self.encipher(&mut l, &mut r);
            self.p[i] = l;
            self.p[i + 1] = r;
        }
        for i in 0..4 {
            for k in (0..256).step_by(2) {
                l ^= stream2word(data, &mut j);
                r ^= stream2word(data, &mut j);
                self.encipher(&mut l, &mut r);
                self.s[i][k] = l;
                self.s[i][k + 1] = r;
            }
        }
    }

    /// Encrypts 64-bit blocks held as pairs of words, in place.
    pub fn encrypt(&self, data: &mut [u32]) {
        for [l, r] in data.as_chunks_mut::<2>().0 {
            self.encipher(l, r);
        }
    }
}

const WORDS: usize = 8;
const HASHSIZE: usize = WORDS * 4;

fn bcrypt_hash(sha2pass: &[u8; 64], sha2salt: &[u8; 64], out: &mut [u8; HASHSIZE]) {
    const MAGIC: &[u8; HASHSIZE] = b"OxychromaticBlowfishSwatDynamite";
    let mut state = Blowfish::new();
    state.expandstate(sha2salt, sha2pass);
    for _ in 0..64 {
        state.expand0state(sha2salt);
        state.expand0state(sha2pass);
    }
    let mut cdata = [0_u32; WORDS];
    let mut j = 0;
    for word in &mut cdata {
        *word = stream2word(MAGIC, &mut j);
    }
    for _ in 0..64 {
        state.encrypt(&mut cdata);
    }
    for (i, word) in cdata.iter_mut().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&word.to_le_bytes());
        *word = 0;
    }
}

fn sha512(parts: &[&[u8]]) -> Option<[u8; 64]> {
    let mut md = MdCtx::new(Md::Sha512)?;
    for part in parts {
        if !md.update(part) {
            return None;
        }
    }
    md.finish()
}

/// Fills `key` from the passphrase and salt; false for parameters OpenBSD
/// also refuses, or a digest failure.
pub fn bcrypt_pbkdf(pass: &[u8], salt: &[u8], key: &mut [u8], rounds: u32) -> bool {
    if rounds < 1
        || pass.is_empty()
        || salt.is_empty()
        || key.is_empty()
        || key.len() > HASHSIZE * HASHSIZE
        || salt.len() > 1 << 20
    {
        return false;
    }
    let key_len = key.len();
    let stride = key_len.div_ceil(HASHSIZE);
    let mut amt = key_len.div_ceil(stride);
    let mut remaining = key_len;
    let Some(mut sha2pass) = sha512(&[pass]) else { return false };
    let mut out = [0_u8; HASHSIZE];
    let mut tmp = [0_u8; HASHSIZE];
    let mut ok = true;
    let mut count = 1_u32;
    while ok && remaining > 0 {
        let Some(mut sha2salt) = sha512(&[salt, &count.to_be_bytes()]) else {
            ok = false;
            break;
        };
        bcrypt_hash(&sha2pass, &sha2salt, &mut tmp);
        out = tmp;
        for _ in 1..rounds {
            let Some(s) = sha512(&[&tmp]) else {
                ok = false;
                break;
            };
            sha2salt = s;
            bcrypt_hash(&sha2pass, &sha2salt, &mut tmp);
            for (o, t) in out.iter_mut().zip(&tmp) {
                *o ^= t;
            }
        }
        cleanse(&mut sha2salt);
        // Output bytes are interleaved across the blocks, so each block
        // contributes to the whole key rather than to one stretch of it.
        amt = amt.min(remaining);
        let mut i = 0;
        while i < amt {
            let dest = i * stride + (count as usize - 1);
            if dest >= key_len {
                break;
            }
            key[dest] = out[i];
            i += 1;
        }
        remaining -= i;
        count += 1;
    }
    cleanse(&mut sha2pass);
    cleanse(&mut out);
    cleanse(&mut tmp);
    ok
}

#[cfg(test)]
mod tests {
    use super::{INIT_P, INIT_S};

    #[test]
    fn blowfish_tables_are_pi() {
        assert_eq!(INIT_P[0], 0x243f_6a88);
        assert_eq!(INIT_S[3][255], 0x3ac3_72e6);
    }
}
