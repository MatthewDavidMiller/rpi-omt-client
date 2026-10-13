//! The terminal deployer's views, drawn into a screen buffer each frame.

use std::fmt::Write as _;

use omt_common::VERSION;
use omt_deploy::capsule::Capsule;
use omt_deploy::core::IMAGE_MEMBER;

use crate::app::{App, Kind, VIEWS, View};
use crate::screen::{ATTR_BOLD, COLOR_BLACK, COLOR_CYAN, COLOR_YELLOW, PLAIN, Rect, Screen, Style};

pub const UI_MIN_WIDTH: i32 = 40;
pub const UI_MIN_HEIGHT: i32 = 11;

/// Terminals vary in how many colours they honour, so emphasis carries
/// through bold as well as colour: a monochrome console still shows focus.
const FOCUS: Style = Style { fg: COLOR_BLACK, bg: COLOR_CYAN, attr: ATTR_BOLD };
const BOLD: Style = Style { fg: 0, bg: 0, attr: ATTR_BOLD };
const WARNING: Style = Style { fg: COLOR_YELLOW, bg: 0, attr: 0 };

fn chars(text: &str) -> usize {
    text.chars().count()
}

/// The first `n` characters.
fn prefix(text: &str, n: usize) -> &str {
    &text[..text.char_indices().nth(n).map_or(text.len(), |(i, _)| i)]
}

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\x0c' | '\x0b')
}

fn dim(v: i32) -> usize {
    usize::try_from(v).unwrap_or(0)
}

fn coord(v: usize) -> i32 {
    i32::try_from(v).unwrap_or(i32::MAX)
}

/// Text cut to `width` characters, a cut marked with `~`.
pub fn truncate(text: &str, width: usize) -> String {
    if chars(text) <= width {
        return text.to_owned();
    }
    let mut out = prefix(text, width.saturating_sub(1)).to_owned();
    out.push('~');
    out
}

/// Breaks a word too long for the area into pieces that fit: the capsule
/// digest is 64 characters, and broken across rows it can still be read.
fn word_pieces(word: &str, width: usize) -> Vec<&str> {
    let width = width.max(1);
    let mut pieces = Vec::new();
    let mut rest = word;
    while !rest.is_empty() {
        let take = prefix(rest, width);
        pieces.push(take);
        rest = &rest[take.len()..];
    }
    pieces
}

/// Wraps text to `width`, keeping each source line's indentation. A line
/// that already fits is kept exactly, so the key legend's columns survive.
pub fn wrap(text: &str, width: usize, out: &mut Vec<String>) {
    let width = width.max(1);
    let mut rest = text;
    while !rest.is_empty() {
        let (src, next) = match rest.find('\n') {
            Some(nl) => (&rest[..nl], &rest[nl + 1..]),
            None => (rest, ""),
        };
        rest = next;
        let src = src.strip_suffix('\r').unwrap_or(src);
        if chars(src) <= width {
            out.push(src.to_owned());
            continue;
        }
        let mut indent = src.len() - src.trim_start_matches(is_space).len();
        if indent >= width {
            indent = 0;
        }
        let lead = &src[..indent];
        let margin = indent;
        let mut line = lead.to_owned();
        for word in src[indent..].split(is_space).filter(|w| !w.is_empty()) {
            for piece in word_pieces(word, width - margin) {
                let filled = chars(&line);
                let occupied = filled > margin;
                if occupied && filled + 1 + chars(piece) > width {
                    out.push(std::mem::replace(&mut line, lead.to_owned()));
                } else if occupied {
                    line.push(' ');
                }
                line.push_str(piece);
            }
        }
        out.push(line);
    }
}

fn trim_tail(text: &str) -> &str {
    text.trim_end_matches(|c| c == '\n' || is_space(c))
}

/// The About document: the licence and the third-party notices, which the
/// single-file deployer is the only copy of an operator receives.
pub fn about_text(capsule: &Capsule, width: usize, out: &mut Vec<String>) {
    let mut head = format!("Raspberry Pi OMT client deployer {VERSION}\n\n");
    // The digest is the point: a single-file deployer is otherwise opaque
    // about which appliance build it carries.
    match capsule.report() {
        Ok(report) => head.push_str(&report),
        Err(_) => {
            let _ = write!(head, "This deployer was built without {IMAGE_MEMBER}.");
        }
    }
    head.push_str(
        "\n\nEverything this deploys is compiled in; no checkout is needed.\n\n\
         Keys:\n\
         \x20 F1-F8 / Ctrl+Left / Ctrl+Right   switch view\n\
         \x20 Tab / Shift+Tab                  move between fields\n\
         \x20 Enter                            toggle, or run the focused action\n\
         \x20 y / n                            answer a confirmation\n\
         \x20 Ctrl+R                           reveal or hide secrets\n\
         \x20 Esc                              cancel a running job\n\
         \x20 PageUp / PageDown / Up / Down    scroll Activity and About\n\
         \x20 Home / End                       jump to the ends of this view\n\
         \x20 Ctrl+Q                           quit, asking while a job runs",
    );
    wrap(&head, width, out);
    wrap("\nLICENSE\n-------", width, out);
    wrap(trim_tail(capsule.license), width, out);
    wrap("\nTHIRD-PARTY NOTICES\n-------------------", width, out);
    wrap(trim_tail(capsule.notices), width, out);
}

fn draw_tabs(s: &mut Screen, area: Rect, a: &App) {
    let title = format!(" Raspberry Pi OMT deployer {VERSION} ");
    let inner = s.block(area, Some(&title), PLAIN);
    let mut x = inner.x;
    let end = inner.x + inner.w;
    for (i, &v) in VIEWS.iter().enumerate() {
        if x >= end {
            break;
        }
        if i > 0 {
            s.put(x, inner.y, 0x2502, PLAIN);
            x += 1;
        }
        let label = format!("F{} {}", i + 1, v.title());
        let st = if v == a.current { FOCUS } else { PLAIN };
        s.put(x, inner.y, u32::from(' '), PLAIN);
        x += 1;
        x += s.text(x, inner.y, end - x, &label, st);
        if x < end {
            s.put(x, inner.y, u32::from(' '), PLAIN);
        }
        x += 1;
    }
}

/// The first form row shown so the focused one is visible.
pub fn first_visible_slot(focus: usize, count: usize, visible: usize) -> usize {
    let last_page = count.saturating_sub(visible);
    let first = focus.saturating_sub(visible.saturating_sub(1));
    first.min(last_page)
}

/// The label column: half the width, within 12..=32.
pub fn label_width(width: i32) -> usize {
    (dim(width) / 2).clamp(12, 32)
}

fn value_scroll(cursor: usize, columns: usize) -> usize {
    cursor.saturating_sub(columns.saturating_sub(1))
}

/// One row per slot, only the rows that fit, the window following the
/// focus. Rows laid out unconditionally used to vanish while staying
/// focusable, so a short terminal offered an invisible button that erases
/// the Pi's disk.
fn draw_form(s: &mut Screen, area: Rect, a: &App) {
    let slots = a.current.slots();
    let count = slots.len();
    let inner = Rect { x: area.x + 1, y: area.y + 1, w: area.w - 2, h: area.h - 2 };
    let visible = dim(inner.h).max(1).min(count);
    let first = first_visible_slot(a.focus, count, visible);
    let title = if visible < count {
        format!(" {} (fields {}-{} of {count}) ", a.current.title(), first + 1, first + visible)
    } else {
        format!(" {} ", a.current.title())
    };
    s.block(area, Some(&title), PLAIN);
    let gutter = label_width(inner.w);
    let columns = dim(inner.w).saturating_sub(gutter);
    let mut value = String::new();
    for (offset, &sl) in
        slots.iter().enumerate().skip(first).take(visible).map(|(i, s)| (i - first, s))
    {
        if coord(offset) >= inner.h {
            break;
        }
        let index = first + offset;
        let focused = index == a.focus;
        let scroll = if focused { value_scroll(a.cursor, columns) } else { 0 };
        let y = inner.y + coord(offset);
        let mut label = String::from(if focused { "> " } else { "  " });
        label.push_str(&truncate(sl.label(), gutter - 2));
        let pad = (gutter - 2).saturating_sub(chars(sl.label()));
        label.extend(std::iter::repeat_n(' ', pad));
        let used = s.text(inner.x, y, inner.w, &label, if focused { FOCUS } else { PLAIN });
        value.clear();
        let v = a.value(sl);
        match sl.kind() {
            Kind::Text => value.push_str(v),
            Kind::Secret if a.reveal => value.push_str(v),
            Kind::Secret => value.extend(std::iter::repeat_n('*', chars(v))),
            Kind::Toggle => value.push_str(if a.toggle(sl) { "[x]" } else { "[ ]" }),
            Kind::Button => {
                value.push_str(if a.busy() { "[ running... ]" } else { "[ press Enter ]" });
            }
        }
        let skip = prefix(&value, scroll).len();
        s.text(inner.x + used, y, inner.w - used, &value[skip..], PLAIN);
        if focused && matches!(sl.kind(), Kind::Text | Kind::Secret) {
            // The operator's own terminal shows the insertion point.
            s.set_cursor(inner.x + coord(gutter + a.cursor - scroll), y);
        }
    }
}

fn draw_activity(s: &mut Screen, area: Rect, a: &App) {
    let title = if a.follow_log {
        " Activity (following) ".to_owned()
    } else {
        format!(" Activity (scrolled, {} lines) ", a.log.len())
    };
    let inner = s.block(area, Some(&title), PLAIN);
    let height = dim(inner.h);
    // Following pins the view to the tail; otherwise the operator's scroll
    // position is authoritative even as new lines arrive.
    let start = if a.follow_log {
        a.log.len().saturating_sub(height)
    } else {
        a.log_scroll.min(a.log.len().saturating_sub(1))
    };
    for (i, line) in a.log.iter().skip(start).take(height).enumerate() {
        s.text(inner.x, inner.y + coord(i), inner.w, line, PLAIN);
    }
}

fn draw_about(s: &mut Screen, area: Rect, a: &App) {
    let inner = Rect { x: area.x + 1, y: area.y + 1, w: area.w - 2, h: area.h - 2 };
    let mut lines = Vec::new();
    about_text(a.capsule, dim(inner.w).max(1), &mut lines);
    let height = dim(inner.h);
    // The far end is clamped here, where the wrapped length is known.
    let offset = a.about_scroll.min(lines.len().saturating_sub(height));
    let last = (offset + height).min(lines.len());
    let title = if lines.len() > height {
        format!(" About (lines {}-{last} of {}) ", offset + 1, lines.len())
    } else {
        " About ".to_owned()
    };
    s.block(area, Some(&title), PLAIN);
    for (i, line) in lines[offset..last].iter().enumerate() {
        s.text(inner.x, inner.y + coord(i), inner.w, line, PLAIN);
    }
}

fn draw_status(s: &mut Screen, area: Rect, a: &App) {
    let inner = s.block(area, None, PLAIN);
    // A multi-line status shows its first line here; the log has the rest.
    let text = match a.status.split_once('\n') {
        Some((first, _)) => format!(" {first} "),
        None => format!(" {} ", a.status),
    };
    let used = s.text(inner.x, inner.y, inner.w, &text, BOLD);
    let tail = format!(
        "| secrets {} | {}",
        if a.reveal { "shown" } else { "hidden" },
        if a.busy() { "Esc cancel" } else { "Enter act  Ctrl+R secrets  Ctrl+Q quit" }
    );
    s.text(inner.x + used, inner.y, inner.w - used, &tail, PLAIN);
}

/// A centred prompt over the view, for the actions that interrupt a
/// running appliance.
fn draw_confirm(s: &mut Screen, prompt: &str) {
    let width = (coord(chars(prompt)) + 8).min(s.w);
    let height = 5.min(s.h);
    let region = Rect { x: (s.w - width) / 2, y: (s.h - height) / 2, w: width, h: height };
    s.fill(region, u32::from(' '), PLAIN);
    let inner = s.block(region, Some(" Confirm "), WARNING);
    if inner.h > 0 {
        s.text(inner.x, inner.y, inner.w, prompt, PLAIN);
    }
    if inner.h > 1 {
        s.text(inner.x, inner.y + 1, inner.w, "y = yes, n = no", PLAIN);
    }
}

pub fn draw(s: &mut Screen, a: &App) {
    if s.w < UI_MIN_WIDTH || s.h < UI_MIN_HEIGHT {
        let text = format!(
            "Terminal is {}x{}; this needs at least {UI_MIN_WIDTH}x{UI_MIN_HEIGHT}.",
            s.w, s.h
        );
        let mut lines = Vec::new();
        wrap(&text, dim(s.w), &mut lines);
        let w = s.w;
        for (i, line) in lines.iter().enumerate().take(dim(s.h)) {
            s.text(0, coord(i), w, line, PLAIN);
        }
        return;
    }
    let tabs = Rect { x: 0, y: 0, w: s.w, h: 3 };
    let body = Rect { x: 0, y: 3, w: s.w, h: s.h - 6 };
    let status = Rect { x: 0, y: s.h - 3, w: s.w, h: 3 };
    draw_tabs(s, tabs, a);
    match a.current {
        View::Activity => draw_activity(s, body, a),
        View::About => draw_about(s, body, a),
        _ => draw_form(s, body, a),
    }
    draw_status(s, status, a);
    if let Some(p) = &a.confirm {
        s.cursor_visible = false;
        draw_confirm(s, &p.prompt);
    }
}
