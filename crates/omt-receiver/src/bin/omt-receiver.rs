//! omt-receiver: discover, probe, and play OMT sources.
//!
//! The CLI is a trust boundary: control-omt.sh and the Web diagnostics both
//! build argument vectors for it, so every rejection is a named usage
//! failure (exit 2) that leaves the command unrun.

#![forbid(unsafe_code)]

use std::fmt::Write as _;
use std::process::ExitCode;
use std::sync::Arc;

use omt_common::timefmt::{now_ms, remaining_ms};
use omt_common::{Error, json};
use omt_protocol::{FrameType, is_valid_target};
use omt_receiver::channel::{Channel, Endpoint, Recv};
use omt_receiver::{discovery, play};
use omt_receiver_core::{PlaybackStatus, VideoCeiling, sanitize_detail};
use omt_sys::posix::{STOP_REQUESTED, install_stop_signals};

/// More options than any subcommand accepts, which bounds the parse.
const MAX_OPTIONS: usize = 8;

fn usage() -> ExitCode {
    eprintln!(
        "Usage: omt-receiver --version | discover --wait-ms N --json | probe --target TARGET \
         --timeout-ms N --json | play --target TARGET --connector auto|HDMI-A-1|HDMI-A-2 \
         --status-file PATH --video-ceiling WIDTHxHEIGHT@FPS[,...]"
    );
    ExitCode::from(2)
}

/// `--key value` pairs and the single `--json` flag (no value).
struct Options(Vec<(String, Option<String>)>);

impl Options {
    fn parse(args: &[String]) -> Result<Self, Error> {
        let mut out: Vec<(String, Option<String>)> = Vec::new();
        let mut it = args.iter();
        while let Some(key) = it.next() {
            if !key.starts_with("--") {
                return Err(Error::new(format!("Unexpected argument: {key}")));
            }
            let value = if key == "--json" {
                None
            } else {
                Some(
                    it.next()
                        .ok_or_else(|| Error::new(format!("Missing value for {key}")))?
                        .clone(),
                )
            };
            if out.iter().any(|(k, _)| k == key) {
                return Err(Error::new(format!("Duplicate option: {key}")));
            }
            if out.len() == MAX_OPTIONS {
                return Err(Error::new("Too many options."));
            }
            out.push((key.clone(), value));
        }
        Ok(Self(out))
    }

    fn find(&self, key: &str) -> Option<&Option<String>> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Rejects any option the subcommand does not define, reporting the
    /// first in sorted order.
    fn allowed(&self, names: &[&str]) -> Result<(), Error> {
        let offending = self.0.iter().map(|(k, _)| k.as_str()).filter(|k| !names.contains(k)).min();
        match offending {
            Some(k) => Err(Error::new(format!("Option {k} is not valid for this command."))),
            None => Ok(()),
        }
    }

    fn required(&self, name: &str) -> Result<&str, Error> {
        match self.find(name) {
            Some(Some(v)) if !v.is_empty() => Ok(v),
            _ => Err(Error::new(format!("{name} is required."))),
        }
    }

    fn flag(&self, name: &str) -> Result<(), Error> {
        match self.find(name) {
            Some(None) => Ok(()),
            _ => Err(Error::new(format!("{name} is required."))),
        }
    }

    fn number(&self, name: &str, default: u64, min: u64, max: u64) -> Result<u64, Error> {
        let Some(value) = self.find(name) else { return Ok(default) };
        let Some(value) = value else {
            return Err(Error::new(format!("{name} requires a value.")));
        };
        // u64::from_str: ASCII digits with an optional leading '+'.
        let text = value.strip_prefix('+').unwrap_or(value);
        omt_common::buf::parse_u64(text.as_bytes(), u64::MAX)
            .filter(|v| (min..=max).contains(v))
            .ok_or_else(|| Error::new(format!("{name} must be between {min} and {max}.")))
    }
}

fn discover(o: &Options) -> Result<ExitCode, Error> {
    o.allowed(&["--wait-ms", "--json"])?;
    o.flag("--json")?;
    let wait = o.number("--wait-ms", 1500, 0, 60000)?;
    let set = discovery::discover_sources(wait);
    let mut out = String::from("[");
    for (i, s) in set.items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"name\":");
        json::write_string(&mut out, &s.name);
        out.push_str(",\"target\":");
        json::write_string(&mut out, &s.name);
        out.push_str(",\"kind\":\"discovered\"}");
    }
    out.push(']');
    println!("{out}");
    Ok(ExitCode::SUCCESS)
}

#[derive(Default)]
struct Measurement {
    video: bool,
    audio: bool,
    width: i32,
    height: i32,
    channels: i32,
    sample_rate: i32,
    frame_rate: f64,
}

/// Reads within the caller's bounded share looking for the wanted type.
fn sample(c: &mut Channel, wanted: FrameType, deadline: u64, error: &mut Option<Error>) -> bool {
    while now_ms() < deadline {
        match c.receive(deadline) {
            (Recv::Ok, _) => {
                if c.frame.frame_type() == Some(wanted) {
                    return true;
                }
            }
            (_, e) => {
                *error = e;
                return false;
            }
        }
    }
    false
}

/// A video frame can legitimately take longer than a fixed short slice on a
/// busy link, so video gets half of what is left while audio is still owed
/// a share, and all of it otherwise.
fn fair_sample_deadline(deadline: u64, reserve_other: bool) -> u64 {
    if reserve_other { now_ms() + remaining_ms(deadline) / 2 } else { deadline }
}

/// Subscribes to both media types and reports whatever arrives, so a
/// video-only or audio-only sender still probes as reachable.
fn measure(ep: &Endpoint, deadline: u64, m: &mut Measurement, error: &mut Option<Error>) {
    let mut video = Channel::new();
    let mut audio = Channel::new();
    let video_connected = match video.connect(ep, FrameType::Video, deadline) {
        Ok(()) => true,
        Err(e) => {
            *error = Some(e);
            false
        }
    };
    let audio_connected = match audio.connect(ep, FrameType::Audio, deadline) {
        Ok(()) => true,
        Err(e) => {
            *error = Some(e);
            false
        }
    };
    while now_ms() < deadline && !(m.video && m.audio) {
        let video_deadline = fair_sample_deadline(deadline, !m.audio && audio_connected);
        if !m.video
            && video_connected
            && sample(&mut video, FrameType::Video, video_deadline, error)
            && let Some(v) = video.frame.video
        {
            m.video = true;
            m.width = v.width;
            m.height = v.height;
            m.frame_rate = f64::from(v.frame_rate_n) / f64::from(v.frame_rate_d);
        }
        if !m.audio
            && audio_connected
            && sample(&mut audio, FrameType::Audio, deadline, error)
            && let Some(a) = audio.frame.audio
        {
            m.audio = true;
            m.channels = a.channels;
            m.sample_rate = a.sample_rate;
        }
        if !video_connected && !audio_connected {
            break;
        }
    }
}

fn probe(o: &Options) -> Result<ExitCode, Error> {
    o.allowed(&["--target", "--timeout-ms", "--json"])?;
    o.flag("--json")?;
    let target = o.required("--target")?;
    let timeout = o.number("--timeout-ms", 3000, 1, 60000)?;
    if !is_valid_target(target) {
        return Err(Error::new("Invalid OMT direct target."));
    }
    let deadline = now_ms() + timeout;
    let mut m = Measurement::default();
    // Half the budget discovers; the rest is left for the measurement.
    let failure = match discovery::resolve(target, timeout / 2) {
        None => Some(Error::new("OMT target was not discovered.")),
        Some(ep) => {
            let mut channel_error = None;
            measure(&ep, deadline, &mut m, &mut channel_error);
            if m.video || m.audio {
                None
            } else {
                Some(channel_error.unwrap_or_else(|| Error::new("No OMT media was received.")))
            }
        }
    };
    let ok = m.video || m.audio;
    let detail = sanitize_detail(failure.as_ref().map_or("", Error::message));
    let mut out = format!("{{\"ok\":{ok},\"target\":");
    json::write_string(&mut out, target);
    let _ = write!(
        out,
        ",\"video\":{},\"audio\":{},\"width\":{},\"height\":{},\"frame_rate\":",
        m.video, m.audio, m.width, m.height
    );
    omt_common::text::write_json_f64(&mut out, m.frame_rate);
    let _ =
        write!(out, ",\"channels\":{},\"sample_rate\":{},\"error\":", m.channels, m.sample_rate);
    json::write_string(&mut out, &detail);
    out.push('}');
    println!("{out}");
    Ok(if ok { ExitCode::SUCCESS } else { ExitCode::from(3) })
}

fn play_command(o: &Options) -> Result<ExitCode, Error> {
    o.allowed(&["--target", "--connector", "--status-file", "--retry-seconds", "--video-ceiling"])?;
    let target = o.required("--target")?;
    let path = o.required("--status-file")?;
    let retry = o.number("--retry-seconds", 2, 1, 30)?;
    let preference = o.find("--connector").and_then(Option::as_deref).unwrap_or("auto");
    // The default is the Pi 5 tier; the installer always passes the board's
    // own ceiling, so this only applies to a hand-run receiver.
    let ceiling = VideoCeiling::parse(
        o.find("--video-ceiling").and_then(Option::as_deref).unwrap_or("1920x1080@60"),
    )?;
    if !is_valid_target(target) || !["auto", "HDMI-A-1", "HDMI-A-2"].contains(&preference) {
        return Err(Error::new("Invalid play options."));
    }
    install_stop_signals()
        .map_err(|e| Error::new(format!("Unable to install the shutdown handler: {e}")))?;
    let status = Arc::new(PlaybackStatus::new(path, target));
    let options = play::PlayOptions {
        target: target.to_owned(),
        preference: preference.to_owned(),
        retry_ms: retry * 1000,
        ceiling,
    };
    play::run(&options, &status, &STOP_REQUESTED);
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 2 && args[1] == "--version" {
        println!("{}", omt_common::VERSION);
        return ExitCode::SUCCESS;
    }
    if args.len() < 2 {
        return usage();
    }
    let options = match Options::parse(&args[2..]) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let result = match args[1].as_str() {
        "discover" => discover(&options),
        "probe" => probe(&options),
        "play" => play_command(&options),
        _ => return usage(),
    };
    result.unwrap_or_else(|e| {
        eprintln!("{e}");
        ExitCode::from(2)
    })
}
