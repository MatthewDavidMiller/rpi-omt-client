#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, Ipv6Addr};
use unicode_normalization::UnicodeNormalization;

pub const HEADER_SIZE: usize = 16;
pub const VIDEO_HEADER_SIZE: usize = 32;
pub const AUDIO_HEADER_SIZE: usize = 24;
pub const VIDEO_MAX_SIZE: usize = 10 * 1024 * 1024;
pub const AUDIO_MAX_SIZE: usize = 1024 * 1024;
pub const METADATA_MAX_SIZE: usize = 64 * 1024;
pub const SOURCE_NAME_MAX_BYTES: usize = 63;
pub const TARGET_MAX_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[repr(u8)]
pub enum FrameType {
    Metadata = 1,
    Video = 2,
    Audio = 4,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum Codec {
    Vmx1 = 0x3158_4d56,
    Fpa1 = 0x3141_5046,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameHeader {
    pub version: u8,
    pub frame_type: FrameType,
    pub timestamp: i64,
    pub metadata_length: u16,
    pub data_length: u32,
}

#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioHeader {
    pub codec: i32,
    pub sample_rate: i32,
    pub samples_per_channel: i32,
    pub channels: i32,
    pub active_channels: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectTarget {
    pub host: String,
    pub port: u16,
    pub ipv6_literal: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    Truncated(&'static str),
    Unsupported(&'static str),
    Oversized(&'static str),
    Invalid(&'static str),
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (prefix, detail) = match self {
            Self::Truncated(v) => ("truncated", *v),
            Self::Unsupported(v) => ("unsupported", *v),
            Self::Oversized(v) => ("oversized", *v),
            Self::Invalid(v) => ("invalid", *v),
        };
        write!(f, "{prefix} {detail}")
    }
}
impl std::error::Error for ProtocolError {}

fn array<const N: usize>(value: &[u8], offset: usize) -> Result<[u8; N], ProtocolError> {
    value
        .get(
            offset
                ..offset
                    .checked_add(N)
                    .ok_or(ProtocolError::Oversized("offset"))?,
        )
        .ok_or(ProtocolError::Truncated("field"))?
        .try_into()
        .map_err(|_| ProtocolError::Truncated("field"))
}

pub fn parse_frame_header(data: &[u8]) -> Result<FrameHeader, ProtocolError> {
    if data.len() < HEADER_SIZE {
        return Err(ProtocolError::Truncated("OMT frame header"));
    }
    if data[0] != 1 {
        return Err(ProtocolError::Unsupported("OMT frame version"));
    }
    let frame_type = match data[1] {
        1 => FrameType::Metadata,
        2 => FrameType::Video,
        4 => FrameType::Audio,
        _ => return Err(ProtocolError::Unsupported("OMT frame type")),
    };
    let timestamp = i64::from_le_bytes(array(data, 2)?);
    let metadata_length = u16::from_le_bytes(array(data, 10)?);
    let data_length = u32::from_le_bytes(array(data, 12)?);
    let extension = match frame_type {
        FrameType::Video => VIDEO_HEADER_SIZE,
        FrameType::Audio => AUDIO_HEADER_SIZE,
        FrameType::Metadata => 0,
    };
    let payload = usize::try_from(data_length)
        .map_err(|_| ProtocolError::Oversized("OMT frame"))?
        .checked_sub(extension)
        .ok_or(ProtocolError::Invalid(
            "OMT frame is shorter than its extended header",
        ))?;
    let maximum = match frame_type {
        FrameType::Video => VIDEO_MAX_SIZE,
        FrameType::Audio => AUDIO_MAX_SIZE,
        FrameType::Metadata => METADATA_MAX_SIZE,
    };
    if payload > maximum {
        return Err(ProtocolError::Oversized("OMT frame payload"));
    }
    if usize::from(metadata_length) > payload {
        return Err(ProtocolError::Invalid(
            "OMT metadata length exceeds the payload",
        ));
    }
    Ok(FrameHeader {
        version: 1,
        frame_type,
        timestamp,
        metadata_length,
        data_length,
    })
}

pub fn parse_video_header(frame: &FrameHeader, data: &[u8]) -> Result<VideoHeader, ProtocolError> {
    if frame.frame_type != FrameType::Video || data.len() < VIDEO_HEADER_SIZE {
        return Err(ProtocolError::Truncated("OMT video header"));
    }
    let video = VideoHeader {
        codec: i32::from_le_bytes(array(data, 0)?),
        width: i32::from_le_bytes(array(data, 4)?),
        height: i32::from_le_bytes(array(data, 8)?),
        frame_rate_n: i32::from_le_bytes(array(data, 12)?),
        frame_rate_d: i32::from_le_bytes(array(data, 16)?),
        aspect_ratio: f32::from_le_bytes(array(data, 20)?),
        flags: u32::from_le_bytes(array(data, 24)?),
        color_space: i32::from_le_bytes(array(data, 28)?),
    };
    if !(16..=1920).contains(&video.width)
        || !(16..=1080).contains(&video.height)
        || video.frame_rate_n <= 0
        || video.frame_rate_d <= 0
    {
        return Err(ProtocolError::Unsupported(
            "OMT video dimensions or frame rate",
        ));
    }
    let rate = f64::from(video.frame_rate_n) / f64::from(video.frame_rate_d);
    if !(0.0..=60.0).contains(&rate) || video.codec != Codec::Vmx1 as i32 {
        return Err(ProtocolError::Unsupported("OMT video format"));
    }
    if !video.aspect_ratio.is_finite()
        || !(0.0..=10.0).contains(&video.aspect_ratio)
        || !matches!(video.color_space, 0 | 601 | 709)
        || video.flags & !31 != 0
    {
        return Err(ProtocolError::Unsupported("OMT video properties"));
    }
    Ok(video)
}

pub fn parse_audio_header(frame: &FrameHeader, data: &[u8]) -> Result<AudioHeader, ProtocolError> {
    if frame.frame_type != FrameType::Audio || data.len() < AUDIO_HEADER_SIZE {
        return Err(ProtocolError::Truncated("OMT audio header"));
    }
    let audio = AudioHeader {
        codec: i32::from_le_bytes(array(data, 0)?),
        sample_rate: i32::from_le_bytes(array(data, 4)?),
        samples_per_channel: i32::from_le_bytes(array(data, 8)?),
        channels: i32::from_le_bytes(array(data, 12)?),
        active_channels: u32::from_le_bytes(array(data, 16)?),
    };
    if audio.codec != Codec::Fpa1 as i32
        || !(8000..=192_000).contains(&audio.sample_rate)
        || !(1..=32).contains(&audio.channels)
        || audio.samples_per_channel < 1
    {
        return Err(ProtocolError::Unsupported("OMT audio format"));
    }
    let channels =
        usize::try_from(audio.channels).map_err(|_| ProtocolError::Invalid("audio channels"))?;
    let samples = usize::try_from(audio.samples_per_channel)
        .map_err(|_| ProtocolError::Invalid("audio samples"))?;
    let decoded = samples
        .checked_mul(channels)
        .and_then(|v| v.checked_mul(4))
        .ok_or(ProtocolError::Oversized("decoded audio"))?;
    if decoded > AUDIO_MAX_SIZE {
        return Err(ProtocolError::Oversized("decoded audio"));
    }
    let allowed = if channels == 32 {
        u32::MAX
    } else {
        (1_u32 << channels) - 1
    };
    if audio.active_channels & !allowed != 0 {
        return Err(ProtocolError::Invalid("active audio channel"));
    }
    let compressed = usize::try_from(audio.active_channels.count_ones())
        .unwrap_or(usize::MAX)
        .checked_mul(samples)
        .and_then(|v| v.checked_mul(4))
        .ok_or(ProtocolError::Oversized("audio payload"))?;
    let payload = usize::try_from(frame.data_length)
        .map_err(|_| ProtocolError::Oversized("audio payload"))?
        .checked_sub(AUDIO_HEADER_SIZE)
        .ok_or(ProtocolError::Invalid("audio payload"))?;
    if compressed.checked_add(usize::from(frame.metadata_length)) != Some(payload) {
        return Err(ProtocolError::Invalid("OMT planar audio payload length"));
    }
    Ok(audio)
}

pub fn build_metadata(xml: &str, timestamp: i64) -> Result<Vec<u8>, ProtocolError> {
    if xml.is_empty() || xml.len() >= METADATA_MAX_SIZE {
        return Err(ProtocolError::Invalid("metadata size"));
    }
    let total = HEADER_SIZE
        .checked_add(xml.len())
        .ok_or(ProtocolError::Oversized("metadata"))?;
    let mut out = Vec::new();
    out.try_reserve_exact(total)
        .map_err(|_| ProtocolError::Oversized("metadata allocation"))?;
    out.extend([1, FrameType::Metadata as u8]);
    out.extend(timestamp.to_le_bytes());
    out.extend(0_u16.to_le_bytes());
    out.extend(
        u32::try_from(xml.len())
            .map_err(|_| ProtocolError::Oversized("metadata"))?
            .to_le_bytes(),
    );
    out.extend(xml.as_bytes());
    Ok(out)
}

fn valid_dns(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && !host.starts_with('.')
        && !host.ends_with('.')
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

pub fn parse_direct_target(value: &str) -> Result<DirectTarget, ProtocolError> {
    if value.len() > TARGET_MAX_BYTES {
        return Err(ProtocolError::Invalid("OMT direct target"));
    }
    let authority = value
        .strip_prefix("omt://")
        .ok_or(ProtocolError::Invalid("OMT direct target"))?;
    if authority
        .bytes()
        .any(|b| matches!(b, b'/' | b'?' | b'#' | b'@' | 0..=31 | 127))
    {
        return Err(ProtocolError::Invalid("OMT direct target"));
    }
    let (host, port_text, ipv6_literal) = if let Some(rest) = authority.strip_prefix('[') {
        let (host, port) = rest
            .split_once("]:")
            .ok_or(ProtocolError::Invalid("IPv6 target"))?;
        if port.contains(':') {
            return Err(ProtocolError::Invalid("IPv6 target"));
        }
        (host, port, true)
    } else {
        let (host, port) = authority
            .rsplit_once(':')
            .ok_or(ProtocolError::Invalid("OMT target port"))?;
        if host.contains(':') {
            return Err(ProtocolError::Invalid("IPv6 brackets"));
        }
        (host, port, false)
    };
    if port_text.is_empty() || !port_text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ProtocolError::Invalid("OMT target port"));
    }
    let port: u16 = port_text
        .parse()
        .map_err(|_| ProtocolError::Invalid("OMT target port"))?;
    if port == 0 {
        return Err(ProtocolError::Invalid("OMT target port"));
    }
    let normalized_host = if ipv6_literal {
        let (address, zone) = match host.split_once("%25") {
            Some((a, z)) => (a, Some(z)),
            None => (host, None),
        };
        if address.parse::<Ipv6Addr>().is_err()
            || zone.is_some_and(|z| {
                z.is_empty()
                    || !z
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
            })
            || (zone.is_none() && host.contains('%'))
        {
            return Err(ProtocolError::Invalid("IPv6 target"));
        }
        zone.map_or_else(|| host.to_owned(), |z| format!("{address}%{z}"))
    } else {
        if host.is_empty()
            || host.len() >= 256
            || (host.parse::<Ipv4Addr>().is_err() && !valid_dns(host))
        {
            return Err(ProtocolError::Invalid("OMT target host"));
        }
        host.to_owned()
    };
    Ok(DirectTarget {
        host: normalized_host,
        port,
        ipv6_literal,
    })
}

pub fn is_valid_source_name(value: &str) -> bool {
    !value.is_empty() && value.len() <= SOURCE_NAME_MAX_BYTES && value.nfc().eq(value.chars())
        && value.chars().all(|c| !c.is_control() && !matches!(c, '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}'))
        && value.chars().next().is_some_and(|c| !c.is_whitespace())
        && value.chars().next_back().is_some_and(|c| !c.is_whitespace())
}

pub fn is_valid_target(value: &str) -> bool {
    if value.starts_with("omt://") {
        parse_direct_target(value).is_ok()
    } else {
        is_valid_source_name(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    #[test]
    fn target_contract() {
        assert_eq!(
            parse_direct_target("omt://camera:6400").map(|v| v.port),
            Ok(6400)
        );
        assert!(parse_direct_target("omt://[fe80::1%25eth0]:6400").is_ok());
        for invalid in [
            "",
            "camera",
            "omt://camera:0",
            "omt://user@camera:1",
            "omt://host_name:1",
            "omt://[192.0.2.1]:1",
        ] {
            assert!(parse_direct_target(invalid).is_err(), "{invalid}");
        }
        assert!(is_valid_source_name("Camera 😀"));
        assert!(!is_valid_source_name("Cafe\u{301}"));
    }
    #[test]
    fn metadata_round_trip() {
        let bytes = build_metadata("<x/>", 42).unwrap_or_default();
        let h = parse_frame_header(&bytes).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(h.timestamp, 42);
    }
    #[derive(Deserialize)]
    struct Vector {
        value: String,
        valid: bool,
    }
    #[derive(Deserialize)]
    struct Vectors {
        source_names: Vec<Vector>,
        direct_targets: Vec<Vector>,
    }
    #[test]
    fn shared_validation_vectors() {
        let vectors: Vectors = serde_json::from_str(include_str!(
            "../../../tests/schema/omt-target-vectors.json"
        ))
        .unwrap_or_else(|e| panic!("{e}"));
        for vector in vectors.source_names {
            assert_eq!(
                is_valid_source_name(&vector.value),
                vector.valid,
                "source {:?}",
                vector.value
            );
        }
        for vector in vectors.direct_targets {
            assert_eq!(
                parse_direct_target(&vector.value).is_ok(),
                vector.valid,
                "target {:?}",
                vector.value
            );
        }
    }
}
