//! A small HTTPS/1.1 server: one thread, one poll() loop, non-blocking
//! OpenSSL connections, and a synchronous handler called once a request is
//! complete.
//!
//! Every connection is bounded in time and number: a client gets the header
//! timeout to send its headers, the body timeout to send its body, and an
//! idle keep-alive connection is closed after the idle timeout, so a slow or
//! silent client cannot hold a slot forever.

use std::ffi::CString;
use std::fmt::Write as _;
use std::net::{IpAddr, SocketAddr, TcpListener};
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};

use omt_common::timefmt::{civil_from_days, now_ms, wall_clock};
use omt_common::{Error, Result};
use omt_sys::posix::{POLLIN, POLLOUT, poll_fds, pollfd};
use omt_sys::ssl::safe::{Context, Io, Stream, clear_errors, take_error};
use omt_sys::ssl::{
    SSL_MODE_ACCEPT_MOVING_WRITE_BUFFER, SSL_MODE_ENABLE_PARTIAL_WRITE,
    SSL_OP_CIPHER_SERVER_PREFERENCE, SSL_OP_NO_COMPRESSION, SSL_OP_NO_RENEGOTIATION,
    SSL_OP_NO_TICKET, TLS1_2_VERSION,
};

pub const MAX_CONNECTIONS: usize = 64;
pub const MAX_HEADER_BYTES: usize = 16 * 1024;
pub const MAX_HEADERS: usize = 64;
pub const HEADER_TIMEOUT_MS: u64 = 15_000;
pub const BODY_TIMEOUT_MS: u64 = 30_000;
pub const IDLE_TIMEOUT_MS: u64 = 60_000;
pub const WRITE_TIMEOUT_MS: u64 = 120_000;
const METHOD_MAX: usize = 15;
const PATH_MAX: usize = 2047;

pub fn reason(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        200 => "OK",
        202 => "Accepted",
        303 => "See Other",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        411 => "Length Required",
        413 => "Payload Too Large",
        414 => "URI Too Long",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        505 => "HTTP Version Not Supported",
        _ => "Unknown",
    }
}

/// A parsed request head. Header names and values are byte ranges into the
/// buffer the head was parsed from.
#[derive(Clone, Debug, Default)]
pub struct Head {
    pub method: String,
    pub path: String,
    pub headers: Vec<(std::ops::Range<usize>, std::ops::Range<usize>)>,
    /// The header block's size, through the blank line.
    pub head_len: usize,
    pub content_length: usize,
    pub keep_alive: bool,
}

#[derive(Clone, Debug)]
pub enum Parse {
    /// More bytes are needed.
    Incomplete,
    Complete(Head),
    /// A request that must be refused with this status.
    Refuse(u16),
}

fn is_tchar(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c)
}

fn trim_ows(s: &[u8]) -> &[u8] {
    let start = s.iter().position(|&c| c != b' ' && c != b'\t').unwrap_or(s.len());
    let end = s.iter().rposition(|&c| c != b' ' && c != b'\t').map_or(start, |i| i + 1);
    &s[start..end.max(start)]
}

/// Parses one request head.
pub fn parse_head(data: &[u8]) -> Parse {
    let window = &data[..data.len().min(MAX_HEADER_BYTES)];
    let Some(end) = window.windows(4).position(|w| w == b"\r\n\r\n") else {
        return if data.len() >= MAX_HEADER_BYTES { Parse::Refuse(431) } else { Parse::Incomplete };
    };
    let head_len = end + 4;
    // Everything up to (not including) the final CRLF CRLF.
    let block = &data[..end];
    // Request line.
    let Some(sp1) = block.iter().position(|&c| c == b' ') else { return Parse::Refuse(400) };
    if sp1 == 0 || sp1 > METHOD_MAX || !block[..sp1].iter().all(|&c| is_tchar(c)) {
        return Parse::Refuse(400);
    }
    let line_end = block[sp1..].iter().position(|&c| c == b'\r').map_or(block.len(), |i| sp1 + i);
    if line_end < block.len() && data.get(line_end + 1) != Some(&b'\n') {
        return Parse::Refuse(400);
    }
    let line = &block[..line_end];
    let Some(sp2) = line[sp1 + 1..].iter().position(|&c| c == b' ').map(|i| sp1 + 1 + i) else {
        return Parse::Refuse(400);
    };
    let target = &line[sp1 + 1..sp2];
    if target.first() != Some(&b'/') || target.iter().any(|&c| c <= 0x20 || c >= 0x7F) {
        return Parse::Refuse(400);
    }
    let path = &target[..target.iter().position(|&c| c == b'?').unwrap_or(target.len())];
    if path.len() > PATH_MAX {
        return Parse::Refuse(414);
    }
    let http11 = match &line[sp2 + 1..] {
        b"HTTP/1.1" => true,
        b"HTTP/1.0" => false,
        _ => return Parse::Refuse(505),
    };
    let mut head = Head {
        method: String::from_utf8_lossy(&block[..sp1]).into_owned(),
        path: String::from_utf8_lossy(path).into_owned(),
        head_len,
        ..Head::default()
    };
    // Header fields.
    let (mut have_length, mut close, mut keep) = (false, false, false);
    let mut pos = line_end + 2;
    while pos < end {
        let eol = block[pos..].iter().position(|&c| c == b'\r').map_or(end, |i| pos + i);
        if eol < end && data.get(eol + 1) != Some(&b'\n') {
            return Parse::Refuse(400);
        }
        let field = &block[pos..eol];
        if matches!(field.first(), Some(b' ' | b'\t')) {
            return Parse::Refuse(400); // obsolete line folding
        }
        let Some(colon) = field.iter().position(|&c| c == b':').filter(|&c| c > 0) else {
            return Parse::Refuse(400);
        };
        let name = &field[..colon];
        if !name.iter().all(|&c| is_tchar(c)) {
            return Parse::Refuse(400);
        }
        let value = trim_ows(&field[colon + 1..]);
        if value.iter().any(|&c| (c < 0x20 && c != b'\t') || c == 0x7F) {
            return Parse::Refuse(400);
        }
        if head.headers.len() == MAX_HEADERS {
            return Parse::Refuse(431);
        }
        let value_start = value.as_ptr() as usize - data.as_ptr() as usize;
        head.headers.push((pos..pos + colon, value_start..value_start + value.len()));
        if name.eq_ignore_ascii_case(b"content-length") {
            let Some(n) = omt_common::buf::parse_u64(value, u64::from(u32::MAX))
                .and_then(|n| usize::try_from(n).ok())
            else {
                return Parse::Refuse(400);
            };
            if have_length && n != head.content_length {
                return Parse::Refuse(400);
            }
            head.content_length = n;
            have_length = true;
        } else if name.eq_ignore_ascii_case(b"transfer-encoding") {
            return Parse::Refuse(411);
        } else if name.eq_ignore_ascii_case(b"connection") {
            // A comma-separated token list.
            for token in value.split(|&c| c == b',').map(trim_ows) {
                close |= token.eq_ignore_ascii_case(b"close");
                keep |= token.eq_ignore_ascii_case(b"keep-alive");
            }
        }
        pos = eol + 2;
    }
    head.keep_alive = !close && (http11 || keep);
    Parse::Complete(head)
}

/// A complete request, as the handler sees it.
#[derive(Debug)]
pub struct Request<'a> {
    pub method: &'a str,
    pub path: &'a str,
    data: &'a [u8],
    headers: &'a [(std::ops::Range<usize>, std::ops::Range<usize>)],
    pub body: &'a [u8],
    /// The client address, for rate limiting.
    pub peer: &'a str,
}

impl<'a> Request<'a> {
    /// The first header named `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&'a [u8]> {
        self.headers
            .iter()
            .find(|(n, _)| self.data[n.clone()].eq_ignore_ascii_case(name.as_bytes()))
            .map(|(_, v)| &self.data[v.clone()])
    }

    /// A request built from a parsed head over `data` (exposed for tests).
    pub fn new(head: &'a Head, data: &'a [u8], peer: &'a str) -> Self {
        let body_end = (head.head_len + head.content_length).min(data.len());
        Self {
            method: &head.method,
            path: &head.path,
            data,
            headers: &head.headers,
            body: &data[head.head_len.min(body_end)..body_end],
            peer,
        }
    }
}

#[derive(Debug, Default)]
pub struct Response {
    pub status: u16,
    /// "name: value\r\n" lines.
    headers: String,
    pub body: Vec<u8>,
    /// Send headers only.
    pub head: bool,
    failed: bool,
}

impl Response {
    pub fn add_header(&mut self, name: &str, value: &str) {
        // A header value never carries a line break; refuse rather than
        // split.
        if name.contains(['\r', '\n', ':']) || value.contains(['\r', '\n']) {
            self.failed = true;
            return;
        }
        let _ = write!(self.headers, "{name}: {value}\r\n");
    }

    pub fn headers(&self) -> &str {
        &self.headers
    }
}

pub trait Handler {
    fn handle(&mut self, req: &Request<'_>, res: &mut Response);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Handshake,
    Read,
    Write,
    Shutdown,
}

#[derive(Debug)]
struct Conn {
    stream: Stream,
    state: State,
    want: i16,
    deadline: u64,
    input: Vec<u8>,
    input_limit: usize,
    out: Vec<u8>,
    out_pos: usize,
    head: Option<Head>,
    close_after: bool,
    sent_continue: bool,
    peer: String,
}

fn http_date() -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] =
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (secs, _) = wall_clock();
    let day_number = secs.div_euclid(86400);
    let rest = secs.rem_euclid(86400);
    let (year, month, day) = civil_from_days(day_number);
    format!(
        "{}, {day:02} {} {year:04} {:02}:{:02}:{:02} GMT",
        DAYS[usize::try_from(day_number.rem_euclid(7)).unwrap_or(0)],
        MONTHS[usize::try_from(month).unwrap_or(1).saturating_sub(1).min(11)],
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

impl Conn {
    /// Serialises a response into the output buffer.
    fn queue(&mut self, mut res: Response) {
        if res.failed {
            res.status = 500;
            res.headers.clear();
            res.body.clear();
        }
        self.out.clear();
        self.out_pos = 0;
        let _ = write!(
            ByteWriter(&mut self.out),
            "HTTP/1.1 {} {}\r\n{}content-length: {}\r\ndate: {}\r\n",
            res.status,
            reason(res.status),
            res.headers,
            res.body.len(),
            http_date()
        );
        if self.close_after {
            self.out.extend_from_slice(b"connection: close\r\n");
        }
        self.out.extend_from_slice(b"\r\n");
        if !res.head {
            self.out.extend_from_slice(&res.body);
        }
        self.state = State::Write;
        self.deadline = now_ms() + WRITE_TIMEOUT_MS;
    }

    /// A protocol-level refusal: no handler involved, and the connection
    /// closes.
    fn refuse(&mut self, status: u16) {
        self.close_after = true;
        self.queue(Response { status, ..Response::default() });
    }

    fn after_write(&mut self) {
        if self.close_after {
            self.state = State::Shutdown;
            return;
        }
        // Keep the connection, and any pipelined bytes after this request.
        if let Some(head) = self.head.take() {
            let used = (head.head_len + head.content_length).min(self.input.len());
            self.input.drain(..used);
        }
        self.sent_continue = false;
        self.state = State::Read;
        self.deadline =
            now_ms() + if self.input.is_empty() { IDLE_TIMEOUT_MS } else { HEADER_TIMEOUT_MS };
    }
}

struct ByteWriter<'a>(&'a mut Vec<u8>);

impl std::fmt::Write for ByteWriter<'_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0.extend_from_slice(s.as_bytes());
        Ok(())
    }
}

#[derive(Debug)]
pub struct Server {
    listener: TcpListener,
    ctx: Context,
    max_body: usize,
    conns: Vec<Option<Conn>>,
}

fn cstring(s: &str) -> Result<CString> {
    CString::new(s).map_err(|_| Error::new("path contains a NUL byte"))
}

impl Server {
    /// Loads the PEM certificate chain and key and binds 0.0.0.0:port.
    pub fn open(port: u16, cert_file: &str, key_file: &str, max_body: usize) -> Result<Self> {
        let tls_error = |what: &str| Error::new(format!("{what}: {}", take_error()));
        let ctx =
            Context::new_server().ok_or_else(|| tls_error("unable to create the TLS context"))?;
        // TLS 1.2 and 1.3 with forward-secret AEAD suites only.
        ctx.set_min_proto_version(TLS1_2_VERSION);
        ctx.set_options(
            SSL_OP_NO_COMPRESSION
                | SSL_OP_NO_RENEGOTIATION
                | SSL_OP_CIPHER_SERVER_PREFERENCE
                | SSL_OP_NO_TICKET,
        );
        ctx.set_mode(SSL_MODE_ENABLE_PARTIAL_WRITE | SSL_MODE_ACCEPT_MOVING_WRITE_BUFFER);
        if !ctx.set_cipher_list(
            c"ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-ECDSA-AES128-GCM-SHA256:\
              ECDHE-ECDSA-CHACHA20-POLY1305:ECDHE-RSA-AES256-GCM-SHA384:\
              ECDHE-RSA-AES128-GCM-SHA256:ECDHE-RSA-CHACHA20-POLY1305",
        ) || !ctx.set_ciphersuites(
            c"TLS_AES_256_GCM_SHA384:TLS_AES_128_GCM_SHA256:TLS_CHACHA20_POLY1305_SHA256",
        ) || !ctx.set_groups_list(c"X25519:P-256:P-384")
        {
            return Err(tls_error("unable to configure TLS"));
        }
        ctx.select_alpn_http11();
        if !ctx.use_certificate_chain_file(&cstring(cert_file)?)
            || !ctx.use_private_key_file(&cstring(key_file)?)
            || !ctx.check_private_key()
        {
            return Err(tls_error("unable to load TLS certificate"));
        }
        let listener = TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], port)))?;
        listener.set_nonblocking(true)?;
        let mut conns = Vec::with_capacity(MAX_CONNECTIONS);
        conns.resize_with(MAX_CONNECTIONS, || None);
        Ok(Self { listener, ctx, max_body, conns })
    }

    /// Serves until a fatal error or `stop` becomes true.
    pub fn run(&mut self, handler: &mut dyn Handler, stop: &AtomicBool) -> Result<()> {
        let mut fds: Vec<pollfd> = Vec::with_capacity(MAX_CONNECTIONS + 1);
        let mut index: Vec<usize> = Vec::with_capacity(MAX_CONNECTIONS + 1);
        while !stop.load(Ordering::Relaxed) {
            fds.clear();
            index.clear();
            fds.push(pollfd { fd: self.listener.as_raw_fd(), events: POLLIN, revents: 0 });
            index.push(usize::MAX);
            let now = now_ms();
            let mut wait = 1000_u64;
            for (i, slot) in self.conns.iter_mut().enumerate() {
                let Some(c) = slot else { continue };
                if c.deadline <= now {
                    *slot = None;
                    continue;
                }
                wait = wait.min(c.deadline - now);
                index.push(i);
                fds.push(pollfd { fd: c.stream.socket().as_raw_fd(), events: c.want, revents: 0 });
            }
            let ready = poll_fds(&mut fds, i32::try_from(wait).unwrap_or(1000))?;
            if ready == 0 {
                continue;
            }
            for (k, fd) in fds.iter().enumerate().skip(1) {
                if fd.revents == 0 {
                    continue;
                }
                let slot = &mut self.conns[index[k]];
                if let Some(c) = slot
                    && !drive(c, self.max_body, handler)
                {
                    *slot = None;
                }
            }
            if fds[0].revents & POLLIN != 0 {
                for _ in 0..16 {
                    if !self.accept_one(handler) {
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    /// Accepts one connection; true when it was kept, so a burst stops at
    /// the first accept that adds nothing.
    fn accept_one(&mut self, handler: &mut dyn Handler) -> bool {
        let Ok((socket, peer)) = self.listener.accept() else { return false };
        let Some(slot) = self.conns.iter().position(Option::is_none) else {
            // Over the connection ceiling: refused at the door.
            return false;
        };
        if socket.set_nonblocking(true).is_err() {
            return false;
        }
        let _ = socket.set_nodelay(true);
        let Some(stream) = self.ctx.accept(socket) else { return false };
        let peer = match peer.ip() {
            IpAddr::V4(a) => a.to_string(),
            IpAddr::V6(_) => "unknown".to_owned(),
        };
        let mut c = Conn {
            stream,
            state: State::Handshake,
            want: POLLIN,
            deadline: now_ms() + HEADER_TIMEOUT_MS,
            input: Vec::new(),
            input_limit: MAX_HEADER_BYTES + self.max_body + 16384,
            out: Vec::new(),
            out_pos: 0,
            head: None,
            close_after: false,
            sent_continue: false,
            peer,
        };
        if !drive(&mut c, self.max_body, handler) {
            return false;
        }
        self.conns[slot] = Some(c);
        true
    }
}

fn dispatch(c: &mut Conn, handler: &mut dyn Handler) {
    let Some(head) = c.head.as_ref() else { return };
    let req = Request::new(head, &c.input, &c.peer);
    let mut res = Response { status: 200, head: head.method == "HEAD", ..Response::default() };
    handler.handle(&req, &mut res);
    c.close_after = !head.keep_alive;
    c.queue(res);
}

/// Tries to parse what has arrived; dispatches a complete request.
fn process_input(c: &mut Conn, max_body: usize, handler: &mut dyn Handler) {
    if c.head.is_none() {
        let head = match parse_head(&c.input) {
            Parse::Incomplete => return,
            Parse::Refuse(status) => {
                c.refuse(status);
                return;
            }
            Parse::Complete(head) => head,
        };
        if head.content_length > max_body {
            c.head = Some(head);
            c.refuse(413);
            return;
        }
        c.deadline = now_ms() + BODY_TIMEOUT_MS;
        let req = Request::new(&head, &c.input, &c.peer);
        let expects_continue = req
            .header("expect")
            .is_some_and(|v| v.len() < 32 && v.eq_ignore_ascii_case(b"100-continue"));
        if head.content_length > 0
            && !c.sent_continue
            && expects_continue
            && c.input.len() < head.head_len + head.content_length
        {
            // Interim response, written directly: it is small and the
            // socket has just delivered data, so a short write is not
            // expected; a failure only costs the client its 1 s wait.
            let _ = c.stream.write(b"HTTP/1.1 100 Continue\r\n\r\n");
            c.sent_continue = true;
        }
        c.head = Some(head);
    }
    let Some(head) = &c.head else { return };
    if c.input.len() - head.head_len < head.content_length {
        return;
    }
    dispatch(c, handler);
}

/// Drives one connection as far as it will go without blocking. Returns
/// false when the connection is finished and should be dropped.
fn drive(c: &mut Conn, max_body: usize, handler: &mut dyn Handler) -> bool {
    loop {
        clear_errors();
        match c.state {
            State::Handshake => match c.stream.handshake() {
                Io::Done(_) => {
                    c.state = State::Read;
                    c.deadline = now_ms() + HEADER_TIMEOUT_MS;
                }
                Io::WantRead => {
                    c.want = POLLIN;
                    return true;
                }
                Io::WantWrite => {
                    c.want = POLLOUT;
                    return true;
                }
                Io::Closed => return false,
            },
            State::Read => {
                let mut chunk = [0_u8; 16384];
                match c.stream.read(&mut chunk) {
                    Io::Done(n) => {
                        if c.input.is_empty() && c.head.is_none() {
                            c.deadline = now_ms() + HEADER_TIMEOUT_MS;
                        }
                        if c.input.len() + n > c.input_limit {
                            c.refuse(413);
                            continue;
                        }
                        c.input.extend_from_slice(&chunk[..n]);
                        process_input(c, max_body, handler);
                    }
                    Io::WantRead => {
                        c.want = POLLIN;
                        return true;
                    }
                    Io::WantWrite => {
                        c.want = POLLOUT;
                        return true;
                    }
                    // Closed by the peer or failed.
                    Io::Closed => return false,
                }
            }
            State::Write => {
                if c.out_pos == c.out.len() {
                    c.after_write();
                    if c.state == State::Read && !c.input.is_empty() {
                        process_input(c, max_body, handler);
                    }
                    continue;
                }
                let end = c.out.len().min(c.out_pos + (1 << 20));
                match c.stream.write(&c.out[c.out_pos..end]) {
                    Io::Done(n) => c.out_pos += n,
                    Io::WantRead => {
                        c.want = POLLIN;
                        return true;
                    }
                    Io::WantWrite => {
                        c.want = POLLOUT;
                        return true;
                    }
                    Io::Closed => return false,
                }
            }
            State::Shutdown => {
                // A best-effort close_notify, then done.
                c.stream.shutdown();
                return false;
            }
        }
    }
}
