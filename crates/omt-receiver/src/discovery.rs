// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// Source discovery. A configured central discovery server takes precedence
// over mDNS, exactly as the appliance's settings contract specifies; when no
// server is configured the receiver browses `_omt._tcp` through the host's
// Avahi daemon.

use crate::channel::{Channel, Endpoint};
use crate::mdns;
use crate::xml::{self, MAX_DOCUMENT_BYTES};
use omt_protocol::{FrameType, is_valid_source_name, parse_direct_target};
use std::collections::BTreeMap;
use std::fs;
use std::net::IpAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The receiver never tracks more sources than the dashboard can present.
pub const MAX_SOURCES: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Source {
    pub name: String,
    pub endpoint: Endpoint,
}

/// Reads the configured central discovery server, if the storage settings
/// name one.
#[must_use]
pub fn configured_server() -> Option<String> {
    let storage = std::env::var_os("OMT_STORAGE_PATH")
        .map_or_else(|| PathBuf::from("/etc/omt/omt"), PathBuf::from);
    let path = storage.join("settings.xml");
    let metadata = fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_DOCUMENT_BYTES as u64 {
        return None;
    }
    let document = fs::read_to_string(&path).ok()?;
    xml::root_is(document.trim(), "Settings").ok()?;
    let target = xml::unique_text(&document, "DiscoveryServer").ok()?;
    parse_direct_target(&target).ok()?;
    Some(target)
}

/// True when some discovery transport could answer.
#[must_use]
pub fn transport_available() -> bool {
    configured_server().is_some() || mdns::available()
}

/// Browses every configured transport for the given budget.
#[must_use]
pub fn sources(wait: Duration) -> Vec<Source> {
    let deadline = Instant::now() + wait;
    let mut found = match configured_server() {
        Some(server) => server_sources(&server, deadline),
        None => mdns::browse(deadline, MAX_SOURCES),
    };
    found.sort_by(|left, right| left.name.cmp(&right.name));
    found.truncate(MAX_SOURCES);
    found
}

/// Turns a target into an endpoint, discovering it by name if needed.
#[must_use]
pub fn resolve(target: &str, wait: Duration) -> Option<Endpoint> {
    if let Ok(direct) = parse_direct_target(target) {
        return Some(Endpoint {
            host: direct.host,
            port: direct.port,
        });
    }
    if target.starts_with("omt://") || !is_valid_source_name(target) {
        return None;
    }
    sources(wait)
        .into_iter()
        .find(|source| source.name == target)
        .map(|source| source.endpoint)
}

/// Subscribes to a central discovery server's metadata stream and collects
/// every announcement that arrives before the deadline.
fn server_sources(server: &str, deadline: Instant) -> Vec<Source> {
    let Ok(direct) = parse_direct_target(server) else {
        return Vec::new();
    };
    let endpoint = Endpoint {
        host: direct.host,
        port: direct.port,
    };
    let mut channel = Channel::new();
    if channel
        .connect(&endpoint, FrameType::Metadata, deadline)
        .is_err()
    {
        return Vec::new();
    }
    // A BTreeMap gives the announcement stream last-writer-wins semantics per
    // name and leaves the result sorted.
    let mut collected: BTreeMap<String, Endpoint> = BTreeMap::new();
    while Instant::now() < deadline && collected.len() <= MAX_SOURCES {
        let Ok(frame) = channel.receive(deadline) else {
            break;
        };
        if frame.header.frame_type != FrameType::Metadata || frame.payload.is_empty() {
            continue;
        }
        let Ok(document) = std::str::from_utf8(&frame.payload) else {
            continue;
        };
        let Ok(name) = xml::unique_text(document, "Name") else {
            continue;
        };
        if !is_valid_source_name(&name) {
            continue;
        }
        if xml::contains_element(document, "Removed", "True") {
            collected.remove(&name);
            continue;
        }
        if let Some(endpoint) = announcement_endpoint(document) {
            collected.insert(name, endpoint);
        }
    }
    collected
        .into_iter()
        .map(|(name, endpoint)| Source { name, endpoint })
        .collect()
}

/// Reads the address and port from one `OMTAddress` announcement.
fn announcement_endpoint(document: &str) -> Option<Endpoint> {
    let address = xml::unique_text(document, "IPAddress").ok()?;
    let port: u16 = xml::unique_text(document, "Port").ok()?.parse().ok()?;
    endpoint_from_parts(&address, port)
}

/// Re-validates a discovered address through the shared direct-target grammar
/// so discovery cannot introduce a target the CLI would have rejected.
pub fn endpoint_from_parts(address: &str, port: u16) -> Option<Endpoint> {
    if port == 0 {
        return None;
    }
    let candidate = if address.parse::<IpAddr>().is_ok_and(|value| value.is_ipv6()) {
        format!("omt://[{address}]:{port}")
    } else {
        format!("omt://{address}:{port}")
    };
    let parsed = parse_direct_target(&candidate).ok()?;
    Some(Endpoint {
        host: parsed.host,
        port: parsed.port,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebuilds_endpoints_through_the_shared_grammar() {
        assert_eq!(
            endpoint_from_parts("192.0.2.10", 6400),
            Some(Endpoint {
                host: "192.0.2.10".into(),
                port: 6400
            })
        );
        assert!(endpoint_from_parts("fe80::1", 6400).is_some());
        assert_eq!(endpoint_from_parts("192.0.2.10", 0), None);
        assert_eq!(endpoint_from_parts("not a host", 6400), None);
        assert_eq!(endpoint_from_parts("192.0.2.10/../x", 6400), None);
    }

    #[test]
    fn reads_an_announcement() {
        let document = "<OMTAddress><Name>Camera</Name><Port>6400</Port>\
            <Addresses><IPAddress>192.0.2.10</IPAddress></Addresses></OMTAddress>";
        assert_eq!(
            announcement_endpoint(document),
            Some(Endpoint {
                host: "192.0.2.10".into(),
                port: 6400
            })
        );
    }
}
