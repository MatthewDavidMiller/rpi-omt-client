// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// Direct KMS scanout. The receiver owns the CRTC, decodes each VMX frame
// straight into a dumb buffer, and page-flips. Mode selection re-runs whenever
// the incoming video format changes, and a format the display cannot show is
// reported as unsupported rather than as a failure, so the playback loop keeps
// the connection instead of reconnecting in a loop.

use crate::channel::Frame;
use drm::buffer::Buffer;
use drm::control::{Device as ControlDevice, Mode, PageFlipFlags, connector, crtc, framebuffer};
use drm::{Device, buffer::DrmFourcc};
use omt_protocol::{VIDEO_HEADER_SIZE, VideoHeader};
use std::fs::{File, OpenOptions};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::{Duration, Instant};
use vmx_decoder::{ColorSpace, Decoder, Dimensions};

/// Buffers in the flip chain. Three lets the compositor-free pipeline keep one
/// scanning out, one queued, and one being decoded into.
const BUFFERS: usize = 3;
/// A flip that has not completed in this long means the display is gone.
const FLIP_TIMEOUT: Duration = Duration::from_millis(500);
/// `O_NONBLOCK`, which the appliance's one target (Linux) spells this way.
///
/// DRM events are delivered by reading the card. On a blocking descriptor that
/// read waits for an event that a vanished display will never send, so
/// `FLIP_TIMEOUT` and the `WouldBlock` arm of `wait_for_flip` -- both written
/// for a descriptor that returns rather than waits -- could never take effect,
/// and a lost flip would park the play loop instead of ending the session.
const O_NONBLOCK: i32 = 0o4000;
/// Decode workers. The Pi 5 has four cores and the audio worker needs one.
const DECODE_WORKERS: usize = 3;

#[derive(Debug, Eq, PartialEq)]
pub enum Present {
    Presented,
    UnsupportedFormat(String),
    Failed(String),
}

struct Card(File);

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}
impl Device for Card {}
impl ControlDevice for Card {}

struct Surface {
    buffer: drm::control::dumbbuffer::DumbBuffer,
    framebuffer: framebuffer::Handle,
}

/// The active mode and everything derived from it.
struct Configured {
    crtc: crtc::Handle,
    surfaces: Vec<Surface>,
    /// The surface the last queued flip names: scanning out, or about to be.
    front: usize,
    /// Whether that flip is still outstanding. DRM allows one per CRTC.
    flip_pending: bool,
    decoder: Decoder,
    format: VideoFormat,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct VideoFormat {
    width: i32,
    height: i32,
    frame_rate_n: i32,
    frame_rate_d: i32,
    color_space: i32,
}

impl VideoFormat {
    fn of(header: &VideoHeader) -> Self {
        Self {
            width: header.width,
            height: header.height,
            frame_rate_n: header.frame_rate_n,
            frame_rate_d: header.frame_rate_d,
            color_space: header.color_space,
        }
    }
}

pub struct Output {
    card: Card,
    connector_id: u32,
    active: Option<Configured>,
    /// The format this display has already refused, with the reason given for
    /// it. Mode selection reads the connector and its whole mode list, and an
    /// unsupported stream keeps arriving at its own frame rate, so the answer
    /// is remembered until the incoming format changes.
    rejected: Option<(VideoFormat, String)>,
}

impl Output {
    /// Opens the connector's card and confirms it can allocate dumb buffers.
    pub fn open(device: &Path, connector_id: u32) -> Result<Self, String> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(O_NONBLOCK)
            .open(device)
            .map_err(|error| format!("Failed to open DRM device: {error}"))?;
        let card = Card(file);
        match card.get_driver_capability(drm::DriverCapability::DumbBuffer) {
            Ok(value) if value != 0 => {}
            _ => return Err("DRM device does not support dumb buffers".into()),
        }
        Ok(Self {
            card,
            connector_id,
            active: None,
            rejected: None,
        })
    }

    /// Decodes and displays one video frame.
    ///
    /// The decode happens before the outstanding flip is retired. With three
    /// surfaces and the one flip DRM allows per CRTC, the target buffer is
    /// neither the one on screen nor the one queued, so decoding into it while
    /// the previous frame is still scanning out is what the third buffer is
    /// for: waiting first would idle the decoder for most of every frame
    /// interval and cap playback at whatever a serial decode-then-scan allows.
    pub fn present(&mut self, frame: &Frame) -> Present {
        let Some(header) = frame.video.as_ref() else {
            return Present::UnsupportedFormat("Unsupported video frame".into());
        };
        let format = VideoFormat::of(header);
        if let Some((rejected, detail)) = self.rejected.as_ref() {
            if *rejected == format {
                return Present::UnsupportedFormat(detail.clone());
            }
            self.rejected = None;
        }
        if self.active.as_ref().is_none_or(|a| a.format != format) {
            match self.configure(header) {
                Ok(()) => {}
                Err(Present::UnsupportedFormat(detail)) => {
                    self.rejected = Some((format, detail.clone()));
                    return Present::UnsupportedFormat(detail);
                }
                Err(outcome) => return outcome,
            }
        }
        let Some(compressed) = frame.media(VIDEO_HEADER_SIZE) else {
            return Present::Failed("Truncated VMX frame".into());
        };
        let Some(active) = self.active.as_mut() else {
            return Present::Failed("DRM output is not configured".into());
        };

        let next = (active.front + 1) % active.surfaces.len();
        let Some(surface) = active.surfaces.get_mut(next) else {
            return Present::Failed("DRM buffer is unavailable".into());
        };
        let pitch = surface.buffer.pitch() as usize;
        if let Err(error) = active.decoder.load(compressed) {
            return Present::Failed(format!("VMX decoder rejected the frame: {error}"));
        }
        // The mapping lives only for the decode. The scanout reads the buffer
        // through the GPU, so nothing needs the CPU view between frames.
        let decoded = match self.card.map_dumb_buffer(&mut surface.buffer) {
            Ok(mut mapping) => active.decoder.decode_bgrx(mapping.as_mut(), pitch),
            Err(error) => return Present::Failed(format!("Unable to map DRM buffer: {error}")),
        };
        if let Err(error) = decoded {
            return Present::Failed(format!("VMX decoder rejected the frame: {error}"));
        }

        let framebuffer = surface.framebuffer;
        let crtc = active.crtc;
        if let Err(error) = self.retire_flip() {
            return Present::Failed(error);
        }
        if let Err(error) = self
            .card
            .page_flip(crtc, framebuffer, PageFlipFlags::EVENT, None)
        {
            return Present::Failed(format!("Unable to queue DRM page flip: {error}"));
        }
        if let Some(active) = self.active.as_mut() {
            active.front = next;
            active.flip_pending = true;
        }
        Present::Presented
    }

    /// Waits for the outstanding flip, if there is one, and clears it.
    fn retire_flip(&mut self) -> Result<(), String> {
        if !self
            .active
            .as_ref()
            .is_some_and(|active| active.flip_pending)
        {
            return Ok(());
        }
        let outcome = self.wait_for_flip();
        if let Some(active) = self.active.as_mut() {
            // Cleared either way: a flip that timed out is not going to be
            // retired by a later frame, and the session is ending regardless.
            active.flip_pending = false;
        }
        outcome
    }

    /// Retires any outstanding flip and hands every DRM object back.
    ///
    /// Dropping a `Surface` releases nothing: a dumb buffer and its framebuffer
    /// are kernel objects that outlive the handle unless they are destroyed, so
    /// this is the single owner of that teardown for both the reconfiguration
    /// path and `Drop`.
    fn release(&mut self) {
        let _ = self.retire_flip();
        if let Some(active) = self.active.take() {
            for surface in active.surfaces {
                let _ = self.card.destroy_framebuffer(surface.framebuffer);
                let _ = self.card.destroy_dumb_buffer(surface.buffer);
            }
        }
    }

    fn wait_for_flip(&self) -> Result<(), String> {
        let deadline = Instant::now() + FLIP_TIMEOUT;
        while Instant::now() < deadline {
            match self.card.receive_events() {
                Ok(events) => {
                    for event in events {
                        if matches!(event, drm::control::Event::PageFlip(_)) {
                            return Ok(());
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(format!("Unable to wait for DRM page flip: {error}")),
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Err("DRM page flip timed out".into())
    }

    /// Selects a mode for the incoming format and rebuilds the flip chain.
    fn configure(&mut self, header: &VideoHeader) -> Result<(), Present> {
        // Releasing the previous chain first keeps peak allocation to one
        // mode's worth of buffers.
        self.release();

        let handle = connector::Handle::from(
            std::num::NonZeroU32::new(self.connector_id)
                .ok_or_else(|| Present::Failed("Invalid DRM connector".into()))?,
        );
        let info = self.card.get_connector(handle, false).map_err(|error| {
            Present::Failed(format!("Selected HDMI connector is unavailable: {error}"))
        })?;
        if info.state() != connector::State::Connected {
            return Err(Present::Failed(
                "Selected HDMI connector is unavailable".into(),
            ));
        }
        let crtc = info
            .current_encoder()
            .and_then(|encoder| self.card.get_encoder(encoder).ok())
            .and_then(|encoder| encoder.crtc())
            .ok_or_else(|| Present::Failed("Selected HDMI encoder is unavailable".into()))?;

        let mode = select_mode(info.modes(), header).ok_or_else(|| {
            Present::UnsupportedFormat("Display has no mode for the OMT video format".into())
        })?;

        let mut surfaces = Vec::new();
        for _ in 0..BUFFERS {
            surfaces.push(self.create_surface(&mode)?);
        }
        let first = surfaces
            .first()
            .map(|surface| surface.framebuffer)
            .ok_or_else(|| Present::Failed("Unable to create DRM buffers".into()))?;
        self.card
            .set_crtc(crtc, Some(first), (0, 0), &[handle], Some(mode))
            .map_err(|error| Present::Failed(format!("Unable to set DRM mode: {error}")))?;

        let width = usize::try_from(header.width)
            .map_err(|_| Present::UnsupportedFormat("Unsupported video width".into()))?;
        let height = usize::try_from(header.height)
            .map_err(|_| Present::UnsupportedFormat("Unsupported video height".into()))?;
        let decoder = Decoder::new(
            Dimensions { width, height },
            ColorSpace::resolve(header.color_space, height),
            DECODE_WORKERS,
        )
        .map_err(|error| {
            Present::UnsupportedFormat(format!("Unable to create the VMX decoder: {error}"))
        })?;

        self.active = Some(Configured {
            crtc,
            surfaces,
            // `set_crtc` scanned out the first surface without a flip.
            front: 0,
            flip_pending: false,
            decoder,
            format: VideoFormat::of(header),
        });
        Ok(())
    }

    fn create_surface(&self, mode: &Mode) -> Result<Surface, Present> {
        let (width, height) = mode.size();
        let buffer = self
            .card
            .create_dumb_buffer(
                (u32::from(width), u32::from(height)),
                DrmFourcc::Xrgb8888,
                32,
            )
            .map_err(|error| Present::Failed(format!("Unable to create DRM buffer: {error}")))?;
        let framebuffer = self
            .card
            .add_framebuffer(&buffer, 24, 32)
            .map_err(|error| {
                Present::Failed(format!("Unable to register DRM framebuffer: {error}"))
            })?;
        Ok(Surface {
            buffer,
            framebuffer,
        })
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        self.release();
    }
}

/// What mode selection reads from one connector mode.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ModeShape {
    width: u16,
    height: u16,
    refresh: f64,
    interlaced: bool,
}

fn shape_of(mode: &Mode) -> ModeShape {
    let (width, height) = mode.size();
    ModeShape {
        width,
        height,
        refresh: refresh_rate(mode),
        interlaced: mode.flags().contains(drm::control::ModeFlags::INTERLACE),
    }
}

/// Picks the display mode for an incoming format: the exact refresh rate, then
/// its nearest whole rate, then 60 Hz. Interlaced modes are never selected.
fn select_mode(modes: &[Mode], header: &VideoHeader) -> Option<Mode> {
    let width = u16::try_from(header.width).ok()?;
    let height = u16::try_from(header.height).ok()?;
    let requested = f64::from(header.frame_rate_n) / f64::from(header.frame_rate_d);
    let shapes: Vec<ModeShape> = modes.iter().map(shape_of).collect();
    let index = choose_mode(&shapes, width, height, requested)?;
    modes.get(index).copied()
}

/// The selection itself, over what a mode means rather than over DRM handles,
/// so the fallback order can be tested without a display.
fn choose_mode(shapes: &[ModeShape], width: u16, height: u16, requested: f64) -> Option<usize> {
    for expected in [requested, requested.round(), 60.0] {
        let found = shapes.iter().position(|shape| {
            shape.width == width
                && shape.height == height
                && !shape.interlaced
                && (shape.refresh - expected).abs() < 0.02
        });
        if found.is_some() {
            return found;
        }
    }
    None
}

fn refresh_rate(mode: &Mode) -> f64 {
    let (htotal, vtotal) = (f64::from(mode.hsync().2), f64::from(mode.vsync().2));
    if htotal == 0.0 || vtotal == 0.0 {
        return 0.0;
    }
    f64::from(mode.clock()) * 1000.0 / (htotal * vtotal)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(width: u16, height: u16, refresh: f64) -> ModeShape {
        ModeShape {
            width,
            height,
            refresh,
            interlaced: false,
        }
    }

    /// A sender's rate is rarely a mode's rate. The order matters: 59.94 has to
    /// take a 59.94 mode when one exists and a 60 Hz mode when one does not,
    /// and must never take the 1080i mode that a Pi HDMI display also offers.
    #[test]
    fn mode_selection_prefers_exact_then_rounded_then_sixty() {
        let modes = [
            ModeShape {
                interlaced: true,
                ..shape(1920, 1080, 59.94)
            },
            shape(1920, 1080, 50.0),
            shape(1920, 1080, 59.94),
            shape(1920, 1080, 60.0),
            shape(1280, 720, 59.94),
        ];
        assert_eq!(choose_mode(&modes, 1920, 1080, 59.94), Some(2));
        assert_eq!(choose_mode(&modes, 1920, 1080, 60.0), Some(3));
        assert_eq!(choose_mode(&modes, 1920, 1080, 50.0), Some(1));
        assert_eq!(choose_mode(&modes, 1280, 720, 59.94), Some(4));
        // 29.97 rounds to 30, which this display has no mode for, so the 60 Hz
        // fallback carries it.
        assert_eq!(choose_mode(&modes, 1920, 1080, 29.97), Some(3));
        // Only the interlaced mode matches, so there is nothing to select.
        assert_eq!(
            choose_mode(&modes[..1], 1920, 1080, 59.94),
            None,
            "an interlaced mode was selected"
        );
        assert_eq!(choose_mode(&modes, 1920, 1200, 60.0), None);
        assert_eq!(choose_mode(&[], 1920, 1080, 60.0), None);
    }

    #[test]
    fn a_rate_within_the_tolerance_is_the_same_mode() {
        let modes = [shape(1920, 1080, 59.9401)];
        assert_eq!(choose_mode(&modes, 1920, 1080, 59.94), Some(0));
        assert_eq!(choose_mode(&modes, 1920, 1080, 59.9), None);
    }

    /// `refresh_rate` divides by the mode's totals, so a mode with a zero total
    /// has to report a rate that never matches rather than an infinity that
    /// compares equal to nothing and a NaN that compares equal to everything.
    #[test]
    fn a_degenerate_mode_is_never_selected() {
        let modes = [shape(1920, 1080, 0.0)];
        assert_eq!(choose_mode(&modes, 1920, 1080, 60.0), None);
        assert_eq!(choose_mode(&modes, 1920, 1080, 0.0), Some(0));
    }
}
