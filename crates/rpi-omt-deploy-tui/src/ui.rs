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
        View::About => draw_about(frame, areas[1]),
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

fn draw_about(frame: &mut Frame, area: Rect) {
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
    let body = vec![
        Line::from(format!("Raspberry Pi OMT client deployer {VERSION}")),
        Line::from(""),
        Line::from(capsule),
        Line::from(""),
        Line::from("Everything this deploys is compiled in; no checkout is needed."),
        Line::from(""),
        Line::from("Keys:"),
        Line::from("  F1-F7 / Ctrl+Left / Ctrl+Right   switch view"),
        Line::from("  Tab / Shift+Tab / Up / Down      move between fields"),
        Line::from("  Enter                            toggle, or run the focused action"),
        Line::from("  Ctrl+R                           reveal or hide secrets"),
        Line::from("  Esc                              cancel a running job"),
        Line::from("  Ctrl+Q                           quit"),
    ];
    let block = Block::default().borders(Borders::ALL).title(" About ");
    frame.render_widget(
        Paragraph::new(body).block(block).wrap(Wrap { trim: false }),
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
