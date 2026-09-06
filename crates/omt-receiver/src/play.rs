// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// The playback supervisor. It owns the outer retry loop, the video session,
// and the bounded-stack audio worker, and it is the only place that decides
// which playback state the status document reports.

use crate::audio;
use crate::channel::{Channel, Endpoint, remaining};
use crate::connector::{self, Connector};
use crate::discovery;
use crate::jitter::{self, FillGate, Queue};
use crate::video::{self, Present};
use omt_protocol::FrameType;
use omt_receiver_core::{AudioState, PlaybackStatus, VideoCeiling, VideoState, sanitize_detail};
use std::fmt::Write;
use std::io::ErrorKind;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// Bounded stack for the audio worker, matching the media-worker budget.
const AUDIO_STACK: usize = 128 * 1024;
/// How long a session waits for its first frame before saying so.
const MEDIA_GRACE: Duration = Duration::from_secs(5);
/// How long a session tolerates a *connected* socket that delivers nothing
/// before it rebuilds.
///
/// [`MEDIA_GRACE`] only decides when the operator is told frames are missing;
/// it ends nothing. A TCP socket can stay ESTABLISHED indefinitely with a peer
/// that is gone -- a firewall or NAT that drops the flow mid-stream, an access
/// point that forgets the association, a sender whose reset never reaches this
/// end -- and no read on it ever returns an error, so nothing here observes a
/// close. The in-session reconnect budget cannot cover that case: it is armed
/// only when the channel reports itself disconnected. Without this bound such a
/// session waits forever on a socket that will never deliver again, reporting
/// "retrying" while never reaching the outer loop, which is the only place that
/// re-resolves discovery.
///
/// Three grace periods, because it has to mean the flow is gone rather than
/// that the link is having a bad moment: at 30 fps this is some 450 missed
/// frames, far past any burst gap a working link produces. The picture is
/// already frozen by then, so waiting longer buys nothing.
const MEDIA_STALL: Duration = Duration::from_secs(15);
/// How often a running session re-checks the display for a hotplug.
const CONNECTOR_POLL: Duration = Duration::from_millis(500);
/// Per-receive slice, which is also the heartbeat cadence while idle.
const RECEIVE_SLICE: Duration = Duration::from_millis(500);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const RESOLVE_TIMEOUT: Duration = Duration::from_millis(1500);
/// Consecutive in-session video reconnects before the session is rebuilt.
///
/// Recovering the video socket alone is what keeps the picture on screen and
/// audio unbroken across a blip, so it is worth a few tries. It is not worth
/// more: a sender that keeps dropping the socket needs the outer loop, which is
/// the only place that backs off and asks discovery where the source is now.
///
/// How much wall clock those tries buy depends entirely on how the endpoint
/// fails, and the two ends of that range are far apart. See [`RECOVER_BACKOFF`]
/// for the short end, which is the one that decides what this rides out.
const RECOVER_ATTEMPTS: u32 = 3;
/// Added before each attempt after the first. A peer that accepts and then
/// immediately closes fails a reconnect as fast as the kernel can complete the
/// handshake, and without this that is a spin on a core and a flood of
/// connections at the sender.
///
/// This also sets the floor on the whole budget, and the floor is what a
/// restarting sender actually meets. A closed port answers with a reset rather
/// than timing out, so all [`RECOVER_ATTEMPTS`] attempts cost nothing but these
/// backoffs: 0 + 250 + 500 ms. Measured on a Pi 4, a sender killed at t=0
/// reported the exhausted budget at t=831 ms. So the picture is held for
/// roughly eight-tenths of a second against a sender whose port is shut, and a
/// sender that takes longer than that to start listening again gets a session
/// rebuild instead of a reconnect -- deliberately, because holding longer means
/// holding a frozen frame longer, which is the cost this budget exists to cap.
/// Raising it is not a free improvement; it trades a blink for a freeze.
const RECOVER_BACKOFF: Duration = Duration::from_millis(250);
/// How long one in-session reconnect waits, against [`CONNECT_TIMEOUT`] for the
/// first connect of a session.
///
/// This one races a frozen picture: the endpoint was reachable moments ago, so
/// a handshake that has not completed in this long is a link problem. It sets
/// the *ceiling* on the budget rather than the typical cost, and only a
/// blackholed SYN reaches it: three attempts that each wait the full timeout,
/// plus the backoffs, is under four seconds of held frame.
const RECOVER_TIMEOUT: Duration = Duration::from_secs(1);
/// After one A/V queue has filled, wait this long for the other before
/// starting alone. Missing audio must not block HDMI.
const PEER_WAIT: Duration = Duration::from_secs(1);
/// Tiny receive slice used to copy frames already in the kernel buffer
/// without waiting, so a present deadline is not starved.
const DRAIN_SLICE: Duration = Duration::from_millis(1);

pub struct Options {
    pub target: String,
    pub preference: String,
    pub retry: Duration,
    /// What this board is allowed to attempt, from the installer's board
    /// profile or the operator's override.
    pub ceiling: VideoCeiling,
    /// Operator playout delay. Zero is the official low-latency profile;
    /// four seconds is the Wi-Fi default.
    pub playout_delay: Duration,
}

/// Runs until `stop` is raised by a signal, then reports a stopped document.
pub fn run(options: &Options, status: &Arc<PlaybackStatus>, stop: &Arc<AtomicBool>) {
    let direct = options.target.starts_with("omt://");
    let mut status_failed = false;
    while !stop.load(Ordering::Relaxed) {
        if !direct && !discovery::transport_available() {
            note_status(
                &mut status_failed,
                status.video(
                    VideoState::WaitingForDiscovery,
                    "No configured OMT discovery transport is available.",
                    &omt_receiver_core::Connector::none(),
                ),
            );
            wait(
                Duration::from_secs(1),
                status,
                None,
                stop,
                &mut status_failed,
            );
            continue;
        }
        let Some(connector) = connector::find(&options.preference) else {
            note_status(
                &mut status_failed,
                status.video(
                    VideoState::WaitingForHdmi,
                    "No supported HDMI display is connected.",
                    &omt_receiver_core::Connector::none(),
                ),
            );
            wait(
                Duration::from_secs(1),
                status,
                None,
                stop,
                &mut status_failed,
            );
            continue;
        };
        if let Err(error) = session(options, &connector, status, stop, &mut status_failed)
            && !stop.load(Ordering::Relaxed)
        {
            note_status(
                &mut status_failed,
                status.video(
                    VideoState::Retrying,
                    &sanitize_detail(&error),
                    &connector.describe(),
                ),
            );
            wait(
                options.retry,
                status,
                Some(&connector),
                stop,
                &mut status_failed,
            );
        }
    }
    note_status(&mut status_failed, status.stopped("Playback stopped."));
}

fn session(
    options: &Options,
    connector: &Connector,
    status: &Arc<PlaybackStatus>,
    stop: &Arc<AtomicBool>,
    status_failed: &mut bool,
) -> Result<(), String> {
    let endpoint = discovery::resolve(&options.target, RESOLVE_TIMEOUT)
        .ok_or_else(|| "OMT target was not discovered.".to_owned())?;
    let mut output =
        video::Output::open(&connector.card_path, connector.id, options.ceiling.clone())?;
    let mut video = Channel::new();
    video
        .connect(
            &endpoint,
            FrameType::Video,
            Instant::now() + CONNECT_TIMEOUT,
        )
        .map_err(|error| error.to_string())?;

    let gate = FillGate::new();
    let audio = AudioWorker::start(
        &endpoint,
        connector,
        status,
        stop,
        options.playout_delay,
        Arc::clone(&gate),
    );
    let described = connector.describe();
    let fill_detail = if options.playout_delay.is_zero() {
        "Waiting for OMT media.".to_owned()
    } else {
        format!(
            "Waiting for {} second playout buffer.",
            options.playout_delay.as_secs()
        )
    };
    note_status(
        status_failed,
        status.video(VideoState::Starting, &fill_detail, &described),
    );

    // A fresh connection gets the same grace as one that has been delivering,
    // so "starting" stays visible while the sender ramps up.
    let mut last_frame = Instant::now() + MEDIA_GRACE;
    let mut stalled = Instant::now() + MEDIA_STALL;
    let mut next_connector_check = Instant::now();
    let mut failure = None;
    let mut reconnects = 0_u64;
    let mut skipped = 0_u64;
    let mut underruns = 0_u64;
    let mut attempts = 0_u32;
    let mut running = RunningDetail::default();
    let mut queue = Queue::video(options.playout_delay);
    let mut playing = false;
    let mut next_due = Instant::now();
    let mut filled_at = None;
    let mut frame_interval = None;

    while !stop.load(Ordering::Relaxed) {
        if Instant::now() >= next_connector_check {
            next_connector_check = Instant::now() + CONNECTOR_POLL;
            if !connector.is_connected() {
                failure = Some("HDMI display disconnected.".to_owned());
                break;
            }
        }

        let now = Instant::now();
        let present_due = playing && now >= next_due;
        let wait_until = if present_due {
            now
        } else if playing {
            next_due.min(now + RECEIVE_SLICE)
        } else {
            now + RECEIVE_SLICE
        };

        match drain_video(&mut video, &mut queue, playing, wait_until) {
            Ok(received) => {
                if received {
                    attempts = 0;
                    last_frame = Instant::now() + MEDIA_GRACE;
                    stalled = Instant::now() + MEDIA_STALL;
                }
            }
            Err(error) => {
                note_status(status_failed, status.heartbeat(&described));
                if !video.connected() {
                    if attempts >= RECOVER_ATTEMPTS {
                        failure = Some(format!(
                            "{error}; {attempts} in-session video reconnects did not hold."
                        ));
                        break;
                    }
                    note_status(
                        status_failed,
                        status.video(VideoState::Retrying, &sanitize_detail(&error), &described),
                    );
                    wait(
                        RECOVER_BACKOFF * attempts,
                        status,
                        Some(connector),
                        stop,
                        status_failed,
                    );
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    attempts += 1;
                    if let Err(error) = recover_video(&mut video, &endpoint) {
                        if attempts >= RECOVER_ATTEMPTS {
                            failure = Some(format!(
                                "{error}; {attempts} in-session video reconnects did not hold."
                            ));
                            break;
                        }
                        continue;
                    }
                    reconnects = reconnects.saturating_add(1);
                    last_frame = Instant::now() + MEDIA_GRACE;
                    stalled = Instant::now() + MEDIA_STALL;
                    continue;
                }
            }
        }

        if queue.filled() {
            gate.set_video(true);
            let ready_since = filled_at.get_or_insert_with(Instant::now);
            let peer_ok = options.playout_delay.is_zero() || gate.audio_ready();
            let waited = Instant::now() >= *ready_since + PEER_WAIT;
            if !playing && (peer_ok || waited) {
                playing = true;
                next_due = Instant::now();
                if options.playout_delay.is_zero() {
                    queue.keep_latest_only();
                }
            }
        }

        if playing && Instant::now() >= next_due {
            if let Some(frame) = queue.pop() {
                if let Some(header) = frame.video.as_ref() {
                    frame_interval = jitter::video_interval(header).or(frame_interval);
                }
                let interlaced = frame.video.as_ref().is_some_and(|v| v.flags & 1 != 0);
                match output.present(&frame) {
                    outcome @ (Present::Presented | Present::Skipped) => {
                        if outcome == Present::Skipped {
                            skipped = skipped.saturating_add(1);
                        }
                        next_due = next_due
                            .checked_add(frame_interval.unwrap_or(Duration::from_millis(33)))
                            .unwrap_or(next_due);
                        let base = output.presentation_detail(interlaced);
                        note_status(
                            status_failed,
                            status.video(
                                VideoState::Running,
                                running.detail(
                                    base,
                                    options.playout_delay,
                                    queue.duration(),
                                    reconnects,
                                    skipped,
                                    underruns,
                                ),
                                &described,
                            ),
                        );
                    }
                    Present::UnsupportedFormat(detail) => {
                        note_status(
                            status_failed,
                            status.video(VideoState::UnsupportedFormat, &detail, &described),
                        );
                    }
                    Present::Failed(detail) => {
                        failure = Some(detail);
                        break;
                    }
                }
            } else {
                underruns = underruns.saturating_add(1);
                playing = false;
                filled_at = None;
                gate.set_video(false);
                note_status(
                    status_failed,
                    status.video(
                        VideoState::Running,
                        running.detail(
                            output.presentation_detail(false),
                            options.playout_delay,
                            Duration::ZERO,
                            reconnects,
                            skipped,
                            underruns,
                        ),
                        &described,
                    ),
                );
            }
        }

        if queue.is_empty() && Instant::now() >= stalled {
            failure = Some(format!(
                "No video frames for {} seconds on a connected socket.",
                MEDIA_STALL.as_secs()
            ));
            break;
        }
        if queue.is_empty() && !playing && Instant::now() >= last_frame {
            note_status(
                status_failed,
                status.video(
                    VideoState::Retrying,
                    "Waiting for video frames.",
                    &described,
                ),
            );
        }
    }

    audio.stop();
    note_status(
        status_failed,
        status.audio(AudioState::Stopped, "", &described),
    );
    failure.map_or(Ok(()), Err)
}

/// Copies every complete video frame the socket will give without stalling the
/// accept path, then waits until `wait_until` for the next one if a present is
/// not already due.
fn drain_video(
    channel: &mut Channel,
    queue: &mut Queue,
    playing: bool,
    wait_until: Instant,
) -> Result<bool, String> {
    let mut received = false;
    loop {
        match channel.receive(Instant::now() + DRAIN_SLICE) {
            Ok(frame) => {
                if frame.header.frame_type == FrameType::Video {
                    queue.push(channel.take_frame(), playing);
                    received = true;
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
            Err(error) => return Err(error.to_string()),
        }
    }
    if remaining(wait_until).is_zero() {
        return Ok(received);
    }
    match channel.receive(wait_until) {
        Ok(frame) => {
            if frame.header.frame_type == FrameType::Video {
                queue.push(channel.take_frame(), playing);
                received = true;
            }
            loop {
                match channel.receive(Instant::now() + DRAIN_SLICE) {
                    Ok(frame) => {
                        if frame.header.frame_type == FrameType::Video {
                            queue.push(channel.take_frame(), playing);
                            received = true;
                        }
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                    Err(error) => return Err(error.to_string()),
                }
            }
            Ok(received)
        }
        Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(received),
        Err(error) => Err(error.to_string()),
    }
}

fn drain_audio(
    channel: &mut Channel,
    queue: &mut Queue,
    playing: bool,
    wait_until: Instant,
) -> Result<bool, String> {
    let mut received = false;
    loop {
        match channel.receive(Instant::now() + DRAIN_SLICE) {
            Ok(frame) => {
                if frame.header.frame_type == FrameType::Audio {
                    queue.push(channel.take_frame(), playing);
                    received = true;
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
            Err(error) => return Err(error.to_string()),
        }
    }
    if remaining(wait_until).is_zero() {
        return Ok(received);
    }
    match channel.receive(wait_until) {
        Ok(frame) => {
            if frame.header.frame_type == FrameType::Audio {
                queue.push(channel.take_frame(), playing);
                received = true;
            }
            loop {
                match channel.receive(Instant::now() + DRAIN_SLICE) {
                    Ok(frame) => {
                        if frame.header.frame_type == FrameType::Audio {
                            queue.push(channel.take_frame(), playing);
                            received = true;
                        }
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                    Err(error) => return Err(error.to_string()),
                }
            }
            Ok(received)
        }
        Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(received),
        Err(error) => Err(error.to_string()),
    }
}

/// One in-session reconnect to the endpoint this session already resolved.
///
/// Success keeps the DRM configuration, the picture on screen, and the audio
/// worker; the sender resumes at its live point and VMX carries no inter-frame
/// prediction, so there is nothing to wait for and nothing to re-sync. Failure,
/// or a run of them, returns to the outer loop so discovery can follow a source
/// that has moved. `WouldBlock` never reaches here: an idle socket whose polling
/// slice expired is still connected.
fn recover_video(channel: &mut Channel, endpoint: &Endpoint) -> Result<(), String> {
    channel
        .connect(endpoint, FrameType::Video, Instant::now() + RECOVER_TIMEOUT)
        .map_err(|error| error.to_string())
}

/// The running message, with this session's delay, buffer, and counts.
///
/// The playback loop republishes the running state for every frame it displays,
/// so this is rebuilt only when a count moves, the delay or buffered seconds
/// change, or the presenter's sentence changes: a healthy session formats one
/// string until something it names moves.
#[derive(Default)]
struct RunningDetail {
    delay_secs: u64,
    buffered_secs: u64,
    reconnects: u64,
    skipped: u64,
    underruns: u64,
    base: String,
    text: String,
}

impl RunningDetail {
    fn detail(
        &mut self,
        base: &str,
        delay: Duration,
        buffered: Duration,
        reconnects: u64,
        skipped: u64,
        underruns: u64,
    ) -> &str {
        let delay_secs = delay.as_secs();
        let buffered_secs = buffered.as_secs();
        if self.reconnects != reconnects
            || self.skipped != skipped
            || self.underruns != underruns
            || self.delay_secs != delay_secs
            || self.buffered_secs != buffered_secs
            || self.base != base
        {
            self.reconnects = reconnects;
            self.skipped = skipped;
            self.underruns = underruns;
            self.delay_secs = delay_secs;
            self.buffered_secs = buffered_secs;
            base.clone_into(&mut self.base);
            self.text = describe_running(base, delay, buffered, reconnects, skipped, underruns);
        }
        &self.text
    }
}

fn describe_running(
    base: &str,
    delay: Duration,
    buffered: Duration,
    reconnects: u64,
    skipped: u64,
    underruns: u64,
) -> String {
    let mut text = if delay.is_zero() {
        base.to_owned()
    } else {
        format!(
            "{base} {}s playout delay (~{}s buffered).",
            delay.as_secs(),
            buffered.as_secs()
        )
    };
    if reconnects > 0 && skipped > 0 {
        let _ = write!(
            text,
            " {reconnects} video reconnect(s) and {skipped} skipped frame(s) in this session."
        );
    } else if reconnects > 0 {
        let _ = write!(text, " {reconnects} video reconnect(s) in this session.");
    } else if skipped > 0 {
        let _ = write!(text, " {skipped} skipped frame(s) in this session.");
    }
    if underruns > 0 {
        let _ = write!(text, " {underruns} buffer underrun(s) in this session.");
    }
    text
}

struct AudioWorker {
    active: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl AudioWorker {
    fn start(
        endpoint: &Endpoint,
        connector: &Connector,
        status: &Arc<PlaybackStatus>,
        stop: &Arc<AtomicBool>,
        delay: Duration,
        gate: Arc<FillGate>,
    ) -> Self {
        let active = Arc::new(AtomicBool::new(true));
        let described = connector.describe();
        let gate = Arc::clone(&gate);
        let context = AudioContext {
            endpoint: endpoint.clone(),
            device: connector.alsa_device.clone(),
            connector: described.clone(),
            status: Arc::clone(status),
            active: Arc::clone(&active),
            stop: Arc::clone(stop),
            delay,
            gate: Arc::clone(&gate),
            status_failed: AtomicBool::new(false),
        };
        let spawned = thread::Builder::new()
            .name("omt-audio".into())
            .stack_size(AUDIO_STACK)
            .spawn(move || audio_loop(&context));
        let Ok(handle) = spawned else {
            active.store(false, Ordering::Relaxed);
            gate.set_audio_missing();
            // Best-effort: the audio worker never started, so a failed publish
            // here is still the first diagnostic the operator can see.
            if let Err(error) = status.audio(
                AudioState::Failed,
                "Audio unavailable: unable to create bounded-stack worker.",
                &described,
            ) {
                eprintln!("playback status publish failed: {error}");
            }
            return Self {
                active,
                handle: None,
            };
        };
        Self {
            active,
            handle: Some(handle),
        }
    }

    fn stop(mut self) {
        self.active.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

struct AudioContext {
    endpoint: Endpoint,
    device: String,
    connector: omt_receiver_core::Connector,
    status: Arc<PlaybackStatus>,
    active: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    delay: Duration,
    gate: Arc<FillGate>,
    status_failed: AtomicBool,
}

impl AudioContext {
    fn wanted(&self) -> bool {
        self.active.load(Ordering::Relaxed) && !self.stop.load(Ordering::Relaxed)
    }

    fn note_status(&self, result: std::io::Result<bool>) {
        match result {
            Ok(_) => self.status_failed.store(false, Ordering::Relaxed),
            Err(error) if !self.status_failed.swap(true, Ordering::Relaxed) => {
                eprintln!("playback status publish failed: {error}");
            }
            Err(_) => {}
        }
    }
}

/// What the dashboard says while audio plays.
///
/// An underrun is a gap the operator heard. Reporting the count is what turns
/// "the sound is choppy" into something with a number behind it, and it
/// separates a starved sink from a link that is dropping the audio stream
/// altogether -- the two look identical from the room.
fn describe_audio(underruns: u64) -> String {
    if underruns == 0 {
        return "Playing OMT video and audio.".to_owned();
    }
    format!(
        "Playing OMT video and audio. {underruns} audio underrun(s) in this session; \
         the sender's audio is arriving later than the display consumes it."
    )
}

/// Audio runs independently of video: a failing sink degrades the session
/// rather than ending it, and the worker keeps retrying behind a backoff.
fn audio_loop(context: &AudioContext) {
    while context.wanted() {
        let mut channel = Channel::new();
        let mut output = audio::Output::new();
        let mut failure = String::new();
        match channel.connect(
            &context.endpoint,
            FrameType::Audio,
            Instant::now() + CONNECT_TIMEOUT,
        ) {
            Ok(()) => {
                let mut detail = describe_audio(0);
                let mut reported = 0_u64;
                let mut stalled = Instant::now() + MEDIA_STALL;
                let mut queue = Queue::audio(context.delay);
                let mut playing = false;
                let mut next_due = Instant::now();
                let mut filled_at = None;
                let mut frame_interval = None;
                while context.wanted() {
                    let now = Instant::now();
                    let present_due = playing && now >= next_due;
                    let wait_until = if present_due {
                        now
                    } else if playing {
                        next_due.min(now + RECEIVE_SLICE)
                    } else {
                        now + RECEIVE_SLICE
                    };
                    match drain_audio(&mut channel, &mut queue, playing, wait_until) {
                        Ok(received) => {
                            if received {
                                stalled = Instant::now() + MEDIA_STALL;
                            }
                        }
                        Err(error) => {
                            if !channel.connected() {
                                failure = error;
                                break;
                            }
                        }
                    }
                    if queue.filled() {
                        context.gate.set_audio(true);
                        let ready_since = filled_at.get_or_insert_with(Instant::now);
                        let peer_ok = context.delay.is_zero() || context.gate.video_ready();
                        let waited = Instant::now() >= *ready_since + PEER_WAIT;
                        if !playing && (peer_ok || waited) {
                            playing = true;
                            next_due = Instant::now();
                            if context.delay.is_zero() {
                                queue.keep_latest_only();
                            }
                        }
                    }
                    if playing && Instant::now() >= next_due {
                        if let Some(frame) = queue.pop() {
                            if let Some(header) = frame.audio.as_ref() {
                                frame_interval = jitter::audio_interval(header).or(frame_interval);
                            }
                            if let Err(error) = output.write(&frame, &context.device) {
                                failure = error;
                                break;
                            }
                            next_due = next_due
                                .checked_add(frame_interval.unwrap_or(Duration::from_millis(20)))
                                .unwrap_or(next_due);
                            let underruns = output.underruns();
                            if underruns != reported {
                                reported = underruns;
                                detail = describe_audio(underruns);
                            }
                            context.note_status(context.status.audio(
                                AudioState::Running,
                                &detail,
                                &context.connector,
                            ));
                        } else {
                            playing = false;
                            filled_at = None;
                            context.gate.set_audio(false);
                        }
                    }
                    if queue.is_empty() && Instant::now() >= stalled {
                        failure = format!(
                            "no audio frames for {} seconds on a connected socket",
                            MEDIA_STALL.as_secs()
                        );
                        break;
                    }
                }
            }
            Err(error) => {
                context.gate.set_audio_missing();
                failure = error.to_string();
            }
        }
        drop(output);
        if !context.wanted() {
            break;
        }
        context.note_status(context.status.audio(
            AudioState::Failed,
            &format!("Audio unavailable: {}", sanitize_detail(&failure)),
            &context.connector,
        ));
        // Back off before reconnecting so a dead sink cannot spin a core.
        for _ in 0..10 {
            if !context.wanted() {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
    context.note_status(
        context
            .status
            .audio(AudioState::Stopped, "", &context.connector),
    );
}

/// Sleeps in short slices so a signal is noticed promptly and the status file
/// keeps its heartbeat while the receiver waits.
fn wait(
    total: Duration,
    status: &Arc<PlaybackStatus>,
    connector: Option<&Connector>,
    stop: &Arc<AtomicBool>,
    status_failed: &mut bool,
) {
    let deadline = Instant::now() + total;
    while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(100).min(remaining(deadline)));
        let described =
            connector.map_or_else(omt_receiver_core::Connector::none, Connector::describe);
        note_status(status_failed, status.heartbeat(&described));
    }
}

/// Logs the first status-publish failure in each contiguous outage so a full
/// or briefly missing `/run/omt` is diagnosable without flooding stderr.
fn note_status(logged: &mut bool, result: std::io::Result<bool>) {
    match result {
        Ok(_) => *logged = false,
        Err(error) if !*logged => {
            eprintln!("playback status publish failed: {error}");
            *logged = true;
        }
        Err(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CONNECT_TIMEOUT, MEDIA_GRACE, MEDIA_STALL, RECEIVE_SLICE, RECOVER_ATTEMPTS,
        RECOVER_BACKOFF, RECOVER_TIMEOUT, RunningDetail, describe_audio, describe_running,
        note_status, recover_video,
    };
    use crate::channel::{Channel, Endpoint};
    use omt_protocol::FrameType;
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    /// A clean session must not carry a count the operator has to read past,
    /// and a session with underruns must name the number rather than only
    /// saying that playback is running.
    #[test]
    fn the_audio_detail_reports_underruns_only_when_there_are_some() {
        assert_eq!(describe_audio(0), "Playing OMT video and audio.");
        let reported = describe_audio(7);
        assert!(
            reported.starts_with("Playing OMT video and audio."),
            "{reported}"
        );
        assert!(reported.contains('7'), "{reported}");
    }

    #[test]
    fn a_success_rearms_status_failure_reporting() {
        let mut logged = false;
        note_status(&mut logged, Err(std::io::Error::other("first outage")));
        assert!(logged);
        note_status(&mut logged, Err(std::io::Error::other("same outage")));
        assert!(logged);

        note_status(&mut logged, Ok(false));
        assert!(!logged);
        note_status(&mut logged, Err(std::io::Error::other("later outage")));
        assert!(logged);
    }

    /// A clean session must read exactly as it did before there were counts to
    /// report, so the operator has nothing to read past.
    #[test]
    fn zero_running_counts_read_as_the_presenter_wrote_them() {
        let mut cache = RunningDetail::default();
        let base = "Playing OMT video.";
        assert_eq!(
            cache.detail(base, Duration::ZERO, Duration::ZERO, 0, 0, 0),
            base
        );
        assert_eq!(
            describe_running(base, Duration::ZERO, Duration::ZERO, 0, 0, 0),
            base
        );
    }

    #[test]
    fn running_detail_names_reconnects_and_skips() {
        let base = "Playing OMT video.";
        assert_eq!(
            describe_running(base, Duration::ZERO, Duration::ZERO, 2, 0, 0),
            "Playing OMT video. 2 video reconnect(s) in this session."
        );
        assert_eq!(
            describe_running(base, Duration::ZERO, Duration::ZERO, 0, 3, 0),
            "Playing OMT video. 3 skipped frame(s) in this session."
        );
        assert_eq!(
            describe_running(base, Duration::ZERO, Duration::ZERO, 2, 3, 0),
            "Playing OMT video. 2 video reconnect(s) and 3 skipped frame(s) in this session."
        );
        let scaled = "Playing OMT video. Scaled from 1920x1080 to the display's 1280x720 mode.";
        assert_eq!(
            describe_running(scaled, Duration::ZERO, Duration::ZERO, 1, 0, 0),
            "Playing OMT video. Scaled from 1920x1080 to the display's 1280x720 mode. 1 video reconnect(s) in this session."
        );
        assert!(
            describe_running(base, Duration::ZERO, Duration::ZERO, u64::MAX, 0, 0)
                .contains(&u64::MAX.to_string())
        );
        assert_eq!(u64::MAX.saturating_add(1), u64::MAX);
    }

    #[test]
    fn running_detail_names_delay_and_buffer_underruns() {
        let base = "Playing OMT video.";
        assert_eq!(
            describe_running(
                base,
                Duration::from_secs(4),
                Duration::from_secs(4),
                0,
                0,
                0
            ),
            "Playing OMT video. 4s playout delay (~4s buffered)."
        );
        let with_underrun = describe_running(
            base,
            Duration::from_secs(4),
            Duration::from_secs(1),
            0,
            0,
            2,
        );
        assert!(
            with_underrun.contains("4s playout delay"),
            "{with_underrun}"
        );
        assert!(
            with_underrun.contains("2 buffer underrun(s)"),
            "{with_underrun}"
        );
        assert!(!with_underrun.contains("skipped frame"), "{with_underrun}");
    }

    /// The loop republishes this for every displayed frame, so an unchanged
    /// message must not be rebuilt -- and a changed one must be.
    #[test]
    fn the_running_detail_is_reused_until_something_it_names_changes() {
        let mut cache = RunningDetail::default();
        let base = "Playing OMT video.";
        let ptr = cache
            .detail(base, Duration::ZERO, Duration::ZERO, 2, 3, 0)
            .as_ptr();
        assert_eq!(
            cache
                .detail(base, Duration::ZERO, Duration::ZERO, 2, 3, 0)
                .as_ptr(),
            ptr
        );

        let rebuilt = cache.detail(base, Duration::ZERO, Duration::ZERO, 2, 4, 0);
        assert_ne!(rebuilt.as_ptr(), ptr);
        assert!(rebuilt.contains("4 skipped frame(s)"), "{rebuilt}");

        // A mid-session format change moves the presenter's own sentence, and
        // the interlaced and progressive sentences are never the same string.
        let interlaced = "Playing interlaced input progressively without deinterlacing.";
        let switched = cache.detail(interlaced, Duration::ZERO, Duration::ZERO, 2, 4, 0);
        assert!(switched.starts_with(interlaced), "{switched}");
        assert!(switched.contains("4 skipped frame(s)"), "{switched}");
    }

    #[test]
    fn recover_video_resubscribes_after_the_socket_closes() {
        let listener =
            TcpListener::bind("127.0.0.1:0").unwrap_or_else(|error| panic!("bind: {error}"));
        let port = listener
            .local_addr()
            .unwrap_or_else(|error| panic!("addr: {error}"))
            .port();
        let (to_server, from_client) = mpsc::channel::<&'static str>();
        let (to_client, from_server) = mpsc::channel::<&'static str>();
        let server = thread::spawn(move || {
            let (first, _) = listener
                .accept()
                .unwrap_or_else(|error| panic!("first accept: {error}"));
            to_client
                .send("first")
                .unwrap_or_else(|error| panic!("first: {error}"));
            assert_eq!(
                from_client
                    .recv()
                    .unwrap_or_else(|error| panic!("close: {error}")),
                "close"
            );
            drop(first);
            to_client
                .send("closed")
                .unwrap_or_else(|error| panic!("closed: {error}"));
            let (_second, _) = listener
                .accept()
                .unwrap_or_else(|error| panic!("second accept: {error}"));
            to_client
                .send("second")
                .unwrap_or_else(|error| panic!("second: {error}"));
            thread::sleep(Duration::from_millis(200));
        });

        let endpoint = Endpoint {
            host: "127.0.0.1".into(),
            port,
        };
        let mut channel = Channel::new();
        channel
            .connect(
                &endpoint,
                FrameType::Video,
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap_or_else(|error| panic!("connect: {error}"));
        assert_eq!(
            from_server
                .recv()
                .unwrap_or_else(|error| panic!("first: {error}")),
            "first"
        );
        to_server
            .send("close")
            .unwrap_or_else(|error| panic!("close: {error}"));
        assert_eq!(
            from_server
                .recv()
                .unwrap_or_else(|error| panic!("closed: {error}")),
            "closed"
        );
        let error = channel.receive(Instant::now() + Duration::from_secs(1));
        assert!(error.is_err(), "peer close must fail the receive");
        assert!(
            !channel.connected(),
            "peer close must drop the video channel"
        );
        recover_video(&mut channel, &endpoint).unwrap_or_else(|error| panic!("recover: {error}"));
        assert!(channel.connected(), "recover must resubscribe");
        assert_eq!(
            from_server
                .recv()
                .unwrap_or_else(|error| panic!("second: {error}")),
            "second"
        );
        drop(channel);
        server
            .join()
            .unwrap_or_else(|_| panic!("server thread panicked"));
    }

    #[test]
    fn a_failed_in_session_reconnect_returns_err() {
        let listener =
            TcpListener::bind("127.0.0.1:0").unwrap_or_else(|error| panic!("bind: {error}"));
        let port = listener
            .local_addr()
            .unwrap_or_else(|error| panic!("addr: {error}"))
            .port();
        drop(listener);
        let endpoint = Endpoint {
            host: "127.0.0.1".into(),
            port,
        };
        let mut channel = Channel::new();
        assert!(
            recover_video(&mut channel, &endpoint).is_err(),
            "a closed listener must fail the in-session reconnect"
        );
        assert!(
            !channel.connected(),
            "a failed reconnect must leave the channel down"
        );
    }

    /// A socket that stays ESTABLISHED while its peer is gone delivers no
    /// frames and never errors, so it is the reconnect budget's blind spot: the
    /// budget is armed only by a channel that reports itself disconnected. The
    /// stall bound is what ends that session instead of waiting on it forever.
    #[test]
    fn a_stalled_session_is_bounded_and_reported_before_it_is_rebuilt() {
        assert!(
            MEDIA_STALL > MEDIA_GRACE,
            "the operator must be told frames are missing before the rebuild"
        );
        // The reconnect path owns a socket that closes; the stall bound must
        // not fire first and turn that into a session rebuild.
        let worst_case: Duration = (0..RECOVER_ATTEMPTS)
            .map(|attempt| RECOVER_TIMEOUT + RECOVER_BACKOFF * attempt)
            .sum();
        assert!(
            MEDIA_STALL > worst_case,
            "a closed socket belongs to the reconnect budget: {MEDIA_STALL:?} vs {worst_case:?}"
        );
        // Checked once per receive slice, so the bound has to be a great deal
        // more than one slice for the deadline to mean what it says.
        assert!(MEDIA_STALL > RECEIVE_SLICE * 10);
        // Still prompt: the picture is already frozen for this whole window.
        assert!(MEDIA_STALL <= Duration::from_secs(30));
    }

    /// The whole point of recovering in-session is that the picture and audio
    /// survive it, so the wait before giving up has to stay short enough that
    /// a frozen frame is not what the operator is left looking at.
    #[test]
    fn in_session_recovery_is_bounded_to_a_few_seconds() {
        let worst_case: Duration = (0..RECOVER_ATTEMPTS)
            .map(|attempt| RECOVER_TIMEOUT + RECOVER_BACKOFF * attempt)
            .sum();
        assert!(
            worst_case <= Duration::from_secs(4),
            "a held picture must not outlast a session rebuild: {worst_case:?}"
        );
        assert!(
            RECOVER_TIMEOUT < CONNECT_TIMEOUT,
            "a reconnect races a frozen picture; the first connect of a session does not"
        );

        // The floor, which is what a restarting sender actually meets: a shut
        // port answers with a reset rather than timing out, so the attempts
        // cost nothing but their backoffs. This is the number the docs quote
        // (831 ms measured on a Pi 4, this plus publish overhead), and it is
        // deliberately sub-second. Widening it is not a free improvement --
        // every endpoint that never comes back would hold a frozen picture for
        // however long this becomes -- so it is pinned rather than left to
        // drift upward one plausible-looking change at a time.
        let refused: Duration = (0..RECOVER_ATTEMPTS)
            .map(|attempt| RECOVER_BACKOFF * attempt)
            .sum();
        assert!(
            refused < Duration::from_secs(1),
            "a refused reconnect must spend the budget in under a second: {refused:?}"
        );
    }
}
