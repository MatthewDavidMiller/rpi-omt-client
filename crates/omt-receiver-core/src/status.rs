//! The playback status document the Web frontend reads. The video and audio
//! threads both report through here, so every entry point takes the lock.
//! Unchanged state is rewritten at most once per heartbeat, which is what
//! lets the consumer treat a stale timestamp as a receiver that stopped
//! publishing.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use omt_common::{Error, json, timefmt};
use omt_sys::posix::{O_DIRECTORY, O_NOFOLLOW};

pub const HEARTBEAT_MS: u64 = 500;
pub const DETAIL_LIMIT: usize = 2048;
pub const VIDEO_STATE_COUNT: usize = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoState {
    Running,
    WaitingForDiscovery,
    WaitingForHdmi,
    Retrying,
    UnsupportedFormat,
    Starting,
    Stopped,
}

impl VideoState {
    pub const ALL: [VideoState; VIDEO_STATE_COUNT] = [
        VideoState::Running,
        VideoState::WaitingForDiscovery,
        VideoState::WaitingForHdmi,
        VideoState::Retrying,
        VideoState::UnsupportedFormat,
        VideoState::Starting,
        VideoState::Stopped,
    ];

    pub fn name(self) -> &'static str {
        match self {
            VideoState::Running => "running",
            VideoState::WaitingForDiscovery => "waiting-for-discovery",
            VideoState::WaitingForHdmi => "waiting-for-hdmi",
            VideoState::Retrying => "retrying",
            VideoState::UnsupportedFormat => "unsupported-format",
            VideoState::Starting => "starting",
            VideoState::Stopped => "stopped",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioState {
    Stopped,
    Running,
    Failed,
}

impl AudioState {
    pub fn name(self) -> &'static str {
        match self {
            AudioState::Stopped => "stopped",
            AudioState::Running => "running",
            AudioState::Failed => "failed",
        }
    }
}

/// The HDMI output in use and its DRM and ALSA devices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connector {
    pub name: String,
    pub drm_device: String,
    pub alsa_device: String,
}

impl Connector {
    pub fn none() -> Self {
        Self {
            name: "none".to_owned(),
            drm_device: "none".to_owned(),
            alsa_device: "none".to_owned(),
        }
    }
}

/// Drops control characters, keeps at most [`DETAIL_LIMIT`] bytes of whole
/// characters, and trims Unicode whitespace.
pub fn sanitize_detail(value: &str) -> String {
    let mut out = String::with_capacity(value.len().min(DETAIL_LIMIT));
    for c in value.chars() {
        let cp = u32::from(c);
        let control = cp < 0x20 || (0x7F..=0x9F).contains(&cp);
        if !control && out.len() + c.len_utf8() <= DETAIL_LIMIT {
            out.push(c);
        }
    }
    let trimmed = out.trim();
    if trimmed.len() == out.len() { out } else { trimmed.to_owned() }
}

/// Status files under /run live on tmpfs and are not fsynced.
pub fn durable_status_parent(parent: &Path) -> bool {
    // Path::starts_with compares whole components.
    !parent.starts_with("/run")
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Projection {
    video: VideoState,
    audio: AudioState,
    video_detail: String,
    audio_detail: String,
    connector: Connector,
}

#[derive(Debug)]
struct Inner {
    current: Projection,
    published: Option<Projection>,
    published_at_ms: u64,
    sequence: u64,
    directory_ready: bool,
}

#[derive(Debug)]
pub struct PlaybackStatus {
    path: PathBuf,
    target: String,
    inner: Mutex<Inner>,
}

/// Which detail a report updates.
#[derive(Clone, Copy)]
enum Which {
    Video,
    Audio,
}

impl PlaybackStatus {
    pub fn new(path: impl Into<PathBuf>, target: &str) -> Self {
        Self {
            path: path.into(),
            target: target.to_owned(),
            inner: Mutex::new(Inner {
                current: Projection {
                    video: VideoState::Stopped,
                    audio: AudioState::Stopped,
                    video_detail: "Playback stopped.".to_owned(),
                    audio_detail: String::new(),
                    connector: Connector::none(),
                },
                published: None,
                published_at_ms: 0,
                sequence: 0,
                directory_ready: false,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Each report returns `Ok(true)` when the document was rewritten and
    /// `Ok(false)` when nothing changed inside the heartbeat.
    pub fn video(
        &self,
        state: VideoState,
        detail: &str,
        connector: &Connector,
    ) -> Result<bool, Error> {
        let mut inner = self.lock();
        let changed = inner.current.video != state;
        inner.current.video = state;
        update_detail(&mut inner.current, Which::Video, changed, detail, connector);
        self.publish(&mut inner, false)
    }

    pub fn audio(
        &self,
        state: AudioState,
        detail: &str,
        connector: &Connector,
    ) -> Result<bool, Error> {
        let mut inner = self.lock();
        let changed = inner.current.audio != state;
        inner.current.audio = state;
        update_detail(&mut inner.current, Which::Audio, changed, detail, connector);
        self.publish(&mut inner, false)
    }

    pub fn heartbeat(&self, connector: &Connector) -> Result<bool, Error> {
        let mut inner = self.lock();
        if inner.current.connector != *connector {
            inner.current.connector = connector.clone();
        }
        self.publish(&mut inner, false)
    }

    pub fn stopped(&self, detail: &str) -> Result<bool, Error> {
        let mut inner = self.lock();
        inner.current.video = VideoState::Stopped;
        inner.current.audio = AudioState::Stopped;
        inner.current.video_detail = sanitize_detail(detail);
        inner.current.audio_detail.clear();
        inner.current.connector = Connector::none();
        self.publish(&mut inner, true)
    }

    fn publish(&self, inner: &mut Inner, force: bool) -> Result<bool, Error> {
        let now = timefmt::now_ms();
        let unchanged = inner.published.as_ref() == Some(&inner.current);
        if !force && unchanged && now.saturating_sub(inner.published_at_ms) < HEARTBEAT_MS {
            return Ok(false);
        }
        let c = &inner.current;
        let (state, detail) = if c.video == VideoState::Running && c.audio == AudioState::Failed {
            let detail = if c.audio_detail.is_empty() {
                "Video is playing but audio is unavailable."
            } else {
                c.audio_detail.as_str()
            };
            ("degraded", detail)
        } else {
            (c.video.name(), c.video_detail.as_str())
        };
        let mut doc = String::with_capacity(512);
        doc.push_str("{\"schema\":1,\"state\":");
        json::write_string(&mut doc, state);
        doc.push_str(",\"video_state\":");
        json::write_string(&mut doc, c.video.name());
        doc.push_str(",\"audio_state\":");
        json::write_string(&mut doc, c.audio.name());
        doc.push_str(",\"target\":");
        json::write_string(&mut doc, &self.target);
        doc.push_str(",\"detail\":");
        json::write_string(&mut doc, detail);
        doc.push_str(",\"connector\":");
        json::write_string(&mut doc, &c.connector.name);
        doc.push_str(",\"drm_device\":");
        json::write_string(&mut doc, &c.connector.drm_device);
        doc.push_str(",\"alsa_device\":");
        json::write_string(&mut doc, &c.connector.alsa_device);
        doc.push_str(",\"updated_at\":");
        json::write_string(&mut doc, &timefmt::now_rfc3339_millis());
        doc.push('}');
        if doc.len() > 64 * 1024 {
            return Err(Error::new("status document too large"));
        }
        self.replace(inner, doc.as_bytes())?;
        inner.directory_ready = true;
        inner.sequence += 1;
        inner.published = Some(inner.current.clone());
        inner.published_at_ms = timefmt::now_ms();
        Ok(true)
    }

    /// Replaces the status file through a private stage and a rename. The
    /// parent is created on the first publish only; if it later disappears
    /// the stage's open fails and the caller reports it, which is the honest
    /// signal.
    fn replace(&self, inner: &Inner, doc: &[u8]) -> Result<(), Error> {
        let parent = match self.path.parent() {
            Some(p) if p.as_os_str().is_empty() => Path::new("."),
            Some(p) => p,
            None => Path::new("/"),
        };
        if !inner.directory_ready {
            omt_common::fsio::mkdir_all(parent, 0o777)?;
        }
        let stage =
            parent.join(format!(".omt-status.{}.{:016x}", std::process::id(), inner.sequence));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(O_NOFOLLOW)
            .open(&stage)?;
        let durable = durable_status_parent(parent);
        let written = (|| -> std::io::Result<()> {
            file.write_all(doc)?;
            // Per-boot status on tmpfs cannot survive a restart, so fsync
            // there is a syscall tax twice a second for nothing.
            if durable {
                file.sync_all()?;
            }
            drop(file);
            fs::rename(&stage, &self.path)
        })();
        if let Err(e) = written {
            let _ = fs::remove_file(&stage);
            return Err(e.into());
        }
        if durable
            && let Ok(dir) = OpenOptions::new().read(true).custom_flags(O_DIRECTORY).open(parent)
        {
            let _ = dir.sync_all();
        }
        Ok(())
    }
}

/// Folds one worker's detail and connector into the projection. Both workers
/// call in on every frame with a detail that is almost always the one they
/// last sent, so the unchanged case compares the raw text before
/// sanitizing.
fn update_detail(
    current: &mut Projection,
    which: Which,
    state_changed: bool,
    detail: &str,
    connector: &Connector,
) {
    let connector_changed = current.connector != *connector;
    let slot = match which {
        Which::Video => &mut current.video_detail,
        Which::Audio => &mut current.audio_detail,
    };
    if state_changed || connector_changed {
        *slot = sanitize_detail(detail);
        if connector_changed {
            current.connector = connector.clone();
        }
    } else if slot != detail {
        *slot = sanitize_detail(detail);
    }
}
