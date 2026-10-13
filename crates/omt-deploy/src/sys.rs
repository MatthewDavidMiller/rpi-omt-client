//! The deployer's operating-system layer. The deployer is the one program
//! that ships for both Linux and Windows; the standard library covers
//! sockets, threads, files, and processes on both, and what it does not --
//! the console, a whole-tree kill, and file identity -- is behind small
//! platform functions in omt-sys.

use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use omt_common::{Error, Result};

pub use omt_common::timefmt::now_ms;

/// A shared cancellation flag, set by a frontend and polled by the worker.
pub type Cancel = AtomicBool;

pub fn cancelled(flag: Option<&Cancel>) -> bool {
    flag.is_some_and(|f| f.load(Ordering::Relaxed))
}

pub fn cancelled_error() -> Error {
    Error::new("operation cancelled")
}

/// Fails with "operation cancelled" once the flag is set.
pub fn check_cancel(flag: Option<&Cancel>) -> Result<()> {
    if cancelled(flag) { Err(cancelled_error()) } else { Ok(()) }
}

/// "<what>: <OS error text>".
pub fn os_error(what: &str, e: &io::Error) -> Error {
    Error::new(format!("{what}: {e}"))
}

pub fn sleep_ms(ms: u64) {
    thread::sleep(Duration::from_millis(ms));
}

pub const ON_WINDOWS: bool = cfg!(windows);
pub const PATH_LIST_SEP: char = if ON_WINDOWS { ';' } else { ':' };

// ----------------------------------------------------------------- sockets

/// What one read produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Received {
    Data(usize),
    Eof,
    Timeout,
}

/// A connected, non-blocking TCP socket: every operation tries first and
/// waits in poll only when the kernel says it would block, as the C client
/// did, so steady traffic costs one system call per read or write.
#[derive(Debug)]
pub struct Sock(TcpStream);

fn duration(ms: u64) -> Duration {
    Duration::from_millis(ms.max(1))
}

impl Sock {
    /// Resolves `host` and connects to the first address that answers
    /// within `timeout_ms` overall.
    pub fn connect(host: &str, port: u16, timeout_ms: u64) -> Result<Self> {
        let addrs: Vec<_> = (host, port)
            .to_socket_addrs()
            .map_err(|e| {
                let text = e.to_string();
                let detail =
                    text.strip_prefix("failed to lookup address information: ").unwrap_or(&text);
                Error::new(format!("cannot resolve {host}: {detail}"))
            })?
            .collect();
        let deadline = now_ms() + timeout_ms;
        let mut last: Option<io::Error> = None;
        for addr in addrs {
            let now = now_ms();
            if now >= deadline {
                break;
            }
            match TcpStream::connect_timeout(&addr, duration(deadline - now)) {
                Ok(s) => {
                    let _ = s.set_nodelay(true);
                    s.set_nonblocking(true)
                        .map_err(|e| os_error(&format!("cannot connect to {host}:{port}"), &e))?;
                    return Ok(Self(s));
                }
                Err(e) => last = Some(e),
            }
        }
        match last {
            Some(e) if e.kind() != ErrorKind::TimedOut && e.kind() != ErrorKind::WouldBlock => {
                Err(os_error(&format!("cannot connect to {host}:{port}"), &e))
            }
            _ => Err(Error::new(format!("connecting to {host}:{port} timed out"))),
        }
    }

    /// Sends everything, waiting at most `timeout_ms` for the socket to
    /// drain.
    pub fn send(&mut self, data: &[u8], timeout_ms: u64) -> Result<()> {
        let deadline = now_ms() + timeout_ms;
        let mut rest = data;
        while !rest.is_empty() {
            match self.0.write(rest) {
                Ok(0) => return Err(Error::new("network write failed: connection closed")),
                Ok(n) => rest = &rest[n..],
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    let now = now_ms();
                    if now >= deadline || !wait_socket(&self.0, true, deadline - now)? {
                        return Err(Error::new("network write timed out"));
                    }
                }
                Err(e) => return Err(os_error("network write failed", &e)),
            }
        }
        Ok(())
    }

    /// Waits up to `timeout_ms` for data.
    pub fn recv(&mut self, buf: &mut [u8], timeout_ms: u64) -> Result<Received> {
        loop {
            match self.0.read(buf) {
                Ok(0) => return Ok(Received::Eof),
                Ok(n) => return Ok(Received::Data(n)),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    if !wait_socket(&self.0, false, timeout_ms)? {
                        return Ok(Received::Timeout);
                    }
                }
                Err(e) => return Err(os_error("network read failed", &e)),
            }
        }
    }

    pub fn shutdown(&self) {
        let _ = self.0.shutdown(Shutdown::Both);
    }

    /// The connected stream, for a layer (TLS) that drives it itself.
    pub fn into_inner(self) -> TcpStream {
        self.0
    }
}

/// Waits up to `timeout_ms` for a socket to become readable (or writable);
/// false when the wait expired.
pub fn wait_socket(s: &TcpStream, write: bool, timeout_ms: u64) -> Result<bool> {
    let ms = i32::try_from(timeout_ms).unwrap_or(i32::MAX);
    #[cfg(unix)]
    {
        use omt_sys::posix::{POLLIN, POLLOUT, poll_fds, pollfd};
        use std::os::fd::AsRawFd;
        let mut fds = [pollfd {
            fd: s.as_raw_fd(),
            events: if write { POLLOUT } else { POLLIN },
            revents: 0,
        }];
        poll_fds(&mut fds, ms).map(|n| n > 0).map_err(|e| os_error("network wait failed", &e))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawSocket;
        omt_sys::win32::wait_socket(s.as_raw_socket(), write, ms)
            .map_err(|e| os_error("network wait failed", &e))
    }
}

// ------------------------------------------------------------------- files

/// What a path is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathKind {
    Missing,
    File,
    Dir,
    /// A symbolic link or Windows reparse point, never followed.
    Link,
    Other,
    Error,
}

fn kind_of(m: io::Result<fs::Metadata>) -> PathKind {
    match m {
        Ok(m) if m.file_type().is_symlink() => PathKind::Link,
        Ok(m) if m.is_file() => PathKind::File,
        Ok(m) if m.is_dir() => PathKind::Dir,
        Ok(_) => PathKind::Other,
        Err(e) if e.kind() == ErrorKind::NotFound || e.kind() == ErrorKind::NotADirectory => {
            PathKind::Missing
        }
        Err(_) => PathKind::Error,
    }
}

/// What `path` itself is, without following a final link.
pub fn path_lstat(path: impl AsRef<Path>) -> PathKind {
    kind_of(fs::symlink_metadata(path))
}

/// What `path` resolves to, following links.
pub fn path_stat(path: impl AsRef<Path>) -> PathKind {
    kind_of(fs::metadata(path))
}

pub fn is_file(path: impl AsRef<Path>) -> bool {
    path_stat(path) == PathKind::File
}

pub fn is_dir(path: impl AsRef<Path>) -> bool {
    path_stat(path) == PathKind::Dir
}

/// Size of a regular file, following links.
pub fn file_size(path: impl AsRef<Path>) -> Option<u64> {
    fs::metadata(path).ok().filter(fs::Metadata::is_file).map(|m| m.len())
}

pub fn is_executable_file(path: impl AsRef<Path>) -> bool {
    let Ok(m) = fs::metadata(path) else { return false };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        m.is_file() && m.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        m.is_file()
    }
}

pub fn path_join(dir: &str, name: &str) -> String {
    let sep = if ON_WINDOWS { '\\' } else { '/' };
    let has_sep = dir.ends_with('/') || (ON_WINDOWS && dir.ends_with('\\'));
    if dir.is_empty() || has_sep { format!("{dir}{name}") } else { format!("{dir}{sep}{name}") }
}

pub fn open_read(path: &str) -> Result<File> {
    File::open(path).map_err(|e| os_error(path, &e))
}

/// Reads into `buf`; 0 at end of file.
pub fn read_some(f: &mut File, buf: &mut [u8]) -> Result<usize> {
    loop {
        match f.read(buf) {
            Ok(n) => return Ok(n),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(os_error("read failed", &e)),
        }
    }
}

/// Reads a whole file of at most `limit` bytes.
pub fn read_file(path: &str, limit: usize) -> Result<Vec<u8>> {
    let mut f = open_read(path)?;
    let mut out = Vec::new();
    let mut chunk = vec![0_u8; 16384];
    loop {
        let n = read_some(&mut f, &mut chunk)?;
        if n == 0 {
            return Ok(out);
        }
        if out.len() + n > limit {
            return Err(Error::new(format!("{path} is larger than {limit} bytes")));
        }
        out.extend_from_slice(&chunk[..n]);
    }
}

/// Creates or truncates `path`, writes `data`, and flushes it to the device.
pub fn write_file_sync(path: &str, data: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o644).custom_flags(omt_sys::posix::O_NOFOLLOW);
    }
    let mut f = options.open(path).map_err(|e| os_error(path, &e))?;
    f.write_all(data).map_err(|e| os_error(path, &e))?;
    f.sync_all().map_err(|e| os_error(path, &e))
}

/// The identity of a regular, non-link file: device, inode, size, and
/// modification time.
pub fn file_fingerprint(path: &str) -> Result<String> {
    let unsafe_path = || Error::new(format!("deployment artifact is missing or unsafe: {path}"));
    let m = fs::symlink_metadata(path).map_err(|_| unsafe_path())?;
    if !m.file_type().is_file() {
        return Err(unsafe_path());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let nanos = u64::try_from(m.mtime())
            .unwrap_or(0)
            .wrapping_mul(1_000_000_000)
            .wrapping_add(u64::try_from(m.mtime_nsec()).unwrap_or(0));
        Ok(format!("{}:{}:{}:{}", m.dev(), m.ino(), m.size(), nanos))
    }
    #[cfg(windows)]
    {
        omt_sys::win32::file_fingerprint(path).ok_or_else(unsafe_path)
    }
}

// ------------------------------------------------------------- environment

/// The variable's value, when it is set.
pub fn getenv(name: &str) -> Option<String> {
    std::env::var_os(name).map(|v| v.to_string_lossy().into_owned())
}

/// The current directory, or "." when it cannot be read.
pub fn current_dir() -> String {
    std::env::current_dir().map_or_else(|_| ".".to_owned(), |p| p.to_string_lossy().into_owned())
}

// --------------------------------------------------------------- processes

/// stdout then stderr, capped together.
pub const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub struct ProcessResult {
    pub exit_code: i32,
    /// stdout, then as much of stderr as still fits, lossily decoded.
    pub output: String,
}

/// Reads a stream to its end on a thread, keeping at most `OUTPUT_LIMIT`
/// bytes and draining the rest so the child never blocks on a full pipe.
fn drain<R: Read + Send + 'static>(mut stream: R) -> Option<thread::JoinHandle<Vec<u8>>> {
    thread::Builder::new()
        .name("deploy-pipe".into())
        .stack_size(64 * 1024)
        .spawn(move || {
            let mut kept = Vec::new();
            let mut chunk = [0_u8; 8192];
            loop {
                match stream.read(&mut chunk) {
                    Ok(0) => return kept,
                    Ok(n) => {
                        let room = OUTPUT_LIMIT.saturating_sub(kept.len());
                        kept.extend_from_slice(&chunk[..n.min(room)]);
                    }
                    Err(e) if e.kind() == ErrorKind::Interrupted => {}
                    Err(_) => return kept,
                }
            }
        })
        .ok()
}

/// Puts the child in its own process group (POSIX) or kill-on-close job
/// (Windows), so cancelling stops its descendants too.
fn isolate(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(omt_sys::win32::CREATE_NO_WINDOW);
    }
}

fn kill_tree(child: &mut Child, guard: Option<&TreeGuard>) {
    #[cfg(unix)]
    {
        // The group outlives the direct child while descendants run.
        if let Ok(pid) = i32::try_from(child.id()) {
            let _ = omt_sys::posix::send_signal(-pid, omt_sys::posix::SIGKILL);
        }
    }
    #[cfg(windows)]
    if let Some(g) = guard {
        g.job.terminate();
    }
    let _ = guard;
    let _ = child.kill();
    let _ = child.wait();
}

/// Keeps a Windows job object alive for the child's lifetime; nothing on
/// POSIX, where the process group does the same work.
#[derive(Debug)]
struct TreeGuard {
    #[cfg(windows)]
    job: omt_sys::win32::Job,
}

#[allow(clippy::unnecessary_wraps)] // fallible on Windows
fn guard_tree(child: &Child) -> Option<TreeGuard> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        let job = omt_sys::win32::Job::kill_on_close()?;
        job.assign(child.as_raw_handle())?;
        Some(TreeGuard { job })
    }
    #[cfg(not(windows))]
    {
        let _ = child;
        Some(TreeGuard {})
    }
}

/// Runs `program` with `args` in `directory`, stdin closed, both streams
/// captured. `env` adds or replaces variables in the inherited environment.
/// Cancelling kills the whole process tree.
pub fn run_process(
    program: &str,
    args: &[&str],
    directory: Option<&str>,
    env: &[(&str, &str)],
    cancel: Option<&Cancel>,
) -> Result<ProcessResult> {
    check_cancel(cancel)?;
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(dir) = directory {
        command.current_dir(dir);
    }
    for (k, v) in env {
        command.env(k, v);
    }
    isolate(&mut command);
    let mut child = command.spawn().map_err(|e| os_error(&format!("cannot run {program}"), &e))?;
    let guard = guard_tree(&child);
    let out = child.stdout.take().and_then(drain);
    let err = child.stderr.take().and_then(drain);
    // The streams are read to their end, not just to the child's exit: a
    // descendant can hold them open, and cancelling must still reach it.
    let mut exited = None;
    let status = loop {
        if cancelled(cancel) {
            kill_tree(&mut child, guard.as_ref());
            return Err(cancelled_error());
        }
        if exited.is_none() {
            match child.try_wait() {
                Ok(s) => exited = s,
                Err(e) => {
                    kill_tree(&mut child, guard.as_ref());
                    return Err(os_error(&format!("cannot wait for {program}"), &e));
                }
            }
        }
        let drained = out.as_ref().is_none_or(thread::JoinHandle::is_finished)
            && err.as_ref().is_none_or(thread::JoinHandle::is_finished);
        if let Some(s) = exited
            && drained
        {
            break s;
        }
        sleep_ms(25);
    };
    let mut bytes = out.and_then(|h| h.join().ok()).unwrap_or_default();
    let err_bytes = err.and_then(|h| h.join().ok()).unwrap_or_default();
    let room = OUTPUT_LIMIT.saturating_sub(bytes.len());
    bytes.extend_from_slice(&err_bytes[..err_bytes.len().min(room)]);
    Ok(ProcessResult {
        exit_code: status.code().unwrap_or(1),
        output: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

// ----------------------------------------------------------------- console

/// Windows: UTF-8 for the console's input and output, so progress and
/// prompts with non-ASCII text survive the console's starting code page.
#[cfg(windows)]
pub fn init_console() {
    omt_sys::win32::set_console_utf8();
}

/// Prompts on the controlling terminal and reads a line with echo off. The
/// line ending is removed.
pub fn prompt_secret(prompt: &str) -> Result<String> {
    #[cfg(unix)]
    let line = omt_sys::posix::read_secret_line(prompt, 4 * 1024 * 1024);
    #[cfg(windows)]
    let line = omt_sys::win32::read_secret_line(prompt, 4 * 1024 * 1024);
    match line {
        Ok(Some(mut bytes)) => {
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
            String::from_utf8(bytes).map_err(|_| Error::new("password input is not UTF-8"))
        }
        Ok(None) => Err(Error::new("password input is too long")),
        Err(e) => Err(os_error("cannot open the terminal for a password prompt", &e)),
    }
}

/// Reads standard input to end of stream; `None` once more than `limit`
/// bytes arrive.
pub fn read_stdin(limit: usize) -> Result<Option<Vec<u8>>> {
    let mut stdin = io::stdin().lock();
    let mut out = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        match stdin.read(&mut chunk) {
            Ok(0) => return Ok(Some(out)),
            Ok(n) => {
                if out.len() + n > limit {
                    omt_common::buf::wipe_vec(&mut out);
                    return Ok(None);
                }
                out.extend_from_slice(&chunk[..n]);
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(os_error("cannot read standard input", &e)),
        }
    }
}
