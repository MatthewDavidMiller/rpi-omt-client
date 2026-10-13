//! Port of the receiver-core suite, including the shared status vectors.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

use omt_common::json::{self, Value};
use omt_receiver_core::*;

fn ceiling(text: &str) -> VideoCeiling {
    VideoCeiling::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"))
}

#[test]
fn status_on_tmpfs_skips_fsync() {
    assert!(!durable_status_parent(Path::new("/run/omt/state")));
    assert!(!durable_status_parent(Path::new("/run")));
    assert!(durable_status_parent(Path::new("/etc/omt/run")));
    assert!(durable_status_parent(Path::new("/tmp/omt")));
    assert!(durable_status_parent(Path::new("/running")));
}

#[test]
fn board_tiers_admit_their_intended_formats() {
    let (pi5, pi4, pi3) =
        (ceiling("1920x1080@60"), ceiling("1920x1080@30,1280x720@60"), ceiling("1280x720@60"));
    assert!(pi5.admits(1920, 1080, 60.0).is_ok());
    assert!(pi5.admits(1280, 720, 60.0).is_ok());
    assert!(pi4.admits(1920, 1080, 30.0).is_ok());
    assert!(pi4.admits(1280, 720, 60.0).is_ok());
    assert!(pi4.admits(1920, 1080, 60.0).is_err());
    assert!(pi3.admits(1280, 720, 60.0).is_ok());
    assert!(pi3.admits(1280, 720, 30.0).is_ok());
    assert!(pi3.admits(1920, 1080, 30.0).is_err());
    assert!(pi3.admits(1920, 1080, 60.0).is_err());
    let tier = ceiling("1280x720@60");
    assert!(tier.admits(1280, 720, 60000.0 / 1001.0).is_ok());
    assert!(tier.admits(1280, 720, 60.5).is_err());
    let wide = ceiling("1920x1080@30");
    assert!(wide.admits(640, 480, 25.0).is_ok());
    assert!(wide.admits(1920, 240, 30.0).is_ok());
    assert!(wide.admits(16, 16, 1.0).is_ok());
}

#[test]
fn a_refusal_names_the_stream_and_the_ceiling() {
    let c = ceiling("1280x720@60");
    assert_eq!(
        c.admits(1920, 1080, 59.94).unwrap_err().message(),
        "1920x1080 at 59.94 fps exceeds this appliance's limit of 1280x720 at 60 fps."
    );
    assert_eq!(
        ceiling("1920x1080@30,1280x720@60").describe(),
        "1920x1080 at 30 fps, or 1280x720 at 60 fps"
    );
}

#[test]
fn rejects_malformed_and_out_of_range_ceilings() {
    for bad in [
        "",
        "1920x1080",
        "1920X1080@60",
        "1920x1080@60Hz",
        "1921x1080@60",
        "1920x1081@60",
        "1920x1080@61",
        "3840x2160@30",
        "15x15@60",
        "1920x1080@0",
        "0x1080@60",
        "0640x480@30",
        "+640x480@30",
        " 640x480@30",
        "640x480@30 ",
        "1920x1080@60,",
        ",1920x1080@60",
        "1920x1080@60,,1280x720@30",
        "1920x1080@60 1280x720@30",
        "12345x1080@60",
        "640x480@25,800x600@30,1280x720@50,1920x1080@30,640x360@24",
    ] {
        assert!(VideoCeiling::parse(bad).is_err(), "accepted [{bad}]");
    }
    assert_eq!(
        VideoCeiling::parse("1920x1080@61").unwrap_err().message(),
        "Video ceiling 1920x1080@61 is outside the supported 1920x1080 at 60 fps maximum."
    );
    assert_eq!(
        VideoCeiling::parse("abc").unwrap_err().message(),
        "Invalid video ceiling: abc. Expected WIDTHxHEIGHT@FPS."
    );
    for good in [
        "1920x1080@60",
        "1920x1080@30,1280x720@60",
        "1280x720@60",
        "640x480@25,800x600@30,1280x720@50,1920x1080@30",
    ] {
        let _ = ceiling(good);
    }
}

#[test]
fn detail_contract() {
    assert_eq!(sanitize_detail("  a\nb  "), "ab");
    assert_eq!(sanitize_detail(&"x".repeat(4096)).len(), DETAIL_LIMIT);
    // Whole characters only: a multibyte character that would straddle the
    // limit is dropped rather than cut.
    let edge = format!("{}\u{e9}", "x".repeat(DETAIL_LIMIT - 1));
    assert_eq!(sanitize_detail(&edge).len(), DETAIL_LIMIT - 1);
}

#[test]
fn format_timestamp_contract() {
    use omt_common::timefmt::{civil_from_days, days_from_civil, rfc3339_millis};
    for (s, ms, expected) in [
        (0, 0, "1970-01-01T00:00:00.000Z"),
        (1, 7, "1970-01-01T00:00:01.007Z"),
        (86_399, 999, "1970-01-01T23:59:59.999Z"),
        (86_400, 0, "1970-01-02T00:00:00.000Z"),
        (951_782_400, 0, "2000-02-29T00:00:00.000Z"),
        (4_107_456_000, 0, "2100-02-28T00:00:00.000Z"),
        (4_107_456_000 + 86_400, 0, "2100-03-01T00:00:00.000Z"),
        (1_767_225_600, 250, "2026-01-01T00:00:00.250Z"),
        (2_147_483_648, 0, "2038-01-19T03:14:08.000Z"),
    ] {
        assert_eq!(rfc3339_millis(s, ms), expected);
        let (y, m, d) = civil_from_days(s / 86_400);
        assert_eq!(days_from_civil(y, m, d), s / 86_400);
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("omt-core-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn read_doc(path: &Path) -> Value {
    json::parse(&omt_common::fsio::read_bounded(path, 1 << 20).unwrap().unwrap()).unwrap()
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap()
}

#[test]
fn shared_status_vectors() {
    let vectors = read_doc(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/schema/playback-status-vectors.json"
    )));
    let fields: Vec<&str> = vectors
        .get("fields")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap())
        .collect();
    let root = scratch("vectors");
    for (i, v) in vectors.get("projections").and_then(Value::as_array).unwrap().iter().enumerate() {
        let path = root.join(format!("v{i}/status.json"));
        let status = PlaybackStatus::new(&path, "Camera");
        let none = Connector::none();
        for ev in v.get("events").and_then(Value::as_array).unwrap() {
            let _ = match ev.as_str().unwrap() {
                "AudioRunning" => status.audio(AudioState::Running, "", &none),
                "AudioFailed" => status.audio(AudioState::Failed, "Audio unavailable", &none),
                "AudioStopped" => status.audio(AudioState::Stopped, "", &none),
                "VideoStarting" => status.video(VideoState::Starting, "", &none),
                "WaitingForDiscovery" => status.video(VideoState::WaitingForDiscovery, "", &none),
                "WaitingForHdmi" => status.video(VideoState::WaitingForHdmi, "", &none),
                "VideoRetrying" => status.video(VideoState::Retrying, "", &none),
                "UnsupportedFormat" => status.video(VideoState::UnsupportedFormat, "", &none),
                "VideoRunning" => status.video(VideoState::Running, "", &none),
                "Stopped" => status.stopped(""),
                other => panic!("unknown event {other}"),
            };
        }
        if !path.exists() {
            status.stopped("Playback stopped.").unwrap();
        }
        let d = read_doc(&path);
        let name = text(v, "name");
        for key in ["state", "video_state", "audio_state"] {
            assert_eq!(text(&d, key), text(v, key), "{name}: {key}");
        }
        assert_eq!(d.as_object().unwrap().len(), fields.len(), "{name}");
        for f in &fields {
            assert!(d.get(f).is_some(), "{name} missing {f}");
        }
        assert_eq!(text(&d, "updated_at").len(), 24);
    }
    let _ = std::fs::remove_dir_all(&root);
    // The producer's state names must be exactly the consumer's accept-list.
    let states = vectors.get("video_states").and_then(Value::as_array).unwrap();
    assert_eq!(states.len(), VIDEO_STATE_COUNT);
    for s in VideoState::ALL {
        assert!(states.iter().any(|k| k.as_str() == Some(s.name())), "{}", s.name());
    }
}

#[test]
fn heartbeat_republishes_after_the_interval() {
    let root = scratch("hb");
    let path = root.join("status.json");
    let s = PlaybackStatus::new(&path, "Camera");
    let none = Connector::none();
    assert!(s.video(VideoState::Running, "Playing OMT video.", &none).unwrap());
    assert!(!s.video(VideoState::Running, "Playing OMT video.", &none).unwrap());
    assert!(!s.heartbeat(&none).unwrap());
    std::thread::sleep(std::time::Duration::from_millis(HEARTBEAT_MS + 20));
    assert!(s.heartbeat(&none).unwrap());
    assert_eq!(text(&read_doc(&path), "state"), "running");
    let _ = std::fs::remove_dir_all(&root);
}
