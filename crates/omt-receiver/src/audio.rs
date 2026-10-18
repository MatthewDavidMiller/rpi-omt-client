//! HDMI audio through ALSA. OMT sends planar float samples with a bitmask of
//! active channels; inactive channels are silence and absent from the body,
//! so the interleaver writes zeros for them without advancing the source
//! cursor.
//!
//! Audio a sender dropped before sending is concealed with silence. The
//! frames after a hole would otherwise be written straight behind the ones
//! before it, the ring would hold that much less than the sender's timeline
//! says it should, and nothing ever gives the depth back: sender and sink
//! run at the same rate. vMix drops audio whenever its send pool fills, which
//! over Wi-Fi is about a second a minute, so without this the cushion bled
//! away until ordinary jitter ran the ring dry.
//!
//! The ring is never allowed to run dry either. Over HDMI an underrun is not
//! a short gap: ALSA stops the stream, recovery re-prepares it, and the sink
//! drops audio lock and has to find it again, which a television plays as a
//! loud crack followed by a mute. Arriving late every second or two, which
//! Wi-Fi alone produces, that left nothing audible but the cracks. So when
//! the ring runs low with nothing to play, the worker keeps the stream going
//! with silence. The audio that then arrives late plays behind it, which
//! rebuilds the cushion the next late burst needs; the fill target and the
//! backlog shed bound the latency that adds, as a restart's prefill did.

use omt_common::Error;
use omt_common::timefmt::now_ms;
use omt_protocol::AUDIO_HEADER_SIZE;
use omt_sys::alsa::{self, Pcm};

use crate::channel::Frame;

/// Ring capacity, not latency: what the device may hold, so a burst off the
/// network can be written in one go.
pub const BUFFER_US: u32 = 240_000;
pub const PERIOD_US: u32 = 20_000;
/// Audio queued before the device may start: the cushion against one late
/// frame becoming an audible gap.
pub const PREFILL_MS: i64 = 100;
/// How full the worker keeps the ring; between the start threshold and the
/// ring size.
pub const RING_TARGET_MS: i64 = 160;
pub const WRITE_TIMEOUT_MS: u64 = 1000;
/// The longest hole in the sender's timeline concealed with silence. Past
/// this the timestamps were reset, not frames lost.
pub const CONCEAL_MAX_MS: i64 = 1000;
/// The ring depth below which a starved, playing stream is topped up with
/// silence: three periods, so a worker waking once a period still gets there
/// before the device runs dry when the decoder delays its wake by a period.
pub const KEEPALIVE_MS: i64 = 60;
/// OMT timestamps count 100 ns ticks.
pub const TICKS_PER_SECOND: i64 = 10_000_000;
const EAGAIN: i64 = 11;

/// The start threshold: the prefill at this rate, never less than one period
/// and never more than the ring, because a threshold outside that range is
/// one the device can never reach.
pub fn prefill_frames(sample_rate: i32, buffer_frames: i64, period_frames: i64) -> i64 {
    let wanted = if sample_rate > 0 { i64::from(sample_rate) * PREFILL_MS / 1000 } else { 0 };
    let floor = period_frames.max(1);
    let ceiling = buffer_frames.max(floor);
    wanted.clamp(floor, ceiling)
}

pub fn target_frames(sample_rate: i32, buffer_frames: i64, start: i64) -> i64 {
    let wanted = if sample_rate > 0 { i64::from(sample_rate) * RING_TARGET_MS / 1000 } else { 0 };
    let ceiling = buffer_frames.max(start);
    wanted.clamp(start, ceiling)
}

/// The sender timestamp one frame of `samples` at `rate` ends on, which is
/// where the next frame starts when none was lost.
pub fn end_timestamp(timestamp: i64, samples: i64, rate: i32) -> Option<i64> {
    if rate <= 0 || samples <= 0 {
        return None;
    }
    let ticks = samples.checked_mul(TICKS_PER_SECOND)? / i64::from(rate);
    timestamp.checked_add(ticks)
}

/// Frames of silence standing in for audio missing between `expected` and
/// a frame stamped `timestamp`. Only a hole of at least three quarters of
/// this frame counts, so a sender whose stamps jitter is never padded, and
/// a jump back or past [`CONCEAL_MAX_MS`] is a new timeline rather than a
/// loss.
pub fn missing_frames(expected: i64, timestamp: i64, rate: i32, samples: i64) -> i64 {
    let Some(ticks) = timestamp.checked_sub(expected) else { return 0 };
    if rate <= 0 || samples <= 0 || ticks <= 0 || ticks > CONCEAL_MAX_MS * (TICKS_PER_SECOND / 1000)
    {
        return 0;
    }
    // Bounded above by the cap times the rate, far inside i64.
    let frames = ticks * i64::from(rate) / TICKS_PER_SECOND;
    if frames * 4 < samples * 3 { 0 } else { frames }
}

/// Interleaves planar float samples into `out`, which holds at least
/// `samples * channels` values.
pub fn interleave(
    body: &[u8],
    active: u32,
    samples: usize,
    channels: usize,
    out: &mut [f32],
) -> Result<(), Error> {
    let mut cursor = 0;
    for channel in 0..channels {
        // Past the mask width there is no bit to test: a channel that cannot
        // be proven active is silence.
        let on = channel < 32 && active & (1 << channel) != 0;
        for sample in 0..samples {
            let mut value = 0.0_f32;
            if on {
                let bytes = body
                    .get(cursor..cursor + 4)
                    .ok_or_else(|| Error::new("truncated OMT audio frame"))?;
                value = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                cursor += 4;
            }
            let slot = out
                .get_mut(sample * channels + channel)
                .ok_or_else(|| Error::new("OMT audio output buffer is too short"))?;
            *slot = value;
        }
    }
    Ok(())
}

fn alsa_error(prefix: &str, function: &str, code: i32) -> Error {
    Error::new(format!(
        "{prefix}: ALSA function '{function}' failed with error '{}'",
        alsa::strerror(code)
    ))
}

struct Open {
    pcm: Pcm,
    device: String,
    sample_rate: i32,
    channels: i32,
    buffer_frames: u64,
    target_frames: u64,
    start_frames: u64,
    period_frames: u64,
    /// The device has reached its start threshold and is playing, so it
    /// must be kept fed.
    playing: bool,
}

#[derive(Default)]
pub struct AudioOutput {
    open: Option<Open>,
    interleaved: Vec<f32>,
    /// Underruns recovered this session. Every one is a gap the operator
    /// heard, so it is counted and survives a reopen.
    underruns: u64,
    /// Where the sender's timeline says the next frame starts, while the
    /// ring is continuous with it.
    next_timestamp: Option<i64>,
    /// Frames of silence written in place of audio the sender dropped.
    concealed: u64,
    /// Starved since the last frame was written, so a run of top-ups is
    /// counted once.
    starving: bool,
}

impl std::fmt::Debug for AudioOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioOutput")
            .field("underruns", &self.underruns)
            .field("concealed", &self.concealed)
            .finish_non_exhaustive()
    }
}

impl AudioOutput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn underruns(&self) -> u64 {
        self.underruns
    }

    pub fn concealed(&self) -> u64 {
        self.concealed
    }

    /// Whether the device is playing and so must be fed by the worker's
    /// next wake, not whenever audio happens to arrive.
    pub fn playing(&self) -> bool {
        self.open.as_ref().is_some_and(|o| o.playing)
    }

    /// Keeps a playing stream from running dry: below [`KEEPALIVE_MS`] it
    /// writes a period of silence. A starvation is a gap the operator hears, so each one is counted
    /// with the underruns; it is just no longer a restart.
    pub fn keep_alive(&mut self) -> Result<(), Error> {
        let Some(o) = self.open.as_mut() else { return Ok(()) };
        if !o.playing {
            return Ok(());
        }
        // An underrun or suspend: the write path recovers it.
        let Ok(avail) = u64::try_from(o.pcm.avail_update()) else { return Ok(()) };
        let used = o.buffer_frames.saturating_sub(avail);
        let low = u64::try_from(i64::from(o.sample_rate) * KEEPALIVE_MS / 1000).unwrap_or(0);
        if used >= low {
            return Ok(());
        }
        let channels = o.channels.unsigned_abs() as usize;
        let frames = usize::try_from(o.period_frames).unwrap_or(0).max(1);
        let required = frames * channels;
        if required > self.interleaved.len() {
            self.interleaved
                .try_reserve_exact(required - self.interleaved.len())
                .map_err(|_| Error::new("Unable to allocate bounded audio buffer"))?;
            self.interleaved.resize(required, 0.0);
        }
        self.interleaved[..required].fill(0.0);
        if !self.starving {
            self.starving = true;
            self.underruns = self.underruns.saturating_add(1);
        }
        self.play(0, frames, channels)
    }

    /// Breaks continuity with the sender's timeline, so the next frame is
    /// not padded: the gap before it was made here, by shedding a backlog.
    pub fn forget_timeline(&mut self) {
        self.next_timestamp = None;
    }

    /// Frames queued in the ring ahead of the speaker, if the device can say.
    pub fn queued_frames(&mut self) -> Option<u64> {
        let o = self.open.as_mut()?;
        let avail = u64::try_from(o.pcm.avail_update()).ok()?;
        Some(o.buffer_frames.saturating_sub(avail))
    }

    /// The open device's sample rate, if one is open.
    pub fn sample_rate(&self) -> Option<i32> {
        self.open.as_ref().map(|o| o.sample_rate)
    }

    /// Whether the ring holds less than the fill target (or cannot say).
    pub fn room(&mut self) -> bool {
        let Some(o) = self.open.as_mut() else { return true };
        let avail = o.pcm.avail_update();
        // An underrun or suspend reads as room: the write path recovers it.
        let Ok(avail) = u64::try_from(avail) else { return true };
        let used = o.buffer_frames.saturating_sub(avail);
        used < o.target_frames
    }

    fn configure(&mut self, device: &str, rate: i32, channels: i32) -> Result<(), Error> {
        self.open = None;
        self.next_timestamp = None;
        let mut pcm = Pcm::open(device)
            .map_err(|rc| alsa_error("Unable to open audio device", "snd_pcm_open", rc))?;
        let (buffer, period) = pcm
            .configure_hw(channels.unsigned_abs(), rate.unsigned_abs(), BUFFER_US, PERIOD_US)
            .map_err(|(step, rc)| alsa_error("Unable to configure audio device", step, rc))?;
        // The device refines both sizes; the thresholds use what it chose.
        let start = prefill_frames(
            rate,
            i64::try_from(buffer).unwrap_or(i64::MAX),
            i64::try_from(period).unwrap_or(i64::MAX),
        );
        // The ring running dry stays an error the writer is told about, so
        // an underrun is counted and recovered rather than passing as
        // silence.
        pcm.configure_sw(period, start.unsigned_abs(), buffer)
            .map_err(|(step, rc)| alsa_error("Unable to configure audio timing", step, rc))?;
        let target = target_frames(rate, i64::try_from(buffer).unwrap_or(i64::MAX), start);
        self.open = Some(Open {
            pcm,
            device: device.to_owned(),
            sample_rate: rate,
            channels,
            buffer_frames: buffer,
            target_frames: target.unsigned_abs(),
            start_frames: start.unsigned_abs(),
            period_frames: period,
            playing: false,
        });
        self.starving = false;
        Ok(())
    }

    /// Writes one frame, reopening the device if the format or device
    /// changed.
    pub fn write(&mut self, frame: &Frame, device: &str) -> Result<(), Error> {
        let Some(h) = frame.audio else { return Err(Error::new("not an OMT audio frame")) };
        let current = self.open.as_ref().is_some_and(|o| {
            o.sample_rate == h.sample_rate && o.channels == h.channels && o.device == device
        });
        if !current {
            self.configure(device, h.sample_rate, h.channels)?;
        }
        let channels = h.channels.unsigned_abs() as usize;
        let samples = h.samples_per_channel.unsigned_abs() as usize;
        // Both bounded by the protocol.
        let required = samples * channels;
        if required > self.interleaved.len() {
            self.interleaved
                .try_reserve_exact(required - self.interleaved.len())
                .map_err(|_| Error::new("Unable to allocate bounded audio buffer"))?;
            self.interleaved.resize(required, 0.0);
        }
        let body = frame
            .media(AUDIO_HEADER_SIZE as usize)
            .ok_or_else(|| Error::new("truncated OMT audio frame"))?;
        let timestamp = frame.header.map(|f| f.timestamp);
        let mut silence = self.silence_frames(timestamp, h.sample_rate, samples);
        if silence > 0 {
            self.interleaved[..required].fill(0.0);
            self.concealed =
                self.concealed.saturating_add(u64::try_from(silence).unwrap_or(u64::MAX));
            while silence > 0 {
                let chunk = silence.min(samples);
                self.play(0, chunk, channels)?;
                silence -= chunk;
            }
        }
        interleave(body, h.active_channels, samples, channels, &mut self.interleaved)?;
        self.starving = false;
        self.play(0, samples, channels)?;
        self.next_timestamp = timestamp.and_then(|t| {
            end_timestamp(t, i64::try_from(samples).unwrap_or(i64::MAX), h.sample_rate)
        });
        Ok(())
    }

    /// Silence owed before a frame stamped `timestamp`, no more than the ring
    /// can take alongside the frame itself. A ring that already ran dry gets
    /// none: the underrun spent that time, and padding it again would only
    /// add latency.
    fn silence_frames(&mut self, timestamp: Option<i64>, rate: i32, samples: usize) -> usize {
        let (Some(expected), Some(timestamp), Some(o)) =
            (self.next_timestamp, timestamp, self.open.as_mut())
        else {
            return 0;
        };
        let Ok(free) = usize::try_from(o.pcm.avail_update()) else { return 0 };
        let wanted =
            missing_frames(expected, timestamp, rate, i64::try_from(samples).unwrap_or(i64::MAX));
        usize::try_from(wanted).unwrap_or(0).min(free.saturating_sub(samples))
    }

    /// Writes interleaved frames `from..samples` of the scratch buffer.
    fn play(&mut self, from: usize, samples: usize, channels: usize) -> Result<(), Error> {
        let Some(o) = self.open.as_mut() else {
            return Err(Error::new("audio device is not open"));
        };
        let mut offset = from;
        let mut stalled_since: Option<u64> = None;
        // One frame needing several recoveries is still one gap, counted
        // once.
        let mut recovered = false;
        // Waits for the device to take more samples, giving up once one write
        // has made no progress for the write timeout.
        let stall = |pcm: &mut Pcm, since: &mut Option<u64>| -> Result<(), Error> {
            let start = *since.get_or_insert_with(now_ms);
            if now_ms().saturating_sub(start) > WRITE_TIMEOUT_MS {
                return Err(Error::new("Audio device remained unavailable for one second"));
            }
            let _ = pcm.wait(100);
            Ok(())
        };
        while offset < samples {
            let written =
                o.pcm.writei(&self.interleaved[offset * channels..samples * channels], channels);
            if written > 0 {
                offset += usize::try_from(written).unwrap_or(0);
                stalled_since = None;
            } else if written == 0 || written == -EAGAIN {
                // A full ring is the device asking for time, not a fault.
                stall(&mut o.pcm, &mut stalled_since)?;
            } else {
                // Recover from an underrun or a suspend once; a second
                // failure means the sink is gone. Recovery re-prepares the
                // device, so the start threshold rebuilds the cushion before
                // it plays.
                let rc = o.pcm.recover(i32::try_from(written).unwrap_or(i32::MIN));
                if rc < 0 {
                    return Err(Error::new(format!(
                        "Unable to write audio: ALSA function 'snd_pcm_recover' failed with error '{}'",
                        alsa::strerror(rc)
                    )));
                }
                recovered = true;
                o.playing = false;
                stall(&mut o.pcm, &mut stalled_since)?;
            }
        }
        if !o.playing
            && let Ok(avail) = u64::try_from(o.pcm.avail_update())
        {
            o.playing = o.buffer_frames.saturating_sub(avail) >= o.start_frames;
        }
        if recovered {
            self.underruns = self.underruns.saturating_add(1);
        }
        Ok(())
    }
}
