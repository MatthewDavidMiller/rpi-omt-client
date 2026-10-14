//! The musl C library calls the standard library does not expose, for the
//! two Linux targets the project ships (`x86_64` and `aarch64`).
//!
//! Only what std lacks is declared here. Files, sockets, threads, and
//! processes go through std; this covers process groups and signals, `poll`,
//! `getrandom`, and the open flags `OpenOptionsExt::custom_flags` takes.

use std::ffi::{c_char, c_int, c_long, c_short, c_uint, c_ulong, c_void};
use std::io;
use std::net::{SocketAddr, TcpStream};
use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};

pub type pid_t = c_int;
pub type nfds_t = c_ulong;
pub type ssize_t = isize;

pub const SIGINT: c_int = 2;
pub const SIGKILL: c_int = 9;
pub const SIGPIPE: c_int = 13;
pub const SIGTERM: c_int = 15;

pub const F_GETFL: c_int = 3;
pub const F_SETFL: c_int = 4;

pub const O_NONBLOCK: c_int = 0o4000;
pub const O_CLOEXEC: c_int = 0o2_000_000;
#[cfg(target_arch = "x86_64")]
pub const O_DIRECTORY: c_int = 0o200_000;
#[cfg(target_arch = "x86_64")]
pub const O_NOFOLLOW: c_int = 0o400_000;
#[cfg(target_arch = "aarch64")]
pub const O_DIRECTORY: c_int = 0o40_000;
#[cfg(target_arch = "aarch64")]
pub const O_NOFOLLOW: c_int = 0o100_000;

pub const POLLIN: c_short = 0x001;
pub const POLLOUT: c_short = 0x004;
pub const POLLERR: c_short = 0x008;
pub const POLLHUP: c_short = 0x010;
pub const POLLNVAL: c_short = 0x020;

pub const EINTR: i32 = 4;
pub const EINPROGRESS: i32 = 115;
pub const EPIPE: i32 = 32;
pub const EAGAIN: i32 = 11;
pub const ENOENT: i32 = 2;
pub const EEXIST: i32 = 17;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct pollfd {
    pub fd: c_int,
    pub events: c_short,
    pub revents: c_short,
}

pub const SIG_SETMASK: c_int = 2;

pub const AF_INET: c_int = 2;
pub const AF_INET6: c_int = 10;
pub const SOCK_STREAM: c_int = 1;
pub const SOCK_NONBLOCK: c_int = 0o4000;
pub const SOCK_CLOEXEC: c_int = 0o2_000_000;
pub const SOL_SOCKET: c_int = 1;
pub const SO_ERROR: c_int = 4;
pub const SO_RCVBUF: c_int = 8;

pub const PROT_READ: c_int = 1;
pub const PROT_WRITE: c_int = 2;
pub const MAP_SHARED: c_int = 1;

pub type socklen_t = c_uint;
pub type uid_t = c_uint;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct sockaddr_in {
    pub sin_family: u16,
    pub sin_port: u16,
    pub sin_addr: [u8; 4],
    pub sin_zero: [u8; 8],
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct sockaddr_in6 {
    pub sin6_family: u16,
    pub sin6_port: u16,
    pub sin6_flowinfo: u32,
    pub sin6_addr: [u8; 16],
    pub sin6_scope_id: u32,
}

/// musl's `struct sigaction`: the handler, the mask, the flags, and the
/// restorer musl fills in itself.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct sigaction_t {
    pub sa_handler: usize,
    pub sa_mask: sigset_t,
    pub sa_flags: c_int,
    pub sa_restorer: usize,
}

/// musl's `sigset_t`: 128 bytes of signal bits.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct sigset_t {
    bits: [c_ulong; 128 / size_of::<c_ulong>()],
}

impl sigset_t {
    pub const EMPTY: Self = Self { bits: [0; 128 / size_of::<c_ulong>()] };
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct timespec {
    pub tv_sec: i64,
    pub tv_nsec: c_long,
}

pub const CLOCK_MONOTONIC: c_int = 1;

unsafe extern "C" {
    pub fn clock_gettime(clock: c_int, ts: *mut timespec) -> c_int;
    pub fn getrandom(buf: *mut c_void, len: usize, flags: c_uint) -> ssize_t;
    pub fn kill(pid: pid_t, sig: c_int) -> c_int;
    pub fn poll(fds: *mut pollfd, nfds: nfds_t, timeout: c_int) -> c_int;
    pub fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
    pub fn sigfillset(set: *mut sigset_t) -> c_int;
    pub fn sigemptyset(set: *mut sigset_t) -> c_int;
    pub fn sigaction(sig: c_int, act: *const sigaction_t, old: *mut sigaction_t) -> c_int;
    pub fn socket(domain: c_int, kind: c_int, protocol: c_int) -> c_int;
    pub fn setsockopt(
        fd: c_int,
        level: c_int,
        name: c_int,
        value: *const c_void,
        len: socklen_t,
    ) -> c_int;
    pub fn getsockopt(
        fd: c_int,
        level: c_int,
        name: c_int,
        value: *mut c_void,
        len: *mut socklen_t,
    ) -> c_int;
    pub fn connect(fd: c_int, addr: *const c_void, len: socklen_t) -> c_int;
    pub fn close(fd: c_int) -> c_int;
    pub fn getuid() -> uid_t;
    pub fn ioctl(fd: c_int, request: c_int, ...) -> c_int;
    pub fn mmap(
        addr: *mut c_void,
        len: usize,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: i64,
    ) -> *mut c_void;
    pub fn munmap(addr: *mut c_void, len: usize) -> c_int;
    pub fn strerror(errnum: c_int) -> *const c_char;
    pub fn pthread_sigmask(how: c_int, set: *const sigset_t, old: *mut sigset_t) -> c_int;
}

/// Blocks every signal on the calling thread and returns the previous mask.
///
/// Threads inherit the mask of the thread that creates them, so blocking
/// around a spawn makes a worker that never takes a process signal; the
/// caller restores its own mask with [`restore_signal_mask`] afterwards.
///
/// Under Miri, which cannot call into the C library, this is a no-op: the
/// mask decides which thread takes a signal, not what memory is touched.
pub fn block_all_signals() -> sigset_t {
    if cfg!(miri) {
        return sigset_t::EMPTY;
    }
    let mut all = sigset_t::EMPTY;
    let mut previous = sigset_t::EMPTY;
    // SAFETY: both sets are valid, writable sigset_t values.
    unsafe {
        sigfillset(&raw mut all);
    }
    // SAFETY: as above; SIG_SETMASK with a full set cannot fail.
    unsafe {
        pthread_sigmask(SIG_SETMASK, &raw const all, &raw mut previous);
    }
    previous
}

/// Restores a mask returned by [`block_all_signals`].
pub fn restore_signal_mask(previous: &sigset_t) {
    if cfg!(miri) {
        return;
    }
    // SAFETY: `previous` is a valid sigset_t; the old-mask pointer may be null.
    unsafe {
        pthread_sigmask(SIG_SETMASK, previous, std::ptr::null_mut());
    }
}

/// Fills `buf` from the kernel's random source, retrying on interruption.
pub fn fill_random(buf: &mut [u8]) -> io::Result<()> {
    let mut done = 0;
    while done < buf.len() {
        let rest = &mut buf[done..];
        // SAFETY: `rest` is a valid, writable region of `rest.len()` bytes.
        let n = unsafe { getrandom(rest.as_mut_ptr().cast(), rest.len(), 0) };
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(EINTR) {
                continue;
            }
            return Err(err);
        }
        done += n.unsigned_abs();
    }
    Ok(())
}

/// Sends `sig` to a process (`pid` > 0) or a whole process group (`pid` < 0).
pub fn send_signal(pid: pid_t, sig: c_int) -> io::Result<()> {
    // SAFETY: kill takes plain integers and has no memory effects.
    if unsafe { kill(pid, sig) } == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// Nanoseconds on `CLOCK_MONOTONIC`, the clock DRM stamps vblank events
/// with, so the two can be compared directly.
pub fn monotonic_ns() -> u64 {
    let mut ts = timespec::default();
    // SAFETY: `ts` is a valid, writable timespec.
    if unsafe { clock_gettime(CLOCK_MONOTONIC, &raw mut ts) } != 0 {
        return 0;
    }
    u64::try_from(ts.tv_sec).unwrap_or(0) * 1_000_000_000 + u64::try_from(ts.tv_nsec).unwrap_or(0)
}

/// Waits for readiness on `fds` for at most `timeout_ms` (-1: forever).
/// Interruption by a signal reports zero ready descriptors.
pub fn poll_fds(fds: &mut [pollfd], timeout_ms: c_int) -> io::Result<usize> {
    let count = nfds_t::try_from(fds.len()).map_err(|_| io::Error::other("too many fds"))?;
    // SAFETY: `fds` is a valid array of `count` pollfd structures.
    let n = unsafe { poll(fds.as_mut_ptr(), count, timeout_ms) };
    if n < 0 {
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(EINTR) {
            return Ok(0);
        }
        return Err(err);
    }
    Ok(n.unsigned_abs() as usize)
}

/// Puts a descriptor into non-blocking mode.
pub fn set_nonblocking(fd: c_int) -> io::Result<()> {
    // SAFETY: F_GETFL takes no third argument and only reads the flags.
    let flags = unsafe { fcntl(fd, F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: F_SETFL takes an int argument, passed as one.
    if unsafe { fcntl(fd, F_SETFL, flags | O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The process's real user id.
pub fn current_uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { getuid() }
}

/// Opens a non-blocking TCP connection to `addr` whose kernel receive buffer
/// is set to `receive_buffer` before the connect -- so TCP window scaling is
/// negotiated for it, which std's connect cannot do -- waiting at most
/// `timeout_ms`. The returned stream is non-blocking.
pub fn tcp_connect(
    addr: &SocketAddr,
    receive_buffer: c_int,
    timeout_ms: u64,
) -> io::Result<TcpStream> {
    let (family, raw, len): (c_int, Vec<u8>, socklen_t) = match addr {
        SocketAddr::V4(a) => {
            let sa = sockaddr_in {
                sin_family: AF_INET as u16,
                sin_port: a.port().to_be(),
                sin_addr: a.ip().octets(),
                sin_zero: [0; 8],
            };
            (AF_INET, struct_bytes(&sa), size_of::<sockaddr_in>() as socklen_t)
        }
        SocketAddr::V6(a) => {
            let sa = sockaddr_in6 {
                sin6_family: AF_INET6 as u16,
                sin6_port: a.port().to_be(),
                sin6_flowinfo: a.flowinfo().to_be(),
                sin6_addr: a.ip().octets(),
                sin6_scope_id: a.scope_id(),
            };
            (AF_INET6, struct_bytes(&sa), size_of::<sockaddr_in6>() as socklen_t)
        }
    };
    // SAFETY: socket takes plain integers.
    let fd = unsafe { socket(family, SOCK_STREAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a fresh descriptor this function owns from here on.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    // Linux may double the request internally; a refusal is logged and the
    // connection goes ahead with the default buffer.
    // SAFETY: `receive_buffer` is a live int of the length passed.
    let set = unsafe {
        setsockopt(
            fd,
            SOL_SOCKET,
            SO_RCVBUF,
            (&raw const receive_buffer).cast(),
            size_of::<c_int>() as socklen_t,
        )
    };
    if set != 0 {
        eprintln!(
            "unable to set OMT receive buffer to {receive_buffer} bytes: {}",
            io::Error::last_os_error()
        );
    }
    // SAFETY: `raw` holds a sockaddr of `len` bytes for this family.
    if unsafe { connect(fd, raw.as_ptr().cast(), len) } != 0 {
        let err = io::Error::last_os_error();
        if err.raw_os_error() != Some(EINPROGRESS) {
            return Err(err);
        }
        let mut fds = [pollfd { fd, events: POLLOUT, revents: 0 }];
        let wait = c_int::try_from(timeout_ms).unwrap_or(c_int::MAX);
        loop {
            // SAFETY: `fds` is a valid one-element array.
            let rc = unsafe { poll(fds.as_mut_ptr(), 1, wait) };
            if rc < 0 && io::Error::last_os_error().raw_os_error() == Some(EINTR) {
                continue;
            }
            if rc == 0 {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "connection timed out"));
            }
            if rc < 0 {
                return Err(io::Error::last_os_error());
            }
            break;
        }
        let mut so_error: c_int = 0;
        let mut so_len = size_of::<c_int>() as socklen_t;
        // SAFETY: both out-pointers are live and sized for an int.
        let rc = unsafe {
            getsockopt(fd, SOL_SOCKET, SO_ERROR, (&raw mut so_error).cast(), &raw mut so_len)
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        if so_error != 0 {
            return Err(io::Error::from_raw_os_error(so_error));
        }
    }
    Ok(TcpStream::from(owned))
}

fn struct_bytes<T: Copy>(value: &T) -> Vec<u8> {
    let mut out = vec![0_u8; size_of::<T>()];
    // SAFETY: `out` holds size_of::<T>() bytes; T is a plain C struct with no
    // padding the kernel reads as meaningful.
    unsafe {
        std::ptr::copy_nonoverlapping(
            std::ptr::from_ref::<T>(value).cast::<u8>(),
            out.as_mut_ptr(),
            out.len(),
        );
    }
    out
}

/// A shared, writable memory mapping of a device, unmapped on drop.
#[derive(Debug)]
pub struct Mapping {
    ptr: *mut u8,
    len: usize,
}

// SAFETY: the mapping is owned by this handle; access goes through &self /
// &mut self like any owned buffer.
unsafe impl Send for Mapping {}

impl Mapping {
    /// Maps `len` bytes of `fd` at `offset`, readable and writable, shared.
    pub fn map_shared(fd: c_int, len: usize, offset: u64) -> io::Result<Self> {
        let offset = i64::try_from(offset).map_err(|_| io::Error::from_raw_os_error(22))?;
        // SAFETY: a fresh mapping at a kernel-chosen address touches no
        // existing memory.
        let ptr = unsafe {
            mmap(std::ptr::null_mut(), len, PROT_READ | PROT_WRITE, MAP_SHARED, fd, offset)
        };
        if ptr as isize == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { ptr: ptr.cast(), len })
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: the mapping is `len` bytes, writable, and exclusively
        // borrowed through `&mut self`.
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: the region was mapped by map_shared and is unmapped once.
        unsafe {
            munmap(self.ptr.cast(), self.len);
        }
    }
}

/// Raised by SIGINT or SIGTERM once [`install_stop_signals`] has run.
pub static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_stop_signal(_signal: c_int) {
    STOP_REQUESTED.store(true, Ordering::SeqCst);
}

/// Routes SIGINT and SIGTERM to [`STOP_REQUESTED`]. No SA_RESTART: a
/// blocking poll returns EINTR, so a signal is noticed promptly.
pub fn install_stop_signals() -> io::Result<()> {
    let mut action = sigaction_t {
        sa_handler: on_stop_signal as extern "C" fn(c_int) as usize,
        sa_mask: sigset_t::EMPTY,
        sa_flags: 0,
        sa_restorer: 0,
    };
    // SAFETY: the mask is a valid sigset_t inside `action`.
    unsafe {
        sigemptyset(&raw mut action.sa_mask);
    }
    for sig in [SIGINT, SIGTERM] {
        // SAFETY: `action` is a fully initialised sigaction; the handler only
        // stores to an atomic, which is async-signal-safe.
        if unsafe { sigaction(sig, &raw const action, std::ptr::null_mut()) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// An ioctl, retried on EINTR and EAGAIN as the DRM interfaces require.
///
/// # Safety
///
/// `arg` must point to the structure `request` expects, valid for the
/// kernel to read and write, and any pointers inside it must be valid for
/// the lengths they are passed with.
pub unsafe fn ioctl_retry(fd: c_int, request: u32, arg: *mut c_void) -> io::Result<()> {
    loop {
        // musl declares the request as int; the kernel reads the same 32
        // bits either way.
        // SAFETY: the caller's guarantee.
        let rc = unsafe { ioctl(fd, request.cast_signed(), arg) };
        if rc != -1 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        match err.raw_os_error() {
            Some(EINTR | EAGAIN) => {}
            _ => return Err(err),
        }
    }
}

// ---------------------------------------------------------------- terminal

/// Linux `struct termios` (the kernel's termios2 layout musl exposes): the
/// same 60 bytes on x86_64 and aarch64.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct termios {
    pub c_iflag: c_uint,
    pub c_oflag: c_uint,
    pub c_cflag: c_uint,
    pub c_lflag: c_uint,
    pub c_line: u8,
    pub c_cc: [u8; 32],
    pub c_ispeed: c_uint,
    pub c_ospeed: c_uint,
}

const _: () = assert!(size_of::<termios>() == 60);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct winsize {
    pub ws_row: u16,
    pub ws_col: u16,
    pub ws_xpixel: u16,
    pub ws_ypixel: u16,
}

pub const ISIG: c_uint = 0o1;
pub const ICANON: c_uint = 0o2;
pub const ECHO: c_uint = 0o10;
pub const ECHONL: c_uint = 0o100;
pub const IEXTEN: c_uint = 0o100_000;
pub const BRKINT: c_uint = 0o2;
pub const INPCK: c_uint = 0o20;
pub const ISTRIP: c_uint = 0o40;
pub const ICRNL: c_uint = 0o400;
pub const IXON: c_uint = 0o2000;
pub const OPOST: c_uint = 0o1;
pub const CS8: c_uint = 0o60;
pub const VTIME: usize = 5;
pub const VMIN: usize = 6;
pub const TCSANOW: c_int = 0;
pub const TCSAFLUSH: c_int = 2;
pub const TIOCGWINSZ: c_int = 0x5413;

unsafe extern "C" {
    pub fn tcgetattr(fd: c_int, t: *mut termios) -> c_int;
    pub fn tcsetattr(fd: c_int, action: c_int, t: *const termios) -> c_int;
}

/// The terminal settings of `fd`.
pub fn terminal_attrs(fd: c_int) -> io::Result<termios> {
    let mut t = termios::default();
    // SAFETY: `t` is a valid termios for the kernel to fill.
    if unsafe { tcgetattr(fd, &raw mut t) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(t)
}

pub fn set_terminal_attrs(fd: c_int, action: c_int, t: &termios) -> io::Result<()> {
    // SAFETY: `t` is a fully initialised termios.
    if unsafe { tcsetattr(fd, action, t) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The terminal's size in columns and rows.
pub fn window_size(fd: c_int) -> Option<(u16, u16)> {
    let mut ws = winsize::default();
    // SAFETY: TIOCGWINSZ writes one winsize, which `ws` is.
    let rc = unsafe { ioctl(fd, TIOCGWINSZ, &raw mut ws) };
    (rc == 0 && ws.ws_col > 0 && ws.ws_row > 0).then_some((ws.ws_col, ws.ws_row))
}

/// Prompts on /dev/tty and reads one line with echo off, without its
/// newline; `Ok(None)` once the line passes `limit` bytes.
pub fn read_secret_line(prompt: &str, limit: usize) -> io::Result<Option<Vec<u8>>> {
    use std::fs::OpenOptions;
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    const O_NOCTTY: c_int = 0o400;
    let mut tty =
        OpenOptions::new().read(true).write(true).custom_flags(O_NOCTTY).open("/dev/tty")?;
    let fd = tty.as_raw_fd();
    let saved = terminal_attrs(fd).ok();
    if let Some(saved) = saved {
        let mut quiet = saved;
        quiet.c_lflag &= !(ECHO | ECHONL);
        quiet.c_lflag |= ICANON;
        let _ = set_terminal_attrs(fd, TCSAFLUSH, &quiet);
    }
    // The prompt is a courtesy; reading still works without it.
    let _ = tty.write_all(prompt.as_bytes());
    let mut line = Vec::new();
    let mut too_long = false;
    let mut byte = [0_u8; 1];
    loop {
        match tty.read(&mut byte) {
            Ok(0) => break,
            Ok(_) if byte[0] == b'\n' => break,
            Ok(_) => {
                if line.len() >= limit {
                    too_long = true;
                    break;
                }
                line.push(byte[0]);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    if let Some(saved) = saved {
        let _ = set_terminal_attrs(fd, TCSAFLUSH, &saved);
    }
    let _ = tty.write_all(b"\n");
    Ok(if too_long { None } else { Some(line) })
}

// ------------------------------------------------------------ raw terminal

unsafe extern "C" {
    #[link_name = "signal"]
    fn c_signal(sig: c_int, handler: usize) -> usize;
    fn raise(sig: c_int) -> c_int;
    fn isatty(fd: c_int) -> c_int;
    fn read(fd: c_int, buf: *mut c_void, len: usize) -> ssize_t;
    fn write(fd: c_int, buf: *const c_void, len: usize) -> ssize_t;
}

const SIG_DFL: usize = 0;
const FATAL_SIGNALS: [c_int; 8] = [11, 7, 8, 4, 6, SIGTERM, 1, 3]; // SEGV BUS FPE ILL ABRT TERM HUP QUIT

/// Leaves the alternate screen with the cursor shown and attributes reset.
pub const TERMINAL_LEAVE_SEQUENCE: &[u8] = b"\x1b[0m\x1b[?25h\x1b[?1049l";
/// Enters the alternate screen with the cursor hidden.
pub const TERMINAL_ENTER_SEQUENCE: &[u8] = b"\x1b[?1049h\x1b[?25l\x1b[2J";

static TERMINAL_SAVED: std::sync::OnceLock<termios> = std::sync::OnceLock::new();
static TERMINAL_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Writes everything to a descriptor, retrying on EINTR; async-signal-safe.
pub fn write_fd_all(fd: c_int, mut data: &[u8]) {
    while !data.is_empty() {
        // SAFETY: `data` is readable for its length.
        let n = unsafe { write(fd, data.as_ptr().cast(), data.len()) };
        if n > 0 {
            data = &data[usize::try_from(n).unwrap_or(data.len()).min(data.len())..];
        } else if n == 0 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return;
        }
    }
}

/// Reads what is available on a descriptor (a raw read, no buffering).
pub fn read_fd(fd: c_int, buf: &mut [u8]) -> io::Result<usize> {
    loop {
        // SAFETY: `buf` is writable for its length.
        let n = unsafe { read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n >= 0 {
            return Ok(usize::try_from(n).unwrap_or(0));
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

pub fn is_tty(fd: c_int) -> bool {
    // SAFETY: isatty only inspects the descriptor.
    unsafe { isatty(fd) == 1 }
}

/// Restores the terminal saved by [`terminal_enter_raw`]. Safe to call
/// more than once, and from a signal handler: it uses only tcsetattr and
/// write.
pub fn terminal_leave() {
    if !TERMINAL_ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    if let Some(saved) = TERMINAL_SAVED.get() {
        // SAFETY: a fully initialised termios.
        unsafe { tcsetattr(0, TCSAFLUSH, saved) };
    }
    write_fd_all(1, TERMINAL_LEAVE_SEQUENCE);
}

extern "C" fn on_fatal_signal(sig: c_int) {
    terminal_leave();
    // SAFETY: restoring the default action and re-raising are
    // async-signal-safe; the process then dies as it would have.
    unsafe {
        c_signal(sig, SIG_DFL);
    }
    // SAFETY: as above.
    unsafe {
        raise(sig);
    }
}

/// Raw mode on the controlling terminal (no echo, no line discipline, no
/// signal keys, so Ctrl+C reaches the program), the alternate screen, and a
/// hidden cursor. The terminal is restored on every exit path the process
/// can take: [`terminal_leave`], a termination signal, and a crash.
pub fn terminal_enter_raw() -> io::Result<()> {
    let saved = terminal_attrs(0)?;
    let saved = *TERMINAL_SAVED.get_or_init(|| saved);
    let mut raw = saved;
    raw.c_iflag &= !(BRKINT | ICRNL | INPCK | ISTRIP | IXON);
    raw.c_oflag &= !OPOST;
    raw.c_cflag |= CS8;
    raw.c_lflag &= !(ECHO | ICANON | IEXTEN | ISIG);
    raw.c_cc[VMIN] = 0;
    raw.c_cc[VTIME] = 0;
    set_terminal_attrs(0, TCSAFLUSH, &raw)?;
    TERMINAL_ACTIVE.store(true, Ordering::SeqCst);
    for sig in FATAL_SIGNALS {
        // SAFETY: the handler only calls async-signal-safe functions.
        unsafe { c_signal(sig, on_fatal_signal as extern "C" fn(c_int) as usize) };
    }
    write_fd_all(1, TERMINAL_ENTER_SEQUENCE);
    Ok(())
}
