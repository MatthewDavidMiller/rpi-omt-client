#![forbid(unsafe_code)]

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use unicode_normalization::UnicodeNormalization;

pub const HEADER_SIZE: usize = 16;
pub const VIDEO_HEADER_SIZE: usize = 32;
pub const AUDIO_HEADER_SIZE: usize = 24;
pub const VIDEO_MAX_SIZE: usize = 10 * 1024 * 1024;
pub const AUDIO_MAX_SIZE: usize = 1024 * 1024;
pub const METADATA_MAX_SIZE: usize = 64 * 1024;
pub const SOURCE_NAME_MAX_BYTES: usize = 63;
pub const TARGET_MAX_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
        // A zone index cannot be carried through to connect: `Endpoint::resolve`
        // has nowhere to put it, and an unscoped link-local address is the
        // record a dual-stack resolver emits beside a usable one. Refuse both
        // here so the dashboard cannot save a target that only fails later.
        if host.contains('%') {
            return Err(ProtocolError::Invalid("IPv6 target"));
        }
        let address: Ipv6Addr = host
            .parse()
            .map_err(|_| ProtocolError::Invalid("IPv6 target"))?;
        if is_undiallable_ip(IpAddr::V6(address)) {
            return Err(ProtocolError::Invalid("IPv6 target"));
        }
        host.to_owned()
    } else {
        if host.is_empty()
            || host.len() >= 256
            || (host.parse::<Ipv4Addr>().is_err() && !valid_dns(host))
        {
            return Err(ProtocolError::Invalid("OMT target host"));
        }
        if let Ok(v4) = host.parse::<Ipv4Addr>()
            && is_undiallable_ip(IpAddr::V4(v4))
        {
            return Err(ProtocolError::Invalid("OMT target host"));
        }
        host.to_owned()
    };
    Ok(DirectTarget {
        host: normalized_host,
        port,
    })
}

/// Every code point a source name may not contain, as sorted, non-overlapping
/// inclusive ranges.
///
/// These are the Unicode general categories `Cc`, `Cf`, `Cs`, `Zl`, and `Zp`:
/// control, format, surrogate, line separator, and paragraph separator. The Web
/// layer takes the same authority from `unicodedata`, so the two sides have to
/// agree on the whole set rather than on the handful of code points either
/// happened to think of. The published form of this table, and the check that
/// it still matches the Unicode data, live in
/// `tests/schema/omt-target-vectors.json`; `shared_validation_vectors` below
/// asserts this copy against it.
/// The surrogate range the published table carries and this one cannot.
pub const SURROGATE_RANGE: (u32, u32) = (0xD800, 0xDFFF);

const FORBIDDEN_NAME_RANGES: [(char, char); 23] = [
    ('\u{0}', '\u{1f}'),
    ('\u{7f}', '\u{9f}'),
    ('\u{ad}', '\u{ad}'),
    ('\u{600}', '\u{605}'),
    ('\u{61c}', '\u{61c}'),
    ('\u{6dd}', '\u{6dd}'),
    ('\u{70f}', '\u{70f}'),
    ('\u{890}', '\u{891}'),
    ('\u{8e2}', '\u{8e2}'),
    ('\u{180e}', '\u{180e}'),
    ('\u{200b}', '\u{200f}'),
    ('\u{2028}', '\u{202e}'),
    ('\u{2060}', '\u{2064}'),
    ('\u{2066}', '\u{206f}'),
    // U+D800-U+DFFF, the surrogates, cannot be a `char` or valid UTF-8, so the
    // published table lists them and this one has nothing to match them with.
    ('\u{feff}', '\u{feff}'),
    ('\u{fff9}', '\u{fffb}'),
    ('\u{110bd}', '\u{110bd}'),
    ('\u{110cd}', '\u{110cd}'),
    ('\u{13430}', '\u{1343f}'),
    ('\u{1bca0}', '\u{1bca3}'),
    ('\u{1d173}', '\u{1d17a}'),
    ('\u{e0001}', '\u{e0001}'),
    ('\u{e0020}', '\u{e007f}'),
];

fn is_forbidden_name_char(value: char) -> bool {
    FORBIDDEN_NAME_RANGES
        .binary_search_by(|(low, high)| {
            if value < *low {
                std::cmp::Ordering::Greater
            } else if value > *high {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

pub fn is_valid_source_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= SOURCE_NAME_MAX_BYTES
        && value.nfc().eq(value.chars())
        && !value.chars().any(is_forbidden_name_char)
        && value.chars().next().is_some_and(|c| !c.is_whitespace())
        && value
            .chars()
            .next_back()
            .is_some_and(|c| !c.is_whitespace())
}

/// True for a literal address no connect can use as it stands.
///
/// IPv4 link-local is deliberately absent: a `169.254.0.0/16` peer is reached
/// over the one link it is on with no extra addressing. IPv6 link-local is
/// included, because it repeats per interface and needs a zone index the
/// target grammar does not carry through to connect.
#[must_use]
pub fn is_undiallable_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(value) => value.is_unspecified() || value.is_multicast() || value.is_broadcast(),
        IpAddr::V6(value) => {
            value.is_unspecified()
                || value.is_multicast()
                // `Ipv6Addr::is_unicast_link_local` is still unstable.
                || (value.segments()[0] & 0xFFC0) == 0xFE80
        }
    }
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
        assert!(parse_direct_target("omt://[fe80::1%25eth0]:6400").is_err());
        assert!(parse_direct_target("omt://[fe80::1]:6400").is_err());
        assert!(parse_direct_target("omt://0.0.0.0:6400").is_err());
        assert!(parse_direct_target("omt://255.255.255.255:1").is_err());
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

    /// Widths on the wire are `u32`; the sizes they are built from are not.
    fn wire(value: usize) -> u32 {
        u32::try_from(value).unwrap_or_else(|error| panic!("{value}: {error}"))
    }

    fn frame(frame_type: FrameType, data_length: u32, metadata_length: u16) -> FrameHeader {
        FrameHeader {
            frame_type,
            timestamp: 0,
            metadata_length,
            data_length,
        }
    }

    fn video_bytes(header: &VideoHeader) -> [u8; VIDEO_HEADER_SIZE] {
        let mut bytes = [0_u8; VIDEO_HEADER_SIZE];
        for (offset, field) in [
            header.codec.to_le_bytes(),
            header.width.to_le_bytes(),
            header.height.to_le_bytes(),
            header.frame_rate_n.to_le_bytes(),
            header.frame_rate_d.to_le_bytes(),
            header.aspect_ratio.to_le_bytes(),
            header.flags.to_le_bytes(),
            header.color_space.to_le_bytes(),
        ]
        .into_iter()
        .enumerate()
        {
            bytes[offset * 4..offset * 4 + 4].copy_from_slice(&field);
        }
        bytes
    }

    fn supported_video() -> VideoHeader {
        VideoHeader {
            codec: Codec::Vmx1 as i32,
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
        let header = frame(FrameType::Video, wire(VIDEO_HEADER_SIZE), 0);
        assert_eq!(
            parse_video_header(&header, &video_bytes(&supported_video())),
            Ok(supported_video())
        );
        // Interlaced input is presented progressively, so its flag is accepted.
        let interlaced = VideoHeader {
            flags: 1,
            ..supported_video()
        };
        assert!(parse_video_header(&header, &video_bytes(&interlaced)).is_ok());

        for (label, video) in [
            (
                "over the width ceiling",
                VideoHeader {
                    width: 1922,
                    ..supported_video()
                },
            ),
            (
                "over the height ceiling",
                VideoHeader {
                    height: 1082,
                    ..supported_video()
                },
            ),
            (
                "over 60 fps",
                VideoHeader {
                    frame_rate_n: 120,
                    ..supported_video()
                },
            ),
            (
                "a zero frame-rate denominator",
                VideoHeader {
                    frame_rate_d: 0,
                    ..supported_video()
                },
            ),
            (
                "a codec that is not VMX1",
                VideoHeader {
                    codec: Codec::Fpa1 as i32,
                    ..supported_video()
                },
            ),
            (
                "an unknown colour space",
                VideoHeader {
                    color_space: 2020,
                    ..supported_video()
                },
            ),
            (
                "a reserved flag",
                VideoHeader {
                    flags: 32,
                    ..supported_video()
                },
            ),
            (
                "a non-finite aspect ratio",
                VideoHeader {
                    aspect_ratio: f32::NAN,
                    ..supported_video()
                },
            ),
        ] {
            assert!(
                parse_video_header(&header, &video_bytes(&video)).is_err(),
                "{label} was accepted"
            );
        }
        // A video header only ever reads a video frame, and never a short one.
        assert!(
            parse_video_header(
                &frame(FrameType::Audio, wire(VIDEO_HEADER_SIZE), 0),
                &video_bytes(&supported_video())
            )
            .is_err()
        );
        assert!(parse_video_header(&header, &[0_u8; VIDEO_HEADER_SIZE - 1]).is_err());
    }

    fn audio_bytes(codec: i32, rate: i32, samples: i32, channels: i32, active: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend(codec.to_le_bytes());
        bytes.extend(rate.to_le_bytes());
        bytes.extend(samples.to_le_bytes());
        bytes.extend(channels.to_le_bytes());
        bytes.extend(active.to_le_bytes());
        // The wire header is wider than the five fields the receiver reads.
        bytes.resize(AUDIO_HEADER_SIZE, 0);
        bytes
    }

    /// The declared payload length is what sizes the read, so its arithmetic is
    /// the boundary between a bounded frame and an attacker-chosen allocation.
    #[test]
    fn audio_headers_bind_the_payload_to_the_active_channels() {
        let samples = 480_i32;
        let channels = 2_i32;
        // One 32-bit sample per channel per frame, for the channels that are
        // actually sent.
        let compressed = 480 * 2 * 4_u32;
        let data_length = wire(AUDIO_HEADER_SIZE) + compressed;
        let header = frame(FrameType::Audio, data_length, 0);
        let bytes = audio_bytes(Codec::Fpa1 as i32, 48_000, samples, channels, 0b11);
        assert_eq!(
            parse_audio_header(&header, &bytes).map(|value| value.channels),
            Ok(channels)
        );

        // Half the channels are silent, so half the payload is not sent.
        let one_active = audio_bytes(Codec::Fpa1 as i32, 48_000, samples, channels, 0b01);
        assert!(parse_audio_header(&header, &one_active).is_err());
        let halved = frame(
            FrameType::Audio,
            wire(AUDIO_HEADER_SIZE) + compressed / 2,
            0,
        );
        assert!(parse_audio_header(&halved, &one_active).is_ok());

        // Per-frame metadata is part of the declared payload.
        let with_metadata = frame(FrameType::Audio, data_length + 16, 16);
        assert!(parse_audio_header(&with_metadata, &bytes).is_ok());

        // An active bit above the channel count would index past the payload.
        assert!(
            parse_audio_header(
                &header,
                &audio_bytes(Codec::Fpa1 as i32, 48_000, samples, channels, 0b111)
            )
            .is_err()
        );
        // 32 channels is the ceiling, where the mask uses every bit.
        let full = 480 * 32 * 4_u32;
        assert!(
            parse_audio_header(
                &frame(FrameType::Audio, wire(AUDIO_HEADER_SIZE) + full, 0),
                &audio_bytes(Codec::Fpa1 as i32, 48_000, samples, 32, u32::MAX)
            )
            .is_ok()
        );

        for (label, bytes) in [
            (
                "a codec that is not FPA1",
                audio_bytes(Codec::Vmx1 as i32, 48_000, samples, channels, 0b11),
            ),
            (
                "a sample rate below the floor",
                audio_bytes(Codec::Fpa1 as i32, 4000, samples, channels, 0b11),
            ),
            (
                "a sample rate above the ceiling",
                audio_bytes(Codec::Fpa1 as i32, 200_000, samples, channels, 0b11),
            ),
            (
                "more than 32 channels",
                audio_bytes(Codec::Fpa1 as i32, 48_000, samples, 33, 0b11),
            ),
            (
                "no channels",
                audio_bytes(Codec::Fpa1 as i32, 48_000, samples, 0, 0),
            ),
            (
                "no samples",
                audio_bytes(Codec::Fpa1 as i32, 48_000, 0, channels, 0b11),
            ),
            (
                "a decoded size past the audio ceiling",
                audio_bytes(Codec::Fpa1 as i32, 48_000, i32::MAX, channels, 0b11),
            ),
        ] {
            assert!(
                parse_audio_header(&header, &bytes).is_err(),
                "{label} was accepted"
            );
        }
    }

    /// The fixed header decides how many bytes the channel will read next.
    #[test]
    fn frame_headers_bound_every_payload() {
        let mut bytes = build_metadata("<x/>", 0).unwrap_or_default();
        bytes[0] = 2;
        assert_eq!(
            parse_frame_header(&bytes),
            Err(ProtocolError::Unsupported("OMT frame version"))
        );
        bytes[0] = 1;
        bytes[1] = 3;
        assert_eq!(
            parse_frame_header(&bytes),
            Err(ProtocolError::Unsupported("OMT frame type"))
        );
        assert_eq!(
            parse_frame_header(&bytes[..HEADER_SIZE - 1]),
            Err(ProtocolError::Truncated("OMT frame header"))
        );

        let mut header = [0_u8; HEADER_SIZE];
        header[0] = 1;
        header[1] = FrameType::Video as u8;
        // A video frame shorter than its own extended header.
        header[12..16].copy_from_slice(&(wire(VIDEO_HEADER_SIZE) - 1).to_le_bytes());
        assert!(matches!(
            parse_frame_header(&header),
            Err(ProtocolError::Invalid(_))
        ));
        // A payload past the per-type ceiling is refused before it is allocated.
        let ceiling = wire(VIDEO_MAX_SIZE + VIDEO_HEADER_SIZE);
        header[12..16].copy_from_slice(&ceiling.to_le_bytes());
        assert!(parse_frame_header(&header).is_ok(), "the ceiling itself");
        header[12..16].copy_from_slice(&(ceiling + 1).to_le_bytes());
        assert_eq!(
            parse_frame_header(&header),
            Err(ProtocolError::Oversized("OMT frame payload"))
        );
        // Metadata cannot claim more of the frame than the frame carries.
        header[12..16].copy_from_slice(&(wire(VIDEO_HEADER_SIZE) + 8).to_le_bytes());
        header[10..12].copy_from_slice(&9_u16.to_le_bytes());
        assert!(matches!(
            parse_frame_header(&header),
            Err(ProtocolError::Invalid(_))
        ));
    }
    #[derive(Deserialize)]
    struct Vector {
        value: String,
        valid: bool,
    }
    #[derive(Deserialize)]
    struct ForbiddenCodepoints {
        ranges: Vec<[String; 2]>,
    }
    #[derive(Deserialize)]
    struct Vectors {
        source_names: Vec<Vector>,
        direct_targets: Vec<Vector>,
        forbidden_name_codepoints: ForbiddenCodepoints,
    }
    #[test]
    fn shared_validation_vectors() {
        let vectors: Vectors = serde_json::from_str(include_str!(
            "../../../tests/schema/omt-target-vectors.json"
        ))
        .unwrap_or_else(|e| panic!("{e}"));
        published_table_matches(&vectors.forbidden_name_codepoints);
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

    /// The compiled table has to be the published one, minus the surrogates it
    /// cannot express. Comparing the tables rather than sampling them is what
    /// makes the Web layer's `unicodedata` sweep speak for this validator too.
    fn published_table_matches(published: &ForbiddenCodepoints) {
        let expected: Vec<(u32, u32)> = published
            .ranges
            .iter()
            .map(|[low, high]| {
                (
                    u32::from_str_radix(low, 16).unwrap_or_else(|e| panic!("{low}: {e}")),
                    u32::from_str_radix(high, 16).unwrap_or_else(|e| panic!("{high}: {e}")),
                )
            })
            .filter(|range| *range != SURROGATE_RANGE)
            .collect();
        let actual: Vec<(u32, u32)> = FORBIDDEN_NAME_RANGES
            .iter()
            .map(|(low, high)| (u32::from(*low), u32::from(*high)))
            .collect();
        assert_eq!(
            actual, expected,
            "compiled table differs from the published one"
        );
        assert!(
            published
                .ranges
                .iter()
                .any(|[low, _]| u32::from_str_radix(low, 16) == Ok(SURROGATE_RANGE.0)),
            "the published table no longer lists the surrogates this one filters"
        );
        assert!(
            actual.windows(2).all(|pair| pair[0].1 < pair[1].0),
            "the compiled table must stay sorted and non-overlapping for the binary search"
        );
        for (low, high) in FORBIDDEN_NAME_RANGES {
            for boundary in [low, high] {
                assert!(is_forbidden_name_char(boundary), "{boundary:?}");
                assert!(
                    !is_valid_source_name(&format!("a{boundary}b")),
                    "{boundary:?}"
                );
            }
        }
    }
}
