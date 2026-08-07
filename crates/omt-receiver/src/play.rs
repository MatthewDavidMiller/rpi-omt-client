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
use crate::video::{self, Present};
use omt_protocol::FrameType;
use omt_receiver_core::{AudioState, PlaybackStatus, VideoState, sanitize_detail};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// Bounded stack for the audio worker, matching the media-worker budget.
const AUDIO_STACK: usize = 512 * 1024;
/// How long a session waits for its first frame before saying so.
const MEDIA_GRACE: Duration = Duration::from_secs(5);
/// How often a running session re-checks the display for a hotplug.
const CONNECTOR_POLL: Duration = Duration::from_millis(500);
/// Per-receive slice, which is also the heartbeat cadence while idle.
const RECEIVE_SLICE: Duration = Duration::from_millis(500);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const RESOLVE_TIMEOUT: Duration = Duration::from_millis(1500);

pub struct Options {
    pub target: String,
    pub preference: String,
    pub retry: Duration,
}

/// Runs until `stop` is raised by a signal, then reports a stopped document.
pub fn run(options: &Options, status: &Arc<PlaybackStatus>, stop: &Arc<AtomicBool>) {
    let direct = options.target.starts_with("omt://");
    while !stop.load(Ordering::Relaxed) {
        if !direct && !discovery::transport_available() {
            let _ = status.video(
                VideoState::WaitingForDiscovery,
                "No configured OMT discovery transport is available.",
                &omt_receiver_core::Connector::none(),
            );
            wait(Duration::from_secs(1), status, None, stop);
            continue;
        }
        let Some(connector) = connector::find(&options.preference) else {
            let _ = status.video(
                VideoState::WaitingForHdmi,
                "No supported HDMI display is connected.",
                &omt_receiver_core::Connector::none(),
            );
            wait(Duration::from_secs(1), status, None, stop);
            continue;
        };
        if let Err(error) = session(options, &connector, status, stop)
            && !stop.load(Ordering::Relaxed)
        {
            let _ = status.video(
                VideoState::Retrying,
                &sanitize_detail(&error),
                &connector.describe(),
            );
            wait(options.retry, status, Some(&connector), stop);
        }
    }
    let _ = status.stopped("Playback stopped.");
}

fn session(
    options: &Options,
    connector: &Connector,
    status: &Arc<PlaybackStatus>,
    stop: &Arc<AtomicBool>,
) -> Result<(), String> {
    let endpoint = discovery::resolve(&options.target, RESOLVE_TIMEOUT)
        .ok_or_else(|| "OMT target was not discovered.".to_owned())?;
    let mut output = video::Output::open(&connector.card_path, connector.id)?;
    let mut video = Channel::new();
    video
        .connect(
            &endpoint,
            FrameType::Video,
            Instant::now() + CONNECT_TIMEOUT,
        )
        .map_err(|error| error.to_string())?;

    let audio = AudioWorker::start(&endpoint, connector, status, stop);
    let described = connector.describe();
    let _ = status.video(VideoState::Starting, "Waiting for OMT media.", &described);

    // A fresh connection gets the same grace as one that has been delivering,
    // so "starting" stays visible while the sender ramps up.
    let mut last_frame = Instant::now() + MEDIA_GRACE;
    let mut next_connector_check = Instant::now();
    let mut failure = None;

    while !stop.load(Ordering::Relaxed) {
        if Instant::now() >= next_connector_check {
            next_connector_check = Instant::now() + CONNECTOR_POLL;
            if !connector.is_connected() {
                failure = Some("HDMI display disconnected.".to_owned());
                break;
            }
        }
        let deadline = Instant::now() + RECEIVE_SLICE;
        let outcome = next_video_frame(&mut video, deadline);
        let frame = match outcome {
            Ok(()) => {
                // The borrow of the channel ends with each receive, so the
                // frame is re-read here for presentation.
                video.frame()
            }
            Err(error) => {
                let _ = status.heartbeat(&described);
                if !video.connected() {
                    failure = Some(error);
                    break;
                }
                if Instant::now() >= last_frame {
                    let _ = status.video(
                        VideoState::Retrying,
                        "Waiting for video frames.",
                        &described,
                    );
                }
                continue;
            }
        };
        last_frame = Instant::now() + MEDIA_GRACE;
        let interlaced = frame.video.as_ref().is_some_and(|v| v.flags & 1 != 0);
        match output.present(frame) {
            Present::Presented => {
                let _ = status.video(
                    VideoState::Running,
                    if interlaced {
                        "Playing interlaced input progressively without deinterlacing."
                    } else {
                        "Playing OMT video."
                    },
                    &described,
                );
            }
            Present::UnsupportedFormat(detail) => {
                let _ = status.video(VideoState::UnsupportedFormat, &detail, &described);
            }
            Present::Failed(detail) => {
                failure = Some(detail);
                break;
            }
        }
    }

    audio.stop();
    let _ = status.audio(AudioState::Stopped, "", &described);
    failure.map_or(Ok(()), Err)
}

/// Reads until a video frame arrives or the slice expires.
fn next_video_frame(channel: &mut Channel, deadline: Instant) -> Result<(), String> {
    while !remaining(deadline).is_zero() {
        match channel.receive(deadline) {
            Ok(frame) if frame.header.frame_type == FrameType::Video => return Ok(()),
            Ok(_) => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("OMT media deadline expired".into())
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
    ) -> Self {
        let active = Arc::new(AtomicBool::new(true));
        let described = connector.describe();
        let context = AudioContext {
            endpoint: endpoint.clone(),
            device: connector.alsa_device.clone(),
            connector: described.clone(),
            status: Arc::clone(status),
            active: Arc::clone(&active),
            stop: Arc::clone(stop),
        };
        let spawned = thread::Builder::new()
            .name("omt-audio".into())
            .stack_size(AUDIO_STACK)
            .spawn(move || audio_loop(&context));
        let Ok(handle) = spawned else {
            active.store(false, Ordering::Relaxed);
            let _ = status.audio(
                AudioState::Failed,
                "Audio unavailable: unable to create bounded-stack worker.",
                &described,
            );
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
}

impl AudioContext {
    fn wanted(&self) -> bool {
        self.active.load(Ordering::Relaxed) && !self.stop.load(Ordering::Relaxed)
    }
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
                while context.wanted() {
                    let deadline = Instant::now() + Duration::from_millis(100);
                    match channel.receive(deadline) {
                        Ok(frame) if frame.header.frame_type == FrameType::Audio => {
                            let frame = channel.frame();
                            if let Err(error) = output.write(frame, &context.device) {
                                failure = error;
                                break;
                            }
                            let _ = context.status.audio(
                                AudioState::Running,
                                "Playing OMT video and audio.",
                                &context.connector,
                            );
                        }
                        Ok(_) => {}
                        Err(error) => {
                            if !channel.connected() {
                                failure = error.to_string();
                                break;
                            }
                        }
                    }
                }
            }
            Err(error) => failure = error.to_string(),
        }
        drop(output);
        if !context.wanted() {
            break;
        }
        let _ = context.status.audio(
            AudioState::Failed,
            &format!("Audio unavailable: {}", sanitize_detail(&failure)),
            &context.connector,
        );
        // Back off before reconnecting so a dead sink cannot spin a core.
        for _ in 0..10 {
            if !context.wanted() {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
    let _ = context
        .status
        .audio(AudioState::Stopped, "", &context.connector);
}

/// Sleeps in short slices so a signal is noticed promptly and the status file
/// keeps its heartbeat while the receiver waits.
fn wait(
    total: Duration,
    status: &Arc<PlaybackStatus>,
    connector: Option<&Connector>,
    stop: &Arc<AtomicBool>,
) {
    let deadline = Instant::now() + total;
    while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(100).min(remaining(deadline)));
        let described =
            connector.map_or_else(omt_receiver_core::Connector::none, Connector::describe);
        let _ = status.heartbeat(&described);
    }
}
