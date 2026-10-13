//! The SSH transport (RFC 4253): identification exchange, the binary packet
//! protocol, and the transport messages a session never needs to see.

use omt_common::buf::{Cursor, wipe_vec};
use omt_common::{Error, Result};
use omt_sys::evp::safe::random_bytes;

use super::cipher::PacketCipher;
use super::keys::HOSTKEY_LIST;
use super::wire::{self, Put, namelist_has};
use super::{MAX_PACKET, msg};
use crate::sys::{self, Cancel, Received, Sock, now_ms};

const IDENT: &str = "SSH-2.0-rpi_omt_deploy";
const MAX_PREFACE_LINES: usize = 64;
const MAX_LINE: usize = 255;
pub const CONNECT_TIMEOUT_MS: u64 = 15_000;
pub const IDLE_TIMEOUT_MS: u64 = 60_000;

/// What a wait for a packet ended with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recv {
    Packet,
    Timeout,
}

#[derive(Debug)]
pub struct Transport<'c> {
    sock: Option<Sock>,
    pub(crate) cancel: Option<&'c Cancel>,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) known_hosts: String,

    /// Identification strings, without CR LF.
    pub(crate) v_c: Vec<u8>,
    pub(crate) v_s: Vec<u8>,
    /// KEXINIT payloads.
    pub(crate) i_c: Vec<u8>,
    pub(crate) i_s: Vec<u8>,
    pub(crate) session_id: Vec<u8>,
    /// The host key blob verified at the first exchange.
    pub(crate) host_key: Vec<u8>,
    pub(crate) preferred_hostkeys: String,

    pub(crate) send_seq: u32,
    pub(crate) recv_seq: u32,
    pub(crate) send: PacketCipher,
    pub(crate) recv: PacketCipher,
    pub(crate) strict_kex: bool,
    pub(crate) first_kex_done: bool,
    pub(crate) server_sig_algs: Vec<u8>,

    /// Bytes received and not yet consumed.
    rbuf: Vec<u8>,
    /// The current packet's length, once known.
    pkt_len: Option<u32>,
    /// The most recent packet's payload.
    pub payload: Vec<u8>,
    sbuf: Vec<u8>,

    /// How long a read may wait for the peer.
    pub idle_ms: u64,
}

impl Drop for Transport<'_> {
    fn drop(&mut self) {
        wipe_vec(&mut self.session_id);
        wipe_vec(&mut self.rbuf);
        wipe_vec(&mut self.payload);
        wipe_vec(&mut self.sbuf);
    }
}

fn describe_disconnect(payload: &[u8]) -> Error {
    let mut r = Cursor::new(payload.get(1..).unwrap_or_default());
    let reason = wire::u32(&mut r);
    let text = reason.and_then(|_| wire::string(&mut r)).unwrap_or_default();
    let safe: String = text
        .iter()
        .take(512)
        .map(|&c| if (0x20..0x7f).contains(&c) { char::from(c) } else { '?' })
        .collect();
    Error::new(format!("the SSH server disconnected (reason {}): {safe}", reason.unwrap_or(0)))
}

impl<'c> Transport<'c> {
    pub fn new(cancel: Option<&'c Cancel>) -> Self {
        Self {
            sock: None,
            cancel,
            host: String::new(),
            port: 22,
            known_hosts: String::new(),
            v_c: Vec::new(),
            v_s: Vec::new(),
            i_c: Vec::new(),
            i_s: Vec::new(),
            session_id: Vec::new(),
            host_key: Vec::new(),
            preferred_hostkeys: String::new(),
            send_seq: 0,
            recv_seq: 0,
            send: PacketCipher::default(),
            recv: PacketCipher::default(),
            strict_kex: false,
            first_kex_done: false,
            server_sig_algs: Vec::new(),
            rbuf: Vec::new(),
            pkt_len: None,
            payload: Vec::new(),
            sbuf: Vec::new(),
            idle_ms: IDLE_TIMEOUT_MS,
        }
    }

    /// The payload's message number.
    pub fn msg_type(&self) -> u8 {
        self.payload.first().copied().unwrap_or(0)
    }

    /// The payload after its message number.
    pub fn body(&self) -> &[u8] {
        self.payload.get(1..).unwrap_or_default()
    }

    fn sock(&mut self) -> Result<&mut Sock> {
        self.sock.as_mut().ok_or_else(|| Error::new("the SSH connection is closed"))
    }

    /// Reads more bytes into the receive buffer, waiting until the deadline
    /// at most; false when the deadline passed.
    fn fill(&mut self, deadline_ms: u64) -> Result<bool> {
        let mut chunk = [0_u8; 16384];
        loop {
            sys::check_cancel(self.cancel)?;
            let now = now_ms();
            if now >= deadline_ms {
                return Ok(false);
            }
            let wait = (deadline_ms - now).min(100);
            match self.sock()?.recv(&mut chunk, wait)? {
                Received::Timeout => {}
                Received::Eof => return Err(Error::new("the SSH server closed the connection")),
                Received::Data(n) => {
                    if self.rbuf.len() + n > 2 * MAX_PACKET + 4096 {
                        return Err(Error::new("SSH receive buffer overflow"));
                    }
                    self.rbuf.extend_from_slice(&chunk[..n]);
                    return Ok(true);
                }
            }
        }
    }

    fn send_raw(&mut self, data: &[u8]) -> Result<()> {
        let idle = self.idle_ms;
        self.sock()?.send(data, idle)
    }

    fn exchange_identification(&mut self, deadline: u64) -> Result<()> {
        self.v_c = IDENT.as_bytes().to_vec();
        self.send_raw(format!("{IDENT}\r\n").as_bytes())?;
        let overlong = || Error::new("the SSH server sent an overlong identification line");
        let mut lines = 0;
        while lines < MAX_PREFACE_LINES {
            let Some(n) = self.rbuf.iter().position(|&b| b == b'\n') else {
                if self.rbuf.len() > MAX_LINE {
                    return Err(overlong());
                }
                if !self.fill(deadline)? {
                    return Err(Error::new("SSH connection timed out"));
                }
                continue;
            };
            let text = if n > 0 && self.rbuf[n - 1] == b'\r' { n - 1 } else { n };
            if n + 1 > MAX_LINE {
                return Err(overlong());
            }
            let line = &self.rbuf[..text];
            if line.starts_with(b"SSH-") {
                if !line.starts_with(b"SSH-2.0-") && !line.starts_with(b"SSH-1.99-") {
                    return Err(Error::new("the server does not speak SSH protocol 2"));
                }
                if line.iter().any(|&c| !(0x20..=0x7e).contains(&c)) {
                    return Err(Error::new("the SSH server sent an invalid identification string"));
                }
                self.v_s = line.to_vec();
                self.rbuf.drain(..=n);
                return Ok(());
            }
            self.rbuf.drain(..=n);
            lines += 1;
        }
        Err(Error::new("the SSH server sent no identification string"))
    }

    /// Sends one packet with the given payload.
    pub fn send(&mut self, payload: &[u8]) -> Result<()> {
        if payload.is_empty() || payload.len() > MAX_PACKET - 64 {
            return Err(Error::new("SSH packet is too large or could not be built"));
        }
        let block = self.send.block();
        let aad = if self.send.aead() { 4 } else { 0 };
        let unpadded = 4 + 1 + payload.len() - aad;
        let mut padding = block - unpadded % block;
        if padding < 4 {
            padding += block;
        }
        let packet_len = 1 + payload.len() + padding;
        let total = 4 + packet_len;
        let tag = self.send.tag_len();
        let mut b = std::mem::take(&mut self.sbuf);
        b.clear();
        b.reserve(total + tag);
        b.put_u32(u32::try_from(packet_len).unwrap_or(u32::MAX));
        b.put_u8(u8::try_from(padding).unwrap_or(u8::MAX));
        b.extend_from_slice(payload);
        let mut pad = [0_u8; 64];
        if !random_bytes(&mut pad[..padding]) {
            self.sbuf = b;
            return Err(Error::new("random padding is unavailable"));
        }
        b.extend_from_slice(&pad[..padding]);
        let mut mac = [0_u8; 64];
        if !self.send.seal(self.send_seq, &mut b[..total], &mut mac) {
            self.sbuf = b;
            return Err(Error::new("SSH packet encryption failed"));
        }
        b.extend_from_slice(&mac[..tag]);
        self.send_seq = self.send_seq.wrapping_add(1);
        let result = self.send_raw(&b);
        b.clear();
        self.sbuf = b;
        result
    }

    /// The next packet as received, with no transport handling, by an
    /// absolute deadline. The payload is in `self.payload`.
    pub fn read_packet(&mut self, deadline_ms: u64) -> Result<Recv> {
        loop {
            if self.pkt_len.is_none() && self.rbuf.len() >= self.recv.first_len() {
                let len = self
                    .recv
                    .length(self.recv_seq, &mut self.rbuf)
                    .ok_or_else(|| Error::new("SSH packet decryption failed"))?;
                let block = self.recv.block();
                let aligned = if self.recv.aead() { len as usize } else { len as usize + 4 };
                if len < 5 || len as usize > MAX_PACKET || aligned % block != 0 {
                    return Err(Error::new("the SSH server sent a malformed packet"));
                }
                self.pkt_len = Some(len);
            }
            if let Some(len) = self.pkt_len {
                let tag = self.recv.tag_len();
                let total = 4 + len as usize;
                if self.rbuf.len() >= total + tag {
                    let (packet, rest) = self.rbuf.split_at_mut(total);
                    if !self.recv.open(self.recv_seq, packet, &rest[..tag]) {
                        return Err(Error::new("SSH packet authentication failed"));
                    }
                    let padding = usize::from(packet[4]);
                    if padding < 4 || padding + 1 >= len as usize {
                        return Err(Error::new("the SSH server sent a malformed packet"));
                    }
                    let payload_len = len as usize - 1 - padding;
                    self.payload.clear();
                    self.payload.extend_from_slice(&packet[5..5 + payload_len]);
                    omt_sys::openssl::safe::cleanse(&mut self.rbuf[..total + tag]);
                    self.rbuf.drain(..total + tag);
                    self.pkt_len = None;
                    self.recv_seq = self.recv_seq.wrapping_add(1);
                    return Ok(Recv::Packet);
                }
            }
            if !self.fill(deadline_ms)? {
                return Ok(Recv::Timeout);
            }
        }
    }

    /// Records the server's EXT_INFO (RFC 8308).
    pub fn handle_ext_info(&mut self) -> Result<()> {
        let bad = || Error::new("the SSH server sent a malformed EXT_INFO");
        let mut r = Cursor::new(self.body());
        let count = wire::u32(&mut r).filter(|&c| c <= 1024).ok_or_else(bad)?;
        let mut sig_algs = None;
        for _ in 0..count {
            let name = wire::string(&mut r).ok_or_else(bad)?;
            let value = wire::string(&mut r).ok_or_else(bad)?;
            if name == b"server-sig-algs" {
                sig_algs = Some(value.to_vec());
            }
        }
        if let Some(v) = sig_algs {
            self.server_sig_algs = v;
        }
        Ok(())
    }

    /// Waits up to `wait_ms` for the next packet the caller should see.
    /// Transport messages (ignore, debug, a re-key, a global request) are
    /// handled here.
    pub fn recv(&mut self, wait_ms: u64) -> Result<Recv> {
        let deadline = now_ms() + wait_ms;
        loop {
            if self.read_packet(deadline)? == Recv::Timeout {
                return Ok(Recv::Timeout);
            }
            match self.msg_type() {
                msg::DISCONNECT => return Err(describe_disconnect(&self.payload)),
                msg::IGNORE | msg::DEBUG => {}
                msg::UNIMPLEMENTED => {
                    return Err(Error::new("the SSH server rejected a message as unimplemented"));
                }
                msg::EXT_INFO => self.handle_ext_info()?,
                // A server-initiated re-key, handled inline before anything
                // else is sent.
                msg::KEXINIT => super::kex::run(self, true)?,
                msg::GLOBAL_REQUEST => {
                    let mut r = Cursor::new(self.body());
                    let want_reply = wire::string(&mut r).and_then(|_| wire::boolean(&mut r));
                    match want_reply {
                        None => {
                            return Err(Error::new(
                                "the SSH server sent a malformed global request",
                            ));
                        }
                        Some(true) => self.send(&[msg::REQUEST_FAILURE])?,
                        Some(false) => {}
                    }
                }
                _ => return Ok(Recv::Packet),
            }
        }
    }

    /// Like `recv`, but waits out the idle timeout and fails on silence.
    pub fn recv_wait(&mut self) -> Result<()> {
        match self.recv(self.idle_ms)? {
            Recv::Packet => Ok(()),
            Recv::Timeout => Err(Error::new("the SSH server stopped responding")),
        }
    }

    pub fn disconnect(&mut self, reason: u32, text: &str) {
        if self.sock.is_none() || !self.recv.active() {
            return;
        }
        let mut m = vec![msg::DISCONNECT];
        m.put_u32(reason);
        m.put_string(text.as_bytes());
        m.put_string(b"");
        self.idle_ms = 2000;
        let _ = self.send(&m);
    }

    /// Host-key algorithms in preference order, with those whose key type
    /// known_hosts already lists for this host moved to the front: offering
    /// a type the operator never verified would only end in a refusal.
    fn order_hostkeys(&mut self, known_types: &str) {
        let mut ordered: Vec<&str> = Vec::new();
        for pass in 0..2 {
            for alg in HOSTKEY_LIST.split(',') {
                let key_type =
                    if alg == "rsa-sha2-512" || alg == "rsa-sha2-256" { "ssh-rsa" } else { alg };
                if (pass == 0) == namelist_has(known_types.as_bytes(), key_type) {
                    ordered.push(alg);
                }
            }
        }
        self.preferred_hostkeys = ordered.join(",");
    }

    /// TCP connect, version exchange, and the first key exchange with
    /// host-key verification, all within the connect timeout.
    pub fn open(&mut self, host: &str, port: u16, known_hosts: &str) -> Result<()> {
        host.clone_into(&mut self.host);
        self.port = port;
        known_hosts.clone_into(&mut self.known_hosts);
        let types = super::known_hosts::types(known_hosts, host, port)?;
        self.order_hostkeys(&types);
        let deadline = now_ms() + CONNECT_TIMEOUT_MS;
        self.sock = Some(Sock::connect(host, port, CONNECT_TIMEOUT_MS)?);
        // The handshake shares the connect budget.
        let saved_idle = self.idle_ms;
        self.idle_ms = CONNECT_TIMEOUT_MS;
        let result =
            self.exchange_identification(deadline).and_then(|()| super::kex::run(self, false));
        self.idle_ms = saved_idle;
        result?;
        if now_ms() > deadline {
            return Err(Error::new("SSH connection timed out"));
        }
        Ok(())
    }

    pub fn close(&mut self) {
        if let Some(s) = self.sock.take() {
            s.shutdown();
        }
    }
}
