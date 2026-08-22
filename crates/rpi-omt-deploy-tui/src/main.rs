// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//! The Linux deployer: the same deployment the egui application performs on
//! Windows, driven from a terminal.
//!
//! A terminal frontend is what lets this ship as one static binary. The egui
//! stack reaches the screen through libEGL, libGL, libX11, and
//! libwayland-client, which it opens at runtime with `dlopen`; those are the
//! operator's graphics driver and are linked against that machine's glibc, so
//! a GUI build is tied to a glibc floor no matter how it is packaged. Terminal
//! output is `read`/`write`/`ioctl` on file descriptors and opens nothing, so
//! this links fully static and runs on every distribution, musl ones included.
//!
//! It also works over SSH, which matters for an appliance that usually lives
//! in a rack.

#![forbid(unsafe_code)]

mod app;
mod ui;

use app::{App, View};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io::{self, Stdout};
use std::time::Duration;

/// How long a redraw waits for a keystroke before looping.
///
/// The worker delivers progress on a channel rather than waking the loop, so
/// this is also the longest an operator waits to see a new line during a
/// deployment.
const TICK: Duration = Duration::from_millis(100);

fn main() -> io::Result<()> {
    let mut terminal = enter()?;
    let outcome = run(&mut terminal);
    // Restore the terminal even when the loop failed: leaving a console in raw
    // mode with the alternate screen active makes the operator's shell
    // unusable, which is a worse outcome than whatever went wrong.
    let restored = leave(&mut terminal);
    outcome.and(restored)
}

fn enter() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    Terminal::new(CrosstermBackend::new(stdout))
}

fn leave(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()
}

fn run(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    let mut app = App::default();
    while !app.should_quit {
        app.poll_worker();
        terminal.draw(|frame| ui::draw(frame, &app))?;
        // The `KeyEventKind::Press` test is not redundant: terminals that
        // report key release would otherwise double every keystroke.
        if event::poll(TICK)?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            handle_key(&mut app, key);
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, key: KeyEvent) {
    // A pending confirmation owns the keyboard until it is answered, so a
    // reboot cannot be triggered by a keystroke meant for the form behind it.
    if app.pending.is_some() {
        match key.code {
            KeyCode::Char('y' | 'Y') => app.confirm_pending(true),
            KeyCode::Char('n' | 'N') | KeyCode::Esc => app.confirm_pending(false),
            _ => {}
        }
        return;
    }

    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('q') if control => app.should_quit = true,
        KeyCode::Char('c') if control => {
            // Ctrl+C stops the job rather than the program while one is
            // running: a half-finished deployment should be told to stop.
            if app.busy() {
                app.cancel_job();
            } else {
                app.should_quit = true;
            }
        }
        KeyCode::Char('r') if control => app.reveal = !app.reveal,
        KeyCode::Esc => app.cancel_job(),

        KeyCode::Right if control => cycle_view(app, 1),
        KeyCode::Left if control => cycle_view(app, -1),
        KeyCode::F(number) => {
            if let Some(view) = View::ALL.get(usize::from(number).saturating_sub(1)) {
                app.select_view(*view);
            }
        }

        // About has no fields, so the keys that would move between them read
        // the licence and the notices instead. Home and End are the two ends of
        // the document; End overshoots deliberately and is clamped at render,
        // where the wrapped line count is known.
        KeyCode::Down if app.view == View::About => app.scroll_about(1),
        KeyCode::Up if app.view == View::About => app.scroll_about(-1),
        KeyCode::Home if app.view == View::About => app.about_scroll = 0,
        KeyCode::End if app.view == View::About => app.about_scroll = usize::MAX,

        KeyCode::Tab | KeyCode::Down => app.move_focus(1),
        KeyCode::BackTab | KeyCode::Up => app.move_focus(-1),
        KeyCode::Enter => app.activate(),

        KeyCode::Left => app.move_cursor(-1),
        KeyCode::Right => app.move_cursor(1),
        KeyCode::Home => app.cursor_home(),
        KeyCode::End => app.cursor_end(),
        KeyCode::Backspace => app.backspace(),
        KeyCode::Delete => app.delete(),

        KeyCode::PageUp => scroll(app, -1),
        KeyCode::PageDown => scroll(app, 1),

        KeyCode::Char(character) if !control => app.insert(character),
        _ => {}
    }
}

fn cycle_view(app: &mut App, delta: isize) {
    let count = isize::try_from(View::ALL.len()).unwrap_or(1);
    let current = View::ALL
        .iter()
        .position(|view| *view == app.view)
        .and_then(|index| isize::try_from(index).ok())
        .unwrap_or(0);
    let next = (current + delta).rem_euclid(count);
    if let Some(view) = usize::try_from(next).ok().and_then(|i| View::ALL.get(i)) {
        app.select_view(*view);
    }
}

/// A page of whichever view scrolls: the About document, or the activity log.
fn scroll(app: &mut App, direction: isize) {
    if app.view == View::About {
        app.scroll_about(direction * 10);
    } else {
        scroll_log(app, direction);
    }
}

fn scroll_log(app: &mut App, direction: isize) {
    if direction < 0 {
        app.follow_log = false;
        app.log_scroll = app.log_scroll.saturating_sub(10);
    } else {
        app.log_scroll = app.log_scroll.saturating_add(10);
        // Scrolling back to the tail resumes following, so an operator who
        // scrolled up to read something does not have to know a key to undo it.
        if app.log_scroll >= app.log.len() {
            app.follow_log = true;
        }
    }
}
