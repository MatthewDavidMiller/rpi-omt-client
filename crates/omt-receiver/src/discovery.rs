//! Source discovery. A configured central discovery server takes precedence
//! over mDNS, as the appliance's settings contract specifies; with no server
//! configured the receiver browses _omt._tcp through the host's Avahi daemon.

use std::net::{Ipv4Addr, Ipv6Addr};

use omt_common::timefmt::now_ms;
use omt_common::xml;
use omt_protocol::{
    FrameType, is_undiallable_ipv4, is_undiallable_ipv6, is_valid_source_name, parse_direct_target,
};

use crate::channel::{Channel, Endpoint, Recv};

/// The receiver never tracks more sources than the dashboard can present.
pub const MAX_SOURCES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub name: String,
    pub endpoint: Endpoint,
}

/// A bounded, name-keyed set of sources; insertion replaces an existing name.
#[derive(Clone, Debug, Default)]
pub struct SourceSet {
    pub items: Vec<Source>,
}

impl SourceSet {
    /// Inserts or replaces; false when full and the name is new.
    pub fn put(&mut self, name: &str, ep: &Endpoint) -> bool {
        if let Some(s) = self.items.iter_mut().find(|s| s.name == name) {
            s.endpoint = ep.clone();
            return true;
        }
        if self.items.len() >= MAX_SOURCES {
            return false;
        }
        self.items.push(Source { name: name.to_owned(), endpoint: ep.clone() });
        true
    }

    pub fn remove(&mut self, name: &str) {
        if let Some(i) = self.items.iter().position(|s| s.name == name) {
            self.items.swap_remove(i);
        }
    }

    pub fn sort(&mut self) {
        self.items.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// The configured central discovery server's target, if storage names one.
pub fn configured_server() -> Option<String> {
    let storage = std::env::var("OMT_STORAGE_PATH").ok().filter(|s| !s.is_empty());
    let path = format!("{}/settings.xml", storage.as_deref().unwrap_or("/etc/omt/omt"));
    let doc = omt_common::fsio::read_text(path, xml::MAX_DOCUMENT).ok()??;
    xml::root_is(doc.trim().as_bytes(), "Settings").ok()?;
    let server =
        String::from_utf8(xml::unique_text(doc.as_bytes(), "DiscoveryServer").ok()?).ok()?;
    parse_direct_target(&server).ok()?;
    Some(server)
}

pub fn transport_available() -> bool {
    configured_server().is_some() || crate::mdns::available()
}

/// Re-validates a discovered address through the shared target grammar, and
/// refuses literals no connect can use (unscoped link-local IPv6 and so on).
pub fn endpoint_from_parts(address: &str, port: u16) -> Option<Endpoint> {
    if port == 0 {
        return None;
    }
    let v4 = address.parse::<Ipv4Addr>().ok();
    let v6 = if v4.is_none() { address.parse::<Ipv6Addr>().ok() } else { None };
    if v4.is_some_and(is_undiallable_ipv4) || v6.is_some_and(is_undiallable_ipv6) {
        return None;
    }
    let candidate = if v6.is_some() {
        format!("omt://[{address}]:{port}")
    } else {
        format!("omt://{address}:{port}")
    };
    let t = parse_direct_target(&candidate).ok()?;
    Some(Endpoint { host: t.host, port: t.port })
}

/// One OMTAddress announcement read in a single pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Announcement {
    pub name: String,
    pub endpoint: Option<Endpoint>,
    pub removed: bool,
}

pub fn read_announcement(doc: &[u8]) -> Option<Announcement> {
    let texts = xml::unique_texts(doc, &["Name", "Removed", "IPAddress", "Port"]).ok()?;
    let name = std::str::from_utf8(texts[0].as_deref()?).ok()?;
    if !is_valid_source_name(name) {
        return None;
    }
    let removed = xml::element_is(texts[1].as_deref(), "True");
    let endpoint = match (&texts[2], &texts[3]) {
        (Some(address), Some(port)) => {
            // u16::from_str: digits with an optional leading '+'.
            let port = port.strip_prefix(b"+").unwrap_or(port);
            omt_common::buf::parse_u64(port, 65535)
                .and_then(|p| u16::try_from(p).ok())
                .and_then(|p| endpoint_from_parts(std::str::from_utf8(address).ok()?, p))
        }
        _ => None,
    };
    Some(Announcement { name: name.to_owned(), endpoint, removed })
}

/// Subscribes to a central discovery server's metadata stream and collects
/// every announcement that arrives before the deadline, last writer wins.
fn server_sources(server: &str, deadline_ms: u64, out: &mut SourceSet) {
    let Ok(t) = parse_direct_target(server) else { return };
    let ep = Endpoint { host: t.host, port: t.port };
    let mut c = Channel::new();
    if c.connect(&ep, FrameType::Metadata, deadline_ms).is_err() {
        return;
    }
    while now_ms() < deadline_ms && out.len() < MAX_SOURCES {
        if c.receive(deadline_ms).0 != Recv::Ok {
            break;
        }
        if c.frame.frame_type() != Some(FrameType::Metadata) || c.frame.len == 0 {
            continue;
        }
        let payload = c.frame.payload();
        if std::str::from_utf8(payload).is_err() {
            continue;
        }
        let Some(a) = read_announcement(payload) else { continue };
        if a.removed {
            out.remove(&a.name);
        } else if let Some(ep) = &a.endpoint {
            let _ = out.put(&a.name, ep);
        }
    }
}

/// Browses every configured transport for `wait_ms`; sorted by name.
pub fn discover_sources(wait_ms: u64) -> SourceSet {
    let mut out = SourceSet::default();
    let deadline = now_ms() + wait_ms;
    if let Some(server) = configured_server() {
        server_sources(&server, deadline, &mut out);
    } else {
        crate::mdns::browse(deadline, MAX_SOURCES, &mut out);
    }
    out.sort();
    out
}

/// Turns a target into an endpoint, discovering it by name if needed.
pub fn resolve(target: &str, wait_ms: u64) -> Option<Endpoint> {
    if let Ok(t) = parse_direct_target(target) {
        return Some(Endpoint { host: t.host, port: t.port });
    }
    if target.starts_with("omt://") || !is_valid_source_name(target) {
        return None;
    }
    discover_sources(wait_ms).items.into_iter().find(|s| s.name == target).map(|s| s.endpoint)
}
