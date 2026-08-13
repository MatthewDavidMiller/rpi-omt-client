// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// Decode-only VMX1 support for the appliance. The encoder, the preview
// resolutions, and the unused colour-conversion outputs of the reference codec
// are intentionally absent; see third_party/omt/PROVENANCE.md.
// Unsafe is denied everywhere except the architecture-specific IDCT kernels,
// which opt in per module; see `idct/neon.rs` for the justification of each
// operation.
#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

mod bitstream;
mod convert;
mod idct;
mod plane;
mod pool;
mod tables;

use bitstream::BitReader;
use plane::{PlaneShape, SliceStreams, decode_plane};
use pool::WorkerPool;
use tables::{SLICE_HEIGHT, YUV_RGB_601, YUV_RGB_709, decode_matrix, quality_index};

/// Every media worker runs on an explicitly sized stack. The kernel keeps only
/// 64 KiB of block scratch live, so this bounds a worker well above its needs
/// while keeping a full pool far under the appliance's memory budget.
pub const WORKER_STACK_SIZE: usize = 128 * 1024;
pub const MAX_WIDTH: usize = 1920;
pub const MAX_HEIGHT: usize = 1080;
pub const MAX_COMPRESSED_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_WORKERS: usize = 8;

/// The reference codec's `VMX_CODEC_FORMAT` byte.
const CODEC_FORMAT_PROGRESSIVE: u8 = 1;
const CODEC_FORMAT_INTERLACED: u8 = 2;
const CODEC_FORMAT_EXTENDED: u8 = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Dimensions {
    pub width: usize,
    pub height: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorSpace {
    Bt601,
    Bt709,
}

impl ColorSpace {
    /// Mirrors the reference codec's undefined-colour-space default.
    #[must_use]
    pub fn resolve(value: i32, height: usize) -> Self {
        match value {
            601 => Self::Bt601,
            709 => Self::Bt709,
            _ if height >= 720 => Self::Bt709,
            _ => Self::Bt601,
        }
    }
    fn coefficients(self) -> &'static [i16; 5] {
        match self {
            Self::Bt601 => &YUV_RGB_601,
            Self::Bt709 => &YUV_RGB_709,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum DecodeError {
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

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for DecodeError {}

/// One slice of the frame: its two bitstreams. Plane scratch is per-worker
/// rather than per-slice, because each worker decodes its slices in sequence
/// and the YUV planes are consumed into the output before the next slice.
struct Slice {
    streams: SliceStreams,
    /// Rows of this slice that belong to the visible image.
    rows: usize,
}

/// Reused YUV planes for one slice at a time.
pub(crate) struct PlaneScratch {
    luma: Vec<u8>,
    blue: Vec<u8>,
    red: Vec<u8>,
}

impl PlaneScratch {
    pub(crate) fn new(luma_stride: usize, chroma_stride: usize) -> Result<Self, DecodeError> {
        Ok(Self {
            luma: try_zeroed(
                luma_stride
                    .checked_mul(SLICE_HEIGHT)
                    .ok_or(DecodeError::WorkerFailure)?,
            )?,
            blue: try_zeroed(
                chroma_stride
                    .checked_mul(SLICE_HEIGHT)
                    .ok_or(DecodeError::WorkerFailure)?,
            )?,
            red: try_zeroed(
                chroma_stride
                    .checked_mul(SLICE_HEIGHT)
                    .ok_or(DecodeError::WorkerFailure)?,
            )?,
        })
    }
}

/// A bounded VMX1 decoder for one fixed frame geometry.
pub struct Decoder {
    dimensions: Dimensions,
    color_space: ColorSpace,
    luma_stride: usize,
    chroma_stride: usize,
    slices: Vec<Slice>,
    workers: usize,
    pool: WorkerPool,
    /// Used when decode stays on this thread (`workers == 1`).
    scratch: PlaneScratch,
    matrix: [u16; 64],
    dc_shift: i32,
    loaded: bool,
}

impl Decoder {
    /// Creates a decoder for a fixed frame geometry with a bounded worker pool.
    ///
    /// # Errors
    /// Returns [`DecodeError::InvalidDimensions`] for unsupported dimensions or
    /// worker counts, and [`DecodeError::WorkerFailure`] if the bounded slice
    /// allocations cannot be reserved.
    pub fn new(
        dimensions: Dimensions,
        color_space: ColorSpace,
        workers: usize,
    ) -> Result<Self, DecodeError> {
        if dimensions.width < 16
            || dimensions.width > MAX_WIDTH
            || !dimensions.width.is_multiple_of(2)
            || dimensions.height < 16
            || dimensions.height > MAX_HEIGHT
            || workers == 0
            || workers > MAX_WORKERS
        {
            return Err(DecodeError::InvalidDimensions);
        }
        let luma_stride = align8(dimensions.width);
        let chroma_stride = align8(dimensions.width / 2);
        let aligned_height = dimensions.height.next_multiple_of(SLICE_HEIGHT);
        let slice_count = aligned_height / SLICE_HEIGHT;
        // The reference codec sizes the per-slice streams from the luma stride.
        let dc_capacity = luma_stride * SLICE_HEIGHT * 2;
        let ac_capacity = luma_stride * SLICE_HEIGHT * 4;

        let mut slices = Vec::new();
        slices
            .try_reserve_exact(slice_count)
            .map_err(|_| DecodeError::WorkerFailure)?;
        for index in 0..slice_count {
            let visible = dimensions.height.saturating_sub(index * SLICE_HEIGHT);
            slices.push(Slice {
                streams: SliceStreams {
                    dc: BitReader::with_capacity(dc_capacity).ok_or(DecodeError::WorkerFailure)?,
                    ac: BitReader::with_capacity(ac_capacity).ok_or(DecodeError::WorkerFailure)?,
                },
                rows: visible.min(SLICE_HEIGHT),
            });
        }

        let workers = workers.min(slice_count.max(1));
        let pool = WorkerPool::new(workers, luma_stride, chroma_stride)?;
        let scratch = PlaneScratch::new(luma_stride, chroma_stride)?;

        Ok(Self {
            dimensions,
            color_space,
            luma_stride,
            chroma_stride,
            slices,
            workers,
            pool,
            scratch,
            matrix: decode_matrix(0),
            dc_shift: 0,
            loaded: false,
        })
    }

    /// Validates the bounded VMX1 envelope before decoding.
    fn validate_stream(input: &[u8]) -> Result<&[u8], DecodeError> {
        if input.is_empty() {
            return Err(DecodeError::Empty);
        }
        if input.len() > MAX_COMPRESSED_BYTES {
            return Err(DecodeError::Oversized);
        }
        if input.len() < 5 {
            return Err(DecodeError::Truncated);
        }
        match input[0] {
            CODEC_FORMAT_PROGRESSIVE | CODEC_FORMAT_EXTENDED => Ok(input),
            CODEC_FORMAT_INTERLACED => Err(DecodeError::UnsupportedFormat),
            _ => Err(DecodeError::InvalidFormat),
        }
    }

    /// `VMX_LoadFrom`: splits a validated frame into its per-slice bitstreams.
    ///
    /// # Errors
    /// Returns a typed error for an unknown envelope, a slice count that does
    /// not match the decoder geometry, or a truncated stream table.
    pub fn load(&mut self, input: &[u8]) -> Result<(), DecodeError> {
        self.loaded = false;
        let input = Self::validate_stream(input)?;
        let (offset, dc_shift) = if input[0] == CODEC_FORMAT_EXTENDED {
            (2_usize, i32::from(input[1]))
        } else {
            (0_usize, 0)
        };
        let format = input.get(offset).copied().ok_or(DecodeError::Truncated)?;
        if format != CODEC_FORMAT_PROGRESSIVE {
            // Interlaced frames use the reference codec's field-paired slice
            // layout, which the appliance's progressive pipeline never emits.
            return Err(DecodeError::UnsupportedFormat);
        }
        let quality = i32::from(*input.get(offset + 1).ok_or(DecodeError::Truncated)?);
        let slices = usize::from(*input.get(offset + 2).ok_or(DecodeError::Truncated)?);
        if slices != self.slices.len() {
            return Err(DecodeError::SliceCount);
        }

        let mut cursor = 3 + offset;
        for index in 0..self.slices.len() {
            let (data, next) = take_stream(input, cursor)?;
            if !self.slices[index].streams.dc.load(data) {
                return Err(DecodeError::Oversized);
            }
            cursor = next;
        }
        // A preview-only frame carries the DC streams alone.
        let has_ac = cursor < input.len();
        for index in 0..self.slices.len() {
            let data = if has_ac {
                let (data, next) = take_stream(input, cursor)?;
                cursor = next;
                data
            } else {
                &[][..]
            };
            if !self.slices[index].streams.ac.load(data) {
                return Err(DecodeError::Oversized);
            }
        }

        self.matrix = decode_matrix(quality_index(quality));
        self.dc_shift = dc_shift;
        self.loaded = true;
        Ok(())
    }

    /// Decodes the loaded frame into packed UYVY.
    ///
    /// # Errors
    /// Returns [`DecodeError::OutputSize`] for an undersized destination and
    /// [`DecodeError::CorruptStream`] if any slice rejects its bitstream.
    pub fn decode_uyvy(&mut self, output: &mut [u8], stride: usize) -> Result<(), DecodeError> {
        self.decode(output, stride, self.dimensions.width * 2, Pixels::Uyvy)
    }

    /// Decodes the loaded frame into packed BGRX, the layout the display
    /// scanout consumes as XRGB8888.
    ///
    /// # Errors
    /// Returns [`DecodeError::OutputSize`] for an undersized destination and
    /// [`DecodeError::CorruptStream`] if any slice rejects its bitstream.
    pub fn decode_bgrx(&mut self, output: &mut [u8], stride: usize) -> Result<(), DecodeError> {
        self.decode(output, stride, self.dimensions.width * 4, Pixels::Bgrx)
    }

    fn decode(
        &mut self,
        output: &mut [u8],
        stride: usize,
        minimum_stride: usize,
        pixels: Pixels,
    ) -> Result<(), DecodeError> {
        if !self.loaded {
            return Err(DecodeError::Empty);
        }
        if stride < minimum_stride
            || output.len()
                < stride
                    .checked_mul(self.dimensions.height)
                    .ok_or(DecodeError::OutputSize)?
        {
            return Err(DecodeError::OutputSize);
        }

        for slice in &mut self.slices {
            slice.streams.dc.reset();
            slice.streams.ac.reset();
        }

        let geometry = DecodeGeometry {
            width: self.dimensions.width,
            stride,
            luma_stride: self.luma_stride,
            chroma_stride: self.chroma_stride,
            dc_shift: self.dc_shift,
            pixels,
        };
        let matrix = self.matrix;
        let coefficients = self.color_space.coefficients();

        // A single worker keeps the decode on this thread: the pool still
        // exists for Drop symmetry, but the channel round-trip would only add
        // latency when there is nothing to parallelise.
        let ok = if self.workers <= 1 {
            decode_group(
                &mut self.slices,
                output,
                geometry,
                &matrix,
                coefficients,
                &mut self.scratch,
            )
        } else {
            self.pool
                .decode(&mut self.slices, output, geometry, &matrix, coefficients)?
        };
        if ok {
            Ok(())
        } else {
            Err(DecodeError::CorruptStream)
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Pixels {
    Uyvy,
    Bgrx,
}

#[derive(Clone, Copy)]
pub(crate) struct DecodeGeometry {
    width: usize,
    stride: usize,
    luma_stride: usize,
    chroma_stride: usize,
    dc_shift: i32,
    pixels: Pixels,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn decode_group(
    slices: &mut [Slice],
    region: &mut [u8],
    geometry: DecodeGeometry,
    matrix: &[u16; 64],
    coefficients: &[i16; 5],
    scratch: &mut PlaneScratch,
) -> bool {
    for (index, slice) in slices.iter_mut().enumerate() {
        let luma = PlaneShape {
            stride: geometry.luma_stride,
            bias: 128,
        };
        let chroma = PlaneShape {
            stride: geometry.chroma_stride,
            bias: 0,
        };
        if !decode_plane(
            &mut slice.streams,
            &luma,
            matrix,
            geometry.dc_shift,
            &mut scratch.luma,
        ) || !decode_plane(
            &mut slice.streams,
            &chroma,
            matrix,
            geometry.dc_shift,
            &mut scratch.blue,
        ) || !decode_plane(
            &mut slice.streams,
            &chroma,
            matrix,
            geometry.dc_shift,
            &mut scratch.red,
        ) {
            return false;
        }

        let start = index * SLICE_HEIGHT * geometry.stride;
        let Some(target) = region.get_mut(start..) else {
            return false;
        };
        let planes = convert::Planes {
            luma: &scratch.luma,
            luma_stride: geometry.luma_stride,
            blue: &scratch.blue,
            red: &scratch.red,
            chroma_stride: geometry.chroma_stride,
        };
        let rectangle = convert::Target {
            stride: geometry.stride,
            width: geometry.width,
            height: slice.rows,
        };
        match geometry.pixels {
            Pixels::Uyvy => {
                if convert::planar_to_uyvy(&planes, rectangle, target).is_err() {
                    return false;
                }
            }
            Pixels::Bgrx => {
                if convert::yuv422_to_bgra(&planes, rectangle, target, coefficients).is_err() {
                    return false;
                }
            }
        }
    }
    true
}

fn align8(value: usize) -> usize {
    value.next_multiple_of(8)
}

fn try_zeroed(length: usize) -> Result<Vec<u8>, DecodeError> {
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(length)
        .map_err(|_| DecodeError::WorkerFailure)?;
    buffer.resize(length, 0);
    Ok(buffer)
}

/// Reads one length-prefixed slice stream, returning it with the next cursor.
fn take_stream(input: &[u8], cursor: usize) -> Result<(&[u8], usize), DecodeError> {
    let header_end = cursor.checked_add(4).ok_or(DecodeError::Truncated)?;
    let header = input
        .get(cursor..header_end)
        .ok_or(DecodeError::Truncated)?;
    let mut length_bytes = [0_u8; 4];
    length_bytes.copy_from_slice(header);
    let length =
        usize::try_from(u32::from_le_bytes(length_bytes)).map_err(|_| DecodeError::Oversized)?;
    let end = header_end
        .checked_add(length)
        .ok_or(DecodeError::Oversized)?;
    let data = input.get(header_end..end).ok_or(DecodeError::Truncated)?;
    Ok((data, end))
}
