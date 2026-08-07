// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// `_omt._tcp` browsing through the host's Avahi daemon.
//
// The receiver speaks the daemon's D-Bus interface directly rather than
// linking libavahi-client, which keeps the appliance image free of the Avahi
// client libraries. Only the two Avahi interfaces the browse needs are called,
// every reply is re-validated against the shared target grammar, and the whole
// browse is bounded by the caller's deadline: a daemon that stops answering
// ends the browse on the timer rather than stalling playback.

use crate::channel::Endpoint;
use crate::discovery::{Source, endpoint_from_parts};
use async_io::Timer;
use futures_lite::StreamExt;
use futures_lite::future;
use omt_protocol::is_valid_source_name;
use std::collections::BTreeMap;
use std::time::Instant;
use zbus::zvariant::OwnedObjectPath;
use zbus::{Connection, Message, MessageStream, Proxy};

const SERVICE: &str = "org.freedesktop.Avahi";
const SERVICE_TYPE: &str = "_omt._tcp";
/// `AVAHI_IF_UNSPEC` and `AVAHI_PROTO_UNSPEC`.
const UNSPEC: i32 = -1;
/// Cap on concurrently resolving services, bounding a flooded local network.
const MAX_RESOLVERS: usize = 256;

/// True when the system bus is reachable and Avahi is registered on it.
#[must_use]
pub fn available() -> bool {
    future::block_on(async {
        let Ok(connection) = Connection::system().await else {
            return false;
        };
        let Ok(proxy) = zbus::fdo::DBusProxy::new(&connection).await else {
            return false;
        };
        let Ok(name) = SERVICE.try_into() else {
            return false;
        };
        proxy.name_has_owner(name).await.unwrap_or(false)
    })
}

/// Browses for OMT sources until the deadline expires.
#[must_use]
pub fn browse(deadline: Instant, capacity: usize) -> Vec<Source> {
    // Discovery is best-effort: a missing or unhealthy daemon reports no
    // sources, which the playback loop already treats as "keep waiting".
    future::block_on(browse_inner(deadline, capacity)).unwrap_or_default()
}

async fn browse_inner(deadline: Instant, capacity: usize) -> zbus::Result<Vec<Source>> {
    let connection = Connection::system().await?;
    let server = Proxy::new(&connection, SERVICE, "/", "org.freedesktop.Avahi.Server").await?;
    let mut stream = MessageStream::from(&connection);
    let browser: OwnedObjectPath = server
        .call(
            "ServiceBrowserNew",
            &(UNSPEC, UNSPEC, SERVICE_TYPE, "", 0_u32),
        )
        .await?;

    let mut resolvers: Vec<OwnedObjectPath> = Vec::new();
    let mut found: BTreeMap<String, Endpoint> = BTreeMap::new();
    let mut removed: Vec<String> = Vec::new();

    loop {
        let next = future::or(async { stream.next().await.map(Some) }, async {
            Timer::at(deadline).await;
            Some(None)
        })
        .await;
        let Some(Some(Ok(message))) = next else {
            // The timer fired, the bus closed, or a message failed to decode.
            break;
        };
        let header = message.header();
        match header.member().map(zbus::names::MemberName::as_str) {
            Some("ItemNew") if resolvers.len() < MAX_RESOLVERS => {
                if let Ok((interface, protocol, name, kind, domain)) = message
                    .body()
                    .deserialize::<(i32, i32, String, String, String)>()
                    && kind == SERVICE_TYPE
                    && let Ok(resolver) = server
                        .call::<_, _, OwnedObjectPath>(
                            "ServiceResolverNew",
                            &(interface, protocol, name, kind, domain, UNSPEC, 0_u32),
                        )
                        .await
                {
                    resolvers.push(resolver);
                }
            }
            Some("ItemRemove") => {
                if let Ok((_, _, name, _, _)) = message
                    .body()
                    .deserialize::<(i32, i32, String, String, String)>()
                {
                    found.remove(&name);
                    removed.push(name);
                }
            }
            Some("Found") => {
                if let Some(source) = resolved_source(&message)
                    && !removed.contains(&source.name)
                    && found.len() < capacity
                {
                    found.insert(source.name, source.endpoint);
                }
            }
            _ => {}
        }
    }

    // Avahi keeps browser and resolver objects alive until they are freed.
    for resolver in resolvers {
        let _ = free(&connection, &resolver, "ServiceResolver").await;
    }
    let _ = free(&connection, &browser, "ServiceBrowser").await;

    Ok(found
        .into_iter()
        .map(|(name, endpoint)| Source { name, endpoint })
        .collect())
}

/// `org.freedesktop.Avahi.ServiceResolver.Found`.
fn resolved_source(message: &Message) -> Option<Source> {
    let (_interface, _protocol, name, _kind, _domain, _host, _address_protocol, address, port) =
        message
            .body()
            .deserialize::<(i32, i32, String, String, String, String, i32, String, u16)>()
            .ok()?;
    if !is_valid_source_name(&name) {
        return None;
    }
    Some(Source {
        name,
        endpoint: endpoint_from_parts(&address, port)?,
    })
}

async fn free(
    connection: &Connection,
    path: &OwnedObjectPath,
    interface: &str,
) -> zbus::Result<()> {
    Proxy::new(
        connection,
        SERVICE,
        path.as_str(),
        format!("org.freedesktop.Avahi.{interface}"),
    )
    .await?
    .call("Free", &())
    .await
}
