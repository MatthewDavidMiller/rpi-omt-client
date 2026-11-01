//! omt-web: the appliance's HTTPS operator interface, and the small helper
//! commands the container scripts call (initialize, set-password,
//! play-target, video-ceiling).

#![forbid(unsafe_code)]

use std::io::{ErrorKind, Read, Write as _};
use std::process::ExitCode;

use omt_common::buf::wipe_vec;
use omt_sys::posix::{STOP_REQUESTED, install_stop_signals};
use omt_web::app::App;
use omt_web::auth::{self, MAXIMUM_PASSWORD_BYTES};
use omt_web::http::Server;
use omt_web::settings::Settings;
use omt_web::state;

fn fail(message: impl std::fmt::Display) -> ExitCode {
    eprintln!("omt-web: {message}");
    ExitCode::FAILURE
}

fn set_password(s: &Settings) -> ExitCode {
    // At most the longest valid password, its newline, and one byte more to
    // tell an over-long one apart.
    let mut input = vec![0_u8; MAXIMUM_PASSWORD_BYTES + 2];
    let mut len = 0;
    let mut stdin = std::io::stdin().lock();
    while len < input.len() {
        match stdin.read(&mut input[len..]) {
            Ok(0) => break,
            Ok(n) => len += n,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => {
                wipe_vec(&mut input);
                return fail(format!("unable to read the new password: {e}"));
            }
        }
    }
    input.truncate(len);
    if input.last() == Some(&b'\n') && std::str::from_utf8(&input).is_ok() {
        input.pop();
    }
    let result = match std::str::from_utf8(&input) {
        Err(_) => Err("unable to read the new password: stream did not contain valid UTF-8".into()),
        Ok(password) => auth::replace_password(s, password).map_err(|e| e.to_string()),
    };
    wipe_vec(&mut input);
    if let Err(message) = result {
        return fail(message);
    }
    println!("Web GUI password updated.");
    ExitCode::SUCCESS
}

fn serve(settings: &Settings) -> ExitCode {
    let mut app = match App::new(settings) {
        Ok(app) => app,
        Err(e) => return fail(e),
    };
    auth::remove_legacy_secret(settings);
    let mut server = match Server::open(
        settings.web_port,
        &settings.tls_cert_file,
        &settings.tls_key_file,
        settings.max_request_bytes,
    ) {
        Ok(server) => server,
        Err(e) => return fail(e),
    };
    if let Err(e) = install_stop_signals() {
        return fail(format!("unable to install the shutdown handler: {e}"));
    }
    println!("omt-web listening on https://0.0.0.0:{}", settings.web_port);
    let _ = std::io::stdout().flush();
    match server.run(&mut app, &STOP_REQUESTED) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(format!("web server failed: {e}")),
    }
}

fn main() -> ExitCode {
    let settings = match Settings::load() {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let args: Vec<String> =
        std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["--version" | "-V"] => {
            println!("{}", omt_common::VERSION);
            ExitCode::SUCCESS
        }
        ["initialize"] => match auth::initialize(&settings) {
            Err(e) => fail(e),
            Ok(generated) => {
                if let Some(mut password) = generated {
                    println!("{password}");
                    omt_common::buf::wipe_string(&mut password);
                }
                ExitCode::SUCCESS
            }
        },
        ["set-password"] => set_password(&settings),
        ["play-target", path] => match state::read_source(path) {
            Err(e) => fail(e),
            Ok(None) => fail("saved OMT target is missing"),
            Ok(Some(target)) => {
                println!("{}", target.value);
                ExitCode::SUCCESS
            }
        },
        ["video-ceiling", path, board_default] => {
            match state::effective_video_ceiling(path, board_default) {
                Err(e) => fail(e),
                Ok(value) => {
                    println!("{value}");
                    ExitCode::SUCCESS
                }
            }
        }
        [] => serve(&settings),
        _ => fail(
            "usage: omt-web [initialize | set-password | play-target PATH | video-ceiling PATH \
             BOARD_DEFAULT]",
        ),
    }
}
