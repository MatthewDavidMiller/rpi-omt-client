//! Drives the SSH client against a real server for
//! tests/integration/test_ssh_client.sh. Not a unit suite: it needs a
//! server.
//!
//!   ssh_interop HOST PORT USER KNOWN_HOSTS KEY|- run COMMAND [STDIN]
//!   ssh_interop HOST PORT USER KNOWN_HOSTS KEY|- pty COMMAND MARKER STDIN
//!   ssh_interop HOST PORT USER KNOWN_HOSTS KEY|- upload LOCAL REMOTE
//!   ssh_interop HOST PORT USER KNOWN_HOSTS KEY|- upload-bytes SIZE REMOTE
//!   ssh_interop HOST PORT USER KNOWN_HOSTS KEY|- shell-upload LOCAL REMOTE
//!
//! The password (or key passphrase) comes from OMT_TEST_SECRET. A run
//! prints the remote stdout, then "exit=N" on stderr.

#![forbid(unsafe_code)]
#![allow(clippy::cast_possible_truncation)] // the fill pattern is masked to a byte

use std::io::Write as _;
use std::process::ExitCode;

use omt_common::Error;
use omt_deploy::core::{Auth, Connection, Secret};
use omt_deploy::ssh::Session;
use omt_deploy::ssh::channel::{Source, shell_upload};
use omt_deploy::sys::{self, Cancel};

fn fail(e: &Error) -> ExitCode {
    eprintln!("ERROR: {e}");
    ExitCode::from(1)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args_os().map(|a| a.to_string_lossy().into_owned()).collect();
    if args.len() < 8 {
        eprintln!("usage: see the header of crates/omt-deploy/src/bin/ssh_interop.rs");
        return ExitCode::from(2);
    }
    let mut c = Connection::new();
    c.host.clone_from(&args[1]);
    c.port = args[2].parse().unwrap_or(0);
    c.username.clone_from(&args[3]);
    c.known_hosts_path = Some(args[4].clone());
    let secret = sys::getenv("OMT_TEST_SECRET");
    if args[5] == "-" {
        c.auth = Auth::Password;
        c.password = Secret::new(secret.as_deref().unwrap_or("")).unwrap_or_default();
    } else {
        c.auth = Auth::Key;
        c.key_path = Some(args[5].clone());
        if let Some(s) = &secret {
            c.key_passphrase = Secret::new(s).unwrap_or_default();
        }
    }
    let cancel = Cancel::new(false);
    let mut s = match Session::connect(&c, Some(&cancel)) {
        Ok(s) => s,
        Err(e) => return fail(&e),
    };
    let arg = |i: usize| args.get(i).map_or("", String::as_str);
    let result = match args[6].as_str() {
        mode @ ("run" | "pty") => {
            let r = if mode == "pty" {
                s.run_pty_after_marker(arg(7), arg(8), arg(9).as_bytes(), Some(&cancel))
            } else {
                s.run(arg(7), arg(8).as_bytes(), Some(&cancel))
            };
            r.map(|r| {
                let _ = std::io::stdout().write_all(r.out.as_bytes());
                if !r.err.is_empty() {
                    eprint!("stderr={}", r.err);
                }
                eprintln!("exit={}", r.exit_code);
            })
        }
        "upload" if args.len() > 8 => s.upload_file(arg(7), arg(8), Some(&cancel)),
        "shell-upload" if args.len() > 8 => sys::open_read(arg(7))
            .and_then(|f| shell_upload(&mut s, &mut Source::File(f), arg(8), Some(&cancel))),
        "upload-bytes" if args.len() > 8 => {
            let size: usize = arg(7).parse().unwrap_or(0);
            let data: Vec<u8> =
                (0..size).map(|i| ((i.wrapping_mul(131) + (i >> 9)) & 0xff) as u8).collect();
            s.upload_bytes(&data, arg(8), Some(&cancel))
        }
        mode => {
            eprintln!("unknown mode {mode}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(&e),
    }
}
