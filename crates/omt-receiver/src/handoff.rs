//! Frame handoff from the socket to the display and the audio device.
//!
//! There is no playout buffer. An OMT sender keeps none, so frames held here
//! would only add latency the picture cannot win back. TCP reads stay greedy
//! (OMT: never stall the accept path), and what they deliver is handed on as
//! soon as the consumer can take it:
//!
//!   * Video keeps only the newest frame. The session decodes and flips it as
//!     soon as it arrives; a frame a newer one replaces before the display
//!     took it is dropped and counted.
//!   * Audio cannot skip ahead, so it queues in arrival order for the ALSA
//!     ring, which paces it. A backlog past [`AUDIO_BACKLOG_NS`] -- a sender
//!     running faster than the sink -- sheds its oldest frames so latency
//!     cannot creep.

use std::collections::VecDeque;

use omt_protocol::{AudioHeader, FrameType, VIDEO_MAX_SIZE};

use crate::channel::Frame;

/// The most compressed media a queue may hold: one video frame, or the
/// audio backlog the trim below allows several times over.
pub const VIDEO_BYTE_CAP: usize = VIDEO_MAX_SIZE as usize + 4096;
pub const AUDIO_BYTE_CAP: usize = 8 * 1024 * 1024;
/// Queued audio beyond this is a sender ahead of the sink.
pub const AUDIO_BACKLOG_NS: u64 = 100_000_000;
/// Payload buffers kept for reuse.
pub const SPARE_BUFFERS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueKind {
    Video,
    Audio,
}

#[derive(Debug)]
pub struct Queue {
    frames: VecDeque<Frame>,
    bytes: usize,
    byte_cap: usize,
    interval_ns: Option<u64>,
    kind: QueueKind,
    spares: Vec<Vec<u8>>,
}

impl Queue {
    pub fn new(kind: QueueKind, byte_cap: usize) -> Self {
        Self {
            frames: VecDeque::new(),
            bytes: 0,
            byte_cap,
            interval_ns: None,
            kind,
            spares: Vec::with_capacity(SPARE_BUFFERS),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn spare_count(&self) -> usize {
        self.spares.len()
    }

    /// A recycled payload buffer for the next received frame, or an empty
    /// one.
    pub fn take_spare(&mut self) -> Vec<u8> {
        self.spares.pop().unwrap_or_default()
    }

    pub fn recycle(&mut self, buffer: Vec<u8>) {
        if buffer.capacity() > 0 && self.spares.len() < SPARE_BUFFERS {
            self.spares.push(buffer);
        }
    }

    fn drop_oldest(&mut self) {
        if let Some(f) = self.frames.pop_front() {
            self.bytes = self.bytes.saturating_sub(f.len);
            self.recycle(f.buffer);
        }
    }

    /// Takes ownership of `frame`. Metadata is ignored. Returns how many
    /// older frames were dropped to make way for it; a frame that could not
    /// be stored is recycled.
    pub fn push(&mut self, frame: Frame) -> u64 {
        let wanted = match self.kind {
            QueueKind::Video => {
                frame.video.is_some() && frame.frame_type() == Some(FrameType::Video)
            }
            QueueKind::Audio => {
                frame.audio.is_some() && frame.frame_type() == Some(FrameType::Audio)
            }
        };
        if !wanted || frame.len > self.byte_cap {
            self.recycle(frame.buffer);
            return 0;
        }
        let mut dropped = 0;
        if self.kind == QueueKind::Video {
            // Video is a single slot: the newer frame always wins.
            while !self.frames.is_empty() {
                self.drop_oldest();
                dropped += 1;
            }
        } else if let Some(interval) = frame.audio.as_ref().and_then(audio_interval_ns) {
            self.interval_ns = Some(interval);
        }
        if self.frames.try_reserve(1).is_err() {
            self.recycle(frame.buffer);
            return dropped;
        }
        self.bytes += frame.len;
        self.frames.push_back(frame);
        while self.frames.len() > 1 && (self.bytes > self.byte_cap || self.audio_backlog_exceeded())
        {
            self.drop_oldest();
            dropped += 1;
        }
        dropped
    }

    /// The oldest frame, if any.
    pub fn pop(&mut self) -> Option<Frame> {
        let f = self.frames.pop_front()?;
        self.bytes = self.bytes.saturating_sub(f.len);
        Some(f)
    }

    /// The queued audio's duration, from the latest frame's interval.
    fn audio_backlog_exceeded(&self) -> bool {
        match self.interval_ns {
            Some(interval) if self.frames.len() > 1 => {
                u128::from(interval) * self.frames.len() as u128 > u128::from(AUDIO_BACKLOG_NS)
            }
            _ => false,
        }
    }
}

/// One audio frame's duration, rounded to the nearest nanosecond as
/// Duration::try_from_secs_f64 rounds.
pub fn audio_interval_ns(h: &AudioHeader) -> Option<u64> {
    if h.samples_per_channel <= 0 || h.sample_rate <= 0 {
        return None;
    }
    let seconds = f64::from(h.samples_per_channel) / f64::from(h.sample_rate);
    if !seconds.is_finite() || seconds <= 0.0 || seconds > 1.8e10 {
        return None;
    }
    // Bounded above by 1.8e19, inside u64.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some((seconds * 1e9).round() as u64)
}
