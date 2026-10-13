//! HDMI audio through ALSA. OMT sends planar float samples with a bitmask of
//! active channels; inactive channels are silence and absent from the body,
//! so the interleaver writes zeros for them without advancing the source
//! cursor.

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
}

#[derive(Default)]
pub struct AudioOutput {
    open: Option<Open>,
    interleaved: Vec<f32>,
    /// Underruns recovered this session. Every one is a gap the operator
    /// heard, so it is counted and survives a reopen.
    underruns: u64,
}

impl std::fmt::Debug for AudioOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioOutput").field("underruns", &self.underruns).finish_non_exhaustive()
    }
}

impl AudioOutput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn underruns(&self) -> u64 {
        self.underruns
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
        });
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
        interleave(body, h.active_channels, samples, channels, &mut self.interleaved)?;
        self.play(samples, channels)
    }

    fn play(&mut self, samples: usize, channels: usize) -> Result<(), Error> {
        let Some(o) = self.open.as_mut() else {
            return Err(Error::new("audio device is not open"));
        };
        let mut offset = 0;
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
                stall(&mut o.pcm, &mut stalled_since)?;
            }
        }
        if recovered {
            self.underruns = self.underruns.saturating_add(1);
        }
        Ok(())
    }
}
