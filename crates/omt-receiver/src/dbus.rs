//! A minimal D-Bus client: enough of the wire protocol to call methods on
//! the system bus and read the signals sent back. It marshals only the types
//! the Avahi browse uses, and every incoming message is bounded and
//! validated before a field is read out of it.

use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::ops::Range;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixStream};
use std::time::Duration;

use omt_common::Error;
use omt_common::timefmt::remaining_ms;

/// Largest message accepted from the bus. Avahi's replies and signals are a
/// few hundred bytes; this leaves room for long TXT records.
pub const MAX_MESSAGE: usize = 256 * 1024;
/// Signals held while a method call waits for its reply.
pub const MAX_PENDING: usize = 256;

pub const METHOD_CALL: u8 = 1;
pub const METHOD_RETURN: u8 = 2;
pub const ERROR: u8 = 3;
pub const SIGNAL: u8 = 4;

const FIELD_PATH: u8 = 1;
const FIELD_INTERFACE: u8 = 2;
const FIELD_MEMBER: u8 = 3;
const FIELD_ERROR_NAME: u8 = 4;
const FIELD_REPLY_SERIAL: u8 = 5;
const FIELD_DESTINATION: u8 = 6;
const FIELD_SENDER: u8 = 7;
const FIELD_SIGNATURE: u8 = 8;
const FIELD_UNIX_FDS: u8 = 9;
const FLAG_NO_REPLY_EXPECTED: u8 = 0x1;
const DEFAULT_SYSTEM_BUS: &str = "unix:path=/var/run/dbus/system_bus_socket";

/// One received message. Header strings are ranges into `raw`.
#[derive(Clone, Debug)]
pub struct Message {
    raw: Vec<u8>,
    big_endian: bool,
    pub kind: u8,
    pub serial: u32,
    pub reply_serial: u32,
    path: Option<Range<usize>>,
    interface: Option<Range<usize>>,
    member: Option<Range<usize>>,
    signature: Option<Range<usize>>,
    body: Range<usize>,
}

fn load32(raw: &[u8], at: usize, big: bool) -> u32 {
    let b = [raw[at], raw[at + 1], raw[at + 2], raw[at + 3]];
    if big { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) }
}

fn align_to(raw: &[u8], pos: &mut usize, end: usize, alignment: usize) -> bool {
    let aligned = pos.next_multiple_of(alignment);
    if aligned > end {
        return false;
    }
    // The specification requires padding bytes to be zero.
    if raw[*pos..aligned].iter().any(|&b| b != 0) {
        return false;
    }
    *pos = aligned;
    true
}

/// Reads a u32-length string (or a u8-length signature) at `pos`.
fn read_str(
    raw: &[u8],
    pos: &mut usize,
    end: usize,
    big: bool,
    signature: bool,
) -> Option<Range<usize>> {
    let len = if signature {
        let len = usize::from(*raw.get(*pos).filter(|_| *pos < end)?);
        *pos += 1;
        len
    } else {
        if !align_to(raw, pos, end, 4) || end - *pos < 4 {
            return None;
        }
        let len = load32(raw, *pos, big) as usize;
        *pos += 4;
        len
    };
    if len >= end - *pos || raw[*pos + len] != 0 {
        return None;
    }
    let range = *pos..*pos + len;
    let s = &raw[range.clone()];
    if s.contains(&0) || std::str::from_utf8(s).is_err() {
        return None;
    }
    *pos += len + 1;
    Some(range)
}

/// The total length of the message starting at `data` once 16 bytes are
/// known, or `None` when it is malformed or oversized.
pub fn message_length(data: &[u8]) -> Option<usize> {
    if data.len() < 16 {
        return None;
    }
    let big = match data[0] {
        b'l' => false,
        b'B' => true,
        _ => return None,
    };
    if data[3] != 1 {
        return None;
    }
    let body = u64::from(load32(data, 4, big));
    let fields = u64::from(load32(data, 12, big));
    let header = (16 + fields).next_multiple_of(8);
    let total = header + body;
    usize::try_from(total).ok().filter(|&t| t <= MAX_MESSAGE)
}

impl Message {
    /// Parses one complete message.
    pub fn parse(raw: Vec<u8>) -> Option<Self> {
        let total = message_length(&raw)?;
        if total != raw.len() {
            return None;
        }
        let big = raw[0] == b'B';
        let kind = raw[1];
        let serial = load32(&raw, 8, big);
        if !(1..=4).contains(&kind) || serial == 0 {
            return None;
        }
        let fields_len = load32(&raw, 12, big) as usize;
        let mut m = Message {
            big_endian: big,
            kind,
            serial,
            reply_serial: 0,
            path: None,
            interface: None,
            member: None,
            signature: None,
            body: 0..0,
            raw: Vec::new(),
        };
        let (mut pos, end) = (16, 16 + fields_len);
        while pos < end {
            if !align_to(&raw, &mut pos, end, 8) || pos >= end {
                return None;
            }
            let code = raw[pos];
            pos += 1;
            let sig = read_str(&raw, &mut pos, end, big, true)?;
            if sig.len() != 1 {
                return None;
            }
            let sig = raw[sig.start];
            match code {
                FIELD_PATH | FIELD_INTERFACE | FIELD_MEMBER | FIELD_ERROR_NAME
                | FIELD_DESTINATION | FIELD_SENDER => {
                    let want = if code == FIELD_PATH { b'o' } else { b's' };
                    if sig != want {
                        return None;
                    }
                    let s = read_str(&raw, &mut pos, end, big, false)?;
                    match code {
                        FIELD_PATH => m.path = Some(s),
                        FIELD_INTERFACE => m.interface = Some(s),
                        FIELD_MEMBER => m.member = Some(s),
                        _ => {}
                    }
                }
                FIELD_SIGNATURE => {
                    if sig != b'g' {
                        return None;
                    }
                    m.signature = Some(read_str(&raw, &mut pos, end, big, true)?);
                }
                FIELD_REPLY_SERIAL | FIELD_UNIX_FDS => {
                    if sig != b'u' || !align_to(&raw, &mut pos, end, 4) || end - pos < 4 {
                        return None;
                    }
                    let v = load32(&raw, pos, big);
                    if code == FIELD_REPLY_SERIAL {
                        m.reply_serial = v;
                    } else if v != 0 {
                        return None;
                    }
                    pos += 4;
                }
                // Unknown fields must be skipped; only simple types are
                // accepted so the skip stays a fixed-size step.
                _ => match sig {
                    b's' | b'o' => {
                        read_str(&raw, &mut pos, end, big, false)?;
                    }
                    b'g' => {
                        read_str(&raw, &mut pos, end, big, true)?;
                    }
                    b'u' | b'i' => {
                        if !align_to(&raw, &mut pos, end, 4) || end - pos < 4 {
                            return None;
                        }
                        pos += 4;
                    }
                    b'y' => {
                        if pos >= end {
                            return None;
                        }
                        pos += 1;
                    }
                    _ => return None,
                },
            }
        }
        let body = end.next_multiple_of(8);
        if body > raw.len() || raw[end..body].iter().any(|&b| b != 0) {
            return None;
        }
        m.body = body..raw.len();
        m.raw = raw;
        Some(m)
    }

    fn text(&self, r: Option<&Range<usize>>) -> Option<&str> {
        r.and_then(|r| std::str::from_utf8(&self.raw[r.clone()]).ok())
    }

    pub fn path(&self) -> Option<&str> {
        self.text(self.path.as_ref())
    }

    pub fn interface(&self) -> Option<&str> {
        self.text(self.interface.as_ref())
    }

    pub fn member(&self) -> Option<&str> {
        self.text(self.member.as_ref())
    }

    pub fn signature(&self) -> &str {
        self.text(self.signature.as_ref()).unwrap_or("")
    }

    pub fn reader(&self) -> Reader<'_> {
        Reader { m: self, pos: self.body.start, end: self.body.end, failed: false }
    }
}

/// A cursor over a message body. Every read checks bounds, alignment
/// padding, and string termination; any failure sets `failed` and later
/// reads return zero values.
#[derive(Debug)]
pub struct Reader<'a> {
    m: &'a Message,
    pub pos: usize,
    pub end: usize,
    pub failed: bool,
}

impl<'a> Reader<'a> {
    fn align(&mut self, n: usize) -> bool {
        // Body alignment is relative to the body start, which is 8-aligned.
        if !self.failed && !align_to(&self.m.raw, &mut self.pos, self.end, n) {
            self.failed = true;
        }
        !self.failed
    }

    pub fn u32(&mut self) -> u32 {
        if !self.align(4) || self.end - self.pos < 4 {
            self.failed = true;
            return 0;
        }
        let v = load32(&self.m.raw, self.pos, self.m.big_endian);
        self.pos += 4;
        v
    }

    pub fn i32(&mut self) -> i32 {
        self.u32().cast_signed()
    }

    pub fn u16(&mut self) -> u16 {
        if !self.align(2) || self.end - self.pos < 2 {
            self.failed = true;
            return 0;
        }
        let b = [self.m.raw[self.pos], self.m.raw[self.pos + 1]];
        self.pos += 2;
        if self.m.big_endian { u16::from_be_bytes(b) } else { u16::from_le_bytes(b) }
    }

    pub fn bool(&mut self) -> bool {
        let v = self.u32();
        if v > 1 {
            self.failed = true;
        }
        v == 1
    }

    /// Strings and object paths.
    pub fn string(&mut self) -> &'a str {
        if self.failed {
            return "";
        }
        if let Some(r) = read_str(&self.m.raw, &mut self.pos, self.end, self.m.big_endian, false) {
            std::str::from_utf8(&self.m.raw[r]).unwrap_or("")
        } else {
            self.failed = true;
            ""
        }
    }

    /// Skips an `aay` (an array of byte arrays).
    pub fn skip_aay(&mut self) {
        let outer = self.u32() as usize;
        // Elements of an `ay` array are 4-aligned (their u32 length).
        if self.failed || !self.align(4) {
            return;
        }
        if outer > self.end - self.pos {
            self.failed = true;
            return;
        }
        let stop = self.pos + outer;
        while self.pos < stop && !self.failed {
            let inner = self.u32() as usize;
            if self.failed || inner > stop.saturating_sub(self.pos) {
                self.failed = true;
                return;
            }
            self.pos += inner;
        }
        if self.pos != stop {
            self.failed = true;
        }
    }
}

/// A body argument for a call.
#[derive(Clone, Copy, Debug)]
pub enum Arg<'a> {
    I32(i32),
    U32(u32),
    Str(&'a str),
    Path(&'a str),
}

fn w_align(b: &mut Vec<u8>, n: usize) {
    b.resize(b.len().next_multiple_of(n), 0);
}

fn w_u32(b: &mut Vec<u8>, v: u32) {
    w_align(b, 4);
    b.extend_from_slice(&v.to_le_bytes());
}

fn w_str(b: &mut Vec<u8>, s: &str) -> Option<()> {
    w_u32(b, u32::try_from(s.len()).ok()?);
    b.extend_from_slice(s.as_bytes());
    b.push(0);
    Some(())
}

fn w_sig(b: &mut Vec<u8>, s: &str) -> Option<()> {
    b.push(u8::try_from(s.len()).ok()?);
    b.extend_from_slice(s.as_bytes());
    b.push(0);
    Some(())
}

fn w_field(b: &mut Vec<u8>, code: u8, kind: u8, s: &str) -> Option<()> {
    w_align(b, 8);
    b.push(code);
    b.extend_from_slice(&[1, kind, 0]);
    if kind == b'g' { w_sig(b, s) } else { w_str(b, s) }
}

/// Marshals a method call.
#[allow(clippy::too_many_arguments)]
pub fn build_call(
    serial: u32,
    flags: u8,
    destination: Option<&str>,
    path: &str,
    interface: Option<&str>,
    member: &str,
    args: &[Arg<'_>],
) -> Option<Vec<u8>> {
    if args.len() >= 32 {
        return None;
    }
    let mut signature = String::new();
    let mut body = Vec::new();
    for a in args {
        match a {
            Arg::I32(v) => {
                signature.push('i');
                w_u32(&mut body, v.cast_unsigned());
            }
            Arg::U32(v) => {
                signature.push('u');
                w_u32(&mut body, *v);
            }
            Arg::Str(s) => {
                signature.push('s');
                w_str(&mut body, s)?;
            }
            Arg::Path(s) => {
                signature.push('o');
                w_str(&mut body, s)?;
            }
        }
    }
    let mut out = vec![b'l', METHOD_CALL, flags, 1];
    out.extend_from_slice(&u32::try_from(body.len()).ok()?.to_le_bytes());
    out.extend_from_slice(&serial.to_le_bytes());
    w_u32(&mut out, 0); // field array length, patched below
    let fields_start = out.len();
    w_field(&mut out, FIELD_PATH, b'o', path)?;
    if let Some(i) = interface {
        w_field(&mut out, FIELD_INTERFACE, b's', i)?;
    }
    w_field(&mut out, FIELD_MEMBER, b's', member)?;
    if let Some(d) = destination {
        w_field(&mut out, FIELD_DESTINATION, b's', d)?;
    }
    if !args.is_empty() {
        w_field(&mut out, FIELD_SIGNATURE, b'g', &signature)?;
    }
    let fields_len = u32::try_from(out.len() - fields_start).ok()?;
    w_align(&mut out, 8);
    out.extend_from_slice(&body);
    out[12..16].copy_from_slice(&fields_len.to_le_bytes());
    (out.len() <= MAX_MESSAGE).then_some(out)
}

#[derive(Debug)]
pub struct Bus {
    stream: UnixStream,
    serial: u32,
    pending: VecDeque<Message>,
    inbox: Vec<u8>,
}

fn timeout(deadline_ms: u64) -> Option<Duration> {
    let left = remaining_ms(deadline_ms);
    (left > 0).then(|| Duration::from_millis(left))
}

/// Decodes the %xx escapes D-Bus addresses use for values.
fn unescape(value: &str) -> Option<Vec<u8>> {
    let b = value.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    Some(out)
}

fn connect_address(address: &str) -> Option<UnixStream> {
    let rest = address.strip_prefix("unix:")?;
    let mut target: Option<SocketAddr> = None;
    for pair in rest.split(',') {
        let Some((key, value)) = pair.split_once('=') else { continue };
        let value = unescape(value)?;
        if value.len() >= 108 {
            return None;
        }
        match key {
            "path" => {
                let path = <std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(&value);
                target = SocketAddr::from_pathname(path).ok();
            }
            "abstract" => target = SocketAddr::from_abstract_name(&value).ok(),
            _ => {}
        }
    }
    UnixStream::connect_addr(&target?).ok()
}

impl Bus {
    /// Connects to DBUS_SYSTEM_BUS_ADDRESS (or the default socket),
    /// authenticates with EXTERNAL, and says Hello, all before the deadline.
    pub fn open_system(deadline_ms: u64) -> Result<Self, Error> {
        let configured = std::env::var("DBUS_SYSTEM_BUS_ADDRESS").ok().filter(|a| !a.is_empty());
        let address = configured.as_deref().unwrap_or(DEFAULT_SYSTEM_BUS);
        // An address may list alternatives separated by ';'.
        let stream = address
            .split(';')
            .find_map(connect_address)
            .ok_or_else(|| Error::new("unable to connect to the system bus"))?;
        let mut bus = Bus { stream, serial: 0, pending: VecDeque::new(), inbox: Vec::new() };
        bus.authenticate(deadline_ms)?;
        bus.call(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "Hello",
            &[],
            deadline_ms,
        )?;
        Ok(bus)
    }

    fn send_all(&mut self, data: &[u8], deadline_ms: u64) -> Result<(), Error> {
        let t = timeout(deadline_ms).ok_or_else(|| Error::new("D-Bus write timed out"))?;
        self.stream.set_write_timeout(Some(t))?;
        match self.stream.write_all(data) {
            Ok(()) => Ok(()),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                Err(Error::new("D-Bus write timed out"))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Reads into the inbox; false at the deadline.
    fn fill(
        &mut self,
        deadline_ms: u64,
        timeout_text: &str,
        closed_text: &str,
    ) -> Result<(), Error> {
        let t = timeout(deadline_ms).ok_or_else(|| Error::new(timeout_text))?;
        self.stream.set_read_timeout(Some(t))?;
        let mut chunk = [0_u8; 8192];
        loop {
            match self.stream.read(&mut chunk) {
                Ok(0) => return Err(Error::new(closed_text)),
                Ok(n) => {
                    if self.inbox.len() + n > MAX_MESSAGE * 2 {
                        return Err(Error::new("D-Bus input exceeds its buffer"));
                    }
                    self.inbox.extend_from_slice(&chunk[..n]);
                    return Ok(());
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    return Err(Error::new(timeout_text));
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    fn authenticate(&mut self, deadline_ms: u64) -> Result<(), Error> {
        let uid = omt_sys::posix::current_uid().to_string();
        let hex = omt_common::buf::hex(uid.as_bytes());
        self.send_all(format!("\0AUTH EXTERNAL {hex}\r\n").as_bytes(), deadline_ms)?;
        // One CRLF-terminated line.
        let line = loop {
            if let Some(end) = self.inbox.windows(2).position(|w| w == b"\r\n") {
                let line = String::from_utf8_lossy(&self.inbox[..end]).into_owned();
                self.inbox.drain(..end + 2);
                break line;
            }
            if self.inbox.len() >= 512 {
                return Err(Error::new("D-Bus authentication line too long"));
            }
            self.fill(
                deadline_ms,
                "D-Bus authentication timed out",
                "D-Bus connection closed during authentication",
            )?;
        };
        if !line.starts_with("OK ") {
            return Err(Error::new("D-Bus authentication was refused"));
        }
        self.send_all(b"BEGIN\r\n", deadline_ms)
    }

    /// Reads the next complete message from the socket.
    fn receive(&mut self, deadline_ms: u64) -> Result<Message, Error> {
        loop {
            if self.inbox.len() >= 16 {
                let total = message_length(&self.inbox)
                    .ok_or_else(|| Error::new("malformed or oversized D-Bus message"))?;
                if self.inbox.len() >= total {
                    let raw: Vec<u8> = self.inbox.drain(..total).collect();
                    return Message::parse(raw)
                        .ok_or_else(|| Error::new("malformed D-Bus message"));
                }
            }
            self.fill(deadline_ms, "D-Bus deadline expired", "D-Bus connection closed")?;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn send_call(
        &mut self,
        flags: u8,
        destination: Option<&str>,
        path: &str,
        interface: Option<&str>,
        member: &str,
        args: &[Arg<'_>],
        deadline_ms: u64,
    ) -> Result<u32, Error> {
        self.serial = self.serial.wrapping_add(1).max(1);
        let serial = self.serial;
        let msg = build_call(serial, flags, destination, path, interface, member, args)
            .ok_or_else(|| Error::new("unable to marshal the D-Bus call"))?;
        self.send_all(&msg, deadline_ms)?;
        Ok(serial)
    }

    /// Calls a method and waits until the deadline for its reply. Signals
    /// that arrive meanwhile are queued for [`Bus::next`]. An error reply
    /// fails the call with its name.
    pub fn call(
        &mut self,
        destination: Option<&str>,
        path: &str,
        interface: Option<&str>,
        member: &str,
        args: &[Arg<'_>],
        deadline_ms: u64,
    ) -> Result<Message, Error> {
        let serial = self.send_call(0, destination, path, interface, member, args, deadline_ms)?;
        loop {
            let m = self.receive(deadline_ms)?;
            if (m.kind == METHOD_RETURN || m.kind == ERROR) && m.reply_serial == serial {
                if m.kind == ERROR {
                    return Err(Error::new(format!("D-Bus call {member} failed")));
                }
                return Ok(m);
            }
            if m.kind == SIGNAL {
                // Bounded: the oldest queued signal gives way, as a broadcast
                // queue with overflow does.
                if self.pending.len() == MAX_PENDING {
                    self.pending.pop_front();
                }
                self.pending.push_back(m);
            }
        }
    }

    /// Sends a call flagged NO_REPLY_EXPECTED and does not wait.
    pub fn send_no_reply(
        &mut self,
        destination: &str,
        path: &str,
        interface: &str,
        member: &str,
        deadline_ms: u64,
    ) -> Result<(), Error> {
        self.send_call(
            FLAG_NO_REPLY_EXPECTED,
            Some(destination),
            path,
            Some(interface),
            member,
            &[],
            deadline_ms,
        )
        .map(|_| ())
    }

    /// The next queued or incoming message.
    pub fn next(&mut self, deadline_ms: u64) -> Result<Message, Error> {
        if let Some(m) = self.pending.pop_front() {
            return Ok(m);
        }
        self.receive(deadline_ms)
    }
}
