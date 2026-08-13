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
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AudioState {
    Stopped,
    Running,
    Failed,
}

/// The decode ceiling this board is allowed to attempt.
///
/// A ceiling is a list of shapes and a frame is admitted when it fits inside
/// any one of them, which is what lets a Pi 4 take either 1080p30 or 720p60
/// without a pixel-rate budget nobody can explain to an operator. The
/// installer derives the default from the board (`deploy/lib/board-profile.sh`)
/// and passes it as `--video-ceiling`; the operator may override it.
///
/// This is policy, not safety. `omt_protocol::parse_video_header` still refuses
/// anything above 1920x1080@60 outright, because that bound is what sizes the
/// decoder's allocations. A ceiling can only ever be at or below it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VideoCeiling {
    shapes: Vec<Shape>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Shape {
    width: i32,
    height: i32,
    fps: i32,
}

/// Absolute limits, mirroring `omt_protocol::parse_video_header`. A ceiling
/// above these would promise what the fixed allocations cannot deliver.
const CEILING_MAX_WIDTH: i32 = 1920;
const CEILING_MAX_HEIGHT: i32 = 1080;
const CEILING_MAX_FPS: i32 = 60;
const CEILING_MIN_DIMENSION: i32 = 16;
/// More than any board profile needs, which bounds the parse.
const CEILING_MAX_SHAPES: usize = 4;

impl Shape {
    fn admits(self, width: i32, height: i32, rate: f64) -> bool {
        // The rate is compared with a tolerance because 59.94 arrives as
        // 60000/1001 and must not be refused by a 60 fps ceiling.
        width <= self.width && height <= self.height && rate <= f64::from(self.fps) + 0.01
    }

    fn describe(self) -> String {
        format!("{}x{} at {} fps", self.width, self.height, self.fps)
    }
}

impl VideoCeiling {
    /// Parses `WIDTHxHEIGHT@FPS[,WIDTHxHEIGHT@FPS...]`.
    ///
    /// Every rejection is a named error rather than a fallback: the ceiling
    /// comes from the installer and from operator input, and silently
    /// substituting a default for a malformed one would present a board as
    /// capable of something nobody chose.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut shapes = Vec::new();
        for field in text.split(',') {
            if shapes.len() == CEILING_MAX_SHAPES {
                return Err(format!(
                    "A video ceiling may list at most {CEILING_MAX_SHAPES} resolutions."
                ));
            }
            shapes.push(Self::shape(field)?);
        }
        if shapes.is_empty() {
            return Err("A video ceiling must list at least one resolution.".into());
        }
        Ok(Self { shapes })
    }

    fn shape(field: &str) -> Result<Shape, String> {
        let invalid = || format!("Invalid video ceiling: {field}. Expected WIDTHxHEIGHT@FPS.");
        let (dimensions, rate) = field.split_once('@').ok_or_else(invalid)?;
        let (width, height) = dimensions.split_once('x').ok_or_else(invalid)?;
        let shape = Shape {
            width: Self::number(width).ok_or_else(invalid)?,
            height: Self::number(height).ok_or_else(invalid)?,
            fps: Self::number(rate).ok_or_else(invalid)?,
        };
        if !(CEILING_MIN_DIMENSION..=CEILING_MAX_WIDTH).contains(&shape.width)
            || !(CEILING_MIN_DIMENSION..=CEILING_MAX_HEIGHT).contains(&shape.height)
            || !(1..=CEILING_MAX_FPS).contains(&shape.fps)
        {
            return Err(format!(
                "Video ceiling {field} is outside the supported {CEILING_MAX_WIDTH}x{CEILING_MAX_HEIGHT} at {CEILING_MAX_FPS} fps maximum."
            ));
        }
        Ok(shape)
    }

    /// Digits only, so a leading `+`, a leading zero, or surrounding space is a
    /// rejection rather than something `str::parse` would quietly accept.
    fn number(text: &str) -> Option<i32> {
        if text.is_empty()
            || text.len() > 4
            || !text.bytes().all(|byte| byte.is_ascii_digit())
            || text.starts_with('0')
        {
            return None;
        }
        text.parse().ok()
    }

    /// Whether this board may attempt the format, and why not when it may not.
    ///
    /// The detail is what the operator reads on the dashboard next to
    /// `unsupported-format`, so it names both the stream and the ceiling.
    pub fn admits(&self, width: i32, height: i32, rate: f64) -> Result<(), String> {
        if self
            .shapes
            .iter()
            .any(|shape| shape.admits(width, height, rate))
        {
            return Ok(());
        }
        Err(format!(
            "{width}x{height} at {rate:.2} fps exceeds this appliance's limit of {}.",
            self.describe()
        ))
    }

    /// The ceiling as operator-facing prose, for status and the Web UI.
    #[must_use]
    pub fn describe(&self) -> String {
        let shapes: Vec<String> = self.shapes.iter().map(|shape| shape.describe()).collect();
        shapes.join(", or ")
    }
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
    /// Whether the status directory has been created for this process yet.
    directory_ready: bool,
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
                directory_ready: false,
            }),
        }
    }
    pub fn video(
        &self,
        state: VideoState,
        detail: &str,
        connector: &Connector,
    ) -> io::Result<bool> {
        let mut inner = self.lock()?;
        let changed = inner.current.video != state;
        inner.current.video = state;
        update_detail(
            &mut inner.current,
            |projection| &mut projection.video_detail,
            changed,
            detail,
            connector,
        );
        self.publish(&mut inner, false)
    }
    pub fn audio(
        &self,
        state: AudioState,
        detail: &str,
        connector: &Connector,
    ) -> io::Result<bool> {
        let mut inner = self.lock()?;
        let changed = inner.current.audio != state;
        inner.current.audio = state;
        update_detail(
            &mut inner.current,
            |projection| &mut projection.audio_detail,
            changed,
            detail,
            connector,
        );
        self.publish(&mut inner, false)
    }
    fn lock(&self) -> io::Result<std::sync::MutexGuard<'_, Inner>> {
        self.inner
            .lock()
            .map_err(|_| io::Error::other("status lock poisoned"))
    }
    pub fn heartbeat(&self, connector: &Connector) -> io::Result<bool> {
        let mut inner = self.lock()?;
        if inner.current.connector != *connector {
            inner.current.connector = connector.clone();
        }
        self.publish(&mut inner, false)
    }
    pub fn stopped(&self, detail: &str) -> io::Result<bool> {
        let mut inner = self.lock()?;
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
        atomic_replace(&self.path, &bytes, inner.sequence, inner.directory_ready)?;
        inner.directory_ready = true;
        inner.sequence = inner.sequence.wrapping_add(1);
        inner.published = Some(Published {
            projection: inner.current.clone(),
            at: Instant::now(),
        });
        Ok(true)
    }
}

/// Folds one worker's detail and connector into the projection.
///
/// The video and audio paths had this rule written out twice with the field
/// names swapped, which is how they could have drifted apart. `sanitize_detail`
/// allocates, and both workers call in on every frame with a detail that is
/// almost always the same string they last sent, so the unchanged case compares
/// the raw text first and only sanitizes when that comparison fails.
fn update_detail(
    current: &mut Projection,
    which: fn(&mut Projection) -> &mut String,
    state_changed: bool,
    detail: &str,
    connector: &Connector,
) {
    if state_changed || current.connector != *connector {
        *which(current) = sanitize_detail(detail);
        current.connector.clone_from(connector);
    } else if which(current) != detail {
        let sanitized = sanitize_detail(detail);
        if *which(current) != sanitized {
            *which(current) = sanitized;
        }
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
/// Replaces the status file through a private stage and a rename.
///
/// `directory_ready` skips `create_dir_all` after the first success. The
/// entrypoint owns `/run/omt` and creates it 0700 before the receiver starts,
/// so re-asserting the directory on every publish -- on every state change and
/// then twice a second, forever -- bought nothing. If it does disappear, the
/// stage's `open` fails and the caller reports it, which is the honest signal.
fn atomic_replace(
    path: &Path,
    bytes: &[u8],
    sequence: u64,
    directory_ready: bool,
) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if !directory_ready {
        fs::create_dir_all(parent)?;
    }
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
    // Per-boot status lives on tmpfs under `/run`. fsync there is a syscall
    // tax twice a second for state that cannot survive a restart anyway.
    if durable_status_parent(parent) {
        file.sync_all()?;
    }
    drop(file);
    fs::rename(&stage, path)?;
    if durable_status_parent(parent)
        && let Ok(directory) = std::fs::File::open(parent)
    {
        let _ = directory.sync_all();
    }
    Ok(())
}

fn durable_status_parent(parent: &Path) -> bool {
    !parent.starts_with("/run")
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

    fn ceiling(text: &str) -> VideoCeiling {
        VideoCeiling::parse(text).unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn status_on_tmpfs_skips_fsync() {
        assert!(!durable_status_parent(Path::new("/run/omt/state")));
        assert!(!durable_status_parent(Path::new("/run")));
        assert!(durable_status_parent(Path::new("/etc/omt/run")));
        assert!(durable_status_parent(Path::new("/tmp/omt")));
    }

    /// The four board tiers from `deploy/lib/board-profile.sh`. This test and
    /// `tests/unit/test_board_profile.sh` are the two ends of that contract:
    /// the shell owns which board gets which string, this owns what the string
    /// then admits.
    #[test]
    fn board_tiers_admit_their_intended_formats() {
        let pi5 = ceiling("1920x1080@60");
        let pi4 = ceiling("1920x1080@30,1280x720@60");
        let pi3 = ceiling("1280x720@60");

        assert!(pi5.admits(1920, 1080, 60.0).is_ok());
        assert!(pi5.admits(1280, 720, 60.0).is_ok());

        // The Pi 4 tier is the reason a ceiling is a list: neither shape alone
        // expresses "1080p30 or 720p60".
        assert!(pi4.admits(1920, 1080, 30.0).is_ok());
        assert!(pi4.admits(1280, 720, 60.0).is_ok());
        assert!(pi4.admits(1920, 1080, 60.0).is_err());

        assert!(pi3.admits(1280, 720, 60.0).is_ok());
        assert!(pi3.admits(1280, 720, 30.0).is_ok());
        assert!(pi3.admits(1920, 1080, 30.0).is_err());
        assert!(pi3.admits(1920, 1080, 60.0).is_err());
    }

    /// 59.94 arrives as 60000/1001 and is what most broadcast senders emit. A
    /// 60 fps ceiling that refused it would report `unsupported-format` for the
    /// single most common input on every board.
    #[test]
    fn a_sixty_fps_ceiling_admits_59_94() {
        let tier = ceiling("1280x720@60");
        assert!(tier.admits(1280, 720, 60_000.0 / 1001.0).is_ok());
        assert!(tier.admits(1280, 720, 60.0).is_ok());
        assert!(tier.admits(1280, 720, 60.5).is_err());
    }

    /// Smaller-than-ceiling input is admitted in both dimensions
    /// independently, so an unusual aspect ratio is not refused for being
    /// narrow.
    #[test]
    fn smaller_formats_fit_inside_a_shape() {
        let tier = ceiling("1920x1080@30");
        assert!(tier.admits(640, 480, 25.0).is_ok());
        assert!(tier.admits(1920, 240, 30.0).is_ok());
        assert!(tier.admits(16, 16, 1.0).is_ok());
    }

    #[test]
    fn a_refusal_names_the_stream_and_the_ceiling() {
        let Err(detail) = ceiling("1280x720@60").admits(1920, 1080, 59.94) else {
            panic!("1080p must not fit a 720p ceiling")
        };
        assert!(detail.contains("1920x1080"), "{detail}");
        assert!(detail.contains("1280x720 at 60 fps"), "{detail}");
    }

    #[test]
    fn describes_every_shape_for_the_operator() {
        assert_eq!(ceiling("1920x1080@60").describe(), "1920x1080 at 60 fps");
        assert_eq!(
            ceiling("1920x1080@30,1280x720@60").describe(),
            "1920x1080 at 30 fps, or 1280x720 at 60 fps"
        );
    }

    /// A ceiling above the protocol's own limits would promise what the fixed
    /// allocations cannot deliver, so it is refused however it is spelled.
    #[test]
    fn rejects_malformed_and_out_of_range_ceilings() {
        for text in [
            "",
            "1920x1080",
            "1920X1080@60",
            "1920x1080@60Hz",
            "1921x1080@60",
            "1920x1081@60",
            "1920x1080@61",
            "3840x2160@30",
            "15x15@60",
            "1920x1080@0",
            "0x1080@60",
            "0640x480@30",
            "+640x480@30",
            " 640x480@30",
            "640x480@30 ",
            "1920x1080@60,",
            ",1920x1080@60",
            "1920x1080@60,,1280x720@30",
            "1920x1080@60 1280x720@30",
            "12345x1080@60",
            "640x480@25,800x600@30,1280x720@50,1920x1080@30,640x360@24",
        ] {
            assert!(
                VideoCeiling::parse(text).is_err(),
                "accepted an unsupported ceiling: [{text}]"
            );
        }
    }

    #[test]
    fn accepts_every_shipped_board_profile() {
        for text in [
            "1920x1080@60",
            "1920x1080@30,1280x720@60",
            "1280x720@60",
            "640x480@25,800x600@30,1280x720@50,1920x1080@30",
        ] {
            assert!(
                VideoCeiling::parse(text).is_ok(),
                "rejected a supported ceiling: [{text}]"
            );
        }
    }

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

    /// Every state this crate can put in the `video_state` field.
    ///
    /// Listing them by hand is the point: `video_name` is a `match`, so adding
    /// a variant without adding it here fails to compile, and the case below
    /// then fails unless the published contract carries it too.
    const ALL_VIDEO_STATES: [VideoState; 7] = [
        VideoState::Running,
        VideoState::WaitingForDiscovery,
        VideoState::WaitingForHdmi,
        VideoState::Retrying,
        VideoState::UnsupportedFormat,
        VideoState::Starting,
        VideoState::Stopped,
    ];

    /// The producer's state names must be exactly the consumer's accept-list.
    ///
    /// The Python half already asserts itself against this file
    /// (`tests/unit/test_playback_failures.py`), so without this the Rust side
    /// was the one place a state could be added -- or left behind after it
    /// stopped being reachable -- without anything noticing. A name only this
    /// side knows makes every status record unparseable for the dashboard; one
    /// only the contract knows is dead weight in two languages.
    #[test]
    fn video_names_match_the_published_contract() {
        #[derive(Deserialize)]
        struct StateVectors {
            video_states: Vec<String>,
        }
        let vectors: StateVectors = serde_json::from_str(include_str!(
            "../../../tests/schema/playback-status-vectors.json"
        ))
        .unwrap_or_else(|e| panic!("{e}"));
        let published: std::collections::BTreeSet<&str> =
            vectors.video_states.iter().map(String::as_str).collect();
        let produced: std::collections::BTreeSet<&str> =
            ALL_VIDEO_STATES.into_iter().map(video_name).collect();
        assert_eq!(produced, published);
        // A `match` arm per variant is only total if no two share a name.
        assert_eq!(produced.len(), ALL_VIDEO_STATES.len());
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
