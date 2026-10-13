//! Decode-only VMX1 for the appliance. The encoder, the preview resolutions,
//! and the unused colour-conversion outputs of the reference codec are
//! intentionally absent; see third_party/omt/PROVENANCE.md.
//!
//! A [`Decoder`] is built for one fixed frame geometry. Frames are loaded
//! (the envelope is validated and split into per-slice bitstreams) and then
//! decoded into packed UYVY or BGRX, planar YUV 4:2:2, or BGRX resampled
//! into a placement. Slices are decoded in parallel by a bounded pool; the
//! calling thread is one of the workers, so a pool of N decodes on N threads
//! with N-1 spawned.
//!
//! Slices are claimed one at a time from a shared counter rather than handed
//! out as fixed bands: slice cost follows picture detail, so fixed bands
//! made every frame wait for whichever worker drew the busiest part of the
//! picture.

// The decoder transcribes a codec defined in SSE2 integer arithmetic: its
// results are specified by 16-bit truncations, sign reinterpretations, and
// wraps, so those casts are the definition here, not accidents -- the
// conformance vectors pin every one of them. The always-inline helpers are
// the lane loops and bit-reader steps whose inlining was measured to matter
// (see the benchmark notes in docs/ARCHITECTURE.md).
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::inline_always
)]

#[rustfmt::skip] // generated; tools/gen/gen_vmx_tables.py --check holds it
mod ac_lookahead;
#[cfg(target_arch = "aarch64")]
mod asm;
mod bits;
mod convert;
mod idct;
#[cfg(target_arch = "x86_64")]
mod idct_sse2;
mod output;
mod plane;
mod tables;

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};

use bits::Stream;
use output::{Out, PlacementRef};
use tables::SLICE_HEIGHT;

pub const WORKER_STACK_SIZE: usize = 128 * 1024;
pub const MAX_WIDTH: usize = 1920;
pub const MAX_HEIGHT: usize = 1080;
pub const MAX_COMPRESSED_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_WORKERS: usize = 8;
/// The largest placed area a decode accepts, which bounds each worker's row
/// buffer. Wider than any mode a supported board drives.
pub const MAX_PLACED_WIDTH: usize = 8192;
pub const MAX_PLACED_HEIGHT: usize = 8192;

const FORMAT_PROGRESSIVE: u8 = 1;
const FORMAT_INTERLACED: u8 = 2;
const FORMAT_EXTENDED: u8 = 3;

/// Why a decode failed. The names are the ones status files and logs carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    InvalidDimensions,
    Empty,
    Oversized,
    Truncated,
    InvalidFormat,
    UnsupportedFormat,
    SliceCount,
    OutputSize,
    WorkerFailure,
    CorruptStream,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Status::InvalidDimensions => "InvalidDimensions",
            Status::Empty => "Empty",
            Status::Oversized => "Oversized",
            Status::Truncated => "Truncated",
            Status::InvalidFormat => "InvalidFormat",
            Status::UnsupportedFormat => "UnsupportedFormat",
            Status::SliceCount => "SliceCount",
            Status::OutputSize => "OutputSize",
            Status::WorkerFailure => "WorkerFailure",
            Status::CorruptStream => "CorruptStream",
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl std::error::Error for Status {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSpace {
    Bt601,
    Bt709,
}

impl ColorSpace {
    /// Mirrors the reference codec's undefined-colour-space default.
    pub fn resolve(value: i32, height: usize) -> Self {
        match value {
            601 => ColorSpace::Bt601,
            709 => ColorSpace::Bt709,
            _ if height >= 720 => ColorSpace::Bt709,
            _ => ColorSpace::Bt601,
        }
    }

    fn coefficients(self) -> &'static [i16; 5] {
        match self {
            ColorSpace::Bt601 => &tables::YUV_RGB_601,
            ColorSpace::Bt709 => &tables::YUV_RGB_709,
        }
    }
}

/// One plane of a planar output: rows `stride` bytes apart.
#[derive(Debug)]
pub struct PlaneMut<'a> {
    pub data: &'a mut [u8],
    pub stride: usize,
}

/// Nearest-neighbour placement of the picture into a destination of another
/// size: destination pixel (x + i, y + j) takes source pixel (columns[i],
/// rows[j]). `rows` must be non-decreasing; every entry must lie inside the
/// decoder's geometry.
#[derive(Clone, Copy, Debug)]
pub struct Placement<'a> {
    pub x: usize,
    pub y: usize,
    pub columns: &'a [u32],
    pub rows: &'a [u32],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pixels {
    Uyvy,
    Bgrx,
    Yuv422p,
}

/// One slice's streams and the rows of it inside the visible image.
#[derive(Debug)]
struct Slice {
    dc: Stream,
    ac: Stream,
    rows: usize,
    /// For a placed decode, the destination rows this slice fills.
    placed: (usize, usize),
}

/// Reused YUV planes for one slice at a time, and for a placed decode, one
/// converted source row and one gathered destination row.
#[derive(Debug)]
struct Scratch {
    luma: Vec<u8>,
    blue: Vec<u8>,
    red: Vec<u8>,
    source_row: Vec<u8>,
    placed_row: Vec<u8>,
}

/// What every worker decodes for one frame. It is published under the pool
/// lock, so every write that set it up happens-before the workers read it.
#[derive(Clone, Copy, Debug)]
struct Job {
    pixels: Pixels,
    matrix: [u16; 64],
    dc_shift: i32,
    coefficients: &'static [i16; 5],
    output: Out,
    planes: [Out; 3],
    plane_strides: [usize; 3],
    stride: usize,
    placement: Option<PlacementRef>,
}

#[derive(Debug)]
struct PoolState {
    generation: u64,
    running: usize,
    shutdown: bool,
    job: Option<Job>,
}

#[derive(Debug)]
struct Shared {
    width: usize,
    luma_stride: usize,
    chroma_stride: usize,
    /// Each slice index is claimed by exactly one thread per frame, so these
    /// locks are never contended; they make the hand-over between frames
    /// and threads explicit.
    slices: Vec<Padded<Mutex<Slice>>>,
    /// `scratch[0]` is the calling thread's; `scratch[i]` is worker i's.
    scratch: Vec<Padded<Mutex<Scratch>>>,
    state: Mutex<PoolState>,
    start: Condvar,
    finished: Condvar,
    next: AtomicUsize,
    failed: AtomicBool,
}

/// A value on cache lines of its own. Neighbouring slices are decoded by
/// different threads at the same time, and their locks and stream headers
/// sharing a line made the cores trade it back and forth on every claim.
#[derive(Debug)]
#[repr(align(128))]
struct Padded<T>(T);

impl<T> std::ops::Deref for Padded<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug)]
pub struct Decoder {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
    height: usize,
    color_space: ColorSpace,
    matrix: [u16; 64],
    dc_shift: i32,
    loaded: bool,
    placed_capacity: usize,
}

const fn align8(v: usize) -> usize {
    (v + 7) & !7
}

impl Decoder {
    pub fn new(
        width: usize,
        height: usize,
        color_space: ColorSpace,
        workers: usize,
    ) -> Result<Self, Status> {
        if !(16..=MAX_WIDTH).contains(&width)
            || !width.is_multiple_of(2)
            || !(16..=MAX_HEIGHT).contains(&height)
            || !(1..=MAX_WORKERS).contains(&workers)
        {
            return Err(Status::InvalidDimensions);
        }
        let luma_stride = align8(width);
        let chroma_stride = align8(width / 2);
        let slice_count = height.div_ceil(SLICE_HEIGHT);
        // The reference codec sizes the per-slice streams from the luma stride.
        let dc_capacity = luma_stride * SLICE_HEIGHT * 2;
        let ac_capacity = luma_stride * SLICE_HEIGHT * 4;
        let slices = (0..slice_count)
            .map(|i| {
                let visible = height.saturating_sub(i * SLICE_HEIGHT);
                Padded(Mutex::new(Slice {
                    dc: Stream::new(dc_capacity),
                    ac: Stream::new(ac_capacity),
                    rows: visible.min(SLICE_HEIGHT),
                    placed: (0, 0),
                }))
            })
            .collect();
        let workers = workers.min(slice_count);
        let scratch = (0..workers)
            .map(|_| {
                Padded(Mutex::new(Scratch {
                    luma: vec![0; luma_stride * SLICE_HEIGHT],
                    blue: vec![0; chroma_stride * SLICE_HEIGHT],
                    red: vec![0; chroma_stride * SLICE_HEIGHT],
                    source_row: vec![0; width * 4],
                    placed_row: Vec::new(),
                }))
            })
            .collect();
        let shared = Arc::new(Shared {
            width,
            luma_stride,
            chroma_stride,
            slices,
            scratch,
            state: Mutex::new(PoolState { generation: 0, running: 0, shutdown: false, job: None }),
            start: Condvar::new(),
            finished: Condvar::new(),
            next: AtomicUsize::new(0),
            failed: AtomicBool::new(false),
        });
        let mut decoder = Self {
            shared,
            threads: Vec::with_capacity(workers.saturating_sub(1)),
            height,
            color_space,
            matrix: tables::decode_matrix(0),
            dc_shift: 0,
            loaded: false,
            placed_capacity: 0,
        };
        // Workers never take a process signal; shutdown is the main thread's.
        #[cfg(unix)]
        let previous = omt_sys::posix::block_all_signals();
        let mut spawned = true;
        for i in 1..workers {
            let shared = Arc::clone(&decoder.shared);
            let built = thread::Builder::new()
                .name(format!("vmx-decode-{}", (i - 1) % MAX_WORKERS))
                .stack_size(WORKER_STACK_SIZE)
                .spawn(move || worker_main(&shared, i));
            if let Ok(handle) = built {
                decoder.threads.push(handle);
            } else {
                spawned = false;
                break;
            }
        }
        #[cfg(unix)]
        omt_sys::posix::restore_signal_mask(&previous);
        if !spawned {
            return Err(Status::WorkerFailure);
        }
        Ok(decoder)
    }

    pub fn width(&self) -> usize {
        self.shared.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// VMX_LoadFrom: validates the envelope and splits it into slice streams.
    pub fn load(&mut self, input: &[u8]) -> Result<(), Status> {
        self.loaded = false;
        if input.is_empty() {
            return Err(Status::Empty);
        }
        if input.len() > MAX_COMPRESSED_BYTES {
            return Err(Status::Oversized);
        }
        if input.len() < 5 {
            return Err(Status::Truncated);
        }
        match input[0] {
            FORMAT_INTERLACED => return Err(Status::UnsupportedFormat),
            FORMAT_PROGRESSIVE | FORMAT_EXTENDED => {}
            _ => return Err(Status::InvalidFormat),
        }
        let (offset, dc_shift) =
            if input[0] == FORMAT_EXTENDED { (2, i32::from(input[1])) } else { (0, 0) };
        if offset + 2 >= input.len() {
            return Err(Status::Truncated);
        }
        // Interlaced frames use the reference's field-paired slice layout,
        // which the appliance's progressive pipeline never emits.
        if input[offset] != FORMAT_PROGRESSIVE {
            return Err(Status::UnsupportedFormat);
        }
        let quality = i32::from(input[offset + 1]);
        if usize::from(input[offset + 2]) != self.shared.slices.len() {
            return Err(Status::SliceCount);
        }
        let mut cursor = 3 + offset;
        for slice in &self.shared.slices {
            let data = take_stream(input, &mut cursor)?;
            if !lock(slice).dc.load(data) {
                return Err(Status::Oversized);
            }
        }
        // A preview-only frame carries the DC streams alone.
        let has_ac = cursor < input.len();
        for slice in &self.shared.slices {
            let data = if has_ac { take_stream(input, &mut cursor)? } else { &[] };
            if !lock(slice).ac.load(data) {
                return Err(Status::Oversized);
            }
        }
        self.matrix = tables::decode_matrix(tables::quality_index(quality));
        self.dc_shift = dc_shift;
        self.loaded = true;
        Ok(())
    }

    pub fn decode_uyvy(&mut self, output: &mut [u8], stride: usize) -> Result<(), Status> {
        self.check_packed(output.len(), stride, self.shared.width * 2)?;
        self.run(Pixels::Uyvy, Out::new(output), stride, [Out::EMPTY; 3], [0; 3], None)
    }

    /// BGRX is the byte order the DRM scanout reads as XRGB8888.
    pub fn decode_bgrx(&mut self, output: &mut [u8], stride: usize) -> Result<(), Status> {
        self.check_packed(output.len(), stride, self.shared.width * 4)?;
        self.run(Pixels::Bgrx, Out::new(output), stride, [Out::EMPTY; 3], [0; 3], None)
    }

    /// Planar YUV 4:2:2 as the decoder holds it: luma, then Cb, then Cr at
    /// half width, each row copied once with no conversion. This is
    /// DRM_FORMAT_YUV422, which display hardware that converts YCbCr itself
    /// scans out directly, at half the bytes of BGRX.
    pub fn decode_yuv422p(&mut self, planes: [PlaneMut<'_>; 3]) -> Result<(), Status> {
        if !self.loaded {
            return Err(Status::Empty);
        }
        let width = self.shared.width;
        let widths = [width, width / 2, width / 2];
        for (p, w) in planes.iter().zip(widths) {
            let needed = p.stride.checked_mul(self.height).ok_or(Status::OutputSize)?;
            if p.stride < w || p.data.len() < needed {
                return Err(Status::OutputSize);
            }
        }
        let strides = [planes[0].stride, planes[1].stride, planes[2].stride];
        let [a, b, c] = planes;
        let outs = [Out::new(a.data), Out::new(b.data), Out::new(c.data)];
        self.run(Pixels::Yuv422p, Out::EMPTY, 0, outs, strides, None)
    }

    /// BGRX, resampled into `placement` within the output. Each worker
    /// converts only the source rows its slices contribute and stores each
    /// placed row once, so the resample runs on the whole pool and no
    /// full-size intermediate frame is written. Pixels outside the placed
    /// area are left untouched.
    pub fn decode_bgrx_placed(
        &mut self,
        output: &mut [u8],
        stride: usize,
        placement: &Placement<'_>,
    ) -> Result<(), Status> {
        if !self.loaded {
            return Err(Status::Empty);
        }
        self.prepare_placement(placement, output.len(), stride)?;
        let reference = PlacementRef::new(placement);
        self.run(Pixels::Bgrx, Out::new(output), stride, [Out::EMPTY; 3], [0; 3], Some(reference))
    }

    fn check_packed(&self, len: usize, stride: usize, minimum: usize) -> Result<(), Status> {
        if !self.loaded {
            return Err(Status::Empty);
        }
        let needed = stride.checked_mul(self.height).ok_or(Status::OutputSize)?;
        if stride < minimum || len < needed {
            return Err(Status::OutputSize);
        }
        Ok(())
    }

    /// Checks a placement against this decoder and the output it is written
    /// into, then readies the per-slice row ranges and the workers' row
    /// buffers. Every index a worker will follow is bounded here, before any
    /// worker runs.
    fn prepare_placement(
        &mut self,
        p: &Placement<'_>,
        output_len: usize,
        stride: usize,
    ) -> Result<(), Status> {
        let (width, height) = (p.columns.len(), p.rows.len());
        if width == 0 || height == 0 || width > MAX_PLACED_WIDTH || height > MAX_PLACED_HEIGHT {
            return Err(Status::InvalidDimensions);
        }
        if p.columns.iter().any(|&c| c as usize >= self.shared.width) {
            return Err(Status::InvalidDimensions);
        }
        if p.rows.iter().any(|&r| r as usize >= self.height)
            || p.rows.windows(2).any(|w| w[1] < w[0])
        {
            return Err(Status::InvalidDimensions);
        }
        let right =
            p.x.checked_add(width).and_then(|r| r.checked_mul(4)).ok_or(Status::OutputSize)?;
        let end =
            p.y.checked_add(height - 1)
                .and_then(|last| last.checked_mul(stride))
                .and_then(|e| e.checked_add(right))
                .ok_or(Status::OutputSize)?;
        if stride < right || end > output_len {
            return Err(Status::OutputSize);
        }
        let row_bytes = width * 4;
        if row_bytes > self.placed_capacity {
            for scratch in &self.shared.scratch {
                let mut s = lock(scratch);
                let extra = row_bytes.saturating_sub(s.placed_row.len());
                s.placed_row.try_reserve_exact(extra).map_err(|_| Status::WorkerFailure)?;
                s.placed_row.resize(row_bytes, 0);
            }
            self.placed_capacity = row_bytes;
        }
        let mut y = 0;
        let mut first = 0;
        for (i, slice) in self.shared.slices.iter().enumerate() {
            while y < height && (p.rows[y] as usize) < (i + 1) * SLICE_HEIGHT {
                y += 1;
            }
            lock(slice).placed = (first, y);
            first = y;
        }
        Ok(())
    }

    fn run(
        &mut self,
        pixels: Pixels,
        output: Out,
        stride: usize,
        planes: [Out; 3],
        plane_strides: [usize; 3],
        placement: Option<PlacementRef>,
    ) -> Result<(), Status> {
        if !self.loaded {
            return Err(Status::Empty);
        }
        let job = Job {
            pixels,
            matrix: self.matrix,
            dc_shift: self.dc_shift,
            coefficients: self.color_space.coefficients(),
            output,
            planes,
            plane_strides,
            stride,
            placement,
        };
        let shared = &*self.shared;
        shared.next.store(0, Ordering::Relaxed);
        shared.failed.store(false, Ordering::Relaxed);
        if self.threads.is_empty() {
            decode_claimed(shared, &job, 0);
        } else {
            // The job is published under the lock the workers wake on, which
            // orders every write above before their first claim.
            {
                let mut state = lock(&shared.state);
                state.job = Some(job);
                state.running = self.threads.len();
                state.generation += 1;
                shared.start.notify_all();
            }
            decode_claimed(shared, &job, 0);
            let mut state = lock(&shared.state);
            while state.running > 0 {
                state = shared.finished.wait(state).unwrap_or_else(PoisonError::into_inner);
            }
            // No worker holds the job past this point; the output buffers it
            // points into are released when this call returns.
            state.job = None;
        }
        if shared.failed.load(Ordering::Relaxed) { Err(Status::CorruptStream) } else { Ok(()) }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        {
            let mut state = lock(&self.shared.state);
            state.shutdown = true;
            self.shared.start.notify_all();
        }
        for handle in self.threads.drain(..) {
            let _ = handle.join();
        }
    }
}

/// Reads one length-prefixed slice stream at `cursor`.
fn take_stream<'a>(input: &'a [u8], cursor: &mut usize) -> Result<&'a [u8], Status> {
    let rest = input.get(*cursor..).ok_or(Status::Truncated)?;
    let (head, rest) = rest.split_first_chunk::<4>().ok_or(Status::Truncated)?;
    let length = u32::from_le_bytes(*head) as usize;
    let data = rest.get(..length).ok_or(Status::Truncated)?;
    *cursor += 4 + length;
    Ok(data)
}

fn worker_main(shared: &Shared, index: usize) {
    let mut seen = 0;
    let mut state = lock(&shared.state);
    loop {
        while !state.shutdown && state.generation == seen {
            state = shared.start.wait(state).unwrap_or_else(PoisonError::into_inner);
        }
        if state.shutdown {
            break;
        }
        seen = state.generation;
        let job = state.job;
        drop(state);
        if let Some(job) = job {
            decode_claimed(shared, &job, index);
        }
        state = lock(&shared.state);
        state.running -= 1;
        if state.running == 0 {
            shared.finished.notify_one();
        }
    }
}

/// Decodes slices claimed from the shared counter until none are left. A
/// failed slice fails the frame and stops the claiming for every thread.
fn decode_claimed(shared: &Shared, job: &Job, scratch_index: usize) {
    let mut scratch = lock(&shared.scratch[scratch_index]);
    let count = shared.slices.len();
    loop {
        let index = shared.next.fetch_add(1, Ordering::Relaxed);
        if index >= count {
            return;
        }
        if !decode_slice(shared, job, index, &mut scratch) {
            shared.failed.store(true, Ordering::Relaxed);
            shared.next.store(count, Ordering::Relaxed);
            return;
        }
    }
}

/// Decodes one slice into its band of the output.
fn decode_slice(shared: &Shared, job: &Job, index: usize, scratch: &mut Scratch) -> bool {
    let slice = lock(&shared.slices[index]);
    let (ls, cs) = (shared.luma_stride, shared.chroma_stride);
    let mut dc = slice.dc.reader();
    let mut ac = slice.ac.reader();
    let Scratch { luma, blue, red, source_row, placed_row } = scratch;
    if !plane::decode_plane(&mut dc, &mut ac, ls, 128, &job.matrix, job.dc_shift, luma)
        || !plane::decode_plane(&mut dc, &mut ac, cs, 0, &job.matrix, job.dc_shift, blue)
        || !plane::decode_plane(&mut dc, &mut ac, cs, 0, &job.matrix, job.dc_shift, red)
    {
        return false;
    }
    let width = shared.width;
    let first_row = index * SLICE_HEIGHT;

    if let Some(p) = &job.placement {
        // Rows are non-decreasing, so consecutive destination rows that take
        // the same source row reuse the one already converted and gathered.
        // SAFETY: the decode that published this job is still running.
        let (columns, rows) = unsafe { p.tables() };
        let row_bytes = columns.len() * 4;
        let mut converted = usize::MAX;
        for (y, &source) in rows.iter().enumerate().take(slice.placed.1).skip(slice.placed.0) {
            let source = source as usize;
            if source != converted {
                let row = source - first_row;
                convert::bgra_row(
                    &luma[row * ls..],
                    &blue[row * cs..],
                    &red[row * cs..],
                    width,
                    source_row,
                    job.coefficients,
                );
                for (dst, &c) in
                    placed_row[..row_bytes].as_chunks_mut::<4>().0.iter_mut().zip(columns)
                {
                    let c = c as usize * 4;
                    dst.copy_from_slice(&source_row[c..c + 4]);
                }
                converted = source;
            }
            // One contiguous store per row into what is usually a
            // write-combined scanout mapping.
            let offset = (p.y + y) * job.stride + p.x * 4;
            // SAFETY: destination row y belongs to this slice alone (the
            // slices' placed ranges are disjoint), so no other thread writes
            // these bytes during the frame.
            let Some(out) = (unsafe { job.output.row(offset, row_bytes) }) else { return false };
            out.copy_from_slice(&placed_row[..row_bytes]);
        }
        return true;
    }

    if job.pixels == Pixels::Yuv422p {
        let widths = [width, width / 2, width / 2];
        let sources: [&[u8]; 3] = [luma, blue, red];
        let strides = [ls, cs, cs];
        for p in 0..3 {
            let band = Band { first_row, rows: slice.rows, width: widths[p] };
            // SAFETY: rows first_row.. of every plane belong to this slice
            // alone, so no other thread writes these bytes.
            if !unsafe {
                copy_band(sources[p], strides[p], &job.planes[p], job.plane_strides[p], &band)
            } {
                return false;
            }
        }
        return true;
    }

    let band = Band { first_row, rows: slice.rows, width };
    // SAFETY: output rows first_row.. belong to this slice alone.
    unsafe { convert_band([luma, blue, red], [ls, cs], job, &band) }
}

/// Converts one slice's rows into a packed output, out of line for the same
/// reason as [`copy_band`]: the kernels' row stores then run back to back.
///
/// Every row is bounds-checked before the kernel runs, so no kernel can be
/// handed a rectangle it would run past.
///
/// # Safety
///
/// As for [`Out::row`]: the output must still be borrowed by the running
/// decode, and these rows must belong to the calling slice alone.
#[inline(never)]
unsafe fn convert_band(planes: [&[u8]; 3], strides: [usize; 2], job: &Job, band: &Band) -> bool {
    let [luma, blue, red] = planes;
    let [ls, cs] = strides;
    let Band { first_row, rows, width } = *band;
    let row_bytes = width * if job.pixels == Pixels::Uyvy { 2 } else { 4 };
    for row in 0..rows {
        let offset = (first_row + row) * job.stride;
        // SAFETY: the caller's guarantee covers every row of the band.
        let Some(out) = (unsafe { job.output.row(offset, row_bytes) }) else { return false };
        let (l, b, r) = (&luma[row * ls..], &blue[row * cs..], &red[row * cs..]);
        if job.pixels == Pixels::Uyvy {
            convert::uyvy_row(l, b, r, width, out);
        } else {
            convert::bgra_row(l, b, r, width, out, job.coefficients);
        }
    }
    true
}

/// The rows of one slice in a plane: where they start and how wide they are.
#[derive(Clone, Copy, Debug)]
struct Band {
    first_row: usize,
    rows: usize,
    width: usize,
}

/// Copies one slice's rows of a plane out of the scratch, one plane at a
/// time.
///
/// Kept out of line, and per plane rather than interleaving the three: the
/// copies then run back to back with nothing stored between them. When both
/// sides are tightly packed, the whole band is one copy.
///
/// # Safety
///
/// As for [`Out::row`]: the plane must still be borrowed by the running
/// decode, and these rows must belong to the calling slice alone.
#[inline(never)]
unsafe fn copy_band(
    source: &[u8],
    source_stride: usize,
    plane: &Out,
    stride: usize,
    band: &Band,
) -> bool {
    let Band { first_row, rows, width } = *band;
    if source_stride == width && stride == width {
        let len = rows * width;
        // SAFETY: the caller's guarantee covers every row of the band.
        let Some(out) = (unsafe { plane.row(first_row * stride, len) }) else { return false };
        out.copy_from_slice(&source[..len]);
        return true;
    }
    for row in 0..rows {
        // SAFETY: as above.
        let Some(out) = (unsafe { plane.row((first_row + row) * stride, width) }) else {
            return false;
        };
        out.copy_from_slice(&source[row * source_stride..row * source_stride + width]);
    }
    true
}

/// Kernel entry points for the agreement tests and the benchmark: the
/// portable kernels, and the ones the decoder actually runs on this target.
#[doc(hidden)]
pub mod kernels {
    pub const QUALITY_COUNT: usize = crate::tables::QUALITY_COUNT;
    pub const YUV_RGB_601: [i16; 5] = crate::tables::YUV_RGB_601;
    pub const YUV_RGB_709: [i16; 5] = crate::tables::YUV_RGB_709;

    pub fn decode_matrix(index: usize) -> [u16; 64] {
        crate::tables::decode_matrix(index)
    }

    pub fn idct_scalar(
        block: &[i16; 64],
        matrix: &[u16; 64],
        dst: &mut [u8],
        stride: usize,
        bias: i16,
    ) {
        crate::idct::idct_scalar(block, matrix, dst, stride, bias);
    }

    pub fn idct(
        block: &[i16; 64],
        matrix: &[u16; 64],
        dst: &mut [u8],
        stride: usize,
        bias: i16,
        rows: u8,
    ) {
        crate::idct::idct(block, matrix, dst, stride, bias, rows);
    }

    pub fn bgra_row_scalar(
        luma: &[u8],
        blue: &[u8],
        red: &[u8],
        width: usize,
        out: &mut [u8],
        c: &[i16; 5],
    ) {
        crate::convert::bgra_row_scalar(luma, blue, red, width, out, c);
    }

    pub fn bgra_row(
        luma: &[u8],
        blue: &[u8],
        red: &[u8],
        width: usize,
        out: &mut [u8],
        c: &[i16; 5],
    ) {
        crate::convert::bgra_row(luma, blue, red, width, out, c);
    }
}
