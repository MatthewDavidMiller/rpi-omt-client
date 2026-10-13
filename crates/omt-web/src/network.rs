//! The OMT SDK's network settings file (settings.xml) and its one field the
//! Web frontend edits: the central Discovery Server.

use std::net::{Ipv4Addr, Ipv6Addr};

use omt_common::xml::{EventKind, Reader};
use omt_common::{Error, Result, fsio};

const SETTINGS_LIMIT: usize = 64 * 1024;
const DEFAULT_PORT: u64 = 6399;

#[derive(Clone, Debug, Default)]
pub struct NetworkConfiguration {
    pub discovery_server: String,
    pub error: String,
}

fn canonical_host(host: &str) -> Option<String> {
    if host.is_empty() || !host.is_ascii() {
        return None;
    }
    if let Ok(v4) = host.parse::<Ipv4Addr>() {
        return Some(v4.to_string());
    }
    if let Ok(v6) = host.parse::<Ipv6Addr>() {
        return Some(format!("[{v6}]"));
    }
    if host.len() > 253 || host.starts_with('.') || host.ends_with('.') {
        return None;
    }
    for label in host.split('.') {
        if label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return None;
        }
    }
    Some(host.to_ascii_lowercase())
}

fn parse_port(s: &str) -> Option<u64> {
    s.parse::<u16>().ok().map(u64::from)
}

/// Normalizes operator input to omt://host:port, or "" for an empty field.
pub fn normalize_server(value: &str) -> Result<String> {
    let v = value.trim();
    if v.is_empty() {
        return Ok(String::new());
    }
    if v.len() > 512 || v.bytes().any(|c| !(32..0x80).contains(&c) || c == 127) {
        return Err(Error::new("Discovery Server contains unsupported characters."));
    }
    let auth = v.strip_prefix("omt://").unwrap_or(v);
    if auth.contains(['/', '?', '#', '@']) {
        return Err(Error::new("Discovery Server must be a host or omt://host:port."));
    }
    let invalid = || Error::new("Discovery Server is invalid.");
    let colons = auth.matches(':').count();
    let (host, port) = if let Some(rest) = auth.strip_prefix('[') {
        let (host, suffix) = rest.split_once(']').ok_or_else(invalid)?;
        let port = if suffix.is_empty() {
            DEFAULT_PORT
        } else {
            suffix.strip_prefix(':').and_then(parse_port).ok_or_else(invalid)?
        };
        (host, port)
    } else if colons == 1 {
        let (host, port) = auth.split_once(':').ok_or_else(invalid)?;
        (host, parse_port(port).ok_or_else(invalid)?)
    } else if colons == 0 {
        (auth, DEFAULT_PORT)
    } else {
        return Err(Error::new("Discovery Server IPv6 addresses must be bracketed."));
    };
    if port == 0 {
        return Err(invalid());
    }
    let canonical =
        canonical_host(host).ok_or_else(|| Error::new("Discovery Server host is invalid."))?;
    Ok(format!("omt://{canonical}:{port}"))
}

/// Where the parts of a settings document are, for splicing.
#[derive(Default)]
struct Shape {
    content_start: usize,
    content_end: usize,
    /// A `<DiscoveryServer>...</DiscoveryServer>` child.
    has_element: bool,
    /// A `<DiscoveryServer/>` child.
    has_empty: bool,
    empty_start: usize,
    empty_end: usize,
    /// `<Settings/>`.
    root_empty: bool,
    root_start: usize,
    root_end: usize,
    /// Offset of `</Settings>`.
    settings_end_start: usize,
}

/// Walks the document with the strict rules: one `<Settings>` root, no
/// doctype, no entity references at all, at most one direct
/// DiscoveryServer child. Returns the shape and the server's raw text.
fn walk(xml: &[u8]) -> Result<(Shape, String)> {
    let mut shape = Shape::default();
    if std::str::from_utf8(xml).is_err() {
        return Err(Error::new("OMT settings XML is invalid: invalid UTF-8"));
    }
    let mut raw = Vec::new();
    let mut r = Reader::new(xml);
    let (mut depth, mut discovery_count) = (0_usize, 0_usize);
    let (mut root_seen, mut in_discovery) = (false, false);
    let one_root = || Error::new("OMT settings XML must contain exactly one root element.");
    let bad_root = || Error::new("OMT settings root must be <Settings>.");
    loop {
        let start = r.position();
        let ev = r
            .next_event()
            .map_err(|_| Error::new("OMT settings XML is invalid: ill-formed document"))?;
        match ev.kind {
            EventKind::Doctype | EventKind::Ref => {
                return Err(Error::new("OMT settings XML must not declare a doctype or entities."));
            }
            EventKind::Start => {
                if depth == 0 && root_seen {
                    return Err(one_root());
                }
                depth += 1;
                if depth == 1 {
                    if ev.name != b"Settings" {
                        return Err(bad_root());
                    }
                    root_seen = true;
                } else if depth == 2 && ev.name == b"DiscoveryServer" {
                    discovery_count += 1;
                    in_discovery = true;
                    shape.has_element = true;
                    shape.content_start = r.position();
                }
            }
            EventKind::Empty => {
                if depth == 0 {
                    if root_seen {
                        return Err(one_root());
                    }
                    if ev.name != b"Settings" {
                        return Err(bad_root());
                    }
                    root_seen = true;
                    shape.root_empty = true;
                    shape.root_start = start;
                    shape.root_end = r.position();
                } else if depth == 1 && ev.name == b"DiscoveryServer" {
                    discovery_count += 1;
                    shape.has_empty = true;
                    shape.empty_start = start;
                    shape.empty_end = r.position();
                }
            }
            EventKind::Text => {
                if in_discovery && depth == 2 {
                    raw.extend_from_slice(ev.content);
                }
            }
            EventKind::End => {
                if depth == 2 && ev.name == b"DiscoveryServer" && in_discovery {
                    in_discovery = false;
                    shape.content_end = start;
                }
                if depth == 1 {
                    shape.settings_end_start = start;
                }
                depth = depth.saturating_sub(1);
            }
            EventKind::Eof => {
                if !root_seen || depth != 0 {
                    return Err(Error::new("OMT settings XML is invalid."));
                }
                if discovery_count > 1 {
                    return Err(Error::new(
                        "OMT settings contain duplicate DiscoveryServer entries.",
                    ));
                }
                // Text runs split only at markup, so the collected bytes are
                // whole UTF-8 sequences of a document already checked valid.
                return Ok((shape, String::from_utf8_lossy(&raw).into_owned()));
            }
            _ => {}
        }
    }
}

/// Strict read of the configured server from a settings document.
pub fn parse_server(xml: &[u8]) -> Result<String> {
    let (_, raw) = walk(xml)?;
    normalize_server(&raw)
}

pub fn read_network_configuration(path: &str) -> NetworkConfiguration {
    let parsed = match fsio::read_bounded(path, SETTINGS_LIMIT) {
        Ok(None) => return NetworkConfiguration::default(),
        Ok(Some(xml)) => parse_server(&xml),
        Err(e) => Err(e),
    };
    match parsed {
        Ok(discovery_server) => NetworkConfiguration { discovery_server, error: String::new() },
        Err(e) => NetworkConfiguration { discovery_server: String::new(), error: e.to_string() },
    }
}

/// Rewrites only the DiscoveryServer value, keeping every other byte of the
/// operator's file; `None` when the document already says the same.
pub fn update_settings_xml(xml: &[u8], normalized: &str) -> Result<Option<Vec<u8>>> {
    // A strict parse before any transform; then only the value is spliced.
    if parse_server(xml)? == normalized {
        return Ok(None);
    }
    let (shape, _) = walk(xml)?;
    let element = format!("<DiscoveryServer>{normalized}</DiscoveryServer>");
    let mut out = Vec::with_capacity(xml.len() + element.len() + 32);
    if shape.has_element {
        out.extend_from_slice(&xml[..shape.content_start]);
        out.extend_from_slice(normalized.as_bytes());
        out.extend_from_slice(&xml[shape.content_end..]);
    } else if shape.has_empty {
        out.extend_from_slice(&xml[..shape.empty_start]);
        out.extend_from_slice(element.as_bytes());
        out.extend_from_slice(&xml[shape.empty_end..]);
    } else if shape.root_empty {
        out.extend_from_slice(&xml[..shape.root_start]);
        out.extend_from_slice(format!("<Settings>\n  {element}\n</Settings>").as_bytes());
        out.extend_from_slice(&xml[shape.root_end..]);
    } else {
        // Indent like the line the closing tag starts, when it starts one.
        let at = shape.settings_end_start;
        out.extend_from_slice(&xml[..at]);
        let line_start = at == 0 || xml[at - 1] == b'\n';
        if line_start {
            out.extend_from_slice(b"  ");
        }
        out.extend_from_slice(element.as_bytes());
        if line_start {
            out.push(b'\n');
        }
        out.extend_from_slice(&xml[at..]);
    }
    if out.last() != Some(&b'\n') {
        out.push(b'\n');
    }
    if out.len() > SETTINGS_LIMIT {
        return Err(Error::new("updated OMT settings exceed their size limit"));
    }
    Ok(Some(out))
}

/// True when the file changed, false when it already matched.
pub fn save_network_configuration(path: &str, value: &str) -> Result<bool> {
    let normalized = normalize_server(value)?;
    let current = fsio::read_bounded(path, SETTINGS_LIMIT)?
        .unwrap_or_else(|| b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<Settings />\n".to_vec());
    match update_settings_xml(&current, &normalized)? {
        None => Ok(false),
        Some(updated) => {
            fsio::atomic_replace(path, &updated, SETTINGS_LIMIT)?;
            Ok(true)
        }
    }
}
