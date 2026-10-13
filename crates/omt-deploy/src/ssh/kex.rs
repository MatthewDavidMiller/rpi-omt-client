//! Key exchange: algorithm negotiation (RFC 4253 section 7), the exchange
//! methods, host-key verification against known_hosts, and key derivation.
//!
//! Methods, in preference order: mlkem768x25519-sha256 (post-quantum
//! hybrid), curve25519-sha256 (RFC 8731), diffie-hellman-group-exchange-
//! sha256 (RFC 4419), and the RFC 8268 MODP groups. Strict key exchange is
//! mandatory: a server that does not offer it is refused, because without it
//! an attacker on the path can delete the first packets after the handshake
//! undetected.

use std::cmp::Ordering;
use std::sync::OnceLock;

use omt_common::buf::{Cursor, wipe_vec};
use omt_common::{Error, Result};
use omt_sys::evp::EVP_PKEY_X25519;
use omt_sys::evp::safe::{Bn, BnCtx, Pkey, pkey_algorithm_available, random_bytes};
use omt_sys::openssl::safe::{Md, MdCtx, cleanse};

use super::cipher::{self, CIPHER_LIST, Kind, MAC_LIST, PacketCipher};
use super::keys::{PublicKey, hostkey_alg_matches};
use super::known_hosts::{self, HostStatus};
use super::msg;
use super::transport::{Recv, Transport};
use super::wire::{self, Put, first_name, namelist_has};
use crate::sys::now_ms;

const STRICT_C: &str = "kex-strict-c-v00@openssh.com";
const STRICT_S: &str = "kex-strict-s-v00@openssh.com";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Method {
    MlKem768X25519,
    Curve25519,
    DhGex,
    DhGroup(u32),
}

struct KexMethod {
    name: &'static str,
    method: Method,
    hash: Md,
}

static METHODS: [KexMethod; 9] = [
    KexMethod { name: "mlkem768x25519-sha256", method: Method::MlKem768X25519, hash: Md::Sha256 },
    KexMethod { name: "curve25519-sha256", method: Method::Curve25519, hash: Md::Sha256 },
    KexMethod {
        name: "curve25519-sha256@libssh.org",
        method: Method::Curve25519,
        hash: Md::Sha256,
    },
    KexMethod {
        name: "diffie-hellman-group-exchange-sha256",
        method: Method::DhGex,
        hash: Md::Sha256,
    },
    KexMethod {
        name: "diffie-hellman-group18-sha512",
        method: Method::DhGroup(8192),
        hash: Md::Sha512,
    },
    KexMethod {
        name: "diffie-hellman-group17-sha512",
        method: Method::DhGroup(6144),
        hash: Md::Sha512,
    },
    KexMethod {
        name: "diffie-hellman-group16-sha512",
        method: Method::DhGroup(4096),
        hash: Md::Sha512,
    },
    KexMethod {
        name: "diffie-hellman-group15-sha512",
        method: Method::DhGroup(3072),
        hash: Md::Sha512,
    },
    KexMethod {
        name: "diffie-hellman-group14-sha256",
        method: Method::DhGroup(2048),
        hash: Md::Sha256,
    },
];

/// Group exchange bounds: no group smaller than the smallest fixed one.
const GEX_MIN: u32 = 2048;
const GEX_PREFERRED: u32 = 4096;
const GEX_MAX: u32 = 8192;
const MLKEM768_PK: usize = 1184;
const MLKEM768_CT: usize = 1088;

fn mlkem_available() -> bool {
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| pkey_algorithm_available(c"ML-KEM-768"))
}

fn kex_list() -> String {
    let mut names: Vec<&str> = METHODS
        .iter()
        .filter(|m| m.method != Method::MlKem768X25519 || mlkem_available())
        .map(|m| m.name)
        .collect();
    names.push("ext-info-c");
    names.push(STRICT_C);
    names.join(",")
}

struct KexInit<'a> {
    lists: [&'a [u8]; 10],
    first_follows: bool,
}

fn parse_kexinit(payload: &[u8]) -> Option<KexInit<'_>> {
    let mut r = Cursor::new(payload);
    if wire::u8(&mut r)? != msg::KEXINIT {
        return None;
    }
    r.skip(16)?;
    let mut lists: [&[u8]; 10] = [&[]; 10];
    for list in &mut lists {
        *list = wire::string(&mut r)?;
    }
    let first_follows = wire::boolean(&mut r)?;
    wire::u32(&mut r)?;
    Some(KexInit { lists, first_follows })
}

/// The first entry of ours that theirs also lists.
fn choose(ours: &str, theirs: &[u8]) -> Option<String> {
    ours.split(',').find(|name| name.len() < 96 && namelist_has(theirs, name)).map(str::to_owned)
}

fn send_kexinit(t: &mut Transport<'_>) -> Result<()> {
    let mut b = vec![msg::KEXINIT];
    let mut cookie = [0_u8; 16];
    if !random_bytes(&mut cookie) {
        return Err(Error::new("randomness is unavailable"));
    }
    b.extend_from_slice(&cookie);
    b.put_string(kex_list().as_bytes());
    b.put_string(t.preferred_hostkeys.as_bytes());
    for list in [CIPHER_LIST, CIPHER_LIST, MAC_LIST, MAC_LIST, "none", "none", "", ""] {
        b.put_string(list.as_bytes());
    }
    b.put_bool(false);
    b.put_u32(0);
    t.i_c.clone_from(&b);
    t.send(&b)
}

fn digest(md: Md, parts: &[&[u8]]) -> Option<Vec<u8>> {
    let mut ctx = MdCtx::new(md)?;
    for part in parts {
        if !ctx.update(part) {
            return None;
        }
    }
    match md {
        Md::Sha1 => ctx.finish::<20>().map(|d| d.to_vec()),
        Md::Sha256 => ctx.finish::<32>().map(|d| d.to_vec()),
        Md::Sha384 => ctx.finish::<48>().map(|d| d.to_vec()),
        Md::Sha512 => ctx.finish::<64>().map(|d| d.to_vec()),
    }
}

/// What a method contributes to the exchange hash and keys.
#[derive(Default)]
struct KexOut {
    /// The shared secret, encoded as the method encodes it.
    k: Vec<u8>,
    /// The host key blob.
    k_s: Vec<u8>,
    sig: Vec<u8>,
    /// Method-specific exchange-hash fields after K_S.
    prefix: Vec<u8>,
}

impl Drop for KexOut {
    fn drop(&mut self) {
        wipe_vec(&mut self.k);
    }
}

fn expect(t: &mut Transport<'_>, kind: u8, deadline: u64) -> Result<()> {
    loop {
        if t.read_packet(deadline)? == Recv::Timeout {
            return Err(Error::new("SSH key exchange timed out"));
        }
        let got = t.msg_type();
        if got == kind {
            return Ok(());
        }
        if got == msg::DISCONNECT {
            return Err(Error::new("the SSH server disconnected during key exchange"));
        }
        // Strict mode allows nothing else during the first exchange; a
        // re-key may still interleave the harmless transport messages.
        if t.first_kex_done && (got == msg::IGNORE || got == msg::DEBUG) {
            continue;
        }
        return Err(Error::new(format!("unexpected SSH message {got} during key exchange")));
    }
}

fn x25519_derive(mine: &Pkey, peer_raw: &[u8], secret: &mut [u8; 32]) -> bool {
    let ok = Pkey::from_raw_public(EVP_PKEY_X25519, peer_raw)
        .is_some_and(|peer| mine.derive(&peer, secret));
    // An all-zero result means the peer sent a low-order point.
    ok && secret.iter().any(|&b| b != 0)
}

/// Reads the common reply: string K_S, string Q_S, string signature.
fn read_ecdh_reply(t: &mut Transport<'_>, o: &mut KexOut, deadline: u64) -> Result<Vec<u8>> {
    expect(t, msg::KEX_31, deadline)?;
    let mut r = Cursor::new(t.body());
    let parts = (wire::string(&mut r), wire::string(&mut r), wire::string(&mut r));
    let (Some(ks), Some(q_s), Some(sig)) = parts else {
        return Err(Error::new("malformed key exchange reply"));
    };
    if r.remaining() != 0 {
        return Err(Error::new("malformed key exchange reply"));
    }
    o.k_s = ks.to_vec();
    o.sig = sig.to_vec();
    Ok(q_s.to_vec())
}

fn run_curve25519(t: &mut Transport<'_>, o: &mut KexOut, deadline: u64) -> Result<()> {
    let mine = Pkey::generate(c"X25519");
    let q_c = mine.as_ref().and_then(Pkey::raw_public).filter(|q| q.len() == 32);
    let (Some(mine), Some(q_c)) = (mine, q_c) else {
        return Err(Error::new("X25519 key generation failed"));
    };
    let mut m = vec![msg::KEX_30];
    m.put_string(&q_c);
    t.send(&m)?;
    let q_s = read_ecdh_reply(t, o, deadline)?;
    let mut secret = [0_u8; 32];
    if q_s.len() != 32 || !x25519_derive(&mine, &q_s, &mut secret) {
        cleanse(&mut secret);
        return Err(Error::new("invalid curve25519 key exchange value"));
    }
    o.prefix.put_string(&q_c);
    o.prefix.put_string(&q_s);
    o.k.put_mpint(&secret);
    cleanse(&mut secret);
    Ok(())
}

fn run_mlkem(t: &mut Transport<'_>, o: &mut KexOut, deadline: u64) -> Result<()> {
    let kem = Pkey::generate(c"ML-KEM-768");
    let ecdh = Pkey::generate(c"X25519");
    let q_c = match (&kem, &ecdh) {
        (Some(k), Some(e)) => match (k.raw_public(), e.raw_public()) {
            (Some(kp), Some(ep)) if kp.len() == MLKEM768_PK && ep.len() == 32 => {
                Some([kp, ep].concat())
            }
            _ => None,
        },
        _ => None,
    };
    let (Some(kem), Some(ecdh), Some(q_c)) = (kem, ecdh, q_c) else {
        return Err(Error::new("ML-KEM-768 key generation failed"));
    };
    let mut m = vec![msg::KEX_30];
    m.put_string(&q_c);
    t.send(&m)?;
    let q_s = read_ecdh_reply(t, o, deadline)?;
    let mut shared = [0_u8; 64];
    let ok = q_s.len() == MLKEM768_CT + 32 && {
        let (ct, peer) = q_s.split_at(MLKEM768_CT);
        let (kem_ss, ecdh_ss) = shared.split_at_mut(32);
        let mut ecdh_secret = [0_u8; 32];
        let ok = kem.decapsulate(ct, kem_ss) && x25519_derive(&ecdh, peer, &mut ecdh_secret);
        ecdh_ss.copy_from_slice(&ecdh_secret);
        cleanse(&mut ecdh_secret);
        ok
    };
    let hashed = if ok { digest(Md::Sha256, &[&shared]) } else { None };
    cleanse(&mut shared);
    let Some(mut hashed) = hashed else {
        return Err(Error::new("invalid mlkem768x25519 key exchange value"));
    };
    o.prefix.put_string(&q_c);
    o.prefix.put_string(&q_s);
    // The hybrid methods encode K as a string, not an mpint.
    o.k.put_string(&hashed);
    wipe_vec(&mut hashed);
    Ok(())
}

/// 1 < v < p - 1
fn dh_value_ok(v: &Bn, p: &Bn) -> bool {
    let Some(mut limit) = p.dup() else { return false };
    limit.sub_word(1) && v.cmp_one() == Ordering::Greater && v.compare(&limit) == Ordering::Less
}

/// Shared by both DH methods once the group is fixed: e out, f and K_S in.
fn run_dh(
    t: &mut Transport<'_>,
    o: &mut KexOut,
    p: &Bn,
    g: &Bn,
    init: u8,
    reply: u8,
    deadline: u64,
) -> Result<()> {
    let generation = || Error::new("Diffie-Hellman key generation failed");
    let mut bn = BnCtx::new().ok_or_else(generation)?;
    // A 512-bit exponent is twice the strength of the largest cipher key.
    let x = Bn::private_random(512).ok_or_else(generation)?;
    let e = Bn::mod_exp_consttime(g, &x, p, &mut bn)
        .filter(|e| dh_value_ok(e, p))
        .ok_or_else(generation)?;
    let mut m = vec![init];
    if !wire::put_bn(&mut m, &e) {
        return Err(generation());
    }
    t.send(&m)?;
    expect(t, reply, deadline)?;
    let invalid = || Error::new("invalid Diffie-Hellman key exchange value");
    let mut r = Cursor::new(t.body());
    let ks = wire::string(&mut r).ok_or_else(invalid)?;
    let f = wire::bn(&mut r, false).ok_or_else(invalid)?;
    let sig = wire::string(&mut r).ok_or_else(invalid)?;
    if r.remaining() != 0 || !dh_value_ok(&f, p) {
        return Err(invalid());
    }
    o.k_s = ks.to_vec();
    o.sig = sig.to_vec();
    let k = Bn::mod_exp_consttime(&f, &x, p, &mut bn).ok_or_else(invalid)?;
    if !(wire::put_bn(&mut o.k, &k)
        && wire::put_bn(&mut o.prefix, &e)
        && wire::put_bn(&mut o.prefix, &f))
    {
        return Err(invalid());
    }
    Ok(())
}

fn run_dh_group(t: &mut Transport<'_>, o: &mut KexOut, bits: u32, deadline: u64) -> Result<()> {
    let p = Bn::rfc3526_prime(bits);
    let g = Bn::from_word(2);
    let (Some(p), Some(g)) = (p, g) else {
        return Err(Error::new("Diffie-Hellman key generation failed"));
    };
    run_dh(t, o, &p, &g, msg::KEX_30, msg::KEX_31, deadline)
}

fn run_dh_gex(t: &mut Transport<'_>, o: &mut KexOut, deadline: u64) -> Result<()> {
    let mut m = vec![msg::KEX_DH_GEX_REQUEST];
    m.put_u32(GEX_MIN);
    m.put_u32(GEX_PREFERRED);
    m.put_u32(GEX_MAX);
    t.send(&m)?;
    expect(t, msg::KEX_31, deadline)?;
    let unacceptable = || Error::new("the SSH server offered an unacceptable Diffie-Hellman group");
    let mut r = Cursor::new(t.body());
    let p = wire::bn(&mut r, false).ok_or_else(unacceptable)?;
    let g = wire::bn(&mut r, false).ok_or_else(unacceptable)?;
    let bits = p.num_bits();
    if r.remaining() != 0
        || bits < GEX_MIN as usize
        || bits > GEX_MAX as usize
        || !p.is_odd()
        || !dh_value_ok(&g, &p)
    {
        return Err(unacceptable());
    }
    o.prefix.put_u32(GEX_MIN);
    o.prefix.put_u32(GEX_PREFERRED);
    o.prefix.put_u32(GEX_MAX);
    if !(wire::put_bn(&mut o.prefix, &p) && wire::put_bn(&mut o.prefix, &g)) {
        return Err(unacceptable());
    }
    run_dh(t, o, &p, &g, msg::KEX_DH_GEX_INIT, msg::KEX_DH_GEX_REPLY, deadline)
}

/// K1 = HASH(K || H || letter || session_id), Kn = HASH(K || H || K1..Kn-1)
fn derive(
    hash: Md,
    k: &[u8],
    h: &[u8],
    letter: u8,
    session_id: &[u8],
    need: usize,
) -> Option<Vec<u8>> {
    let mut material: Vec<u8> = Vec::with_capacity(need + 64);
    while material.len() < need {
        let next = if material.is_empty() {
            digest(hash, &[k, h, &[letter], session_id])?
        } else {
            digest(hash, &[k, h, &material])?
        };
        material.extend_from_slice(&next);
    }
    material.truncate(need);
    Some(material)
}

fn host_key_trusted(t: &Transport<'_>, k_s: &[u8]) -> Result<()> {
    if t.first_kex_done {
        // A re-key must present the key the session was established with.
        if k_s != t.host_key {
            return Err(Error::new("the SSH server changed its host key during a re-key"));
        }
        return Ok(());
    }
    let (host, path) = (&t.host, &t.known_hosts);
    match known_hosts::check(path, host, t.port, k_s)? {
        HostStatus::Known => Ok(()),
        HostStatus::Unknown => Err(Error::new(format!("the host key of {host} is not in {path}"))),
        HostStatus::Changed => Err(Error::new(format!(
            "the host key of {host} does not match {path}; it may be an impersonation"
        ))),
        HostStatus::Revoked => {
            Err(Error::new(format!("the host key of {host} is revoked in {path}")))
        }
    }
}

/// One key exchange; `server_kexinit_received` when the server's KEXINIT is
/// already in the payload (a server-initiated re-key).
pub fn run(t: &mut Transport<'_>, server_kexinit_received: bool) -> Result<()> {
    let deadline = now_ms() + t.idle_ms;
    let mut server_init = if server_kexinit_received { t.payload.clone() } else { Vec::new() };
    send_kexinit(t)?;
    if !server_kexinit_received {
        expect(t, msg::KEXINIT, deadline)?;
        server_init.clone_from(&t.payload);
    }
    t.i_s = server_init;
    let i_s = t.i_s.clone();
    let theirs =
        parse_kexinit(&i_s).ok_or_else(|| Error::new("malformed KEXINIT from the SSH server"))?;
    if !t.first_kex_done {
        if !namelist_has(theirs.lists[0], STRICT_S) {
            return Err(Error::new(
                "the SSH server does not support strict key exchange (the Terrapin mitigation); \
                 update its SSH server",
            ));
        }
        t.strict_kex = true;
    }
    let kex_name = choose(&kex_list(), theirs.lists[0])
        .filter(|n| n != "ext-info-c" && n != STRICT_C)
        .ok_or_else(|| Error::new("no common SSH key exchange algorithm"))?;
    let hostkey = choose(&t.preferred_hostkeys, theirs.lists[1])
        .ok_or_else(|| Error::new("no common SSH host key algorithm"))?;
    let no_cipher = || Error::new("no common SSH cipher");
    let enc_cs = choose(CIPHER_LIST, theirs.lists[2]).ok_or_else(no_cipher)?;
    let enc_sc = choose(CIPHER_LIST, theirs.lists[3]).ok_or_else(no_cipher)?;
    let cs = cipher::find_cipher(enc_cs.as_bytes()).ok_or_else(no_cipher)?;
    let sc = cipher::find_cipher(enc_sc.as_bytes()).ok_or_else(no_cipher)?;
    let no_mac = || Error::new("no common SSH MAC algorithm");
    let mcs = if cs.kind == Kind::Ctr {
        Some(
            choose(MAC_LIST, theirs.lists[4])
                .and_then(|n| cipher::find_mac(n.as_bytes()))
                .ok_or_else(no_mac)?,
        )
    } else {
        None
    };
    let msc = if sc.kind == Kind::Ctr {
        Some(
            choose(MAC_LIST, theirs.lists[5])
                .and_then(|n| cipher::find_mac(n.as_bytes()))
                .ok_or_else(no_mac)?,
        )
    } else {
        None
    };
    if choose("none", theirs.lists[6]).is_none() || choose("none", theirs.lists[7]).is_none() {
        return Err(Error::new(
            "the SSH server requires compression, which this client does not offer",
        ));
    }
    let method = METHODS
        .iter()
        .find(|m| m.name == kex_name)
        .ok_or_else(|| Error::new("no common SSH key exchange algorithm"))?;
    // A server that guessed wrong sends a key exchange packet to discard.
    if theirs.first_follows
        && (first_name(theirs.lists[0]) != kex_name.as_bytes()
            || first_name(theirs.lists[1]) != hostkey.as_bytes())
        && t.read_packet(deadline)? == Recv::Timeout
    {
        return Err(Error::new("SSH key exchange timed out"));
    }

    let mut o = KexOut::default();
    match method.method {
        Method::MlKem768X25519 => run_mlkem(t, &mut o, deadline)?,
        Method::Curve25519 => run_curve25519(t, &mut o, deadline)?,
        Method::DhGex => run_dh_gex(t, &mut o, deadline)?,
        Method::DhGroup(bits) => run_dh_group(t, &mut o, bits, deadline)?,
    }

    // H = HASH(V_C || V_S || I_C || I_S || K_S || method fields || K)
    let mut fields = Vec::new();
    for part in [&t.v_c, &t.v_s, &t.i_c, &t.i_s, &o.k_s] {
        fields.put_string(part);
    }
    fields.extend_from_slice(&o.prefix);
    let h =
        digest(method.hash, &[&fields, &o.k]).ok_or_else(|| Error::new("exchange hash failed"))?;

    let key = PublicKey::parse(&o.k_s)?;
    if !hostkey_alg_matches(&hostkey, &key.name) {
        return Err(Error::new("host key signature verification failed"));
    }
    key.verify(&hostkey, &o.sig, &h)?;
    host_key_trusted(t, &o.k_s)?;
    if !t.first_kex_done {
        t.session_id.clone_from(&h);
        t.host_key.clone_from(&o.k_s);
    }

    // Keys for both directions, then NEWKEYS each way.
    let hash = method.hash;
    let sid = t.session_id.clone();
    let derived = (|| {
        Some((
            derive(hash, &o.k, &h, b'A', &sid, cs.iv_len)?,
            derive(hash, &o.k, &h, b'B', &sid, sc.iv_len)?,
            derive(hash, &o.k, &h, b'C', &sid, cs.key_len)?,
            derive(hash, &o.k, &h, b'D', &sid, sc.key_len)?,
            match mcs {
                Some(m) => derive(hash, &o.k, &h, b'E', &sid, m.key_len)?,
                None => Vec::new(),
            },
            match msc {
                Some(m) => derive(hash, &o.k, &h, b'F', &sid, m.key_len)?,
                None => Vec::new(),
            },
        ))
    })();
    let Some((mut iv_cs, mut iv_sc, mut key_cs, mut key_sc, mut mk_cs, mut mk_sc)) = derived else {
        return Err(Error::new("key derivation failed"));
    };
    let result: Result<()> = (|| {
        t.send(&[msg::NEWKEYS])?;
        if t.strict_kex {
            t.send_seq = 0;
        }
        t.send = PacketCipher::init(cs, mcs, &key_cs, &iv_cs, &mk_cs, true)
            .ok_or_else(|| Error::new("cipher initialization failed"))?;
        expect(t, msg::NEWKEYS, deadline)?;
        if t.strict_kex {
            t.recv_seq = 0;
        }
        t.recv = PacketCipher::init(sc, msc, &key_sc, &iv_sc, &mk_sc, false)
            .ok_or_else(|| Error::new("cipher initialization failed"))?;
        Ok(())
    })();
    for v in [&mut iv_cs, &mut iv_sc, &mut key_cs, &mut key_sc, &mut mk_cs, &mut mk_sc] {
        wipe_vec(v);
    }
    result?;
    t.first_kex_done = true;
    Ok(())
}
