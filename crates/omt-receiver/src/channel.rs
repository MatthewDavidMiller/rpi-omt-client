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
}

impl Channel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn connected(&self) -> bool {
        self.stream.is_some()
    }

    pub fn close(&mut self) {
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
    pub fn receive(&mut self, deadline_ms: u64) -> (Recv, Option<Error>) {
        let (status, err) = self.receive_inner(deadline_ms);
        // A deadline with nothing consumed leaves the channel usable;
        // anything else means the stream is no longer trustworthy.
        if status != Recv::Ok && status != Recv::WouldBlock {
            self.close();
        }
        (status, err)
    }

    fn receive_inner(&mut self, deadline_ms: u64) -> (Recv, Option<Error>) {
        let Some(stream) = self.stream.as_mut() else {
            return (Recv::Error, Some(Error::new("OMT socket closed")));
        };
        let mut fixed = [0_u8; HEADER_SIZE];
        match read_exact(stream, &mut fixed, deadline_ms, true) {
            (Recv::Ok, _) => {}
            other => return other,
        }
        let header = match parse_frame_header(&fixed) {
            Ok(h) => h,
            Err(e) => return (Recv::Error, Some(Error::new(e.to_string()))),
        };
        let required = header.data_length as usize;
        let f = &mut self.frame;
        // Grows the payload buffer in place. Only bytes it has never held
        // are zeroed, and the read overwrites the rest.
        if required > f.buffer.len() {
            if f.buffer.try_reserve_exact(required - f.buffer.len()).is_err() {
                return (Recv::Error, Some(Error::new("unable to allocate the bounded OMT frame")));
            }
            f.buffer.resize(required, 0);
        }
        f.len = required;
        // The fixed header committed this connection to a frame.
        match read_exact(stream, &mut f.buffer[..required], deadline_ms, false) {
            (Recv::Ok, _) => {}
            (Recv::WouldBlock, e) => return (Recv::TimedOut, e),
            other => return other,
        }
        f.video = None;
        f.audio = None;
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

/// `resumable` marks a read that has consumed none of the frame, so the
/// slice can expire without invalidating the connection. From the first
/// byte on, the body budget applies instead.
fn read_exact(
    stream: &mut TcpStream,
    target: &mut [u8],
    slice_ms: u64,
    resumable: bool,
) -> (Recv, Option<Error>) {
    let mut filled = 0;
    let mut deadline = if resumable { slice_ms } else { body_deadline(slice_ms) };
    while filled < target.len() {
        let left = remaining_ms(deadline);
        if left == 0 {
            break;
        }
        match poll_one(stream.as_raw_fd(), POLLIN, left) {
            Ok(0) => {
                // Interrupted by a signal, or the wait elapsed. A signal
                // mid-frame ends the read as a truncation.
                if filled != 0 && remaining_ms(deadline) > 0 {
                    break;
                }
                continue;
            }
            Ok(_) => {}
            Err(e) => return (Recv::Error, Some(e.into())),
        }
        match stream.read(&mut target[filled..]) {
            Ok(0) => return (Recv::Error, Some(Error::new("OMT source disconnected"))),
            Ok(n) => {
                // This read is now committed to a frame, so the polling
                // slice stops bounding it.
                if filled == 0 {
                    deadline = body_deadline(slice_ms);
                }
                filled += n;
            }
            Err(e) if matches!(e.kind(), ErrorKind::Interrupted | ErrorKind::WouldBlock) => {}
            Err(e) => return (Recv::Error, Some(e.into())),
        }
    }
    if filled == target.len() {
        return (Recv::Ok, None);
    }
    if resumable && filled == 0 {
        return (Recv::WouldBlock, Some(Error::new("OMT socket deadline expired")));
    }
    (Recv::TimedOut, Some(Error::new("OMT frame was truncated by a timeout")))
}
