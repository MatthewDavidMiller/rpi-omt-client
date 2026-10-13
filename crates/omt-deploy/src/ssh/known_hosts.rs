//! OpenSSH known_hosts, as sshd(8) documents the format: plain, wildcard,
//! and negated host patterns, `[host]:port` for a non-default port, hashed
//! `|1|` entries, and `@revoked` markers. `@cert-authority` lines are
//! skipped, so a host trusted only through a CA is unknown here: this
//! client does not verify certificates.
//!
//! Checking is strict. A matching key is the only success; a host with no
//! entry for the offered key is refused, and the file has to exist.

use omt_common::buf::Cursor;
use omt_common::{Error, Result};
use omt_sys::evp::safe::Mac;

use super::wire::{self, base64_decode};
use crate::sys;

const KNOWN_HOSTS_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostStatus {
    Known,
    Unknown,
    Changed,
    Revoked,
}

/// Glob match with `*` and `?`, case-insensitively, as OpenSSH's
/// match_pattern.
fn glob(s: &[u8], p: &[u8]) -> bool {
    let (mut si, mut pi) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while si < s.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi].eq_ignore_ascii_case(&s[si])) {
            si += 1;
            pi += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            pi += 1;
            mark = si;
        } else if let Some(st) = star {
            pi = st + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

/// `|1|base64(salt)|base64(HMAC-SHA1(salt, name))`
fn hashed_match(entry: &[u8], name: &str) -> bool {
    let Some(rest) = entry.strip_prefix(b"|1|") else { return false };
    let Some(bar) = rest.iter().position(|&b| b == b'|') else { return false };
    let (Some(salt), Some(hash)) =
        (base64_decode(&rest[..bar], false), base64_decode(&rest[bar + 1..], false))
    else {
        return false;
    };
    if salt.len() != 20 || hash.len() != 20 {
        return false;
    }
    let mut mac = [0_u8; 20];
    Mac::fetch(c"HMAC")
        .is_some_and(|m| m.compute(Some(c"SHA1"), &salt, &[name.as_bytes()], &mut mac))
        && mac == hash[..]
}

/// Whether a hostnames field names `name`. A matching negated pattern
/// excludes the line however the other patterns match.
fn hosts_match(field: &[u8], name: &str) -> bool {
    if field.first() == Some(&b'|') {
        return hashed_match(field, name);
    }
    let mut positive = false;
    for pattern in field.split(|&b| b == b',') {
        let (negated, pat) = match pattern.strip_prefix(b"!") {
            Some(p) => (true, p),
            None => (false, pattern),
        };
        if pat.is_empty() {
            continue;
        }
        if glob(name.as_bytes(), pat) {
            if negated {
                return false;
            }
            positive = true;
        }
    }
    positive
}

/// The key type a blob declares, or an empty slice.
fn blob_type(blob: &[u8]) -> &[u8] {
    wire::string(&mut Cursor::new(blob)).unwrap_or(&[])
}

fn lookup_name(host: &str, port: u16) -> String {
    let lowered: String = host.chars().take(255).collect::<String>().to_ascii_lowercase();
    if port == 22 { lowered } else { format!("[{lowered}]:{port}") }
}

/// Looks up `host` (port 22) or `[host]:port`; with `types`, also collects
/// the key types listed for the host, comma-separated in file order, so
/// negotiation can prefer an algorithm whose key is already trusted.
pub fn parse(
    text: &[u8],
    host: &str,
    port: u16,
    key_blob: &[u8],
    mut types: Option<&mut String>,
) -> HostStatus {
    let name = lookup_name(host, port);
    let offered_type = blob_type(key_blob);
    let (mut known, mut revoked, mut same_type) = (false, false, false);
    for raw_line in text.split(|&b| b == b'\n') {
        let line = raw_line.strip_suffix(b"\r").unwrap_or(raw_line);
        let mut fields = line.split(|&b| b == b' ' || b == b'\t').filter(|f| !f.is_empty());
        let Some(mut field) = fields.next() else { continue };
        if field.first() == Some(&b'#') {
            continue;
        }
        let mut is_revoked = false;
        if field.first() == Some(&b'@') {
            if field != b"@revoked" {
                continue; // @cert-authority and anything unknown
            }
            is_revoked = true;
            let Some(next) = fields.next() else { continue };
            field = next;
        }
        let hosts = field;
        let (Some(_key_type), Some(key_data)) = (fields.next(), fields.next()) else { continue };
        let Some(line_key) = base64_decode(key_data, false) else { continue };
        let line_type = blob_type(&line_key);
        let equal = !key_blob.is_empty() && line_key == key_blob;
        if is_revoked {
            // A revoked key is refused for every host it might appear for.
            revoked |= equal;
            continue;
        }
        if !hosts_match(hosts, &name) {
            continue;
        }
        known |= equal;
        same_type |= !offered_type.is_empty() && line_type == offered_type;
        if let Some(types) = types.as_deref_mut()
            && !line_type.is_empty()
        {
            if !types.is_empty() {
                types.push(',');
            }
            types.push_str(&String::from_utf8_lossy(line_type));
        }
    }
    if revoked {
        HostStatus::Revoked
    } else if known {
        HostStatus::Known
    } else if same_type {
        HostStatus::Changed
    } else {
        HostStatus::Unknown
    }
}

pub fn missing_message(path: &str) -> String {
    format!(
        "OpenSSH known_hosts was not found at {path}. Connect with ssh first and verify the \
         Raspberry Pi host key."
    )
}

fn read(path: &str) -> Result<Vec<u8>> {
    if !sys::is_file(path) {
        return Err(Error::new(missing_message(path)));
    }
    sys::read_file(path, KNOWN_HOSTS_LIMIT)
}

pub fn check(path: &str, host: &str, port: u16, key_blob: &[u8]) -> Result<HostStatus> {
    Ok(parse(&read(path)?, host, port, key_blob, None))
}

pub fn types(path: &str, host: &str, port: u16) -> Result<String> {
    let mut types = String::new();
    parse(&read(path)?, host, port, &[], Some(&mut types));
    Ok(types)
}
