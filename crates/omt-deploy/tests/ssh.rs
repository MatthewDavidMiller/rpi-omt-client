//! Port of the SSH unit suite: wire encoding, base64, every cipher and MAC
//! round-tripping and refusing tampering, the absence of legacy algorithms,
//! known_hosts matching (plain, patterns, ports, revocation, hashing),
//! bcrypt_pbkdf's published vector, and user keys signing what their public
//! halves verify.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use std::process::Command;

use omt_common::buf::Cursor;
use omt_deploy::ssh::bcrypt::bcrypt_pbkdf;
use omt_deploy::ssh::cipher::{self, CIPHER_LIST, MAC_LIST, PacketCipher};
use omt_deploy::ssh::keys::{HOSTKEY_LIST, PrivateKey, PublicKey};
use omt_deploy::ssh::known_hosts::{self, HostStatus};
use omt_deploy::ssh::wire::{self, Put, base64_decode, namelist_has};
use omt_sys::evp::safe::{Mac, random_bytes};

#[test]
fn mpints_are_minimal_and_non_negative() {
    let mut b = Vec::new();
    b.put_mpint(&[0, 0]);
    b.put_mpint(&[0x00, 0x80, 0x01]);
    b.put_mpint(&[0x7f]);
    assert_eq!(b, [0, 0, 0, 0, 0, 0, 0, 3, 0, 0x80, 0x01, 0, 0, 0, 1, 0x7f]);
    let mut r = Cursor::new(&b);
    assert_eq!(wire::mpint(&mut r), Some(&[][..]));
    assert_eq!(wire::mpint(&mut r), Some(&[0x80, 0x01][..]));
    assert_eq!(wire::mpint(&mut r), Some(&[0x7f][..]));
    assert!(wire::mpint(&mut r).is_none());
    assert!(wire::mpint(&mut Cursor::new(&[0, 0, 0, 1, 0x80])).is_none());
}

#[test]
fn strings_and_name_lists_are_bounded() {
    assert!(wire::string(&mut Cursor::new(&[0, 0, 0, 9, b'a', b'b'])).is_none());
    let list = b"curve25519-sha256,ext-info-s,kex-strict-s-v00@openssh.com";
    assert!(namelist_has(list, "ext-info-s"));
    assert!(namelist_has(list, "kex-strict-s-v00@openssh.com"));
    assert!(!namelist_has(list, "curve25519"));
    assert!(!namelist_has(b"", "x"));
}

#[test]
fn base64_is_strict() {
    assert_eq!(base64_decode(b"aGVsbG8=", false).unwrap(), b"hello");
    assert_eq!(base64_decode(b"aGVs\nbG8=\n", true).unwrap(), b"hello");
    assert!(base64_decode(b"aGVsbG8", false).is_none()); // unpadded
    assert!(base64_decode(b"aGVs bG8=", false).is_none()); // space not allowed
    assert!(base64_decode(b"aGVsbG8=aa==", false).is_none()); // data after padding
    assert!(base64_decode(b"aGVsbG9=", false).is_none()); // non-zero trailing bits
}

/// Seals with one direction's state and opens with an identically keyed
/// one, as client and server would, for several packets in a row.
fn cipher_round_trip(cipher_name: &str, mac_name: Option<&str>) {
    let alg = cipher::find_cipher(cipher_name.as_bytes()).unwrap();
    let mac = mac_name.map(|m| cipher::find_mac(m.as_bytes()).unwrap());
    let (mut key, mut iv, mut mk) = ([0_u8; 64], [0_u8; 16], [0_u8; 64]);
    assert!(random_bytes(&mut key) && random_bytes(&mut iv) && random_bytes(&mut mk));
    let mut tx = PacketCipher::init(alg, mac, &key, &iv, &mk, true).unwrap();
    let mut rx = PacketCipher::init(alg, mac, &key, &iv, &mk, false).unwrap();
    let block = tx.block();
    for seq in 7_u32..12 {
        let payload = 20 + seq as usize;
        let aad = if tx.aead() { 4 } else { 0 };
        let mut padding = block - (5 + payload - aad) % block;
        if padding < 4 {
            padding += block;
        }
        let len = 4 + 1 + payload + padding;
        let mut packet = vec![0_u8; len];
        packet[..4].copy_from_slice(&u32::try_from(len - 4).unwrap().to_be_bytes());
        packet[4] = u8::try_from(padding).unwrap();
        for (i, b) in packet.iter_mut().enumerate().skip(5) {
            *b = (i as u32).wrapping_mul(seq) as u8;
        }
        let plain = packet.clone();
        let mut tag = [0_u8; 64];
        assert!(tx.seal(seq, &mut packet, &mut tag));
        assert_ne!(packet[5..5 + payload], plain[5..5 + payload]);
        assert_eq!(rx.length(seq, &mut packet), Some(u32::try_from(len - 4).unwrap()));
        assert!(rx.open(seq, &mut packet, &tag), "{cipher_name}");
        assert_eq!(packet[4..], plain[4..]);
    }
    // A flipped ciphertext bit is refused.
    let mut packet = [0_u8; 64];
    packet[..4].copy_from_slice(&60_u32.to_be_bytes());
    packet[4] = 10;
    let mut tag = [0_u8; 64];
    assert!(tx.seal(99, &mut packet, &mut tag));
    packet[20] ^= 1;
    assert!(rx.length(99, &mut packet).is_some());
    assert!(!rx.open(99, &mut packet, &tag));
}

#[test]
fn every_cipher_and_mac_round_trips() {
    cipher_round_trip("chacha20-poly1305@openssh.com", None);
    cipher_round_trip("aes256-gcm@openssh.com", None);
    cipher_round_trip("aes128-gcm@openssh.com", None);
    cipher_round_trip("aes256-ctr", Some("hmac-sha2-512-etm@openssh.com"));
    cipher_round_trip("aes192-ctr", Some("hmac-sha2-256-etm@openssh.com"));
    cipher_round_trip("aes128-ctr", Some("hmac-sha2-512"));
    cipher_round_trip("aes128-ctr", Some("hmac-sha2-256"));
}

#[test]
fn legacy_algorithms_are_not_offered() {
    assert!(!CIPHER_LIST.contains("cbc") && !CIPHER_LIST.contains("3des"));
    assert!(!MAC_LIST.contains("sha1"));
    assert!(!HOSTKEY_LIST.contains("ssh-rsa") && !HOSTKEY_LIST.contains("ssh-dss"));
    assert!(cipher::find_cipher(b"aes128-cbc").is_none());
    assert!(cipher::find_mac(b"hmac-sha1").is_none());
}

fn blob_for(key_type: &str, fill: u8) -> Vec<u8> {
    let mut b = Vec::new();
    b.put_string(key_type.as_bytes());
    b.put_string(&[fill; 32]);
    b
}

fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let n =
            chunk.iter().enumerate().fold(0_u32, |acc, (i, &b)| acc | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[(n >> (18 - 6 * i) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn check(file: &str, host: &str, port: u16, key: &[u8]) -> HostStatus {
    known_hosts::parse(file.as_bytes(), host, port, key, None)
}

#[test]
fn known_hosts_matching_is_strict() {
    let (key, other, ecdsa) = (
        blob_for("ssh-ed25519", 0x11),
        blob_for("ssh-ed25519", 0x22),
        blob_for("ecdsa-sha2-nistp256", 0x33),
    );
    let (k64, o64, e64) = (base64(&key), base64(&other), base64(&ecdsa));
    let file = format!("# comment\n\npi.local ssh-ed25519 {k64} operator@desk\n");
    assert_eq!(check(&file, "pi.local", 22, &key), HostStatus::Known);
    assert_eq!(check(&file, "PI.LOCAL", 22, &key), HostStatus::Known);
    assert_eq!(check(&file, "pi.local", 22, &other), HostStatus::Changed);
    assert_eq!(check(&file, "pi.local", 22, &ecdsa), HostStatus::Unknown);
    assert_eq!(check(&file, "other.local", 22, &key), HostStatus::Unknown);
    // A non-default port is only ever `[host]:port`.
    assert_eq!(check(&file, "pi.local", 2222, &key), HostStatus::Unknown);
    let file = format!("[pi.local]:2222 ssh-ed25519 {k64}\r\n");
    assert_eq!(check(&file, "pi.local", 2222, &key), HostStatus::Known);

    // Patterns, and a negation that excludes a host the wildcard admits.
    let file = format!("*.local,!evil.local ssh-ed25519 {k64}\n");
    assert_eq!(check(&file, "pi.local", 22, &key), HostStatus::Known);
    assert_eq!(check(&file, "evil.local", 22, &key), HostStatus::Unknown);
    let file = format!("10.0.0.? ssh-ed25519 {k64}\n");
    assert_eq!(check(&file, "10.0.0.7", 22, &key), HostStatus::Known);
    assert_eq!(check(&file, "10.0.0.17", 22, &key), HostStatus::Unknown);

    // Revocation wins over a match, for every host.
    let file = format!("pi.local ssh-ed25519 {k64}\n@revoked * ssh-ed25519 {k64}\n");
    assert_eq!(check(&file, "pi.local", 22, &key), HostStatus::Revoked);
    // CA lines are not trust for a plain key.
    let file = format!("@cert-authority pi.local ssh-ed25519 {k64}\n");
    assert_eq!(check(&file, "pi.local", 22, &key), HostStatus::Unknown);
    // Garbage lines are skipped, not fatal.
    let file = format!("pi.local\npi.local ssh-ed25519 !!!!\npi.local ssh-ed25519 {k64}\n");
    assert_eq!(check(&file, "pi.local", 22, &key), HostStatus::Known);

    // The types a host is known by, for negotiation order.
    let file = format!("pi.local ecdsa-sha2-nistp256 {e64}\npi.local ssh-ed25519 {o64}\n");
    let mut types = String::new();
    known_hosts::parse(file.as_bytes(), "pi.local", 22, &[], Some(&mut types));
    assert_eq!(types, "ecdsa-sha2-nistp256,ssh-ed25519");
}

#[test]
fn hashed_entries_match_by_hmac() {
    let salt: Vec<u8> = (1..=20).collect();
    let mut mac = [0_u8; 20];
    assert!(Mac::fetch(c"HMAC").unwrap().compute(Some(c"SHA1"), &salt, &[b"pi.local"], &mut mac));
    let key = blob_for("ssh-ed25519", 0x44);
    let file = format!("|1|{}|{} ssh-ed25519 {}\n", base64(&salt), base64(&mac), base64(&key));
    assert_eq!(check(&file, "pi.local", 22, &key), HostStatus::Known);
    assert_eq!(check(&file, "pi.locak", 22, &key), HostStatus::Unknown);
}

#[test]
fn bcrypt_pbkdf_matches_the_published_vector() {
    // pyca/bcrypt's kdf test vector: password "password", salt "salt", 4
    // rounds, 32 bytes.
    const EXPECT: [u8; 32] = [
        0x5b, 0xbf, 0x0c, 0xc2, 0x93, 0x58, 0x7f, 0x1c, 0x36, 0x35, 0x55, 0x5c, 0x27, 0x79, 0x65,
        0x98, 0xd4, 0x7e, 0x57, 0x90, 0x71, 0xbf, 0x42, 0x7e, 0x9d, 0x8f, 0xbe, 0x84, 0x2a, 0xba,
        0x34, 0xd9,
    ];
    let mut key = [0_u8; 32];
    assert!(bcrypt_pbkdf(b"password", b"salt", &mut key, 4));
    assert_eq!(key, EXPECT);
    assert!(!bcrypt_pbkdf(b"", b"salt", &mut key, 4));
    assert!(!bcrypt_pbkdf(b"pw", b"salt", &mut key, 0));
}

/// A fresh PEM private key from the openssl command line.
fn pem_key(args: &[&str]) -> Vec<u8> {
    let out = Command::new("openssl").arg("genpkey").args(args).output().expect("openssl genpkey");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    out.stdout
}

fn sign_and_verify(args: &[&str], alg: &str) {
    let key = PrivateKey::parse(&pem_key(args), b"").unwrap();
    let data = b"exchange hash stand-in\0";
    let sig = key.sign(alg, data).unwrap();
    let public = PublicKey::parse(&key.blob).unwrap();
    assert!(public.verify(alg, &sig, data).is_ok(), "{alg}");
    assert!(public.verify(alg, &sig, b"exchange hash stand-iN\0").is_err());
}

#[test]
fn every_user_key_type_signs_what_its_public_half_verifies() {
    sign_and_verify(&["-algorithm", "ED25519"], "ssh-ed25519");
    sign_and_verify(
        &["-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:P-384"],
        "ecdsa-sha2-nistp384",
    );
    let rsa = ["-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048"];
    sign_and_verify(&rsa, "rsa-sha2-512");
    sign_and_verify(&rsa, "rsa-sha2-256");
}

#[test]
fn short_rsa_keys_are_refused() {
    let pem = pem_key(&["-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:1024"]);
    assert!(PrivateKey::parse(&pem, b"").unwrap_err().message().contains("2048"));
}

#[test]
fn malformed_openssh_keys_are_refused() {
    // The markers are split so a secret scanner reading this file does not
    // take the malformed fixtures for keys.
    let begin = concat!("-----BEGIN OPENSSH PRIVATE KEY", "-----\n");
    let end = concat!("-----END OPENSSH PRIVATE KEY", "-----\n");
    for case in [
        format!("{begin}{end}"),
        format!("{begin}b3BlbnNzaC1rZXktdjEA\n{end}"),
        format!("{begin}!!!!\n{end}"),
        begin.to_owned(),
        "not a key at all".to_owned(),
    ] {
        assert!(PrivateKey::parse(case.as_bytes(), b"").is_err(), "{case}");
    }
}

#[test]
fn signatures_for_another_algorithm_are_refused() {
    let key = PrivateKey::parse(&pem_key(&["-algorithm", "ED25519"]), b"").unwrap();
    let sig = key.sign("ssh-ed25519", b"h").unwrap();
    let public = PublicKey::parse(&key.blob).unwrap();
    // The negotiated algorithm and the signature's own name must agree.
    assert!(public.verify("rsa-sha2-512", &sig, b"h").is_err());
}
