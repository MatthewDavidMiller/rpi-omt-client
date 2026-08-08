// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// The receiver's only network entry point. Every frame is bounded by
// omt-protocol before a byte of payload is allocated, and a parse failure
// closes the connection rather than resynchronising on attacker-chosen bytes.

use omt_protocol::{
    AudioHeader, FrameHeader, FrameType, VideoHeader, build_metadata, parse_audio_header,
    parse_frame_header, parse_video_header,
};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// Maximum addresses tried for one endpoint, bounding a hostile DNS answer.
const MAX_ADDRESSES: usize = 16;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
}

impl Endpoint {
    /// Resolves the endpoint to a bounded address list.
    pub fn resolve(&self) -> io::Result<Vec<SocketAddr>> {
        // A scoped IPv6 literal keeps its zone for display but not for lookup.
        let host = self
            .host
            .split_once('%')
            .map_or(self.host.as_str(), |(h, _)| h);
        Ok((host, self.port)
            .to_socket_addrs()?
            .take(MAX_ADDRESSES)
            .collect())
    }
}

/// A received OMT frame with its parsed, validated headers.
pub struct Frame {
    pub header: FrameHeader,
    pub video: Option<VideoHeader>,
    pub audio: Option<AudioHeader>,
    pub payload: Vec<u8>,
}

impl Frame {
    fn new() -> Self {
        Self {
            header: FrameHeader {
                version: 1,
                frame_type: FrameType::Metadata,
                timestamp: 0,
                metadata_length: 0,
                data_length: 0,
            },
            video: None,
            audio: None,
            payload: Vec::new(),
        }
    }

    /// The compressed media body, excluding the extended header and any
    /// trailing per-frame metadata.
    #[must_use]
    pub fn media(&self, extended_header: usize) -> Option<&[u8]> {
        let end = self
            .payload
            .len()
            .checked_sub(usize::from(self.header.metadata_length))?;
        self.payload.get(extended_header..end)
    }
}

pub struct Channel {
    stream: Option<TcpStream>,
    frame: Frame,
}

impl Channel {
    #[must_use]
    pub fn new() -> Self {
        Self {
            stream: None,
            frame: Frame::new(),
        }
    }

    #[must_use]
    pub fn connected(&self) -> bool {
        self.stream.is_some()
    }

    /// The most recently received frame.
    #[must_use]
    pub fn frame(&self) -> &Frame {
        &self.frame
    }

    pub fn close(&mut self) {
        if let Some(stream) = self.stream.take() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }

    /// Connects and subscribes. A video subscription also asks for metadata,
    /// matching the sender handshake the reference implementation expects.
    pub fn connect(
        &mut self,
        endpoint: &Endpoint,
        subscription: FrameType,
        deadline: Instant,
    ) -> io::Result<()> {
        self.close();
        let addresses = endpoint.resolve()?;
        let mut last = io::Error::other("unable to connect to the OMT source");
        for address in addresses {
            let remaining = remaining(deadline);
            if remaining.is_zero() {
                break;
            }
            match TcpStream::connect_timeout(&address, remaining) {
                Ok(stream) => {
                    stream.set_nodelay(true)?;
                    self.stream = Some(stream);
                    break;
                }
                Err(error) => last = error,
            }
        }
        if self.stream.is_none() {
            return Err(last);
        }
        if subscription == FrameType::Video {
            self.subscribe(FrameType::Metadata, deadline)?;
        }
        self.subscribe(subscription, deadline)?;
        Ok(())
    }

    fn subscribe(&mut self, subscription: FrameType, deadline: Instant) -> io::Result<()> {
        let xml = match subscription {
            FrameType::Video => "<OMTSubscribe Video=\"true\" />",
            FrameType::Audio => "<OMTSubscribe Audio=\"true\" />",
            FrameType::Metadata => "<OMTSubscribe Metadata=\"true\" />",
        };
        let bytes = build_metadata(xml, 0).map_err(io::Error::other)?;
        let result = (|| {
            let stream = self
                .stream
                .as_mut()
                .ok_or_else(|| io::Error::other("OMT socket closed"))?;
            stream.set_write_timeout(Some(positive(remaining(deadline))))?;
            stream.write_all(&bytes)
        })();
        if result.is_err() {
            self.close();
        }
        result
    }

    /// Reads the next frame, closing the channel on any protocol violation.
    pub fn receive(&mut self, deadline: Instant) -> io::Result<&Frame> {
        let outcome = self.receive_inner(deadline);
        match outcome {
            Ok(()) => Ok(&self.frame),
            Err(error) => {
                // A deadline with nothing consumed leaves the channel usable;
                // anything else means the stream is no longer trustworthy.
                if error.kind() != io::ErrorKind::WouldBlock {
                    self.close();
                }
                Err(error)
            }
        }
    }

    fn receive_inner(&mut self, deadline: Instant) -> io::Result<()> {
        let mut fixed = [0_u8; omt_protocol::HEADER_SIZE];
        self.read_exact(&mut fixed, deadline, true)?;
        let header = parse_frame_header(&fixed).map_err(io::Error::other)?;
        let required = usize::try_from(header.data_length).map_err(io::Error::other)?;

        // Grow or shrink in place. `clear` before `resize` would re-zero the
        // entire previous capacity every frame; only newly grown bytes need to
        // be zeroed, and `read_exact` overwrites the kept prefix.
        ensure_payload(&mut self.frame.payload, required)?;
        let mut payload = std::mem::take(&mut self.frame.payload);
        let outcome = self.read_exact(&mut payload, deadline, false);
        self.frame.payload = payload;
        outcome?;

        self.frame.video = None;
        self.frame.audio = None;
        match header.frame_type {
            FrameType::Video => {
                self.frame.video = Some(
                    parse_video_header(&header, &self.frame.payload).map_err(io::Error::other)?,
                );
            }
            FrameType::Audio => {
                self.frame.audio = Some(
                    parse_audio_header(&header, &self.frame.payload).map_err(io::Error::other)?,
                );
            }
            FrameType::Metadata => {}
        }
        self.frame.header = header;
        Ok(())
    }

    /// `resumable` marks a read that has not yet consumed any of the frame, so
    /// a deadline can expire without invalidating the connection.
    fn read_exact(
        &mut self,
        target: &mut [u8],
        deadline: Instant,
        resumable: bool,
    ) -> io::Result<()> {
        let mut filled = 0;
        let expired = |filled: usize| {
            if resumable && filled == 0 {
                io::Error::new(io::ErrorKind::WouldBlock, "OMT socket deadline expired")
            } else {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "OMT frame was truncated by a timeout",
                )
            }
        };
        while filled < target.len() {
            let left = remaining(deadline);
            if left.is_zero() {
                return Err(expired(filled));
            }
            let stream = self
                .stream
                .as_mut()
                .ok_or_else(|| io::Error::other("OMT socket closed"))?;
            stream.set_read_timeout(Some(left))?;
            match stream.read(&mut target[filled..]) {
                Ok(0) => return Err(io::Error::other("OMT source disconnected")),
                Ok(count) => filled += count,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                    ) =>
                {
                    // A partial frame cannot be resumed on a later call, so a
                    // stalled read is fatal once any byte has been consumed.
                    if filled != 0 || remaining(deadline).is_zero() {
                        return Err(expired(filled));
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

impl Default for Channel {
    fn default() -> Self {
        Self::new()
    }
}

#[must_use]
pub fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

fn positive(value: Duration) -> Duration {
    if value.is_zero() {
        Duration::from_millis(1)
    } else {
        value
    }
}

fn ensure_payload(payload: &mut Vec<u8>, required: usize) -> io::Result<()> {
    payload
        .try_reserve(required)
        .map_err(|_| io::Error::other("unable to allocate the bounded OMT frame"))?;
    payload.resize(required, 0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn received(payload: usize, metadata_length: u16) -> Frame {
        let mut frame = Frame::new();
        frame.payload = vec![0_u8; payload];
        frame.header.metadata_length = metadata_length;
        frame
    }

    /// `media` is what every decoder is handed, so its arithmetic decides
    /// whether a crafted frame can make a codec read the trailing metadata --
    /// or a length that underflows -- as media.
    #[test]
    fn media_excludes_the_extended_header_and_trailing_metadata() {
        let frame = received(100, 10);
        assert_eq!(frame.media(32).map(<[u8]>::len), Some(58));
        assert_eq!(frame.media(0).map(<[u8]>::len), Some(90));
        // Exactly consumed is an empty body, not a failure.
        assert_eq!(received(32, 0).media(32).map(<[u8]>::len), Some(0));
        // Metadata longer than the payload, and an extended header longer than
        // what the metadata leaves, are both refusals rather than wraps.
        assert_eq!(received(8, 10).media(0), None);
        assert_eq!(received(40, 10).media(32), None);
        assert_eq!(received(100, 10).media(usize::MAX), None);
    }

    /// A zone belongs to the display form of a scoped IPv6 target; passing it
    /// to the resolver would fail the lookup the target exists to make.
    #[test]
    fn resolve_drops_an_ipv6_zone_and_keeps_the_port() {
        let scoped = Endpoint {
            host: "fe80::1%eth0".into(),
            port: 6400,
        };
        let addresses = scoped.resolve().unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(addresses.len(), 1);
        assert_eq!(addresses[0].port(), 6400);
        assert!(addresses[0].is_ipv6());

        let literal = Endpoint {
            host: "127.0.0.1".into(),
            port: 1,
        };
        assert_eq!(
            literal
                .resolve()
                .unwrap_or_else(|error| panic!("{error}"))
                .len(),
            1
        );
        assert!(
            Endpoint {
                host: "not a host".into(),
                port: 1,
            }
            .resolve()
            .is_err()
        );
    }

    #[test]
    fn a_deadline_that_has_passed_is_zero_rather_than_a_wrap() {
        let past = Instant::now()
            .checked_sub(Duration::from_secs(5))
            .unwrap_or_else(|| panic!("the monotonic clock has no past"));
        assert!(remaining(past).is_zero());
        assert_eq!(positive(remaining(past)), Duration::from_millis(1));
        assert!(!remaining(Instant::now() + Duration::from_secs(5)).is_zero());
    }

    #[test]
    fn payload_resize_reuses_capacity_without_clearing() {
        let mut payload = Vec::new();
        ensure_payload(&mut payload, 64).unwrap_or_else(|error| panic!("{error}"));
        payload.fill(0xAB);
        let capacity = payload.capacity();
        ensure_payload(&mut payload, 32).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(payload.len(), 32);
        assert!(payload.capacity() >= capacity);
        assert!(payload.iter().all(|&byte| byte == 0xAB));
        ensure_payload(&mut payload, 96).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(payload.len(), 96);
        assert!(payload[..32].iter().all(|&byte| byte == 0xAB));
        assert!(payload[32..].iter().all(|&byte| byte == 0));
    }
}
