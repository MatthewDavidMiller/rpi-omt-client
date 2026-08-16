// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// Direct KMS scanout. The receiver owns the CRTC, decodes each VMX frame
// straight into a dumb buffer, and page-flips. Mode selection re-runs whenever
// the incoming video format changes, and a format the display cannot show is
// reported as unsupported rather than as a failure, so the playback loop keeps
// the connection instead of reconnecting in a loop.
//
// A display's mode list rarely contains the sender's format. HDMI sinks
// advertise what they were built to show, not what a production switcher emits,
// and a set that stops at 720p is common on small panels and on TVs whose
// larger timings the kernel prunes. Requiring an exact match meant the whole
// picture was refused for a resolution mismatch alone, so when no mode carries
// the format the closest usable one is taken and each frame is resampled into
// it, aspect ratio preserved; see `scale.rs`.

use crate::channel::Frame;
use crate::scale::{Placement, Scaler};
use drm::buffer::Buffer;
use drm::control::{Device as ControlDevice, Mode, PageFlipFlags, connector, crtc, framebuffer};
use drm::{Device, buffer::DrmFourcc};
use omt_protocol::{VIDEO_HEADER_SIZE, VideoHeader};
use omt_receiver_core::VideoCeiling;
use std::fs::{File, OpenOptions};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::{Duration, Instant};
use vmx_decoder::{ColorSpace, DecodeError, Decoder, Dimensions, MAX_HEIGHT, MAX_WIDTH};

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
/// Decode workers. Every supported board -- Pi 3, Pi 4, Pi 5, and Zero 2 W --
/// is quad-core, and the audio worker needs one of those cores.
const DECODE_WORKERS: usize = 3;
/// How close a mode's refresh has to be to count as the rate that was asked
/// for. Displays advertise 59.94 as 59.94 and 60 as 60, so this only has to
/// absorb the rounding in the timings, not bridge two rates.
const RATE_TOLERANCE: f64 = 0.02;
/// Undecodable frames held over in a row before the session is rebuilt.
///
/// Holding the last picture is only worth it while the *next* frame might
/// decode. TCP delivers the bytes it delivers intact, so a frame the decoder
/// rejects is a sender-side glitch rather than link damage; a glitch clears
/// within a frame or two, and a run this long is a stream this receiver cannot
/// decode at all. At 60 Hz the bound is a third of a second of frozen picture,
/// which is shorter than the blink a session rebuild costs and far shorter than
/// the indefinite freeze an unbounded hold would report as healthy playback.
const SKIP_BUDGET: u32 = 20;

#[derive(Debug, Eq, PartialEq)]
pub enum Present {
    Presented,
    /// Frame-local VMX damage on a configuration that has already page-flipped,
    /// inside [`SKIP_BUDGET`]. The on-screen buffer is left alone and the play
    /// loop keeps the session, so audio never breaks for a dropped frame.
    Skipped,
    UnsupportedFormat(String),
    Failed(String),
}

/// Decode vs resample failures from [`paint`]. Decode errors stay typed so
/// [`present`] can classify them without parsing a formatted string.
enum PaintError {
    Decode(DecodeError),
    Scale(String),
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

/// The resample interposed between the decoder and the scanout buffer when the
/// selected mode is not the video's size, with the frame it decodes into.
///
/// The intermediate costs one full frame of ordinary memory -- 8 MiB at the
/// 1920x1080 maximum, against the appliance's 128 MiB container -- and it is
/// only allocated for a session that actually needs scaling. It also puts the
/// decoder's writes back on cached memory; only the resample's output crosses
/// into the write-combined scanout mapping.
struct Scaled {
    scaler: Scaler,
    frame: Vec<u8>,
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
    /// `None` when the mode carries the video's own size and the decoder can
    /// write the scanout buffer directly.
    scale: Option<Scaled>,
    format: VideoFormat,
    /// What the dashboard says while this configuration presents. Built once
    /// here rather than per frame, because the playback loop republishes the
    /// running state for every frame it displays.
    progressive_detail: String,
    interlaced_detail: String,
    /// Set after the first successful page flip for this configuration.
    /// Frame-local VMX skips are only allowed once a picture is on screen:
    /// before that there is nothing to hold but the buffer `set_crtc` scanned
    /// out, and freezing on that is worse than rebuilding the session.
    presented: bool,
    /// Undecodable frames since the last presented one, against
    /// [`SKIP_BUDGET`]. Consecutive, so a stream that loses the occasional
    /// frame is tolerated for as long as it keeps recovering.
    skips: u32,
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
    /// What this board is allowed to attempt, independent of what the attached
    /// display advertises.
    ceiling: VideoCeiling,
}

impl Output {
    /// Opens the connector's card and confirms it can allocate dumb buffers.
    pub fn open(device: &Path, connector_id: u32, ceiling: VideoCeiling) -> Result<Self, String> {
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
            ceiling,
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
        // `metadata_length` is already known not to exceed the payload, and the
        // payload is already known to hold the video header, so a body that
        // cannot be sliced out means the two overlap: the sender and this
        // receiver disagree about frame layout, which no later frame repairs.
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
        let framebuffer = surface.framebuffer;
        if let Err(error) = active.decoder.load(compressed) {
            return classify_decode(error, active.presented, &mut active.skips);
        }
        // The mapping lives only for the decode. The scanout reads the buffer
        // through the GPU, so nothing needs the CPU view between frames.
        let decoded = match self.card.map_dumb_buffer(&mut surface.buffer) {
            Ok(mut mapping) => paint(
                &mut active.decoder,
                active.scale.as_mut(),
                mapping.as_mut(),
                pitch,
            ),
            Err(error) => return Present::Failed(format!("Unable to map DRM buffer: {error}")),
        };
        if let Err(error) = decoded {
            return match error {
                PaintError::Decode(error) => {
                    classify_decode(error, active.presented, &mut active.skips)
                }
                PaintError::Scale(detail) => Present::Failed(detail),
            };
        }

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
            active.presented = true;
            active.skips = 0;
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
        // The board's ceiling is checked before anything is released. A
        // display that can show 1080p60 says nothing about whether this SoC can
        // decode it, so an over-ceiling stream is refused here rather than
        // accepted by `select_mode` and then dropped frames; and refusing it
        // without tearing down the flip chain leaves a working session intact
        // when a sender changes format mid-stream.
        let rate = if header.frame_rate_d > 0 {
            f64::from(header.frame_rate_n) / f64::from(header.frame_rate_d)
        } else {
            0.0
        };
        if let Err(detail) = self.ceiling.admits(header.width, header.height, rate) {
            return Err(Present::UnsupportedFormat(detail));
        }

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

        let selected = select_mode(info.modes(), header).ok_or_else(|| {
            Present::UnsupportedFormat(format!(
                "The display offers no usable mode for {}x{} video.",
                header.width, header.height
            ))
        })?;
        let mode = selected.mode;
        let (mode_width, mode_height) = mode.size();
        let mode_size = (usize::from(mode_width), usize::from(mode_height));

        // Build the software side before changing the CRTC. If decoder
        // allocation or worker creation fails, no newly active framebuffer
        // has to be torn down from an error path.
        let Ok(width) = usize::try_from(header.width) else {
            return Err(Present::UnsupportedFormat("Unsupported video width".into()));
        };
        let Ok(height) = usize::try_from(header.height) else {
            return Err(Present::UnsupportedFormat(
                "Unsupported video height".into(),
            ));
        };
        let decoder = Decoder::new(
            Dimensions { width, height },
            ColorSpace::resolve(header.color_space, height),
            DECODE_WORKERS,
        )
        .map_err(|error| {
            Present::UnsupportedFormat(format!("Unable to create the VMX decoder: {error}"))
        })?;
        let scale = if selected.scaled {
            Some(build_scale((width, height), mode_size)?)
        } else {
            None
        };

        let mut surfaces = Vec::new();
        for _ in 0..BUFFERS {
            match self.create_surface(&mode) {
                Ok(surface) => surfaces.push(surface),
                Err(error) => {
                    self.destroy_surfaces(surfaces);
                    return Err(error);
                }
            }
        }
        // A resample that letterboxes leaves the bars unwritten for the life of
        // the configuration. Fresh dumb buffers come back zeroed, which is
        // already black in XRGB8888, but the bars are what the operator sees
        // for as long as the session lasts, so they are cleared rather than
        // assumed.
        if scale
            .as_ref()
            .is_some_and(|scaled| !scaled.scaler.covers(mode_size))
            && let Err(error) = self.clear_surfaces(&mut surfaces)
        {
            self.destroy_surfaces(surfaces);
            return Err(error);
        }
        let Some(first) = surfaces.first().map(|surface| surface.framebuffer) else {
            self.destroy_surfaces(surfaces);
            return Err(Present::Failed("Unable to create DRM buffers".into()));
        };
        if let Err(error) = self
            .card
            .set_crtc(crtc, Some(first), (0, 0), &[handle], Some(mode))
        {
            self.destroy_surfaces(surfaces);
            return Err(Present::Failed(format!("Unable to set DRM mode: {error}")));
        }

        let scaled_to = selected.scaled.then_some(mode_size);
        self.active = Some(Configured {
            crtc,
            surfaces,
            // `set_crtc` scanned out the first surface without a flip.
            front: 0,
            flip_pending: false,
            decoder,
            scale,
            format: VideoFormat::of(header),
            progressive_detail: describe_presentation(false, (width, height), scaled_to),
            interlaced_detail: describe_presentation(true, (width, height), scaled_to),
            presented: false,
            skips: 0,
        });
        Ok(())
    }

    /// What the dashboard reports while the current configuration presents.
    ///
    /// Falls back to the plain message when nothing is configured, which the
    /// playback loop cannot observe: it only asks after a presented frame.
    pub fn presentation_detail(&self, interlaced: bool) -> &str {
        self.active.as_ref().map_or(PLAYING, |active| {
            if interlaced {
                &active.interlaced_detail
            } else {
                &active.progressive_detail
            }
        })
    }

    /// Fills every surface with black before the first frame is decoded into it.
    fn clear_surfaces(&self, surfaces: &mut [Surface]) -> Result<(), Present> {
        for surface in surfaces {
            match self.card.map_dumb_buffer(&mut surface.buffer) {
                Ok(mut mapping) => mapping.as_mut().fill(0),
                Err(error) => {
                    return Err(Present::Failed(format!(
                        "Unable to map DRM buffer: {error}"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Destroys dumb buffers and framebuffers that never reached `active`.
    ///
    /// Kernel objects outlive a dropped Rust handle, so every configure error
    /// path that already allocated surfaces must hand them back here.
    fn destroy_surfaces(&self, surfaces: Vec<Surface>) {
        for surface in surfaces {
            let _ = self.card.destroy_framebuffer(surface.framebuffer);
            let _ = self.card.destroy_dumb_buffer(surface.buffer);
        }
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
        let framebuffer = match self.card.add_framebuffer(&buffer, 24, 32) {
            Ok(framebuffer) => framebuffer,
            Err(error) => {
                let _ = self.card.destroy_dumb_buffer(buffer);
                return Err(Present::Failed(format!(
                    "Unable to register DRM framebuffer: {error}"
                )));
            }
        };
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

/// The plain running message, and the base the scaled ones extend.
const PLAYING: &str = "Playing OMT video.";
const PLAYING_INTERLACED: &str = "Playing interlaced input progressively without deinterlacing.";

/// What the dashboard reports for one configuration.
///
/// A resampled picture is softer than a native one, so the message names both
/// sizes: it is the only place the operator can see that the display, not the
/// sender, decided the resolution.
fn describe_presentation(
    interlaced: bool,
    source: (usize, usize),
    scaled_to: Option<(usize, usize)>,
) -> String {
    let base = if interlaced {
        PLAYING_INTERLACED
    } else {
        PLAYING
    };
    match scaled_to {
        None => base.to_owned(),
        Some((width, height)) => format!(
            "{base} Scaled from {}x{} to the display's {width}x{height} mode.",
            source.0, source.1
        ),
    }
}

/// Builds the resample from the video's size into the selected mode.
fn build_scale(source: (usize, usize), mode: (usize, usize)) -> Result<Scaled, Present> {
    let placement = Placement::fit(source, mode).ok_or_else(|| {
        Present::UnsupportedFormat("The display's mode cannot carry the OMT video format.".into())
    })?;
    let unsupported = || Present::UnsupportedFormat("Unsupported video dimensions".into());
    let stride = source.0.checked_mul(4).ok_or_else(unsupported)?;
    let length = stride.checked_mul(source.1).ok_or_else(unsupported)?;
    let mut frame = Vec::new();
    frame
        .try_reserve_exact(length)
        .map_err(|_| Present::Failed("Unable to reserve the scaled video frame".into()))?;
    frame.resize(length, 0);
    let scaler = Scaler::new(source, stride, placement).map_err(Present::Failed)?;
    Ok(Scaled { scaler, frame })
}

/// Decides what one decoder rejection means for the session, and counts the
/// run of held frames against [`SKIP_BUDGET`].
///
/// Only damage to this frame's own bitstream can be waited out, and only while
/// a picture is on screen to hold and the run is short enough to still be a
/// glitch. An interlaced envelope is format policy; an empty body is a framing
/// disagreement; the remaining variants are this receiver's own state. None of
/// those three improves by looking at another frame from the same sender.
fn classify_decode(error: DecodeError, presented: bool, skips: &mut u32) -> Present {
    let rejected = || format!("VMX decoder rejected the frame: {error}");
    match error {
        DecodeError::Truncated
        | DecodeError::InvalidFormat
        | DecodeError::Oversized
        | DecodeError::SliceCount
        | DecodeError::CorruptStream => {
            if !presented {
                return Present::Failed(rejected());
            }
            *skips += 1;
            if *skips <= SKIP_BUDGET {
                Present::Skipped
            } else {
                Present::Failed(format!(
                    "VMX decoder rejected {skips} consecutive frames: {error}"
                ))
            }
        }
        DecodeError::UnsupportedFormat => Present::UnsupportedFormat(rejected()),
        DecodeError::Empty
        | DecodeError::InvalidDimensions
        | DecodeError::OutputSize
        | DecodeError::WorkerFailure => Present::Failed(rejected()),
    }
}

/// Decodes the loaded frame into the mapped scanout buffer, going through the
/// intermediate frame first when the mode is not the video's size.
fn paint(
    decoder: &mut Decoder,
    scale: Option<&mut Scaled>,
    target: &mut [u8],
    pitch: usize,
) -> Result<(), PaintError> {
    match scale {
        Some(scaled) => {
            let stride = scaled.scaler.source_stride();
            decoder
                .decode_bgrx(&mut scaled.frame, stride)
                .map_err(PaintError::Decode)?;
            scaled
                .scaler
                .render(&scaled.frame, target, pitch)
                .map_err(PaintError::Scale)
        }
        None => decoder
            .decode_bgrx(target, pitch)
            .map_err(PaintError::Decode),
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

impl ModeShape {
    /// A mode this appliance will drive: progressive, with timings that yield a
    /// real refresh rate, and inside the fixed video envelope the decoder and
    /// the scanout buffers are sized for.
    ///
    /// A mode whose totals are zero has no rate to match and setting it would
    /// fail, so it is excluded here rather than left for the scaled fallback to
    /// pick when nothing else is on offer.
    fn is_usable(self) -> bool {
        !self.interlaced
            && self.refresh > 0.0
            && usize::from(self.width) <= MAX_WIDTH
            && usize::from(self.height) <= MAX_HEIGHT
    }
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

/// The mode chosen for an incoming format, and whether it has to be resampled.
struct Selected {
    mode: Mode,
    scaled: bool,
}

/// Picks the display mode for an incoming format.
fn select_mode(modes: &[Mode], header: &VideoHeader) -> Option<Selected> {
    let width = u16::try_from(header.width).ok()?;
    let height = u16::try_from(header.height).ok()?;
    let requested = f64::from(header.frame_rate_n) / f64::from(header.frame_rate_d);
    let shapes: Vec<ModeShape> = modes.iter().map(shape_of).collect();
    let choice = choose_mode(&shapes, width, height, requested)?;
    Some(Selected {
        mode: *modes.get(choice.index)?,
        scaled: choice.scaled,
    })
}

/// One mode's place in the list, and whether taking it means resampling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Choice {
    index: usize,
    scaled: bool,
}

/// The selection itself, over what a mode means rather than over DRM handles,
/// so the fallback order can be tested without a display.
///
/// A mode at the video's own size always wins, because the decoder can write
/// the scanout buffer directly. Only when the display advertises no such mode
/// does the resampled path open, and it takes the closest usable size rather
/// than refusing the picture for a resolution mismatch alone.
fn choose_mode(shapes: &[ModeShape], width: u16, height: u16, requested: f64) -> Option<Choice> {
    let usable: Vec<usize> = shapes
        .iter()
        .enumerate()
        .filter(|(_, shape)| shape.is_usable())
        .map(|(index, _)| index)
        .collect();

    let native: Vec<usize> = usable
        .iter()
        .copied()
        .filter(|&index| shapes[index].width == width && shapes[index].height == height)
        .collect();
    if let Some(index) = best_rate(shapes, &native, requested) {
        return Some(Choice {
            index,
            scaled: false,
        });
    }

    let size = scaled_size(shapes, &usable, width, height)?;
    let candidates: Vec<usize> = usable
        .iter()
        .copied()
        .filter(|&index| (shapes[index].width, shapes[index].height) == size)
        .collect();
    best_rate(shapes, &candidates, requested).map(|index| Choice {
        index,
        scaled: true,
    })
}

/// The mode size to resample into: the largest the video can be reduced into,
/// or the smallest on offer when every mode is larger than the video.
///
/// Reduction is preferred over enlargement. It is the case an appliance
/// actually meets -- a production switcher sending more than the panel can
/// show -- and it never asks the resample to write more pixels than the
/// decoder produced.
fn scaled_size(
    shapes: &[ModeShape],
    usable: &[usize],
    width: u16,
    height: u16,
) -> Option<(u16, u16)> {
    let area = |index: &usize| u32::from(shapes[*index].width) * u32::from(shapes[*index].height);
    let reduced = usable
        .iter()
        .copied()
        .filter(|&index| shapes[index].width <= width && shapes[index].height <= height)
        .max_by_key(|index| area(index));
    let chosen = match reduced {
        Some(index) => index,
        None => usable.iter().copied().min_by_key(|index| area(index))?,
    };
    Some((shapes[chosen].width, shapes[chosen].height))
}

/// The best refresh among the modes of one size: the rate that was asked for,
/// then its nearest whole rate, then 60 Hz, then the fastest on offer.
///
/// The last fallback is what keeps a display that advertises only 50 Hz from
/// refusing a 24 fps stream outright. The flip loop paces on arriving frames
/// rather than on vblank, so a mode faster than the stream costs nothing and
/// puts each frame on screen sooner.
fn best_rate(shapes: &[ModeShape], candidates: &[usize], requested: f64) -> Option<usize> {
    for expected in [requested, requested.round(), 60.0] {
        let found = candidates
            .iter()
            .copied()
            .find(|&index| (shapes[index].refresh - expected).abs() < RATE_TOLERANCE);
        if found.is_some() {
            return found;
        }
    }
    candidates.iter().copied().reduce(|best, index| {
        if shapes[index].refresh > shapes[best].refresh {
            index
        } else {
            best
        }
    })
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

    fn native(index: usize) -> Choice {
        Choice {
            index,
            scaled: false,
        }
    }

    fn scaled(index: usize) -> Choice {
        Choice {
            index,
            scaled: true,
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
        assert_eq!(choose_mode(&modes, 1920, 1080, 59.94), Some(native(2)));
        assert_eq!(choose_mode(&modes, 1920, 1080, 60.0), Some(native(3)));
        assert_eq!(choose_mode(&modes, 1920, 1080, 50.0), Some(native(1)));
        assert_eq!(choose_mode(&modes, 1280, 720, 59.94), Some(native(4)));
        // 29.97 rounds to 30, which this display has no mode for, so the 60 Hz
        // fallback carries it.
        assert_eq!(choose_mode(&modes, 1920, 1080, 29.97), Some(native(3)));
        // Only the interlaced mode is on offer, so there is nothing to select.
        assert_eq!(
            choose_mode(&modes[..1], 1920, 1080, 59.94),
            None,
            "an interlaced mode was selected"
        );
        assert_eq!(choose_mode(&[], 1920, 1080, 60.0), None);
    }

    /// The display this appliance failed on: a TV whose mode list stops at
    /// 720p, fed 1080p30. Refusing it left the operator looking at a console
    /// login, so the largest mode the frame reduces into is taken instead.
    #[test]
    fn a_format_no_mode_carries_is_scaled_into_the_largest_that_fits() {
        let modes = [
            shape(1280, 720, 60.0),
            shape(1280, 720, 59.94),
            shape(800, 600, 60.0),
            shape(640, 480, 60.0),
            ModeShape {
                interlaced: true,
                ..shape(1920, 1080, 60.0)
            },
        ];
        // 30 fps has no mode, so the 60 Hz fallback applies inside the size
        // that was chosen: 1280x720 over the smaller ones, and never the 1080i.
        assert_eq!(choose_mode(&modes, 1920, 1080, 30.0), Some(scaled(0)));
        assert_eq!(choose_mode(&modes, 1920, 1080, 59.94), Some(scaled(1)));
        // A native size still wins over any resample.
        assert_eq!(choose_mode(&modes, 1280, 720, 60.0), Some(native(0)));
    }

    /// A stream smaller than everything the display offers has to be enlarged
    /// into the smallest mode rather than refused, and a mode outside the
    /// appliance's fixed video envelope is never selected at all.
    #[test]
    fn a_small_format_takes_the_smallest_mode_and_oversized_modes_are_skipped() {
        let modes = [
            shape(1920, 1080, 60.0),
            shape(1280, 720, 60.0),
            shape(3840, 2160, 60.0),
        ];
        assert_eq!(choose_mode(&modes, 640, 480, 60.0), Some(scaled(1)));
        // 1920x1200 reduces into 1920x1080; the 4K mode is outside the envelope
        // the decoder and the scanout buffers are sized for.
        assert_eq!(choose_mode(&modes, 1920, 1200, 60.0), Some(scaled(0)));
        assert_eq!(choose_mode(&modes[2..], 1920, 1080, 60.0), None);
    }

    /// When no mode's rate is the stream's, its rounding, or 60 Hz, the fastest
    /// mode carries it. A display that offers only 50 Hz used to refuse a 24
    /// fps stream outright.
    #[test]
    fn the_fastest_mode_carries_a_rate_nothing_matches() {
        let modes = [shape(1920, 1080, 24.0), shape(1920, 1080, 50.0)];
        assert_eq!(choose_mode(&modes, 1920, 1080, 25.0), Some(native(1)));
        assert_eq!(choose_mode(&modes, 1920, 1080, 24.0), Some(native(0)));
    }

    #[test]
    fn a_rate_within_the_tolerance_is_the_same_mode() {
        let modes = [shape(1920, 1080, 59.9401), shape(1920, 1080, 60.0)];
        assert_eq!(choose_mode(&modes, 1920, 1080, 59.94), Some(native(0)));
        // 59.9 is outside the tolerance of the 59.9401 mode, so it rounds to
        // 60 and takes the mode that advertises exactly that.
        assert_eq!(choose_mode(&modes, 1920, 1080, 59.9), Some(native(1)));
    }

    /// `refresh_rate` divides by the mode's totals, so a mode with a zero total
    /// reports a rate of zero. Such a mode cannot be set, so it is excluded
    /// outright rather than left for a zero-rate stream to match.
    #[test]
    fn a_degenerate_mode_is_never_selected() {
        let modes = [shape(1920, 1080, 0.0)];
        assert_eq!(choose_mode(&modes, 1920, 1080, 60.0), None);
        assert_eq!(choose_mode(&modes, 1920, 1080, 0.0), None);
    }

    /// The running detail is what tells the operator the display, not the
    /// sender, chose the resolution, so a resampled session must say so and a
    /// native one must not.
    #[test]
    fn the_running_detail_names_a_resample() {
        assert_eq!(describe_presentation(false, (1280, 720), None), PLAYING);
        assert_eq!(
            describe_presentation(true, (1280, 720), None),
            PLAYING_INTERLACED
        );
        assert_eq!(
            describe_presentation(false, (1920, 1080), Some((1280, 720))),
            "Playing OMT video. Scaled from 1920x1080 to the display's 1280x720 mode."
        );
        assert!(
            describe_presentation(true, (1920, 1080), Some((1280, 720)))
                .starts_with(PLAYING_INTERLACED)
        );
    }

    #[test]
    fn the_card_is_opened_nonblocking() {
        assert_eq!(O_NONBLOCK, 0o4000);
    }

    const ALL_DECODE_ERRORS: [DecodeError; 10] = [
        DecodeError::InvalidDimensions,
        DecodeError::Empty,
        DecodeError::Oversized,
        DecodeError::Truncated,
        DecodeError::InvalidFormat,
        DecodeError::UnsupportedFormat,
        DecodeError::SliceCount,
        DecodeError::OutputSize,
        DecodeError::WorkerFailure,
        DecodeError::CorruptStream,
    ];

    /// Damage to one frame's bitstream, which the next frame does not inherit.
    fn frame_local(error: DecodeError) -> bool {
        matches!(
            error,
            DecodeError::Truncated
                | DecodeError::InvalidFormat
                | DecodeError::Oversized
                | DecodeError::SliceCount
                | DecodeError::CorruptStream
        )
    }

    /// Every decoder error has a home, and none of them is inferred from text.
    /// Only frame-local damage may hold the last picture, and only once a
    /// picture is on screen: an empty body, an interlaced envelope, and the
    /// decoder's own failures all repeat on the next frame.
    #[test]
    fn decode_errors_are_classified_without_parsing_strings() {
        for error in ALL_DECODE_ERRORS {
            let mut skips = 0;
            match classify_decode(error, true, &mut skips) {
                Present::Skipped => assert!(frame_local(error), "{error:?} skipped"),
                Present::UnsupportedFormat(_) => {
                    assert_eq!(error, DecodeError::UnsupportedFormat);
                }
                Present::Failed(_) => assert!(
                    !frame_local(error) && error != DecodeError::UnsupportedFormat,
                    "{error:?} must not fail when it should skip or stay unsupported"
                ),
                Present::Presented => panic!("{error:?} classified as presented"),
            }
            let mut before_first_flip = 0;
            match classify_decode(error, false, &mut before_first_flip) {
                Present::Skipped => panic!("{error:?} skipped before a frame presented"),
                Present::UnsupportedFormat(_) => {
                    assert_eq!(error, DecodeError::UnsupportedFormat);
                }
                Present::Failed(_) => assert_ne!(error, DecodeError::UnsupportedFormat),
                Present::Presented => panic!("{error:?} classified as presented"),
            }
            assert_eq!(
                before_first_flip, 0,
                "{error:?} counted a skip it was not granted"
            );
        }
    }

    /// A frozen picture is worth a glitch, not a stream that never decodes.
    #[test]
    fn a_run_of_undecodable_frames_ends_the_session() {
        let mut skips = 0;
        for held in 1..=SKIP_BUDGET {
            assert_eq!(
                classify_decode(DecodeError::CorruptStream, true, &mut skips),
                Present::Skipped,
                "frame {held} must hold the last picture"
            );
            assert_eq!(skips, held);
        }
        let Present::Failed(detail) = classify_decode(DecodeError::CorruptStream, true, &mut skips)
        else {
            panic!("a run past the budget must end the session")
        };
        assert!(
            detail.contains(&(SKIP_BUDGET + 1).to_string()) && detail.contains("CorruptStream"),
            "{detail}"
        );
    }

    /// The budget counts a *run*: a stream that keeps recovering is tolerated
    /// for as long as it does, because each held frame is a glitch of its own.
    #[test]
    fn a_presented_frame_clears_the_held_run() {
        let mut skips = 0;
        for _ in 0..SKIP_BUDGET * 4 {
            assert_eq!(
                classify_decode(DecodeError::CorruptStream, true, &mut skips),
                Present::Skipped
            );
            // What `present` does after a page flip.
            skips = 0;
        }
    }
}
