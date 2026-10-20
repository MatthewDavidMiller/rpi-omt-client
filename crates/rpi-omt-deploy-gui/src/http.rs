//! Just enough HTTP/1.1 for one browser on the loopback interface: a
//! bounded request reader, a response writer, and query decoding. Every
//! connection carries one request and is closed after the response, so
//! there is no keep-alive state to bound.
//!
//! The request-head rules are the ones omt-web's server enforces; that
//! server is not shared because it is a TLS poll loop over Unix descriptors
//! and this one also runs on Windows.

use std::fmt::Write as _;
use std::io::{ErrorKind, Read, Write};

pub const MAX_HEADER_BYTES: usize = 16 * 1024;
pub const MAX_HEADERS: usize = 64;
pub const MAX_BODY_BYTES: usize = 64 * 1024;
const METHOD_MAX: usize = 15;
const TARGET_MAX: usize = 8191;

pub fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Payload Too Large",
        414 => "URI Too Long",
        415 => "Unsupported Media Type",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        505 => "HTTP Version Not Supported",
        _ => "Unknown",
    }
}

#[derive(Clone, Debug, Default)]
pub struct Request {
    pub method: String,
    pub path: String,
    /// The raw query string, without the `?`.
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    /// The first header named `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// The first query parameter named `name`, percent-decoded.
    pub fn param(&self, name: &str) -> Option<String> {
        self.query.split('&').find_map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(k)? == name).then(|| percent_decode(v)).flatten()
        })
    }
}

/// Decodes `%XX` escapes and `+`; `None` for a malformed escape or a result
/// that is not UTF-8.
pub fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = bytes.get(i + 1..i + 3)?;
                let hex = std::str::from_utf8(hex).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

fn is_tchar(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c)
}

fn trim_ows(s: &[u8]) -> &[u8] {
    let start = s.iter().position(|&c| c != b' ' && c != b'\t').unwrap_or(s.len());
    let end = s.iter().rposition(|&c| c != b' ' && c != b'\t').map_or(start, |i| i + 1);
    &s[start..end.max(start)]
}

/// Parses a complete request head (through, not including, the blank
/// line); the status to refuse it with on failure. Returns the request and
/// its declared body length.
pub fn parse_head(block: &[u8]) -> Result<(Request, usize), u16> {
    let mut lines = block.split(|&c| c == b'\n').map(|l| l.strip_suffix(b"\r").unwrap_or(l));
    let line = lines.next().ok_or(400_u16)?;
    let mut parts = line.splitn(3, |&c| c == b' ');
    let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(400);
    };
    if method.is_empty() || method.len() > METHOD_MAX || !method.iter().all(|&c| is_tchar(c)) {
        return Err(400);
    }
    if target.first() != Some(&b'/') || target.iter().any(|&c| c <= 0x20 || c >= 0x7F) {
        return Err(400);
    }
    if target.len() > TARGET_MAX {
        return Err(414);
    }
    if version != b"HTTP/1.1" && version != b"HTTP/1.0" {
        return Err(505);
    }
    // Every byte checked above is printable ASCII.
    let target = String::from_utf8_lossy(target);
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let mut r = Request {
        method: String::from_utf8_lossy(method).into_owned(),
        path: path.to_owned(),
        query: query.to_owned(),
        ..Request::default()
    };
    let mut length: Option<usize> = None;
    for field in lines {
        if matches!(field.first(), Some(b' ' | b'\t')) {
            return Err(400); // obsolete line folding
        }
        let colon = field.iter().position(|&c| c == b':').filter(|&c| c > 0).ok_or(400_u16)?;
        let name = &field[..colon];
        if !name.iter().all(|&c| is_tchar(c)) {
            return Err(400);
        }
        let value = trim_ows(&field[colon + 1..]);
        if value.iter().any(|&c| (c < 0x20 && c != b'\t') || c == 0x7F) {
            return Err(400);
        }
        if r.headers.len() == MAX_HEADERS {
            return Err(431);
        }
        if name.eq_ignore_ascii_case(b"content-length") {
            let n = omt_common::buf::parse_u64(value, u64::from(u32::MAX))
                .and_then(|n| usize::try_from(n).ok())
                .ok_or(400_u16)?;
            if length.is_some_and(|l| l != n) {
                return Err(400);
            }
            length = Some(n);
        } else if name.eq_ignore_ascii_case(b"transfer-encoding") {
            return Err(411);
        }
        r.headers.push((
            String::from_utf8_lossy(name).into_owned(),
            String::from_utf8_lossy(value).into_owned(),
        ));
    }
    Ok((r, length.unwrap_or(0)))
}

/// Reads one request, head then body, never holding more than the head and
/// body ceilings. The stream's read timeout bounds how long a silent client
/// is waited for.
pub fn read_request(stream: &mut impl Read) -> Result<Request, u16> {
    let mut data = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    let head_end = loop {
        if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break end;
        }
        if data.len() >= MAX_HEADER_BYTES {
            return Err(431);
        }
        let room = (MAX_HEADER_BYTES + 3 - data.len()).min(chunk.len());
        match stream.read(&mut chunk[..room]) {
            Ok(0) => return Err(400),
            Ok(n) => data.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => return Err(408),
        }
    };
    let (mut r, length) = parse_head(&data[..head_end])?;
    if length > MAX_BODY_BYTES {
        return Err(413);
    }
    let mut body = data.split_off(head_end + 4);
    if body.len() > length {
        return Err(400);
    }
    while body.len() < length {
        let room = (length - body.len()).min(chunk.len());
        match stream.read(&mut chunk[..room]) {
            Ok(0) => return Err(400),
            Ok(n) => body.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => return Err(408),
        }
    }
    r.body = body;
    Ok(r)
}

#[derive(Debug, Default)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) -> Self {
        Self { status, content_type, body: body.into() }
    }

    pub fn json(body: String) -> Self {
        Self::new(200, "application/json", body)
    }

    /// A refusal, with the reason as a JSON `error` the page can show.
    pub fn error(status: u16, message: &str) -> Self {
        let mut body = String::from("{\"error\":");
        omt_common::json::write_string(&mut body, message);
        body.push('}');
        Self::new(status, "application/json", body)
    }
}

/// The headers every response carries: nothing is cached, nothing may frame
/// the page, and the page may load only what this server serves.
pub const SECURITY_HEADERS: &str = "Cache-Control: no-store\r\n\
     Content-Security-Policy: default-src 'none'; script-src 'self'; style-src 'self'; \
     img-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'; \
     frame-ancestors 'none'\r\n\
     Cross-Origin-Opener-Policy: same-origin\r\n\
     Cross-Origin-Resource-Policy: same-origin\r\n\
     Referrer-Policy: no-referrer\r\n\
     X-Content-Type-Options: nosniff\r\n\
     X-Frame-Options: DENY\r\n";

pub fn write_response(stream: &mut impl Write, r: &Response) -> std::io::Result<()> {
    let mut head = String::with_capacity(768);
    let _ = write!(
        head,
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n",
        r.status,
        reason(r.status),
        r.content_type,
        r.body.len(),
        SECURITY_HEADERS
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(&r.body)?;
    stream.flush()
}
