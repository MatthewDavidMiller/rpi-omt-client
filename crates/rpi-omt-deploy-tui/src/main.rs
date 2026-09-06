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
use crossterm::cursor::Show;
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
    install_panic_hook();
    let mut terminal = enter()?;
    let outcome = run(&mut terminal);
    // Restore the terminal even when the loop failed: leaving a console in raw
    // mode with the alternate screen active makes the operator's shell
    // unusable, which is a worse outcome than whatever went wrong.
    let restored = leave(&mut terminal);
    outcome.and(restored)
}

/// Restore the terminal before a panic reaches the operator.
///
/// The `Err` path above cannot cover this one: the release profile aborts on
/// panic, so without a hook the process dies with raw mode and the alternate
/// screen still active and the panic message painted somewhere the shell will
/// never show again. The hook runs before the abort, and chaining to the
/// previous one keeps the message itself.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
        previous(info);
    }));
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
        // Quitting with a job running abandons a worker that may be
        // mid-transaction, so it is confirmed rather than immediate.
        KeyCode::Char('q') if control => app.request_quit(),
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
        // Leaving the tail starts from the tail. `log_scroll` is not tracked
        // while the view follows, so paging up from a following view used to
        // move from zero and land on the oldest line of the run -- the
        // opposite end of the log from the one being read.
        if app.follow_log {
            app.log_scroll = app.log.len().saturating_sub(1);
        }
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

#[cfg(test)]
mod tests {
    use super::{App, scroll_log};

    fn with_log(lines: usize) -> App {
        let mut app = App::default();
        for index in 0..lines {
            app.push_log(index.to_string());
        }
        app
    }

    /// A deployment scrolls past hundreds of lines, and the reason to page up
    /// is always something just above the tail. Paging up used to move from a
    /// `log_scroll` that had never left zero, so the first press jumped to the
    /// oldest line of the run and getting back took a press per ten lines.
    #[test]
    fn paging_up_leaves_the_tail_rather_than_the_beginning() {
        let mut app = with_log(500);
        assert!(app.follow_log);

        scroll_log(&mut app, -1);

        assert!(!app.follow_log);
        assert_eq!(app.log_scroll, 489);
    }

    /// Paging back down returns to following, from one page rather than fifty.
    #[test]
    fn paging_back_down_resumes_following() {
        let mut app = with_log(500);
        scroll_log(&mut app, -1);
        scroll_log(&mut app, 1);
        assert_eq!(app.log_scroll, 499);

        scroll_log(&mut app, 1);
        assert!(app.follow_log);
    }

    /// Repeated pages still reach the oldest line, and stop there.
    #[test]
    fn paging_up_stops_at_the_start_of_the_log() {
        let mut app = with_log(30);
        for _ in 0..10 {
            scroll_log(&mut app, -1);
        }
        assert_eq!(app.log_scroll, 0);
        assert!(!app.follow_log);
    }

    /// An empty log has no tail to leave.
    #[test]
    fn paging_an_empty_log_is_harmless() {
        let mut app = with_log(0);
        scroll_log(&mut app, -1);
        assert_eq!(app.log_scroll, 0);
    }
}
