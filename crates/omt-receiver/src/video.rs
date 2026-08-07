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
use std::path::Path;
use std::time::{Duration, Instant};
use vmx_decoder::{ColorSpace, Decoder, Dimensions};

/// Buffers in the flip chain. Three lets the compositor-free pipeline keep one
/// scanning out, one queued, and one being decoded into.
const BUFFERS: usize = 3;
/// A flip that has not completed in this long means the display is gone.
const FLIP_TIMEOUT: Duration = Duration::from_millis(500);
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
    front: usize,
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
}

impl Output {
    /// Opens the connector's card and confirms it can allocate dumb buffers.
    pub fn open(device: &Path, connector_id: u32) -> Result<Self, String> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
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
        })
    }

    /// Decodes and displays one video frame.
    pub fn present(&mut self, frame: &Frame) -> Present {
        let Some(header) = frame.video.as_ref() else {
            return Present::UnsupportedFormat("Unsupported video frame".into());
        };
        let format = VideoFormat::of(header);
        if self.active.as_ref().is_none_or(|a| a.format != format) {
            match self.configure(header) {
                Ok(()) => {}
                Err(outcome) => return outcome,
            }
        }
        let Some(active) = self.active.as_mut() else {
            return Present::Failed("DRM output is not configured".into());
        };
        let Some(compressed) = frame.media(VIDEO_HEADER_SIZE) else {
            return Present::Failed("Truncated VMX frame".into());
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
        if let Err(error) = self
            .card
            .page_flip(crtc, framebuffer, PageFlipFlags::EVENT, None)
        {
            return Present::Failed(format!("Unable to queue DRM page flip: {error}"));
        }
        if let Err(error) = self.wait_for_flip() {
            return Present::Failed(error);
        }
        if let Some(active) = self.active.as_mut() {
            active.front = next;
        }
        Present::Presented
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
        self.active = None;

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
            front: 0,
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
        if let Some(active) = self.active.take() {
            for surface in active.surfaces {
                let _ = self.card.destroy_framebuffer(surface.framebuffer);
                let _ = self.card.destroy_dumb_buffer(surface.buffer);
            }
        }
    }
}

/// Picks the display mode for an incoming format: the exact refresh rate, then
/// its nearest whole rate, then 60 Hz. Interlaced modes are never selected.
fn select_mode(modes: &[Mode], header: &VideoHeader) -> Option<Mode> {
    let width = u16::try_from(header.width).ok()?;
    let height = u16::try_from(header.height).ok()?;
    let requested = f64::from(header.frame_rate_n) / f64::from(header.frame_rate_d);
    for expected in [requested, requested.round(), 60.0] {
        let found = modes.iter().find(|mode| {
            mode.size() == (width, height)
                && !mode.flags().contains(drm::control::ModeFlags::INTERLACE)
                && (refresh_rate(mode) - expected).abs() < 0.02
        });
        if let Some(mode) = found {
            return Some(*mode);
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
