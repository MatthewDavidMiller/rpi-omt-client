#![forbid(unsafe_code)]

use axum_server::tls_rustls::RustlsConfig;
use omt_web::{
    app::{self, AppState},
    auth,
    settings::Settings,
    state,
};
use std::{
    env,
    net::{Ipv4Addr, SocketAddr},
    process::ExitCode,
};

const VERSION: &str = match option_env!("RPI_OMT_CLIENT_VERSION") {
    Some(value) => value,
    None => env!("CARGO_PKG_VERSION"),
};

fn fail(message: &str) -> ExitCode {
    eprintln!("omt-web: {message}");
    ExitCode::FAILURE
}

#[tokio::main]
async fn main() -> ExitCode {
    let settings = match Settings::load() {
        Ok(value) => value,
        Err(error) => return fail(&error),
    };
    let arguments: Vec<_> = env::args().skip(1).collect();
    match arguments.as_slice() {
        [command] if command == "--version" || command == "-V" => {
            println!("{VERSION}");
            return ExitCode::SUCCESS;
        }
        [command] if command == "initialize" => match auth::initialize(&settings) {
            Ok(Some(password)) => {
                println!("{password}");
                return ExitCode::SUCCESS;
            }
            Ok(None) => return ExitCode::SUCCESS,
            Err(error) => return fail(&error),
        },
        [command, path] if command == "play-target" => match state::read_source(path.as_ref()) {
            Ok(Some(target)) => {
                println!("{}", target.value());
                return ExitCode::SUCCESS;
            }
            Ok(None) => return fail("saved OMT target is missing"),
            Err(error) => return fail(&error),
        },
        [command, path, board_default] if command == "video-ceiling" => {
            match state::effective_video_ceiling(path.as_ref(), board_default) {
                Ok(value) => {
                    println!("{value}");
                    return ExitCode::SUCCESS;
                }
                Err(error) => return fail(&error),
            }
        }
        [] => {}
        _ => {
            return fail(
                "usage: omt-web [initialize | play-target PATH | video-ceiling PATH BOARD_DEFAULT]",
            );
        }
    }

    if let Err(error) = rustls::crypto::ring::default_provider().install_default() {
        return fail(&format!("unable to install TLS provider: {error:?}"));
    }
    let state = match AppState::build(settings.clone()) {
        Ok(value) => value,
        Err(error) => return fail(&error),
    };
    auth::remove_legacy_secret(&settings);
    let tls =
        match RustlsConfig::from_pem_file(&settings.tls_cert_file, &settings.tls_key_file).await {
            Ok(value) => value,
            Err(error) => return fail(&format!("unable to load TLS certificate: {error}")),
        };
    let address = SocketAddr::from((Ipv4Addr::UNSPECIFIED, settings.web_port));
    println!("omt-web listening on https://{address}");
    if let Err(error) = axum_server::bind_rustls(address, tls)
        .serve(app::router(state).into_make_service_with_connect_info::<SocketAddr>())
        .await
    {
        return fail(&format!("web server failed: {error}"));
    }
    ExitCode::SUCCESS
}
