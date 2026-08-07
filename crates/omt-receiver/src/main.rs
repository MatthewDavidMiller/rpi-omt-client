// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// Playback behaviour is derived from the MIT-licensed Open Media Transport
// projects; see third_party/omt/PROVENANCE.md.
#![forbid(unsafe_code)]

mod audio;
mod channel;
mod cli;
mod connector;
mod discovery;
mod mdns;
mod play;
mod video;
mod xml;

use channel::{Channel, Endpoint};
use cli::{Options, usage};
use omt_protocol::{FrameType, is_valid_target};
use omt_receiver_core::{PlaybackStatus, sanitize_detail};
use serde::Serialize;
use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

const VERSION: &str = match option_env!("RPI_OMT_CLIENT_VERSION") {
    Some(value) => value,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Serialize)]
struct Discovered {
    name: String,
    target: String,
    kind: &'static str,
}

#[derive(Serialize)]
struct ProbeResult<'a> {
    ok: bool,
    target: &'a str,
    video: bool,
    audio: bool,
    width: i32,
    height: i32,
    frame_rate: f64,
    channels: i32,
    sample_rate: i32,
    error: &'a str,
}

fn discover(options: &Options) -> Result<i32, String> {
    options.allowed(&["--wait-ms", "--json"])?;
    options.flag("--json")?;
    let wait = options.number("--wait-ms", 1500, 0, 60_000)?;
    let sources: Vec<Discovered> = discovery::sources(Duration::from_millis(wait))
        .into_iter()
        .map(|source| Discovered {
            target: source.name.clone(),
            name: source.name,
            kind: "discovered",
        })
        .collect();
    println!(
        "{}",
        serde_json::to_string(&sources).map_err(|error| error.to_string())?
    );
    Ok(0)
}

fn probe(options: &Options) -> Result<i32, String> {
    options.allowed(&["--target", "--timeout-ms", "--json"])?;
    options.flag("--json")?;
    let target = options.required("--target")?;
    let timeout = options.number("--timeout-ms", 3000, 1, 60_000)?;
    if !is_valid_target(target) {
        return Err("Invalid OMT direct target.".into());
    }

    let budget = Duration::from_millis(timeout);
    let deadline = Instant::now() + budget;
    let mut measurement = Measurement::default();
    let mut failure = String::new();

    match discovery::resolve(target, budget) {
        None => failure = "OMT target was not discovered.".into(),
        Some(endpoint) => {
            let mut channel_error = String::new();
            measure(&endpoint, deadline, &mut measurement, &mut channel_error);
            if !(measurement.video || measurement.audio) {
                failure = if channel_error.is_empty() {
                    "No OMT media was received.".into()
                } else {
                    channel_error
                };
            }
        }
    }

    let ok = measurement.video || measurement.audio;
    let error = sanitize_detail(&failure);
    println!(
        "{}",
        serde_json::to_string(&ProbeResult {
            ok,
            target,
            video: measurement.video,
            audio: measurement.audio,
            width: measurement.width,
            height: measurement.height,
            frame_rate: measurement.frame_rate,
            channels: measurement.channels,
            sample_rate: measurement.sample_rate,
            error: &error,
        })
        .map_err(|error| error.to_string())?
    );
    Ok(if ok { 0 } else { 3 })
}

#[derive(Default)]
struct Measurement {
    video: bool,
    audio: bool,
    width: i32,
    height: i32,
    frame_rate: f64,
    channels: i32,
    sample_rate: i32,
}

/// Subscribes to both media types and reports whatever arrives first, so a
/// video-only or audio-only sender still probes as reachable.
fn measure(
    endpoint: &Endpoint,
    deadline: Instant,
    measurement: &mut Measurement,
    error: &mut String,
) {
    let mut video = Channel::new();
    let mut audio = Channel::new();
    let video_connected = record(video.connect(endpoint, FrameType::Video, deadline), error);
    let audio_connected = record(audio.connect(endpoint, FrameType::Audio, deadline), error);

    while Instant::now() < deadline && !(measurement.video && measurement.audio) {
        if !measurement.video
            && video_connected
            && sample(&mut video, FrameType::Video, deadline, error)
            && let Some(header) = video.frame().video.as_ref()
        {
            measurement.video = true;
            measurement.width = header.width;
            measurement.height = header.height;
            measurement.frame_rate =
                f64::from(header.frame_rate_n) / f64::from(header.frame_rate_d);
        }
        if !measurement.audio
            && audio_connected
            && sample(&mut audio, FrameType::Audio, deadline, error)
            && let Some(header) = audio.frame().audio.as_ref()
        {
            measurement.audio = true;
            measurement.channels = header.channels;
            measurement.sample_rate = header.sample_rate;
        }
        if !video_connected && !audio_connected {
            break;
        }
    }
}

fn record(outcome: std::io::Result<()>, error: &mut String) -> bool {
    match outcome {
        Ok(()) => true,
        Err(reason) => {
            *error = reason.to_string();
            false
        }
    }
}

/// Reads one 100 ms slice looking for the wanted frame type.
fn sample(channel: &mut Channel, wanted: FrameType, deadline: Instant, error: &mut String) -> bool {
    let slice = deadline.min(Instant::now() + Duration::from_millis(100));
    while Instant::now() < slice {
        match channel.receive(slice) {
            Ok(frame) if frame.header.frame_type == wanted => return true,
            Ok(_) => {}
            Err(reason) => {
                *error = reason.to_string();
                return false;
            }
        }
    }
    false
}

fn play(options: &Options) -> Result<i32, String> {
    options.allowed(&[
        "--target",
        "--connector",
        "--status-file",
        "--retry-seconds",
    ])?;
    let target = options.required("--target")?.to_owned();
    let path = PathBuf::from(options.required("--status-file")?);
    let retry = options.number("--retry-seconds", 2, 1, 30)?;
    let preference = options.value("--connector").unwrap_or("auto").to_owned();
    if !is_valid_target(&target) || !matches!(preference.as_str(), "auto" | "HDMI-A-1" | "HDMI-A-2")
    {
        return Err("Invalid play options.".into());
    }

    // signal-hook raises the flag on delivery, so shutdown is the raised
    // state and the playback loop runs while it is clear.
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(signal, Arc::clone(&stop))
            .map_err(|error| format!("Unable to install the shutdown handler: {error}"))?;
    }

    let status = Arc::new(PlaybackStatus::new(path, target.clone()));
    play::run(
        &play::Options {
            target,
            preference,
            retry: Duration::from_secs(retry),
        },
        &status,
        &stop,
    );
    Ok(0)
}

fn run() -> i32 {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.as_slice() == ["--version"] {
        println!("{VERSION}");
        return 0;
    }
    let Some(command) = args.first() else {
        return usage();
    };
    let options = match Options::parse(&args[1..]) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{error}");
            return 2;
        }
    };
    let result = match command.as_str() {
        "discover" => discover(&options),
        "probe" => probe(&options),
        "play" => play(&options),
        _ => return usage(),
    };
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            2
        }
    }
}

fn main() {
    std::process::exit(run());
}
