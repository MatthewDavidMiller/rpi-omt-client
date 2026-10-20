//! Win32 for the Windows deployer: what the standard library does not wrap.
//! Randomness (BCrypt), the ROOT certificate store, socket readiness
//! (WSAPoll), file identity, job objects that take a process tree down with
//! them, and a no-echo console prompt.
//!
//! Text crosses this boundary as UTF-8 and is converted to UTF-16 for the
//! wide APIs, so a path with non-ASCII characters works whatever the system
//! code page is.

use std::ffi::c_void;
use std::io;
use std::os::windows::io::RawHandle;
use std::os::windows::io::RawSocket;

pub type HANDLE = *mut c_void;
pub type BOOL = i32;
pub type DWORD = u32;
pub type WORD = u16;

pub const INVALID_HANDLE_VALUE: HANDLE = -1_isize as HANDLE;
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const GENERIC_READ: DWORD = 0x8000_0000;
const GENERIC_WRITE: DWORD = 0x4000_0000;
const FILE_SHARE_READ: DWORD = 1;
const FILE_SHARE_WRITE: DWORD = 2;
const OPEN_EXISTING: DWORD = 3;
const FILE_ATTRIBUTE_DIRECTORY: DWORD = 0x10;
const FILE_ATTRIBUTE_REPARSE_POINT: DWORD = 0x400;
const GET_FILE_EX_INFO_STANDARD: i32 = 0;
const CP_UTF8: u32 = 65001;

const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 2;

const POLLRDNORM: i16 = 0x0100;
const POLLWRNORM: i16 = 0x0010;

const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: i32 = 9;
const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: DWORD = 0x2000;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct FILETIME {
    pub dwLowDateTime: DWORD,
    pub dwHighDateTime: DWORD,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct WIN32_FILE_ATTRIBUTE_DATA {
    pub dwFileAttributes: DWORD,
    pub ftCreationTime: FILETIME,
    pub ftLastAccessTime: FILETIME,
    pub ftLastWriteTime: FILETIME,
    pub nFileSizeHigh: DWORD,
    pub nFileSizeLow: DWORD,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct WSAPOLLFD {
    pub fd: usize,
    pub events: i16,
    pub revents: i16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct JOBOBJECT_BASIC_LIMIT_INFORMATION {
    pub PerProcessUserTimeLimit: i64,
    pub PerJobUserTimeLimit: i64,
    pub LimitFlags: DWORD,
    pub MinimumWorkingSetSize: usize,
    pub MaximumWorkingSetSize: usize,
    pub ActiveProcessLimit: DWORD,
    pub Affinity: usize,
    pub PriorityClass: DWORD,
    pub SchedulingClass: DWORD,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct IO_COUNTERS {
    pub ReadOperationCount: u64,
    pub WriteOperationCount: u64,
    pub OtherOperationCount: u64,
    pub ReadTransferCount: u64,
    pub WriteTransferCount: u64,
    pub OtherTransferCount: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
    pub BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION,
    pub IoInfo: IO_COUNTERS,
    pub ProcessMemoryLimit: usize,
    pub JobMemoryLimit: usize,
    pub PeakProcessMemoryUsed: usize,
    pub PeakJobMemoryUsed: usize,
}

#[repr(C)]
#[derive(Debug)]
pub struct CERT_CONTEXT {
    pub dwCertEncodingType: DWORD,
    pub pbCertEncoded: *const u8,
    pub cbCertEncoded: DWORD,
    pub pCertInfo: *mut c_void,
    pub hCertStore: HANDLE,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLastError() -> DWORD;
    fn CloseHandle(h: HANDLE) -> BOOL;
    fn CreateFileW(
        name: *const u16,
        access: DWORD,
        share: DWORD,
        security: *mut c_void,
        disposition: DWORD,
        flags: DWORD,
        template: HANDLE,
    ) -> HANDLE;
    fn GetFileAttributesExW(name: *const u16, level: i32, out: *mut c_void) -> BOOL;
    fn CreateJobObjectW(security: *mut c_void, name: *const u16) -> HANDLE;
    fn SetInformationJobObject(job: HANDLE, class: i32, info: *mut c_void, len: DWORD) -> BOOL;
    fn AssignProcessToJobObject(job: HANDLE, process: HANDLE) -> BOOL;
    fn TerminateJobObject(job: HANDLE, code: u32) -> BOOL;
    fn GetConsoleMode(h: HANDLE, mode: *mut DWORD) -> BOOL;
    fn SetConsoleMode(h: HANDLE, mode: DWORD) -> BOOL;
    fn SetConsoleOutputCP(cp: u32) -> BOOL;
    fn SetConsoleCP(cp: u32) -> BOOL;
    fn WriteConsoleW(
        h: HANDLE,
        text: *const u16,
        len: DWORD,
        wrote: *mut DWORD,
        reserved: *mut c_void,
    ) -> BOOL;
    fn ReadConsoleW(
        h: HANDLE,
        out: *mut u16,
        len: DWORD,
        got: *mut DWORD,
        control: *mut c_void,
    ) -> BOOL;
}

#[link(name = "ws2_32")]
unsafe extern "system" {
    fn WSAPoll(fds: *mut WSAPOLLFD, n: u32, timeout: i32) -> i32;
    fn WSAGetLastError() -> i32;
}

#[link(name = "bcrypt")]
unsafe extern "system" {
    fn BCryptGenRandom(alg: *mut c_void, out: *mut u8, len: u32, flags: u32) -> i32;
}

#[link(name = "crypt32")]
unsafe extern "system" {
    fn CertOpenSystemStoreW(provider: usize, name: *const u16) -> HANDLE;
    fn CertEnumCertificatesInStore(store: HANDLE, prev: *const CERT_CONTEXT)
    -> *const CERT_CONTEXT;
    fn CertCloseStore(store: HANDLE, flags: DWORD) -> BOOL;
}

fn last_error() -> io::Error {
    // SAFETY: GetLastError reads the calling thread's error slot.
    let code = unsafe { GetLastError() };
    io::Error::from_raw_os_error(code as i32)
}

/// UTF-16 with a terminating NUL; `None` when the text holds a NUL itself.
fn wide(text: &str) -> Option<Vec<u16>> {
    if text.contains('\0') {
        return None;
    }
    Some(text.encode_utf16().chain(std::iter::once(0)).collect())
}

/// An owned kernel handle.
#[derive(Debug)]
struct Owned(HANDLE);

impl Owned {
    fn new(h: HANDLE) -> Option<Self> {
        (!h.is_null() && h != INVALID_HANDLE_VALUE).then_some(Self(h))
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the handle is owned and closed exactly once, here.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

// SAFETY: a kernel handle is a process-wide value usable from any thread.
unsafe impl Send for Owned {}
// SAFETY: the handle is only passed to thread-safe kernel calls.
unsafe impl Sync for Owned {}

// ------------------------------------------------------------- randomness

/// Fills `buf` from the system's preferred random generator.
pub fn fill_random(buf: &mut [u8]) -> io::Result<()> {
    for chunk in buf.chunks_mut(0x1000_0000) {
        // SAFETY: `chunk` is writable for its length, which fits in a u32.
        let status = unsafe {
            BCryptGenRandom(
                std::ptr::null_mut(),
                chunk.as_mut_ptr(),
                chunk.len() as u32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if status != 0 {
            return Err(io::Error::other(format!("BCryptGenRandom failed (0x{status:08x})")));
        }
    }
    Ok(())
}

// ------------------------------------------------------------ trust roots

/// Calls `each` with every certificate in the machine's ROOT store, as DER.
pub fn for_each_root_certificate(mut each: impl FnMut(&[u8])) -> io::Result<()> {
    let name = wide("ROOT").unwrap_or_default();
    // SAFETY: `name` is a NUL-terminated UTF-16 string.
    let raw = unsafe { CertOpenSystemStoreW(0, name.as_ptr()) };
    if raw.is_null() {
        return Err(last_error());
    }
    let mut cert: *const CERT_CONTEXT = std::ptr::null();
    loop {
        // SAFETY: `raw` is an open store and `cert` is null or the previous
        // context it returned, which this call releases.
        cert = unsafe { CertEnumCertificatesInStore(raw, cert) };
        if cert.is_null() {
            break;
        }
        // SAFETY: a non-null context from the store is valid until the next
        // enumeration call.
        let context = unsafe { &*cert };
        if context.pbCertEncoded.is_null() {
            continue;
        }
        // SAFETY: the context owns `cbCertEncoded` bytes at `pbCertEncoded`.
        let der = unsafe {
            std::slice::from_raw_parts(context.pbCertEncoded, context.cbCertEncoded as usize)
        };
        each(der);
    }
    // SAFETY: the store is open and the enumeration has released its last
    // context.
    unsafe {
        CertCloseStore(raw, 0);
    }
    Ok(())
}

// ---------------------------------------------------------------- sockets

/// Waits for a socket to become readable or writable; false on timeout.
pub fn wait_socket(socket: RawSocket, write: bool, timeout_ms: i32) -> io::Result<bool> {
    let mut fd = WSAPOLLFD {
        fd: socket as usize,
        events: if write { POLLWRNORM } else { POLLRDNORM },
        revents: 0,
    };
    // SAFETY: one valid WSAPOLLFD.
    let rc = unsafe { WSAPoll(&raw mut fd, 1, timeout_ms) };
    if rc < 0 {
        // SAFETY: reads the calling thread's Winsock error.
        let code = unsafe { WSAGetLastError() };
        return Err(io::Error::from_raw_os_error(code));
    }
    Ok(rc > 0)
}

// ------------------------------------------------------------------ files

/// Size and modification time: Windows has no device and inode pair to
/// add. `None` for a directory, a reparse point, or a missing path.
pub fn file_fingerprint(path: &str) -> Option<String> {
    let name = wide(path)?;
    let mut data = WIN32_FILE_ATTRIBUTE_DATA::default();
    // SAFETY: `name` is NUL-terminated and `data` is the standard-info
    // structure the level names.
    let ok = unsafe {
        GetFileAttributesExW(name.as_ptr(), GET_FILE_EX_INFO_STANDARD, (&raw mut data).cast())
    };
    if ok == 0
        || data.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0
    {
        return None;
    }
    let size = (u64::from(data.nFileSizeHigh) << 32) | u64::from(data.nFileSizeLow);
    let ticks = (u64::from(data.ftLastWriteTime.dwHighDateTime) << 32)
        | u64::from(data.ftLastWriteTime.dwLowDateTime);
    Some(format!("{size}:{ticks}"))
}

// -------------------------------------------------------------- processes

/// A job object that kills every process in it when the last handle
/// closes, so a cancelled build takes its whole tree down.
#[derive(Debug)]
pub struct Job(Owned);

impl Job {
    pub fn kill_on_close() -> Option<Self> {
        // SAFETY: an anonymous job with default security.
        let job = Owned::new(unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) })?;
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `limits` is the structure the information class names, and
        // its size is passed with it.
        let ok = unsafe {
            SetInformationJobObject(
                job.0,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
                (&raw mut limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as DWORD,
            )
        };
        (ok != 0).then_some(Self(job))
    }

    /// Adds a process; its children join the job as they start.
    pub fn assign(&self, process: RawHandle) -> Option<()> {
        // SAFETY: both handles are open; the process handle is borrowed.
        let ok = unsafe { AssignProcessToJobObject(self.0.0, process.cast()) };
        (ok != 0).then_some(())
    }

    pub fn terminate(&self) {
        // SAFETY: the job handle is open.
        unsafe {
            TerminateJobObject(self.0.0, 1);
        }
    }
}

// ---------------------------------------------------------------- console

/// UTF-8 for the console's input and output, so non-ASCII text survives
/// whatever code page the console started in.
pub fn set_console_utf8() {
    // SAFETY: plain setters on the process's console; failure (no console)
    // is harmless.
    unsafe {
        SetConsoleOutputCP(CP_UTF8);
    }
    // SAFETY: as above.
    unsafe {
        SetConsoleCP(CP_UTF8);
    }
}

fn open_console(name: &str) -> Option<Owned> {
    let name = wide(name)?;
    // SAFETY: `name` is NUL-terminated; the other arguments are constants.
    let h = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    Owned::new(h)
}

fn write_console(h: HANDLE, text: &[u16]) {
    let mut wrote = 0;
    // SAFETY: `text` is readable for its length.
    unsafe {
        WriteConsoleW(h, text.as_ptr(), text.len() as DWORD, &raw mut wrote, std::ptr::null_mut());
    }
}

/// Prompts on the console and reads one line with echo off. `Ok(None)` when
/// the line is longer than `limit` bytes. The line ending is removed.
pub fn read_secret_line(prompt: &str, limit: usize) -> io::Result<Option<Vec<u8>>> {
    let input = open_console("CONIN$").ok_or_else(last_error)?;
    let output = open_console("CONOUT$").ok_or_else(last_error)?;
    let mut mode: DWORD = 0;
    // SAFETY: `input` is an open console handle; `mode` receives its mode.
    let restore = unsafe { GetConsoleMode(input.0, &raw mut mode) } != 0;
    if restore {
        const ENABLE_PROCESSED_INPUT: DWORD = 1;
        const ENABLE_LINE_INPUT: DWORD = 2;
        const ENABLE_ECHO_INPUT: DWORD = 4;
        // SAFETY: as above.
        unsafe {
            SetConsoleMode(
                input.0,
                (mode | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT) & !ENABLE_ECHO_INPUT,
            );
        }
    }
    let prompt: Vec<u16> = prompt.encode_utf16().collect();
    write_console(output.0, &prompt);
    let mut buffer = vec![0_u16; 4096];
    let mut got: DWORD = 0;
    // SAFETY: `buffer` is writable for 4096 units.
    let ok = unsafe {
        ReadConsoleW(input.0, buffer.as_mut_ptr(), 4096, &raw mut got, std::ptr::null_mut())
    } != 0;
    let mut n = (got as usize).min(buffer.len());
    while n > 0 && (buffer[n - 1] == u16::from(b'\n') || buffer[n - 1] == u16::from(b'\r')) {
        n -= 1;
    }
    let line = String::from_utf16_lossy(&buffer[..n]).into_bytes();
    buffer.fill(0);
    std::hint::black_box(&buffer);
    if restore {
        // SAFETY: restores the mode read above.
        unsafe {
            SetConsoleMode(input.0, mode);
        }
    }
    write_console(output.0, &[u16::from(b'\r'), u16::from(b'\n')]);
    if !ok {
        return Err(last_error());
    }
    Ok((line.len() <= limit).then_some(line))
}
