#![forbid(unsafe_code)]

use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const HEARTBEAT: Duration = Duration::from_millis(500);
pub const DETAIL_LIMIT: usize = 2048;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum VideoState {
    Running,
    WaitingForDiscovery,
    WaitingForHdmi,
    Retrying,
    UnsupportedFormat,
    Starting,
    Stopped,
    Failed,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AudioState {
    Stopped,
    Running,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Connector {
    pub name: String,
    pub drm_device: String,
    pub alsa_device: String,
}
impl Connector {
    pub fn none() -> Self {
        Self {
            name: "none".into(),
            drm_device: "none".into(),
            alsa_device: "none".into(),
        }
    }
}

#[derive(Serialize)]
struct StatusDocument<'a> {
    schema: u8,
    state: &'a str,
    video_state: VideoState,
    audio_state: AudioState,
    target: &'a str,
    detail: &'a str,
    connector: &'a str,
    drm_device: &'a str,
    alsa_device: &'a str,
    updated_at: String,
}

#[derive(Clone)]
struct Projection {
    video: VideoState,
    audio: AudioState,
    video_detail: String,
    audio_detail: String,
    connector: Connector,
}
struct Published {
    projection: Projection,
    at: Instant,
}
struct Inner {
    current: Projection,
    published: Option<Published>,
    sequence: u64,
}

pub struct PlaybackStatus {
    path: PathBuf,
    target: String,
    inner: Mutex<Inner>,
}
impl PlaybackStatus {
    pub fn new(path: impl Into<PathBuf>, target: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            target: target.into(),
            inner: Mutex::new(Inner {
                current: Projection {
                    video: VideoState::Stopped,
                    audio: AudioState::Stopped,
                    video_detail: "Playback stopped.".into(),
                    audio_detail: String::new(),
                    connector: Connector::none(),
                },
                published: None,
                sequence: 0,
            }),
        }
    }
    pub fn video(
        &self,
        state: VideoState,
        detail: &str,
        connector: &Connector,
    ) -> io::Result<bool> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| io::Error::other("status lock poisoned"))?;
        if inner.current.video != state || inner.current.connector != *connector {
            inner.current.video = state;
            inner.current.video_detail = sanitize_detail(detail);
            inner.current.connector = connector.clone();
        } else if inner.current.video_detail != detail {
            let sanitized = sanitize_detail(detail);
            if inner.current.video_detail != sanitized {
                inner.current.video_detail = sanitized;
            }
        }
        self.publish(&mut inner, false)
    }
    pub fn audio(
        &self,
        state: AudioState,
        detail: &str,
        connector: &Connector,
    ) -> io::Result<bool> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| io::Error::other("status lock poisoned"))?;
        if inner.current.audio != state || inner.current.connector != *connector {
            inner.current.audio = state;
            inner.current.audio_detail = sanitize_detail(detail);
            inner.current.connector = connector.clone();
        } else if inner.current.audio_detail != detail {
            let sanitized = sanitize_detail(detail);
            if inner.current.audio_detail != sanitized {
                inner.current.audio_detail = sanitized;
            }
        }
        self.publish(&mut inner, false)
    }
    pub fn heartbeat(&self, connector: &Connector) -> io::Result<bool> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| io::Error::other("status lock poisoned"))?;
        if inner.current.connector != *connector {
            inner.current.connector = connector.clone();
        }
        self.publish(&mut inner, false)
    }
    pub fn stopped(&self, detail: &str) -> io::Result<bool> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| io::Error::other("status lock poisoned"))?;
        inner.current.video = VideoState::Stopped;
        inner.current.audio = AudioState::Stopped;
        inner.current.video_detail = sanitize_detail(detail);
        inner.current.audio_detail.clear();
        inner.current.connector = Connector::none();
        self.publish(&mut inner, true)
    }
    fn publish(&self, inner: &mut Inner, force: bool) -> io::Result<bool> {
        let unchanged = inner
            .published
            .as_ref()
            .is_some_and(|p| same(&p.projection, &inner.current));
        if !force
            && unchanged
            && inner
                .published
                .as_ref()
                .is_some_and(|p| p.at.elapsed() < HEARTBEAT)
        {
            return Ok(false);
        }
        let (state, detail) = if inner.current.video == VideoState::Running
            && inner.current.audio == AudioState::Failed
        {
            (
                "degraded",
                if inner.current.audio_detail.is_empty() {
                    "Video is playing but audio is unavailable."
                } else {
                    &inner.current.audio_detail
                },
            )
        } else {
            (
                video_name(inner.current.video),
                inner.current.video_detail.as_str(),
            )
        };
        let document = StatusDocument {
            schema: 1,
            state,
            video_state: inner.current.video,
            audio_state: inner.current.audio,
            target: &self.target,
            detail,
            connector: &inner.current.connector.name,
            drm_device: &inner.current.connector.drm_device,
            alsa_device: &inner.current.connector.alsa_device,
            updated_at: timestamp(),
        };
        let bytes = serde_json::to_vec(&document).map_err(io::Error::other)?;
        atomic_replace(&self.path, &bytes, inner.sequence)?;
        inner.sequence = inner.sequence.wrapping_add(1);
        inner.published = Some(Published {
            projection: inner.current.clone(),
            at: Instant::now(),
        });
        Ok(true)
    }
}

fn same(a: &Projection, b: &Projection) -> bool {
    a.video == b.video
        && a.audio == b.audio
        && a.video_detail == b.video_detail
        && a.audio_detail == b.audio_detail
        && a.connector == b.connector
}
fn video_name(value: VideoState) -> &'static str {
    match value {
        VideoState::Running => "running",
        VideoState::WaitingForDiscovery => "waiting-for-discovery",
        VideoState::WaitingForHdmi => "waiting-for-hdmi",
        VideoState::Retrying => "retrying",
        VideoState::UnsupportedFormat => "unsupported-format",
        VideoState::Starting => "starting",
        VideoState::Stopped => "stopped",
        VideoState::Failed => "failed",
    }
}
fn timestamp() -> String {
    format_timestamp(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default(),
    )
}

/// Formats a Unix duration as the RFC 3339 instant the status contract carries.
///
/// The conversion is the civil-from-days algorithm rather than a date library:
/// the receiver's dependency closure is audited, and this is the only date
/// arithmetic in it. `format_timestamp_contract` pins it, because the Web
/// consumer rejects a record whose timestamp is stale or future-dated, so a
/// wrong calendar here reads as a receiver that has stopped publishing.
fn format_timestamp(duration: Duration) -> String {
    let seconds = i64::try_from(duration.as_secs()).unwrap_or(i64::MAX);
    let days = seconds / 86_400;
    let day_seconds = seconds % 86_400;
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    let hour = day_seconds / 3600;
    let minute = day_seconds % 3600 / 60;
    let second = day_seconds % 60;
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{:03}Z",
        duration.subsec_millis()
    )
}
fn atomic_replace(path: &Path, bytes: &[u8], sequence: u64) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let stage = parent.join(format!(
        ".omt-status.{}.{sequence:016x}",
        std::process::id()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&stage)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&stage, path)?;
    if let Ok(directory) = std::fs::File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

pub fn sanitize_detail(value: &str) -> String {
    let mut out = String::with_capacity(value.len().min(DETAIL_LIMIT));
    for c in value.chars() {
        if !c.is_control() && out.len() + c.len_utf8() <= DETAIL_LIMIT {
            out.push(c);
        }
    }
    out.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    #[test]
    fn detail_contract() {
        assert_eq!(sanitize_detail("  a\nb  "), "ab");
        assert_eq!(sanitize_detail(&"x".repeat(4096)).len(), DETAIL_LIMIT);
    }
    #[test]
    fn format_timestamp_contract() {
        for (seconds, millis, expected) in [
            (0_u64, 0_u32, "1970-01-01T00:00:00.000Z"),
            (1, 7, "1970-01-01T00:00:01.007Z"),
            (86_399, 999, "1970-01-01T23:59:59.999Z"),
            (86_400, 0, "1970-01-02T00:00:00.000Z"),
            // 2000 is a leap year and 2100 is not: the two cases the
            // hand-written era arithmetic exists to get right.
            (951_782_400, 0, "2000-02-29T00:00:00.000Z"),
            (4_107_456_000, 0, "2100-02-28T00:00:00.000Z"),
            (4_107_456_000 + 86_400, 0, "2100-03-01T00:00:00.000Z"),
            // A representative present-day instant and a 32-bit rollover.
            (1_767_225_600, 250, "2026-01-01T00:00:00.250Z"),
            (2_147_483_648, 0, "2038-01-19T03:14:08.000Z"),
        ] {
            assert_eq!(
                format_timestamp(Duration::new(seconds, millis * 1_000_000)),
                expected,
                "{seconds}s + {millis}ms"
            );
        }
    }
    #[test]
    fn published_timestamps_parse_as_the_web_consumer_reads_them() {
        // The consumer accepts `...Z` and compares against the current time, so
        // a published record has to be fixed width and describe now.
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let published = format_timestamp(now);
        assert_eq!(published.len(), 24, "{published}");
        assert!(published.ends_with('Z'), "{published}");
        assert_eq!(&published[4..5], "-", "{published}");
        assert_eq!(&published[10..11], "T", "{published}");
    }
    #[derive(Deserialize)]
    struct Vectors {
        projections: Vec<ProjectionVector>,
    }
    #[derive(Deserialize)]
    struct ProjectionVector {
        name: String,
        events: Vec<String>,
        state: String,
        video_state: String,
        audio_state: String,
    }
    #[test]
    fn shared_status_vectors() {
        let vectors: Vectors = serde_json::from_str(include_str!(
            "../../../tests/schema/playback-status-vectors.json"
        ))
        .unwrap_or_else(|e| panic!("{e}"));
        for (index, vector) in vectors.projections.into_iter().enumerate() {
            let directory = std::env::temp_dir()
                .join(format!("omt-rust-status-{}-{index}", std::process::id()));
            let path = directory.join("status.json");
            let status = PlaybackStatus::new(&path, "Camera");
            let connector = Connector::none();
            for event in vector.events {
                match event.as_str() {
                    "AudioRunning" => {
                        let _ = status.audio(AudioState::Running, "", &connector);
                    }
                    "AudioFailed" => {
                        let _ = status.audio(AudioState::Failed, "Audio unavailable", &connector);
                    }
                    "AudioStopped" => {
                        let _ = status.audio(AudioState::Stopped, "", &connector);
                    }
                    "VideoStarting" => {
                        let _ = status.video(VideoState::Starting, "", &connector);
                    }
                    "WaitingForDiscovery" => {
                        let _ = status.video(VideoState::WaitingForDiscovery, "", &connector);
                    }
                    "WaitingForHdmi" => {
                        let _ = status.video(VideoState::WaitingForHdmi, "", &connector);
                    }
                    "VideoRetrying" => {
                        let _ = status.video(VideoState::Retrying, "", &connector);
                    }
                    "UnsupportedFormat" => {
                        let _ = status.video(VideoState::UnsupportedFormat, "", &connector);
                    }
                    "VideoRunning" => {
                        let _ = status.video(VideoState::Running, "", &connector);
                    }
                    "Stopped" => {
                        let _ = status.stopped("");
                    }
                    other => panic!("unknown event {other}"),
                }
            }
            if !path.exists() {
                let _ = status.stopped("Playback stopped.");
            }
            let document: serde_json::Value = serde_json::from_slice(
                &std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", vector.name)),
            )
            .unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(document["state"], vector.state, "{}", vector.name);
            assert_eq!(
                document["video_state"], vector.video_state,
                "{}",
                vector.name
            );
            assert_eq!(
                document["audio_state"], vector.audio_state,
                "{}",
                vector.name
            );
            let keys: std::collections::BTreeSet<&str> = document
                .as_object()
                .unwrap_or_else(|| panic!("{}: not an object", vector.name))
                .keys()
                .map(String::as_str)
                .collect();
            let expected: std::collections::BTreeSet<&str> = vectors_fields().into_iter().collect();
            assert_eq!(keys, expected, "{}", vector.name);
            let _ = std::fs::remove_dir_all(directory);
        }
    }

    #[derive(Deserialize)]
    struct FieldVectors {
        fields: Vec<String>,
    }

    fn vectors_fields() -> Vec<&'static str> {
        let vectors: FieldVectors = serde_json::from_str(include_str!(
            "../../../tests/schema/playback-status-vectors.json"
        ))
        .unwrap_or_else(|e| panic!("{e}"));
        // Leak is fine in tests; pinning the contract is what matters.
        vectors
            .fields
            .into_iter()
            .map(|field| Box::leak(field.into_boxed_str()) as &'static str)
            .collect()
    }

    #[test]
    fn heartbeat_republishes_after_the_interval_without_changing_state() {
        let directory =
            std::env::temp_dir().join(format!("omt-rust-heartbeat-{}", std::process::id()));
        let path = directory.join("status.json");
        let status = PlaybackStatus::new(&path, "Camera");
        let connector = Connector::none();
        assert!(
            status
                .video(VideoState::Running, "Playing OMT video.", &connector)
                .unwrap_or_else(|e| panic!("{e}"))
        );
        assert!(
            !status
                .heartbeat(&connector)
                .unwrap_or_else(|e| panic!("{e}")),
            "unchanged heartbeat inside the interval must not rewrite"
        );
        std::thread::sleep(HEARTBEAT + std::time::Duration::from_millis(20));
        assert!(
            status
                .heartbeat(&connector)
                .unwrap_or_else(|e| panic!("{e}")),
            "heartbeat after the interval must rewrite"
        );
        let document: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap_or_else(|e| panic!("{e}")))
                .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(document["video_state"], "running");
        assert_eq!(document["state"], "running");
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn unchanged_video_skips_detail_allocation_path() {
        let directory =
            std::env::temp_dir().join(format!("omt-rust-status-same-{}", std::process::id()));
        let path = directory.join("status.json");
        let status = PlaybackStatus::new(&path, "Camera");
        let connector = Connector::none();
        assert!(
            status
                .video(VideoState::Running, "Playing OMT video.", &connector)
                .unwrap_or_else(|e| panic!("{e}"))
        );
        assert!(
            !status
                .video(VideoState::Running, "Playing OMT video.", &connector)
                .unwrap_or_else(|e| panic!("{e}"))
        );
        let _ = std::fs::remove_dir_all(directory);
    }
}
