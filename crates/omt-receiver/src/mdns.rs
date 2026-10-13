//! _omt._tcp browsing through the host's Avahi daemon over D-Bus, without
//! the Avahi client libraries. Only the two Avahi interfaces the browse
//! needs are called, every reply is re-validated against the shared target
//! grammar, and the whole browse -- method calls included -- is bounded by
//! the caller's deadline: a daemon that stops answering ends the browse on
//! the timer.

use omt_common::timefmt::now_ms;
use omt_protocol::{SOURCE_NAME_MAX_BYTES, is_valid_source_name};

use crate::dbus::{Arg, Bus, Message, SIGNAL};
use crate::discovery::{SourceSet, endpoint_from_parts};

const SERVICE: &str = "org.freedesktop.Avahi";
const SERVER_INTERFACE: &str = "org.freedesktop.Avahi.Server";
const BROWSER_INTERFACE: &str = "org.freedesktop.Avahi.ServiceBrowser";
const RESOLVER_INTERFACE: &str = "org.freedesktop.Avahi.ServiceResolver";
const SERVICE_TYPE: &str = "_omt._tcp";
/// AVAHI_IF_UNSPEC and AVAHI_PROTO_UNSPEC.
const UNSPEC: i32 = -1;
/// Cap on concurrently resolving services, bounding a flooded network.
const MAX_RESOLVERS: usize = 256;
/// Cap on remembered removals so a flapping network cannot grow the set.
const MAX_REMOVED: usize = 256;
/// How long `available` waits for the bus.
const AVAILABLE_TIMEOUT_MS: u64 = 2000;

pub(crate) fn available() -> bool {
    let deadline = now_ms() + AVAILABLE_TIMEOUT_MS;
    let Ok(mut bus) = Bus::open_system(deadline) else { return false };
    bus.call(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "NameHasOwner",
        &[Arg::Str(SERVICE)],
        deadline,
    )
    .is_ok_and(|reply| {
        let mut r = reply.reader();
        reply.signature() == "b" && r.bool() && !r.failed
    })
}

fn is_signal(m: &Message, interface: &str, member: &str) -> bool {
    m.kind == SIGNAL && m.member() == Some(member) && m.interface() == Some(interface)
}

/// ItemNew and ItemRemove carry `iisssu`: interface, protocol, name, type,
/// domain, flags.
struct BrowserItem<'a> {
    interface: i32,
    protocol: i32,
    name: &'a str,
    kind: &'a str,
    domain: &'a str,
}

fn read_browser_item(m: &Message) -> Option<BrowserItem<'_>> {
    if m.signature() != "iisssu" {
        return None;
    }
    let mut r = m.reader();
    let item = BrowserItem {
        interface: r.i32(),
        protocol: r.i32(),
        name: r.string(),
        kind: r.string(),
        domain: r.string(),
    };
    let _ = r.u32();
    (!r.failed).then_some(item)
}

/// Found carries `iissssisqaayu`: the browser item's five leading fields,
/// then host, address protocol, address, port, the TXT records, and flags.
fn resolved_source(m: &Message) -> Option<(String, crate::channel::Endpoint)> {
    if m.signature() != "iissssisqaayu" {
        return None;
    }
    let mut r = m.reader();
    let _ = (r.i32(), r.i32());
    let name = r.string();
    let _ = (r.string(), r.string(), r.string(), r.i32());
    let address = r.string();
    let port = r.u16();
    r.skip_aay();
    let _ = r.u32();
    if r.failed || r.pos != r.end || !is_valid_source_name(name) {
        return None;
    }
    Some((name.to_owned(), endpoint_from_parts(address, port)?))
}

fn reply_path(reply: &Message) -> Option<String> {
    if reply.signature() != "o" {
        return None;
    }
    let mut r = reply.reader();
    let path = r.string();
    (!r.failed).then(|| path.to_owned())
}

pub(crate) fn browse(deadline_ms: u64, capacity: usize, out: &mut SourceSet) {
    out.items.clear();
    let Ok(mut bus) = Bus::open_system(deadline_ms) else { return };
    let browse_args =
        [Arg::I32(UNSPEC), Arg::I32(UNSPEC), Arg::Str(SERVICE_TYPE), Arg::Str(""), Arg::U32(0)];
    let Some(browser) = bus
        .call(
            Some(SERVICE),
            "/",
            Some(SERVER_INTERFACE),
            "ServiceBrowserNew",
            &browse_args,
            deadline_ms,
        )
        .ok()
        .and_then(|reply| reply_path(&reply))
    else {
        return;
    };
    let mut resolvers: Vec<String> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    // The timer fired, the bus closed, or a message failed to decode.
    while let Ok(m) = bus.next(deadline_ms) {
        if is_signal(&m, BROWSER_INTERFACE, "ItemNew") {
            if resolvers.len() < MAX_RESOLVERS
                && let Some(item) = read_browser_item(&m)
                && item.kind == SERVICE_TYPE
            {
                let args = [
                    Arg::I32(item.interface),
                    Arg::I32(item.protocol),
                    Arg::Str(item.name),
                    Arg::Str(item.kind),
                    Arg::Str(item.domain),
                    Arg::I32(UNSPEC),
                    Arg::U32(0),
                ];
                if let Some(path) = bus
                    .call(
                        Some(SERVICE),
                        "/",
                        Some(SERVER_INTERFACE),
                        "ServiceResolverNew",
                        &args,
                        deadline_ms,
                    )
                    .ok()
                    .and_then(|reply| reply_path(&reply))
                {
                    resolvers.push(path);
                }
            }
        } else if is_signal(&m, BROWSER_INTERFACE, "ItemRemove") {
            if let Some(item) = read_browser_item(&m) {
                out.remove(item.name);
                if removed.len() < MAX_REMOVED
                    && item.name.len() <= SOURCE_NAME_MAX_BYTES
                    && !removed.iter().any(|n| n == item.name)
                {
                    removed.push(item.name.to_owned());
                }
            }
        } else if is_signal(&m, RESOLVER_INTERFACE, "Found")
            && let Some((name, endpoint)) = resolved_source(&m)
            && !removed.contains(&name)
            && out.len() < capacity
        {
            let _ = out.put(&name, &endpoint);
        }
    }
    // Avahi keeps browser and resolver objects alive until freed. The frees
    // expect no reply, so teardown never waits on the daemon.
    let teardown = now_ms() + 500;
    for path in &resolvers {
        let _ = bus.send_no_reply(SERVICE, path, RESOLVER_INTERFACE, "Free", teardown);
    }
    let _ = bus.send_no_reply(SERVICE, &browser, BROWSER_INTERFACE, "Free", teardown);
}
