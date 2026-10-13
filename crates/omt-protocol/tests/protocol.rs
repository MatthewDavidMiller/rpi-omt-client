//! Port of the C protocol suite, plus the shared schema vectors the Web and
//! receiver suites also assert.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use omt_common::json::{self, Value};
use omt_protocol::*;

fn target_ok(s: &str) -> bool {
    parse_direct_target(s).is_ok()
}

#[test]
fn target_contract() {
    let t = parse_direct_target("omt://camera:6400").unwrap();
    assert_eq!((t.host.as_str(), t.port), ("camera", 6400));
    for bad in [
        "omt://[fe80::1%25eth0]:6400",
        "omt://[fe80::1]:6400",
        "omt://0.0.0.0:6400",
        "omt://255.255.255.255:1",
        "",
        "camera",
        "omt://camera:0",
        "omt://user@camera:1",
        "omt://host_name:1",
        "omt://[192.0.2.1]:1",
    ] {
        assert!(!target_ok(bad), "{bad}");
    }
    assert!(is_valid_source_name("Camera \u{1f600}"));
    assert!(!is_valid_source_name("Cafe\u{301}"));
    assert!(target_ok("omt://[2001:db8::1]:6400"));
    assert!(target_ok("omt://192.0.2.1:6400"));
}

#[test]
fn metadata_round_trip() {
    let mut b = Vec::new();
    build_metadata(b"<x/>", 42, &mut b).unwrap();
    let h = parse_frame_header(&b).unwrap();
    assert_eq!(h.timestamp, 42);
    assert_eq!(h.frame_type, FrameType::Metadata);
}

fn video_bytes(v: &VideoHeader) -> [u8; 32] {
    let mut out = [0_u8; 32];
    let fields = [v.codec, v.width, v.height, v.frame_rate_n, v.frame_rate_d];
    for (i, f) in fields.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&f.to_le_bytes());
    }
    out[20..24].copy_from_slice(&v.aspect_ratio.to_bits().to_le_bytes());
    out[24..28].copy_from_slice(&v.flags.to_le_bytes());
    out[28..32].copy_from_slice(&v.color_space.to_le_bytes());
    out
}

fn supported() -> VideoHeader {
    VideoHeader {
        codec: CODEC_VMX1,
        width: 1920,
        height: 1080,
        frame_rate_n: 60,
        frame_rate_d: 1,
        aspect_ratio: 16.0 / 9.0,
        flags: 0,
        color_space: 709,
    }
}

#[test]
fn video_headers_accept_only_the_appliance_format() {
    let h = FrameHeader {
        frame_type: FrameType::Video,
        timestamp: 0,
        metadata_length: 0,
        data_length: VIDEO_HEADER_SIZE,
    };
    let out = parse_video_header(&h, &video_bytes(&supported())).unwrap();
    assert!(out.width == 1920 && out.color_space == 709);
    let mut v = supported();
    v.flags = 1;
    assert!(parse_video_header(&h, &video_bytes(&v)).is_ok());
    let mut bad = [supported(); 8];
    bad[0].width = 1922;
    bad[1].height = 1082;
    bad[2].frame_rate_n = 120;
    bad[3].frame_rate_d = 0;
    bad[4].codec = CODEC_FPA1;
    bad[5].color_space = 2020;
    bad[6].flags = 32;
    bad[7].aspect_ratio = f32::NAN;
    for (i, b) in bad.iter().enumerate() {
        assert!(parse_video_header(&h, &video_bytes(b)).is_err(), "case {i}");
    }
    let audio = FrameHeader { frame_type: FrameType::Audio, ..h };
    let bytes = video_bytes(&supported());
    assert!(parse_video_header(&audio, &bytes).is_err());
    assert!(parse_video_header(&h, &bytes[..31]).is_err());
}

fn audio_bytes(codec: i32, rate: i32, samples: i32, channels: i32, active: u32) -> [u8; 24] {
    let mut out = [0_u8; 24];
    out[0..4].copy_from_slice(&codec.to_le_bytes());
    out[4..8].copy_from_slice(&rate.to_le_bytes());
    out[8..12].copy_from_slice(&samples.to_le_bytes());
    out[12..16].copy_from_slice(&channels.to_le_bytes());
    out[16..20].copy_from_slice(&active.to_le_bytes());
    out
}

fn audio_frame(metadata_length: u16, data_length: u32) -> FrameHeader {
    FrameHeader { frame_type: FrameType::Audio, timestamp: 0, metadata_length, data_length }
}

#[test]
fn audio_headers_bind_the_payload_to_the_active_channels() {
    let compressed = 480 * 2 * 4;
    let h = audio_frame(0, AUDIO_HEADER_SIZE + compressed);
    let a = parse_audio_header(&h, &audio_bytes(CODEC_FPA1, 48000, 480, 2, 3)).unwrap();
    assert_eq!(a.channels, 2);
    let one = audio_bytes(CODEC_FPA1, 48000, 480, 2, 1);
    assert!(parse_audio_header(&h, &one).is_err());
    assert!(parse_audio_header(&audio_frame(0, AUDIO_HEADER_SIZE + compressed / 2), &one).is_ok());
    let both = audio_bytes(CODEC_FPA1, 48000, 480, 2, 3);
    assert!(
        parse_audio_header(&audio_frame(16, AUDIO_HEADER_SIZE + compressed + 16), &both).is_ok()
    );
    assert!(parse_audio_header(&h, &audio_bytes(CODEC_FPA1, 48000, 480, 2, 7)).is_err());
    let full = audio_frame(0, AUDIO_HEADER_SIZE + 480 * 32 * 4);
    assert!(parse_audio_header(&full, &audio_bytes(CODEC_FPA1, 48000, 480, 32, u32::MAX)).is_ok());
    let bad = [
        (CODEC_VMX1, 48000, 480, 2, 3),
        (CODEC_FPA1, 4000, 480, 2, 3),
        (CODEC_FPA1, 200_000, 480, 2, 3),
        (CODEC_FPA1, 48000, 480, 33, 3),
        (CODEC_FPA1, 48000, 480, 0, 0),
        (CODEC_FPA1, 48000, 0, 2, 3),
        (CODEC_FPA1, 48000, i32::MAX, 2, 3),
    ];
    for (i, (c, r, s, ch, act)) in bad.into_iter().enumerate() {
        assert!(parse_audio_header(&h, &audio_bytes(c, r, s, ch, act)).is_err(), "case {i}");
    }
}

#[test]
fn frame_headers_bound_every_payload() {
    let mut b = Vec::new();
    build_metadata(b"<x/>", 0, &mut b).unwrap();
    b[0] = 2;
    let e = parse_frame_header(&b).unwrap_err();
    assert_eq!((e.kind, e.detail), (ProtoKind::Unsupported, "OMT frame version"));
    b[0] = 1;
    b[1] = 3;
    assert_eq!(parse_frame_header(&b).unwrap_err().detail, "OMT frame type");
    assert_eq!(parse_frame_header(&b[..HEADER_SIZE - 1]).unwrap_err().kind, ProtoKind::Truncated);

    let mut header = [0_u8; 16];
    header[0] = 1;
    header[1] = 2;
    header[12..16].copy_from_slice(&(VIDEO_HEADER_SIZE - 1).to_le_bytes());
    assert_eq!(parse_frame_header(&header).unwrap_err().kind, ProtoKind::Invalid);
    let ceiling = VIDEO_MAX_SIZE + VIDEO_HEADER_SIZE;
    header[12..16].copy_from_slice(&ceiling.to_le_bytes());
    assert!(parse_frame_header(&header).is_ok());
    header[12..16].copy_from_slice(&(ceiling + 1).to_le_bytes());
    let e = parse_frame_header(&header).unwrap_err();
    assert_eq!((e.kind, e.detail), (ProtoKind::Oversized, "OMT frame payload"));
    header[12..16].copy_from_slice(&(VIDEO_HEADER_SIZE + 8).to_le_bytes());
    header[10..12].copy_from_slice(&9_u16.to_le_bytes());
    assert_eq!(parse_frame_header(&header).unwrap_err().kind, ProtoKind::Invalid);
    let fe = ProtoError { kind: ProtoKind::Unsupported, detail: "OMT frame version" };
    assert_eq!(fe.to_string(), "unsupported OMT frame version");
}

fn vectors() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/schema/omt-target-vectors.json");
    let raw = omt_common::fsio::read_bounded(path, 1 << 20).unwrap().unwrap();
    json::parse(&raw).unwrap()
}

#[test]
fn shared_validation_vectors() {
    let root = vectors();
    for (i, item) in root.get("source_names").unwrap().as_array().unwrap().iter().enumerate() {
        let v = item.get("value").unwrap().as_raw_str().unwrap();
        let valid = item.get("valid").unwrap().as_bool().unwrap();
        assert_eq!(is_valid_source_name(v), valid, "source {i} {v:?}");
    }
    for (i, item) in root.get("direct_targets").unwrap().as_array().unwrap().iter().enumerate() {
        let v = item.get("value").unwrap().as_raw_str().unwrap();
        let valid = item.get("valid").unwrap().as_bool().unwrap();
        assert_eq!(parse_direct_target(v).is_ok(), valid, "target {i} {v:?}");
    }
    // The compiled table has to be the published one, range for range.
    let ranges = root
        .get("forbidden_name_codepoints")
        .and_then(|f| f.get("ranges"))
        .and_then(Value::as_array)
        .unwrap();
    assert_eq!(ranges.len(), FORBIDDEN_NAME_RANGES.len());
    for (i, r) in ranges.iter().enumerate() {
        let pair = r.as_array().unwrap();
        let hex = |v: &Value| {
            u32::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
        };
        assert_eq!((hex(&pair[0]), hex(&pair[1])), FORBIDDEN_NAME_RANGES[i]);
        if i > 0 {
            assert!(FORBIDDEN_NAME_RANGES[i - 1].1 < FORBIDDEN_NAME_RANGES[i].0);
        }
    }
    for &(low, high) in &FORBIDDEN_NAME_RANGES {
        for cp in [low, high] {
            assert!(is_forbidden_name_char(cp));
            let Some(c) = char::from_u32(cp) else { continue };
            assert!(!is_valid_source_name(&format!("a{c}b")), "U+{cp:04X}");
        }
    }
}
