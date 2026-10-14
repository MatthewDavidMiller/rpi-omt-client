//! Port of the receiver suite: the channel's timing contract against live
//! loopback sockets, the frame handoff, connector selection over a scratch
//! sysfs tree, the scaler, mode selection, decode classification, the
//! running details, the interleaver, discovery parsing, and the D-Bus
//! marshalling.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::float_cmp, clippy::cast_possible_truncation, clippy::many_single_char_names)]
#![allow(
    clippy::needless_pass_by_value,
    clippy::disallowed_methods,
    clippy::assertions_on_constants
)]

use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use omt_common::timefmt::{now_ms, remaining_ms};
use omt_protocol::{
    AudioHeader, FrameHeader, FrameType, VIDEO_HEADER_SIZE, VideoHeader, build_metadata,
};
use omt_receiver::audio::{interleave, prefill_frames, target_frames};
use omt_receiver::channel::{
    BODY_BUDGET_MS, Channel, Endpoint, Frame, Recv, body_deadline, resolve,
};
use omt_receiver::connector::{alsa_device_in, find, named_in};
use omt_receiver::dbus::{Arg, METHOD_CALL, Message, build_call, message_length};
use omt_receiver::discovery::{endpoint_from_parts, read_announcement};
use omt_receiver::handoff::{
    AUDIO_BYTE_CAP, Queue, QueueKind, SPARE_BUFFERS, VIDEO_BYTE_CAP, audio_interval_ns,
};
use omt_receiver::play::{
    CONNECT_TIMEOUT_MS, MEDIA_GRACE_MS, MEDIA_STALL_MS, RECEIVE_SLICE_MS, RECOVER_ATTEMPTS,
    RECOVER_BACKOFF_MS, RECOVER_TIMEOUT_MS, RunningDetail, describe_audio, describe_running,
};
use omt_receiver::scale::{Placement, Scaler, sample};
use omt_receiver::video::{
    ModeShape, Present, SKIP_BUDGET, choose_mode, classify_decode, describe_presentation,
};
use omt_vmx::Status;

// ------------------------------------------------------------- channel

struct Plan {
    header_delay_ms: u64,
    send_header: bool,
    send_one_byte: bool,
    hold_ms: u64,
}

fn serve(plan: Plan) -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut frame = Vec::new();
        build_metadata(b"<OMTPayload/>", 7, &mut frame).unwrap();
        if plan.send_header {
            let _ = s.write_all(&frame[..16]);
            if plan.send_one_byte {
                let _ = s.write_all(b"x");
            } else {
                thread::sleep(Duration::from_millis(plan.header_delay_ms));
                let _ = s.write_all(&frame[16..]);
            }
        }
        thread::sleep(Duration::from_millis(plan.hold_ms));
    });
    (port, handle)
}

fn connect_to(port: u16) -> Channel {
    let mut c = Channel::new();
    c.connect(
        &Endpoint { host: "127.0.0.1".to_owned(), port },
        FrameType::Metadata,
        now_ms() + 5000,
    )
    .unwrap();
    c
}

fn payload_case(delay_ms: u64) {
    let (port, t) = serve(Plan {
        header_delay_ms: delay_ms,
        send_header: true,
        send_one_byte: false,
        hold_ms: 500,
    });
    let mut c = connect_to(port);
    let (status, err) = c.receive(now_ms() + 50);
    assert_eq!(status, Recv::Ok, "{err:?}");
    assert_eq!(c.frame.header.unwrap().timestamp, 7);
    assert_eq!(c.frame.payload(), b"<OMTPayload/>");
    assert!(c.connected());
    drop(c);
    t.join().unwrap();
}

#[test]
fn a_payload_arriving_after_the_slice_is_received() {
    payload_case(300);
}

#[test]
fn a_payload_paused_for_three_and_a_half_seconds_completes() {
    payload_case(3500);
}

#[test]
fn an_idle_connected_socket_would_block_without_closing() {
    let (port, t) =
        serve(Plan { header_delay_ms: 0, send_header: false, send_one_byte: false, hold_ms: 500 });
    let mut c = connect_to(port);
    assert_eq!(c.receive(now_ms() + 50).0, Recv::WouldBlock);
    assert!(c.connected());
    drop(c);
    t.join().unwrap();
}

#[test]
fn a_body_that_never_finishes_is_truncated() {
    let (port, t) = serve(Plan {
        header_delay_ms: 0,
        send_header: true,
        send_one_byte: true,
        hold_ms: BODY_BUDGET_MS + 2000,
    });
    let mut c = connect_to(port);
    let (status, err) = c.receive(now_ms() + 50);
    assert_eq!(status, Recv::TimedOut);
    assert!(err.unwrap().message().contains("truncated by a timeout"));
    assert!(!c.connected());
    t.join().unwrap();
}

/// A ready-only read takes a frame already in the socket, leaves the next
/// one -- whose header has landed but whose body has not -- partly read
/// without waiting for it, and finishes it once the rest arrives.
#[test]
fn a_frame_still_arriving_does_not_hold_back_a_complete_one() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (go, wait) = std::sync::mpsc::channel::<()>();
    let t = thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let (mut first, mut second) = (Vec::new(), Vec::new());
        build_metadata(b"<first/>", 1, &mut first).unwrap();
        build_metadata(b"<second/>", 2, &mut second).unwrap();
        s.write_all(&first).unwrap();
        s.write_all(&second[..20]).unwrap();
        wait.recv().unwrap();
        s.write_all(&second[20..]).unwrap();
        thread::sleep(Duration::from_millis(500));
    });
    let mut c = connect_to(port);
    // Let both writes land before the ready-only reads.
    thread::sleep(Duration::from_millis(100));
    assert_eq!(c.receive_ready().0, Recv::Ok);
    assert_eq!(c.frame.payload(), b"<first/>");
    let started = now_ms();
    assert_eq!(c.receive_ready().0, Recv::WouldBlock);
    assert!(now_ms() - started < 50, "a ready-only read must not wait");
    assert!(c.connected());
    go.send(()).unwrap();
    let (status, err) = c.receive(now_ms() + 2000);
    assert_eq!(status, Recv::Ok, "{err:?}");
    assert_eq!(c.frame.header.unwrap().timestamp, 2);
    assert_eq!(c.frame.payload(), b"<second/>");
    drop(c);
    t.join().unwrap();
}

fn frame_with(len: usize, metadata_length: u16) -> Frame {
    Frame {
        header: Some(FrameHeader {
            frame_type: FrameType::Video,
            timestamp: 0,
            metadata_length,
            data_length: 0,
        }),
        buffer: vec![0; len.max(100)],
        len,
        ..Frame::default()
    }
}

#[test]
fn deadlines_and_media_slices() {
    assert_eq!(remaining_ms(now_ms().saturating_sub(5000)), 0);
    assert!(remaining_ms(body_deadline(now_ms().saturating_sub(5000))) > 1000);
    let generous = now_ms() + BODY_BUDGET_MS + 30000;
    assert_eq!(body_deadline(generous), generous);
    const { assert!(BODY_BUDGET_MS > 3000 && BODY_BUDGET_MS < 8000) };
    assert_eq!(frame_with(100, 10).media(32).map(<[u8]>::len), Some(58));
    assert_eq!(frame_with(100, 10).media(0).map(<[u8]>::len), Some(90));
    assert_eq!(frame_with(32, 0).media(32).map(<[u8]>::len), Some(0));
    assert!(frame_with(8, 10).media(0).is_none());
    assert!(frame_with(40, 10).media(32).is_none());
    assert!(frame_with(100, 10).media(usize::MAX).is_none());
}

#[test]
fn resolve_drops_an_ipv6_zone() {
    let scoped = resolve(&Endpoint { host: "fe80::1%eth0".to_owned(), port: 6400 }).unwrap();
    assert_eq!(scoped.len(), 1);
    assert!(scoped[0].is_ipv6() && scoped[0].port() == 6400);
    assert_eq!(resolve(&Endpoint { host: "127.0.0.1".to_owned(), port: 1 }).unwrap().len(), 1);
    assert!(
        resolve(&Endpoint { host: "not a host".to_owned(), port: 1 })
            .map_or(true, |a| a.is_empty())
    );
}

#[test]
fn a_failed_reconnect_leaves_the_channel_down() {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut c = Channel::new();
    let ep = Endpoint { host: "127.0.0.1".to_owned(), port };
    assert!(c.connect(&ep, FrameType::Video, now_ms() + RECOVER_TIMEOUT_MS).is_err());
    assert!(!c.connected());
}

// ------------------------------------------------------------- handoff

fn video_frame(payload: usize) -> Frame {
    let len = VIDEO_HEADER_SIZE as usize + payload;
    Frame {
        header: Some(FrameHeader {
            frame_type: FrameType::Video,
            timestamp: 0,
            metadata_length: 0,
            data_length: 0,
        }),
        video: Some(VideoHeader {
            codec: 0,
            width: 1920,
            height: 1080,
            frame_rate_n: 30,
            frame_rate_d: 1,
            aspect_ratio: 0.0,
            flags: 0,
            color_space: 0,
        }),
        buffer: vec![0; len],
        len,
        ..Frame::default()
    }
}

fn audio_frame(samples: i32, rate: i32) -> Frame {
    Frame {
        header: Some(FrameHeader {
            frame_type: FrameType::Audio,
            timestamp: 0,
            metadata_length: 0,
            data_length: 0,
        }),
        audio: Some(AudioHeader {
            codec: 0,
            sample_rate: rate,
            samples_per_channel: samples,
            channels: 2,
            active_channels: 3,
        }),
        buffer: vec![0; 24],
        len: 24,
        ..Frame::default()
    }
}

/// Video is one slot: each frame replaces whatever the display has not
/// taken, so nothing waits behind the newest picture.
#[test]
fn queue_contract() {
    let mut q = Queue::new(QueueKind::Video, VIDEO_BYTE_CAP);
    assert!(q.is_empty());
    assert_eq!(q.push(video_frame(32)), 0);
    assert_eq!(q.len(), 1);
    assert_eq!(q.push(video_frame(48)), 1);
    assert_eq!(q.push(video_frame(64)), 1);
    assert_eq!(q.len(), 1);
    assert_eq!(q.pop().unwrap().len, VIDEO_HEADER_SIZE as usize + 64);
    assert!(q.pop().is_none() && q.is_empty() && q.bytes() == 0);

    // Metadata, the wrong media kind, and an over-cap frame are refused
    // without disturbing what is held.
    let mut q = Queue::new(QueueKind::Video, 250);
    let meta = Frame {
        header: Some(FrameHeader {
            frame_type: FrameType::Metadata,
            timestamp: 0,
            metadata_length: 0,
            data_length: 0,
        }),
        ..Frame::default()
    };
    assert_eq!(q.push(meta), 0);
    assert!(q.is_empty());
    assert_eq!(q.push(audio_frame(960, 48000)), 0);
    assert!(q.is_empty());
    assert_eq!(q.push(video_frame(80)), 0);
    assert_eq!(q.push(video_frame(400)), 0);
    assert!(q.len() == 1 && q.bytes() <= 250);

    // The frame just superseded is the buffer the next receive reuses.
    let mut q = Queue::new(QueueKind::Video, VIDEO_BYTE_CAP);
    assert_eq!(q.take_spare().capacity(), 0);
    for _ in 0..10 {
        let _ = q.push(video_frame(64));
    }
    assert_eq!(q.spare_count(), SPARE_BUFFERS);
    assert!(q.take_spare().capacity() >= 64);
    q.recycle(Vec::new());
    assert_eq!(q.spare_count(), SPARE_BUFFERS - 1);
}

/// Audio keeps arrival order for the ALSA ring and sheds only a backlog
/// past the backlog limit.
#[test]
fn audio_queue_contract() {
    let h = AudioHeader {
        codec: 0,
        sample_rate: 48000,
        samples_per_channel: 48000,
        channels: 2,
        active_channels: 3,
    };
    assert_eq!(audio_interval_ns(&h), Some(1_000_000_000));
    assert_eq!(
        audio_interval_ns(&AudioHeader { sample_rate: 0, samples_per_channel: 960, ..h }),
        None
    );

    let mut q = Queue::new(QueueKind::Audio, AUDIO_BYTE_CAP);
    // 20 ms frames: five fit the 100 ms backlog, the rest push out the
    // oldest.
    for _ in 0..5 {
        assert_eq!(q.push(audio_frame(960, 48000)), 0);
    }
    assert_eq!(q.len(), 5);
    let dropped: u64 = (0..7).map(|_| q.push(audio_frame(960, 48000))).sum();
    assert_eq!((q.len(), dropped), (5, 7));
    assert_eq!(q.push(video_frame(32)), 0);
    assert_eq!(q.len(), 5);

    // A single frame longer than the backlog is still played, not
    // discarded.
    let mut q = Queue::new(QueueKind::Audio, AUDIO_BYTE_CAP);
    assert_eq!(q.push(audio_frame(48000, 48000)), 0);
    assert_eq!(q.len(), 1);
    assert_eq!(q.push(audio_frame(48000, 48000)), 1);
    assert_eq!(q.len(), 1);
}

// ----------------------------------------------------------- connector

struct Tree(PathBuf);

impl Tree {
    fn new(label: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("omt-connector-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for sub in ["sys", "dev", "sound"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }
        Self(root)
    }

    fn put(&self, rel: &str, content: &str) {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn card(&self, name: &str, status: Option<&str>, id: Option<&str>) {
        std::fs::create_dir_all(self.0.join("sys").join(name)).unwrap();
        if let Some(s) = status {
            self.put(&format!("sys/{name}/status"), s);
        }
        if let Some(i) = id {
            self.put(&format!("sys/{name}/connector_id"), i);
        }
    }

    fn find(&self, name: &str) -> Option<omt_receiver::connector::Hdmi> {
        named_in(&self.0.join("sys"), &self.0.join("dev"), &self.0.join("sound"), name)
    }

    fn sound(&self) -> PathBuf {
        self.0.join("sound")
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn connector_selection() {
    let t = Tree::new("order");
    t.card("card0-HDMI-A-1", Some("connected"), Some("32"));
    t.card("card1-HDMI-A-1", Some("connected"), Some("48"));
    t.put("dev/card0", "");
    t.put("dev/card1", "");
    let c = t.find("HDMI-A-1").unwrap();
    assert_eq!(c.id, 32);
    assert_eq!(c.alsa_device, "plughw:CARD=vc4hdmi0,DEV=0");

    let t = Tree::new("unreadable");
    t.card("card0-HDMI-A-1", None, Some("32"));
    t.card("card1-HDMI-A-1", Some("connected"), Some("48"));
    t.put("dev/card0", "");
    t.put("dev/card1", "");
    assert_eq!(t.find("HDMI-A-1").unwrap().id, 48);

    let t = Tree::new("skip");
    t.card("card0-HDMI-A-2", Some("disconnected"), Some("32"));
    t.card("card1-HDMI-A-2", Some("connected"), None);
    t.card("card2-HDMI-A-2", Some("connected"), Some("0"));
    t.card("card3-HDMI-A-2", Some("connected"), Some("64"));
    t.card("card4-HDMI-A-2", Some("connected"), Some("80"));
    for card in ["card0", "card1", "card2", "card4"] {
        t.put(&format!("dev/{card}"), "");
    }
    let c = t.find("HDMI-A-2").unwrap();
    assert_eq!(c.id, 80);
    assert_eq!(c.alsa_device, "plughw:CARD=vc4hdmi1,DEV=0");

    let t = Tree::new("none");
    t.card("card0-HDMI-A-1", Some("disconnected"), Some("32"));
    t.put("dev/card0", "");
    assert!(t.find("HDMI-A-1").is_none());
    assert!(t.find("HDMI-A-2").is_none());

    let t = Tree::new("renumber");
    t.card("card0-HDMI-A-1", Some("connected"), Some("32"));
    t.put("dev/card0", "");
    let c = t.find("HDMI-A-1").unwrap();
    assert!(c.is_connected());
    t.card("card0-HDMI-A-1", Some("connected"), Some("48"));
    assert!(!c.is_connected());
    t.card("card0-HDMI-A-1", Some("disconnected"), Some("32"));
    assert!(!c.is_connected());

    let t = Tree::new("audio");
    t.put("sound/card0/id", "vc4hdmi0");
    t.put("sound/card1/id", "vc4hdmi1");
    assert_eq!(alsa_device_in(&t.sound(), "HDMI-A-1"), "plughw:CARD=vc4hdmi0,DEV=0");
    assert_eq!(alsa_device_in(&t.sound(), "HDMI-A-2"), "plughw:CARD=vc4hdmi1,DEV=0");
    let t = Tree::new("single");
    t.put("sound/card0/id", "vc4hdmi");
    assert_eq!(alsa_device_in(&t.sound(), "HDMI-A-1"), "plughw:CARD=vc4hdmi,DEV=0");
    assert_eq!(alsa_device_in(&t.sound(), "HDMI-A-2"), "plughw:CARD=vc4hdmi,DEV=0");
    let t = Tree::new("mixed");
    t.put("sound/card0/id", "Headphones");
    t.put("sound/card1/id", "vc4hdmi0");
    t.put("sound/card2/id", "vc4hdmi1");
    assert_eq!(alsa_device_in(&t.sound(), "HDMI-A-2"), "plughw:CARD=vc4hdmi1,DEV=0");
    assert_eq!(
        alsa_device_in(Path::new("/nonexistent-omt-sound-root"), "HDMI-A-2"),
        "plughw:CARD=vc4hdmi1,DEV=0"
    );
    assert!(find("HDMI-A-9").is_none());
    assert!(find("../../etc").is_none());
}

// --------------------------------------------------------------- scale

#[test]
fn scaler_contract() {
    let p = Placement::fit(1920, 1080, 1280, 720).unwrap();
    assert_eq!(p, Placement { x: 0, y: 0, width: 1280, height: 720 });
    assert!(p.fills(1280, 720));
    assert_eq!(
        Placement::fit(1920, 1080, 1024, 768).unwrap(),
        Placement { x: 0, y: 96, width: 1024, height: 576 }
    );
    assert_eq!(
        Placement::fit(640, 480, 1280, 720).unwrap(),
        Placement { x: 160, y: 0, width: 960, height: 720 }
    );
    assert!(Placement::fit(0, 1080, 1280, 720).is_none());
    assert!(Placement::fit(1920, 1080, 1280, 0).is_none());
    let tiny = Placement::fit(1920, 16, 64, 64).unwrap();
    assert!(tiny.width >= 1 && tiny.height >= 1);
    assert_eq!((sample(0, 2, 4), sample(1, 2, 4)), (1, 3));
    for (i, want) in [0, 2, 3, 5].into_iter().enumerate() {
        assert_eq!(sample(i, 4, 6), want);
    }
    // The tables the decode resamples with: identity at the same size,
    // pixel-centre samples otherwise, rows never decreasing.
    let s = Scaler::new(4, 4, Placement::fit(4, 4, 4, 4).unwrap()).unwrap();
    assert_eq!(s.rows, [0, 1, 2, 3]);
    assert_eq!(s.columns, [0, 1, 2, 3]);
    let p = Placement::fit(1920, 1080, 1280, 720).unwrap();
    let s = Scaler::new(1920, 1080, p).unwrap();
    assert!(s.rows.windows(2).all(|w| w[1] >= w[0]));
    assert!(s.rows[p.height - 1] < 1080 && s.columns[p.width - 1] < 1920);
    assert_eq!(s.columns[1] as usize, sample(1, 1280, 1920));
    assert!(Scaler::new(0, 4, p).is_err());
}

// --------------------------------------------------------- mode select

fn shape(width: u16, height: u16, refresh: f64) -> ModeShape {
    ModeShape { width, height, refresh, interlaced: false }
}

fn choose(s: &[ModeShape], w: u16, h: u16, r: f64) -> Option<(usize, bool)> {
    choose_mode(s, w, h, r).map(|c| (c.index, c.scaled))
}

#[test]
fn mode_selection() {
    let a = [
        ModeShape { width: 1920, height: 1080, refresh: 59.94, interlaced: true },
        shape(1920, 1080, 50.0),
        shape(1920, 1080, 59.94),
        shape(1920, 1080, 60.0),
        shape(1280, 720, 59.94),
    ];
    assert_eq!(choose(&a, 1920, 1080, 59.94), Some((2, false)));
    assert_eq!(choose(&a, 1920, 1080, 60.0), Some((3, false)));
    assert_eq!(choose(&a, 1920, 1080, 50.0), Some((1, false)));
    assert_eq!(choose(&a, 1280, 720, 59.94), Some((4, false)));
    assert_eq!(choose(&a, 1920, 1080, 29.97), Some((2, false)));
    assert_eq!(choose(&a[..1], 1920, 1080, 59.94), None);
    assert_eq!(choose(&[], 1920, 1080, 60.0), None);

    let b = [
        shape(1280, 720, 60.0),
        shape(1280, 720, 59.94),
        shape(800, 600, 60.0),
        shape(640, 480, 60.0),
        ModeShape { width: 1920, height: 1080, refresh: 60.0, interlaced: true },
    ];
    assert_eq!(choose(&b, 1920, 1080, 30.0), Some((0, true)));
    assert_eq!(choose(&b, 1920, 1080, 59.94), Some((1, true)));
    assert_eq!(choose(&b, 1280, 720, 60.0), Some((0, false)));

    let d = [shape(1920, 1080, 60.0), shape(1280, 720, 60.0), shape(3840, 2160, 60.0)];
    assert_eq!(choose(&d, 640, 480, 60.0), Some((1, true)));
    assert_eq!(choose(&d, 1920, 1200, 60.0), Some((0, true)));
    assert_eq!(choose(&d[2..], 1920, 1080, 60.0), None);

    let e = [shape(1920, 1080, 24.0), shape(1920, 1080, 50.0)];
    assert_eq!(choose(&e, 1920, 1080, 25.0), Some((1, false)));
    assert_eq!(choose(&e, 1920, 1080, 24.0), Some((0, false)));
    let f = [shape(1920, 1080, 59.9401), shape(1920, 1080, 60.0)];
    assert_eq!(choose(&f, 1920, 1080, 59.94), Some((0, false)));
    assert_eq!(choose(&f, 1920, 1080, 59.9), Some((1, false)));
    assert_eq!(choose(&[shape(1920, 1080, 0.0)], 1920, 1080, 60.0), None);

    // A low source rate goes out at twice its rate when the display has it.
    let g = [
        shape(1920, 1080, 30.0),
        shape(1920, 1080, 60.0),
        shape(1920, 1080, 25.0),
        shape(1920, 1080, 50.0),
        shape(1920, 1080, 29.97),
        shape(1920, 1080, 59.94),
    ];
    assert_eq!(choose(&g, 1920, 1080, 30.0), Some((1, false)));
    assert_eq!(choose(&g, 1920, 1080, 25.0), Some((3, false)));
    assert_eq!(choose(&g, 1920, 1080, 29.97), Some((5, false)));
    assert_eq!(choose(&g, 1920, 1080, 50.0), Some((3, false)));
    assert_eq!(choose(&g[..1], 1920, 1080, 30.0), Some((0, false)));
    assert_eq!(
        choose(&[shape(1920, 1080, 30.0), shape(1920, 1080, 50.0)], 1920, 1080, 30.0),
        Some((0, false))
    );
}

#[test]
fn decode_classification() {
    let all = [
        Status::InvalidDimensions,
        Status::Empty,
        Status::Oversized,
        Status::Truncated,
        Status::InvalidFormat,
        Status::UnsupportedFormat,
        Status::SliceCount,
        Status::OutputSize,
        Status::WorkerFailure,
        Status::CorruptStream,
    ];
    for e in all {
        let local = matches!(
            e,
            Status::Truncated
                | Status::InvalidFormat
                | Status::Oversized
                | Status::SliceCount
                | Status::CorruptStream
        );
        let mut skips = 0;
        match classify_decode(e, true, &mut skips) {
            Present::Skipped => assert!(local),
            Present::Unsupported(_) => assert_eq!(e, Status::UnsupportedFormat),
            Present::Failed(_) => assert!(!local && e != Status::UnsupportedFormat),
            Present::Presented => panic!("{e}"),
        }
        let mut before = 0;
        assert_ne!(classify_decode(e, false, &mut before), Present::Skipped);
        assert_eq!(before, 0);
    }
    let mut skips = 0;
    for held in 1..=SKIP_BUDGET {
        assert_eq!(classify_decode(Status::CorruptStream, true, &mut skips), Present::Skipped);
        assert_eq!(skips, held);
    }
    assert_eq!(
        classify_decode(Status::CorruptStream, true, &mut skips),
        Present::Failed("VMX decoder rejected 21 consecutive frames: CorruptStream".to_owned())
    );
    assert_eq!(
        describe_presentation(false, 1920, 1080, true, 1280, 720),
        "Playing OMT video. Scaled from 1920x1080 to the display's 1280x720 mode."
    );
    assert_eq!(
        describe_presentation(true, 1280, 720, false, 0, 0),
        "Playing interlaced input progressively without deinterlacing."
    );
}

// ------------------------------------------------------------- details

#[test]
fn running_details() {
    let base = "Playing OMT video.";
    assert_eq!(describe_running(base, 0, 0, 0), base);
    assert_eq!(
        describe_running(base, 2, 0, 0),
        "Playing OMT video. 2 video reconnect(s) in this session."
    );
    assert_eq!(
        describe_running(base, 0, 3, 0),
        "Playing OMT video. 3 skipped frame(s) in this session."
    );
    assert_eq!(
        describe_running(base, 2, 3, 0),
        "Playing OMT video. 2 video reconnect(s) and 3 skipped frame(s) in this session."
    );
    assert_eq!(
        describe_running(base, 0, 0, 4),
        "Playing OMT video. 4 frame(s) replaced by a newer one before display in this session."
    );
    let mixed = describe_running(base, 1, 0, 4);
    assert!(
        mixed.contains("1 video reconnect(s)")
            && mixed.contains("4 frame(s) replaced")
            && !mixed.contains("skipped frame")
    );
    assert!(describe_running(base, u64::MAX, 0, 0).contains("18446744073709551615"));

    let mut cache = RunningDetail::default();
    assert_eq!(cache.get(base, 0, 0, 0), base);
    assert!(cache.get(base, 0, 0, 2).contains("2 frame(s) replaced"));
    assert!(
        cache
            .get("Playing OMT video and audio.", 0, 0, 2)
            .starts_with("Playing OMT video and audio.")
    );
    assert_eq!(describe_audio(0), "Playing OMT video and audio.");
    let audio = describe_audio(7);
    assert!(audio.starts_with("Playing OMT video and audio.") && audio.contains('7'));

    // The reconnect budget's bounds.
    let worst: u64 =
        (0..RECOVER_ATTEMPTS).map(|a| RECOVER_TIMEOUT_MS + RECOVER_BACKOFF_MS * u64::from(a)).sum();
    let refused: u64 = (0..RECOVER_ATTEMPTS).map(|a| RECOVER_BACKOFF_MS * u64::from(a)).sum();
    assert!(MEDIA_STALL_MS > MEDIA_GRACE_MS && MEDIA_STALL_MS > worst);
    assert!(MEDIA_STALL_MS > RECEIVE_SLICE_MS * 10 && MEDIA_STALL_MS <= 30000);
    assert!(worst <= 4000 && RECOVER_TIMEOUT_MS < CONNECT_TIMEOUT_MS && refused < 1000);
}

// --------------------------------------------------------------- audio

fn floats(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

#[test]
fn audio_contract() {
    assert_eq!(prefill_frames(48000, 11520, 960), 4800);
    assert_eq!(prefill_frames(44100, 10584, 882), 4410);
    assert_eq!(prefill_frames(48000, 512, 128), 512);
    assert_eq!(prefill_frames(48000, 16384, 8192), 8192);
    assert_eq!(prefill_frames(48000, 0, 0), 1);
    assert_eq!(prefill_frames(0, 11520, 960), 960);
    assert_eq!(prefill_frames(-1, 11520, 960), 960);
    assert_eq!(target_frames(48000, 11520, 4800), 7680);
    assert_eq!(target_frames(48000, 11520, 9000), 9000);
    assert_eq!(target_frames(48000, 4000, 1000), 4000);
    assert_eq!(target_frames(0, 11520, 960), 960);

    let mut out = [0.0_f32; 33];
    interleave(&floats(&[1.0, 2.0, 3.0, -1.0, -2.0, -3.0]), 3, 3, 2, &mut out[..6]).unwrap();
    assert_eq!(out[..6], [1.0, -1.0, 2.0, -2.0, 3.0, -3.0]);
    let body = floats(&[7.0, 8.0]);
    interleave(&body, 2, 2, 2, &mut out[..4]).unwrap();
    assert_eq!(out[..4], [0.0, 7.0, 0.0, 8.0]);
    interleave(&body, 1, 2, 2, &mut out[..4]).unwrap();
    assert_eq!(out[..4], [7.0, 0.0, 8.0, 0.0]);
    interleave(&[], 0, 2, 2, &mut out[..4]).unwrap();
    assert!(interleave(&floats(&[1.0, 2.0, 3.0]), 3, 2, 2, &mut out[..4]).is_err());
    assert!(interleave(&floats(&[1.0; 4]), 3, 2, 2, &mut out[..3]).is_err());
    interleave(&floats(&[1.0; 32]), u32::MAX, 1, 33, &mut out).unwrap();
    assert!(out[32] == 0.0 && out[0] == 1.0 && out[31] == 1.0);
}

// ----------------------------------------------------------- discovery

#[test]
fn discovery_parsing() {
    assert_eq!(endpoint_from_parts("192.0.2.10", 6400).unwrap().host, "192.0.2.10");
    assert!(endpoint_from_parts("2001:db8::1", 6400).is_some());
    assert!(endpoint_from_parts("192.0.2.10", 0).is_none());
    assert!(endpoint_from_parts("not a host", 6400).is_none());
    assert!(endpoint_from_parts("192.0.2.10/../x", 6400).is_none());
    for refused in ["fe80::e65f:1ff:fe2e:9f04", "febf::1", "::", "ff02::1", "0.0.0.0", "239.0.0.1"]
    {
        assert!(endpoint_from_parts(refused, 6400).is_none(), "{refused}");
    }
    assert!(endpoint_from_parts("169.254.10.20", 6400).is_some());
    assert!(endpoint_from_parts("source.example", 6400).is_some());
    assert!(endpoint_from_parts("fec0::1", 6400).is_some());

    let a = read_announcement(
        b"<OMTAddress><Name>Camera</Name><Port>6400</Port>\
          <Addresses><IPAddress>192.0.2.10</IPAddress></Addresses></OMTAddress>",
    )
    .unwrap();
    assert!(a.name == "Camera" && !a.removed && a.endpoint.as_ref().unwrap().port == 6400);
    let gone =
        read_announcement(b"<OMTAddress><Name>Camera</Name><Removed>true</Removed></OMTAddress>")
            .unwrap();
    assert!(gone.removed && gone.endpoint.is_none());
    for bad in [
        &b"<OMTAddress><Port>6400</Port></OMTAddress>"[..],
        "<OMTAddress><Name>bad\u{202e}name</Name></OMTAddress>".as_bytes(),
        b"<OMTAddress><Name>Camera</Name><Port>6400</Port><Port>1</Port></OMTAddress>",
    ] {
        assert!(read_announcement(bad).is_none());
    }
    let zero = read_announcement(
        b"<OMTAddress><Name>Camera</Name><Port>0</Port><IPAddress>192.0.2.10</IPAddress></OMTAddress>",
    )
    .unwrap();
    assert!(zero.endpoint.is_none());
}

// ---------------------------------------------------------------- dbus

#[test]
fn dbus_marshalling_round_trips() {
    let args = [Arg::I32(-1), Arg::I32(-1), Arg::Str("_omt._tcp"), Arg::Str(""), Arg::U32(0)];
    let b = build_call(
        7,
        0,
        Some("org.freedesktop.Avahi"),
        "/",
        Some("org.freedesktop.Avahi.Server"),
        "ServiceBrowserNew",
        &args,
    )
    .unwrap();
    assert_eq!(message_length(&b), Some(b.len()));
    let m = Message::parse(b.clone()).unwrap();
    assert_eq!((m.kind, m.serial), (METHOD_CALL, 7));
    assert_eq!(m.member(), Some("ServiceBrowserNew"));
    assert_eq!(m.interface(), Some("org.freedesktop.Avahi.Server"));
    assert_eq!(m.path(), Some("/"));
    assert_eq!(m.signature(), "iissu");
    let mut r = m.reader();
    assert_eq!((r.i32(), r.i32()), (-1, -1));
    assert_eq!(r.string(), "_omt._tcp");
    assert_eq!(r.string(), "");
    assert_eq!(r.u32(), 0);
    assert!(!r.failed && r.pos == r.end);
    let _ = r.u32();
    assert!(r.failed);

    // Every truncation and every single-byte corruption parses or is
    // refused without reading out of bounds.
    for len in 0..b.len() {
        assert!(Message::parse(b[..len].to_vec()).is_none());
    }
    for i in 0..b.len() {
        let mut copy = b.clone();
        copy[i] ^= 0x5A;
        if let Some(t) = Message::parse(copy) {
            let mut tr = t.reader();
            let _ = tr.i32();
            let _ = tr.string();
            tr.skip_aay();
        }
    }
}
