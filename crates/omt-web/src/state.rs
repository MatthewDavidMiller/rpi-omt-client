use crate::{
    io::{atomic_replace, read_bounded, remove_file_durable},
    json,
};
use omt_protocol::{is_valid_source_name, parse_direct_target};
use serde::{Deserialize, Serialize};
use std::path::Path;

const SOURCE_LIMIT: usize = 1_024;
const CEILING_LIMIT: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceTarget {
    Discovered(String),
    Direct(String),
}

impl SourceTarget {
    pub fn value(&self) -> &str {
        match self {
            Self::Discovered(value) | Self::Direct(value) => value,
        }
    }

    pub fn is_direct(&self) -> bool {
        matches!(self, Self::Direct(_))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTarget {
    schema: u8,
    kind: String,
    name: Option<String>,
    uri: Option<String>,
}

pub fn read_source(path: &Path) -> Result<Option<SourceTarget>, String> {
    let Some(data) = read_bounded(path, SOURCE_LIMIT)? else {
        return Ok(None);
    };
    let raw: RawTarget = json::from_slice(&data)
        .map_err(|error| format!("saved OMT target is invalid JSON: {error}"))?;
    if raw.schema != 1 {
        return Err("saved OMT target has an invalid schema".to_owned());
    }
    match (raw.kind.as_str(), raw.name, raw.uri) {
        ("discovered", Some(name), None) if is_valid_source_name(&name) => {
            Ok(Some(SourceTarget::Discovered(name)))
        }
        ("direct", None, Some(uri)) if parse_direct_target(&uri).is_ok() => {
            Ok(Some(SourceTarget::Direct(uri)))
        }
        _ => Err("saved OMT target kind or value is invalid".to_owned()),
    }
}

#[derive(Serialize)]
struct SavedTarget<'a> {
    schema: u8,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    uri: Option<&'a str>,
}

pub fn save_source(path: &Path, target: Option<&SourceTarget>) -> Result<(), String> {
    let Some(target) = target else {
        return remove_file_durable(path);
    };
    let raw = match target {
        SourceTarget::Discovered(name) if is_valid_source_name(name) => SavedTarget {
            schema: 1,
            kind: "discovered",
            name: Some(name),
            uri: None,
        },
        SourceTarget::Direct(uri) if parse_direct_target(uri).is_ok() => SavedTarget {
            schema: 1,
            kind: "direct",
            name: None,
            uri: Some(uri),
        },
        _ => return Err("invalid OMT target kind or value".to_owned()),
    };
    let mut data = serde_json::to_vec(&raw).map_err(|error| error.to_string())?;
    data.push(b'\n');
    atomic_replace(path, &data, SOURCE_LIMIT)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SavedCeiling {
    schema: u8,
    ceiling: String,
}

pub fn parse_video_ceiling(value: &str) -> Result<String, String> {
    let shapes: Vec<_> = value.split(',').collect();
    if shapes.is_empty() || shapes.len() > 4 {
        return Err("A video limit must list between 1 and 4 resolutions.".to_owned());
    }
    for shape in &shapes {
        let (dimensions, fps_text) = shape
            .split_once('@')
            .ok_or_else(|| format!("Invalid video limit: {shape}. Expected WIDTHxHEIGHT@FPS."))?;
        let (width_text, height_text) = dimensions
            .split_once('x')
            .ok_or_else(|| format!("Invalid video limit: {shape}. Expected WIDTHxHEIGHT@FPS."))?;
        if width_text.len() < 2
            || width_text.len() > 4
            || height_text.len() < 2
            || height_text.len() > 4
            || fps_text.is_empty()
            || fps_text.len() > 3
            || !width_text
                .bytes()
                .chain(height_text.bytes())
                .chain(fps_text.bytes())
                .all(|byte| byte.is_ascii_digit())
        {
            return Err(format!(
                "Invalid video limit: {shape}. Expected WIDTHxHEIGHT@FPS."
            ));
        }
        let width: u16 = width_text
            .parse()
            .map_err(|_| "Invalid video width".to_owned())?;
        let height: u16 = height_text
            .parse()
            .map_err(|_| "Invalid video height".to_owned())?;
        let fps: u8 = fps_text
            .parse()
            .map_err(|_| "Invalid frame rate".to_owned())?;
        if !(16..=1920).contains(&width) {
            return Err(format!("Width {width} is outside 16-1920."));
        }
        if !(16..=1080).contains(&height) {
            return Err(format!("Height {height} is outside 16-1080."));
        }
        if !(1..=60).contains(&fps) {
            return Err(format!("Frame rate {fps} is outside 1-60."));
        }
    }
    Ok(value.to_owned())
}

pub fn describe_video_ceiling(value: &str) -> String {
    value
        .split(',')
        .map(|shape| {
            let Some((dimensions, fps)) = shape.split_once('@') else {
                return shape.to_owned();
            };
            let normalized = fps
                .parse::<u8>()
                .map_or_else(|_| fps.to_owned(), |number| number.to_string());
            format!("{dimensions} at {normalized} fps")
        })
        .collect::<Vec<_>>()
        .join(", or ")
}

pub fn read_video_ceiling(path: &Path) -> Result<Option<String>, String> {
    let Some(data) = read_bounded(path, CEILING_LIMIT)? else {
        return Ok(None);
    };
    let saved: SavedCeiling = json::from_slice(&data)
        .map_err(|error| format!("saved video limit is invalid JSON: {error}"))?;
    if saved.schema != 1 {
        return Err("saved video limit has an invalid schema".to_owned());
    }
    parse_video_ceiling(&saved.ceiling).map(Some)
}

pub fn effective_video_ceiling(path: &Path, board_default: &str) -> Result<String, String> {
    let default = parse_video_ceiling(board_default)?;
    read_video_ceiling(path).map(|override_value| override_value.unwrap_or(default))
}

pub fn save_video_ceiling(path: &Path, ceiling: Option<&str>) -> Result<(), String> {
    let Some(value) = ceiling else {
        return remove_file_durable(path);
    };
    let saved = SavedCeiling {
        schema: 1,
        ceiling: parse_video_ceiling(value)?,
    };
    let mut data = serde_json::to_vec(&saved).map_err(|error| error.to_string())?;
    data.push(b'\n');
    atomic_replace(path, &data, CEILING_LIMIT)
}

const DELAY_LIMIT: usize = 64;
/// No playout delay: frames go to HDMI and ALSA as they arrive.
pub const DEFAULT_PLAYOUT_DELAY_MS: u64 = 0;
pub const MAX_PLAYOUT_DELAY_MS: u64 = 8000;
/// The saved-file schema. Schema 1 held whole seconds; see
/// [`read_playout_delay`] for what becomes of such a file.
const DELAY_SCHEMA: u8 = 2;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SavedDelay {
    schema: u8,
    milliseconds: u64,
}

/// Only the schema, so a schema-1 file can be recognised without its
/// seconds field failing the strict parse.
#[derive(Deserialize)]
struct DelaySchema {
    schema: u8,
}

pub fn parse_playout_delay(value: &str) -> Result<u64, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
        return Ok(DEFAULT_PLAYOUT_DELAY_MS);
    }
    let invalid = || {
        format!(
            "Invalid playout delay: {trimmed}. Expected a whole number of milliseconds from 0 to {MAX_PLAYOUT_DELAY_MS}."
        )
    };
    // `u64::from_str` also takes a leading `+`; a setting is digits only.
    if !trimmed.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    let parsed = trimmed.parse::<u64>().map_err(|_| invalid())?;
    if parsed > MAX_PLAYOUT_DELAY_MS {
        return Err(format!(
            "Playout delay {parsed} ms is outside 0-{MAX_PLAYOUT_DELAY_MS}."
        ));
    }
    Ok(parsed)
}

/// The saved override in milliseconds, or `None` when there is none.
///
/// A schema-1 file is from before the delay was in milliseconds. It is
/// deliberately treated as absent rather than converted or refused: the
/// default moved from 4 s to 0 at the same time, and refusing it would stop
/// the launcher from starting playback at all after an upgrade.
pub fn read_playout_delay(path: &Path) -> Result<Option<u64>, String> {
    let Some(data) = read_bounded(path, DELAY_LIMIT)? else {
        return Ok(None);
    };
    let schema: DelaySchema = json::from_slice(&data)
        .map_err(|error| format!("saved playout delay is invalid JSON: {error}"))?;
    if schema.schema == 1 {
        return Ok(None);
    }
    if schema.schema != DELAY_SCHEMA {
        return Err("saved playout delay has an invalid schema".to_owned());
    }
    let saved: SavedDelay = json::from_slice(&data)
        .map_err(|error| format!("saved playout delay is invalid JSON: {error}"))?;
    if saved.milliseconds > MAX_PLAYOUT_DELAY_MS {
        return Err(format!(
            "Playout delay {} ms is outside 0-{MAX_PLAYOUT_DELAY_MS}.",
            saved.milliseconds
        ));
    }
    Ok(Some(saved.milliseconds))
}

pub fn effective_playout_delay(path: &Path, default: &str) -> Result<u64, String> {
    let default = parse_playout_delay(default)?;
    read_playout_delay(path).map(|override_value| override_value.unwrap_or(default))
}

pub fn save_playout_delay(path: &Path, milliseconds: Option<u64>) -> Result<(), String> {
    let Some(milliseconds) = milliseconds else {
        return remove_file_durable(path);
    };
    if milliseconds > MAX_PLAYOUT_DELAY_MS {
        return Err(format!(
            "Playout delay {milliseconds} ms is outside 0-{MAX_PLAYOUT_DELAY_MS}."
        ));
    }
    if milliseconds == DEFAULT_PLAYOUT_DELAY_MS {
        return remove_file_durable(path);
    }
    let saved = SavedDelay {
        schema: DELAY_SCHEMA,
        milliseconds,
    };
    let mut data = serde_json::to_vec(&saved).map_err(|error| error.to_string())?;
    data.push(b'\n');
    atomic_replace(path, &data, DELAY_LIMIT)
}

pub fn pixel_rate(value: &str) -> u64 {
    value
        .split(',')
        .filter_map(|shape| {
            let (dimensions, fps) = shape.split_once('@')?;
            let (width, height) = dimensions.split_once('x')?;
            Some(
                width.parse::<u64>().ok()?
                    * height.parse::<u64>().ok()?
                    * fps.parse::<u64>().ok()?,
            )
        })
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ceilings_are_strict_and_described() {
        assert_eq!(
            parse_video_ceiling("1920x1080@60"),
            Ok("1920x1080@60".to_owned())
        );
        assert!(parse_video_ceiling("1921x1080@60").is_err());
        assert!(parse_video_ceiling("1920x1080@0").is_err());
        assert_eq!(
            describe_video_ceiling("1920x1080@30,1280x720@60"),
            "1920x1080 at 30 fps, or 1280x720 at 60 fps"
        );
    }

    #[test]
    fn playout_delay_parses_empty_auto_and_bounds() {
        assert_eq!(parse_playout_delay(""), Ok(0));
        assert_eq!(parse_playout_delay("auto"), Ok(0));
        assert_eq!(parse_playout_delay("0"), Ok(0));
        assert_eq!(parse_playout_delay(" 250 "), Ok(250));
        assert_eq!(parse_playout_delay("8000"), Ok(8000));
        assert!(parse_playout_delay("8001").is_err());
        assert!(parse_playout_delay("-1").is_err());
        assert!(parse_playout_delay("+5").is_err());
        assert!(parse_playout_delay("1.5").is_err());
        assert!(parse_playout_delay("99999999999999999999999").is_err());
    }
}
