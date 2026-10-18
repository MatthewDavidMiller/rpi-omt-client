//! The receiver's only network entry point. Every frame is bounded by the
//! protocol module before a byte of payload is allocated, and a parse
//! failure closes the connection rather than resynchronising on
//! attacker-chosen bytes.

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::os::fd::AsRawFd;

use omt_common::Error;
use omt_common::timefmt::{now_ms, remaining_ms};
use omt_protocol::{
    AudioHeader, FrameHeader, FrameType, HEADER_SIZE, VideoHeader, build_metadata,
    parse_audio_header, parse_frame_header, parse_video_header,
};
use omt_sys::posix::{self, POLLIN, POLLOUT, pollfd};

/// Maximum addresses tried for one endpoint, bounding a hostile DNS answer.
pub const MAX_ADDRESSES: usize = 16;
/// How long a frame that has started arriving may take to finish. The
/// caller's deadline is a polling slice, not a frame budget: it bounds only
/// the wait for a frame to begin. Six seconds is above the 3.5 s Wi-Fi stalls
/// measured against vMix and under the SIGTERM grace in control-omt.sh.
pub const BODY_BUDGET_MS: u64 = 6000;
/// Kernel receive buffer matching libomtnet NETWORK_RECEIVE_BUFFER.
pub const RECV_BUFFER: i32 = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
}

/// A received frame with its parsed, validated headers. The payload buffer
/// keeps its high-water size; `len` is how much of it this frame fills.
#[derive(Debug, Default)]
pub struct Frame {
    pub header: Option<FrameHeader>,
    pub video: Option<VideoHeader>,
    pub audio: Option<AudioHeader>,
    pub buffer: Vec<u8>,
    pub len: usize,
    /// `CLOCK_MONOTONIC` nanoseconds when the fixed header and the last
    /// payload byte were read, for the opt-in latency trace.
    pub header_ns: u64,
    pub complete_ns: u64,
}

impl Frame {
    pub fn payload(&self) -> &[u8] {
        &self.buffer[..self.len]
    }

    pub fn frame_type(&self) -> Option<FrameType> {
        self.header.map(|h| h.frame_type)
    }

    pub fn metadata_length(&self) -> usize {
        self.header.map_or(0, |h| usize::from(h.metadata_length))
    }

    /// The compressed media body, excluding the extended header and any
    /// trailing per-frame metadata; `None` when the two overlap.
    pub fn media(&self, extended_header: usize) -> Option<&[u8]> {
        let end = self.len.checked_sub(self.metadata_length())?;
        if extended_header > end {
            return None;
        }
        Some(&self.buffer[extended_header..end])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recv {
    Ok,
    /// The slice expired with nothing consumed; the channel stays open.
    WouldBlock,
    /// A frame in flight did not finish inside the body budget.
    TimedOut,
    Error,
}

/// The deadline a frame already in flight finishes on: the slice, raised to
/// at least the body budget from now.
pub fn body_deadline(slice_ms: u64) -> u64 {
    slice_ms.max(now_ms() + BODY_BUDGET_MS)
}

/// Resolves to at most [`MAX_ADDRESSES`] socket addresses. A scoped IPv6
/// literal keeps its zone for display but not for lookup.
pub fn resolve(ep: &Endpoint) -> Result<Vec<SocketAddr>, Error> {
    let host = ep.host.split_once('%').map_or(ep.host.as_str(), |(h, _)| h);
    let addrs = (host, ep.port).to_socket_addrs()?;
    Ok(addrs.take(MAX_ADDRESSES).collect())
}

#[derive(Debug, Default)]
pub struct Channel {
    stream: Option<TcpStream>,
    pub frame: Frame,
    partial: Partial,
}

/// A frame read so far. Reads resume from here, so a caller that only takes
/// what has already arrived never has to wait out the rest of a frame that
/// has merely started.
#[derive(Debug, Default)]
struct Partial {
    fixed: [u8; HEADER_SIZE],
    fixed_len: usize,
    header: Option<FrameHeader>,
    header_ns: u64,
    body_len: usize,
    /// When the frame must be complete: the body budget from its first byte.
    budget_ms: u64,
}

impl Partial {
    fn started(&self) -> bool {
        self.fixed_len > 0
    }
}

impl Channel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn connected(&self) -> bool {
        self.stream.is_some()
    }

    pub fn close(&mut self) {
        self.partial = Partial::default();
        if let Some(s) = self.stream.take() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }

    /// Connects and subscribes. A video subscription also asks for
    /// metadata, matching the sender handshake the reference implementation
    /// expects.
    pub fn connect(
        &mut self,
        ep: &Endpoint,
        subscription: FrameType,
        deadline_ms: u64,
    ) -> Result<(), Error> {
        self.close();
        let addrs = resolve(ep)?;
        if addrs.is_empty() {
            return Err(Error::new("unable to connect to the OMT source"));
        }
        let mut last = Error::new("unable to connect to the OMT source");
        for addr in &addrs {
            let left = remaining_ms(deadline_ms);
            if left == 0 {
                break;
            }
            match posix::tcp_connect(addr, RECV_BUFFER, left).and_then(|s| {
                s.set_nodelay(true)?;
                Ok(s)
            }) {
                Ok(stream) => {
                    self.stream = Some(stream);
                    break;
                }
                Err(e) => last = e.into(),
            }
        }
        if self.stream.is_none() {
            return Err(last);
        }
        if subscription == FrameType::Video {
            self.subscribe(FrameType::Metadata, deadline_ms)?;
        }
        self.subscribe(subscription, deadline_ms)
    }

    fn subscribe(&mut self, kind: FrameType, deadline_ms: u64) -> Result<(), Error> {
        let xml: &[u8] = match kind {
            FrameType::Video => b"<OMTSubscribe Video=\"true\" />",
            FrameType::Audio => b"<OMTSubscribe Audio=\"true\" />",
            FrameType::Metadata => b"<OMTSubscribe Metadata=\"true\" />",
        };
        let mut message = Vec::with_capacity(64);
        build_metadata(xml, 0, &mut message).map_err(|e| Error::new(e.to_string()))?;
        let result = match self.stream.as_mut() {
            Some(s) => write_all_deadline(s, &message, deadline_ms),
            None => Err(Error::new("OMT socket closed")),
        };
        if result.is_err() {
            self.close();
        }
        result
    }

    /// Reads the next frame into `self.frame`, closing the channel on
    /// anything but a would-block. The error explains every non-OK result.
    /// A frame that has begun arriving is finished on the body budget even
    /// past `deadline_ms`; one that has not begun is waited for until it.
    pub fn receive(&mut self, deadline_ms: u64) -> (Recv, Option<Error>) {
        self.finish(deadline_ms, true)
    }

    /// Waits until `deadline_ms` for a frame to complete, and no longer even
    /// once one has begun: a frame still arriving stays partly read for the
    /// next call, with the body budget still bounding how long it may take.
    /// For a caller with something else due at its deadline.
    pub fn receive_until(&mut self, deadline_ms: u64) -> (Recv, Option<Error>) {
        self.finish(deadline_ms, false)
    }

    /// Completes a frame only from bytes the socket already holds, never
    /// waiting. A frame still arriving stays partly read for the next call,
    /// so a complete frame is never held back behind one that has merely
    /// started; the body budget still bounds how long that frame may take.
    pub fn receive_ready(&mut self) -> (Recv, Option<Error>) {
        self.finish(0, false)
    }

    fn finish(&mut self, deadline_ms: u64, wait: bool) -> (Recv, Option<Error>) {
        let (status, err) = self.receive_inner(deadline_ms, wait);
        // A deadline leaves the channel usable, with any partial frame kept;
        // anything else means the stream is no longer trustworthy.
        if status != Recv::Ok && status != Recv::WouldBlock {
            self.close();
        }
        (status, err)
    }

    fn receive_inner(&mut self, deadline_ms: u64, wait: bool) -> (Recv, Option<Error>) {
        let Some(stream) = self.stream.as_mut() else {
            return (Recv::Error, Some(Error::new("OMT socket closed")));
        };
        let p = &mut self.partial;
        let f = &mut self.frame;
        loop {
            let target: &mut [u8] = match p.header {
                None => &mut p.fixed[p.fixed_len..],
                Some(h) => &mut f.buffer[p.body_len..h.data_length as usize],
            };
            // Never empty: a header-only frame completes as its header does.
            match stream.read(target) {
                Ok(0) => {
                    return (Recv::Error, Some(Error::new("OMT source disconnected")));
                }
                Ok(n) => {
                    if !p.started() {
                        // This read is now committed to a frame, so the
                        // polling slice stops bounding it.
                        p.budget_ms = now_ms() + BODY_BUDGET_MS;
                    }
                    if p.header.is_none() {
                        p.fixed_len += n;
                        if p.fixed_len < HEADER_SIZE {
                            continue;
                        }
                        p.header_ns = posix::monotonic_ns();
                        let header = match parse_frame_header(&p.fixed) {
                            Ok(h) => h,
                            Err(e) => return (Recv::Error, Some(Error::new(e.to_string()))),
                        };
                        let required = header.data_length as usize;
                        // Grows the payload buffer in place. Only bytes it
                        // has never held are zeroed, and the read overwrites
                        // the rest.
                        if required > f.buffer.len() {
                            if f.buffer.try_reserve_exact(required - f.buffer.len()).is_err() {
                                return (
                                    Recv::Error,
                                    Some(Error::new("unable to allocate the bounded OMT frame")),
                                );
                            }
                            f.buffer.resize(required, 0);
                        }
                        p.header = Some(header);
                    } else {
                        p.body_len += n;
                    }
                    if let Some(h) = p.header
                        && p.body_len == h.data_length as usize
                    {
                        let header_ns = p.header_ns;
                        *p = Partial::default();
                        return complete(f, h, header_ns);
                    }
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    let started = p.started();
                    if started && now_ms() >= p.budget_ms {
                        return (
                            Recv::TimedOut,
                            Some(Error::new("OMT frame was truncated by a timeout")),
                        );
                    }
                    let until =
                        if started && wait { deadline_ms.max(p.budget_ms) } else { deadline_ms };
                    let left = remaining_ms(until);
                    if left == 0 {
                        return (Recv::WouldBlock, Some(Error::new("OMT socket deadline expired")));
                    }
                    match poll_one(stream.as_raw_fd(), POLLIN, left) {
                        // A signal: hand control back with the frame kept,
                        // so a stop request is seen without waiting it out.
                        Ok(0) if remaining_ms(until) > 0 => {
                            return (Recv::WouldBlock, Some(Error::new("OMT read interrupted")));
                        }
                        Ok(_) => {}
                        Err(e) => return (Recv::Error, Some(e.into())),
                    }
                }
                Err(e) => return (Recv::Error, Some(e.into())),
            }
        }
    }

    /// Swaps the received frame out so a queue can own its payload, leaving
    /// `replacement` (a recycled buffer, possibly empty) to read the next
    /// one into.
    pub fn take_frame(&mut self, replacement: Vec<u8>) -> Frame {
        std::mem::replace(&mut self.frame, Frame { buffer: replacement, ..Frame::default() })
    }
}

fn poll_one(fd: i32, events: i16, timeout_ms: u64) -> std::io::Result<usize> {
    let mut fds = [pollfd { fd, events, revents: 0 }];
    posix::poll_fds(&mut fds, i32::try_from(timeout_ms).unwrap_or(i32::MAX))
}

fn write_all_deadline(
    stream: &mut TcpStream,
    mut data: &[u8],
    deadline_ms: u64,
) -> Result<(), Error> {
    while !data.is_empty() {
        match stream.write(data) {
            Ok(0) => return Err(Error::os(omt_sys::posix::EPIPE)),
            Ok(n) => data = &data[n..],
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                // A write timeout of zero would mean "block forever" to a
                // socket; the shortest real bound is one millisecond.
                let left = remaining_ms(deadline_ms).max(1);
                if poll_one(stream.as_raw_fd(), POLLOUT, left)? == 0 {
                    return Err(Error::os(omt_sys::posix::EAGAIN));
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Validates a fully read frame's extended header and publishes it.
fn complete(f: &mut Frame, header: FrameHeader, header_ns: u64) -> (Recv, Option<Error>) {
    let required = header.data_length as usize;
    f.len = required;
    f.header_ns = header_ns;
    f.complete_ns = posix::monotonic_ns();
    f.video = None;
    f.audio = None;
    // The header is cleared until this frame validates, so a rejected frame
    // can never be taken for the one before it.
    f.header = None;
    match header.frame_type {
        FrameType::Video => match parse_video_header(&header, &f.buffer[..required]) {
            Ok(v) => f.video = Some(v),
            Err(e) => return (Recv::Error, Some(Error::new(e.to_string()))),
        },
        FrameType::Audio => match parse_audio_header(&header, &f.buffer[..required]) {
            Ok(a) => f.audio = Some(a),
            Err(e) => return (Recv::Error, Some(Error::new(e.to_string()))),
        },
        FrameType::Metadata => {}
    }
    f.header = Some(header);
    (Recv::Ok, None)
}
