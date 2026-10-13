//! An immediate-mode cell buffer. A frame is drawn into it from scratch,
//! then compared with the previous one, and only the cells that changed are
//! written to the terminal.

use std::fmt::Write as _;

pub const COLOR_DEFAULT: u8 = 0;
pub const COLOR_BLACK: u8 = 1;
pub const COLOR_RED: u8 = 2;
pub const COLOR_GREEN: u8 = 3;
pub const COLOR_YELLOW: u8 = 4;
pub const COLOR_BLUE: u8 = 5;
pub const COLOR_MAGENTA: u8 = 6;
pub const COLOR_CYAN: u8 = 7;
pub const COLOR_WHITE: u8 = 8;

pub const ATTR_BOLD: u8 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub fg: u8,
    pub bg: u8,
    pub attr: u8,
}

pub const PLAIN: Style = Style { fg: COLOR_DEFAULT, bg: COLOR_DEFAULT, attr: 0 };

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    /// 0 marks the second half of a wide character.
    pub ch: u32,
    pub st: Style,
}

const BLANK: Cell = Cell { ch: b' ' as u32, st: PLAIN };

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Debug, Default)]
pub struct Screen {
    pub w: i32,
    pub h: i32,
    cells: Vec<Cell>,
    prev: Vec<Cell>,
    /// The previous frame is unknown: write everything.
    redraw: bool,
    pub cursor_visible: bool,
    cursor_x: i32,
    cursor_y: i32,
}

/// Columns a code point occupies: 0, 1, or 2.
pub fn char_width(cp: u32) -> i32 {
    // East Asian wide and fullwidth ranges, and emoji.
    const WIDE: [(u32, u32); 14] = [
        (0x1100, 0x115F),
        (0x2E80, 0x303E),
        (0x3041, 0x33FF),
        (0x3400, 0x4DBF),
        (0x4E00, 0x9FFF),
        (0xA000, 0xA4CF),
        (0xAC00, 0xD7A3),
        (0xF900, 0xFAFF),
        (0xFE30, 0xFE4F),
        (0xFF00, 0xFF60),
        (0xFFE0, 0xFFE6),
        (0x1F300, 0x1F64F),
        (0x1F900, 0x1F9FF),
        (0x20000, 0x3FFFD),
    ];
    if cp == 0 {
        return 0;
    }
    // Combining marks and zero-width format characters.
    if (0x0300..=0x036F).contains(&cp)
        || (0x200B..=0x200F).contains(&cp)
        || (0xFE00..=0xFE0F).contains(&cp)
    {
        return 0;
    }
    if WIDE.iter().any(|&(lo, hi)| (lo..=hi).contains(&cp)) { 2 } else { 1 }
}

fn is_control(cp: u32) -> bool {
    cp < 0x20 || (0x7F..0xA0).contains(&cp)
}

/// Display columns of text.
pub fn text_width(text: &str) -> i32 {
    text.chars().map(|c| if is_control(u32::from(c)) { 1 } else { char_width(u32::from(c)) }).sum()
}

impl Screen {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resizes for a new frame and blanks it.
    pub fn begin(&mut self, w: i32, h: i32) {
        let (w, h) = (w.clamp(1, 1000), h.clamp(1, 1000));
        if w != self.w || h != self.h || self.cells.is_empty() {
            let n = (w * h) as usize;
            self.cells = vec![BLANK; n];
            self.prev = vec![BLANK; n];
            self.w = w;
            self.h = h;
            self.redraw = true;
        }
        self.cells.fill(BLANK);
        self.cursor_visible = false;
    }

    pub fn put(&mut self, x: i32, y: i32, ch: u32, st: Style) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        self.cells[(y * self.w + x) as usize] = Cell { ch, st };
    }

    pub fn cell(&self, x: i32, y: i32) -> Option<Cell> {
        (x >= 0 && y >= 0 && x < self.w && y < self.h)
            .then(|| self.cells[(y * self.w + x) as usize])
    }

    /// Draws text on one row, clipped to `max_w` columns; returns the
    /// columns used. Control characters are drawn as U+FFFD.
    pub fn text(&mut self, x: i32, y: i32, max_w: i32, text: &str, st: Style) -> i32 {
        let mut used = 0;
        for c in text.chars() {
            if used >= max_w {
                break;
            }
            let mut cp = u32::from(c);
            if is_control(cp) {
                cp = 0xFFFD;
            }
            let w = char_width(cp);
            if w == 0 {
                continue;
            }
            if used + w > max_w {
                break;
            }
            self.put(x + used, y, cp, st);
            if w == 2 {
                self.put(x + used + 1, y, 0, st);
            }
            used += w;
        }
        used
    }

    /// As [`Screen::text`], over raw bytes: an invalid sequence draws one
    /// U+FFFD per byte that does not start a valid character.
    pub fn text_bytes(&mut self, x: i32, y: i32, max_w: i32, bytes: &[u8], st: Style) -> i32 {
        self.text(x, y, max_w, &String::from_utf8_lossy(bytes), st)
    }

    pub fn fill(&mut self, r: Rect, ch: u32, st: Style) {
        for y in r.y..r.y + r.h {
            for x in r.x..r.x + r.w {
                self.put(x, y, ch, st);
            }
        }
    }

    /// A box with an optional title on its top border; returns the inner
    /// area.
    pub fn block(&mut self, r: Rect, title: Option<&str>, border: Style) -> Rect {
        let inner = Rect { x: r.x + 1, y: r.y + 1, w: (r.w - 2).max(0), h: (r.h - 2).max(0) };
        if r.w < 1 || r.h < 1 {
            return inner;
        }
        for x in r.x + 1..r.x + r.w - 1 {
            self.put(x, r.y, 0x2500, border);
            self.put(x, r.y + r.h - 1, 0x2500, border);
        }
        for y in r.y + 1..r.y + r.h - 1 {
            self.put(r.x, y, 0x2502, border);
            self.put(r.x + r.w - 1, y, 0x2502, border);
        }
        self.put(r.x, r.y, 0x250C, border);
        self.put(r.x + r.w - 1, r.y, 0x2510, border);
        self.put(r.x, r.y + r.h - 1, 0x2514, border);
        self.put(r.x + r.w - 1, r.y + r.h - 1, 0x2518, border);
        if let Some(t) = title
            && r.w > 2
        {
            self.text(r.x + 1, r.y, r.w - 2, t, border);
        }
        inner
    }

    pub fn set_cursor(&mut self, x: i32, y: i32) {
        self.cursor_visible = x >= 0 && y >= 0 && x < self.w && y < self.h;
        self.cursor_x = x;
        self.cursor_y = y;
    }

    /// The VT bytes that turn the previous frame into this one.
    pub fn flush(&mut self, out: &mut String) {
        fn sgr(out: &mut String, st: Style) {
            out.push_str("\x1b[0");
            if st.attr & ATTR_BOLD != 0 {
                out.push_str(";1");
            }
            if st.fg != 0 {
                let _ = write!(out, ";{}", 29 + u32::from(st.fg));
            }
            if st.bg != 0 {
                let _ = write!(out, ";{}", 39 + u32::from(st.bg));
            }
            out.push('m');
        }
        out.push_str("\x1b[?25l");
        if self.redraw {
            out.push_str("\x1b[0m\x1b[2J");
        }
        let mut current: Option<Style> = None;
        let (mut cx, mut cy) = (-1, -1);
        for y in 0..self.h {
            for x in 0..self.w {
                let i = (y * self.w + x) as usize;
                let c = self.cells[i];
                if !self.redraw && c == self.prev[i] {
                    continue;
                }
                if c.ch == 0 {
                    continue; // covered by the wide character before it
                }
                if cx != x || cy != y {
                    let _ = write!(out, "\x1b[{};{}H", y + 1, x + 1);
                }
                if current != Some(c.st) {
                    sgr(out, c.st);
                    current = Some(c.st);
                }
                out.push(char::from_u32(c.ch).unwrap_or('\u{FFFD}'));
                cx = x + char_width(c.ch);
                cy = y;
            }
        }
        out.push_str("\x1b[0m");
        if self.cursor_visible {
            let _ = write!(out, "\x1b[{};{}H\x1b[?25h", self.cursor_y + 1, self.cursor_x + 1);
        }
        self.prev.clone_from(&self.cells);
        self.redraw = false;
    }

    /// Row y as text, for the tests.
    pub fn row_text(&self, y: i32) -> String {
        if y < 0 || y >= self.h {
            return String::new();
        }
        (0..self.w)
            .filter_map(|x| {
                self.cells[(y * self.w + x) as usize].ch.checked_sub(0).filter(|&c| c != 0)
            })
            .filter_map(char::from_u32)
            .collect()
    }
}
