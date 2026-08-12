use crate::{
    command::{CommandResult, run},
    io::read_bounded,
    json,
    settings::Settings,
    state::{self, SourceTarget},
};
use omt_protocol::{is_valid_source_name, parse_direct_target};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    sync::Mutex,
    time::{Duration, Instant, SystemTime},
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Clone, Debug, Default)]
pub struct ActionResult {
    pub ok: bool,
    pub message: String,
    pub error: String,
}

impl ActionResult {
    fn success(message: impl Into<String>) -> Self {
        Self {
            ok: true,
            message: message.into(),
            error: String::new(),
        }
    }
    fn failure(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: String::new(),
            error: error.into(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SourceChoice {
    pub name: String,
    pub backend: &'static str,
    pub selection_value: String,
    pub display_label: String,
}

#[derive(Clone, Debug, Default, Serialize)]
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

#[derive(Clone, Debug, Serialize)]
pub struct PlaybackSummary {
    pub state: String,
    pub label: String,
    pub detail: String,
    pub tone: String,
    pub source: String,
    pub direct_address: String,
}

#[derive(Clone, Debug, Serialize)]
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

#[derive(Deserialize)]
struct DiscoveryEntry {
    name: String,
    target: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusRecord {
    schema: u8,
    state: String,
    video_state: String,
    audio_state: String,
    target: String,
    detail: String,
    connector: String,
    drm_device: String,
    alsa_device: String,
    updated_at: String,
}

struct SourceCache {
    expires: Instant,
    sources: Vec<SourceChoice>,
}

pub struct Playback {
    settings: Settings,
    state_lock: Mutex<()>,
    cache: Mutex<SourceCache>,
}

impl Playback {
    pub fn new(settings: &Settings) -> Self {
        Self {
            settings: settings.clone(),
            state_lock: Mutex::new(()),
            cache: Mutex::new(SourceCache {
                expires: Instant::now(),
                sources: Vec::new(),
            }),
        }
    }

    pub fn configuration(&self) -> SourceConfiguration {
        match state::read_source(&self.settings.source_target_file) {
            Ok(None) => SourceConfiguration::default(),
            Ok(Some(target)) => SourceConfiguration {
                source: target.value().to_owned(),
                direct_address: if target.is_direct() {
                    target.value().to_owned()
                } else {
                    String::new()
                },
                error: String::new(),
            },
            Err(error) => SourceConfiguration {
                error,
                ..SourceConfiguration::default()
            },
        }
    }

    pub fn sources(&self) -> Vec<SourceChoice> {
        let Ok(mut cache) = self.cache.lock() else {
            return Vec::new();
        };
        if Instant::now() < cache.expires {
            return cache.sources.clone();
        }
        let result = run(
            &self.settings.receiver_command,
            &["discover", "--wait-ms", "1500", "--json"],
            self.settings.control_timeout.max(Duration::from_secs(3)),
        );
        let choices = if result.returncode == Some(0) {
            parse_sources(&result.stdout)
        } else {
            Vec::new()
        };
        cache.sources.clone_from(&choices);
        cache.expires = Instant::now() + self.settings.source_cache_ttl;
        choices
    }

    pub fn refresh(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.expires = Instant::now();
            cache.sources.clear();
        }
    }

    fn control(&self, action: &str) -> CommandResult {
        run(
            &self.settings.control_command,
            &[action],
            self.settings.control_timeout,
        )
    }

    fn save_and_restart(&self, target: SourceTarget, label: &str) -> ActionResult {
        let Ok(_guard) = self.state_lock.lock() else {
            return ActionResult::failure("Source configuration lock failed.");
        };
        if let Err(error) = state::save_source(&self.settings.source_target_file, Some(&target)) {
            return ActionResult::failure(error);
        }
        self.refresh();
        let restarted = self.control("restart");
        if restarted.returncode == Some(0) {
            ActionResult::success(format!("{label} saved and running."))
        } else {
            ActionResult::failure(format!(
                "{label} was saved, but playback could not be restarted. {}",
                restarted.failure_detail()
            ))
        }
    }

    pub fn select(&self, selection: &str) -> ActionResult {
        let trimmed = selection.trim();
        if let Some(name) = trimmed
            .strip_prefix("discovered|")
            .filter(|name| is_valid_source_name(name))
        {
            return self
                .save_and_restart(SourceTarget::Discovered(name.to_owned()), "OMT discovery");
        }
        if let Some(uri) = trimmed
            .strip_prefix("direct|")
            .filter(|uri| parse_direct_target(uri).is_ok())
        {
            return self
                .save_and_restart(SourceTarget::Direct(uri.to_owned()), "OMT direct target");
        }
        if is_valid_source_name(trimmed) {
            return self.save_and_restart(
                SourceTarget::Discovered(trimmed.to_owned()),
                "OMT discovery",
            );
        }
        ActionResult::failure("Invalid OMT source selection.")
    }

    pub fn save_direct(&self, address: &str) -> ActionResult {
        if parse_direct_target(address).is_err() {
            return ActionResult::failure(
                "Direct target must use omt://host:port with no path or credentials.",
            );
        }
        self.save_and_restart(
            SourceTarget::Direct(address.to_owned()),
            "OMT direct target",
        )
    }

    pub fn restart(&self) -> ActionResult {
        match state::read_source(&self.settings.source_target_file) {
            Err(error) => ActionResult::failure(format!("Saved OMT target is invalid: {error}")),
            Ok(None) => ActionResult::failure("No OMT source is configured."),
            Ok(Some(_)) => {
                let result = self.control("restart");
                if result.returncode == Some(0) {
                    ActionResult::success("OMT playback restarted.")
                } else {
                    ActionResult::failure(format!(
                        "Unable to restart OMT playback. {}",
                        result.failure_detail()
                    ))
                }
            }
        }
    }

    pub fn clear(&self) -> ActionResult {
        let Ok(_guard) = self.state_lock.lock() else {
            return ActionResult::failure("Source configuration lock failed.");
        };
        let stopped = self.control("stop");
        if !matches!(stopped.returncode, Some(0 | 3)) {
            return ActionResult::failure(format!(
                "Playback could not be stopped, so the saved target was retained. {}",
                stopped.failure_detail()
            ));
        }
        if let Err(error) = state::save_source(&self.settings.source_target_file, None) {
            return ActionResult::failure(format!(
                "Playback stopped, but the saved target could not be cleared. {error}"
            ));
        }
        self.refresh();
        ActionResult::success("Playback stopped and the saved target was cleared.")
    }

    pub fn video_limit(&self) -> VideoLimit {
        let board_default = self.settings.board_video_ceiling.clone();
        match state::effective_video_ceiling(&self.settings.video_ceiling_file, &board_default) {
            Ok(effective) => {
                let overridden = effective != board_default;
                VideoLimit {
                    board_label: self.settings.board_label.clone(),
                    effective_description: state::describe_video_ceiling(&effective),
                    board_default_description: state::describe_video_ceiling(&board_default),
                    above_board_default: overridden
                        && state::pixel_rate(&effective) > state::pixel_rate(&board_default),
                    overridden,
                    effective,
                    board_default,
                    error: String::new(),
                }
            }
            Err(error) => VideoLimit {
                board_label: self.settings.board_label.clone(),
                effective_description: state::describe_video_ceiling(&board_default),
                board_default_description: state::describe_video_ceiling(&board_default),
                overridden: false,
                above_board_default: false,
                effective: board_default.clone(),
                board_default,
                error,
            },
        }
    }

    pub fn save_video_limit(&self, value: &str) -> ActionResult {
        let requested = value.trim();
        let result = if requested.is_empty() {
            state::save_video_ceiling(&self.settings.video_ceiling_file, None)
        } else {
            state::save_video_ceiling(&self.settings.video_ceiling_file, Some(requested))
        };
        if let Err(error) = result {
            return ActionResult::failure(error);
        }
        let label = if requested.is_empty() {
            "Video limit cleared".to_owned()
        } else {
            format!(
                "Video limit set to {}",
                state::describe_video_ceiling(requested)
            )
        };
        let restarted = self.control("restart");
        if restarted.returncode == Some(0) {
            ActionResult::success(format!("{label} and playback restarted."))
        } else {
            ActionResult::failure(format!(
                "{label}, but playback could not be restarted. {}",
                restarted.failure_detail()
            ))
        }
    }

    pub fn playback(&self) -> PlaybackSummary {
        let target = match state::read_source(&self.settings.source_target_file) {
            Err(error) => {
                return summary(
                    "configuration-error",
                    "Source configuration invalid",
                    &error,
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
            Ok(Some(target)) => target,
        };
        let source = target.value();
        let direct = if target.is_direct() { source } else { "" };
        let Ok(Some(data)) = read_bounded(&self.settings.playback_status_file, 4_096) else {
            let control = self.control("status");
            return if control.returncode == Some(0) {
                summary(
                    "starting",
                    "Starting playback",
                    "The receiver is running and has not published fresh status yet.",
                    "warning",
                    source,
                    direct,
                )
            } else {
                summary(
                    "stopped",
                    "Playback stopped",
                    "A target is saved but the receiver is not running.",
                    "neutral",
                    source,
                    direct,
                )
            };
        };
        let valid = json::from_slice::<StatusRecord>(&data)
            .ok()
            .filter(|status| status_valid(status, source, self.settings.playback_status_stale));
        let Some(status) = valid else {
            return summary(
                "stale",
                "Playback status stale",
                "The receiver status record is unavailable or stale.",
                "warning",
                source,
                direct,
            );
        };
        let (public, label, tone) = match status.state.as_str() {
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
        summary(public, label, &status.detail, tone, source, direct)
    }
}

fn summary(
    state: &str,
    label: &str,
    detail: &str,
    tone: &str,
    source: &str,
    direct: &str,
) -> PlaybackSummary {
    PlaybackSummary {
        state: state.to_owned(),
        label: label.to_owned(),
        detail: detail.to_owned(),
        tone: tone.to_owned(),
        source: source.to_owned(),
        direct_address: direct.to_owned(),
    }
}

fn parse_sources(output: &str) -> Vec<SourceChoice> {
    if output.len() > 256 * 1024 {
        return Vec::new();
    }
    let Ok(entries) = json::from_str::<Vec<DiscoveryEntry>>(output) else {
        return Vec::new();
    };
    entries
        .into_iter()
        .filter_map(|entry| {
            if entry.name == entry.target && is_valid_source_name(&entry.name) {
                Some(entry.name)
            } else {
                None
            }
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|name| SourceChoice {
            selection_value: format!("discovered|{name}"),
            display_label: format!("{name} — OMT discovery"),
            name,
            backend: "OMT discovery",
        })
        .collect()
}

fn status_valid(status: &StatusRecord, expected: &str, stale: Duration) -> bool {
    let receiver_states = [
        "running",
        "waiting-for-discovery",
        "waiting-for-hdmi",
        "retrying",
        "degraded",
        "unsupported-format",
        "starting",
        "stopped",
    ];
    let video_states = [
        "running",
        "waiting-for-discovery",
        "waiting-for-hdmi",
        "retrying",
        "unsupported-format",
        "starting",
        "stopped",
    ];
    if status.schema != 1
        || !receiver_states.contains(&status.state.as_str())
        || !video_states.contains(&status.video_state.as_str())
        || !["stopped", "running", "failed"].contains(&status.audio_state.as_str())
        || status.target != expected
        || status.detail.len() > 2_048
        || !["none", "HDMI-A-1", "HDMI-A-2"].contains(&status.connector.as_str())
        || status.drm_device.is_empty()
        || status.drm_device.len() > 256
        || status.alsa_device.is_empty()
        || status.alsa_device.len() > 256
        || (status.state == "degraded"
            && (status.video_state != "running" || status.audio_state != "failed"))
        || (status.state == "running"
            && (status.video_state != "running" || status.audio_state == "failed"))
        || (!matches!(status.state.as_str(), "running" | "degraded")
            && status.state != status.video_state)
    {
        return false;
    }
    let Ok(updated) = OffsetDateTime::parse(&status.updated_at, &Rfc3339) else {
        return false;
    };
    let now = OffsetDateTime::from(SystemTime::now());
    let age = now - updated;
    age >= time::Duration::seconds(-5)
        && age <= time::Duration::try_from(stale).unwrap_or(time::Duration::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_is_deduplicated_and_sorted() {
        let values = parse_sources(
            r#"[{"name":"B","target":"B"},{"name":"A","target":"A"},{"name":"A","target":"A"}]"#,
        );
        assert_eq!(
            values
                .iter()
                .map(|value| value.name.as_str())
                .collect::<Vec<_>>(),
            ["A", "B"]
        );
    }

    #[test]
    fn public_states_cover_the_shared_status_contract() {
        let document: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/schema/playback-status-vectors.json"
        ))
        .unwrap_or_else(|error| panic!("{error}"));
        let states = document["receiver_states"]
            .as_array()
            .unwrap_or_else(|| panic!("receiver_states is not an array"));
        let expected: BTreeSet<_> = states
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect();
        let actual: BTreeSet<_> = [
            "running",
            "waiting-for-discovery",
            "waiting-for-hdmi",
            "retrying",
            "degraded",
            "unsupported-format",
            "starting",
            "stopped",
        ]
        .into_iter()
        .collect();
        assert_eq!(actual, expected);
    }
}
