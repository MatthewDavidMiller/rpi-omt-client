//! Session channels (RFC 4254): opening, requests, flow control in both
//! directions, commands with or without a pseudo-terminal, and the `cat`
//! upload fallback. One channel is open at a time; packets for a channel
//! that is already gone are dropped.

use std::fs::File;

use omt_common::buf::{Cursor, wipe_vec};
use omt_common::{Error, Result};

use super::msg;
use super::transport::{IDLE_TIMEOUT_MS, Recv};
use super::wire::{self, Put, contains_bytes};
use super::{Session, SshResult};
use crate::core::shell_quote;
use crate::sys::{self, Cancel, OUTPUT_LIMIT, now_ms};

pub const LOCAL_WINDOW: u32 = 2 * 1024 * 1024;
pub const LOCAL_MAX_PACKET: u32 = 32768;
pub const COMMAND_TIMEOUT_MS: u64 = 30 * 60 * 1000;
pub const UPLOAD_TIMEOUT_MS: u64 = 30 * 60 * 1000;

/// The bytes an upload sends: a file or memory, rewindable for the
/// fallback.
#[derive(Debug)]
pub enum Source<'a> {
    Memory { data: &'a [u8], pos: usize },
    File(File),
}

impl Source<'_> {
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        match self {
            Source::Memory { data, pos } => {
                let n = buf.len().min(data.len() - *pos);
                buf[..n].copy_from_slice(&data[*pos..*pos + n]);
                *pos += n;
                Ok(n)
            }
            Source::File(f) => sys::read_some(f, buf),
        }
    }

    pub fn rewind(&mut self) -> Result<()> {
        match self {
            Source::Memory { pos, .. } => {
                *pos = 0;
                Ok(())
            }
            Source::File(f) => {
                use std::io::Seek;
                f.rewind().map_err(|e| sys::os_error("seek failed", &e))
            }
        }
    }
}

#[derive(Debug)]
pub struct Channel<'s, 'c> {
    pub(crate) session: &'s mut Session<'c>,
    local_id: u32,
    remote_id: u32,
    remote_window: u64,
    remote_max_packet: u32,
    /// Received bytes not yet returned to the window.
    consumed: u32,
    pub(crate) opened: bool,
    open_failed: bool,
    pub(crate) eof_received: bool,
    pub(crate) close_received: bool,
    close_sent: bool,
    eof_sent: bool,
    /// None pending, or the request's success.
    reply: Option<bool>,
    pub(crate) exit: Option<i32>,
    /// Data and extended data, kept only when asked, up to `limit` each.
    pub(crate) out: Vec<u8>,
    pub(crate) err: Vec<u8>,
    /// Packet scratch reused across writes; wiped with the channel.
    scratch: Vec<u8>,
    keep_out: bool,
    keep_err: bool,
    limit: usize,
}

impl Drop for Channel<'_, '_> {
    fn drop(&mut self) {
        wipe_vec(&mut self.out);
        wipe_vec(&mut self.err);
        wipe_vec(&mut self.scratch);
    }
}

impl<'s, 'c> Channel<'s, 'c> {
    /// Opens a session channel. Data and stderr are collected when asked,
    /// up to `limit` bytes each, past which the channel fails.
    pub fn open(
        session: &'s mut Session<'c>,
        keep_out: bool,
        keep_err: bool,
        limit: usize,
    ) -> Result<Self> {
        let local_id = session.next_channel;
        session.next_channel = session.next_channel.wrapping_add(1);
        let mut ch = Self {
            session,
            local_id,
            remote_id: 0,
            remote_window: 0,
            remote_max_packet: 0,
            consumed: 0,
            opened: false,
            open_failed: false,
            eof_received: false,
            close_received: false,
            close_sent: false,
            eof_sent: false,
            reply: None,
            exit: None,
            out: Vec::new(),
            err: Vec::new(),
            scratch: Vec::new(),
            keep_out,
            keep_err,
            limit,
        };
        let mut m = vec![msg::CHANNEL_OPEN];
        m.put_string(b"session");
        m.put_u32(local_id);
        m.put_u32(LOCAL_WINDOW);
        m.put_u32(LOCAL_MAX_PACKET);
        ch.session.t.send(&m)?;
        let idle = ch.session.t.idle_ms;
        ch.wait_until(
            |c| c.opened || c.open_failed,
            idle,
            "the SSH server did not open a channel",
        )?;
        if ch.open_failed {
            return Err(Error::new("the SSH server refused to open a session channel"));
        }
        Ok(ch)
    }

    fn send_window_adjust(&mut self) -> Result<()> {
        if self.consumed < LOCAL_WINDOW / 2 || self.close_sent {
            return Ok(());
        }
        let mut m = vec![msg::CHANNEL_WINDOW_ADJUST];
        m.put_u32(self.remote_id);
        m.put_u32(self.consumed);
        self.consumed = 0;
        self.session.t.send(&m)
    }

    fn sink(&mut self, to_err: Option<bool>, data: &[u8]) -> Result<()> {
        if data.len() > (LOCAL_WINDOW - self.consumed) as usize {
            return Err(Error::new("the SSH server overran the channel window"));
        }
        self.consumed += u32::try_from(data.len()).unwrap_or(u32::MAX);
        if let Some(is_err) = to_err
            && (if is_err { self.keep_err } else { self.keep_out })
        {
            let into = if is_err { &mut self.err } else { &mut self.out };
            if into.len() + data.len() > self.limit {
                return Err(Error::new("remote command output exceeded 4 MiB"));
            }
            into.extend_from_slice(data);
        }
        self.send_window_adjust()
    }

    fn send_close(&mut self) -> Result<()> {
        if self.close_sent || !self.opened {
            return Ok(());
        }
        let mut m = vec![msg::CHANNEL_CLOSE];
        m.put_u32(self.remote_id);
        self.close_sent = true;
        self.session.t.send(&m)
    }

    /// Applies the packet in the transport's payload to the channel.
    fn dispatch(&mut self) -> Result<()> {
        let payload = std::mem::take(&mut self.session.t.payload);
        let result = self.dispatch_payload(&payload);
        self.session.t.payload = payload;
        result
    }

    fn dispatch_payload(&mut self, payload: &[u8]) -> Result<()> {
        let kind = payload.first().copied().unwrap_or(0);
        let mut r = Cursor::new(payload.get(1..).unwrap_or_default());
        let malformed = || Error::new("the SSH server sent a malformed channel message");
        if kind == msg::CHANNEL_OPEN {
            // The server may not open channels to a client that offered no
            // forwarding; refuse whatever it asks for.
            if let (Some(_), Some(sender)) = (wire::string(&mut r), wire::u32(&mut r)) {
                let mut m = vec![msg::CHANNEL_OPEN_FAILURE];
                m.put_u32(sender);
                m.put_u32(1);
                m.put_string(b"");
                m.put_string(b"");
                return self.session.t.send(&m);
            }
            return Err(Error::new(format!("unexpected SSH message {kind}")));
        }
        if !(msg::CHANNEL_OPEN..=msg::CHANNEL_FAILURE).contains(&kind) {
            return Err(Error::new(format!("unexpected SSH message {kind}")));
        }
        let recipient = wire::u32(&mut r).ok_or_else(malformed)?;
        if recipient != self.local_id {
            return Ok(()); // a channel already closed
        }
        match kind {
            msg::CHANNEL_OPEN_CONFIRMATION => {
                self.remote_id = wire::u32(&mut r).ok_or_else(malformed)?;
                self.remote_window = u64::from(wire::u32(&mut r).ok_or_else(malformed)?);
                self.remote_max_packet = wire::u32(&mut r).ok_or_else(malformed)?;
                self.opened = true;
            }
            msg::CHANNEL_OPEN_FAILURE => self.open_failed = true,
            msg::CHANNEL_WINDOW_ADJUST => {
                let add = wire::u32(&mut r).ok_or_else(malformed)?;
                self.remote_window = (self.remote_window + u64::from(add)).min(u64::from(u32::MAX));
            }
            msg::CHANNEL_DATA => {
                let data = wire::string(&mut r).ok_or_else(malformed)?;
                self.sink(Some(false), data)?;
            }
            msg::CHANNEL_EXTENDED_DATA => {
                let code = wire::u32(&mut r).ok_or_else(malformed)?;
                let data = wire::string(&mut r).ok_or_else(malformed)?;
                self.sink(if code == 1 { Some(true) } else { None }, data)?;
            }
            msg::CHANNEL_EOF => self.eof_received = true,
            msg::CHANNEL_CLOSE => {
                self.close_received = true;
                self.send_close()?;
            }
            msg::CHANNEL_REQUEST => {
                let name = wire::string(&mut r).ok_or_else(malformed)?;
                let want_reply = wire::boolean(&mut r).ok_or_else(malformed)?;
                if name == b"exit-status" {
                    let status = wire::u32(&mut r).ok_or_else(malformed)?;
                    self.exit = Some(i32::try_from(status).unwrap_or(1));
                }
                if want_reply {
                    let mut m = vec![msg::CHANNEL_FAILURE];
                    m.put_u32(self.remote_id);
                    self.session.t.send(&m)?;
                }
            }
            msg::CHANNEL_SUCCESS => self.reply = Some(true),
            msg::CHANNEL_FAILURE => self.reply = Some(false),
            _ => return Err(malformed()),
        }
        Ok(())
    }

    /// Waits up to `wait_ms` for one packet and dispatches it; false on
    /// timeout.
    pub fn poll(&mut self, wait_ms: u64) -> Result<bool> {
        if self.session.t.recv(wait_ms)? == Recv::Timeout {
            return Ok(false);
        }
        self.dispatch()?;
        Ok(true)
    }

    /// Waits, servicing the channel, until `done` holds or time runs out.
    fn wait_until(&mut self, done: fn(&Self) -> bool, timeout_ms: u64, text: &str) -> Result<()> {
        let deadline = now_ms() + timeout_ms;
        while !done(self) {
            let now = now_ms();
            if now >= deadline {
                return Err(Error::new(text));
            }
            self.poll((deadline - now).min(1000))?;
        }
        Ok(())
    }

    /// Sends a channel request and, when asked, waits for its reply;
    /// returns whether the server accepted it.
    pub fn request(
        &mut self,
        kind: &str,
        want_reply: bool,
        extra: &[u8],
        timeout_ms: u64,
    ) -> Result<bool> {
        let mut m = vec![msg::CHANNEL_REQUEST];
        m.put_u32(self.remote_id);
        m.put_string(kind.as_bytes());
        m.put_bool(want_reply);
        m.extend_from_slice(extra);
        self.reply = None;
        self.session.t.send(&m)?;
        if !want_reply {
            return Ok(true);
        }
        let text = format!("the SSH server did not answer the {kind} request");
        self.wait_until(|c| c.reply.is_some() || c.close_received, timeout_ms, &text)?;
        Ok(self.reply == Some(true))
    }

    /// Sends data within the peer's window, servicing incoming packets while
    /// it waits for the window to open.
    pub fn write(
        &mut self,
        mut data: &[u8],
        deadline_ms: u64,
        cancel: Option<&Cancel>,
    ) -> Result<()> {
        let mut m = std::mem::take(&mut self.scratch);
        m.reserve(LOCAL_MAX_PACKET as usize + 64);
        let result = (|| {
            while !data.is_empty() {
                sys::check_cancel(cancel)?;
                if self.close_received || self.eof_sent {
                    return Err(Error::new("the remote side closed the channel"));
                }
                let room = self
                    .remote_window
                    .min(u64::from(self.remote_max_packet))
                    .min(u64::from(LOCAL_MAX_PACKET));
                if room == 0 {
                    if now_ms() >= deadline_ms {
                        return Err(Error::new("upload timed out waiting for the SSH window"));
                    }
                    self.poll(100)?;
                    continue;
                }
                let n = data.len().min(usize::try_from(room).unwrap_or(usize::MAX));
                m.clear();
                m.put_u8(msg::CHANNEL_DATA);
                m.put_u32(self.remote_id);
                m.put_string(&data[..n]);
                self.session.t.send(&m)?;
                self.remote_window -= n as u64;
                data = &data[n..];
            }
            Ok(())
        })();
        m.clear();
        self.scratch = m;
        result
    }

    pub fn eof(&mut self) -> Result<()> {
        if self.eof_sent || self.close_sent {
            return Ok(());
        }
        let mut m = vec![msg::CHANNEL_EOF];
        m.put_u32(self.remote_id);
        self.eof_sent = true;
        self.session.t.send(&m)
    }

    pub fn close(&mut self) {
        let _ = self.send_close();
    }
}

/// Feeds raw messages to a fresh channel on an unconnected session, as a
/// server would deliver them; EXT_INFO goes to the transport. For the
/// fuzzer: replies the dispatcher sends fail harmlessly.
#[doc(hidden)]
pub fn dispatch_detached(messages: &[&[u8]]) {
    let mut session = Session { t: super::transport::Transport::new(None), next_channel: 1 };
    let mut ch = Channel {
        session: &mut session,
        local_id: 0,
        remote_id: 0,
        remote_window: 0,
        remote_max_packet: 0,
        consumed: 0,
        opened: true,
        open_failed: false,
        eof_received: false,
        close_received: false,
        close_sent: false,
        eof_sent: false,
        reply: None,
        exit: None,
        out: Vec::new(),
        err: Vec::new(),
        scratch: Vec::new(),
        keep_out: true,
        keep_err: true,
        limit: OUTPUT_LIMIT,
    };
    for m in messages.iter().filter(|m| !m.is_empty()) {
        ch.session.t.payload = m.to_vec();
        let _ = if m[0] == msg::EXT_INFO { ch.session.t.handle_ext_info() } else { ch.dispatch() };
    }
}

fn run_inner(
    s: &mut Session<'_>,
    command: &str,
    marker: Option<&str>,
    input: &[u8],
    pty: bool,
    cancel: Option<&Cancel>,
) -> Result<SshResult> {
    sys::check_cancel(cancel)?;
    let idle = s.t.idle_ms;
    let mut ch = Channel::open(s, true, true, OUTPUT_LIMIT)?;
    if pty {
        let mut req = Vec::new();
        req.put_string(b"dumb");
        req.put_u32(80);
        req.put_u32(24);
        req.put_u32(0);
        req.put_u32(0);
        req.put_string(b"\0"); // no modes: TTY_OP_END only
        if !ch.request("pty-req", true, &req, idle)? {
            return Err(Error::new("the SSH server refused a pseudo-terminal"));
        }
    }
    let mut req = Vec::new();
    req.put_string(command.as_bytes());
    if !ch.request("exec", true, &req, idle)? {
        return Err(Error::new("the SSH server refused to run the command"));
    }
    let deadline = now_ms() + COMMAND_TIMEOUT_MS;
    let mut input_sent = input.is_empty();
    if input_sent || marker.is_none() {
        if !input.is_empty() {
            ch.write(input, deadline, cancel)?;
        }
        input_sent = true;
        ch.eof()?;
    }
    let mut last_activity = now_ms();
    while !ch.close_received {
        if sys::cancelled(cancel) {
            ch.close();
            return Err(sys::cancelled_error());
        }
        let got = ch.poll(100)?;
        let now = now_ms();
        if got {
            last_activity = now;
        }
        if let Some(marker) = marker
            && !input_sent
            && (contains_bytes(&ch.out, marker) || contains_bytes(&ch.err, marker))
        {
            ch.write(input, deadline, cancel)?;
            ch.eof()?;
            input_sent = true;
        }
        if now - last_activity > IDLE_TIMEOUT_MS || now > deadline {
            ch.close();
            return Err(Error::new("remote command produced no output for 60 seconds"));
        }
    }
    Ok(SshResult {
        exit_code: ch.exit.unwrap_or(1),
        out: String::from_utf8_lossy(&ch.out).into_owned(),
        err: String::from_utf8_lossy(&ch.err).into_owned(),
    })
}

pub fn run(
    s: &mut Session<'_>,
    command: &str,
    input: &[u8],
    cancel: Option<&Cancel>,
) -> Result<SshResult> {
    run_inner(s, command, None, input, false, cancel)
}

/// Runs on a pseudo-terminal and sends `input` only once `marker` appears
/// in the output. Reserved for tools such as Alpine's `su`, which read a
/// password from a terminal only.
pub fn run_pty_after_marker(
    s: &mut Session<'_>,
    command: &str,
    marker: &str,
    input: &[u8],
    cancel: Option<&Cancel>,
) -> Result<SshResult> {
    run_inner(s, command, Some(marker), input, true, cancel)
}

/// "umask 077 && cat > '<remote>'", the upload fallback's command.
pub fn shell_upload_command(remote: &str) -> String {
    format!("umask 077 && cat > {}", shell_quote(remote))
}

pub fn shell_upload(
    s: &mut Session<'_>,
    src: &mut Source<'_>,
    remote: &str,
    cancel: Option<&Cancel>,
) -> Result<()> {
    sys::check_cancel(cancel)?;
    let idle = s.t.idle_ms;
    // cat's output is not read: the exit status is the verdict.
    let mut ch = Channel::open(s, false, false, 0)?;
    let mut req = Vec::new();
    req.put_string(shell_upload_command(remote).as_bytes());
    if !ch.request("exec", true, &req, idle)? {
        return Err(Error::new("the SSH server refused to run cat"));
    }
    let deadline = now_ms() + UPLOAD_TIMEOUT_MS;
    let mut chunk = vec![0_u8; LOCAL_MAX_PACKET as usize];
    loop {
        if sys::cancelled(cancel) {
            ch.close();
            return Err(sys::cancelled_error());
        }
        if now_ms() > deadline {
            ch.close();
            return Err(Error::new("shell upload timed out"));
        }
        let n = src.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        ch.write(&chunk[..n], deadline, cancel)?;
    }
    ch.eof()?;
    if let Err(e) = ch.wait_until(
        |c| c.close_received,
        UPLOAD_TIMEOUT_MS,
        "shell upload timed out waiting for exit status",
    ) {
        ch.close();
        return Err(e);
    }
    match ch.exit {
        Some(0) => Ok(()),
        code => Err(Error::new(format!(
            "remote cat exited {} while writing {remote}",
            code.unwrap_or(1)
        ))),
    }
}
