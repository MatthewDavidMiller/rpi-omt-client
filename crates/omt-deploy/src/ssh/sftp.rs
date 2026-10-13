//! The SFTP version 3 subset an upload needs (draft-ietf-secsh-filexfer-02):
//! open for writing with create and truncate, pipelined writes, close, and
//! remove when an upload is cancelled.

use omt_common::buf::Cursor;
use omt_common::{Error, Result};

use super::Session;
use super::channel::{Channel, LOCAL_WINDOW, Source, UPLOAD_TIMEOUT_MS};
use super::transport::IDLE_TIMEOUT_MS;
use super::wire::{self, Put};
use crate::sys::{self, Cancel, now_ms};

const INIT: u8 = 1;
const VERSION: u8 = 2;
const OPEN: u8 = 3;
const CLOSE: u8 = 4;
const WRITE: u8 = 6;
const REMOVE: u8 = 13;
const STATUS: u8 = 101;
const HANDLE: u8 = 102;

const FLAG_WRITE: u32 = 0x02;
const FLAG_CREAT: u32 = 0x08;
const FLAG_TRUNC: u32 = 0x10;
const MAX_PACKET: usize = 256 * 1024;
const CHUNK: usize = 32 * 1024;
const IN_FLIGHT: u32 = 32;
const SUBSYSTEM_TIMEOUT_MS: u64 = 15_000;

struct Sftp<'s, 'c> {
    ch: Channel<'s, 'c>,
    /// The current reply.
    packet: Vec<u8>,
    next_id: u32,
}

fn header(kind: u8, id: u32) -> Vec<u8> {
    let mut b = vec![kind];
    b.put_u32(id);
    b
}

impl Sftp<'_, '_> {
    fn send(&mut self, body: &[u8], deadline: u64, cancel: Option<&Cancel>) -> Result<()> {
        let len = u32::try_from(body.len()).unwrap_or(u32::MAX).to_be_bytes();
        self.ch.write(&len, deadline, cancel)?;
        self.ch.write(body, deadline, cancel)
    }

    /// Reads one reply into `packet`, waiting no more than the idle timeout
    /// between packets and the deadline overall.
    fn reply(&mut self, deadline: u64, cancel: Option<&Cancel>) -> Result<()> {
        let mut last = now_ms();
        loop {
            let rx = &self.ch.out;
            if rx.len() >= 4 {
                let len = u32::from_be_bytes([rx[0], rx[1], rx[2], rx[3]]) as usize;
                if len == 0 || len > MAX_PACKET {
                    return Err(Error::new("the SFTP server sent a malformed reply"));
                }
                if rx.len() >= 4 + len {
                    self.packet.clear();
                    self.packet.extend_from_slice(&rx[4..4 + len]);
                    self.ch.out.drain(..4 + len);
                    return Ok(());
                }
            }
            if self.ch.close_received || self.ch.eof_received {
                return Err(Error::new("the SFTP server closed the session"));
            }
            sys::check_cancel(cancel)?;
            let now = now_ms();
            if now > deadline || now - last > IDLE_TIMEOUT_MS {
                return Err(Error::new("SFTP upload timed out"));
            }
            if self.ch.poll(100)? {
                last = now;
            }
        }
    }

    /// Checks the reply is an OK status for request `id` (or, without one,
    /// any request); returns the id answered.
    fn status_ok(&self, id: Option<u32>, what: &str) -> Result<u32> {
        let mut r = Cursor::new(&self.packet);
        let unexpected =
            || Error::new(format!("the SFTP server sent an unexpected reply to {what}"));
        if wire::u8(&mut r) != Some(STATUS) {
            return Err(unexpected());
        }
        let got = wire::u32(&mut r)
            .filter(|&got| id.is_none_or(|id| got == id))
            .ok_or_else(unexpected)?;
        let code = wire::u32(&mut r).ok_or_else(unexpected)?;
        let message = wire::string(&mut r).ok_or_else(unexpected)?;
        if code != 0 {
            let shown = String::from_utf8_lossy(&message[..message.len().min(200)]);
            return Err(Error::new(format!("SFTP {what} failed: {shown} (status {code})")));
        }
        Ok(got)
    }

    /// Best effort: close the handle and remove the partial file.
    fn abandon(&mut self, handle: &[u8], remote: &str) {
        let deadline = now_ms() + 5000;
        let mut b = header(CLOSE, self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        b.put_string(handle);
        let _ = self.send(&b, deadline, None);
        let mut b = header(REMOVE, self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        b.put_string(remote.as_bytes());
        if self.send(&b, deadline, None).is_ok() {
            // Wait briefly so the removal lands before the channel goes.
            for _ in 0..2 {
                if self.reply(deadline, None).is_err() {
                    break;
                }
            }
        }
    }

    fn finish(&mut self) {
        if !self.ch.opened {
            return;
        }
        if self.ch.eof().is_ok() {
            self.ch.close();
        }
        // Let the server's CLOSE arrive so the next channel starts clean.
        let until = now_ms() + 5000;
        while !self.ch.close_received && now_ms() < until {
            if self.ch.poll(100).is_err() {
                break;
            }
        }
    }
}

pub fn upload(
    s: &mut Session<'_>,
    src: &mut Source<'_>,
    remote: &str,
    cancel: Option<&Cancel>,
) -> Result<()> {
    sys::check_cancel(cancel)?;
    let ch = Channel::open(s, true, false, MAX_PACKET * 2 + LOCAL_WINDOW as usize)?;
    let mut f = Sftp { ch, packet: Vec::new(), next_id: 0 };
    let result = upload_on(&mut f, src, remote, cancel);
    f.finish();
    result
}

fn upload_on(
    f: &mut Sftp<'_, '_>,
    src: &mut Source<'_>,
    remote: &str,
    cancel: Option<&Cancel>,
) -> Result<()> {
    let deadline = now_ms() + UPLOAD_TIMEOUT_MS;
    let mut req = Vec::new();
    req.put_string(b"sftp");
    let accepted = f.ch.request("subsystem", true, &req, SUBSYSTEM_TIMEOUT_MS).map_err(|e| {
        if e.message().starts_with("the SSH server did not answer") {
            Error::new("SFTP subsystem request timed out")
        } else {
            e
        }
    })?;
    if !accepted {
        return Err(Error::new("the SSH server offers no SFTP subsystem"));
    }
    let mut init = vec![INIT];
    init.put_u32(3);
    f.send(&init, deadline, cancel)?;
    f.reply(deadline, cancel)?;
    if f.packet.len() < 5
        || f.packet[0] != VERSION
        || u32::from_be_bytes([f.packet[1], f.packet[2], f.packet[3], f.packet[4]]) < 3
    {
        return Err(Error::new("the SFTP server does not speak version 3"));
    }
    let open_id = f.next_id;
    f.next_id = f.next_id.wrapping_add(1);
    let mut b = header(OPEN, open_id);
    b.put_string(remote.as_bytes());
    b.put_u32(FLAG_WRITE | FLAG_CREAT | FLAG_TRUNC);
    b.put_u32(0); // no attributes
    f.send(&b, deadline, cancel)?;
    f.reply(deadline, cancel)?;
    let handle = {
        let mut r = Cursor::new(&f.packet);
        let parsed = (wire::u8(&mut r), wire::u32(&mut r), wire::string(&mut r));
        match parsed {
            (Some(HANDLE), Some(id), Some(h))
                if id == open_id && !h.is_empty() && h.len() <= 256 =>
            {
                h.to_vec()
            }
            _ => {
                f.status_ok(Some(open_id), "open")?;
                return Err(Error::new("the SFTP server returned no file handle"));
            }
        }
    };
    // Outstanding writes, by id modulo the window. Replies to a single
    // handle normally arrive in order, but any outstanding id is accepted.
    let mut pending = [false; IN_FLIGHT as usize];
    let mut in_flight = 0_u32;
    let mut offset = 0_u64;
    let mut chunk = vec![0_u8; CHUNK];
    let mut body: Vec<u8> = Vec::with_capacity(CHUNK + 1024);
    let mut at_end = false;
    while !at_end || in_flight > 0 {
        if sys::cancelled(cancel) {
            f.abandon(&handle, remote);
            return Err(sys::cancelled_error());
        }
        if !at_end && in_flight < IN_FLIGHT {
            let n = src.read(&mut chunk)?;
            if n == 0 {
                at_end = true;
                continue;
            }
            let id = f.next_id;
            f.next_id = f.next_id.wrapping_add(1);
            body.clear();
            body.put_u8(WRITE);
            body.put_u32(id);
            let b = &mut body;
            b.put_string(&handle);
            b.put_u64(offset);
            b.put_string(&chunk[..n]);
            offset += n as u64;
            pending[(id % IN_FLIGHT) as usize] = true;
            f.send(&body, deadline, cancel)?;
            in_flight += 1;
            continue;
        }
        let answered = f.reply(deadline, cancel).and_then(|()| f.status_ok(None, "write"));
        let id = match answered {
            Ok(id) => id,
            Err(e) => {
                if sys::cancelled(cancel) {
                    f.abandon(&handle, remote);
                }
                return Err(e);
            }
        };
        if f.next_id.wrapping_sub(id) > IN_FLIGHT
            || f.next_id == id
            || !pending[(id % IN_FLIGHT) as usize]
        {
            return Err(Error::new("the SFTP server answered a request that was not made"));
        }
        pending[(id % IN_FLIGHT) as usize] = false;
        in_flight -= 1;
    }
    let close_id = f.next_id;
    f.next_id = f.next_id.wrapping_add(1);
    let mut b = header(CLOSE, close_id);
    b.put_string(&handle);
    f.send(&b, deadline, cancel)?;
    f.reply(deadline, cancel)?;
    f.status_ok(Some(close_id), "close")?;
    Ok(())
}
