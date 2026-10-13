//! Web GUI authentication: the operator password, the persistent session
//! registry, and CSRF tokens.
//!
//! Sessions are random 256-bit identifiers; the registry on disk stores only
//! HMAC(secret, id), so reading it does not yield a usable cookie. Each
//! record also carries HMAC(secret, password), which is how changing the
//! password ends every existing session. CSRF tokens are
//! HMAC(secret, scope, nonce) and are compared in constant time.

use std::env;
use std::fmt::Write as _;
use std::fs;

use omt_common::buf::{wipe_string, wipe_vec};
use omt_common::timefmt::unix_seconds;
use omt_common::{Error, Result, fsio, json, rand};
use omt_crypto::{cleanse, ct_equal, hmac_sha256_hex, pbkdf2_sha256, scrypt};

use crate::settings::Settings;

pub const MINIMUM_PASSWORD_BYTES: usize = 12;
pub const MAXIMUM_PASSWORD_BYTES: usize = 128;
pub const SESSION_ID_HEX: usize = 64;

const SECRET_LIMIT: usize = 256;
const PASSWORD_LIMIT: usize = 16 * 1024;
const REGISTRY_LIMIT: usize = 64 * 1024;
const MAXIMUM_SESSIONS: usize = 64;
const PBKDF2_ITERATIONS: u32 = 600_000;

/// A stored password: a Werkzeug PBKDF2-SHA256 or scrypt hash, or plaintext.
#[derive(Debug)]
pub enum Password {
    Plain(Vec<u8>),
    Pbkdf2 { iterations: u32, salt: Vec<u8>, digest: Vec<u8> },
    Scrypt { n: u64, r: u32, p: u32, salt: Vec<u8>, digest: Vec<u8> },
}

impl Drop for Password {
    fn drop(&mut self) {
        if let Password::Plain(p) = self {
            wipe_vec(p);
        }
    }
}

fn hex_decode(s: &[u8], cap: usize) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || s.len() / 2 > cap {
        return None;
    }
    let nibble = |c: u8| char::from(c).to_digit(16).and_then(|d| u8::try_from(d).ok());
    s.as_chunks::<2>().0.iter().map(|&[hi, lo]| Some(nibble(hi)? << 4 | nibble(lo)?)).collect()
}

/// u32::from_str: ASCII digits with an optional leading '+'.
fn parse_u32(s: &[u8]) -> Option<u32> {
    std::str::from_utf8(s).ok()?.parse().ok()
}

fn find(s: &[u8], c: u8) -> Option<usize> {
    s.iter().position(|&b| b == c)
}

impl Password {
    /// Parses a stored password. Any other pbkdf2:/argon2: prefix is refused.
    pub fn parse(value: &[u8]) -> Result<Self> {
        let fail = |m: &str| Err(Error::new(m));
        if let Some(rest) = value.strip_prefix(b"pbkdf2:sha256:") {
            let Some(d1) = find(rest, b'$') else { return fail("invalid PBKDF2 password hash") };
            let Some(d2) = find(&rest[d1 + 1..], b'$').map(|i| d1 + 1 + i) else {
                return fail("invalid PBKDF2 password hash");
            };
            let Some(iterations) = parse_u32(&rest[..d1]).filter(|&i| i != 0) else {
                return fail("invalid PBKDF2 iterations");
            };
            let Some(digest) = hex_decode(&rest[d2 + 1..], 64) else {
                return fail("invalid hexadecimal value");
            };
            if digest.len() != 32 {
                return fail("invalid PBKDF2 SHA-256 digest length");
            }
            return Ok(Password::Pbkdf2 { iterations, salt: rest[d1 + 1..d2].to_vec(), digest });
        }
        if let Some(rest) = value.strip_prefix(b"scrypt:") {
            let Some(d1) = find(rest, b'$') else { return fail("invalid scrypt password hash") };
            // Exactly N:r:p, each a u32.
            let params_text = &rest[..d1];
            let mut parts = params_text.split(|&b| b == b':');
            let mut params = [0_u32; 3];
            for (i, (missing, invalid)) in [
                ("missing scrypt N", "invalid scrypt N"),
                ("missing scrypt r", "invalid scrypt r"),
                ("missing scrypt p", "invalid scrypt p"),
            ]
            .into_iter()
            .enumerate()
            {
                let Some(part) = parts.next() else { return fail(missing) };
                let Some(v) = parse_u32(part) else { return fail(invalid) };
                params[i] = v;
            }
            if parts.next().is_some() {
                return fail("invalid scrypt parameters");
            }
            let [n, r, p] = params;
            if n < 2 || !n.is_power_of_two() {
                return fail("invalid scrypt parameters");
            }
            let Some(d2) = find(&rest[d1 + 1..], b'$').map(|i| d1 + 1 + i) else {
                return fail("invalid scrypt password hash");
            };
            let digest_text = &rest[d2 + 1..];
            let out_len = digest_text.len() / 2;
            if r == 0
                || p == 0
                || u64::from(r) * u64::from(p) >= 1 << 30
                || !(10..=64).contains(&out_len)
            {
                return fail("unsupported scrypt parameters");
            }
            let Some(digest) = hex_decode(digest_text, 64) else {
                return fail("invalid hexadecimal value");
            };
            return Ok(Password::Scrypt {
                n: u64::from(n),
                r,
                p,
                salt: rest[d1 + 1..d2].to_vec(),
                digest,
            });
        }
        if value.starts_with(b"argon2:") || value.starts_with(b"pbkdf2:") {
            return fail("The Web GUI password hash uses an unsupported format");
        }
        Ok(Password::Plain(value.to_vec()))
    }

    pub fn verify(&self, supplied: &[u8]) -> bool {
        let mut actual = [0_u8; 64];
        let (ok, expected) = match self {
            Password::Plain(p) => return ct_equal(p, supplied),
            Password::Pbkdf2 { iterations, salt, digest } => {
                let out = &mut actual[..digest.len()];
                (pbkdf2_sha256(supplied, salt, *iterations, out).is_ok(), digest)
            }
            Password::Scrypt { n, r, p, salt, digest } => {
                let out = &mut actual[..digest.len()];
                (scrypt(supplied, salt, *n, *r, *p, out).is_ok(), digest)
            }
        };
        let equal = ok && ct_equal(&actual[..expected.len()], expected);
        cleanse(&mut actual);
        equal
    }
}

#[derive(Clone, Debug)]
struct Record {
    key: String,
    expires_at: u64,
    password_digest: String,
}

fn now_epoch() -> u64 {
    u64::try_from(unix_seconds()).unwrap_or(0)
}

fn keyed_hex(key: &[u8], data: &[u8]) -> Option<String> {
    hmac_sha256_hex(key, data).ok().filter(|h| h.len() == SESSION_ID_HEX)
}

#[derive(Debug)]
pub struct Auth {
    registry_path: String,
    session_lifetime_s: u64,
    secret: Vec<u8>,
    password: Password,
    password_digest: String,
}

impl Drop for Auth {
    fn drop(&mut self) {
        wipe_vec(&mut self.secret);
    }
}

/// Reads a file whose trimmed content must be non-empty; `Ok(None)` when it
/// is missing or blank.
fn read_trimmed(path: &str, limit: usize) -> Result<Option<String>> {
    match fsio::read_text(path, limit)? {
        None => Ok(None),
        Some(mut text) => {
            let trimmed = text.trim().to_owned();
            wipe_string(&mut text);
            Ok(if trimmed.is_empty() { None } else { Some(trimmed) })
        }
    }
}

fn emergency_password() -> Option<String> {
    env::var_os("OMT_WEB_PASSWORD")
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string_lossy().into_owned())
}

impl Auth {
    pub fn load(s: &Settings) -> Result<Self> {
        let missing_secret = || Error::new("The Web secret is missing or unsafe");
        let secret = read_trimmed(&format!("{}/web_secret", s.config_dir), SECRET_LIMIT)?
            .ok_or_else(missing_secret)?
            .into_bytes();
        let mut password = match emergency_password() {
            Some(p) => p,
            None => read_trimmed(&s.password_file, PASSWORD_LIMIT)?
                .ok_or_else(|| Error::new("The Web GUI password file is missing or unsafe"))?,
        };
        let digest = keyed_hex(&secret, password.as_bytes());
        let parsed = Password::parse(password.as_bytes());
        wipe_string(&mut password);
        let password_digest =
            digest.ok_or_else(|| Error::new("unable to derive the password digest"))?;
        Ok(Self {
            registry_path: format!("{}/web_sessions.json", s.config_dir),
            session_lifetime_s: s.session_lifetime_s,
            secret,
            password: parsed?,
            password_digest,
        })
    }

    fn session_digest(&self, session_id: &str) -> Option<String> {
        keyed_hex(&self.secret, session_id.as_bytes())
    }

    /// Any unreadable, malformed, or foreign-version registry reads as
    /// empty: a corrupt file costs the operator a login, never access.
    fn read_registry(&self) -> Vec<Record> {
        let Ok(Some(text)) = fsio::read_text(&self.registry_path, REGISTRY_LIMIT) else {
            return Vec::new();
        };
        let Ok(root) = json::parse(text.as_bytes()) else { return Vec::new() };
        let version = root.get("version").and_then(json::Value::as_u64);
        let Some(sessions) = root.get("sessions").and_then(json::Value::as_object) else {
            return Vec::new();
        };
        if !root.only_keys(&["version", "sessions"]) || version != Some(2) {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (key, rec) in sessions {
            let expires = rec.get("expires_at").and_then(json::Value::as_u64);
            let digest = rec.get("password_digest").and_then(json::Value::as_str);
            match (expires, digest) {
                (Some(expires_at), Some(digest))
                    if rec.only_keys(&["expires_at", "password_digest"])
                        && digest.len() <= SESSION_ID_HEX
                        && key.len() <= SESSION_ID_HEX
                        && out.len() < MAXIMUM_SESSIONS + 1 =>
                {
                    out.push(Record {
                        key: key.clone(),
                        expires_at,
                        password_digest: digest.to_owned(),
                    });
                }
                _ => return Vec::new(),
            }
        }
        out
    }

    fn write_registry(&self, records: &mut [Record]) -> Result<()> {
        // Keys sorted, as the BTreeMap the Rust frontend serialized.
        records.sort_by(|a, b| a.key.cmp(&b.key));
        let mut out = String::from("{\"version\":2,\"sessions\":{");
        for (i, r) in records.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            json::write_string(&mut out, &r.key);
            let _ = write!(out, ":{{\"expires_at\":{},\"password_digest\":", r.expires_at);
            json::write_string(&mut out, &r.password_digest);
            out.push('}');
        }
        out.push_str("}}\n");
        if out.len() > REGISTRY_LIMIT {
            return Err(Error::new("session registry exceeds its size limit"));
        }
        fsio::atomic_replace(&self.registry_path, out.as_bytes(), REGISTRY_LIMIT)
    }

    /// `Ok(Some(id))` with a fresh 64-hex session on a correct password,
    /// `Ok(None)` for a wrong one, and an error when the registry could not
    /// be written. `previous` is revoked when given.
    pub fn authenticate(&self, password: &[u8], previous: Option<&str>) -> Result<Option<String>> {
        if !self.password.verify(password) {
            return Ok(None);
        }
        let session = rand::random_hex(32)?;
        let now = now_epoch();
        let previous_digest = previous.and_then(|p| self.session_digest(p));
        let mut records: Vec<Record> = self
            .read_registry()
            .into_iter()
            .filter(|r| {
                r.expires_at > now
                    && r.password_digest == self.password_digest
                    && previous_digest.as_deref() != Some(r.key.as_str())
            })
            .collect();
        let key = self
            .session_digest(&session)
            .ok_or_else(|| Error::new("unable to derive the session key"))?;
        let fresh = Record {
            key,
            expires_at: now.saturating_add(self.session_lifetime_s),
            password_digest: self.password_digest.clone(),
        };
        // Replace an equal key (astronomically unlikely) rather than
        // duplicate it.
        match records.iter().rposition(|r| r.key == fresh.key) {
            Some(i) => records[i] = fresh,
            None => records.push(fresh),
        }
        // Evict the soonest-expiring sessions past the cap.
        while records.len() > MAXIMUM_SESSIONS {
            let oldest = records
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| (a.expires_at, &a.key).cmp(&(b.expires_at, &b.key)))
                .map_or(0, |(i, _)| i);
            records.swap_remove(oldest);
        }
        self.write_registry(&mut records)?;
        Ok(Some(session))
    }

    pub fn is_current(&self, session_id: &str) -> bool {
        let Some(digest) = self.session_digest(session_id) else { return false };
        let now = now_epoch();
        self.read_registry().iter().any(|r| {
            r.key == digest && r.expires_at > now && r.password_digest == self.password_digest
        })
    }

    pub fn revoke(&self, session_id: &str) -> Result<()> {
        let digest = self
            .session_digest(session_id)
            .ok_or_else(|| Error::new("unable to derive the session key"))?;
        let mut records = self.read_registry();
        match records.iter().position(|r| r.key == digest) {
            Some(i) => {
                records.swap_remove(i);
                self.write_registry(&mut records)
            }
            None => Ok(()),
        }
    }

    pub fn csrf_token(&self, scope: &str, nonce: &str) -> Option<String> {
        let mut msg = Vec::with_capacity(6 + scope.len() + nonce.len());
        msg.extend_from_slice(b"csrf\0");
        msg.extend_from_slice(scope.as_bytes());
        msg.push(0);
        msg.extend_from_slice(nonce.as_bytes());
        if msg.len() > 4096 {
            return None;
        }
        keyed_hex(&self.secret, &msg)
    }

    pub fn verify_csrf(&self, scope: &str, nonce: &str, token: &str) -> bool {
        self.csrf_token(scope, nonce).is_some_and(|e| ct_equal(e.as_bytes(), token.as_bytes()))
    }
}

pub fn validate_new_password(password: &str) -> Result<()> {
    if !(MINIMUM_PASSWORD_BYTES..=MAXIMUM_PASSWORD_BYTES).contains(&password.len()) {
        return Err(Error::new(format!(
            "The Web GUI password must contain {MINIMUM_PASSWORD_BYTES}-{MAXIMUM_PASSWORD_BYTES} \
             UTF-8 bytes"
        )));
    }
    if password.chars().any(char::is_control) {
        return Err(Error::new("The Web GUI password must not contain control characters"));
    }
    Ok(())
}

/// "pbkdf2:sha256:600000$SALT$DIGEST\n" for a new password.
pub fn encode_password(password: &str) -> Result<String> {
    let salt = rand::random_hex(16)?;
    let mut digest = [0_u8; 32];
    let derived =
        pbkdf2_sha256(password.as_bytes(), salt.as_bytes(), PBKDF2_ITERATIONS, &mut digest);
    let out = derived.ok().map(|()| {
        format!("pbkdf2:sha256:{PBKDF2_ITERATIONS}${salt}${}\n", omt_common::buf::hex(&digest))
    });
    cleanse(&mut digest);
    out.ok_or_else(|| Error::new("unable to derive the password hash"))
}

pub fn replace_password(s: &Settings, password: &str) -> Result<()> {
    if emergency_password().is_some() {
        return Err(Error::new(
            "OMT_WEB_PASSWORD overrides the password file; remove the emergency override first",
        ));
    }
    validate_new_password(password)?;
    let mut encoded = encode_password(password)?;
    let result = fsio::atomic_replace(&s.password_file, encoded.as_bytes(), PASSWORD_LIMIT);
    wipe_string(&mut encoded);
    result
}

/// Creates the Web secret and, when no password is set, a random one, which
/// is returned (`None` when a password already existed).
pub fn initialize(s: &Settings) -> Result<Option<String>> {
    fsio::mkdir_all(&s.config_dir, 0o777)?;
    let secret_path = format!("{}/web_secret", s.config_dir);
    let legacy_path = format!("{}/flask_secret", s.config_dir);
    if read_trimmed(&secret_path, SECRET_LIMIT)?.is_none() {
        let mut fresh = match read_trimmed(&legacy_path, SECRET_LIMIT)? {
            Some(old) => old,
            None => rand::random_hex(32)?,
        };
        fresh.push('\n');
        let result = fsio::atomic_replace(&secret_path, fresh.as_bytes(), SECRET_LIMIT);
        wipe_string(&mut fresh);
        result?;
    }
    if read_trimmed(&s.password_file, PASSWORD_LIMIT)?.is_some() {
        return Ok(None);
    }
    let generated = rand::random_hex(16)?;
    let mut encoded = encode_password(&generated)?;
    let result = fsio::atomic_replace(&s.password_file, encoded.as_bytes(), PASSWORD_LIMIT);
    wipe_string(&mut encoded);
    result?;
    Ok(Some(generated))
}

pub fn remove_legacy_secret(s: &Settings) {
    let legacy = format!("{}/flask_secret", s.config_dir);
    if fs::metadata(&legacy).is_ok_and(|m| m.is_file()) {
        let _ = fs::remove_file(&legacy);
    }
}
