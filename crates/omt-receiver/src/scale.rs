// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// Aspect-preserving resample for displays whose mode list carries no entry at
// the incoming video format's size. The picture is fitted into the selected
// mode and centred; the bars around it are left at the zero the kernel hands
// back with a fresh dumb buffer, which is black in XRGB8888.
//
// The resample is nearest-neighbour with pixel-centre sampling. It is the only
// filter that fits: the Pi 4 tier already spends 26.4 ms of its 33.3 ms budget
// decoding a 1080p frame, and a bilinear pass over a 720p destination costs
// several times the gather-copy this does. Sampling with centres rather than
// truncation keeps the picture from drifting half a destination pixel up and
// left, which is free.

/// Where the picture lands inside the mode, in destination pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Placement {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

impl Placement {
    /// Fits `source` inside `mode` without changing its aspect ratio, centred.
    ///
    /// Returns `None` for a degenerate source or mode, which callers treat as
    /// a format they cannot present rather than as a division by zero.
    pub fn fit(source: (usize, usize), mode: (usize, usize)) -> Option<Self> {
        let (source_width, source_height) = source;
        let (mode_width, mode_height) = mode;
        if source_width == 0 || source_height == 0 || mode_width == 0 || mode_height == 0 {
            return None;
        }
        // Compare the two aspect ratios by cross-multiplying, so the choice of
        // limiting axis never depends on a rounded floating-point ratio.
        let source_ratio = (source_width as u64) * (mode_height as u64);
        let mode_ratio = (mode_width as u64) * (source_height as u64);
        let (width, height) = if source_ratio <= mode_ratio {
            // Taller than the mode, or the same shape: height is the limit.
            let width = (source_width as u64) * (mode_height as u64) / (source_height as u64);
            (usize::try_from(width).ok()?.min(mode_width), mode_height)
        } else {
            let height = (source_height as u64) * (mode_width as u64) / (source_width as u64);
            (mode_width, usize::try_from(height).ok()?.min(mode_height))
        };
        // A source far narrower or shorter than one destination pixel per
        // sample would otherwise produce a zero-sized rectangle to render into.
        let width = width.max(1);
        let height = height.max(1);
        Some(Self {
            x: (mode_width - width) / 2,
            y: (mode_height - height) / 2,
            width,
            height,
        })
    }

    /// Whether the picture covers the whole mode, so the caller knows if there
    /// are bars that have to be black before the first frame lands.
    pub fn fills(&self, mode: (usize, usize)) -> bool {
        self.x == 0 && self.y == 0 && self.width == mode.0 && self.height == mode.1
    }
}

/// A fixed source geometry resampled into a fixed placement.
///
/// The per-row and per-column source offsets are computed once, when the
/// format or the mode changes, so a frame costs one indexed load and one store
/// per destination pixel and no arithmetic in the inner loop.
pub struct Scaler {
    placement: Placement,
    source_stride: usize,
    source_row_bytes: usize,
    /// Byte offset of each destination row's source row.
    rows: Vec<usize>,
    /// Byte offset of each destination column within its source row.
    columns: Vec<usize>,
}

impl Scaler {
    /// Builds the sample tables for one source geometry and placement.
    ///
    /// # Errors
    /// Returns a message when the bounded tables cannot be reserved or the
    /// source geometry does not fit the stride it was given.
    pub fn new(
        source: (usize, usize),
        source_stride: usize,
        placement: Placement,
    ) -> Result<Self, String> {
        let (source_width, source_height) = source;
        let source_row_bytes = source_width
            .checked_mul(4)
            .ok_or_else(|| "Video frame is too wide to scale".to_owned())?;
        if source_width == 0 || source_height == 0 || source_stride < source_row_bytes {
            return Err("Video frame geometry cannot be scaled".into());
        }
        let mut rows = Vec::new();
        rows.try_reserve_exact(placement.height)
            .map_err(|_| "Unable to reserve the scaler row table".to_owned())?;
        for y in 0..placement.height {
            let source_y = sample(y, placement.height, source_height);
            rows.push(source_y * source_stride);
        }
        let mut columns = Vec::new();
        columns
            .try_reserve_exact(placement.width)
            .map_err(|_| "Unable to reserve the scaler column table".to_owned())?;
        for x in 0..placement.width {
            columns.push(sample(x, placement.width, source_width) * 4);
        }
        Ok(Self {
            placement,
            source_stride,
            source_row_bytes,
            rows,
            columns,
        })
    }

    #[cfg(test)]
    pub fn placement(&self) -> Placement {
        self.placement
    }

    /// Whether the resampled picture covers the whole mode, so the caller knows
    /// whether there are bars that have to be black before the first frame.
    pub fn covers(&self, mode: (usize, usize)) -> bool {
        self.placement.fills(mode)
    }

    /// The stride a decoded source frame must be written with.
    pub fn source_stride(&self) -> usize {
        self.source_stride
    }

    /// Resamples one decoded BGRX frame into the destination buffer.
    ///
    /// # Errors
    /// Returns a message when either buffer is smaller than the geometry the
    /// scaler was built for. Both are sized by this process, so a failure here
    /// is a bug rather than something the stream can provoke.
    pub fn render(
        &self,
        source: &[u8],
        destination: &mut [u8],
        destination_stride: usize,
    ) -> Result<(), String> {
        let short = || "Scaler buffer is too small".to_owned();
        let first = self
            .placement
            .y
            .checked_mul(destination_stride)
            .and_then(|offset| offset.checked_add(self.placement.x.checked_mul(4)?))
            .ok_or_else(short)?;
        let row_bytes = self.placement.width.checked_mul(4).ok_or_else(short)?;
        // The whole placed row has to end inside its own destination row. The
        // slice bounds below would otherwise be satisfied by bleeding into the
        // next row's leading pixels rather than by running off the buffer.
        if destination_stride
            < self
                .placement
                .x
                .checked_mul(4)
                .and_then(|left| left.checked_add(row_bytes))
                .ok_or_else(short)?
        {
            return Err(short());
        }
        let region = destination.get_mut(first..).ok_or_else(short)?;

        for (index, &source_offset) in self.rows.iter().enumerate() {
            let source_row = source
                .get(source_offset..source_offset + self.source_row_bytes)
                .ok_or_else(short)?;
            let start = index * destination_stride;
            let destination_row = region.get_mut(start..start + row_bytes).ok_or_else(short)?;
            for (pixel, &column) in destination_row.chunks_exact_mut(4).zip(&self.columns) {
                let sample = source_row.get(column..column + 4).ok_or_else(short)?;
                pixel.copy_from_slice(sample);
            }
        }
        Ok(())
    }
}

/// The source coordinate a destination coordinate samples, taking the centre of
/// the destination pixel rather than its leading edge.
fn sample(index: usize, destination: usize, source: usize) -> usize {
    debug_assert!(destination > 0 && source > 0);
    let numerator = (2 * index as u64 + 1) * source as u64;
    let coordinate = numerator / (2 * destination as u64);
    // usize on this target is 64-bit and `source` is bounded by the decoder's
    // maximum, so the clamp is what keeps the result in range rather than the
    // conversion.
    usize::try_from(coordinate).unwrap_or(0).min(source - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placement_of(source: (usize, usize), mode: (usize, usize)) -> Placement {
        let Some(placement) = Placement::fit(source, mode) else {
            panic!("{source:?} has no placement in {mode:?}")
        };
        placement
    }

    fn scaler_of(source: (usize, usize), stride: usize, placement: Placement) -> Scaler {
        Scaler::new(source, stride, placement).unwrap_or_else(|error| panic!("{error}"))
    }

    /// The case the appliance is being fixed for: a 1080p stream on a display
    /// whose mode list stops at 720p. Both are 16:9, so the picture fills the
    /// mode and there are no bars to black out.
    #[test]
    fn a_matching_aspect_ratio_fills_the_mode() {
        let placement = placement_of((1920, 1080), (1280, 720));
        assert_eq!(
            placement,
            Placement {
                x: 0,
                y: 0,
                width: 1280,
                height: 720
            }
        );
        assert!(placement.fills((1280, 720)));
    }

    /// A 16:9 stream on a 4:3 mode has to letterbox rather than stretch, and a
    /// 4:3 stream on a 16:9 mode has to pillarbox.
    #[test]
    fn a_mismatched_aspect_ratio_is_barred_not_stretched() {
        let letterbox = placement_of((1920, 1080), (1024, 768));
        assert_eq!(
            letterbox,
            Placement {
                x: 0,
                y: 96,
                width: 1024,
                height: 576
            }
        );
        assert!(!letterbox.fills((1024, 768)));

        let pillarbox = placement_of((640, 480), (1280, 720));
        assert_eq!(
            pillarbox,
            Placement {
                x: 160,
                y: 0,
                width: 960,
                height: 720
            }
        );
        assert!(!pillarbox.fills((1280, 720)));
    }

    #[test]
    fn a_degenerate_geometry_has_no_placement() {
        assert_eq!(Placement::fit((0, 1080), (1280, 720)), None);
        assert_eq!(Placement::fit((1920, 0), (1280, 720)), None);
        assert_eq!(Placement::fit((1920, 1080), (0, 720)), None);
        assert_eq!(Placement::fit((1920, 1080), (1280, 0)), None);
    }

    /// An extreme reduction must still leave a rectangle to render into, not a
    /// zero-width one that renders nothing and reports success.
    #[test]
    fn an_extreme_reduction_keeps_at_least_one_pixel() {
        let placement = placement_of((1920, 16), (64, 64));
        assert!(placement.width >= 1 && placement.height >= 1);
    }

    /// Sampling by pixel centre spreads the dropped rows and columns evenly.
    /// Truncation would take source 0 twice and never take the last source
    /// pixel, which shifts the whole picture up and left by half a pixel.
    #[test]
    fn sampling_uses_pixel_centres() {
        let taken: Vec<usize> = (0..2).map(|i| sample(i, 2, 4)).collect();
        assert_eq!(taken, vec![1, 3]);
        let identity: Vec<usize> = (0..4).map(|i| sample(i, 4, 4)).collect();
        assert_eq!(identity, vec![0, 1, 2, 3]);
        // A 3:2 reduction, which is what 1080 rows into 720 rows is.
        let reduced: Vec<usize> = (0..4).map(|i| sample(i, 4, 6)).collect();
        assert_eq!(reduced, vec![0, 2, 3, 5]);
    }

    /// Every source pixel a 1:1 scaler names must be the one directly under it,
    /// so an exactly-sized placement is a copy and not a resample.
    #[test]
    fn an_unscaled_placement_copies_the_frame() {
        let source: Vec<u8> = (0..64_u8).collect();
        let placement = placement_of((4, 4), (4, 4));
        let scaler = scaler_of((4, 4), 16, placement);
        let mut destination = vec![0_u8; 16 * 4];
        scaler
            .render(&source, &mut destination, 16)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(destination, source);
    }

    /// A halved frame takes the second pixel of each source pair, and a
    /// centred placement leaves the bars it was given untouched.
    #[test]
    fn a_reduction_samples_and_leaves_the_bars_alone() {
        // Four source pixels across, each a distinct byte pattern.
        let mut source = vec![0_u8; 4 * 2 * 4];
        for y in 0_usize..2 {
            for x in 0_usize..4 {
                let value = u8::try_from(y * 4 + x).unwrap_or_default();
                let at = y * 16 + x * 4;
                source[at..at + 4].copy_from_slice(&[value, value, value, 0xff]);
            }
        }
        let placement = Placement {
            x: 1,
            y: 0,
            width: 2,
            height: 1,
        };
        let scaler = scaler_of((4, 2), 16, placement);
        let mut destination = vec![0_u8; 4 * 4];
        scaler
            .render(&source, &mut destination, 16)
            .unwrap_or_else(|error| panic!("{error}"));
        // The single destination row samples source row 1 by pixel centre, and
        // its two columns sample source pixels 1 and 3 of that row. The bar
        // columns on either side keep the value they were given.
        assert_eq!(&destination[0..4], &[0, 0, 0, 0]);
        assert_eq!(&destination[4..8], &[5, 5, 5, 0xff]);
        assert_eq!(&destination[8..12], &[7, 7, 7, 0xff]);
        assert_eq!(&destination[12..16], &[0, 0, 0, 0]);
    }

    /// The scaler sizes both buffers itself, so an undersized one is a bug. It
    /// still has to be reported rather than panic inside the playback loop.
    #[test]
    fn an_undersized_buffer_is_an_error_not_a_panic() {
        let placement = placement_of((4, 4), (4, 4));
        let scaler = scaler_of((4, 4), 16, placement);
        let source = vec![0_u8; 4 * 4 * 4];
        let mut destination = vec![0_u8; 16];
        assert!(scaler.render(&source, &mut destination, 16).is_err());
        assert!(scaler.render(&source[..16], &mut destination, 16).is_err());
    }

    #[test]
    fn a_stride_below_the_row_is_refused() {
        let placement = placement_of((4, 4), (4, 4));
        assert!(Scaler::new((4, 4), 15, placement).is_err());
        assert!(Scaler::new((0, 4), 16, placement).is_err());
        let scaler = scaler_of((4, 4), 16, placement);
        assert_eq!(scaler.source_stride(), 16);
        assert_eq!(scaler.placement(), placement);
        let source = vec![0_u8; 4 * 4 * 4];
        let mut destination = vec![0_u8; 4 * 4 * 4];
        assert!(scaler.render(&source, &mut destination, 15).is_err());
    }
}
