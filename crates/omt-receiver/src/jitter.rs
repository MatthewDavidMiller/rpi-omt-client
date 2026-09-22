// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// Compressed A/V playout queue. TCP reads stay greedy (OMT: never stall the
// accept path); HDMI and ALSA consume this queue on a paced clock. vMix drops
// in-flight extras during a stall, so the queue is a pre-roll cushion of
// frames already accepted, not a catch-up reel.

use crate::channel::Frame;
use omt_protocol::{AudioHeader, VideoHeader};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Payload ceiling for queued compressed video. Decoded 1080p is ~8 MiB per
/// frame and cannot be stored; VMX at a fat vMix quality is around 1 MiB.
pub const VIDEO_BYTE_CAP: usize = 256 * 1024 * 1024;
/// How far past the configured delay a queue may grow before oldest frames
/// are dropped. Half a second covers a sender slightly faster than playout.
pub const TIME_SLACK: Duration = Duration::from_millis(500);
/// How far past the configured delay a *playing* audio queue may grow before
/// its oldest frames are dropped.
///
/// Video can shed frames to stay live and nobody sees it; every audio frame
/// shed is a gap the operator hears. So audio is never trimmed to the delay
/// itself, only past this bound, which is what absorbs a burst of frames that
/// arrived together after a Wi-Fi gap. The ALSA ring is fed by the device
/// clock, so in steady state this queue is empty or nearly so and the bound
/// only engages when a sender clock runs ahead of the HDMI clock or a stall
/// has left latency to shed.
pub const AUDIO_SLACK: Duration = Duration::from_millis(100);
/// Payload buffers kept for reuse. A received frame lands in one of these
/// instead of a fresh allocation, which for a 1080p VMX frame is an mmap, a
/// page fault per 4 KiB, a full zero-fill, and a munmap on every frame.
const SPARE_BUFFERS: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Video,
    Audio,
}

/// Bounded compressed-frame store paced by the announced frame or sample rate.
pub struct Queue {
    frames: VecDeque<Frame>,
    bytes: usize,
    delay: Duration,
    byte_cap: usize,
    interval: Option<Duration>,
    kind: Kind,
    spare: Vec<Vec<u8>>,
}

impl Queue {
    /// Video queue: 256 MiB payload cap and the operator's delay.
    #[must_use]
    pub fn video(delay: Duration) -> Self {
        Self::new(delay, VIDEO_BYTE_CAP, Kind::Video)
    }

    /// Audio queue: the same delay, with a cap far above 8 s of FPA1.
    #[must_use]
    pub fn audio(delay: Duration) -> Self {
        Self::new(delay, 8 * 1024 * 1024, Kind::Audio)
    }

    fn new(delay: Duration, byte_cap: usize, kind: Kind) -> Self {
        Self {
            frames: VecDeque::new(),
            bytes: 0,
            delay,
            byte_cap,
            interval: None,
            kind,
            spare: Vec::new(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Queued media time from the current interval, or zero before the first
    /// timed frame.
    #[must_use]
    pub fn duration(&self) -> Duration {
        match self.interval {
            Some(interval) => {
                let count = u32::try_from(self.frames.len()).unwrap_or(u32::MAX);
                interval.saturating_mul(count)
            }
            None => Duration::ZERO,
        }
    }

    /// Ready to start HDMI or ALSA: delay 0 needs one frame; otherwise the
    /// configured delay, or the byte cap when a fat stream fills RAM first.
    #[must_use]
    pub fn filled(&self) -> bool {
        if self.frames.is_empty() {
            return false;
        }
        if self.delay.is_zero() {
            return true;
        }
        self.duration() >= self.delay || self.bytes >= self.byte_cap
    }

    /// A payload buffer for the next received frame: a recycled one when
    /// there is one, otherwise empty. Its contents are stale and its length
    /// is kept, so the receiver zero-fills only bytes it has never had.
    #[must_use]
    pub fn spare(&mut self) -> Vec<u8> {
        self.spare.pop().unwrap_or_default()
    }

    /// Returns a played frame's payload buffer for reuse.
    pub fn recycle(&mut self, payload: Vec<u8>) {
        if self.spare.len() < SPARE_BUFFERS && payload.capacity() > 0 {
            self.spare.push(payload);
        }
    }

    /// Appends a compressed media frame. Metadata is ignored so it cannot
    /// occupy the video budget. Returns how many older frames were dropped.
    pub fn push(&mut self, frame: Frame, playing: bool) -> u64 {
        if frame.video.is_none() && frame.audio.is_none() {
            self.recycle(frame.payload);
            return 0;
        }
        if let Some(interval) = interval_of(&frame) {
            self.interval = Some(interval);
        }
        let added = frame.payload.len();
        if added > self.byte_cap {
            return 0;
        }
        self.bytes = self.bytes.saturating_add(added);
        self.frames.push_back(frame);
        let mut dropped = self.drop_while_over_bytes();
        dropped = dropped.saturating_add(self.drop_while_over_time());
        if playing {
            dropped = dropped.saturating_add(self.trim_to_target());
        }
        dropped
    }

    /// Takes the oldest unplayed frame.
    pub fn pop(&mut self) -> Option<Frame> {
        let frame = self.frames.pop_front()?;
        self.bytes = self.bytes.saturating_sub(frame.payload.len());
        Some(frame)
    }

    /// Drops every queued video frame except the newest. Delay 0 presents
    /// immediately rather than playing through a burst that arrived before the
    /// first flip. Audio keeps its burst: every frame of it is sound.
    pub fn keep_latest_only(&mut self) {
        if self.kind == Kind::Audio {
            return;
        }
        while self.frames.len() > 1 {
            self.drop_oldest();
        }
    }

    fn drop_oldest(&mut self) {
        if let Some(frame) = self.frames.pop_front() {
            self.bytes = self.bytes.saturating_sub(frame.payload.len());
            self.recycle(frame.payload);
        }
    }

    fn drop_while_over_bytes(&mut self) -> u64 {
        let mut dropped = 0_u64;
        while self.bytes > self.byte_cap && !self.frames.is_empty() {
            self.drop_oldest();
            dropped = dropped.saturating_add(1);
        }
        dropped
    }

    fn drop_while_over_time(&mut self) -> u64 {
        self.drop_while_longer_than(self.delay.saturating_add(TIME_SLACK))
    }

    fn drop_while_longer_than(&mut self, limit: Duration) -> u64 {
        let mut dropped = 0_u64;
        while self.duration() > limit && self.frames.len() > 1 {
            self.drop_oldest();
            dropped = dropped.saturating_add(1);
        }
        dropped
    }

    /// Keeps a running session live when the sender is faster than playout.
    /// Video holds delay 0 at one frame (present immediately) and anything
    /// else at the configured delay; audio only sheds past [`AUDIO_SLACK`].
    fn trim_to_target(&mut self) -> u64 {
        match self.kind {
            Kind::Audio => self.drop_while_longer_than(self.delay.saturating_add(AUDIO_SLACK)),
            Kind::Video if self.delay.is_zero() => {
                let mut dropped = 0_u64;
                while self.frames.len() > 1 {
                    self.drop_oldest();
                    dropped = dropped.saturating_add(1);
                }
                dropped
            }
            Kind::Video => self.drop_while_longer_than(self.delay),
        }
    }
}

fn interval_of(frame: &Frame) -> Option<Duration> {
    if let Some(video) = frame.video.as_ref() {
        video_interval(video)
    } else {
        frame.audio.as_ref().and_then(audio_interval)
    }
}

/// One video frame's duration from `FrameRateN/D`.
#[must_use]
pub fn video_interval(header: &VideoHeader) -> Option<Duration> {
    duration_from_ratio(header.frame_rate_d, header.frame_rate_n)
}

/// One audio frame's duration from samples and sample rate.
#[must_use]
pub fn audio_interval(header: &AudioHeader) -> Option<Duration> {
    duration_from_ratio(header.samples_per_channel, header.sample_rate)
}

fn duration_from_ratio(numerator: i32, denominator: i32) -> Option<Duration> {
    if numerator <= 0 || denominator <= 0 {
        return None;
    }
    let seconds = f64::from(numerator) / f64::from(denominator);
    if !seconds.is_finite() || seconds <= 0.0 {
        return None;
    }
    Duration::try_from_secs_f64(seconds).ok()
}

/// Handshake so HDMI and ALSA start together after pre-roll, without blocking
/// video forever when the sender has no audio.
pub struct FillGate {
    video: AtomicBool,
    audio: AtomicBool,
    audio_missing: AtomicBool,
}

impl FillGate {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            video: AtomicBool::new(false),
            audio: AtomicBool::new(false),
            audio_missing: AtomicBool::new(false),
        })
    }

    pub fn set_video(&self, ready: bool) {
        self.video.store(ready, Ordering::Relaxed);
    }

    pub fn set_audio(&self, ready: bool) {
        self.audio.store(ready, Ordering::Relaxed);
    }

    pub fn set_audio_missing(&self) {
        self.audio_missing.store(true, Ordering::Relaxed);
    }

    #[must_use]
    pub fn video_ready(&self) -> bool {
        self.video.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn audio_ready(&self) -> bool {
        self.audio.load(Ordering::Relaxed) || self.audio_missing.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omt_protocol::{FrameHeader, FrameType, VIDEO_HEADER_SIZE};

    fn video_frame(timestamp: i64, payload_bytes: usize, rate_n: i32, rate_d: i32) -> Frame {
        let payload = vec![0_u8; VIDEO_HEADER_SIZE.saturating_add(payload_bytes)];
        Frame {
            header: FrameHeader {
                frame_type: FrameType::Video,
                timestamp,
                metadata_length: 0,
                data_length: u32::try_from(payload.len()).unwrap_or(u32::MAX),
            },
            video: Some(VideoHeader {
                codec: 0,
                width: 1920,
                height: 1080,
                frame_rate_n: rate_n,
                frame_rate_d: rate_d,
                aspect_ratio: 16.0 / 9.0,
                flags: 0,
                color_space: 0,
            }),
            audio: None,
            payload,
        }
    }

    fn metadata_frame() -> Frame {
        Frame {
            header: FrameHeader {
                frame_type: FrameType::Metadata,
                timestamp: 0,
                metadata_length: 0,
                data_length: 0,
            },
            video: None,
            audio: None,
            payload: Vec::new(),
        }
    }

    fn audio_frame(samples: i32, rate: i32) -> Frame {
        Frame {
            header: FrameHeader {
                frame_type: FrameType::Audio,
                timestamp: 0,
                metadata_length: 0,
                data_length: 24,
            },
            video: None,
            audio: Some(AudioHeader {
                codec: 0,
                sample_rate: rate,
                samples_per_channel: samples,
                channels: 2,
                active_channels: 3,
            }),
            payload: vec![0_u8; 24],
        }
    }

    #[test]
    fn delay_zero_is_filled_by_the_first_frame() {
        let mut queue = Queue::video(Duration::ZERO);
        assert!(!queue.filled());
        assert_eq!(queue.push(video_frame(1, 32, 30, 1), false), 0);
        assert!(queue.filled());
        assert_eq!(queue.frames.len(), 1);
        assert!(queue.push(video_frame(2, 32, 30, 1), true) >= 1);
        assert_eq!(queue.frames.len(), 1);
    }

    #[test]
    fn four_seconds_of_one_hertz_video_fills_a_four_second_delay() {
        let mut queue = Queue::video(Duration::from_secs(4));
        for stamp in 0..4 {
            assert!(!queue.filled(), "not full after {stamp} frames");
            queue.push(video_frame(stamp, 64, 1, 1), false);
        }
        assert!(queue.filled());
        assert_eq!(queue.duration(), Duration::from_secs(4));
    }

    #[test]
    fn a_four_second_fill_covers_a_three_point_five_second_gap() {
        let mut queue = Queue::video(Duration::from_secs(4));
        for stamp in 0..4 {
            queue.push(video_frame(stamp, 64, 1, 1), false);
        }
        assert!(queue.filled());
        for _ in 0..3 {
            assert!(queue.pop().is_some());
        }
        assert!(!queue.is_empty());
        assert!(queue.pop().is_some());
        assert!(queue.is_empty());
    }

    #[test]
    fn a_five_second_gap_exhausts_a_four_second_queue() {
        let mut queue = Queue::video(Duration::from_secs(4));
        for stamp in 0..4 {
            queue.push(video_frame(stamp, 64, 1, 1), false);
        }
        for _ in 0..4 {
            assert!(queue.pop().is_some());
        }
        assert!(queue.pop().is_none());
    }

    #[test]
    fn the_byte_cap_drops_oldest_frames() {
        let mut queue = Queue::new(Duration::from_secs(8), 250, Kind::Video);
        assert_eq!(queue.push(video_frame(1, 80, 1, 1), false), 0);
        assert_eq!(queue.push(video_frame(2, 80, 1, 1), false), 0);
        let dropped = queue.push(video_frame(3, 80, 1, 1), false);
        assert!(dropped >= 1, "expected a drop, got {dropped}");
        assert!(queue.bytes <= 250);
        assert!(queue.frames.len() <= 2);
    }

    #[test]
    fn metadata_does_not_occupy_the_video_budget() {
        let mut queue = Queue::video(Duration::from_secs(4));
        assert_eq!(queue.push(metadata_frame(), false), 0);
        assert!(queue.is_empty());
        assert_eq!(queue.bytes, 0);
    }

    #[test]
    fn a_faster_sender_is_trimmed_to_the_configured_delay_once_playing() {
        let mut queue = Queue::video(Duration::from_secs(2));
        for stamp in 0..2 {
            queue.push(video_frame(stamp, 32, 1, 1), false);
        }
        assert!(queue.filled());
        queue.push(video_frame(2, 32, 1, 1), true);
        assert!(queue.duration() <= Duration::from_secs(2));
        assert_eq!(queue.frames.len(), 2);
    }

    #[test]
    fn audio_interval_follows_sample_count() {
        let header = AudioHeader {
            codec: 0,
            sample_rate: 48_000,
            samples_per_channel: 48_000,
            channels: 2,
            active_channels: 3,
        };
        assert_eq!(audio_interval(&header), Some(Duration::from_secs(1)));
        let mut queue = Queue::audio(Duration::from_secs(2));
        queue.push(audio_frame(48_000, 48_000), false);
        queue.push(audio_frame(48_000, 48_000), false);
        assert!(queue.filled());
    }

    /// The dropout this exists for: at delay 0 a Wi-Fi burst of audio frames
    /// used to be cut to its newest frame, and every frame cut was a gap.
    #[test]
    fn delay_zero_audio_keeps_a_whole_burst() {
        let mut queue = Queue::audio(Duration::ZERO);
        queue.push(audio_frame(960, 48_000), false);
        assert!(queue.filled());
        queue.keep_latest_only();
        for _ in 0..4 {
            assert_eq!(queue.push(audio_frame(960, 48_000), true), 0);
        }
        assert_eq!(queue.frames.len(), 5);
    }

    /// Audio sheds only past the slack, so a sender clock that runs ahead
    /// still cannot grow latency without bound.
    #[test]
    fn playing_audio_is_trimmed_only_past_the_slack() {
        let mut queue = Queue::audio(Duration::ZERO);
        // 20 ms frames: the slack holds five of them and no more.
        let mut dropped = 0;
        for _ in 0..12 {
            dropped += queue.push(audio_frame(960, 48_000), true);
        }
        assert_eq!(queue.frames.len(), 5);
        assert_eq!(dropped, 7);

        let mut delayed = Queue::audio(Duration::from_millis(250));
        for _ in 0..20 {
            delayed.push(audio_frame(960, 48_000), true);
        }
        // 250 ms plus the slack is seventeen 20 ms frames.
        assert_eq!(delayed.frames.len(), 17);
    }

    #[test]
    fn delay_zero_video_still_keeps_only_the_newest_frame() {
        let mut queue = Queue::video(Duration::ZERO);
        queue.push(video_frame(1, 32, 30, 1), false);
        queue.push(video_frame(2, 32, 30, 1), false);
        queue.keep_latest_only();
        assert_eq!(queue.frames.len(), 1);
    }

    /// Dropped and played payloads come back as receive buffers, bounded.
    #[test]
    fn payload_buffers_are_recycled_and_bounded() {
        let mut queue = Queue::video(Duration::ZERO);
        assert_eq!(queue.spare().capacity(), 0);
        for stamp in 0..10 {
            queue.push(video_frame(stamp, 64, 30, 1), true);
        }
        assert_eq!(queue.spare.len(), SPARE_BUFFERS);
        let reused = queue.spare();
        assert!(reused.capacity() >= 64);
        queue.recycle(Vec::new());
        assert_eq!(queue.spare.len(), SPARE_BUFFERS - 1);
    }

    #[test]
    fn fill_gate_starts_video_without_audio_once_marked_missing() {
        let gate = FillGate::new();
        assert!(!gate.audio_ready());
        gate.set_video(true);
        assert!(gate.video_ready());
        assert!(!gate.audio_ready());
        gate.set_audio_missing();
        assert!(gate.audio_ready());
    }
}
