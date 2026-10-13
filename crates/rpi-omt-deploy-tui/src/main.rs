//! rpi-omt-deploy-tui: the deployment, driven from a terminal.
//!
//! A terminal frontend is what lets this ship as one static binary on Linux
//! and one console executable on Windows: it opens no graphics stack, and it
//! also works over SSH, which matters for an appliance that usually lives in
//! a rack.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use omt_common::VERSION;
use omt_tui::app::App;
use omt_tui::screen::Screen;
use omt_tui::term::Terminal;
use omt_tui::ui;

/// How long a redraw waits for a keystroke before looping. The worker
/// delivers progress into shared state rather than waking the loop, so this
/// is also the longest an operator waits to see a new line during a
/// deployment.
const TICK_MS: u64 = 100;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(first) = args.first() {
        if first == "--version" || first == "-V" {
            println!("rpi-omt-deploy-tui {VERSION}");
            return ExitCode::SUCCESS;
        }
        eprintln!("usage: rpi-omt-deploy-tui [--version]");
        return ExitCode::from(2);
    }
    let mut term = match Terminal::enter() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}", e.message());
            return ExitCode::FAILURE;
        }
    };
    let mut a = App::new(&omt_capsule::CAPSULE);
    let mut s = Screen::new();
    let mut frame = String::with_capacity(64 * 1024);
    let mut keys = Vec::with_capacity(64);
    while !a.should_quit {
        a.poll_worker();
        let (w, h) = term.size().unwrap_or((80, 24));
        s.begin(i32::from(w), i32::from(h));
        ui::draw(&mut s, &a);
        frame.clear();
        s.flush(&mut frame);
        term.write(frame.as_bytes());
        term.read_keys(64, TICK_MS, &mut keys);
        for key in &keys {
            if a.should_quit {
                break;
            }
            a.handle_key(key);
        }
    }
    // Restore the terminal before anything else: a console left in raw mode
    // on the alternate screen is worse than whatever went wrong.
    term.leave();
    // A job still running belongs to the worker now; leaving lets the
    // process end it.
    drop(a);
    ExitCode::SUCCESS
}
