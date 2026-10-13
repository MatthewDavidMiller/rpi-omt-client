//! The OMT wire transport and the target grammar shared by the receiver, the
//! Web frontend, and the test sender. Pure parsing and validation, no I/O.
//!
//! The fixed header decides how many bytes the channel reads next, so every
//! length is bounded here before anything is allocated: 10 MiB of video,
//! 1 MiB of audio, 64 KiB of metadata.

#![forbid(unsafe_code)]

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

pub const HEADER_SIZE: usize = 16;
pub const VIDEO_HEADER_SIZE: u32 = 32;
pub const AUDIO_HEADER_SIZE: u32 = 24;
pub const VIDEO_MAX_SIZE: u32 = 10 * 1024 * 1024;
pub const AUDIO_MAX_SIZE: u32 = 1024 * 1024;
pub const METADATA_MAX_SIZE: u32 = 64 * 1024;
pub const SOURCE_NAME_MAX_BYTES: usize = 63;
pub const TARGET_MAX_BYTES: usize = 512;
pub const HOST_MAX_BYTES: usize = 256;

pub const CODEC_VMX1: i32 = 0x3158_4d56;
pub const CODEC_FPA1: i32 = 0x3141_5046;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameType {
    Metadata = 1,
    Video = 2,
    Audio = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameHeader {
    pub frame_type: FrameType,
    pub timestamp: i64,
    pub metadata_length: u16,
    pub data_length: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VideoHeader {
    pub codec: i32,
    pub width: i32,
    pub height: i32,
    pub frame_rate_n: i32,
    pub frame_rate_d: i32,
    pub aspect_ratio: f32,
    pub flags: u32,
    pub color_space: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioHeader {
    pub codec: i32,
    pub sample_rate: i32,
    pub samples_per_channel: i32,
    pub channels: i32,
    pub active_channels: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectTarget {
    pub host: String,
    pub port: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtoKind {
    Truncated,
    Unsupported,
    Oversized,
    Invalid,
}

/// A protocol error: its kind and a static detail, displayed as e.g.
/// "unsupported OMT frame version".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProtoError {
    pub kind: ProtoKind,
    pub detail: &'static str,
}

impl fmt::Display for ProtoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let prefix = match self.kind {
            ProtoKind::Truncated => "truncated",
            ProtoKind::Unsupported => "unsupported",
            ProtoKind::Oversized => "oversized",
            ProtoKind::Invalid => "invalid",
        };
        write!(f, "{prefix} {}", self.detail)
    }
}

impl std::error::Error for ProtoError {}

const fn fail<T>(kind: ProtoKind, detail: &'static str) -> Result<T, ProtoError> {
    Err(ProtoError { kind, detail })
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn le_i32(b: &[u8], at: usize) -> i32 {
    i32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

pub fn parse_frame_header(data: &[u8]) -> Result<FrameHeader, ProtoError> {
    if data.len() < HEADER_SIZE {
        return fail(ProtoKind::Truncated, "OMT frame header");
    }
    if data[0] != 1 {
        return fail(ProtoKind::Unsupported, "OMT frame version");
    }
    let (frame_type, extension, maximum) = match data[1] {
        1 => (FrameType::Metadata, 0, METADATA_MAX_SIZE),
        2 => (FrameType::Video, VIDEO_HEADER_SIZE, VIDEO_MAX_SIZE),
        4 => (FrameType::Audio, AUDIO_HEADER_SIZE, AUDIO_MAX_SIZE),
        _ => return fail(ProtoKind::Unsupported, "OMT frame type"),
    };
    let timestamp = i64::from_le_bytes([
        data[2], data[3], data[4], data[5], data[6], data[7], data[8], data[9],
    ]);
    let metadata_length = le16(data, 10);
    let data_length = le32(data, 12);
    let Some(payload) = data_length.checked_sub(extension) else {
        return fail(ProtoKind::Invalid, "OMT frame is shorter than its extended header");
    };
    if payload > maximum {
        return fail(ProtoKind::Oversized, "OMT frame payload");
    }
    if u32::from(metadata_length) > payload {
        return fail(ProtoKind::Invalid, "OMT metadata length exceeds the payload");
    }
    Ok(FrameHeader { frame_type, timestamp, metadata_length, data_length })
}

pub fn parse_video_header(frame: &FrameHeader, data: &[u8]) -> Result<VideoHeader, ProtoError> {
    if frame.frame_type != FrameType::Video || data.len() < VIDEO_HEADER_SIZE as usize {
        return fail(ProtoKind::Truncated, "OMT video header");
    }
    let v = VideoHeader {
        codec: le_i32(data, 0),
        width: le_i32(data, 4),
        height: le_i32(data, 8),
        frame_rate_n: le_i32(data, 12),
        frame_rate_d: le_i32(data, 16),
        aspect_ratio: f32::from_bits(le32(data, 20)),
        flags: le32(data, 24),
        color_space: le_i32(data, 28),
    };
    if !(16..=1920).contains(&v.width)
        || !(16..=1080).contains(&v.height)
        || v.frame_rate_n <= 0
        || v.frame_rate_d <= 0
    {
        return fail(ProtoKind::Unsupported, "OMT video dimensions or frame rate");
    }
    let rate = f64::from(v.frame_rate_n) / f64::from(v.frame_rate_d);
    if !(0.0..=60.0).contains(&rate) || v.codec != CODEC_VMX1 {
        return fail(ProtoKind::Unsupported, "OMT video format");
    }
    if !v.aspect_ratio.is_finite()
        || !(0.0..=10.0).contains(&v.aspect_ratio)
        || !matches!(v.color_space, 0 | 601 | 709)
        || v.flags & !31 != 0
    {
        return fail(ProtoKind::Unsupported, "OMT video properties");
    }
    Ok(v)
}

pub fn parse_audio_header(frame: &FrameHeader, data: &[u8]) -> Result<AudioHeader, ProtoError> {
    if frame.frame_type != FrameType::Audio || data.len() < AUDIO_HEADER_SIZE as usize {
        return fail(ProtoKind::Truncated, "OMT audio header");
    }
    let a = AudioHeader {
        codec: le_i32(data, 0),
        sample_rate: le_i32(data, 4),
        samples_per_channel: le_i32(data, 8),
        channels: le_i32(data, 12),
        active_channels: le32(data, 16),
    };
    if a.codec != CODEC_FPA1
        || !(8000..=192_000).contains(&a.sample_rate)
        || !(1..=32).contains(&a.channels)
        || a.samples_per_channel < 1
    {
        return fail(ProtoKind::Unsupported, "OMT audio format");
    }
    let channels = u64::from(a.channels.unsigned_abs());
    let samples = u64::from(a.samples_per_channel.unsigned_abs());
    // Both factors are bounded (32 and 2^31), so the products cannot wrap.
    if samples * channels * 4 > u64::from(AUDIO_MAX_SIZE) {
        return fail(ProtoKind::Oversized, "decoded audio");
    }
    let allowed = if channels == 32 { u32::MAX } else { (1_u32 << channels) - 1 };
    if a.active_channels & !allowed != 0 {
        return fail(ProtoKind::Invalid, "active audio channel");
    }
    let compressed = u64::from(a.active_channels.count_ones()) * samples * 4;
    let Some(payload) = frame.data_length.checked_sub(AUDIO_HEADER_SIZE) else {
        return fail(ProtoKind::Invalid, "audio payload");
    };
    if compressed + u64::from(frame.metadata_length) != u64::from(payload) {
        return fail(ProtoKind::Invalid, "OMT planar audio payload length");
    }
    Ok(a)
}

/// Appends a metadata frame carrying `xml` to `out`.
pub fn build_metadata(xml: &[u8], timestamp: i64, out: &mut Vec<u8>) -> Result<(), ProtoError> {
    let Some(len) = u32::try_from(xml.len()).ok().filter(|&n| n > 0 && n < METADATA_MAX_SIZE)
    else {
        return fail(ProtoKind::Invalid, "metadata size");
    };
    out.push(1);
    out.push(FrameType::Metadata as u8);
    out.extend_from_slice(&timestamp.to_le_bytes());
    out.extend_from_slice(&0_u16.to_le_bytes());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(xml);
    Ok(())
}

fn valid_dns(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 {
        return false;
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
    })
}

/// True for a literal no connect can use: unspecified, multicast, broadcast.
pub fn is_undiallable_ipv4(a: Ipv4Addr) -> bool {
    a.is_unspecified() || a.is_multicast() || a.is_broadcast()
}

/// True for an IPv6 literal no connect can use: unspecified, multicast, and
/// link-local, which needs a zone the grammar cannot carry.
pub fn is_undiallable_ipv6(a: Ipv6Addr) -> bool {
    a.is_unspecified() || a.is_multicast() || (a.segments()[0] & 0xFFC0) == 0xFE80
}

pub fn parse_direct_target(value: &str) -> Result<DirectTarget, ProtoError> {
    const TARGET: ProtoError = ProtoError { kind: ProtoKind::Invalid, detail: "OMT direct target" };
    const IPV6: ProtoError = ProtoError { kind: ProtoKind::Invalid, detail: "IPv6 target" };
    const PORT: ProtoError = ProtoError { kind: ProtoKind::Invalid, detail: "OMT target port" };
    const HOST: ProtoError = ProtoError { kind: ProtoKind::Invalid, detail: "OMT target host" };
    if value.len() > TARGET_MAX_BYTES {
        return Err(TARGET);
    }
    let auth = value.strip_prefix("omt://").ok_or(TARGET)?;
    if auth.bytes().any(|b| matches!(b, b'/' | b'?' | b'#' | b'@') || b <= 31 || b == 127) {
        return Err(TARGET);
    }
    let (host, port_text, ipv6) = if let Some(rest) = auth.strip_prefix('[') {
        let close = rest.find("]:").ok_or(IPV6)?;
        let (host, port) = (&rest[..close], &rest[close + 2..]);
        if port.contains(':') {
            return Err(IPV6);
        }
        (host, port, true)
    } else {
        let (host, port) = auth.rsplit_once(':').ok_or(PORT)?;
        if host.contains(':') {
            return Err(ProtoError { kind: ProtoKind::Invalid, detail: "IPv6 brackets" });
        }
        (host, port, false)
    };
    let port = omt_common::buf::parse_u64(port_text.as_bytes(), 65535)
        .and_then(|p| u16::try_from(p).ok())
        .filter(|&p| p != 0)
        .ok_or(PORT)?;
    if ipv6 {
        match host.parse::<Ipv6Addr>() {
            Ok(a) if !host.contains('%') && !is_undiallable_ipv6(a) => {}
            _ => return Err(IPV6),
        }
    } else {
        let v4 = host.parse::<Ipv4Addr>().ok();
        if host.is_empty() || host.len() >= HOST_MAX_BYTES || (v4.is_none() && !valid_dns(host)) {
            return Err(HOST);
        }
        if v4.is_some_and(is_undiallable_ipv4) {
            return Err(HOST);
        }
    }
    if host.len() >= HOST_MAX_BYTES {
        return Err(HOST);
    }
    Ok(DirectTarget { host: host.to_owned(), port })
}

/// The forbidden source-name code points (Cc, Cf, Cs, Zl, Zp), as sorted,
/// inclusive ranges. Surrogates are listed too even though valid UTF-8
/// cannot carry them, so this table equals the published one exactly.
pub const FORBIDDEN_NAME_RANGES: [(u32, u32); 24] = [
    (0x0, 0x1f),
    (0x7f, 0x9f),
    (0xad, 0xad),
    (0x600, 0x605),
    (0x61c, 0x61c),
    (0x6dd, 0x6dd),
    (0x70f, 0x70f),
    (0x890, 0x891),
    (0x8e2, 0x8e2),
    (0x180e, 0x180e),
    (0x200b, 0x200f),
    (0x2028, 0x202e),
    (0x2060, 0x2064),
    (0x2066, 0x206f),
    (0xd800, 0xdfff),
    (0xfeff, 0xfeff),
    (0xfff9, 0xfffb),
    (0x110bd, 0x110bd),
    (0x110cd, 0x110cd),
    (0x13430, 0x1343f),
    (0x1bca0, 0x1bca3),
    (0x1d173, 0x1d17a),
    (0xe0001, 0xe0001),
    (0xe0020, 0xe007f),
];

pub fn is_forbidden_name_char(cp: u32) -> bool {
    FORBIDDEN_NAME_RANGES
        .binary_search_by(|&(low, high)| {
            if high < cp {
                std::cmp::Ordering::Less
            } else if low > cp {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

pub fn is_valid_source_name(value: &str) -> bool {
    if value.is_empty() || value.len() > SOURCE_NAME_MAX_BYTES {
        return false;
    }
    if !omt_common::nfc::is_normalized(value, SOURCE_NAME_MAX_BYTES) {
        return false;
    }
    if value.chars().any(|c| is_forbidden_name_char(u32::from(c))) {
        return false;
    }
    let first = value.chars().next().is_some_and(char::is_whitespace);
    let last = value.chars().next_back().is_some_and(char::is_whitespace);
    !first && !last
}

/// A byte string that is a valid source name.
pub fn is_valid_source_name_bytes(value: &[u8]) -> bool {
    std::str::from_utf8(value).is_ok_and(is_valid_source_name)
}

pub fn is_valid_target(value: &str) -> bool {
    if value.starts_with("omt://") {
        parse_direct_target(value).is_ok()
    } else {
        is_valid_source_name(value)
    }
}
