//! Direct KMS scanout. The receiver owns the CRTC, decodes each VMX frame
//! straight into a dumb buffer, and page-flips. Mode selection re-runs
//! whenever the incoming format changes, a format the display cannot show is
//! reported as unsupported rather than as a failure, and when no mode
//! carries the format the closest usable one is taken and the picture is
//! fitted into it.
//!
//! Where the display hardware can convert YCbCr itself -- the HVS on the
//! Pi 4 and Pi 5 -- the decoder writes planar 4:2:2 (half the bytes of BGRX,
//! and no colour conversion on the CPU) into a plane that the hardware
//! converts with the stream's BT.601 or BT.709 matrix and scales into the
//! mode, through atomic mode-setting. Anything that refuses that path falls
//! back to BGRX through the legacy ioctls, converted and resampled by the
//! decoder.

use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use omt_common::Error;
use omt_common::timefmt::{now_ms, remaining_ms};
use omt_protocol::{VIDEO_HEADER_SIZE, VideoHeader};
use omt_receiver_core::VideoCeiling;
use omt_sys::drm::{self, safe as ds};
use omt_sys::posix::{self, Mapping, O_NONBLOCK, POLLIN, pollfd};
use omt_vmx::{
    ColorSpace, Decoder, MAX_HEIGHT, MAX_WIDTH, Placement as VmxPlacement, PlaneMut, Status,
};

use crate::channel::Frame;
use crate::scale::{Placement, Scaler};

const PLAYING: &str = "Playing OMT video.";
const PLAYING_INTERLACED: &str = "Playing interlaced input progressively without deinterlacing.";

/// Buffers in the flip chain: one scanning out, one queued, one decoding.
pub const DRM_BUFFERS: usize = 3;
/// A flip that has not completed in this long means the display is gone.
pub const FLIP_TIMEOUT_MS: u64 = 500;
/// Decode threads. Every supported board is quad-core, and the audio worker
/// needs one of those cores.
pub const DECODE_WORKERS: usize = 3;
/// How close a mode's refresh has to be to the rate asked for.
pub const RATE_TOLERANCE: f64 = 0.02;
/// Undecodable frames held over in a row before the session is rebuilt.
pub const SKIP_BUDGET: u32 = 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Present {
    Presented,
    /// Frame-local VMX damage on a configuration that has already flipped,
    /// inside the skip budget; the on-screen buffer is left alone.
    Skipped,
    Unsupported(String),
    Failed(String),
}

/// "<what>: <os error>", the shape the presenter has always reported.
fn os_detail(what: &str, err: &io::Error) -> String {
    format!("{what}: {err}")
}

/// One dumb buffer and its framebuffer, released on drop.
#[derive(Debug)]
struct Surface {
    fd: i32,
    handle: u32,
    pitch: u32,
    size: u64,
    framebuffer: u32,
    /// Mapped once, for the life of the surface.
    map: Option<Mapping>,
    /// A YCbCr surface's three planes within the one buffer.
    plane_offsets: [u32; 3],
    plane_pitches: [u32; 3],
}

impl Surface {
    fn empty(fd: i32) -> Self {
        Self {
            fd,
            handle: 0,
            pitch: 0,
            size: 0,
            framebuffer: 0,
            map: None,
            plane_offsets: [0; 3],
            plane_pitches: [0; 3],
        }
    }

    fn create_dumb(&mut self, width: u32, height: u32, bpp: u32) -> Result<(), String> {
        let mut create = drm::drm_mode_create_dumb { width, height, bpp, ..Default::default() };
        drm::ioctl(self.fd, drm::DRM_IOCTL_MODE_CREATE_DUMB, &mut create)
            .map_err(|e| os_detail("Unable to create DRM buffer", &e))?;
        self.handle = create.handle;
        self.pitch = create.pitch;
        self.size = create.size;
        Ok(())
    }

    fn map(&mut self) -> Result<(), String> {
        let mut map = drm::drm_mode_map_dumb { handle: self.handle, ..Default::default() };
        let size =
            usize::try_from(self.size).map_err(|_| "DRM buffer is too large to map".to_owned())?;
        let mapping = drm::ioctl(self.fd, drm::DRM_IOCTL_MODE_MAP_DUMB, &mut map)
            .and_then(|()| Mapping::map_shared(self.fd, size, map.offset))
            .map_err(|e| os_detail("Unable to map DRM buffer", &e))?;
        self.map = Some(mapping);
        Ok(())
    }

    fn bytes(&mut self) -> &mut [u8] {
        self.map.as_mut().map_or(&mut [], Mapping::as_mut_slice)
    }

    /// A 32-bit BGRX buffer at the mode's size.
    fn bgrx(fd: i32, width: u16, height: u16) -> Result<Self, String> {
        let mut s = Self::empty(fd);
        s.create_dumb(u32::from(width), u32::from(height), 32)?;
        let mut fb = drm::drm_mode_fb_cmd {
            width: u32::from(width),
            height: u32::from(height),
            pitch: s.pitch,
            bpp: 32,
            depth: 24,
            handle: s.handle,
            ..Default::default()
        };
        drm::ioctl(fd, drm::DRM_IOCTL_MODE_ADDFB, &mut fb)
            .map_err(|e| os_detail("Unable to register DRM framebuffer", &e))?;
        s.framebuffer = fb.fb_id;
        s.map()?;
        Ok(s)
    }

    /// One dumb buffer holding all three planes: luma rows at the buffer's
    /// pitch, then Cb and Cr at half of it. Filled with YCbCr black, so the
    /// modeset that shows it before the first frame shows black rather than
    /// green.
    fn yuv(fd: i32, width: u16, height: u16) -> Result<Self, String> {
        let mut s = Self::empty(fd);
        s.create_dumb(u32::from(width), u32::from(height) * 2, 8)?;
        let luma_bytes = s.pitch * u32::from(height);
        let chroma_pitch = s.pitch / 2;
        if !s.pitch.is_multiple_of(2) || u64::from(luma_bytes) * 2 > s.size {
            return Err("DRM buffer pitch cannot carry planar YCbCr".to_owned());
        }
        s.plane_offsets = [0, luma_bytes, luma_bytes + chroma_pitch * u32::from(height)];
        s.plane_pitches = [s.pitch, chroma_pitch, chroma_pitch];
        let mut fb = drm::drm_mode_fb_cmd2 {
            width: u32::from(width),
            height: u32::from(height),
            pixel_format: drm::DRM_FORMAT_YUV422,
            ..Default::default()
        };
        for p in 0..3 {
            fb.handles[p] = s.handle;
            fb.pitches[p] = s.plane_pitches[p];
            fb.offsets[p] = s.plane_offsets[p];
        }
        drm::ioctl(fd, drm::DRM_IOCTL_MODE_ADDFB2, &mut fb)
            .map_err(|e| os_detail("Unable to register a YCbCr framebuffer", &e))?;
        s.framebuffer = fb.fb_id;
        s.map()?;
        let luma = luma_bytes as usize;
        let chroma = chroma_pitch as usize * usize::from(height) * 2;
        let bytes = s.bytes();
        bytes[..luma].fill(16);
        bytes[luma..luma + chroma].fill(128);
        Ok(s)
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        self.map = None;
        if self.framebuffer != 0 {
            let mut fb = self.framebuffer;
            let _ = drm::ioctl(self.fd, drm::DRM_IOCTL_MODE_RMFB, &mut fb);
        }
        if self.handle != 0 {
            let mut d = drm::drm_mode_destroy_dumb { handle: self.handle };
            let _ = drm::ioctl(self.fd, drm::DRM_IOCTL_MODE_DESTROY_DUMB, &mut d);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct VideoFormat {
    width: i32,
    height: i32,
    frame_rate_n: i32,
    frame_rate_d: i32,
    color_space: i32,
}

impl VideoFormat {
    fn of(h: &VideoHeader) -> Self {
        Self {
            width: h.width,
            height: h.height,
            frame_rate_n: h.frame_rate_n,
            frame_rate_d: h.frame_rate_d,
            color_space: h.color_space,
        }
    }
}

/// One mode's flip chain and decoder.
#[derive(Debug)]
struct Config {
    crtc: u32,
    surfaces: Vec<Surface>,
    front: usize,
    flip_pending: bool,
    decoder: Decoder,
    /// YCbCr through the display hardware; otherwise the BGRX fallback.
    yuv: bool,
    /// The plane's FB_ID property, which each YCbCr flip sets.
    plane: u32,
    plane_fb_prop: u32,
    scaler: Option<Scaler>,
    format: VideoFormat,
    progressive_detail: String,
    interlaced_detail: String,
    presented: bool,
    skips: u32,
}

#[derive(Debug)]
pub struct VideoOutput {
    // Declared before `file` so every buffer is released before the device
    // closes.
    cfg: Option<Config>,
    file: File,
    connector_id: u32,
    rejected: Option<(VideoFormat, String)>,
    ceiling: VideoCeiling,
    /// `CLOCK_MONOTONIC` instants of the last presented frame, for the
    /// opt-in latency trace.
    pub stamps: Stamps,
}

/// When the last presented frame started decoding, finished decoding, and
/// had its flip queued, and when the most recent flip reached the screen
/// (the vblank DRM reports). Nanoseconds on `CLOCK_MONOTONIC`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stamps {
    pub decode_start: u64,
    pub decoded: u64,
    pub queued: u64,
    pub vblank: u64,
}

pub fn describe_presentation(
    interlaced: bool,
    sw: usize,
    sh: usize,
    scaled: bool,
    mw: usize,
    mh: usize,
) -> String {
    let base = if interlaced { PLAYING_INTERLACED } else { PLAYING };
    if scaled {
        format!("{base} Scaled from {sw}x{sh} to the display's {mw}x{mh} mode.")
    } else {
        base.to_owned()
    }
}

/// What an undecodable frame means for the session.
pub fn classify_decode(error: Status, presented: bool, skips: &mut u32) -> Present {
    let detail = format!("VMX decoder rejected the frame: {error}");
    match error {
        Status::Truncated
        | Status::InvalidFormat
        | Status::Oversized
        | Status::SliceCount
        | Status::CorruptStream => {
            // Only damage to this frame's own bitstream can be waited out,
            // and only while a picture is on screen to hold.
            if !presented {
                return Present::Failed(detail);
            }
            *skips += 1;
            if *skips <= SKIP_BUDGET {
                return Present::Skipped;
            }
            Present::Failed(format!("VMX decoder rejected {skips} consecutive frames: {error}"))
        }
        Status::UnsupportedFormat => Present::Unsupported(detail),
        _ => Present::Failed(detail),
    }
}

/// What a mode means for selection, so selection can be tested without DRM.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModeShape {
    pub width: u16,
    pub height: u16,
    pub refresh: f64,
    pub interlaced: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeChoice {
    pub index: usize,
    pub scaled: bool,
}

fn usable(s: &ModeShape) -> bool {
    !s.interlaced
        && s.refresh > 0.0
        && usize::from(s.width) <= MAX_WIDTH
        && usize::from(s.height) <= MAX_HEIGHT
}

/// Below this, a source rate is shown at twice its rate when the display
/// offers that: 25 and 30 fps go out at 50 and 60 Hz.
pub const DOUBLED_RATE_BELOW: f64 = 40.0;

/// The best refresh among candidate modes: twice a low rate asked for, the
/// rate asked for, its nearest whole rate, then 60 Hz, then the fastest on
/// offer. Doubling repeats every frame exactly twice, so motion is as even
/// as at the source rate, while a frame waits half as long for the next
/// refresh and is scanned onto the screen in half the time.
fn best_rate(shapes: &[ModeShape], candidates: &[usize], requested: f64) -> Option<usize> {
    let doubled =
        if requested > 0.0 && requested < DOUBLED_RATE_BELOW { requested * 2.0 } else { f64::NAN };
    for expected in [doubled, requested, requested.round(), 60.0] {
        if let Some(&i) =
            candidates.iter().find(|&&i| (shapes[i].refresh - expected).abs() < RATE_TOLERANCE)
        {
            return Some(i);
        }
    }
    let mut best = *candidates.first()?;
    for &i in &candidates[1..] {
        if shapes[i].refresh > shapes[best].refresh {
            best = i;
        }
    }
    Some(best)
}

pub fn choose_mode(
    shapes: &[ModeShape],
    width: u16,
    height: u16,
    requested: f64,
) -> Option<ModeChoice> {
    let usable_list: Vec<usize> = (0..shapes.len()).filter(|&i| usable(&shapes[i])).collect();
    // A mode at the video's own size always wins: the decoder can write the
    // scanout buffer directly.
    let exact: Vec<usize> = usable_list
        .iter()
        .copied()
        .filter(|&i| shapes[i].width == width && shapes[i].height == height)
        .collect();
    if let Some(index) = best_rate(shapes, &exact, requested) {
        return Some(ModeChoice { index, scaled: false });
    }
    // Otherwise the largest size the video reduces into, or the smallest on
    // offer when every mode is larger. Ties keep the last of equal area, as
    // max_by_key does, and the first for min_by_key.
    let area = |i: usize| u32::from(shapes[i].width) * u32::from(shapes[i].height);
    let chosen = usable_list
        .iter()
        .copied()
        .filter(|&i| shapes[i].width <= width && shapes[i].height <= height)
        .max_by_key(|&i| area(i))
        .or_else(|| usable_list.iter().copied().min_by_key(|&i| area(i)))?;
    let same: Vec<usize> = usable_list
        .iter()
        .copied()
        .filter(|&i| {
            shapes[i].width == shapes[chosen].width && shapes[i].height == shapes[chosen].height
        })
        .collect();
    best_rate(shapes, &same, requested).map(|index| ModeChoice { index, scaled: true })
}

fn refresh_rate(m: &drm::drm_mode_modeinfo) -> f64 {
    if m.htotal == 0 || m.vtotal == 0 {
        return 0.0;
    }
    f64::from(m.clock) * 1000.0 / (f64::from(m.htotal) * f64::from(m.vtotal))
}

/// Reads the connector without forcing a probe: asking with a non-zero mode
/// count returns the current list rather than re-reading the sink's EDID.
fn get_connector(
    fd: i32,
    id: u32,
) -> io::Result<(drm::drm_mode_get_connector, Vec<drm::drm_mode_modeinfo>)> {
    let mut capacity = 1_usize;
    for _ in 0..4 {
        let mut modes = vec![drm::drm_mode_modeinfo::default(); capacity];
        let info = ds::get_connector(fd, id, &mut modes)?;
        let count = info.count_modes as usize;
        if count <= capacity {
            modes.truncate(count);
            return Ok((info, modes));
        }
        capacity = count.clamp(1, 1024);
    }
    Err(io::Error::from_raw_os_error(posix::EAGAIN))
}

/// Names of the properties the YCbCr path sets, per object.
const CONNECTOR_PROPS: [&str; 1] = ["CRTC_ID"];
const CRTC_PROPS: [&str; 2] = ["MODE_ID", "ACTIVE"];
const PLANE_PROPS: [&str; 12] = [
    "FB_ID",
    "CRTC_ID",
    "SRC_X",
    "SRC_Y",
    "SRC_W",
    "SRC_H",
    "CRTC_X",
    "CRTC_Y",
    "CRTC_W",
    "CRTC_H",
    "COLOR_ENCODING",
    "COLOR_RANGE",
];
const MAX_OBJECT_PROPS: usize = 64;

fn c_name(name: &[u8; 32]) -> &[u8] {
    let end = name.iter().position(|&b| b == 0).unwrap_or(31).min(31);
    &name[..end]
}

fn set_cap(fd: i32, capability: u64) -> bool {
    let mut cap = drm::drm_set_client_cap { capability, value: 1 };
    drm::ioctl(fd, drm::DRM_IOCTL_SET_CLIENT_CAP, &mut cap).is_ok()
}

/// The ids of the named properties on one object, in order; `None` when any
/// is missing. Also the plane "type" property's value, when there is one.
fn find_props<const N: usize>(
    fd: i32,
    object: u32,
    object_type: u32,
    names: &[&str; N],
) -> Option<([u32; N], Option<u64>)> {
    let mut prop_ids = [0_u32; MAX_OBJECT_PROPS];
    let mut values = [0_u64; MAX_OBJECT_PROPS];
    let count = ds::get_object_properties(fd, object, object_type, &mut prop_ids, &mut values)
        .ok()? as usize;
    if count > MAX_OBJECT_PROPS {
        return None;
    }
    let mut ids = [0_u32; N];
    let mut type_value = None;
    for i in 0..count {
        let Ok(prop) = ds::get_property(fd, prop_ids[i]) else { continue };
        let name = c_name(&prop.name);
        if name == b"type" {
            type_value = Some(values[i]);
        }
        for (n, want) in names.iter().enumerate() {
            if name == want.as_bytes() {
                ids[n] = prop_ids[i];
            }
        }
    }
    ids.iter().all(|&id| id != 0).then_some((ids, type_value))
}

/// The value an enum property gives the entry called `name`.
fn enum_value(fd: i32, prop_id: u32, name: &str) -> Option<u64> {
    let prop = ds::get_property(fd, prop_id).ok()?;
    if prop.flags & drm::DRM_MODE_PROP_ENUM == 0 || prop.count_enum_blobs > 16 {
        return None;
    }
    let mut entries = [drm::drm_mode_property_enum::default(); 16];
    let prop = ds::get_property_enums(fd, prop_id, &mut entries).ok()?;
    let n = (prop.count_enum_blobs as usize).min(16);
    entries[..n].iter().find(|e| c_name(&e.name) == name.as_bytes()).map(|e| e.value)
}

/// The primary plane that can scan YCbCr 4:2:2 out on `crtc`, and its
/// property ids.
fn find_yuv_plane(fd: i32, crtc: u32) -> Option<(u32, [u32; 12])> {
    let mut crtcs = [0_u32; 16];
    let n = ds::get_crtcs(fd, &mut crtcs).ok()? as usize;
    if n > 16 {
        return None;
    }
    let index = crtcs[..n].iter().position(|&c| c == crtc)?;
    let crtc_bit = 1_u32 << index;
    let mut planes = [0_u32; 64];
    let n = ds::get_planes(fd, &mut planes).ok()? as usize;
    if n > 64 {
        return None;
    }
    planes[..n].iter().find_map(|&plane| {
        let mut formats = [0_u32; 128];
        let info = ds::get_plane(fd, plane, &mut formats).ok()?;
        let count = info.count_format_types as usize;
        if count > 128
            || info.possible_crtcs & crtc_bit == 0
            || !formats[..count].contains(&drm::DRM_FORMAT_YUV422)
        {
            return None;
        }
        let (ids, kind) = find_props(fd, plane, drm::DRM_MODE_OBJECT_PLANE, &PLANE_PROPS)?;
        (kind == Some(drm::DRM_PLANE_TYPE_PRIMARY)).then_some((plane, ids))
    })
}

/// An atomic request under construction: objects in order, each with its
/// properties.
#[derive(Default)]
struct AtomicRequest {
    objects: Vec<u32>,
    counts: Vec<u32>,
    props: Vec<u32>,
    values: Vec<u64>,
}

impl AtomicRequest {
    fn object(&mut self, id: u32) {
        self.objects.push(id);
        self.counts.push(0);
    }

    fn prop(&mut self, id: u32, value: u64) {
        self.props.push(id);
        self.values.push(value);
        if let Some(c) = self.counts.last_mut() {
            *c += 1;
        }
    }

    fn commit(&self, fd: i32, flags: u32) -> io::Result<()> {
        ds::atomic_commit(fd, flags, &self.objects, &self.counts, &self.props, &self.values)
    }
}

struct Setup<'a> {
    fd: i32,
    connector: u32,
    crtc: u32,
    mode: &'a drm::drm_mode_modeinfo,
    width: usize,
    height: usize,
    color_space: ColorSpace,
    placement: Placement,
    scaled: bool,
}

/// Brings the mode up with the first YCbCr surface on the primary plane,
/// the picture fitted into the placement by the display hardware. `Err`
/// carries the reason when anything refuses; the caller then takes the BGRX
/// path, so nothing this leaves behind is a failure of the session.
fn configure_yuv(s: &Setup<'_>) -> Result<(Vec<Surface>, u32, u32), String> {
    let fd = s.fd;
    if !set_cap(fd, drm::DRM_CLIENT_CAP_UNIVERSAL_PLANES)
        || !set_cap(fd, drm::DRM_CLIENT_CAP_ATOMIC)
    {
        return Err("the DRM device offers no atomic mode-setting".to_owned());
    }
    let none = || "no primary plane on this display takes planar YCbCr 4:2:2".to_owned();
    let (plane, plane_ids) = find_yuv_plane(fd, s.crtc).ok_or_else(none)?;
    let (connector_ids, _) =
        find_props(fd, s.connector, drm::DRM_MODE_OBJECT_CONNECTOR, &CONNECTOR_PROPS)
            .ok_or_else(none)?;
    let (crtc_ids, _) =
        find_props(fd, s.crtc, drm::DRM_MODE_OBJECT_CRTC, &CRTC_PROPS).ok_or_else(none)?;
    let encoding_name = if s.color_space == ColorSpace::Bt601 {
        "ITU-R BT.601 YCbCr"
    } else {
        "ITU-R BT.709 YCbCr"
    };
    let (Some(encoding), Some(range)) = (
        enum_value(fd, plane_ids[10], encoding_name),
        enum_value(fd, plane_ids[11], "YCbCr limited range"),
    ) else {
        return Err("the display plane offers no matching YCbCr colour encoding".to_owned());
    };
    // At the video's own size, nearest-neighbour keeps chroma repeated
    // across each pixel pair, as the decoder's own conversion does: the
    // picture matches it within rounding. A scaled mode takes the hardware's
    // default filter, which is smoother than the nearest-neighbour resample
    // it replaces. Kernels without the property get their own default.
    let filter = find_props(fd, plane, drm::DRM_MODE_OBJECT_PLANE, &["SCALING_FILTER"]).and_then(
        |(ids, _)| {
            enum_value(fd, ids[0], if s.scaled { "Default" } else { "Nearest Neighbor" })
                .map(|v| (ids[0], v))
        },
    );
    let (w16, h16) = (u16::try_from(s.width).unwrap_or(0), u16::try_from(s.height).unwrap_or(0));
    let mut surfaces = Vec::with_capacity(DRM_BUFFERS);
    for _ in 0..DRM_BUFFERS {
        surfaces.push(Surface::yuv(fd, w16, h16)?);
    }
    let blob = ds::create_mode_blob(fd, s.mode)
        .map_err(|e| os_detail("Unable to describe the DRM mode", &e))?;
    let mut r = AtomicRequest::default();
    r.object(s.connector);
    r.prop(connector_ids[0], u64::from(s.crtc));
    r.object(s.crtc);
    r.prop(crtc_ids[0], u64::from(blob));
    r.prop(crtc_ids[1], 1);
    r.object(plane);
    let p = s.placement;
    let plane_values: [u64; 12] = [
        u64::from(surfaces[0].framebuffer),
        u64::from(s.crtc),
        0,
        0,
        (s.width as u64) << 16,
        (s.height as u64) << 16,
        p.x as u64,
        p.y as u64,
        p.width as u64,
        p.height as u64,
        encoding,
        range,
    ];
    for (id, value) in plane_ids.iter().zip(plane_values) {
        r.prop(*id, value);
    }
    if let Some((id, value)) = filter {
        r.prop(id, value);
    }
    // Asked first, so a refusal leaves the display exactly as it was.
    let result = r
        .commit(fd, drm::DRM_MODE_ATOMIC_TEST_ONLY | drm::DRM_MODE_ATOMIC_ALLOW_MODESET)
        .map_err(|e| os_detail("the display hardware refused YCbCr scanout", &e))
        .and_then(|()| {
            r.commit(fd, drm::DRM_MODE_ATOMIC_ALLOW_MODESET)
                .map_err(|e| os_detail("Unable to set DRM mode for YCbCr scanout", &e))
        });
    // The committed state holds its own reference to the mode.
    let mut destroy = drm::drm_mode_destroy_blob { blob_id: blob };
    let _ = drm::ioctl(fd, drm::DRM_IOCTL_MODE_DESTROYPROPBLOB, &mut destroy);
    result.map(|()| (surfaces, plane, plane_ids[0]))
}

impl VideoOutput {
    pub fn open(device: &Path, connector_id: u32, ceiling: &VideoCeiling) -> Result<Self, Error> {
        // Non-blocking, so waiting for a flip event is bounded by poll rather
        // than parked on a read a vanished display will never answer.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(O_NONBLOCK)
            .open(device)
            .map_err(|e| Error::new(format!("Failed to open DRM device: {e}")))?;
        let mut cap = drm::drm_get_cap { capability: drm::DRM_CAP_DUMB_BUFFER, value: 0 };
        if drm::ioctl(file.as_raw_fd(), drm::DRM_IOCTL_GET_CAP, &mut cap).is_err() || cap.value == 0
        {
            return Err(Error::new("DRM device does not support dumb buffers"));
        }
        Ok(Self {
            cfg: None,
            file,
            connector_id,
            rejected: None,
            ceiling: ceiling.clone(),
            stamps: Stamps::default(),
        })
    }

    fn fd(&self) -> i32 {
        self.file.as_raw_fd()
    }

    pub fn presentation_detail(&self, interlaced: bool) -> &str {
        match &self.cfg {
            None => PLAYING,
            Some(c) if interlaced => &c.interlaced_detail,
            Some(c) => &c.progressive_detail,
        }
    }

    fn wait_for_flip(&mut self) -> Result<(), String> {
        let deadline = now_ms() + FLIP_TIMEOUT_MS;
        loop {
            let mut buffer = [0_u8; 1024];
            match self.file.read(&mut buffer) {
                Ok(n) if n > 0 => {
                    let mut at = 0;
                    while n - at >= 8 {
                        let kind = u32::from_ne_bytes([
                            buffer[at],
                            buffer[at + 1],
                            buffer[at + 2],
                            buffer[at + 3],
                        ]);
                        let length = u32::from_ne_bytes([
                            buffer[at + 4],
                            buffer[at + 5],
                            buffer[at + 6],
                            buffer[at + 7],
                        ]) as usize;
                        if length < 8 || length > n - at {
                            break;
                        }
                        if kind == drm::DRM_EVENT_FLIP_COMPLETE {
                            // drm_event_vblank: tv_sec and tv_usec follow the
                            // 8-byte header and the 8-byte user data.
                            if length >= 24 {
                                let word = |o: usize| {
                                    u64::from(u32::from_ne_bytes([
                                        buffer[at + o],
                                        buffer[at + o + 1],
                                        buffer[at + o + 2],
                                        buffer[at + o + 3],
                                    ]))
                                };
                                self.stamps.vblank = word(16) * 1_000_000_000 + word(20) * 1000;
                            }
                            return Ok(());
                        }
                        at += length;
                    }
                    continue;
                }
                Ok(_) => {}
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
                Err(e) => return Err(os_detail("Unable to wait for DRM page flip", &e)),
            }
            let left = remaining_ms(deadline);
            if left == 0 {
                return Err("DRM page flip timed out".to_owned());
            }
            let mut fds = [pollfd { fd: self.fd(), events: POLLIN, revents: 0 }];
            let _ = posix::poll_fds(&mut fds, i32::try_from(left).unwrap_or(i32::MAX));
        }
    }

    fn retire_flip(&mut self) -> Result<(), String> {
        if !self.cfg.as_ref().is_some_and(|c| c.flip_pending) {
            return Ok(());
        }
        let result = self.wait_for_flip();
        // Cleared either way: a flip that timed out is not going to be
        // retired by a later frame, and the session is ending regardless.
        if let Some(c) = self.cfg.as_mut() {
            c.flip_pending = false;
        }
        result
    }

    /// Retires any outstanding flip and hands every DRM object back.
    fn release(&mut self) {
        let _ = self.retire_flip();
        self.cfg = None;
    }

    /// Selects a mode for the incoming format and rebuilds the flip chain.
    fn configure(&mut self, h: &VideoHeader) -> Present {
        // The board's ceiling is checked before anything is released, so an
        // over-ceiling format change leaves a working session intact.
        let rate = if h.frame_rate_d > 0 {
            f64::from(h.frame_rate_n) / f64::from(h.frame_rate_d)
        } else {
            0.0
        };
        if let Err(e) = self.ceiling.admits(h.width, h.height, rate) {
            return Present::Unsupported(e.to_string());
        }
        // Releasing the previous chain first keeps peak allocation to one
        // mode's worth of buffers.
        self.release();
        let fd = self.fd();
        if self.connector_id == 0 {
            return Present::Failed("Invalid DRM connector".to_owned());
        }
        let (info, modes) = match get_connector(fd, self.connector_id) {
            Ok(x) => x,
            Err(e) => {
                return Present::Failed(os_detail("Selected HDMI connector is unavailable", &e));
            }
        };
        if info.connection != drm::DRM_MODE_CONNECTED {
            return Present::Failed("Selected HDMI connector is unavailable".to_owned());
        }
        let mut encoder =
            drm::drm_mode_get_encoder { encoder_id: info.encoder_id, ..Default::default() };
        if info.encoder_id == 0
            || drm::ioctl(fd, drm::DRM_IOCTL_MODE_GETENCODER, &mut encoder).is_err()
            || encoder.crtc_id == 0
        {
            return Present::Failed("Selected HDMI encoder is unavailable".to_owned());
        }
        let crtc = encoder.crtc_id;
        let shapes: Vec<ModeShape> = modes
            .iter()
            .map(|m| ModeShape {
                width: m.hdisplay,
                height: m.vdisplay,
                refresh: refresh_rate(m),
                interlaced: m.flags & drm::DRM_MODE_FLAG_INTERLACE != 0,
            })
            .collect();
        let (w16, h16) =
            (u16::try_from(h.width).unwrap_or(0), u16::try_from(h.height).unwrap_or(0));
        let Some(choice) = choose_mode(&shapes, w16, h16, rate) else {
            return Present::Unsupported(format!(
                "The display offers no usable mode for {}x{} video.",
                h.width, h.height
            ));
        };
        let mode = modes[choice.index];
        let (mode_w, mode_h) = (usize::from(mode.hdisplay), usize::from(mode.vdisplay));
        let (width, height) = (h.width.unsigned_abs() as usize, h.height.unsigned_abs() as usize);
        let color_space = ColorSpace::resolve(h.color_space, height);
        // The software side is built before the CRTC changes, so a failure
        // here has no newly active framebuffer to tear down.
        let decoder = match Decoder::new(width, height, color_space, DECODE_WORKERS) {
            Ok(d) => d,
            Err(e) => {
                return Present::Unsupported(format!("Unable to create the VMX decoder: {e}"));
            }
        };
        let placement = if choice.scaled {
            match Placement::fit(width, height, mode_w, mode_h) {
                Some(p) => p,
                None => {
                    return Present::Unsupported(
                        "The display's mode cannot carry the OMT video format.".to_owned(),
                    );
                }
            }
        } else {
            Placement { x: 0, y: 0, width: mode_w, height: mode_h }
        };
        let setup = Setup {
            fd,
            connector: self.connector_id,
            crtc,
            mode: &mode,
            width,
            height,
            color_space,
            placement,
            scaled: choice.scaled,
        };
        let mut cfg = Config {
            crtc,
            surfaces: Vec::new(),
            front: 0,
            flip_pending: false,
            decoder,
            yuv: false,
            plane: 0,
            plane_fb_prop: 0,
            scaler: None,
            format: VideoFormat::of(h),
            progressive_detail: describe_presentation(
                false,
                width,
                height,
                choice.scaled,
                mode_w,
                mode_h,
            ),
            interlaced_detail: describe_presentation(
                true,
                width,
                height,
                choice.scaled,
                mode_w,
                mode_h,
            ),
            presented: false,
            skips: 0,
        };
        match configure_yuv(&setup) {
            Ok((surfaces, plane, fb_prop)) => {
                eprintln!(
                    "Display path: planar YCbCr 4:2:2, converted{} by the display hardware.",
                    if choice.scaled { " and scaled" } else { "" }
                );
                cfg.surfaces = surfaces;
                cfg.plane = plane;
                cfg.plane_fb_prop = fb_prop;
                cfg.yuv = true;
            }
            Err(why) => {
                // Whatever the YCbCr attempt built is gone (its surfaces
                // dropped) before the fallback builds its own, so peak
                // allocation stays one chain.
                eprintln!("Display path: BGRX converted by the decoder ({why}).");
                if choice.scaled {
                    match Scaler::new(width, height, placement) {
                        Ok(s) => cfg.scaler = Some(s),
                        Err(e) => return Present::Failed(e.to_string()),
                    }
                }
                for _ in 0..DRM_BUFFERS {
                    match Surface::bgrx(fd, mode.hdisplay, mode.vdisplay) {
                        Ok(s) => cfg.surfaces.push(s),
                        Err(detail) => return Present::Failed(detail),
                    }
                }
                // Letterbox bars stay unwritten for the configuration's
                // life, so they are cleared rather than assumed black.
                if choice.scaled && !placement.fills(mode_w, mode_h) {
                    for s in &mut cfg.surfaces {
                        s.bytes().fill(0);
                    }
                }
                if let Err(e) =
                    ds::set_crtc(fd, crtc, cfg.surfaces[0].framebuffer, &[self.connector_id], &mode)
                {
                    return Present::Failed(os_detail("Unable to set DRM mode", &e));
                }
            }
        }
        // The modeset scanned out the first surface without a flip.
        self.cfg = Some(cfg);
        Present::Presented
    }

    /// Decodes and displays one frame.
    pub fn present(&mut self, frame: &Frame) -> Present {
        let Some(h) = frame.video else {
            return Present::Unsupported("Unsupported video frame".to_owned());
        };
        let format = VideoFormat::of(&h);
        if let Some((rejected, detail)) = &self.rejected {
            if *rejected == format {
                return Present::Unsupported(detail.clone());
            }
            self.rejected = None;
        }
        if !self.cfg.as_ref().is_some_and(|c| c.format == format) {
            match self.configure(&h) {
                Present::Presented => {}
                Present::Unsupported(detail) => {
                    // Remembered until the format changes: mode selection
                    // reads the whole connector, and the stream keeps
                    // arriving.
                    self.rejected = Some((format, detail.clone()));
                    return Present::Unsupported(detail);
                }
                other => return other,
            }
        }
        let Some(compressed) = frame.media(VIDEO_HEADER_SIZE as usize) else {
            return Present::Failed("Truncated VMX frame".to_owned());
        };
        let Some(c) = self.cfg.as_mut() else {
            return Present::Failed("Video output is not configured".to_owned());
        };
        let next = (c.front + 1) % c.surfaces.len();
        self.stamps.decode_start = posix::monotonic_ns();
        if let Err(e) = c.decoder.load(compressed) {
            return classify_decode(e, c.presented, &mut c.skips);
        }
        // Decoding happens before the outstanding flip is retired: with three
        // surfaces the target is neither on screen nor queued, which is what
        // the third buffer is for. YCbCr is copied out plane by plane and
        // converted and scaled by the display; the BGRX fallback converts in
        // the decoder, and a scaled mode resamples there too, on the whole
        // worker pool.
        let surface = &mut c.surfaces[next];
        let (offsets, pitches, pitch) =
            (surface.plane_offsets, surface.plane_pitches, surface.pitch as usize);
        let bytes = surface.bytes();
        let decoded = if c.yuv {
            let (o1, o2) = (offsets[1] as usize, offsets[2] as usize);
            let (luma, rest) = bytes.split_at_mut(o1.min(bytes.len()));
            let (cb, cr) = rest.split_at_mut((o2 - o1).min(rest.len()));
            c.decoder.decode_yuv422p([
                PlaneMut { data: luma, stride: pitches[0] as usize },
                PlaneMut { data: cb, stride: pitches[1] as usize },
                PlaneMut { data: cr, stride: pitches[2] as usize },
            ])
        } else if let Some(s) = &c.scaler {
            let p = VmxPlacement {
                x: s.placement.x,
                y: s.placement.y,
                columns: &s.columns,
                rows: &s.rows,
            };
            c.decoder.decode_bgrx_placed(bytes, pitch, &p)
        } else {
            c.decoder.decode_bgrx(bytes, pitch)
        };
        if let Err(e) = decoded {
            return classify_decode(e, c.presented, &mut c.skips);
        }
        let framebuffer = c.surfaces[next].framebuffer;
        self.stamps.decoded = posix::monotonic_ns();
        if let Err(detail) = self.retire_flip() {
            return Present::Failed(detail);
        }
        let fd = self.fd();
        let Some(c) = self.cfg.as_mut() else {
            return Present::Failed("Video output is not configured".to_owned());
        };
        let queued = if c.yuv {
            // A flip is the plane's framebuffer and nothing else.
            let mut r = AtomicRequest::default();
            r.object(c.plane);
            r.prop(c.plane_fb_prop, u64::from(framebuffer));
            r.commit(fd, drm::DRM_MODE_ATOMIC_NONBLOCK | drm::DRM_MODE_PAGE_FLIP_EVENT)
        } else {
            let mut flip = drm::drm_mode_crtc_page_flip {
                crtc_id: c.crtc,
                fb_id: framebuffer,
                flags: drm::DRM_MODE_PAGE_FLIP_EVENT,
                ..Default::default()
            };
            drm::ioctl(fd, drm::DRM_IOCTL_MODE_PAGE_FLIP, &mut flip)
        };
        if let Err(e) = queued {
            return Present::Failed(os_detail("Unable to queue DRM page flip", &e));
        }
        self.stamps.queued = posix::monotonic_ns();
        c.front = next;
        c.flip_pending = true;
        c.presented = true;
        c.skips = 0;
        Present::Presented
    }
}

impl Drop for VideoOutput {
    fn drop(&mut self) {
        self.release();
    }
}
