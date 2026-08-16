// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// HDMI audio through ALSA. OMT sends planar float samples with a bitmask of
// active channels; inactive channels are silence rather than absent, so the
// interleaver writes zeros for them and never advances the source cursor.

use crate::channel::Frame;
use alsa::pcm::{Access, Format, Frames, HwParams, PCM};
use alsa::{Direction, ValueOr};
use omt_protocol::AUDIO_HEADER_SIZE;
use std::time::{Duration, Instant};

/// Playback ring target.
///
/// This is capacity, not latency: what the device is allowed to hold, so a
/// burst of frames off the network can be written in one go instead of being
/// metered out by `stall`. The appliance's link is Wi-Fi carrying this
/// session's own 1080p video, which delivers audio in bursts with gaps
/// between them, and a ring only as deep as a couple of bursts runs dry in
/// the gaps.
const BUFFER: Duration = Duration::from_millis(240);
/// Ring granularity, and the wakeup interval a stalled write waits on.
const PERIOD: Duration = Duration::from_millis(20);
/// How much audio has to be queued before the device is allowed to start.
///
/// This one *is* latency, and it is the whole fix for choppy playback. ALSA's
/// default start threshold is a single frame, so the DAC starts on the first
/// write with nothing behind it and the next network gap is an underrun -- a
/// gap the operator hears. Holding the start until there is a cushion means
/// the ring has this much to give back before the stream breaks, and because
/// recovery re-prepares the device the cushion is rebuilt after every
/// underrun instead of restarting empty into the next one.
///
/// It is chosen against lip sync, not against the network: audio lagging
/// video by this much is inside the ITU-R BT.1359 tolerance, and the video
/// path's own decode and scanout latency offsets part of it.
const PREFILL: Duration = Duration::from_millis(100);
/// A device that will not accept a sample for this long has failed.
const WRITE_TIMEOUT: Duration = Duration::from_secs(1);
/// `EAGAIN`, spelled out rather than pulled in from libc for one integer.
///
/// The PCM is opened non-blocking, so a ring buffer that is momentarily full
/// reports this instead of blocking. It is ordinary back-pressure and it does
/// happen in steady playback, whenever the writer runs ahead of the sink.
/// `snd_pcm_recover` handles only underrun and suspend and hands EAGAIN back
/// unchanged, so routing it there reported a working HDMI sink as lost.
const AGAIN: i32 = 11;

pub struct Output {
    pcm: Option<PCM>,
    device: String,
    sample_rate: i32,
    channels: i32,
    interleaved: Vec<f32>,
    /// Underruns recovered since this output was created.
    ///
    /// Every one of these is a gap the operator heard, so it is counted and
    /// reported rather than silently repaired. It survives `close`, because a
    /// mid-session format change reopens the device and the operator is asking
    /// about the session, not about one PCM handle.
    underruns: u64,
}

impl Output {
    #[must_use]
    pub fn new() -> Self {
        Self {
            pcm: None,
            device: String::new(),
            sample_rate: 0,
            channels: 0,
            interleaved: Vec::new(),
            underruns: 0,
        }
    }

    /// Underruns this output has recovered from, for the playback status.
    pub fn underruns(&self) -> u64 {
        self.underruns
    }

    pub fn close(&mut self) {
        if let Some(pcm) = self.pcm.take() {
            let _ = pcm.drop();
        }
        self.device.clear();
        self.sample_rate = 0;
        self.channels = 0;
    }

    /// Writes one audio frame, reopening the device if the format changed.
    pub fn write(&mut self, frame: &Frame, device: &str) -> Result<(), String> {
        let header = frame
            .audio
            .as_ref()
            .ok_or_else(|| "not an OMT audio frame".to_owned())?;
        if self.pcm.is_none()
            || self.sample_rate != header.sample_rate
            || self.channels != header.channels
            || self.device != device
        {
            self.configure(device, header.sample_rate, header.channels)?;
        }

        let channels = usize::try_from(header.channels).map_err(|error| error.to_string())?;
        let samples =
            usize::try_from(header.samples_per_channel).map_err(|error| error.to_string())?;
        let required = samples
            .checked_mul(channels)
            .ok_or_else(|| "OMT audio sample count is out of range".to_owned())?;
        if required > self.interleaved.len() {
            self.interleaved
                .try_reserve(required - self.interleaved.len())
                .map_err(|_| "Unable to allocate bounded audio buffer".to_owned())?;
        }
        self.interleaved.resize(required, 0.0);

        let body = frame
            .media(AUDIO_HEADER_SIZE)
            .ok_or_else(|| "truncated OMT audio frame".to_owned())?;
        interleave(
            body,
            header.active_channels,
            samples,
            channels,
            &mut self.interleaved,
        )?;

        self.play(samples, channels)
    }

    fn configure(&mut self, device: &str, sample_rate: i32, channels: i32) -> Result<(), String> {
        self.close();
        let pcm = PCM::new(device, Direction::Playback, true)
            .map_err(|error| format!("Unable to open audio device: {error}"))?;
        let geometry = {
            let params = HwParams::any(&pcm)
                .map_err(|error| format!("Unable to configure audio device: {error}"))?;
            let configure = || -> Result<(Frames, Frames), alsa::Error> {
                params.set_channels(u32::try_from(channels).unwrap_or(2))?;
                params.set_rate(
                    u32::try_from(sample_rate).unwrap_or(48_000),
                    ValueOr::Nearest,
                )?;
                params.set_format(Format::float())?;
                params.set_access(Access::RWInterleaved)?;
                params.set_buffer_time_near(microseconds(BUFFER), ValueOr::Nearest)?;
                params.set_period_time_near(microseconds(PERIOD), ValueOr::Nearest)?;
                pcm.hw_params(&params)?;
                // Read the sizes back rather than deriving them from the times
                // that were asked for. The device refines both, and the start
                // threshold below has to be expressed in the frames it chose.
                Ok((params.get_buffer_size()?, params.get_period_size()?))
            };
            configure().map_err(|error| format!("Unable to configure audio device: {error}"))?
        };
        Self::set_timing(&pcm, sample_rate, geometry)?;
        self.pcm = Some(pcm);
        device.clone_into(&mut self.device);
        self.sample_rate = sample_rate;
        self.channels = channels;
        Ok(())
    }

    /// Sets the software timing ALSA would otherwise default.
    ///
    /// Without this the device starts on the first frame written, so playback
    /// begins with no cushion at all and the first network gap is an audible
    /// underrun. `hw_params` alone leaves those defaults in place, which is
    /// why they are written here explicitly rather than assumed.
    fn set_timing(
        pcm: &PCM,
        sample_rate: i32,
        (buffer_frames, period_frames): (Frames, Frames),
    ) -> Result<(), String> {
        let start = prefill_frames(sample_rate, buffer_frames, period_frames);
        let timing = || -> Result<(), alsa::Error> {
            let software = pcm.sw_params_current()?;
            software.set_avail_min(period_frames)?;
            software.set_start_threshold(start)?;
            // The ring running dry stays an error the writer is told about, so
            // an underrun is still counted and recovered rather than passing
            // as silence the operator never hears reported.
            software.set_stop_threshold(buffer_frames)?;
            pcm.sw_params(&software)
        };
        timing().map_err(|error| format!("Unable to configure audio timing: {error}"))
    }

    fn play(&mut self, samples: usize, channels: usize) -> Result<(), String> {
        let pcm = self
            .pcm
            .as_ref()
            .ok_or_else(|| "Audio device is not open".to_owned())?;
        let io = pcm
            .io_f32()
            .map_err(|error| format!("Unable to write audio: {error}"))?;
        let mut offset = 0_usize;
        let mut stalled_since: Option<Instant> = None;
        // One frame that needs several recoveries is still one gap in the
        // sound, so it is counted once rather than once per retry.
        let mut recovered = false;
        while offset < samples {
            let start = offset * channels;
            let slice = self
                .interleaved
                .get(start..samples * channels)
                .ok_or_else(|| "Audio buffer underflow".to_owned())?;
            match io.writei(slice) {
                Ok(0) => stall(pcm, &mut stalled_since)?,
                Ok(written) => {
                    offset += written;
                    stalled_since = None;
                }
                // A full ring buffer is the device asking for time, not
                // reporting a fault, so it waits for room on the same budget a
                // device that accepts nothing gets.
                Err(error) if error.errno() == AGAIN => stall(pcm, &mut stalled_since)?,
                Err(error) => {
                    // Recover from an underrun or a suspended device once; a
                    // second failure means the sink is gone.
                    pcm.try_recover(error, true)
                        .map_err(|error| format!("Unable to write audio: {error}"))?;
                    // Recovery re-prepares the device, so the start threshold
                    // applies again and the ring is refilled to the cushion
                    // before it plays. Restarting empty is what turned one
                    // late frame into a run of them.
                    recovered = true;
                    stall(pcm, &mut stalled_since)?;
                }
            }
        }
        if recovered {
            self.underruns = self.underruns.saturating_add(1);
        }
        Ok(())
    }
}

/// A duration as the microseconds ALSA's `*_time_near` setters take.
fn microseconds(value: Duration) -> u32 {
    u32::try_from(value.as_micros()).unwrap_or(u32::MAX)
}

/// The start threshold in frames: [`PREFILL`] at this rate, but never less
/// than one period and never more than the ring can hold, because a threshold
/// outside that range is one the device can never reach and playback would
/// never start at all.
fn prefill_frames(sample_rate: i32, buffer_frames: Frames, period_frames: Frames) -> Frames {
    let millis = Frames::try_from(PREFILL.as_millis()).unwrap_or(0);
    let wanted = Frames::from(sample_rate)
        .saturating_mul(millis)
        .saturating_div(1000);
    let floor = period_frames.max(1);
    wanted.clamp(floor, buffer_frames.max(floor))
}

/// Waits for the device to take more samples, giving up once one write has
/// made no progress for `WRITE_TIMEOUT`. The wait is bounded, so a sink that
/// has vanished ends the audio session rather than the playback loop.
fn stall(pcm: &PCM, stalled_since: &mut Option<Instant>) -> Result<(), String> {
    let since = *stalled_since.get_or_insert_with(Instant::now);
    if since.elapsed() > WRITE_TIMEOUT {
        return Err("Audio device remained unavailable for one second".into());
    }
    let _ = pcm.wait(Some(100));
    Ok(())
}

/// Expands one planar OMT body into the interleaved buffer ALSA is fed.
///
/// A channel the sender marked inactive is silence, and its samples are absent
/// from the body rather than zero-filled in it, so the source cursor only
/// advances for active channels. Getting that wrong does not fail: it plays the
/// next channel's samples on this one.
fn interleave(
    body: &[u8],
    active_channels: u32,
    samples: usize,
    channels: usize,
    out: &mut [f32],
) -> Result<(), String> {
    let mut cursor = 0_usize;
    for channel in 0..channels {
        let active = u32::try_from(channel)
            .ok()
            .and_then(|index| 1_u32.checked_shl(index))
            .is_some_and(|mask| active_channels & mask != 0);
        for sample in 0..samples {
            let value = if active {
                let end = cursor
                    .checked_add(4)
                    .ok_or_else(|| "truncated OMT audio frame".to_owned())?;
                let bytes: [u8; 4] = body
                    .get(cursor..end)
                    .and_then(|slice| slice.try_into().ok())
                    .ok_or_else(|| "truncated OMT audio frame".to_owned())?;
                cursor = end;
                f32::from_le_bytes(bytes)
            } else {
                0.0
            };
            let Some(slot) = out.get_mut(sample * channels + channel) else {
                return Err("OMT audio output buffer is too short".to_owned());
            };
            *slot = value;
        }
    }
    Ok(())
}

impl Default for Output {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::interleave;

    fn planar(values: &[f32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    /// The interleaver copies samples, it does not compute them, so the
    /// output is compared bit for bit rather than approximately.
    fn bits<const N: usize>(values: [f32; N]) -> [u32; N] {
        values.map(f32::to_bits)
    }

    #[test]
    fn a_full_ring_buffer_is_back_pressure_not_a_fault() {
        assert_eq!(super::AGAIN, 11);
    }

    /// The cushion is what stops one late audio frame from becoming an audible
    /// gap, so at the appliance's one rate it has to be the full [`PREFILL`]
    /// and it has to be a great deal more than ALSA's one-frame default.
    #[test]
    fn the_start_threshold_is_a_real_cushion() {
        // 240 ms of ring and 20 ms periods at 48 kHz.
        assert_eq!(super::prefill_frames(48_000, 11_520, 960), 4_800);
        assert_eq!(super::prefill_frames(44_100, 10_584, 882), 4_410);
    }

    /// A threshold the ring can never reach would leave playback prepared and
    /// silent forever, and one below a period would start the device with less
    /// than it wakes up on. Devices refine both sizes, so neither bound can be
    /// assumed away.
    #[test]
    fn the_start_threshold_stays_inside_what_the_device_chose() {
        // A ring smaller than the cushion: the whole ring is the threshold.
        assert_eq!(super::prefill_frames(48_000, 512, 128), 512);
        // A period larger than the cushion: one period is the floor.
        assert_eq!(super::prefill_frames(48_000, 16_384, 8_192), 8_192);
        // Nothing a device reports can make the threshold zero.
        assert_eq!(super::prefill_frames(48_000, 0, 0), 1);
        assert_eq!(super::prefill_frames(0, 11_520, 960), 960);
        assert_eq!(super::prefill_frames(-1, 11_520, 960), 960);
    }

    /// The ring is deep enough to hold a burst and the period fine enough to
    /// wake the writer inside one, and ALSA is told both in microseconds.
    #[test]
    fn the_ring_is_described_to_alsa_in_microseconds() {
        assert_eq!(super::microseconds(super::BUFFER), 240_000);
        assert_eq!(super::microseconds(super::PERIOD), 20_000);
        assert!(
            super::PREFILL < super::BUFFER,
            "the cushion cannot exceed the ring"
        );
        assert!(
            super::PERIOD < super::PREFILL,
            "the cushion cannot be one period"
        );
    }

    #[test]
    fn every_channel_is_interleaved_in_order() {
        let body = planar(&[1.0, 2.0, 3.0, -1.0, -2.0, -3.0]);
        let mut out = [0.0_f32; 6];
        interleave(&body, 0b11, 3, 2, &mut out).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(bits(out), bits([1.0, -1.0, 2.0, -2.0, 3.0, -3.0]));
    }

    #[test]
    fn an_inactive_channel_is_silence_and_consumes_no_samples() {
        // Only channel 1 is active, so the body holds its samples alone and
        // channel 0 must not read them.
        let body = planar(&[7.0, 8.0]);
        let mut out = [-1.0_f32; 4];
        interleave(&body, 0b10, 2, 2, &mut out).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(bits(out), bits([0.0, 7.0, 0.0, 8.0]));

        // The same body with the other channel active proves the cursor, not
        // the channel index, is what selects the samples.
        let mut out = [-1.0_f32; 4];
        interleave(&body, 0b01, 2, 2, &mut out).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(bits(out), bits([7.0, 0.0, 8.0, 0.0]));
    }

    #[test]
    fn silence_needs_no_body_at_all() {
        let mut out = [-1.0_f32; 4];
        interleave(&[], 0, 2, 2, &mut out).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(bits(out), bits([0.0; 4]));
    }

    #[test]
    fn a_short_body_is_refused_rather_than_read_past() {
        let body = planar(&[1.0, 2.0, 3.0]);
        let mut out = [0.0_f32; 4];
        assert!(interleave(&body, 0b11, 2, 2, &mut out).is_err());
    }

    #[test]
    fn a_short_output_is_refused_rather_than_skipped() {
        let body = planar(&[1.0, 2.0, 3.0, 4.0]);
        let mut out = [0.0_f32; 3];
        assert!(interleave(&body, 0b11, 2, 2, &mut out).is_err());
    }

    /// Above 32 the shift has no bit to test, and a channel that cannot be
    /// proven active is silence rather than a read at an unchecked offset.
    #[test]
    fn channels_past_the_mask_width_are_silent() {
        let body = planar(&[1.0; 32]);
        let mut out = [-1.0_f32; 33];
        interleave(&body, u32::MAX, 1, 33, &mut out).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(out[32].to_bits(), 0.0_f32.to_bits());
        assert!(
            out[..32]
                .iter()
                .all(|value| value.to_bits() == 1.0_f32.to_bits())
        );
    }
}
