// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//! Drawing for the terminal deployer. Reads `App`; never changes it.

use crate::app::{App, Kind, Slot, VERSION, View};
use omt_deployer_core::{IMAGE_MEMBER, embedded_image, embedded_members, sha256_bytes};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Tabs, Wrap};

/// The texts the About view reproduces, taken from the files the release
/// package ships beside the binary. Included the same way the egui application
/// includes them, from the same two paths, so the two deployers cannot come to
/// carry different notices.
const LICENSE: &str = include_str!("../../../LICENSE");
const NOTICES: &str = include_str!("../../../THIRD_PARTY_NOTICES.txt");

/// Terminals vary in how many colours they honour, so emphasis carries through
/// modifiers as well as colour. A monochrome console still shows focus.
const FOCUS: Style = Style::new()
    .fg(Color::Black)
    .bg(Color::Cyan)
    .add_modifier(Modifier::BOLD);

fn clamp_u16(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

/// The header, a usable form, and the status bar do not fit below this, and a
/// layout squeezed past it renders as nothing at all rather than as something
/// cramped. Saying so beats presenting an empty screen.
const MIN_WIDTH: u16 = 40;
const MIN_HEIGHT: u16 = 11;

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        frame.render_widget(
            Paragraph::new(format!(
                "Terminal is {}x{}; this needs at least {MIN_WIDTH}x{MIN_HEIGHT}.",
                area.width, area.height
            ))
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
        ])
        .split(area);

    draw_tabs(frame, areas[0], app);
    match app.view {
        View::Activity => draw_activity(frame, areas[1], app),
        View::About => draw_about(frame, areas[1], app),
        view => draw_form(frame, areas[1], app, view),
    }
    draw_status(frame, areas[2], app);

    if let Some(pending) = app.pending.as_ref() {
        draw_confirm(frame, &pending.prompt);
    }
}

fn draw_tabs(frame: &mut Frame, area: Rect, app: &App) {
    let titles: Vec<Line> = View::ALL
        .iter()
        .enumerate()
        .map(|(index, view)| Line::from(format!("F{} {}", index + 1, view.title())))
        .collect();
    let selected = View::ALL.iter().position(|view| *view == app.view);
    let tabs = Tabs::new(titles)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Raspberry Pi OMT deployer {VERSION} ")),
        )
        .select(selected)
        .highlight_style(FOCUS);
    frame.render_widget(tabs, area);
}

/// One row per slot: label on the left, value or control on the right.
fn draw_form(frame: &mut Frame, area: Rect, app: &App, view: View) {
    let slots = view.slots();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", view.title()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            slots
                .iter()
                .map(|_| Constraint::Length(1))
                .chain(std::iter::once(Constraint::Min(0)))
                .collect::<Vec<_>>(),
        )
        .split(inner);

    for (index, slot) in slots.iter().enumerate() {
        let Some(row) = rows.get(index) else { continue };
        let focused = index == app.focus;
        frame.render_widget(Paragraph::new(row_line(app, *slot, focused)), *row);

        // Put the real terminal cursor where the edit will land, so the
        // operator's own terminal shows the insertion point.
        if focused && matches!(slot.kind(), Kind::Text | Kind::Secret) {
            let prefix = clamp_u16(label_width(*slot) + app.cursor);
            frame.set_cursor_position((row.x.saturating_add(prefix), row.y));
        }
    }
}

fn label_width(slot: Slot) -> usize {
    // A fixed gutter keeps the values aligned down the view rather than
    // stepping in and out with the length of each label.
    let _ = slot;
    32
}

fn row_line(app: &App, slot: Slot, focused: bool) -> Line<'static> {
    let marker = if focused { "> " } else { "  " };
    let label = format!(
        "{marker}{:<width$}",
        slot.label(),
        width = label_width(slot) - 2
    );
    let style = if focused { FOCUS } else { Style::default() };

    let value = match slot.kind() {
        Kind::Text => app.value(slot).to_owned(),
        Kind::Secret => {
            let text = app.value(slot);
            if app.reveal {
                text.to_owned()
            } else {
                "*".repeat(text.chars().count())
            }
        }
        Kind::Toggle => {
            if app.toggle(slot) {
                "[x]".into()
            } else {
                "[ ]".into()
            }
        }
        Kind::Button => {
            if app.busy() {
                "[ running... ]".into()
            } else {
                "[ press Enter ]".into()
            }
        }
    };
    Line::from(vec![Span::styled(label, style), Span::raw(value)])
}

fn draw_activity(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(if app.follow_log {
            " Activity (following) ".to_owned()
        } else {
            format!(" Activity (scrolled, {} lines) ", app.log.len())
        });
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let height = inner.height as usize;
    // Following pins the view to the tail; otherwise the operator's scroll
    // position is authoritative even as new lines arrive underneath it.
    let start = if app.follow_log {
        app.log.len().saturating_sub(height)
    } else {
        app.log_scroll.min(app.log.len().saturating_sub(1))
    };
    let lines: Vec<Line> = app
        .log
        .iter()
        .skip(start)
        .take(height)
        .map(|line| Line::from(line.clone()))
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The About document as one scrollable text, wrapped to `width`.
///
/// The egui application puts the licence and the third-party notices behind
/// collapsing headers on its own About view. A terminal has no such widget, so
/// the same two texts are sections of one document here. They are not optional
/// extras: the deployer ships as a single file with the appliance compiled into
/// it, so this is the only copy of those notices an operator receives.
fn about_text(width: usize) -> Vec<String> {
    // The digest is the point: a single-file deployer is otherwise opaque
    // about which appliance build it carries.
    let capsule = embedded_image().map_or_else(
        || format!("This deployer was built without {IMAGE_MEMBER}."),
        |image| {
            format!(
                "Embedded capsule: {} members, {IMAGE_MEMBER} {} MiB, sha256 {}.",
                embedded_members().len(),
                image.bytes.len() / (1024 * 1024),
                sha256_bytes(image.bytes)
            )
        },
    );
    let head = [
        format!("Raspberry Pi OMT client deployer {VERSION}"),
        String::new(),
        capsule,
        String::new(),
        "Everything this deploys is compiled in; no checkout is needed.".to_owned(),
        String::new(),
        "Keys:".to_owned(),
        "  F1-F7 / Ctrl+Left / Ctrl+Right   switch view".to_owned(),
        "  Tab / Shift+Tab                  move between fields".to_owned(),
        "  Enter                            toggle, or run the focused action".to_owned(),
        "  Ctrl+R                           reveal or hide secrets".to_owned(),
        "  Esc                              cancel a running job".to_owned(),
        "  PageUp / PageDown / Up / Down    scroll Activity and About".to_owned(),
        "  Home / End                       jump to the ends of this view".to_owned(),
        "  Ctrl+Q                           quit".to_owned(),
    ]
    .join("\n");

    let sections = [
        head.as_str(),
        "\nLICENSE\n-------",
        LICENSE.trim_end(),
        "\nTHIRD-PARTY NOTICES\n-------------------",
        NOTICES.trim_end(),
    ];
    sections
        .iter()
        .flat_map(|section| wrap(section, width))
        .collect()
}

/// Wrap `text` to `width`, keeping each source line's indentation.
///
/// Pre-wrapped rather than handed to ratatui's `Wrap`, because that wrapping
/// happens after the scroll offset is applied: the licence's long lines would
/// then expand into more rows than the offset was ever clamped against, and the
/// end of the notices could not be reached. Wrapping here makes the line count
/// the drawing clamps with the same one the operator scrolls through.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut wrapped = Vec::new();
    for source in text.lines() {
        // A line that already fits is kept exactly as written, so the columns
        // the key legend is aligned in survive on a terminal wide enough for
        // them. Only a line that must be broken is re-flowed.
        if source.chars().count() <= width {
            wrapped.push(source.to_owned());
            continue;
        }
        let mut indent: String = source
            .chars()
            .take_while(|character| character.is_whitespace())
            .collect();
        // An indent as wide as the area leaves nowhere to put the words.
        if indent.chars().count() >= width {
            indent = String::new();
        }
        let margin = indent.chars().count();
        let mut line = indent.clone();
        for word in source
            .split_whitespace()
            .flat_map(|word| split_word(word, width - margin))
        {
            let filled = line.chars().count();
            let occupied = filled > margin;
            if occupied && filled + 1 + word.chars().count() > width {
                wrapped.push(std::mem::replace(&mut line, indent.clone()));
            } else if occupied {
                line.push(' ');
            }
            line.push_str(&word);
        }
        wrapped.push(line);
    }
    wrapped
}

/// Break a word too long for the area into pieces that fit.
///
/// The capsule digest is 64 characters and does not fit a narrow terminal, and
/// it is the one line on this view an operator might want to compare against a
/// release. Broken across rows it can still be read; clipped it cannot.
fn split_word(word: &str, width: usize) -> Vec<String> {
    if word.chars().count() <= width {
        return vec![word.to_owned()];
    }
    let mut pieces = Vec::new();
    let mut piece = String::new();
    for character in word.chars() {
        if piece.chars().count() == width {
            pieces.push(std::mem::take(&mut piece));
        }
        piece.push(character);
    }
    if !piece.is_empty() {
        pieces.push(piece);
    }
    pieces
}

fn draw_about(frame: &mut Frame, area: Rect, app: &App) {
    let inner = Block::default().borders(Borders::ALL).inner(area);
    let lines = about_text(usize::from(inner.width));
    let height = usize::from(inner.height);
    // The far end of the scroll is clamped here rather than in `App`, which
    // knows neither the width the text wrapped to nor the height it lands in.
    let offset = app.about_scroll.min(lines.len().saturating_sub(height));
    let last = (offset + height).min(lines.len());
    let title = if lines.len() > height {
        format!(" About (lines {}-{} of {}) ", offset + 1, last, lines.len())
    } else {
        " About ".to_owned()
    };

    let body: Vec<Line> = lines
        .into_iter()
        .skip(offset)
        .take(height)
        .map(Line::from)
        .collect();
    frame.render_widget(
        Paragraph::new(body).block(Block::default().borders(Borders::ALL).title(title)),
        area,
    );
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    let reveal = if app.reveal { "shown" } else { "hidden" };
    let hint = if app.busy() {
        "Esc cancel"
    } else {
        "Enter act  Ctrl+R secrets  Ctrl+Q quit"
    };
    let line = Line::from(vec![
        Span::styled(
            format!(" {} ", app.status),
            Style::new().add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("| secrets {reveal} | {hint}")),
    ]);
    frame.render_widget(
        Paragraph::new(line).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

/// A centred prompt over the view, for the actions that interrupt a running
/// appliance.
fn draw_confirm(frame: &mut Frame, prompt: &str) {
    let area = frame.area();
    let width = clamp_u16(prompt.chars().count() + 8).min(area.width);
    let height = 5.min(area.height);
    let region = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    let body = vec![Line::from(prompt.to_owned()), Line::from("y = yes, n = no")];
    frame.render_widget(ratatui::widgets::Clear, region);
    frame.render_widget(
        Paragraph::new(body).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Confirm ")
                .border_style(Style::new().fg(Color::Yellow)),
        ),
        region,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Everything the egui About view shows. This one shipped with the version
    /// and the capsule but neither of the two texts, and a deployer that is one
    /// file with no package around it is the only place an operator would find
    /// them.
    #[test]
    fn about_reproduces_the_licence_and_the_third_party_notices() {
        let text = about_text(80).join("\n");
        assert!(text.contains(VERSION), "{text}");
        assert!(text.contains("MIT License"));
        assert!(text.contains("Copyright (c) 2026 Matthew David Miller"));
        assert!(text.contains("Permission is hereby granted, free of charge"));
        assert!(text.contains("THIRD-PARTY NOTICES"));
        assert!(text.contains("OPEN MEDIA TRANSPORT COMPONENTS"));
        assert!(text.contains("SPDX license identifiers above are descriptive"));
    }

    /// The two texts are included from the files the release package ships, so
    /// neither can be edited into a paraphrase of what was actually shipped.
    #[test]
    fn the_texts_are_the_files_themselves() {
        let text = about_text(200).join("\n");
        for line in LICENSE.lines().chain(NOTICES.lines()) {
            assert!(text.contains(line.trim_end()), "missing: {line}");
        }
    }

    #[test]
    fn wrapping_keeps_blank_lines_and_indentation() {
        assert_eq!(
            wrap("head\n\n  a bb ccc", 6),
            vec!["head", "", "  a bb", "  ccc"]
        );
    }

    /// The digest is 64 characters and the narrowest allowed view is 38, so
    /// clipping it would leave the one value worth comparing unreadable.
    #[test]
    fn a_word_wider_than_the_view_is_broken_rather_than_clipped() {
        assert_eq!(wrap("abcdefgh", 3), vec!["abc", "def", "gh"]);
        let narrow = about_text(38).join("");
        let digest = embedded_image().map(|image| sha256_bytes(image.bytes));
        if let Some(digest) = digest {
            assert!(narrow.contains(&digest), "{narrow}");
        }
    }

    /// Pre-wrapping is what makes the line count the scroll is clamped against
    /// the count that is actually drawn, so nothing may exceed the area.
    #[test]
    fn the_document_fits_the_width_it_wrapped_to() {
        // 38 is the narrowest inner width `MIN_WIDTH` allows.
        for width in [38, 60, 120] {
            for line in about_text(width) {
                assert!(line.chars().count() <= width, "{width}: {line}");
            }
        }
    }

    /// Scrolling to the end must show the end. An offset clamped against the
    /// unwrapped count would stop short of it by hundreds of lines.
    #[test]
    fn the_end_of_the_notices_can_be_scrolled_to() {
        let mut app = App::default();
        app.view = View::About;
        app.about_scroll = usize::MAX;
        let bottom = render(&app);
        let tail = NOTICES.trim_end().lines().next_back().unwrap_or_default();
        assert!(bottom.contains(tail.trim()), "{bottom}");

        app.about_scroll = 0;
        assert!(render(&app).contains("Raspberry Pi OMT client deployer"));
    }

    /// What a terminal would show, as text.
    fn render(app: &App) -> String {
        let backend = TestBackend::new(80, 30);
        let mut terminal = Terminal::new(backend).unwrap_or_else(|error| panic!("{error}"));
        terminal
            .draw(|frame| draw(frame, app))
            .unwrap_or_else(|error| panic!("{error}"));
        terminal.backend().to_string()
    }
}
