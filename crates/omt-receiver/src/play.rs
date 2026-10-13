//! The playback supervisor: the outer retry loop, the video session, and the
//! bounded-stack audio worker. It is the only place that decides which
//! playback state the status document reports.

use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use omt_common::Error;
use omt_common::timefmt::{now_ms, remaining_ms};
use omt_protocol::FrameType;
use omt_receiver_core::{
    AudioState, Connector, PlaybackStatus, VideoCeiling, VideoState, sanitize_detail,
};

use crate::audio::AudioOutput;
use crate::channel::{Channel, Endpoint, Recv};
use crate::connector::{self, Hdmi};
use crate::discovery;
use crate::handoff::{AUDIO_BYTE_CAP, Queue, QueueKind, VIDEO_BYTE_CAP};
use crate::video::{Present, VideoOutput};

pub const AUDIO_STACK: usize = 128 * 1024;
/// How long a session waits for its first frame before saying so.
pub const MEDIA_GRACE_MS: u64 = 5000;
/// How long a connected socket that delivers nothing is tolerated before
/// the session is rebuilt: three grace periods, far past any burst gap.
pub const MEDIA_STALL_MS: u64 = 15000;
pub const CONNECTOR_POLL_MS: u64 = 500;
/// Per-receive slice, which is also the heartbeat cadence while idle.
pub const RECEIVE_SLICE_MS: u64 = 500;
pub const CONNECT_TIMEOUT_MS: u64 = 3000;
pub const RESOLVE_TIMEOUT_MS: u64 = 1500;
/// In-session video reconnects before the session is rebuilt.
pub const RECOVER_ATTEMPTS: u32 = 3;
/// Added before each reconnect attempt after the first.
pub const RECOVER_BACKOFF_MS: u64 = 250;
pub const RECOVER_TIMEOUT_MS: u64 = 1000;
/// The audio worker's wake interval while the ring is at its target.
pub const AUDIO_FEED_MS: u64 = 20;
/// Receive slice used to copy frames already in the kernel buffer.
pub const DRAIN_SLICE_MS: u64 = 1;

#[derive(Clone, Debug)]
pub struct PlayOptions {
    pub target: String,
    pub preference: String,
    pub retry_ms: u64,
    pub ceiling: VideoCeiling,
}

/// Logs the first status-publish failure in each contiguous outage, so a
/// full or briefly missing /run/omt is diagnosable without flooding stderr.
#[derive(Debug, Default)]
struct StatusLog {
    logged: bool,
}

impl StatusLog {
    fn note(&mut self, result: Result<bool, Error>) {
        match result {
            Ok(_) => self.logged = false,
            Err(e) if !self.logged => {
                eprintln!("playback status publish failed: {e}");
                self.logged = true;
            }
            Err(_) => {}
        }
    }
}

fn stopped(stop: &AtomicBool) -> bool {
    stop.load(Ordering::Relaxed)
}

/// Sleeps in short slices so a signal is noticed promptly and the status
/// file keeps its heartbeat while the receiver waits.
fn wait_with_heartbeat(
    total_ms: u64,
    status: &PlaybackStatus,
    connector: Option<&Hdmi>,
    stop: &AtomicBool,
    log: &mut StatusLog,
) {
    let deadline = now_ms() + total_ms;
    let described = connector.map_or_else(Connector::none, Hdmi::describe);
    while !stopped(stop) && now_ms() < deadline {
        thread::sleep(Duration::from_millis(remaining_ms(deadline).min(100)));
        log.note(status.heartbeat(&described));
    }
}

/// The running detail with this session's counts. `skipped` counts frames
/// the decoder refused; `dropped` counts frames a newer one replaced before
/// the display could take them.
pub fn describe_running(base: &str, reconnects: u64, skipped: u64, dropped: u64) -> String {
    let mut out = base.to_owned();
    if reconnects > 0 && skipped > 0 {
        let _ = write!(
            out,
            " {reconnects} video reconnect(s) and {skipped} skipped frame(s) in this session."
        );
    } else if reconnects > 0 {
        let _ = write!(out, " {reconnects} video reconnect(s) in this session.");
    } else if skipped > 0 {
        let _ = write!(out, " {skipped} skipped frame(s) in this session.");
    }
    if dropped > 0 {
        let _ = write!(
            out,
            " {dropped} frame(s) replaced by a newer one before display in this session."
        );
    }
    out
}

/// The running detail, rebuilt only when something it names changes.
#[derive(Debug, Default)]
pub struct RunningDetail {
    key: Option<(String, u64, u64, u64)>,
    text: String,
}

impl RunningDetail {
    pub fn get(&mut self, base: &str, reconnects: u64, skipped: u64, dropped: u64) -> &str {
        let current = self.key.as_ref().is_some_and(|(b, r, s, d)| {
            b == base && *r == reconnects && *s == skipped && *d == dropped
        });
        if !current {
            self.text = describe_running(base, reconnects, skipped, dropped);
            self.key = Some((base.to_owned(), reconnects, skipped, dropped));
        }
        &self.text
    }
}

pub fn describe_audio(underruns: u64) -> String {
    if underruns == 0 {
        "Playing OMT video and audio.".to_owned()
    } else {
        format!(
            "Playing OMT video and audio. {underruns} audio underrun(s) in this session; the sender's audio is \
             arriving later than the display consumes it."
        )
    }
}

/// Hands the frame just received to the queue when it is the wanted kind,
/// giving the channel a recycled buffer to read the next one into. Frames
/// the queue drops to make way are added to `dropped`.
fn accept_frame(c: &mut Channel, q: &mut Queue, kind: FrameType, dropped: &mut u64) -> bool {
    if c.frame.frame_type() != Some(kind) {
        return false;
    }
    let frame = c.take_frame(q.take_spare());
    *dropped += q.push(frame);
    true
}

/// Reads frames already in the kernel buffer, each on the drain slice.
fn drain_ready(
    c: &mut Channel,
    q: &mut Queue,
    kind: FrameType,
    received: &mut bool,
    dropped: &mut u64,
) -> Result<(), Error> {
    loop {
        match c.receive(now_ms() + DRAIN_SLICE_MS) {
            (Recv::Ok, _) => *received |= accept_frame(c, q, kind, dropped),
            (Recv::WouldBlock, _) => return Ok(()),
            (_, e) => return Err(e.unwrap_or_else(|| Error::new("OMT receive failed"))),
        }
    }
}

/// Copies every complete frame the socket will give without stalling the
/// accept path, then waits until `wait_until` for the next one. Returns
/// whether a frame of the wanted kind arrived.
fn drain(
    c: &mut Channel,
    q: &mut Queue,
    kind: FrameType,
    wait_until: u64,
    dropped: &mut u64,
) -> Result<bool, Error> {
    let mut received = false;
    drain_ready(c, q, kind, &mut received, dropped)?;
    if remaining_ms(wait_until) == 0 {
        return Ok(received);
    }
    match c.receive(wait_until) {
        (Recv::Ok, _) => {
            received |= accept_frame(c, q, kind, dropped);
            drain_ready(c, q, kind, &mut received, dropped)?;
            Ok(received)
        }
        (Recv::WouldBlock, _) => Ok(received),
        (_, e) => Err(e.unwrap_or_else(|| Error::new("OMT receive failed"))),
    }
}

// ------------------------------------------------------------ audio worker

struct AudioContext {
    endpoint: Endpoint,
    device: String,
    connector: Connector,
    status: Arc<PlaybackStatus>,
    active: AtomicBool,
    stop: &'static AtomicBool,
}

impl AudioContext {
    fn wanted(&self) -> bool {
        self.active.load(Ordering::Relaxed) && !stopped(self.stop)
    }
}

/// One connection's worth of audio; returns why it ended.
fn audio_session(ctx: &AudioContext, log: &mut StatusLog) -> Error {
    let mut channel = Channel::new();
    if let Err(e) = channel.connect(&ctx.endpoint, FrameType::Audio, now_ms() + CONNECT_TIMEOUT_MS)
    {
        return e;
    }
    let mut output = AudioOutput::new();
    let mut detail = describe_audio(0);
    let mut reported = 0;
    let mut stall_at = now_ms() + MEDIA_STALL_MS;
    let mut queue = Queue::new(QueueKind::Audio, AUDIO_BYTE_CAP);
    let mut trimmed = 0;
    while ctx.wanted() {
        // Paced by the device, not the wall clock: while audio is queued and
        // the ring is full, wake once a period.
        let wait_until = now_ms() + if queue.is_empty() { RECEIVE_SLICE_MS } else { AUDIO_FEED_MS };
        match drain(&mut channel, &mut queue, FrameType::Audio, wait_until, &mut trimmed) {
            Ok(true) => stall_at = now_ms() + MEDIA_STALL_MS,
            Err(e) if !channel.connected() => return e,
            Ok(false) | Err(_) => {}
        }
        // An empty queue is the steady state: what arrived is already in the
        // ring. The underrun that matters is ALSA's.
        let mut written = false;
        while output.room() {
            let Some(frame) = queue.pop() else { break };
            let result = output.write(&frame, &ctx.device);
            queue.recycle(frame.buffer);
            if let Err(e) = result {
                return e;
            }
            written = true;
        }
        if output.underruns() != reported {
            reported = output.underruns();
            detail = describe_audio(reported);
        }
        if written {
            log.note(ctx.status.audio(AudioState::Running, &detail, &ctx.connector));
        }
        if queue.is_empty() && now_ms() >= stall_at {
            return Error::new(format!(
                "no audio frames for {} seconds on a connected socket",
                MEDIA_STALL_MS / 1000
            ));
        }
    }
    Error::new("")
}

/// Audio runs independently of video: a failing sink degrades the session
/// rather than ending it, and the worker retries behind a backoff.
fn audio_loop(ctx: &AudioContext) {
    let mut log = StatusLog::default();
    while ctx.wanted() {
        let failure = audio_session(ctx, &mut log);
        if !ctx.wanted() {
            break;
        }
        let detail = format!("Audio unavailable: {}", sanitize_detail(failure.message()));
        log.note(ctx.status.audio(AudioState::Failed, &detail, &ctx.connector));
        // Back off before reconnecting so a dead sink cannot spin a core.
        for _ in 0..10 {
            if !ctx.wanted() {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
    log.note(ctx.status.audio(AudioState::Stopped, "", &ctx.connector));
}

struct AudioWorker {
    ctx: Arc<AudioContext>,
    thread: Option<JoinHandle<()>>,
}

impl AudioWorker {
    fn start(
        ep: &Endpoint,
        connector: &Hdmi,
        status: &Arc<PlaybackStatus>,
        stop: &'static AtomicBool,
    ) -> Self {
        let ctx = Arc::new(AudioContext {
            endpoint: ep.clone(),
            device: connector.alsa_device.clone(),
            connector: connector.describe(),
            status: Arc::clone(status),
            active: AtomicBool::new(true),
            stop,
        });
        // Process signals stay with the main thread.
        let previous = omt_sys::posix::block_all_signals();
        let worker = Arc::clone(&ctx);
        let thread = thread::Builder::new()
            .name("omt-audio".to_owned())
            .stack_size(AUDIO_STACK)
            .spawn(move || audio_loop(&worker))
            .ok();
        omt_sys::posix::restore_signal_mask(&previous);
        if thread.is_none() {
            let r = status.audio(
                AudioState::Failed,
                "Audio unavailable: unable to create bounded-stack worker.",
                &connector.describe(),
            );
            if let Err(e) = r {
                eprintln!("playback status publish failed: {e}");
            }
        }
        Self { ctx, thread }
    }

    fn stop(&mut self) {
        self.ctx.active.store(false, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// ------------------------------------------------------------ video session

/// Returns `Ok` on a clean end (stop raised), the failure otherwise.
fn session(
    o: &PlayOptions,
    connector: &Hdmi,
    status: &Arc<PlaybackStatus>,
    stop: &'static AtomicBool,
    log: &mut StatusLog,
) -> Result<(), Error> {
    let endpoint = discovery::resolve(&o.target, RESOLVE_TIMEOUT_MS)
        .ok_or_else(|| Error::new("OMT target was not discovered."))?;
    let mut output = VideoOutput::open(&connector.card_path, connector.id, &o.ceiling)?;
    let mut video = Channel::new();
    video.connect(&endpoint, FrameType::Video, now_ms() + CONNECT_TIMEOUT_MS)?;
    let mut audio = AudioWorker::start(&endpoint, connector, status, stop);
    let described = connector.describe();
    log.note(status.video(VideoState::Starting, "Waiting for OMT media.", &described));

    let mut last_frame = now_ms() + MEDIA_GRACE_MS;
    let mut stall_at = now_ms() + MEDIA_STALL_MS;
    let mut next_connector_check = now_ms();
    let (mut reconnects, mut skipped, mut dropped) = (0_u64, 0_u64, 0_u64);
    let mut attempts = 0_u32;
    let mut running = RunningDetail::default();
    let mut queue = Queue::new(QueueKind::Video, VIDEO_BYTE_CAP);
    let mut failure: Option<Error> = None;

    // No playout clock: each frame is decoded and flipped as soon as it
    // arrives, and the display's own refresh is the only pacing. While a
    // frame is being decoded, whatever arrives behind it waits in the
    // socket; the next drain keeps only the newest of those.
    while !stopped(stop) {
        if now_ms() >= next_connector_check {
            next_connector_check = now_ms() + CONNECTOR_POLL_MS;
            if !connector.is_connected() {
                failure = Some(Error::new("HDMI display disconnected."));
                break;
            }
        }
        match drain(
            &mut video,
            &mut queue,
            FrameType::Video,
            now_ms() + RECEIVE_SLICE_MS,
            &mut dropped,
        ) {
            Ok(received) => {
                if received {
                    attempts = 0;
                    last_frame = now_ms() + MEDIA_GRACE_MS;
                    stall_at = now_ms() + MEDIA_STALL_MS;
                }
            }
            Err(err) => {
                log.note(status.heartbeat(&described));
                if !video.connected() {
                    if attempts >= RECOVER_ATTEMPTS {
                        failure = Some(Error::new(format!(
                            "{err}; {attempts} in-session video reconnects did not hold."
                        )));
                        break;
                    }
                    log.note(status.video(
                        VideoState::Retrying,
                        &sanitize_detail(err.message()),
                        &described,
                    ));
                    wait_with_heartbeat(
                        RECOVER_BACKOFF_MS * u64::from(attempts),
                        status,
                        Some(connector),
                        stop,
                        log,
                    );
                    if stopped(stop) {
                        break;
                    }
                    attempts += 1;
                    // One reconnect to the endpoint this session already
                    // resolved: success keeps the mode, the picture, and the
                    // audio worker.
                    if let Err(e) =
                        video.connect(&endpoint, FrameType::Video, now_ms() + RECOVER_TIMEOUT_MS)
                    {
                        if attempts >= RECOVER_ATTEMPTS {
                            failure = Some(Error::new(format!(
                                "{e}; {attempts} in-session video reconnects did not hold."
                            )));
                            break;
                        }
                        continue;
                    }
                    reconnects += 1;
                    last_frame = now_ms() + MEDIA_GRACE_MS;
                    stall_at = now_ms() + MEDIA_STALL_MS;
                    continue;
                }
            }
        }

        if let Some(frame) = queue.pop() {
            let interlaced = frame.video.is_some_and(|v| v.flags & 1 != 0);
            let outcome = output.present(&frame);
            queue.recycle(frame.buffer);
            match outcome {
                Present::Presented | Present::Skipped => {
                    if outcome == Present::Skipped {
                        skipped += 1;
                    }
                    let text = running.get(
                        output.presentation_detail(interlaced),
                        reconnects,
                        skipped,
                        dropped,
                    );
                    log.note(status.video(VideoState::Running, text, &described));
                }
                Present::Unsupported(detail) => {
                    log.note(status.video(VideoState::UnsupportedFormat, &detail, &described));
                }
                Present::Failed(detail) => {
                    failure = Some(Error::new(detail));
                    break;
                }
            }
        } else if now_ms() >= stall_at {
            failure = Some(Error::new(format!(
                "No video frames for {} seconds on a connected socket.",
                MEDIA_STALL_MS / 1000
            )));
            break;
        } else if now_ms() >= last_frame {
            log.note(status.video(VideoState::Retrying, "Waiting for video frames.", &described));
        }
    }

    audio.stop();
    log.note(status.audio(AudioState::Stopped, "", &described));
    failure.map_or(Ok(()), Err)
}

/// Runs until `stop` is raised, then reports a stopped document.
pub fn run(o: &PlayOptions, status: &Arc<PlaybackStatus>, stop: &'static AtomicBool) {
    let direct = o.target.starts_with("omt://");
    let mut log = StatusLog::default();
    let none = Connector::none();
    while !stopped(stop) {
        if !direct && !discovery::transport_available() {
            log.note(status.video(
                VideoState::WaitingForDiscovery,
                "No configured OMT discovery transport is available.",
                &none,
            ));
            wait_with_heartbeat(1000, status, None, stop, &mut log);
            continue;
        }
        let Some(hdmi) = connector::find(&o.preference) else {
            log.note(status.video(
                VideoState::WaitingForHdmi,
                "No supported HDMI display is connected.",
                &none,
            ));
            wait_with_heartbeat(1000, status, None, stop, &mut log);
            continue;
        };
        if let Err(failure) = session(o, &hdmi, status, stop, &mut log)
            && !stopped(stop)
        {
            log.note(status.video(
                VideoState::Retrying,
                &sanitize_detail(failure.message()),
                &hdmi.describe(),
            ));
            wait_with_heartbeat(o.retry_ms, status, Some(&hdmi), stop, &mut log);
        }
    }
    log.note(status.stopped("Playback stopped."));
}
