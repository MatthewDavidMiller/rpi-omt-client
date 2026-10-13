//! OpenSSL 3 libssl: the TLS server the Web frontend runs.
//!
//! The `SSL_CTX_set_*` macros of the C headers expand to `SSL_CTX_ctrl`
//! calls, which the safe wrappers make directly with the control numbers
//! below.

use std::ffi::{c_char, c_int, c_long, c_uchar, c_uint, c_ulong, c_void};

opaque!(SSL_CTX, SSL, SSL_METHOD, X509, X509_STORE);

pub const TLS1_2_VERSION: c_long = 0x0303;

pub const SSL_OP_NO_TICKET: u64 = 1 << 14;
pub const SSL_OP_NO_COMPRESSION: u64 = 1 << 17;
pub const SSL_OP_CIPHER_SERVER_PREFERENCE: u64 = 1 << 22;
pub const SSL_OP_NO_RENEGOTIATION: u64 = 1 << 30;

pub const SSL_MODE_ENABLE_PARTIAL_WRITE: c_long = 0x1;
pub const SSL_MODE_ACCEPT_MOVING_WRITE_BUFFER: c_long = 0x2;

pub const SSL_CTRL_MODE: c_int = 33;
pub const SSL_CTRL_SET_GROUPS_LIST: c_int = 92;
pub const SSL_CTRL_SET_MIN_PROTO_VERSION: c_int = 123;

pub const SSL_FILETYPE_PEM: c_int = 1;

pub const SSL_ERROR_SSL: c_int = 1;
pub const SSL_ERROR_WANT_READ: c_int = 2;
pub const SSL_ERROR_WANT_WRITE: c_int = 3;
pub const SSL_ERROR_SYSCALL: c_int = 5;
pub const SSL_ERROR_ZERO_RETURN: c_int = 6;
pub const SSL_VERIFY_PEER: c_int = 1;
pub const SSL_CTRL_SET_TLSEXT_HOSTNAME: c_int = 55;
pub const TLSEXT_NAMETYPE_HOST_NAME: c_long = 0;
pub const SSL_R_UNEXPECTED_EOF_WHILE_READING: c_ulong = 294;
pub const ERR_REASON_MASK: c_ulong = 0x7F_FFFF;
pub const X509_V_OK: c_long = 0;

pub const SSL_TLSEXT_ERR_OK: c_int = 0;
pub const SSL_TLSEXT_ERR_NOACK: c_int = 3;

pub type AlpnSelectCb = unsafe extern "C" fn(
    ssl: *mut SSL,
    out: *mut *const c_uchar,
    outlen: *mut c_uchar,
    input: *const c_uchar,
    inlen: c_uint,
    arg: *mut c_void,
) -> c_int;

unsafe extern "C" {
    pub fn TLS_server_method() -> *const SSL_METHOD;
    pub fn TLS_client_method() -> *const SSL_METHOD;
    pub fn SSL_CTX_set_verify(ctx: *mut SSL_CTX, mode: c_int, cb: *const c_void);
    pub fn SSL_CTX_load_verify_locations(
        ctx: *mut SSL_CTX,
        file: *const c_char,
        dir: *const c_char,
    ) -> c_int;
    pub fn SSL_CTX_get_cert_store(ctx: *const SSL_CTX) -> *mut X509_STORE;
    pub fn X509_STORE_add_cert(store: *mut X509_STORE, x: *mut X509) -> c_int;
    pub fn d2i_X509(x: *mut *mut X509, pp: *mut *const u8, len: c_long) -> *mut X509;
    pub fn X509_free(x: *mut X509);
    pub fn SSL_ctrl(ssl: *mut SSL, cmd: c_int, larg: c_long, parg: *mut c_void) -> c_long;
    pub fn SSL_set1_host(ssl: *mut SSL, host: *const c_char) -> c_int;
    pub fn SSL_set_connect_state(ssl: *mut SSL);
    pub fn SSL_get_verify_result(ssl: *const SSL) -> c_long;
    pub fn X509_verify_cert_error_string(n: c_long) -> *const c_char;
    pub fn ERR_peek_error() -> c_ulong;
    pub fn SSL_CTX_new(method: *const SSL_METHOD) -> *mut SSL_CTX;
    pub fn SSL_CTX_free(ctx: *mut SSL_CTX);
    pub fn SSL_CTX_ctrl(ctx: *mut SSL_CTX, cmd: c_int, larg: c_long, parg: *mut c_void) -> c_long;
    pub fn SSL_CTX_set_options(ctx: *mut SSL_CTX, op: u64) -> u64;
    pub fn SSL_CTX_set_cipher_list(ctx: *mut SSL_CTX, list: *const c_char) -> c_int;
    pub fn SSL_CTX_set_ciphersuites(ctx: *mut SSL_CTX, list: *const c_char) -> c_int;
    pub fn SSL_CTX_set_alpn_select_cb(
        ctx: *mut SSL_CTX,
        cb: Option<AlpnSelectCb>,
        arg: *mut c_void,
    );
    pub fn SSL_CTX_use_certificate_chain_file(ctx: *mut SSL_CTX, file: *const c_char) -> c_int;
    pub fn SSL_CTX_use_PrivateKey_file(
        ctx: *mut SSL_CTX,
        file: *const c_char,
        kind: c_int,
    ) -> c_int;
    pub fn SSL_CTX_check_private_key(ctx: *const SSL_CTX) -> c_int;

    pub fn SSL_new(ctx: *mut SSL_CTX) -> *mut SSL;
    pub fn SSL_free(ssl: *mut SSL);
    pub fn SSL_set_fd(ssl: *mut SSL, fd: c_int) -> c_int;
    pub fn SSL_set_accept_state(ssl: *mut SSL);
    pub fn SSL_do_handshake(ssl: *mut SSL) -> c_int;
    pub fn SSL_read(ssl: *mut SSL, buf: *mut c_void, num: c_int) -> c_int;
    pub fn SSL_write(ssl: *mut SSL, buf: *const c_void, num: c_int) -> c_int;
    pub fn SSL_get_error(ssl: *const SSL, ret: c_int) -> c_int;
    pub fn SSL_shutdown(ssl: *mut SSL) -> c_int;

    pub fn ERR_get_error() -> c_ulong;
    pub fn ERR_error_string_n(e: c_ulong, buf: *mut c_char, len: usize);
    pub fn ERR_clear_error();
}

/// Owned contexts and connections. Every OpenSSL object is freed exactly
/// once by `Drop`, and a connection owns the socket it runs over, so the
/// descriptor cannot be closed or reused while OpenSSL still holds it.
pub mod safe {
    #[allow(clippy::wildcard_imports)]
    use super::*;
    use std::ffi::{CStr, c_int, c_long, c_uchar, c_uint, c_void};
    use std::net::TcpStream;
    use std::ptr::{self, NonNull};

    /// The descriptor OpenSSL's socket BIO takes: a file descriptor, or on
    /// Windows the SOCKET handle, which OpenSSL itself narrows to `int`.
    fn raw_socket(s: &TcpStream) -> c_int {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            s.as_raw_fd()
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawSocket;
            s.as_raw_socket() as c_int
        }
    }

    /// The oldest queued OpenSSL error as text ("unknown error" when the
    /// queue is empty); the queue is then cleared.
    pub fn take_error() -> String {
        // SAFETY: reads the calling thread's error queue.
        let code = unsafe { ERR_get_error() };
        let mut text = String::from("unknown error");
        if code != 0 {
            let mut buf = [0_u8; 256];
            // SAFETY: `buf` is writable for its length; OpenSSL writes a NUL
            // terminated string of at most that many bytes.
            unsafe { ERR_error_string_n(code, buf.as_mut_ptr().cast(), buf.len()) };
            if let Ok(s) = CStr::from_bytes_until_nul(&buf) {
                text = s.to_string_lossy().into_owned();
            }
        }
        clear_errors();
        text
    }

    pub fn clear_errors() {
        // SAFETY: clears the calling thread's error queue.
        unsafe { ERR_clear_error() };
    }

    #[derive(Debug)]
    pub struct Context(NonNull<SSL_CTX>);

    impl Drop for Context {
        fn drop(&mut self) {
            // SAFETY: the context is owned and freed once; connections hold
            // their own reference.
            unsafe { SSL_CTX_free(self.0.as_ptr()) };
        }
    }

    /// Chooses "http/1.1" from the client's ALPN list, or declines ALPN.
    unsafe extern "C" fn select_http11(
        _ssl: *mut SSL,
        out: *mut *const c_uchar,
        outlen: *mut c_uchar,
        input: *const c_uchar,
        inlen: c_uint,
        _arg: *mut c_void,
    ) -> c_int {
        if input.is_null() {
            return SSL_TLSEXT_ERR_NOACK;
        }
        // SAFETY: OpenSSL passes the client's protocol list, readable for
        // `inlen` bytes, for the duration of the callback.
        let list = unsafe { std::slice::from_raw_parts(input, inlen as usize) };
        let mut i = 0;
        while i < list.len() {
            let n = usize::from(list[i]);
            let Some(name) = list.get(i + 1..i + 1 + n) else { break };
            if name == b"http/1.1" {
                // SAFETY: `out` and `outlen` are valid out-parameters; the
                // selected name points into the client's list, which
                // OpenSSL keeps alive for the handshake.
                unsafe {
                    *out = name.as_ptr();
                }
                // SAFETY: as above.
                unsafe {
                    *outlen = n as c_uchar;
                }
                return SSL_TLSEXT_ERR_OK;
            }
            i += 1 + n;
        }
        SSL_TLSEXT_ERR_NOACK
    }

    impl Context {
        pub fn new_server() -> Option<Self> {
            // SAFETY: the method getter takes no arguments and returns a
            // static object.
            let method = unsafe { TLS_server_method() };
            // SAFETY: SSL_CTX_new returns an owned context or NULL.
            let ctx = unsafe { SSL_CTX_new(method) };
            NonNull::new(ctx).map(Self)
        }

        fn ctrl(&self, cmd: c_int, larg: c_long, parg: *mut c_void) -> c_long {
            // SAFETY: the context is live; each caller passes the argument
            // shape its control number documents.
            unsafe { SSL_CTX_ctrl(self.0.as_ptr(), cmd, larg, parg) }
        }

        pub fn set_min_proto_version(&self, version: c_long) -> bool {
            self.ctrl(SSL_CTRL_SET_MIN_PROTO_VERSION, version, ptr::null_mut()) == 1
        }

        pub fn set_mode(&self, mode: c_long) {
            self.ctrl(SSL_CTRL_MODE, mode, ptr::null_mut());
        }

        pub fn set_groups_list(&self, groups: &CStr) -> bool {
            // OpenSSL reads the string and never writes through the pointer;
            // the macro's cast to `char *` is only historical.
            self.ctrl(SSL_CTRL_SET_GROUPS_LIST, 0, groups.as_ptr().cast_mut().cast()) == 1
        }

        pub fn set_options(&self, options: u64) {
            // SAFETY: the context is live.
            unsafe { SSL_CTX_set_options(self.0.as_ptr(), options) };
        }

        pub fn set_cipher_list(&self, list: &CStr) -> bool {
            // SAFETY: the context is live and the list NUL-terminated.
            unsafe { SSL_CTX_set_cipher_list(self.0.as_ptr(), list.as_ptr()) == 1 }
        }

        pub fn set_ciphersuites(&self, list: &CStr) -> bool {
            // SAFETY: the context is live and the list NUL-terminated.
            unsafe { SSL_CTX_set_ciphersuites(self.0.as_ptr(), list.as_ptr()) == 1 }
        }

        /// Answers ALPN with HTTP/1.1 only.
        pub fn select_alpn_http11(&self) {
            // SAFETY: the callback is a plain function that reads only its
            // arguments; no user data is registered.
            unsafe {
                SSL_CTX_set_alpn_select_cb(self.0.as_ptr(), Some(select_http11), ptr::null_mut());
            }
        }

        pub fn use_certificate_chain_file(&self, path: &CStr) -> bool {
            // SAFETY: the context is live and the path NUL-terminated.
            unsafe { SSL_CTX_use_certificate_chain_file(self.0.as_ptr(), path.as_ptr()) == 1 }
        }

        pub fn use_private_key_file(&self, path: &CStr) -> bool {
            // SAFETY: the context is live and the path NUL-terminated.
            unsafe {
                SSL_CTX_use_PrivateKey_file(self.0.as_ptr(), path.as_ptr(), SSL_FILETYPE_PEM) == 1
            }
        }

        pub fn check_private_key(&self) -> bool {
            // SAFETY: the context is live.
            unsafe { SSL_CTX_check_private_key(self.0.as_ptr()) == 1 }
        }

        /// A TLS client context that verifies the peer.
        pub fn new_client() -> Option<Self> {
            // SAFETY: the method getter takes no arguments and returns a
            // static object.
            let method = unsafe { TLS_client_method() };
            // SAFETY: SSL_CTX_new returns an owned context or NULL.
            let ctx = NonNull::new(unsafe { SSL_CTX_new(method) }).map(Self)?;
            // SAFETY: the context is live; a NULL callback keeps OpenSSL's
            // own verification.
            unsafe { SSL_CTX_set_verify(ctx.0.as_ptr(), SSL_VERIFY_PEER, ptr::null()) };
            Some(ctx)
        }

        /// Trusts a CA bundle file or a hashed certificate directory.
        pub fn load_verify_locations(&self, file: Option<&CStr>, dir: Option<&CStr>) -> bool {
            let file = file.map_or(ptr::null(), CStr::as_ptr);
            let dir = dir.map_or(ptr::null(), CStr::as_ptr);
            // SAFETY: the context is live; each path is NULL or
            // NUL-terminated.
            unsafe { SSL_CTX_load_verify_locations(self.0.as_ptr(), file, dir) == 1 }
        }

        /// Trusts one DER certificate; false when it does not parse.
        pub fn add_trusted_der(&self, der: &[u8]) -> bool {
            let Ok(len) = c_long::try_from(der.len()) else { return false };
            let mut p = der.as_ptr();
            // SAFETY: `p` points at `len` readable bytes; a NULL first
            // argument allocates a new certificate.
            let x = unsafe { d2i_X509(ptr::null_mut(), &raw mut p, len) };
            if x.is_null() {
                return false;
            }
            // SAFETY: the context is live; the store is borrowed from it.
            let store = unsafe { SSL_CTX_get_cert_store(self.0.as_ptr()) };
            // SAFETY: the store takes its own reference to the certificate.
            let ok = !store.is_null() && unsafe { X509_STORE_add_cert(store, x) } == 1;
            // SAFETY: our reference is released once.
            unsafe { X509_free(x) };
            ok
        }

        /// A client connection to `host` over `socket`, which it takes
        /// over: SNI set and the certificate checked against the name.
        pub fn connect(&self, socket: TcpStream, host: &CStr) -> Option<Stream> {
            // SAFETY: the context is live; SSL_new takes its own reference.
            let ssl = NonNull::new(unsafe { SSL_new(self.0.as_ptr()) })?;
            let stream = Stream { ssl, socket };
            let p = stream.ssl.as_ptr();
            // SAFETY: the connection is live and the descriptor owned by the
            // same Stream.
            if unsafe { SSL_set_fd(p, raw_socket(&stream.socket)) } != 1 {
                return None;
            }
            // SAFETY: OpenSSL copies the name; the cast to `void *` is the
            // macro's own.
            let sni = unsafe {
                SSL_ctrl(
                    p,
                    SSL_CTRL_SET_TLSEXT_HOSTNAME,
                    TLSEXT_NAMETYPE_HOST_NAME,
                    host.as_ptr().cast_mut().cast(),
                )
            };
            // SAFETY: OpenSSL copies the name.
            if sni != 1 || unsafe { SSL_set1_host(p, host.as_ptr()) } != 1 {
                return None;
            }
            // SAFETY: the connection is live.
            unsafe { SSL_set_connect_state(p) };
            Some(stream)
        }

        /// A server-side connection over `socket`, which it takes over.
        pub fn accept(&self, socket: TcpStream) -> Option<Stream> {
            // SAFETY: the context is live; SSL_new takes its own reference.
            let ssl = NonNull::new(unsafe { SSL_new(self.0.as_ptr()) })?;
            let stream = Stream { ssl, socket };
            // SAFETY: the connection is live and the descriptor is owned by
            // the same Stream, which frees the SSL before closing it.
            if unsafe { SSL_set_fd(stream.ssl.as_ptr(), raw_socket(&stream.socket)) } != 1 {
                return None;
            }
            // SAFETY: the connection is live.
            unsafe { SSL_set_accept_state(stream.ssl.as_ptr()) };
            Some(stream)
        }
    }

    /// The outcome of one non-blocking TLS operation.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Io {
        Done(usize),
        WantRead,
        WantWrite,
        /// Closed by the peer, or failed: the connection is finished.
        Closed,
    }

    #[derive(Debug)]
    pub struct Stream {
        ssl: NonNull<SSL>,
        socket: TcpStream,
    }

    impl Drop for Stream {
        fn drop(&mut self) {
            // SAFETY: the connection is owned and freed once, before the
            // socket field drops and closes the descriptor.
            unsafe { SSL_free(self.ssl.as_ptr()) };
        }
    }

    impl Stream {
        pub fn socket(&self) -> &TcpStream {
            &self.socket
        }

        fn classify(&self, rc: c_int) -> Io {
            if rc > 0 {
                return Io::Done(rc as usize);
            }
            // SAFETY: the connection is live and `rc` is the return value of
            // the operation just made on it.
            match unsafe { SSL_get_error(self.ssl.as_ptr(), rc) } {
                SSL_ERROR_WANT_READ => Io::WantRead,
                SSL_ERROR_WANT_WRITE => Io::WantWrite,
                _ => Io::Closed,
            }
        }

        /// `Done(1)` once the handshake has completed.
        pub fn handshake(&mut self) -> Io {
            // SAFETY: the connection is live.
            let rc = unsafe { SSL_do_handshake(self.ssl.as_ptr()) };
            if rc == 1 { Io::Done(1) } else { self.classify(rc.min(0)) }
        }

        pub fn read(&mut self, buf: &mut [u8]) -> Io {
            let n = c_int::try_from(buf.len()).unwrap_or(c_int::MAX);
            // SAFETY: `buf` is writable for at least `n` bytes.
            let rc = unsafe { SSL_read(self.ssl.as_ptr(), buf.as_mut_ptr().cast(), n) };
            self.classify(rc)
        }

        pub fn write(&mut self, buf: &[u8]) -> Io {
            if buf.is_empty() {
                return Io::Done(0);
            }
            let n = c_int::try_from(buf.len()).unwrap_or(c_int::MAX);
            // SAFETY: `buf` is readable for at least `n` bytes; the context
            // allows a moving write buffer, so a retry may pass a new slice.
            let rc = unsafe { SSL_write(self.ssl.as_ptr(), buf.as_ptr().cast(), n) };
            self.classify(rc)
        }

        /// The peer certificate's verification error, when it failed.
        pub fn verify_error(&self) -> Option<String> {
            // SAFETY: the connection is live.
            let code = unsafe { SSL_get_verify_result(self.ssl.as_ptr()) };
            if code == X509_V_OK {
                return None;
            }
            // SAFETY: returns a static string for any code.
            let text = unsafe { X509_verify_cert_error_string(code) };
            if text.is_null() {
                return Some(format!("error {code}"));
            }
            // SAFETY: a NUL-terminated static string.
            Some(unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned())
        }

        /// A client read that tells a clean (or unannounced) end of stream
        /// apart from a failure: `Done(0)` is the end.
        pub fn read_client(&mut self, buf: &mut [u8]) -> Io {
            let n = c_int::try_from(buf.len()).unwrap_or(c_int::MAX);
            // SAFETY: `buf` is writable for at least `n` bytes.
            let rc = unsafe { SSL_read(self.ssl.as_ptr(), buf.as_mut_ptr().cast(), n) };
            if rc > 0 {
                return Io::Done(rc as usize);
            }
            // SAFETY: the connection is live and `rc` came from it.
            let code = unsafe { SSL_get_error(self.ssl.as_ptr(), rc) };
            match code {
                SSL_ERROR_WANT_READ => Io::WantRead,
                SSL_ERROR_WANT_WRITE => Io::WantWrite,
                SSL_ERROR_ZERO_RETURN | SSL_ERROR_SYSCALL => {
                    clear_errors();
                    Io::Done(0)
                }
                SSL_ERROR_SSL => {
                    // SAFETY: reads the calling thread's error queue.
                    let e = unsafe { ERR_peek_error() };
                    if e & ERR_REASON_MASK == SSL_R_UNEXPECTED_EOF_WHILE_READING {
                        clear_errors();
                        Io::Done(0)
                    } else {
                        Io::Closed
                    }
                }
                _ => Io::Closed,
            }
        }

        /// A best-effort close_notify.
        pub fn shutdown(&mut self) {
            // SAFETY: the connection is live.
            unsafe { SSL_shutdown(self.ssl.as_ptr()) };
        }
    }
}
