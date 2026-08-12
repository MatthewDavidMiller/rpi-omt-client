use crate::io::{atomic_replace, read_bounded};
use quick_xml::{
    Reader, Writer,
    events::{BytesEnd, BytesStart, BytesText, Event},
};
use std::{
    io::Cursor,
    net::{Ipv4Addr, Ipv6Addr},
    path::Path,
};

const SETTINGS_LIMIT: usize = 64 * 1024;

#[derive(Clone, Debug, Default)]
pub struct NetworkConfiguration {
    pub discovery_server: String,
    pub error: String,
}

fn canonical_host(host: &str) -> Option<String> {
    if host.is_empty() || !host.is_ascii() {
        return None;
    }
    if let Ok(address) = host.parse::<Ipv4Addr>() {
        return Some(address.to_string());
    }
    if let Ok(address) = host.parse::<Ipv6Addr>() {
        return Some(format!("[{address}]"));
    }
    if host.len() > 253 || host.starts_with('.') || host.ends_with('.') {
        return None;
    }
    if !host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    }) {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

pub fn normalize_server(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(String::new());
    }
    if value.len() > 512 || !value.is_ascii() || value.bytes().any(|byte| byte < 32 || byte == 127)
    {
        return Err("Discovery Server contains unsupported characters.".to_owned());
    }
    let authority = value.strip_prefix("omt://").unwrap_or(value);
    if authority
        .bytes()
        .any(|byte| matches!(byte, b'/' | b'?' | b'#' | b'@'))
    {
        return Err("Discovery Server must be a host or omt://host:port.".to_owned());
    }
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (host, suffix) = rest.split_once(']').ok_or("Discovery Server is invalid.")?;
        let port = if suffix.is_empty() {
            6399
        } else {
            suffix
                .strip_prefix(':')
                .ok_or("Discovery Server is invalid.")?
                .parse::<u16>()
                .map_err(|_| "Discovery Server is invalid.")?
        };
        (host, port)
    } else if authority.matches(':').count() == 1 {
        let (host, port) = authority
            .split_once(':')
            .ok_or("Discovery Server is invalid.")?;
        (
            host,
            port.parse::<u16>()
                .map_err(|_| "Discovery Server is invalid.")?,
        )
    } else if !authority.contains(':') {
        (authority, 6399)
    } else {
        return Err("Discovery Server IPv6 addresses must be bracketed.".to_owned());
    };
    if port == 0 {
        return Err("Discovery Server is invalid.".to_owned());
    }
    let canonical = canonical_host(host).ok_or("Discovery Server host is invalid.")?;
    Ok(format!("omt://{canonical}:{port}"))
}

fn parse_server(xml: &[u8]) -> Result<String, String> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0_usize;
    let mut root_seen = false;
    let mut discovery_count = 0_usize;
    let mut in_discovery = false;
    let mut raw = String::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| format!("OMT settings XML is invalid: {error}"))?
        {
            Event::DocType(_) | Event::GeneralRef(_) => {
                return Err("OMT settings XML must not declare a doctype or entities.".to_owned());
            }
            Event::Start(event) => {
                if depth == 0 && root_seen {
                    return Err(
                        "OMT settings XML must contain exactly one root element.".to_owned()
                    );
                }
                depth += 1;
                if depth == 1 {
                    if event.name().as_ref() != b"Settings" {
                        return Err("OMT settings root must be <Settings>.".to_owned());
                    }
                    root_seen = true;
                } else if depth == 2 && event.name().as_ref() == b"DiscoveryServer" {
                    discovery_count += 1;
                    in_discovery = true;
                }
            }
            Event::Empty(event) => {
                if depth == 0 {
                    if root_seen {
                        return Err(
                            "OMT settings XML must contain exactly one root element.".to_owned()
                        );
                    }
                    if event.name().as_ref() != b"Settings" {
                        return Err("OMT settings root must be <Settings>.".to_owned());
                    }
                    root_seen = true;
                } else if depth == 1 && event.name().as_ref() == b"DiscoveryServer" {
                    discovery_count += 1;
                }
            }
            Event::Text(event) if in_discovery && depth == 2 => {
                raw.push_str(&event.decode().map_err(|error| error.to_string())?);
            }
            Event::End(event) => {
                if depth == 2 && event.name().as_ref() == b"DiscoveryServer" {
                    in_discovery = false;
                }
                depth = depth.saturating_sub(1);
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if !root_seen || depth != 0 {
        return Err("OMT settings XML is invalid.".to_owned());
    }
    if discovery_count > 1 {
        return Err("OMT settings contain duplicate DiscoveryServer entries.".to_owned());
    }
    normalize_server(&raw)
}

pub fn read_configuration(path: &Path) -> NetworkConfiguration {
    match read_bounded(path, SETTINGS_LIMIT) {
        Ok(None) => NetworkConfiguration::default(),
        Ok(Some(xml)) => match parse_server(&xml) {
            Ok(server) => NetworkConfiguration {
                discovery_server: server,
                error: String::new(),
            },
            Err(error) => NetworkConfiguration {
                discovery_server: String::new(),
                error,
            },
        },
        Err(error) => NetworkConfiguration {
            discovery_server: String::new(),
            error,
        },
    }
}

fn update_xml(xml: &[u8], normalized: &str) -> Result<Option<Vec<u8>>, String> {
    if parse_server(xml).ok().as_deref() == Some(normalized) {
        return Ok(None);
    }
    // Parse once strictly before transforming. Re-serialize all existing nodes,
    // replacing only the direct DiscoveryServer value and preserving extensions.
    parse_server(xml)?;
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new_with_indent(Cursor::new(Vec::new()), b' ', 2);
    let mut buffer = Vec::new();
    let mut depth = 0_usize;
    let mut found = false;
    let mut skip_discovery_content = false;
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| error.to_string())?;
        match &event {
            Event::Start(start) => {
                depth += 1;
                if depth == 2 && start.name().as_ref() == b"DiscoveryServer" {
                    found = true;
                    skip_discovery_content = true;
                    writer
                        .write_event(event.into_owned())
                        .map_err(|error| error.to_string())?;
                    writer
                        .write_event(Event::Text(BytesText::new(normalized)))
                        .map_err(|error| error.to_string())?;
                } else if !skip_discovery_content {
                    writer
                        .write_event(event.into_owned())
                        .map_err(|error| error.to_string())?;
                }
            }
            Event::End(end) => {
                if depth == 2 && end.name().as_ref() == b"DiscoveryServer" {
                    skip_discovery_content = false;
                    writer
                        .write_event(event.into_owned())
                        .map_err(|error| error.to_string())?;
                } else if depth == 1 && end.name().as_ref() == b"Settings" {
                    if !found {
                        writer
                            .create_element("DiscoveryServer")
                            .write_text_content(BytesText::new(normalized))
                            .map_err(|error| error.to_string())?;
                    }
                    writer
                        .write_event(event.into_owned())
                        .map_err(|error| error.to_string())?;
                } else if !skip_discovery_content {
                    writer
                        .write_event(event.into_owned())
                        .map_err(|error| error.to_string())?;
                }
                depth = depth.saturating_sub(1);
            }
            Event::Empty(empty) if depth == 0 && empty.name().as_ref() == b"Settings" => {
                writer
                    .write_event(Event::Start(BytesStart::new("Settings")))
                    .map_err(|error| error.to_string())?;
                writer
                    .create_element("DiscoveryServer")
                    .write_text_content(BytesText::new(normalized))
                    .map_err(|error| error.to_string())?;
                writer
                    .write_event(Event::End(BytesEnd::new("Settings")))
                    .map_err(|error| error.to_string())?;
                found = true;
            }
            Event::Empty(empty) if depth == 1 && empty.name().as_ref() == b"DiscoveryServer" => {
                writer
                    .create_element("DiscoveryServer")
                    .write_text_content(BytesText::new(normalized))
                    .map_err(|error| error.to_string())?;
                found = true;
            }
            Event::Eof => break,
            _ if !skip_discovery_content => writer
                .write_event(event.into_owned())
                .map_err(|error| error.to_string())?,
            _ => {}
        }
        buffer.clear();
    }
    let mut output = writer.into_inner().into_inner();
    if !output.ends_with(b"\n") {
        output.push(b'\n');
    }
    Ok(Some(output))
}

pub fn save_configuration(path: &Path, value: &str) -> Result<bool, String> {
    let normalized = normalize_server(value)?;
    let current = read_bounded(path, SETTINGS_LIMIT)?
        .unwrap_or_else(|| b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<Settings />\n".to_vec());
    let Some(updated) = update_xml(&current, &normalized)? else {
        return Ok(false);
    };
    atomic_replace(path, &updated, SETTINGS_LIMIT)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_discovery_servers() {
        assert_eq!(
            normalize_server("Example.COM"),
            Ok("omt://example.com:6399".to_owned())
        );
        assert_eq!(
            normalize_server("[2001:db8::1]"),
            Ok("omt://[2001:db8::1]:6399".to_owned())
        );
        assert!(normalize_server("host/path").is_err());
        assert!(normalize_server("2001:db8::1").is_err());
        assert!(parse_server(b"<Settings /><Settings />").is_err());
        assert!(parse_server(b"<!DOCTYPE Settings><Settings />").is_err());
        let updated = update_xml(b"<Settings />", "omt://example.com:6399")
            .unwrap_or_else(|error| panic!("{error}"))
            .unwrap_or_default();
        assert_eq!(
            parse_server(&updated),
            Ok("omt://example.com:6399".to_owned())
        );
    }
}
