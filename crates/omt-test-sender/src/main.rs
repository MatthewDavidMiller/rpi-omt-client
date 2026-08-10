// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// A bounded, first-party OMT source for exercising the appliance receiver.
// The video bodies are the repository's reference-encoded VMX1 conformance
// frames; audio is generated directly in OMT's planar FPA1 representation.

#![forbid(unsafe_code)]

use omt_protocol::{
    AUDIO_HEADER_SIZE, Codec, FrameType, HEADER_SIZE, METADATA_MAX_SIZE, VIDEO_HEADER_SIZE,
    parse_frame_header,
};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_PORT_FIRST: u16 = 6400;
const DEFAULT_PORT_LAST: u16 = 6600;
const MAX_CLIENTS: usize = 8;
const FRAME_RATE: u64 = 60;
const OMT_TICKS_PER_SECOND: u64 = 10_000_000;
const AUDIO_SAMPLE_RATE: i32 = 48_000;
const AUDIO_SAMPLES: i32 = 800;
const SUBSCRIPTION_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const GRADIENT_VMX: &[u8] = include_bytes!("../../../tests/vectors/vmx/gradient-1920x1080-709.vmx");
const FLAT_VMX: &[u8] = include_bytes!("../../../tests/vectors/vmx/flat-1920x1080-709.vmx");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Options {
    bind: IpAddr,
    port: Option<u16>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Command {
    Run(Options),
    Help,
    Version,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Subscription {
    Video,
    Audio,
}

struct ClientGuard(Arc<AtomicUsize>);

impl Drop for ClientGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn usage() -> &'static str {
    "Usage: omt-test-sender [--bind IP] [--port PORT]\n\
     \n\
     Streams reference VMX1 1920x1080p60 video and stereo FPA1 audio.\n\
     Without --port, the first available TCP port in 6400-6600 is used."
}

fn parse_options<I, S>(arguments: I) -> Result<Command, String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut values = arguments.into_iter().map(Into::into);
    let mut bind = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
    let mut port = None;
    while let Some(argument) = values.next() {
        match argument.as_str() {
            "--help" | "-h" => return Ok(Command::Help),
            "--version" | "-V" => return Ok(Command::Version),
            "--bind" => {
                let value = values
                    .next()
                    .ok_or_else(|| "--bind requires an IP address".to_owned())?;
                bind = value
                    .parse()
                    .map_err(|_| format!("invalid bind IP address: {value}"))?;
            }
            "--port" => {
                let value = values
                    .next()
                    .ok_or_else(|| "--port requires a TCP port".to_owned())?;
                let parsed = value
                    .parse::<u16>()
                    .map_err(|_| format!("invalid TCP port: {value}"))?;
                if parsed == 0 {
                    return Err("TCP port must be between 1 and 65535".to_owned());
                }
                port = Some(parsed);
            }
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    Ok(Command::Run(Options { bind, port }))
}

fn listen(options: Options) -> io::Result<TcpListener> {
    if let Some(port) = options.port {
        return TcpListener::bind(SocketAddr::new(options.bind, port));
    }
    let mut last_error = None;
    for port in DEFAULT_PORT_FIRST..=DEFAULT_PORT_LAST {
        match TcpListener::bind(SocketAddr::new(options.bind, port)) {
            Ok(listener) => return Ok(listener),
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => last_error = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(last_error
        .unwrap_or_else(|| io::Error::new(io::ErrorKind::AddrInUse, "no OMT port available")))
}

fn append_fixed_header(
    output: &mut Vec<u8>,
    frame_type: FrameType,
    data_length: usize,
) -> io::Result<()> {
    let length = u32::try_from(data_length)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "OMT frame is too large"))?;
    output.extend([1, frame_type as u8]);
    output.extend(0_i64.to_le_bytes());
    output.extend(0_u16.to_le_bytes());
    output.extend(length.to_le_bytes());
    Ok(())
}

fn video_frame(vmx: &[u8]) -> io::Result<Vec<u8>> {
    let data_length = VIDEO_HEADER_SIZE
        .checked_add(vmx.len())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "VMX frame is too large"))?;
    let mut output = Vec::with_capacity(HEADER_SIZE + data_length);
    append_fixed_header(&mut output, FrameType::Video, data_length)?;
    output.extend((Codec::Vmx1 as i32).to_le_bytes());
    output.extend(1920_i32.to_le_bytes());
    output.extend(1080_i32.to_le_bytes());
    output.extend(60_i32.to_le_bytes());
    output.extend(1_i32.to_le_bytes());
    output.extend((16.0_f32 / 9.0).to_le_bytes());
    output.extend(0_u32.to_le_bytes());
    output.extend(709_i32.to_le_bytes());
    output.extend(vmx);
    Ok(output)
}

fn audio_frame() -> io::Result<Vec<u8>> {
    let samples = usize::try_from(AUDIO_SAMPLES)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid audio sample count"))?;
    let payload_length = samples * 2 * size_of::<f32>();
    let data_length = AUDIO_HEADER_SIZE + payload_length;
    let mut output = Vec::with_capacity(HEADER_SIZE + data_length);
    append_fixed_header(&mut output, FrameType::Audio, data_length)?;
    output.extend((Codec::Fpa1 as i32).to_le_bytes());
    output.extend(AUDIO_SAMPLE_RATE.to_le_bytes());
    output.extend(AUDIO_SAMPLES.to_le_bytes());
    output.extend(2_i32.to_le_bytes());
    output.extend(0b11_u32.to_le_bytes());
    output.extend(0_i32.to_le_bytes());

    // A modest, deterministic 480 Hz tone. Its eight cycles per 800-sample
    // block repeat without a boundary click. FPA1 is planar, so each channel's
    // complete sample block precedes the next channel.
    for channel in 0..2 {
        let mut phase = 0.0_f32;
        let phase_step = 480.0_f32 * std::f32::consts::TAU / 48_000.0_f32;
        for _ in 0..samples {
            let value = if channel == 0 {
                phase.sin() * 0.1
            } else {
                phase.cos() * 0.1
            };
            output.extend(value.to_le_bytes());
            phase += phase_step;
        }
    }
    Ok(output)
}

fn set_timestamp(frame: &mut [u8], timestamp: i64) {
    frame[2..10].copy_from_slice(&timestamp.to_le_bytes());
}

fn read_subscription(stream: &mut TcpStream) -> io::Result<Subscription> {
    stream.set_read_timeout(Some(SUBSCRIPTION_TIMEOUT))?;
    for _ in 0..4 {
        let mut fixed = [0_u8; HEADER_SIZE];
        stream.read_exact(&mut fixed)?;
        let header = parse_frame_header(&fixed).map_err(io::Error::other)?;
        if header.frame_type != FrameType::Metadata {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "expected an OMT metadata subscription",
            ));
        }
        let length = usize::try_from(header.data_length).map_err(io::Error::other)?;
        if length == 0 || length > METADATA_MAX_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid OMT subscription length",
            ));
        }
        let mut metadata = vec![0_u8; length];
        stream.read_exact(&mut metadata)?;
        let xml = std::str::from_utf8(&metadata)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "subscription is not UTF-8"))?;
        if xml.contains("Video=\"true\"") {
            return Ok(Subscription::Video);
        }
        if xml.contains("Audio=\"true\"") {
            return Ok(Subscription::Audio);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "receiver did not request video or audio",
    ))
}

fn stream_frames(stream: &mut TcpStream, subscription: Subscription) -> io::Result<()> {
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let mut frames = match subscription {
        Subscription::Video => vec![video_frame(GRADIENT_VMX)?, video_frame(FLAT_VMX)?],
        Subscription::Audio => vec![audio_frame()?],
    };
    let started = Instant::now();
    let mut sequence = 0_u64;
    loop {
        let timestamp_u64 = sequence.saturating_mul(OMT_TICKS_PER_SECOND) / FRAME_RATE;
        let timestamp = i64::try_from(timestamp_u64)
            .map_err(|_| io::Error::other("OMT timestamp exhausted"))?;
        let index =
            usize::try_from(sequence % u64::try_from(frames.len()).map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
        let frame = frames
            .get_mut(index)
            .ok_or_else(|| io::Error::other("missing test frame"))?;
        set_timestamp(frame, timestamp);
        stream.write_all(frame)?;
        sequence = sequence.saturating_add(1);

        let elapsed_ns = sequence.saturating_mul(1_000_000_000) / FRAME_RATE;
        let due = started + Duration::from_nanos(elapsed_ns);
        if let Some(delay) = due.checked_duration_since(Instant::now()) {
            thread::sleep(delay);
        }
    }
}

fn handle_client(mut stream: TcpStream, peer: SocketAddr) {
    let result = read_subscription(&mut stream).and_then(|subscription| {
        eprintln!("{peer} subscribed to {subscription:?}");
        stream_frames(&mut stream, subscription)
    });
    if let Err(error) = result {
        eprintln!("{peer} disconnected: {error}");
    }
}

fn serve(listener: TcpListener) -> io::Result<()> {
    let address = listener.local_addr()?;
    println!("OMT test sender listening on omt://{address}");
    println!("Video: VMX1 1920x1080p60; audio: FPA1 stereo 48 kHz");
    let clients = Arc::new(AtomicUsize::new(0));
    for incoming in listener.incoming() {
        let stream = incoming?;
        let peer = stream.peer_addr()?;
        if clients.fetch_add(1, Ordering::AcqRel) >= MAX_CLIENTS {
            clients.fetch_sub(1, Ordering::AcqRel);
            eprintln!("refused {peer}: client limit reached");
            continue;
        }
        let guard = ClientGuard(Arc::clone(&clients));
        thread::Builder::new()
            .name("omt-test-client".to_owned())
            .spawn(move || {
                let _guard = guard;
                handle_client(stream, peer);
            })?;
    }
    Ok(())
}

fn run() -> Result<(), String> {
    match parse_options(std::env::args().skip(1))? {
        Command::Help => println!("{}", usage()),
        Command::Version => println!("omt-test-sender {}", env!("CARGO_PKG_VERSION")),
        Command::Run(options) => {
            let listener = listen(options).map_err(|error| format!("unable to listen: {error}"))?;
            serve(listener).map_err(|error| format!("sender failed: {error}"))?;
        }
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("ERROR: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omt_protocol::{parse_audio_header, parse_video_header};

    #[test]
    fn defaults_are_source_scoped_by_the_lifecycle_tool() {
        assert_eq!(
            parse_options(Vec::<String>::new()),
            Ok(Command::Run(Options {
                bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                port: None,
            }))
        );
    }

    #[test]
    fn explicit_cli_is_parsed_without_an_argument_dependency() {
        assert_eq!(
            parse_options(["--bind", "127.0.0.1", "--port", "6500"]),
            Ok(Command::Run(Options {
                bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
                port: Some(6500),
            }))
        );
        assert!(parse_options(["--port", "0"]).is_err());
        assert!(parse_options(["--bind", "host.example"]).is_err());
    }

    #[test]
    fn reference_video_is_a_valid_receiver_frame() {
        let wire = video_frame(GRADIENT_VMX).unwrap_or_else(|error| panic!("{error}"));
        let header =
            parse_frame_header(&wire[..HEADER_SIZE]).unwrap_or_else(|error| panic!("{error}"));
        let video = parse_video_header(&header, &wire[HEADER_SIZE..])
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(video.codec, Codec::Vmx1 as i32);
        assert_eq!((video.width, video.height), (1920, 1080));
        assert_eq!(
            wire.len(),
            HEADER_SIZE + VIDEO_HEADER_SIZE + GRADIENT_VMX.len()
        );
    }

    #[test]
    fn generated_audio_is_valid_planar_fpa1() {
        let wire = audio_frame().unwrap_or_else(|error| panic!("{error}"));
        let header =
            parse_frame_header(&wire[..HEADER_SIZE]).unwrap_or_else(|error| panic!("{error}"));
        let audio = parse_audio_header(&header, &wire[HEADER_SIZE..])
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(audio.codec, Codec::Fpa1 as i32);
        assert_eq!(audio.sample_rate, AUDIO_SAMPLE_RATE);
        assert_eq!(audio.samples_per_channel, AUDIO_SAMPLES);
        assert_eq!(audio.active_channels, 0b11);
    }

    #[test]
    fn timestamp_updates_only_the_fixed_header_timestamp() {
        let mut wire = video_frame(FLAT_VMX).unwrap_or_else(|error| panic!("{error}"));
        let previous = wire.clone();
        set_timestamp(&mut wire, 123_456);
        assert_eq!(&wire[2..10], &123_456_i64.to_le_bytes());
        assert_eq!(&wire[..2], &previous[..2]);
        assert_eq!(&wire[10..], &previous[10..]);
    }
}
