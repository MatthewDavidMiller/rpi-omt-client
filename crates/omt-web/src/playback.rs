//! What the dashboard knows about playback: the saved source, the discovered
//! sources (cached briefly), the receiver's status record, and the actions
//! that change them through control-omt.sh.

use omt_common::timefmt::{now_ms, parse_rfc3339, wall_clock};
use omt_common::{fsio, json, proc};
use omt_protocol::{is_valid_source_name, parse_direct_target};

use crate::settings::Settings;
use crate::state::{self, SourceTarget};

/// The most text an action result carries, as the C frontend's fixed
/// buffers did.
pub const ACTION_TEXT: usize = 1023;
pub const MAX_CHOICES: usize = 256;

/// `s` cut to at most `max` bytes, on a character boundary.
pub fn clip(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    s
}

#[derive(Clone, Debug, Default)]
pub struct ActionResult {
    pub ok: bool,
    pub message: String,
    pub error: String,
}

impl ActionResult {
    pub fn success(message: impl Into<String>) -> Self {
        Self { ok: true, message: clip(message.into(), ACTION_TEXT), error: String::new() }
    }

    pub fn failure(error: impl Into<String>) -> Self {
        Self { ok: false, message: String::new(), error: clip(error.into(), ACTION_TEXT) }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SourceConfiguration {
    pub source: String,
    pub direct_address: String,
    pub error: String,
}

impl SourceConfiguration {
    pub fn configured(&self) -> bool {
        !self.source.is_empty() && self.error.is_empty()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub state: &'static str,
    pub label: &'static str,
    pub detail: String,
    pub tone: &'static str,
    pub source: String,
    pub direct_address: String,
}

#[derive(Clone, Debug, Default)]
pub struct VideoLimit {
    pub board_label: String,
    pub effective: String,
    pub board_default: String,
    pub error: String,
    pub effective_description: String,
    pub board_default_description: String,
    pub overridden: bool,
    pub above_board_default: bool,
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn ms_of(seconds: f64) -> u64 {
    if seconds <= 0.0 { 0 } else { (seconds * 1000.0) as u64 }
}

/// Runs the controller (control-omt.sh ACTION) with the configured timeout.
pub fn control(s: &Settings, action: &str, timeout_s: f64) -> proc::Output {
    proc::run(&proc::Options::new(&[&s.control_command, action], ms_of(timeout_s)))
}

/// Parses `omt-receiver discover --json` output: deduplicated, sorted, and
/// only names the shared grammar accepts with name == target.
pub fn parse_discovered(output: &str) -> Vec<String> {
    if output.len() > 256 * 1024 {
        return Vec::new();
    }
    let Ok(root) = json::parse(output.as_bytes()) else { return Vec::new() };
    let Some(items) = root.as_array() else { return Vec::new() };
    // Every entry must have string name and target, or the whole answer is
    // refused, as serde refused it.
    let mut pairs = Vec::with_capacity(items.len());
    for e in items {
        let name = e.get("name").and_then(json::Value::as_str);
        let target = e.get("target").and_then(json::Value::as_str);
        match (e.as_object(), name, target) {
            (Some(_), Some(n), Some(t)) => pairs.push((n, t)),
            _ => return Vec::new(),
        }
    }
    let mut out: Vec<String> = Vec::new();
    for (name, target) in pairs {
        if out.len() == MAX_CHOICES {
            break;
        }
        if name == target && is_valid_source_name(name) && !out.iter().any(|n| n == name) {
            out.push(name.to_owned());
        }
    }
    out.sort();
    out
}

const RECEIVER_STATES: [&str; 8] = [
    "running",
    "waiting-for-discovery",
    "waiting-for-hdmi",
    "retrying",
    "degraded",
    "unsupported-format",
    "starting",
    "stopped",
];
const VIDEO_STATES: [&str; 7] = [
    "running",
    "waiting-for-discovery",
    "waiting-for-hdmi",
    "retrying",
    "unsupported-format",
    "starting",
    "stopped",
];
const AUDIO_STATES: [&str; 3] = ["stopped", "running", "failed"];
const CONNECTORS: [&str; 3] = ["none", "HDMI-A-1", "HDMI-A-2"];
const STATUS_FIELDS: [&str; 10] = [
    "schema",
    "state",
    "video_state",
    "audio_state",
    "target",
    "detail",
    "connector",
    "drm_device",
    "alsa_device",
    "updated_at",
];

/// Whether a status record is well-formed, about `expected`, and fresh;
/// its state and detail when it is.
pub fn status_record_valid(
    text: &str,
    expected: &str,
    stale_seconds: u64,
) -> Option<(String, String)> {
    let root = json::parse(text.as_bytes()).ok()?;
    let field = |k: &str| root.get(k).and_then(json::Value::as_str);
    let object = root.as_object()?;
    if object.len() != 10 || !root.only_keys(&STATUS_FIELDS) {
        return None;
    }
    if root.get("schema").and_then(json::Value::as_u64) != Some(1) {
        return None;
    }
    let (s, v, a) = (field("state")?, field("video_state")?, field("audio_state")?);
    let (target, detail, connector) = (field("target")?, field("detail")?, field("connector")?);
    let (drm, alsa, updated) = (field("drm_device")?, field("alsa_device")?, field("updated_at")?);
    let ok = RECEIVER_STATES.contains(&s)
        && VIDEO_STATES.contains(&v)
        && AUDIO_STATES.contains(&a)
        && target == expected
        && detail.len() <= 2048
        && CONNECTORS.contains(&connector)
        && !drm.is_empty()
        && drm.len() <= 256
        && !alsa.is_empty()
        && alsa.len() <= 256
        && !(s == "degraded" && (v != "running" || a != "failed"))
        && !(s == "running" && (v != "running" || a == "failed"))
        && (s == "running" || s == "degraded" || s == v);
    if !ok {
        return None;
    }
    let (at, nanos) = parse_rfc3339(updated)?;
    let (now_s, now_n) = wall_clock();
    // Age in nanoseconds; a record up to five seconds in the future is
    // accepted to tolerate clock skew between the two processes.
    let age = (i128::from(now_s) - i128::from(at)) * 1_000_000_000
        + (i128::from(now_n) - i128::from(nanos));
    if age < -5_000_000_000 || age > i128::from(stale_seconds) * 1_000_000_000 {
        return None;
    }
    Some((s.to_owned(), detail.to_owned()))
}

#[derive(Debug)]
pub struct Playback {
    settings: Settings,
    cache_expires_ms: u64,
    cache: Vec<String>,
}

impl Playback {
    pub fn new(settings: &Settings) -> Self {
        Self { settings: settings.clone(), cache_expires_ms: now_ms(), cache: Vec::new() }
    }

    pub fn configuration(&self) -> SourceConfiguration {
        match state::read_source(&self.settings.source_target_file) {
            Err(e) => SourceConfiguration {
                error: clip(e.to_string(), 255),
                ..SourceConfiguration::default()
            },
            Ok(None) => SourceConfiguration::default(),
            Ok(Some(t)) => SourceConfiguration {
                direct_address: if t.direct { t.value.clone() } else { String::new() },
                source: t.value,
                error: String::new(),
            },
        }
    }

    pub fn sources(&mut self) -> Vec<String> {
        if now_ms() < self.cache_expires_ms {
            return self.cache.clone();
        }
        let s = &self.settings;
        let argv = [s.receiver_command.as_str(), "discover", "--wait-ms", "1500", "--json"];
        let r = proc::run(&proc::Options::new(&argv, ms_of(s.control_timeout_s.max(3.0))));
        let names = if r.succeeded() { parse_discovered(&r.stdout) } else { Vec::new() };
        self.cache.clone_from(&names);
        self.cache_expires_ms = now_ms() + ms_of(s.source_cache_ttl_s);
        names
    }

    pub fn refresh(&mut self) {
        self.cache_expires_ms = now_ms();
        self.cache.clear();
    }

    fn save_and_restart(&mut self, target: &SourceTarget, label: &str) -> ActionResult {
        if let Err(e) = state::save_source(&self.settings.source_target_file, Some(target)) {
            return ActionResult::failure(e.to_string());
        }
        self.refresh();
        let r = control(&self.settings, "restart", self.settings.control_timeout_s);
        if r.succeeded() {
            ActionResult::success(format!("{label} saved and running."))
        } else {
            ActionResult::failure(format!(
                "{label} was saved, but playback could not be restarted. {}",
                r.failure_detail()
            ))
        }
    }

    pub fn select(&mut self, selection: &str) -> ActionResult {
        let trimmed = selection.trim();
        if trimmed.len() >= omt_protocol::TARGET_MAX_BYTES * 2 {
            return ActionResult::failure("Invalid OMT source selection.");
        }
        if let Some(name) = trimmed.strip_prefix("discovered|")
            && is_valid_source_name(name)
        {
            let target = SourceTarget { direct: false, value: name.to_owned() };
            return self.save_and_restart(&target, "OMT discovery");
        }
        if let Some(uri) = trimmed.strip_prefix("direct|")
            && parse_direct_target(uri).is_ok()
        {
            let target = SourceTarget { direct: true, value: uri.to_owned() };
            return self.save_and_restart(&target, "OMT direct target");
        }
        if is_valid_source_name(trimmed) {
            let target = SourceTarget { direct: false, value: trimmed.to_owned() };
            return self.save_and_restart(&target, "OMT discovery");
        }
        ActionResult::failure("Invalid OMT source selection.")
    }

    pub fn save_direct(&mut self, address: &str) -> ActionResult {
        if parse_direct_target(address).is_err() {
            return ActionResult::failure(
                "Direct target must use omt://host:port with no path or credentials.",
            );
        }
        let target = SourceTarget { direct: true, value: address.to_owned() };
        self.save_and_restart(&target, "OMT direct target")
    }

    pub fn restart(&mut self) -> ActionResult {
        match state::read_source(&self.settings.source_target_file) {
            Err(e) => return ActionResult::failure(format!("Saved OMT target is invalid: {e}")),
            Ok(None) => return ActionResult::failure("No OMT source is configured."),
            Ok(Some(_)) => {}
        }
        let c = control(&self.settings, "restart", self.settings.control_timeout_s);
        if c.succeeded() {
            ActionResult::success("OMT playback restarted.")
        } else {
            ActionResult::failure(format!("Unable to restart OMT playback. {}", c.failure_detail()))
        }
    }

    pub fn clear(&mut self) -> ActionResult {
        let c = control(&self.settings, "stop", self.settings.control_timeout_s);
        if !matches!(c.returncode, Some(0 | 3)) {
            return ActionResult::failure(format!(
                "Playback could not be stopped, so the saved target was retained. {}",
                c.failure_detail()
            ));
        }
        if let Err(e) = state::save_source(&self.settings.source_target_file, None) {
            return ActionResult::failure(format!(
                "Playback stopped, but the saved target could not be cleared. {e}"
            ));
        }
        self.refresh();
        ActionResult::success("Playback stopped and the saved target was cleared.")
    }

    pub fn video_limit(&self) -> VideoLimit {
        let s = &self.settings;
        let board_default = s.board_video_ceiling.as_str();
        let mut out = VideoLimit {
            board_label: s.board_label.clone(),
            board_default: board_default.to_owned(),
            board_default_description: state::describe_video_ceiling(board_default),
            ..VideoLimit::default()
        };
        match state::effective_video_ceiling(&s.video_ceiling_file, board_default) {
            Ok(effective) => {
                out.overridden = effective != board_default;
                out.above_board_default = out.overridden
                    && state::pixel_rate(&effective) > state::pixel_rate(board_default);
                out.effective_description = state::describe_video_ceiling(&effective);
                out.effective = effective;
            }
            Err(e) => {
                board_default.clone_into(&mut out.effective);
                out.error = clip(e.to_string(), 255);
                out.effective_description = state::describe_video_ceiling(board_default);
            }
        }
        out
    }

    pub fn save_video_limit(&mut self, value: &str) -> ActionResult {
        let requested = value.trim();
        if requested.len() >= 256 {
            return ActionResult::failure("A video limit must list between 1 and 4 resolutions.");
        }
        let ceiling = if requested.is_empty() { None } else { Some(requested) };
        if let Err(e) = state::save_video_ceiling(&self.settings.video_ceiling_file, ceiling) {
            return ActionResult::failure(e.to_string());
        }
        let label = match ceiling {
            None => "Video limit cleared".to_owned(),
            Some(c) => format!("Video limit set to {}", state::describe_video_ceiling(c)),
        };
        let c = control(&self.settings, "restart", self.settings.control_timeout_s);
        if c.succeeded() {
            ActionResult::success(format!("{label} and playback restarted."))
        } else {
            ActionResult::failure(format!(
                "{label}, but playback could not be restarted. {}",
                c.failure_detail()
            ))
        }
    }

    pub fn summary(&self) -> Summary {
        let summary = |state, label, detail: &str, tone, source: &str, direct: &str| Summary {
            state,
            label,
            detail: detail.to_owned(),
            tone,
            source: source.to_owned(),
            direct_address: direct.to_owned(),
        };
        let target = match state::read_source(&self.settings.source_target_file) {
            Err(e) => {
                let detail = clip(e.to_string(), 255);
                return summary(
                    "configuration-error",
                    "Source configuration invalid",
                    &detail,
                    "danger",
                    "",
                    "",
                );
            }
            Ok(None) => {
                return summary(
                    "unconfigured",
                    "No source configured",
                    "Select a discovered source or configure a direct OMT target.",
                    "neutral",
                    "",
                    "",
                );
            }
            Ok(Some(t)) => t,
        };
        let direct = if target.direct { target.value.as_str() } else { "" };
        let value = target.value.as_str();
        let Ok(Some(data)) = fsio::read_bounded(&self.settings.playback_status_file, 4096) else {
            let c = control(&self.settings, "status", self.settings.control_timeout_s);
            return if c.succeeded() {
                summary(
                    "starting",
                    "Starting playback",
                    "The receiver is running and has not published fresh status yet.",
                    "warning",
                    value,
                    direct,
                )
            } else {
                summary(
                    "stopped",
                    "Playback stopped",
                    "A target is saved but the receiver is not running.",
                    "neutral",
                    value,
                    direct,
                )
            };
        };
        let record = std::str::from_utf8(&data).ok().and_then(|text| {
            status_record_valid(text, value, self.settings.playback_status_stale_s)
        });
        let Some((st, detail)) = record else {
            return summary(
                "stale",
                "Playback status stale",
                "The receiver status record is unavailable or stale.",
                "warning",
                value,
                direct,
            );
        };
        let (public, label, tone) = match st.as_str() {
            "running" => ("playing", "Playing", "success"),
            "waiting-for-discovery" => {
                ("waiting-for-discovery", "Waiting for discovery", "warning")
            }
            "waiting-for-hdmi" => ("waiting-for-hdmi", "Waiting for HDMI", "warning"),
            "retrying" => ("retrying", "Retrying playback", "warning"),
            "degraded" => ("degraded", "Playback degraded", "warning"),
            "unsupported-format" => ("unsupported-format", "Unsupported video format", "danger"),
            "starting" => ("starting", "Starting playback", "warning"),
            _ => ("stopped", "Playback stopped", "neutral"),
        };
        summary(public, label, &detail, tone, value, direct)
    }
}
