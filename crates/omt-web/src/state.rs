//! The operator's saved choices: the OMT target and the video ceiling
//! override. Each lives in a small strict-JSON file that is read with a
//! size cap and replaced atomically.

use omt_common::{Error, Result, fsio, json};
use omt_protocol::{is_valid_source_name, parse_direct_target};

const SOURCE_LIMIT: usize = 1024;
const CEILING_LIMIT: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceTarget {
    pub direct: bool,
    pub value: String,
}

/// Reads a strict JSON record; `Ok(None)` when the file does not exist.
fn read_record(path: &str, limit: usize, what: &str) -> Result<Option<json::Value>> {
    let Some(raw) = fsio::read_bounded(path, limit)? else { return Ok(None) };
    json::parse(&raw).map(Some).map_err(|e| Error::new(format!("{what} is invalid JSON: {e}")))
}

/// serde's `u8` field: a non-negative integer of at most 255.
fn schema(root: &json::Value) -> Option<u64> {
    root.get("schema").and_then(json::Value::as_u64).filter(|&v| v <= 255)
}

/// A string field that may also be absent or null (serde's `Option`): the
/// outer `None` is a field of the wrong type, the inner one an absent value.
#[allow(clippy::option_option)]
fn optional_str<'a>(root: &'a json::Value, key: &str) -> Option<Option<&'a str>> {
    match root.get(key) {
        None => Some(None),
        Some(v) if v.is_null() => Some(None),
        Some(v) => v.as_str().map(Some),
    }
}

pub fn read_source(path: &str) -> Result<Option<SourceTarget>> {
    let Some(root) = read_record(path, SOURCE_LIMIT, "saved OMT target")? else { return Ok(None) };
    let kind = root.get("kind").and_then(json::Value::as_str);
    let name = optional_str(&root, "name");
    let uri = optional_str(&root, "uri");
    let shaped = root.as_object().is_some()
        && root.only_keys(&["schema", "kind", "name", "uri"])
        && schema(&root).is_some()
        && kind.is_some()
        && name.is_some()
        && uri.is_some();
    if !shaped {
        return Err(Error::new("saved OMT target is invalid JSON: unexpected record shape"));
    }
    if schema(&root) != Some(1) {
        return Err(Error::new("saved OMT target has an invalid schema"));
    }
    match (kind, name.flatten(), uri.flatten()) {
        (Some("discovered"), Some(name), None) if is_valid_source_name(name) => {
            Ok(Some(SourceTarget { direct: false, value: name.to_owned() }))
        }
        (Some("direct"), None, Some(uri)) if parse_direct_target(uri).is_ok() => {
            Ok(Some(SourceTarget { direct: true, value: uri.to_owned() }))
        }
        _ => Err(Error::new("saved OMT target kind or value is invalid")),
    }
}

/// Saves, or removes the file when `target` is `None`.
pub fn save_source(path: &str, target: Option<&SourceTarget>) -> Result<()> {
    let Some(target) = target else { return fsio::remove_file_durable(path) };
    let valid = if target.direct {
        parse_direct_target(&target.value).is_ok()
    } else {
        is_valid_source_name(&target.value)
    };
    if !valid {
        return Err(Error::new("invalid OMT target kind or value"));
    }
    let (kind, field) = if target.direct { ("direct", "uri") } else { ("discovered", "name") };
    let mut out = format!("{{\"schema\":1,\"kind\":\"{kind}\",\"{field}\":");
    json::write_string(&mut out, &target.value);
    out.push_str("}\n");
    if out.len() > SOURCE_LIMIT {
        return Err(Error::new("replacement exceeds its size limit"));
    }
    fsio::atomic_replace(path, out.as_bytes(), SOURCE_LIMIT)
}

fn all_digits(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_digit())
}

/// Validates WIDTHxHEIGHT@FPS[,...] with the Web form's rules; the message
/// names the offending part.
pub fn parse_video_ceiling(value: &str) -> Result<()> {
    if value.matches(',').count() + 1 > 4 {
        return Err(Error::new("A video limit must list between 1 and 4 resolutions."));
    }
    for shape in value.split(',') {
        let invalid =
            || Error::new(format!("Invalid video limit: {shape}. Expected WIDTHxHEIGHT@FPS."));
        let (size, fps) = shape.split_once('@').ok_or_else(invalid)?;
        let (w, h) = size.split_once('x').ok_or_else(invalid)?;
        if !(2..=4).contains(&w.len())
            || !(2..=4).contains(&h.len())
            || !(1..=3).contains(&fps.len())
            || !all_digits(w)
            || !all_digits(h)
            || !all_digits(fps)
        {
            return Err(invalid());
        }
        let (w, h, f): (u64, u64, u64) =
            (w.parse().unwrap_or(0), h.parse().unwrap_or(0), fps.parse().unwrap_or(0));
        // fps parses as u8, so a three-digit rate past 255 is a parse error.
        if f > 255 {
            return Err(Error::new("Invalid frame rate"));
        }
        if !(16..=1920).contains(&w) {
            return Err(Error::new(format!("Width {w} is outside 16-1920.")));
        }
        if !(16..=1080).contains(&h) {
            return Err(Error::new(format!("Height {h} is outside 16-1080.")));
        }
        if !(1..=60).contains(&f) {
            return Err(Error::new(format!("Frame rate {f} is outside 1-60.")));
        }
    }
    Ok(())
}

pub fn describe_video_ceiling(value: &str) -> String {
    let mut out = String::new();
    for (i, shape) in value.split(',').enumerate() {
        if i > 0 {
            out.push_str(", or ");
        }
        match shape.split_once('@') {
            None => out.push_str(shape),
            Some((size, fps)) => {
                out.push_str(size);
                out.push_str(" at ");
                // u8::from_str normalizes "060" to 60; anything else is kept.
                match fps.parse::<u8>() {
                    Ok(f) => out.push_str(&f.to_string()),
                    Err(_) => out.push_str(fps),
                }
                out.push_str(" fps");
            }
        }
    }
    out
}

/// The saved override; `Ok(None)` when none is saved.
pub fn read_video_ceiling(path: &str) -> Result<Option<String>> {
    let Some(root) = read_record(path, CEILING_LIMIT, "saved video limit")? else {
        return Ok(None);
    };
    let ceiling = root.get("ceiling").and_then(json::Value::as_str);
    let Some(ceiling) = ceiling.filter(|_| {
        root.as_object().is_some()
            && root.only_keys(&["schema", "ceiling"])
            && schema(&root).is_some()
    }) else {
        return Err(Error::new("saved video limit is invalid JSON: unexpected record shape"));
    };
    if schema(&root) != Some(1) {
        return Err(Error::new("saved video limit has an invalid schema"));
    }
    parse_video_ceiling(ceiling)?;
    Ok(Some(ceiling.to_owned()))
}

pub fn effective_video_ceiling(path: &str, board_default: &str) -> Result<String> {
    parse_video_ceiling(board_default)?;
    Ok(read_video_ceiling(path)?.unwrap_or_else(|| board_default.to_owned()))
}

/// Saves, or removes the file when `ceiling` is `None`.
pub fn save_video_ceiling(path: &str, ceiling: Option<&str>) -> Result<()> {
    let Some(ceiling) = ceiling else { return fsio::remove_file_durable(path) };
    parse_video_ceiling(ceiling)?;
    let mut out = String::from("{\"schema\":1,\"ceiling\":");
    json::write_string(&mut out, ceiling);
    out.push_str("}\n");
    if out.len() > CEILING_LIMIT {
        return Err(Error::new("replacement exceeds its size limit"));
    }
    fsio::atomic_replace(path, out.as_bytes(), CEILING_LIMIT)
}

/// The largest width x height x fps among the shapes.
pub fn pixel_rate(value: &str) -> u64 {
    value
        .split(',')
        .filter_map(|shape| {
            let (size, fps) = shape.split_once('@')?;
            let (w, h) = size.split_once('x')?;
            let digits =
                |s: &str| if s.is_empty() || !all_digits(s) { None } else { s.parse::<u64>().ok() };
            digits(w)?.checked_mul(digits(h)?)?.checked_mul(digits(fps)?)
        })
        .max()
        .unwrap_or(0)
}
