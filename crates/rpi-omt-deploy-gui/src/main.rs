//! rpi-omt-deploy-gui: the deployment, driven from a local web browser.
//!
//! The page is served from this one file to the workstation's own browser
//! over the loopback interface, so the deployer still ships as one static
//! binary on Linux and one console executable on Windows and opens no
//! graphics stack of its own. Over SSH, forward the port
//! (`ssh -L 8765:127.0.0.1:8765`) and start it with `--port 8765
//! --no-browser`.

#![forbid(unsafe_code)]

use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;

use omt_common::VERSION;
use omt_deploy::{core, sys, tools};
use omt_gui::server::{Shared, serve};

const USAGE: &str = "usage: rpi-omt-deploy-gui [--port N] [--no-browser] [--version]";

struct Options {
    port: u16,
    browser: bool,
}

fn parse(args: &[String]) -> Result<Option<Options>, String> {
    let mut o = Options { port: 0, browser: true };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--version" | "-V" => {
                println!("rpi-omt-deploy-gui {VERSION}");
                return Ok(None);
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(None);
            }
            "--no-browser" => o.browser = false,
            "--port" => {
                let v = it.next().ok_or("--port needs a value")?;
                o.port = v.parse().map_err(|_| format!("--port: not a port number: {v}"))?;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Some(o))
}

/// Hands the URL to the desktop's browser. On a thread of its own, so an
/// opener that never returns cannot hold the server up; failing to open
/// one only means the operator pastes the printed URL.
fn open_browser(url: String) {
    let spawned = thread::Builder::new().name("open-browser".into()).stack_size(256 * 1024).spawn(
        move || {
            let opened = if sys::ON_WINDOWS {
                // `start` treats its first quoted argument as a window title.
                sys::run_process("cmd", &["/d", "/c", "start", "", &url], None, &[], None)
            } else if let Some(opener) = tools::find_executable("xdg-open") {
                sys::run_process(&opener, &[&url], None, &[], None)
            } else {
                eprintln!("No xdg-open here; open the URL above in a browser.");
                return;
            };
            if !opened.is_ok_and(|r| r.exit_code == 0) {
                eprintln!("Could not open a browser; open the URL above yourself.");
            }
        },
    );
    if spawned.is_err() {
        eprintln!("Could not open a browser; open the URL above yourself.");
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match parse(&args) {
        Ok(Some(o)) => o,
        Ok(None) => return ExitCode::SUCCESS,
        Err(problem) => {
            eprintln!("{problem}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    #[cfg(windows)]
    sys::init_console();
    let listener = match TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, options.port))) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cannot listen on 127.0.0.1:{}: {e}", options.port);
            return ExitCode::FAILURE;
        }
    };
    let port = match listener.local_addr() {
        Ok(a) => a.port(),
        Err(e) => {
            eprintln!("cannot read the listening address: {e}");
            return ExitCode::FAILURE;
        }
    };
    let token = match core::random_token(32) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}", e.message());
            return ExitCode::FAILURE;
        }
    };
    let shared = Arc::new(Shared::new(&omt_capsule::CAPSULE, token, port));
    let url = shared.launch_url();
    println!("rpi-omt-deploy-gui {VERSION}");
    println!("Open this address in a browser on this computer:\n\n  {url}\n");
    println!("Keep this window open while you work; Quit in the page, or Ctrl+C, stops it.");
    if options.browser {
        open_browser(url);
    }
    let served = serve(&listener, &shared);
    // A job still running belongs to its worker; ending the process ends
    // it. The state, secrets included, is wiped as it drops.
    drop(shared);
    match served {
        Ok(()) => {
            println!("Stopped.");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("server failed: {e}");
            ExitCode::FAILURE
        }
    }
}
