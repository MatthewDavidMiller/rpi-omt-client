//! A bounded HTTPS GET for the one download the deployer makes: HTTPS only,
//! TLS 1.2 or later with the system's trust roots and hostname
//! verification, at most five redirects, a byte ceiling, and one deadline
//! for the whole exchange. The response is the caller's to verify; the
//! SD-card path pins it to a SHA-512.

use std::ffi::CString;
use std::net::TcpStream;

use omt_common::{Error, Result};
use omt_sys::ssl::TLS1_2_VERSION;
use omt_sys::ssl::safe::{Context, Io, Stream, clear_errors, take_error};

use crate::sys::{self, Cancel, Sock, now_ms};

const MAX_HEADER_BYTES: usize = 64 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub struct UrlParts {
    pub host: String,
    pub port: u16,
    pub path: String,
}

pub fn parse_url(url: &str) -> Result<UrlParts> {
    let unsupported = || Error::new(format!("unsupported URL: {url}"));
    let Some(after) = url.strip_prefix("https://") else {
        return Err(Error::new(format!("only https:// URLs are allowed: {url}")));
    };
    let alen = after.find(['/', '?', '#']).unwrap_or(after.len());
    let (authority, rest) = after.split_at(alen);
    let (host, port) = match authority.split_once(':') {
        Some((h, p)) => {
            let port = omt_common::buf::parse_u64(p.as_bytes(), 65535).filter(|&p| p != 0);
            (h, u16::try_from(port.ok_or_else(unsupported)?).map_err(|_| unsupported())?)
        }
        None => (authority, 443),
    };
    if host.is_empty() || host.len() >= 256 || authority.contains(['@', '[']) {
        return Err(unsupported());
    }
    let rest = &rest[..rest.find('#').unwrap_or(rest.len())];
    let path = if rest.starts_with('/') { rest.to_owned() } else { format!("/{rest}") };
    if path.bytes().any(|b| b <= 0x20 || b >= 0x7f) {
        return Err(unsupported());
    }
    Ok(UrlParts { host: host.to_owned(), port, path })
}

struct Conn<'a> {
    tls: Stream,
    deadline: u64,
    cancel: Option<&'a Cancel>,
}

impl Conn<'_> {
    /// Waits on the socket for an operation that wants to retry.
    fn wait(&self, io: Io) -> Result<()> {
        sys::check_cancel(self.cancel)?;
        let now = now_ms();
        if now >= self.deadline {
            return Err(Error::new("timed out"));
        }
        sys::wait_socket(self.tls.socket(), io == Io::WantWrite, (self.deadline - now).min(200))?;
        Ok(())
    }

    fn handshake(&mut self) -> Result<()> {
        loop {
            clear_errors();
            match self.tls.handshake() {
                Io::Done(_) => return Ok(()),
                io @ (Io::WantRead | Io::WantWrite) => self.wait(io)?,
                Io::Closed => {
                    return Err(match self.tls.verify_error() {
                        Some(e) => Error::new(format!("TLS certificate verification failed: {e}")),
                        None => tls_error("TLS handshake failed"),
                    });
                }
            }
        }
    }

    fn write_all(&mut self, mut data: &[u8]) -> Result<()> {
        while !data.is_empty() {
            match self.tls.write(data) {
                Io::Done(n) => data = &data[n..],
                io @ (Io::WantRead | Io::WantWrite) => self.wait(io)?,
                Io::Closed => return Err(tls_error("TLS write failed")),
            }
        }
        Ok(())
    }

    /// Bytes read, 0 at end of stream.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        loop {
            match self.tls.read_client(buf) {
                Io::Done(n) => return Ok(n),
                io @ (Io::WantRead | Io::WantWrite) => self.wait(io)?,
                Io::Closed => return Err(tls_error("TLS read failed")),
            }
        }
    }
}

fn tls_error(what: &str) -> Error {
    let text = take_error();
    if text == "unknown error" { Error::new(what) } else { Error::new(format!("{what}: {text}")) }
}

fn header_value<'h>(headers: &'h str, name: &str) -> Option<&'h str> {
    let mut lines = headers.split("\r\n");
    lines.next();
    for line in lines {
        if line.len() > name.len()
            && line.as_bytes()[name.len()] == b':'
            && line[..name.len()].eq_ignore_ascii_case(name)
        {
            return Some(line[name.len() + 1..].trim());
        }
    }
    None
}

fn too_large(limit: usize) -> Error {
    Error::new(format!("response body exceeds {limit} bytes"))
}

/// Reads a chunked body into `out`; `have` holds bytes already received.
fn read_chunked(
    c: &mut Conn<'_>,
    mut have: Vec<u8>,
    limit: usize,
    out: &mut Vec<u8>,
) -> Result<()> {
    let mut chunk = vec![0_u8; 16384];
    let malformed = || Error::new("malformed chunked response");
    let early = || Error::new("the response ended early");
    loop {
        let nl = loop {
            if let Some(nl) = have.iter().position(|&b| b == b'\n') {
                break nl;
            }
            if have.len() > 1024 {
                return Err(malformed());
            }
            let n = c.read(&mut chunk)?;
            if n == 0 {
                return Err(early());
            }
            have.extend_from_slice(&chunk[..n]);
        };
        let mut size: u64 = 0;
        let mut digits = 0;
        for &ch in &have[..nl] {
            let Some(v) = char::from(ch).to_digit(16) else { break };
            if size > (limit as u64 >> 4) + 1 {
                return Err(too_large(limit));
            }
            size = size * 16 + u64::from(v);
            digits += 1;
        }
        if digits == 0 {
            return Err(malformed());
        }
        have.drain(..=nl);
        if size == 0 {
            return Ok(()); // trailers are not needed
        }
        let size = usize::try_from(size).map_err(|_| too_large(limit))?;
        if out.len() + size > limit {
            return Err(too_large(limit));
        }
        let need = size + 2;
        while have.len() < need {
            let n = c.read(&mut chunk)?;
            if n == 0 {
                return Err(early());
            }
            have.extend_from_slice(&chunk[..n]);
        }
        out.extend_from_slice(&have[..size]);
        have.drain(..need);
    }
}

enum Reply {
    Body(Vec<u8>),
    Redirect(Option<String>),
    Status(u64),
}

fn connect<'a>(
    ctx: &Context,
    u: &UrlParts,
    deadline: u64,
    cancel: Option<&'a Cancel>,
) -> Result<Conn<'a>> {
    let now = now_ms();
    if now >= deadline {
        return Err(Error::new("timed out"));
    }
    let socket: TcpStream = Sock::connect(&u.host, u.port, deadline - now)?.into_inner();
    socket.set_nonblocking(true).map_err(|e| sys::os_error("TLS setup failed", &e))?;
    let host = CString::new(u.host.as_str()).map_err(|_| Error::new("TLS setup failed"))?;
    let tls = ctx.connect(socket, &host).ok_or_else(|| tls_error("TLS setup failed"))?;
    Ok(Conn { tls, deadline, cancel })
}

/// One request: a body, a redirect's target, or another status.
fn request_once(
    ctx: &Context,
    u: &UrlParts,
    deadline: u64,
    limit: usize,
    cancel: Option<&Cancel>,
) -> Result<Reply> {
    let mut c = connect(ctx, u, deadline, cancel)?;
    let reply = exchange(&mut c, u, limit);
    c.tls.shutdown();
    reply
}

fn exchange(c: &mut Conn<'_>, u: &UrlParts, limit: usize) -> Result<Reply> {
    c.handshake()?;
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: rpi-omt-deploy/{}\r\nAccept: */*\r\n\
         Accept-Encoding: identity\r\nConnection: close\r\n\r\n",
        u.path,
        u.host,
        omt_common::VERSION
    );
    c.write_all(req.as_bytes())?;
    let mut head = Vec::new();
    let mut chunk = vec![0_u8; 16384];
    let header_end = loop {
        let n = c.read(&mut chunk)?;
        if n == 0 {
            return Err(Error::new("the server closed the connection before responding"));
        }
        head.extend_from_slice(&chunk[..n]);
        if let Some(i) = head.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if head.len() > MAX_HEADER_BYTES {
            return Err(Error::new("response headers are too large"));
        }
    };
    let rest = head.split_off(header_end);
    let headers = String::from_utf8_lossy(&head).into_owned();
    let malformed = || Error::new("malformed HTTP response");
    if headers.len() < 12 || !headers.starts_with("HTTP/1.") {
        return Err(malformed());
    }
    let code = omt_common::buf::parse_u64(&headers.as_bytes()[9..12], 999).ok_or_else(malformed)?;
    if (300..400).contains(&code) && code != 304 {
        let location = header_value(&headers, "Location").filter(|l| !l.is_empty());
        return Ok(Reply::Redirect(location.map(str::to_owned)));
    }
    if code != 200 {
        return Ok(Reply::Status(code));
    }
    let mut body = Vec::new();
    if header_value(&headers, "Transfer-Encoding")
        .is_some_and(|te| te.eq_ignore_ascii_case("chunked"))
    {
        read_chunked(c, rest, limit, &mut body)?;
        return Ok(Reply::Body(body));
    }
    let length = match header_value(&headers, "Content-Length") {
        Some(v) => Some(
            omt_common::buf::parse_u64(v.as_bytes(), u64::MAX)
                .ok_or_else(|| Error::new("malformed Content-Length"))?,
        ),
        None => None,
    };
    if length.is_some_and(|l| l > limit as u64) {
        return Err(too_large(limit));
    }
    body.extend_from_slice(&rest);
    while length.is_none_or(|l| (body.len() as u64) < l) {
        let n = c.read(&mut chunk)?;
        if n == 0 {
            if length.is_some() {
                return Err(Error::new("the response ended early"));
            }
            break;
        }
        body.extend_from_slice(&chunk[..n]);
        if body.len() > limit {
            return Err(too_large(limit));
        }
    }
    if length.is_some_and(|l| body.len() as u64 != l) {
        return Err(Error::new("the response is longer than its Content-Length"));
    }
    Ok(Reply::Body(body))
}

fn resolve_location(base: &UrlParts, location: &str) -> Result<String> {
    if location.starts_with("https://") {
        Ok(location.to_owned())
    } else if location.starts_with("//") {
        Ok(format!("https:{location}"))
    } else if location.starts_with('/') {
        Ok(if base.port == 443 {
            format!("https://{}{location}", base.host)
        } else {
            format!("https://{}:{}{location}", base.host, base.port)
        })
    } else {
        Err(Error::new(format!("refusing a redirect away from HTTPS: {location}")))
    }
}

/// Loads the operating system's trust roots. A static binary cannot rely
/// on the OpenSSL directory it was configured with -- that is the build
/// host's layout -- so on Linux this probes the bundle locations the
/// distributions use: SSL_CERT_FILE and SSL_CERT_DIR first, then the
/// well-known paths. On Windows it is the machine's ROOT store.
fn load_roots(ctx: &Context) -> Result<()> {
    #[cfg(windows)]
    {
        let mut loaded = 0;
        omt_sys::win32::for_each_root_certificate(|der| {
            loaded += usize::from(ctx.add_trusted_der(der));
        })
        .map_err(|e| sys::os_error("cannot open the Windows ROOT certificate store", &e))?;
        if loaded > 0 {
            return Ok(());
        }
    }
    #[cfg(not(windows))]
    {
        const BUNDLES: [&str; 8] = [
            "/etc/ssl/certs/ca-certificates.crt",
            "/etc/pki/tls/certs/ca-bundle.crt",
            "/etc/ssl/ca-bundle.pem",
            "/etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem",
            "/etc/ssl/cert.pem",
            "/usr/local/share/certs/ca-root-nss.crt",
            "/etc/pki/tls/cacert.pem",
            "/etc/certs/ca-certificates.crt",
        ];
        const DIRECTORIES: [&str; 2] = ["/etc/ssl/certs", "/etc/pki/tls/certs"];
        let c = |s: &str| CString::new(s).ok();
        let mut loaded = false;
        if let Some(file) =
            sys::getenv("SSL_CERT_FILE").filter(|v| !v.is_empty()).and_then(|v| c(&v))
        {
            loaded = ctx.load_verify_locations(Some(&file), None);
        }
        if let Some(dir) = sys::getenv("SSL_CERT_DIR")
            .filter(|v| !v.is_empty() && sys::is_dir(v))
            .and_then(|v| c(&v))
        {
            loaded |= ctx.load_verify_locations(None, Some(&dir));
        }
        for bundle in BUNDLES {
            if !loaded
                && sys::is_file(bundle)
                && let Some(f) = c(bundle)
            {
                loaded = ctx.load_verify_locations(Some(&f), None);
            }
        }
        for dir in DIRECTORIES {
            if !loaded
                && sys::is_dir(dir)
                && let Some(d) = c(dir)
            {
                loaded = ctx.load_verify_locations(None, Some(&d));
            }
        }
        if loaded {
            return Ok(());
        }
    }
    Err(Error::new("no native TLS root certificates were available"))
}

/// GETs `url`, following at most five HTTPS redirects, refusing a body over
/// `limit` bytes, and finishing within `timeout_ms` overall.
pub fn get(url: &str, limit: usize, timeout_ms: u64, cancel: Option<&Cancel>) -> Result<Vec<u8>> {
    let ctx = Context::new_client().ok_or_else(|| tls_error("TLS is unavailable"))?;
    if !ctx.set_min_proto_version(TLS1_2_VERSION) {
        return Err(tls_error("TLS is unavailable"));
    }
    load_roots(&ctx)?;
    let deadline = now_ms() + timeout_ms;
    let mut current = url.to_owned();
    for redirects in 0.. {
        let u = parse_url(&current)?;
        match request_once(&ctx, &u, deadline, limit, cancel)? {
            Reply::Body(body) => return Ok(body),
            Reply::Status(code) => return Err(Error::new(format!("http status: {code}"))),
            Reply::Redirect(None) => return Err(Error::new("redirect without a Location")),
            Reply::Redirect(Some(_)) if redirects == 5 => {
                return Err(Error::new("too many redirects"));
            }
            Reply::Redirect(Some(location)) => current = resolve_location(&u, &location)?,
        }
    }
    Err(Error::new("too many redirects"))
}
