//! omt-test-sender: a bounded, first-party OMT source for exercising the
//! appliance receiver. The video bodies are the repository's
//! reference-encoded VMX1 conformance frames; audio is generated directly in
//! OMT's planar FPA1 representation.

#![forbid(unsafe_code)]
#![allow(clippy::print_stdout)]

use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use omt_protocol::{
    AUDIO_HEADER_SIZE, CODEC_FPA1, CODEC_VMX1, FrameType, HEADER_SIZE, METADATA_MAX_SIZE,
    VIDEO_HEADER_SIZE, parse_frame_header,
};

const DEFAULT_PORT_FIRST: u16 = 6400;
const DEFAULT_PORT_LAST: u16 = 6600;
const MAX_CLIENTS: usize = 8;
const DEFAULT_FRAME_RATE: u32 = 60;
/// The receiver's absolute ceiling; a sender above it could only be refused.
const MAX_FRAME_RATE: u32 = 60;
const OMT_TICKS_PER_SECOND: u64 = 10_000_000;
const AUDIO_SAMPLE_RATE: i32 = 48000;
const AUDIO_SAMPLES: usize = 800;
const AUDIO_BLOCKS_PER_SECOND: u64 = 48000 / 800;
const SUBSCRIPTION_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const CLIENT_STACK: usize = 256 * 1024;

/// The two reference frames, embedded at build time.
static GRADIENT: &[u8] = include_bytes!("../../../tests/vectors/vmx/gradient-1920x1080-709.vmx");
static FLAT: &[u8] = include_bytes!("../../../tests/vectors/vmx/flat-1920x1080-709.vmx");

struct Options {
    bind: IpAddr,
    port: Option<u16>,
    frame_rate: u32,
}

enum Parsed {
    Run(Options),
    Help,
    Version,
}

fn usage() {
    println!(
        "Usage: omt-test-sender [--bind IP] [--port PORT] [--frame-rate FPS]\n\n\
         Streams reference VMX1 1920x1080 video and stereo FPA1 audio.\n\
         Without --port, the first available TCP port in 6400-6600 is used.\n\
         Without --frame-rate, video is announced and paced at 60 fps."
    );
}

fn number(text: &str, max: u64) -> Option<u64> {
    omt_common::buf::parse_u64(text.strip_prefix('+').unwrap_or(text).as_bytes(), max)
}

fn parse_options(args: &[String]) -> Result<Parsed, String> {
    let mut o = Options {
        bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        port: None,
        frame_rate: DEFAULT_FRAME_RATE,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--help" | "-h" => return Ok(Parsed::Help),
            "--version" | "-V" => return Ok(Parsed::Version),
            "--bind" => {
                let v = it.next().ok_or("--bind requires an IP address")?;
                o.bind = v.parse().map_err(|_| format!("invalid bind IP address: {v}"))?;
            }
            "--port" => {
                let v = it.next().ok_or("--port requires a TCP port")?;
                let port = number(v, 65535).ok_or_else(|| format!("invalid TCP port: {v}"))?;
                if port == 0 {
                    return Err("TCP port must be between 1 and 65535".to_owned());
                }
                o.port = u16::try_from(port).ok();
            }
            "--frame-rate" => {
                let v = it.next().ok_or("--frame-rate requires whole frames per second")?;
                let rate = number(v, u64::from(u32::MAX))
                    .ok_or_else(|| format!("invalid frame rate: {v}"))?;
                if rate == 0 || rate > u64::from(MAX_FRAME_RATE) {
                    return Err(format!("frame rate must be between 1 and {MAX_FRAME_RATE}"));
                }
                o.frame_rate = u32::try_from(rate).unwrap_or(DEFAULT_FRAME_RATE);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Parsed::Run(o))
}

fn listen_on(o: &Options) -> Result<TcpListener, String> {
    let (first, last) = o.port.map_or((DEFAULT_PORT_FIRST, DEFAULT_PORT_LAST), |p| (p, p));
    let mut last_error = std::io::Error::from(ErrorKind::AddrInUse);
    for port in first..=last {
        match TcpListener::bind(SocketAddr::new(o.bind, port)) {
            Ok(l) => return Ok(l),
            Err(e) => {
                let in_use = e.kind() == ErrorKind::AddrInUse;
                last_error = e;
                if !in_use || o.port.is_some() {
                    break;
                }
            }
        }
    }
    Err(format!("unable to listen: {last_error}"))
}

fn fixed_header(b: &mut Vec<u8>, kind: FrameType, data_length: usize) {
    let mut h = [0_u8; HEADER_SIZE];
    h[0] = 1;
    h[1] = kind as u8;
    h[12..16].copy_from_slice(&u32::try_from(data_length).unwrap_or(u32::MAX).to_le_bytes());
    b.extend_from_slice(&h);
}

fn put_i32(b: &mut Vec<u8>, v: i32) {
    b.extend_from_slice(&v.to_le_bytes());
}

fn video_frame(vmx: &[u8], frame_rate: u32) -> Vec<u8> {
    let mut b = Vec::with_capacity(HEADER_SIZE + VIDEO_HEADER_SIZE as usize + vmx.len());
    fixed_header(&mut b, FrameType::Video, VIDEO_HEADER_SIZE as usize + vmx.len());
    for v in [CODEC_VMX1, 1920, 1080, i32::try_from(frame_rate).unwrap_or(60), 1] {
        put_i32(&mut b, v);
    }
    b.extend_from_slice(&(16.0_f32 / 9.0).to_le_bytes());
    put_i32(&mut b, 0);
    put_i32(&mut b, 709);
    b.extend_from_slice(vmx);
    b
}

/// A modest, deterministic 480 Hz tone. Its eight cycles per 800-sample
/// block repeat without a boundary click. FPA1 is planar: each channel's
/// complete block precedes the next channel.
fn audio_frame() -> Vec<u8> {
    let payload = AUDIO_SAMPLES * 2 * 4;
    let mut b = Vec::with_capacity(HEADER_SIZE + AUDIO_HEADER_SIZE as usize + payload);
    fixed_header(&mut b, FrameType::Audio, AUDIO_HEADER_SIZE as usize + payload);
    for v in [CODEC_FPA1, AUDIO_SAMPLE_RATE, 800, 2, 3, 0] {
        put_i32(&mut b, v);
    }
    let step = 480.0_f32 * 6.283_185_5 / 48000.0;
    for channel in 0..2 {
        let mut phase = 0.0_f32;
        for _ in 0..AUDIO_SAMPLES {
            let v = if channel == 0 { phase.sin() } else { phase.cos() } * 0.1;
            b.extend_from_slice(&v.to_le_bytes());
            phase += step;
        }
    }
    b
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Subscription {
    Video,
    Audio,
}

fn read_subscription(s: &mut TcpStream) -> Result<Subscription, String> {
    s.set_read_timeout(Some(SUBSCRIPTION_TIMEOUT)).map_err(|e| e.to_string())?;
    let fill = "failed to fill whole buffer";
    for _ in 0..4 {
        let mut fixed = [0_u8; HEADER_SIZE];
        s.read_exact(&mut fixed).map_err(|_| fill.to_owned())?;
        let h = parse_frame_header(&fixed).map_err(|e| e.to_string())?;
        if h.frame_type != FrameType::Metadata {
            return Err("expected an OMT metadata subscription".to_owned());
        }
        if h.data_length == 0 || h.data_length > METADATA_MAX_SIZE {
            return Err("invalid OMT subscription length".to_owned());
        }
        let mut xml = vec![0_u8; h.data_length as usize];
        s.read_exact(&mut xml).map_err(|_| fill.to_owned())?;
        let has = |needle: &[u8]| xml.windows(needle.len()).any(|w| w == needle);
        if has(b"Video=\"true\"") {
            return Ok(Subscription::Video);
        }
        if has(b"Audio=\"true\"") {
            return Ok(Subscription::Audio);
        }
    }
    Err("receiver did not request video or audio".to_owned())
}

fn stream_frames(s: &mut TcpStream, sub: Subscription, frame_rate: u32) -> String {
    if let Err(e) = s.set_write_timeout(Some(WRITE_TIMEOUT)) {
        return e.to_string();
    }
    let mut frames = if sub == Subscription::Video {
        vec![video_frame(GRADIENT, frame_rate), video_frame(FLAT, frame_rate)]
    } else {
        vec![audio_frame()]
    };
    // Audio keeps its own cadence: each block is a fixed number of samples
    // at a fixed rate, so pacing it off the video rate would distort the
    // sound.
    let rate =
        if sub == Subscription::Video { u64::from(frame_rate) } else { AUDIO_BLOCKS_PER_SECOND };
    let started = Instant::now();
    let count = frames.len() as u64;
    for sequence in 0_u64.. {
        let timestamp = sequence * OMT_TICKS_PER_SECOND / rate;
        let frame = &mut frames[usize::try_from(sequence % count).unwrap_or(0)];
        frame[2..10].copy_from_slice(&timestamp.to_le_bytes());
        if let Err(e) = s.write_all(frame) {
            return e.to_string();
        }
        let due = started + Duration::from_nanos((sequence + 1) * 1_000_000_000 / rate);
        if let Some(delay) = due.checked_duration_since(Instant::now()) {
            thread::sleep(delay);
        }
    }
    String::new()
}

fn format_address(a: &SocketAddr) -> String {
    match a {
        SocketAddr::V6(v6) => format!("[{}]:{}", v6.ip(), v6.port()),
        SocketAddr::V4(v4) => v4.to_string(),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let o = match parse_options(&args) {
        Ok(Parsed::Run(o)) => o,
        Ok(Parsed::Help) => {
            usage();
            return ExitCode::SUCCESS;
        }
        Ok(Parsed::Version) => {
            // The bare semantic version.
            println!("omt-test-sender {}", omt_common::VERSION.trim_start_matches('v'));
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("ERROR: {e}");
            return ExitCode::FAILURE;
        }
    };
    let listener = match listen_on(&o) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("ERROR: {e}");
            return ExitCode::FAILURE;
        }
    };
    let local = listener.local_addr().map_or_else(|_| "?".to_owned(), |a| format_address(&a));
    println!("OMT test sender listening on omt://{local}");
    println!("Video: VMX1 1920x1080p{}; audio: FPA1 stereo 48 kHz", o.frame_rate);
    let _ = std::io::stdout().flush();
    let clients = Arc::new(AtomicUsize::new(0));
    loop {
        let (mut stream, peer) = match listener.accept() {
            Ok(x) => x,
            Err(e) if matches!(e.kind(), ErrorKind::Interrupted | ErrorKind::ConnectionAborted) => {
                continue;
            }
            Err(e) => {
                eprintln!("ERROR: sender failed: {e}");
                return ExitCode::FAILURE;
            }
        };
        let name = format_address(&peer);
        if clients.fetch_add(1, Ordering::SeqCst) >= MAX_CLIENTS {
            clients.fetch_sub(1, Ordering::SeqCst);
            eprintln!("refused {name}: client limit reached");
            continue;
        }
        let counter = Arc::clone(&clients);
        let frame_rate = o.frame_rate;
        let spawned = thread::Builder::new().stack_size(CLIENT_STACK).spawn(move || {
            let error = match read_subscription(&mut stream) {
                Ok(sub) => {
                    let kind = if sub == Subscription::Video { "Video" } else { "Audio" };
                    eprintln!("{name} subscribed to {kind}");
                    stream_frames(&mut stream, sub, frame_rate)
                }
                Err(e) => e,
            };
            eprintln!("{name} disconnected: {error}");
            counter.fetch_sub(1, Ordering::SeqCst);
        });
        if spawned.is_err() {
            eprintln!("ERROR: sender failed: unable to start a client thread");
            return ExitCode::FAILURE;
        }
    }
}
