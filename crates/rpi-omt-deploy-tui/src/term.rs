//! The terminal: raw input decoded into key events, and output written as
//! VT sequences. On POSIX this is termios on the controlling terminal; on
//! Windows the console with virtual-terminal processing, which Windows 10
//! and later support in both conhost and Windows Terminal.

use omt_common::{Error, Result};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Code {
    #[default]
    None,
    Char,
    Enter,
    Tab,
    BackTab,
    Esc,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    F,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Key {
    pub code: Code,
    /// `Char`: the code point, lower case for Ctrl+letter.
    pub ch: u32,
    /// `F`: 1-12.
    pub f: u8,
    pub ctrl: bool,
}

impl Key {
    pub fn of(code: Code) -> Self {
        Self { code, ..Self::default() }
    }

    pub fn char(ch: u32) -> Self {
        Self { code: Code::Char, ch, ..Self::default() }
    }

    pub fn ctrl(ch: char) -> Self {
        Self { code: Code::Char, ch: u32::from(ch), f: 0, ctrl: true }
    }

    pub fn f(n: u8) -> Self {
        Self { code: Code::F, f: n, ..Self::default() }
    }
}

/// A CSI or SS3 sequence starting at `data[0] == ESC`: its length and key,
/// `Some((0, _))` if incomplete, or `None` if it is not a sequence this
/// decoder knows.
fn escape(data: &[u8]) -> Option<(usize, Key)> {
    if data.len() < 2 {
        return Some((0, Key::default()));
    }
    let mut ev = Key::default();
    if data[1] == b'O' {
        if data.len() < 3 {
            return Some((0, ev));
        }
        ev.code = match data[2] {
            b'P' | b'Q' | b'R' | b'S' => {
                ev.f = data[2] - b'P' + 1;
                Code::F
            }
            b'H' => Code::Home,
            b'F' => Code::End,
            b'A' => Code::Up,
            b'B' => Code::Down,
            b'C' => Code::Right,
            b'D' => Code::Left,
            _ => Code::None,
        };
        return Some((3, ev));
    }
    if data[1] != b'[' {
        return None;
    }
    let mut params = [0_u32; 4];
    let mut n = 0;
    let mut i = 2;
    while i < data.len() && i < 32 {
        let b = data[i];
        if b.is_ascii_digit() {
            if params[n] < 1000 {
                params[n] = params[n] * 10 + u32::from(b - b'0');
            }
        } else if b == b';' {
            n = (n + 1).min(3);
        } else if (0x40..=0x7e).contains(&b) {
            break;
        } else {
            return Some((i + 1, ev)); // something odd: swallow it
        }
        i += 1;
    }
    if i >= data.len() {
        return Some((if data.len() >= 32 { data.len() } else { 0 }, ev));
    }
    // xterm modifiers: 1 + (shift 1, alt 2, ctrl 4).
    let modifier = if n >= 1 { params[1] } else { 0 };
    ev.ctrl = modifier >= 5 && (modifier - 1) & 4 != 0;
    ev.code = match data[i] {
        b'A' => Code::Up,
        b'B' => Code::Down,
        b'C' => Code::Right,
        b'D' => Code::Left,
        b'H' => Code::Home,
        b'F' => Code::End,
        b'Z' => Code::BackTab,
        b'P' => {
            ev.f = 1;
            Code::F
        }
        b'Q' => {
            ev.f = 2;
            Code::F
        }
        b'S' => {
            ev.f = 4;
            Code::F
        }
        b'~' => match params[0] {
            1 | 7 => Code::Home,
            4 | 8 => Code::End,
            3 => Code::Delete,
            5 => Code::PageUp,
            6 => Code::PageDown,
            p @ 11..=15 => {
                ev.f = (p - 10) as u8;
                Code::F
            }
            p @ 17..=21 => {
                ev.f = (p - 11) as u8;
                Code::F
            }
            p @ (23 | 24) => {
                ev.f = (p - 12) as u8;
                Code::F
            }
            _ => Code::None,
        },
        _ => Code::None,
    };
    Some((i + 1, ev))
}

fn utf8_length(lead: u8) -> usize {
    match lead {
        _ if lead < 0x80 => 1,
        _ if lead & 0xE0 == 0xC0 => 2,
        _ if lead & 0xF0 == 0xE0 => 3,
        _ if lead & 0xF8 == 0xF0 => 4,
        _ => 1,
    }
}

/// Decodes VT input bytes into at most `max` events. Returns the bytes
/// consumed; an incomplete escape sequence at the end is left unconsumed
/// unless `final_` says no more is coming.
pub fn decode(data: &[u8], final_: bool, max: usize, out: &mut Vec<Key>) -> usize {
    out.clear();
    let mut pos = 0;
    while pos < data.len() && out.len() < max {
        let b = data[pos];
        if b == 0x1b {
            match escape(&data[pos..]) {
                Some((0, _)) => {
                    if !final_ {
                        break;
                    }
                    out.push(Key::of(Code::Esc));
                    pos += 1;
                }
                // ESC followed by an ordinary key: report the Esc alone and
                // let the key decode on its own.
                None => {
                    out.push(Key::of(Code::Esc));
                    pos += 1;
                }
                Some((n, ev)) => {
                    if ev.code != Code::None {
                        out.push(ev);
                    }
                    pos += n;
                }
            }
            continue;
        }
        match b {
            b'\r' | b'\n' => out.push(Key::of(Code::Enter)),
            b'\t' => out.push(Key::of(Code::Tab)),
            0x7f | 0x08 => out.push(Key::of(Code::Backspace)),
            0x01..=0x1a => {
                out.push(Key { code: Code::Char, ch: u32::from(b'a' + b - 1), f: 0, ctrl: true });
            }
            0x20..0x7f => out.push(Key::char(u32::from(b))),
            0xc2.. => {
                let need = utf8_length(b);
                if pos + need > data.len() {
                    if !final_ {
                        break;
                    }
                    pos += 1;
                    continue;
                }
                let mut cp = u32::from(
                    b & if need == 2 {
                        0x1F
                    } else if need == 3 {
                        0x0F
                    } else {
                        0x07
                    },
                );
                let mut valid = true;
                for &c in &data[pos + 1..pos + need] {
                    valid &= c & 0xC0 == 0x80;
                    cp = (cp << 6) | u32::from(c & 0x3F);
                }
                if valid && (0xA0..=0x10_FFFF).contains(&cp) && !(0xD800..=0xDFFF).contains(&cp) {
                    out.push(Key::char(cp));
                }
                pos += if valid { need } else { 1 };
                continue;
            }
            _ => {}
        }
        pos += 1;
    }
    pos
}

/// The live terminal.
#[derive(Debug, Default)]
pub struct Terminal {
    /// Bytes read but not yet decoded: a VT sequence can arrive split. The
    /// Windows console delivers whole key records instead.
    #[cfg(unix)]
    pending: Vec<u8>,
}

#[cfg(unix)]
mod platform {
    use super::{Error, Result};
    use omt_sys::posix::{self, POLLIN, poll_fds, pollfd};

    pub(super) fn enter() -> Result<()> {
        if !posix::is_tty(0) || !posix::is_tty(1) {
            return Err(Error::new(
                "rpi-omt-deploy-tui needs an interactive terminal; use rpi-omt-deploy for scripts",
            ));
        }
        posix::terminal_enter_raw()
            .map_err(|e| Error::new(format!("cannot switch the terminal to raw mode: {e}")))
    }

    pub(super) fn leave() {
        posix::terminal_leave();
    }

    pub(super) fn size() -> Option<(u16, u16)> {
        posix::window_size(1)
    }

    pub(super) fn write(data: &[u8]) {
        posix::write_fd_all(1, data);
    }

    /// Appends what stdin has within `timeout_ms`; false when nothing came.
    pub(super) fn fill(pending: &mut Vec<u8>, timeout_ms: u64) -> bool {
        let mut fds = [pollfd { fd: 0, events: POLLIN, revents: 0 }];
        if !matches!(poll_fds(&mut fds, i32::try_from(timeout_ms).unwrap_or(0)), Ok(n) if n > 0) {
            return false;
        }
        let mut chunk = [0_u8; 256];
        let room = 256_usize.saturating_sub(pending.len()).min(chunk.len());
        match posix::read_fd(0, &mut chunk[..room]) {
            Ok(n) if n > 0 => {
                pending.extend_from_slice(&chunk[..n]);
                true
            }
            _ => false,
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::{Error, Result};
    pub(super) use omt_sys::win32::console::{leave, read_keys, size, write};

    pub(super) fn enter() -> Result<()> {
        omt_sys::win32::console::enter().map_err(Error::new)
    }
}

impl Terminal {
    /// Raw mode, the alternate screen, and a hidden cursor.
    pub fn enter() -> Result<Self> {
        platform::enter()?;
        Ok(Self::default())
    }

    /// Restores the terminal. Safe to call more than once.
    pub fn leave(&self) {
        platform::leave();
    }

    /// The window size in cells.
    pub fn size(&self) -> Option<(u16, u16)> {
        platform::size()
    }

    pub fn write(&self, data: &[u8]) {
        platform::write(data);
    }

    /// Waits up to `timeout_ms` for input and decodes what arrived.
    #[cfg(unix)]
    pub fn read_keys(&mut self, max: usize, timeout_ms: u64, out: &mut Vec<Key>) {
        out.clear();
        if self.pending.is_empty() && !platform::fill(&mut self.pending, timeout_ms) {
            return;
        }
        // Drain whatever else arrived with it.
        while self.pending.len() < 256 && platform::fill(&mut self.pending, 0) {}
        let mut used = decode(&self.pending, false, max, out);
        if used == 0 && out.is_empty() && !self.pending.is_empty() {
            // An escape that did not complete: a lone Esc, unless more of
            // the sequence arrives within a moment.
            if !platform::fill(&mut self.pending, 25) {
                used = decode(&self.pending, true, max, out);
            }
        }
        self.pending.drain(..used);
    }

    #[cfg(windows)]
    pub fn read_keys(&mut self, max: usize, timeout_ms: u64, out: &mut Vec<Key>) {
        out.clear();
        for (code, ch, f, ctrl) in platform::read_keys(max, timeout_ms) {
            out.push(Key { code: windows_code(code), ch, f, ctrl });
        }
    }
}

/// The console layer's key codes, in `Code` order.
#[cfg(windows)]
fn windows_code(code: u8) -> Code {
    use Code::{
        BackTab, Backspace, Char, Delete, Down, End, Enter, Esc, F, Home, Left, None, PageDown,
        PageUp, Right, Tab, Up,
    };
    [
        None, Char, Enter, Tab, BackTab, Esc, Backspace, Delete, Left, Right, Up, Down, Home, End,
        PageUp, PageDown, F,
    ]
    .get(usize::from(code))
    .copied()
    .unwrap_or(None)
}
