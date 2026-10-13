//! Win32 for the Windows deployer: what the standard library does not wrap.
//! Randomness (BCrypt), the ROOT certificate store, socket readiness
//! (WSAPoll), file identity, job objects that take a process tree down with
//! them, a no-echo console prompt, and the console the terminal application
//! draws on.
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
const WAIT_OBJECT_0: DWORD = 0;
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

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct COORD {
    pub X: i16,
    pub Y: i16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SMALL_RECT {
    pub Left: i16,
    pub Top: i16,
    pub Right: i16,
    pub Bottom: i16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CONSOLE_SCREEN_BUFFER_INFO {
    pub dwSize: COORD,
    pub dwCursorPosition: COORD,
    pub wAttributes: WORD,
    pub srWindow: SMALL_RECT,
    pub dwMaximumWindowSize: COORD,
}

/// The key half of INPUT_RECORD's event union; `uChar` is the UTF-16 unit.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct KEY_EVENT_RECORD {
    pub bKeyDown: BOOL,
    pub wRepeatCount: WORD,
    pub wVirtualKeyCode: WORD,
    pub wVirtualScanCode: WORD,
    pub uChar: u16,
    pub dwControlKeyState: DWORD,
}

/// An INPUT_RECORD read as a key event. Every other event type has the same
/// size (16 bytes) and is skipped by its tag without reading the body.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct INPUT_RECORD {
    pub EventType: WORD,
    pub Event: KEY_EVENT_RECORD,
}

const KEY_EVENT: WORD = 1;

type HandlerRoutine = unsafe extern "system" fn(DWORD) -> BOOL;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLastError() -> DWORD;
    fn CloseHandle(h: HANDLE) -> BOOL;
    fn GetStdHandle(which: DWORD) -> HANDLE;
    fn CreateFileW(
        name: *const u16,
        access: DWORD,
        share: DWORD,
        security: *mut c_void,
        disposition: DWORD,
        flags: DWORD,
        template: HANDLE,
    ) -> HANDLE;
    fn WriteFile(
        h: HANDLE,
        data: *const c_void,
        len: DWORD,
        wrote: *mut DWORD,
        overlapped: *mut c_void,
    ) -> BOOL;
    fn GetFileAttributesExW(name: *const u16, level: i32, out: *mut c_void) -> BOOL;
    fn CreateJobObjectW(security: *mut c_void, name: *const u16) -> HANDLE;
    fn SetInformationJobObject(job: HANDLE, class: i32, info: *mut c_void, len: DWORD) -> BOOL;
    fn AssignProcessToJobObject(job: HANDLE, process: HANDLE) -> BOOL;
    fn TerminateJobObject(job: HANDLE, code: u32) -> BOOL;
    fn WaitForSingleObject(h: HANDLE, ms: DWORD) -> DWORD;
    fn GetConsoleMode(h: HANDLE, mode: *mut DWORD) -> BOOL;
    fn SetConsoleMode(h: HANDLE, mode: DWORD) -> BOOL;
    fn GetConsoleOutputCP() -> u32;
    fn SetConsoleOutputCP(cp: u32) -> BOOL;
    fn SetConsoleCP(cp: u32) -> BOOL;
    fn SetConsoleCtrlHandler(handler: Option<HandlerRoutine>, add: BOOL) -> BOOL;
    fn GetConsoleScreenBufferInfo(h: HANDLE, info: *mut CONSOLE_SCREEN_BUFFER_INFO) -> BOOL;
    fn GetNumberOfConsoleInputEvents(h: HANDLE, n: *mut DWORD) -> BOOL;
    fn ReadConsoleInputW(h: HANDLE, out: *mut INPUT_RECORD, len: DWORD, got: *mut DWORD) -> BOOL;
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

const STD_INPUT_HANDLE: DWORD = -10_i32 as DWORD;
const STD_OUTPUT_HANDLE: DWORD = -11_i32 as DWORD;

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

fn std_handle(which: DWORD) -> HANDLE {
    // SAFETY: GetStdHandle takes a constant and returns a handle or
    // INVALID_HANDLE_VALUE; nothing is dereferenced.
    unsafe { GetStdHandle(which) }
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

/// The terminal application's console: virtual-terminal output (Windows 10
/// and later, conhost and Windows Terminal alike) and key records decoded
/// by virtual-key code, which reports F-keys and Ctrl+arrows the same on
/// every console host.
pub mod console {
    use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};

    use super::{
        BOOL, CONSOLE_SCREEN_BUFFER_INFO, CP_UTF8, DWORD, GetConsoleMode, GetConsoleOutputCP,
        GetConsoleScreenBufferInfo, GetNumberOfConsoleInputEvents, HANDLE, INPUT_RECORD, KEY_EVENT,
        KEY_EVENT_RECORD, ReadConsoleInputW, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
        SetConsoleCtrlHandler, SetConsoleMode, SetConsoleOutputCP, WAIT_OBJECT_0,
        WaitForSingleObject, WriteFile, std_handle,
    };

    /// Key codes, in the terminal application's `Code` order.
    pub const NONE: u8 = 0;
    pub const CHAR: u8 = 1;
    pub const ENTER: u8 = 2;
    pub const TAB: u8 = 3;
    pub const BACKTAB: u8 = 4;
    pub const ESC: u8 = 5;
    pub const BACKSPACE: u8 = 6;
    pub const DELETE: u8 = 7;
    pub const LEFT: u8 = 8;
    pub const RIGHT: u8 = 9;
    pub const UP: u8 = 10;
    pub const DOWN: u8 = 11;
    pub const HOME: u8 = 12;
    pub const END: u8 = 13;
    pub const PAGE_UP: u8 = 14;
    pub const PAGE_DOWN: u8 = 15;
    pub const F: u8 = 16;

    const ENTER_SEQUENCE: &[u8] = b"\x1b[?1049h\x1b[?25l\x1b[2J";
    const LEAVE_SEQUENCE: &[u8] = b"\x1b[0m\x1b[?25h\x1b[?1049l";

    const ENABLE_PROCESSED_OUTPUT: DWORD = 0x1;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: DWORD = 0x4;
    const DISABLE_NEWLINE_AUTO_RETURN: DWORD = 0x8;
    const ENABLE_WINDOW_INPUT: DWORD = 0x8;
    const ENABLE_EXTENDED_FLAGS: DWORD = 0x80;
    const CTRL_CLOSE_EVENT: DWORD = 2;
    const CTRL_LOGOFF_EVENT: DWORD = 5;
    const CTRL_SHUTDOWN_EVENT: DWORD = 6;
    const RIGHT_ALT_PRESSED: DWORD = 0x1;
    const LEFT_ALT_PRESSED: DWORD = 0x2;
    const RIGHT_CTRL_PRESSED: DWORD = 0x4;
    const LEFT_CTRL_PRESSED: DWORD = 0x8;
    const SHIFT_PRESSED: DWORD = 0x10;

    static ACTIVE: AtomicBool = AtomicBool::new(false);
    static SAVED_IN: AtomicU32 = AtomicU32::new(0);
    static SAVED_OUT: AtomicU32 = AtomicU32::new(0);
    static SAVED_CP: AtomicU32 = AtomicU32::new(0);
    static HIGH_SURROGATE: AtomicU16 = AtomicU16::new(0);

    fn input() -> HANDLE {
        std_handle(STD_INPUT_HANDLE)
    }

    fn output() -> HANDLE {
        std_handle(STD_OUTPUT_HANDLE)
    }

    pub fn write(mut data: &[u8]) {
        let out = output();
        while !data.is_empty() {
            let want = data.len().min(1 << 20);
            let mut wrote: DWORD = 0;
            // SAFETY: `data` is readable for `want` bytes.
            let ok = unsafe {
                WriteFile(
                    out,
                    data.as_ptr().cast(),
                    want as DWORD,
                    &raw mut wrote,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 || wrote == 0 {
                return;
            }
            data = &data[(wrote as usize).min(data.len())..];
        }
    }

    /// Restores the console. Safe to call more than once, and from the
    /// console control handler.
    pub fn leave() {
        if !ACTIVE.swap(false, Ordering::SeqCst) {
            return;
        }
        write(LEAVE_SEQUENCE);
        // SAFETY: restores modes read from these handles in `enter`.
        unsafe {
            SetConsoleMode(input(), SAVED_IN.load(Ordering::SeqCst));
        }
        // SAFETY: as above.
        unsafe {
            SetConsoleMode(output(), SAVED_OUT.load(Ordering::SeqCst));
        }
        let cp = SAVED_CP.load(Ordering::SeqCst);
        if cp != 0 {
            // SAFETY: restores the code page read in `enter`.
            unsafe {
                SetConsoleOutputCP(cp);
            }
        }
    }

    /// Closing the console window or logging off ends the process; restore
    /// the console first so a shared console is left usable.
    unsafe extern "system" fn on_console_event(kind: DWORD) -> BOOL {
        if matches!(kind, CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT) {
            leave();
        }
        0
    }

    /// Virtual-terminal output, raw key input, UTF-8, and the alternate
    /// screen; the error says what the operator should use instead.
    pub fn enter() -> Result<(), &'static str> {
        let (input, output) = (input(), output());
        let (mut saved_in, mut saved_out): (DWORD, DWORD) = (0, 0);
        // SAFETY: the handles are the process's standard handles; the modes
        // are written to locals.
        let consoles = unsafe { GetConsoleMode(input, &raw mut saved_in) } != 0
            // SAFETY: as above.
            && unsafe { GetConsoleMode(output, &raw mut saved_out) } != 0;
        if !consoles {
            return Err(
                "rpi-omt-deploy-tui needs an interactive console; use rpi-omt-deploy for scripts",
            );
        }
        let out_mode = saved_out
            | ENABLE_PROCESSED_OUTPUT
            | ENABLE_VIRTUAL_TERMINAL_PROCESSING
            | DISABLE_NEWLINE_AUTO_RETURN;
        // SAFETY: a mode change on the console's output handle.
        if unsafe { SetConsoleMode(output, out_mode) } == 0 {
            return Err(
                "this console does not support virtual-terminal output; use Windows Terminal or \
                 Windows 10 or later",
            );
        }
        SAVED_IN.store(saved_in, Ordering::SeqCst);
        SAVED_OUT.store(saved_out, Ordering::SeqCst);
        // No processed input: Ctrl+C arrives as a key and cancels a job
        // rather than killing the program. No line input or echo.
        // SAFETY: a mode change on the console's input handle.
        unsafe {
            SetConsoleMode(input, ENABLE_WINDOW_INPUT | ENABLE_EXTENDED_FLAGS);
        }
        // SAFETY: reads the console's output code page.
        SAVED_CP.store(unsafe { GetConsoleOutputCP() }, Ordering::SeqCst);
        // SAFETY: as in `set_console_utf8`.
        unsafe {
            SetConsoleOutputCP(CP_UTF8);
        }
        // SAFETY: the handler is a static function that only restores the
        // console.
        unsafe {
            SetConsoleCtrlHandler(Some(on_console_event), 1);
        }
        ACTIVE.store(true, Ordering::SeqCst);
        write(ENTER_SEQUENCE);
        Ok(())
    }

    /// The visible window in cells.
    pub fn size() -> Option<(u16, u16)> {
        let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
        // SAFETY: `info` receives the buffer description.
        if unsafe { GetConsoleScreenBufferInfo(output(), &raw mut info) } == 0 {
            return None;
        }
        let w = i32::from(info.srWindow.Right) - i32::from(info.srWindow.Left) + 1;
        let h = i32::from(info.srWindow.Bottom) - i32::from(info.srWindow.Top) + 1;
        Some((u16::try_from(w).ok().filter(|&w| w > 0)?, u16::try_from(h).ok().filter(|&h| h > 0)?))
    }

    /// One key record as (code, character, F-key number, ctrl).
    fn map_key(k: &KEY_EVENT_RECORD) -> Option<(u8, u32, u8, bool)> {
        let state = k.dwControlKeyState;
        let ctrl = state & (LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED) != 0;
        let alt = state & (LEFT_ALT_PRESSED | RIGHT_ALT_PRESSED) != 0;
        let shift = state & SHIFT_PRESSED != 0;
        let vk = k.wVirtualKeyCode;
        let code = match vk {
            0x26 => Some(UP),
            0x28 => Some(DOWN),
            0x25 => Some(LEFT),
            0x27 => Some(RIGHT),
            0x24 => Some(HOME),
            0x23 => Some(END),
            0x21 => Some(PAGE_UP),
            0x22 => Some(PAGE_DOWN),
            0x2E => Some(DELETE),
            0x08 => Some(BACKSPACE),
            0x09 => Some(if shift { BACKTAB } else { TAB }),
            0x0D => Some(ENTER),
            0x1B => Some(ESC),
            _ => None,
        };
        if let Some(code) = code {
            return Some((code, 0, 0, ctrl));
        }
        // VK_F1..VK_F12
        if (0x70..=0x7B).contains(&vk) {
            return Some((F, 0, (vk - 0x70 + 1) as u8, ctrl));
        }
        // Ctrl+letter, but not AltGr (reported as Ctrl+Alt), which types
        // characters on many layouts.
        if ctrl && !alt && (u16::from(b'A')..=u16::from(b'Z')).contains(&vk) {
            return Some((CHAR, u32::from(vk - u16::from(b'A') + u16::from(b'a')), 0, true));
        }
        let c = k.uChar;
        if c == 0 {
            return None;
        }
        if (0xD800..=0xDBFF).contains(&c) {
            HIGH_SURROGATE.store(c, Ordering::Relaxed);
            return None;
        }
        let mut cp = u32::from(c);
        if (0xDC00..=0xDFFF).contains(&c) {
            let high = HIGH_SURROGATE.swap(0, Ordering::Relaxed);
            if high == 0 {
                return None;
            }
            cp = 0x10000 + ((u32::from(high) - 0xD800) << 10) + (u32::from(c) - 0xDC00);
        }
        (cp >= 0x20).then_some((CHAR, cp, 0, false))
    }

    /// Waits up to `timeout_ms` for input and returns up to `max` keys.
    pub fn read_keys(max: usize, timeout_ms: u64) -> Vec<(u8, u32, u8, bool)> {
        let mut out = Vec::new();
        let input = input();
        let ms = u32::try_from(timeout_ms).unwrap_or(u32::MAX - 1);
        // SAFETY: waits on the console input handle.
        if unsafe { WaitForSingleObject(input, ms) } != WAIT_OBJECT_0 {
            return out;
        }
        let mut records = [INPUT_RECORD::default(); 32];
        while out.len() < max {
            let mut available: DWORD = 0;
            // SAFETY: `available` receives the count.
            let ok = unsafe { GetNumberOfConsoleInputEvents(input, &raw mut available) };
            if ok == 0 || available == 0 {
                break;
            }
            let mut got: DWORD = 0;
            // SAFETY: `records` is writable for 32 records.
            let ok = unsafe { ReadConsoleInputW(input, records.as_mut_ptr(), 32, &raw mut got) };
            if ok == 0 {
                break;
            }
            for r in &records[..(got as usize).min(records.len())] {
                if r.EventType != KEY_EVENT || r.Event.bKeyDown == 0 {
                    continue;
                }
                let Some(key) = map_key(&r.Event) else { continue };
                for _ in 0..r.Event.wRepeatCount.max(1) {
                    if out.len() >= max {
                        break;
                    }
                    out.push(key);
                }
            }
        }
        out
    }
}
