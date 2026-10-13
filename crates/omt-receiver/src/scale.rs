//! Aspect-preserving resample for displays whose mode list carries no entry
//! at the incoming video's size. The picture is fitted into the mode and
//! centred; the bars are black. The filter is nearest-neighbour with
//! pixel-centre sampling -- the only one the Pi 4's decode budget leaves room
//! for.
//!
//! This module only decides where each destination pixel comes from. The
//! resample itself runs inside the VMX decode
//! (`Decoder::decode_bgrx_placed`), on every worker, straight into the
//! scanout buffer.

use omt_common::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

impl Placement {
    /// Fits `source` inside `mode` without changing its aspect ratio,
    /// centred; `None` for a degenerate source or mode.
    pub fn fit(sw: usize, sh: usize, mw: usize, mh: usize) -> Option<Self> {
        if sw == 0 || sh == 0 || mw == 0 || mh == 0 {
            return None;
        }
        // Cross-multiplied, so the limiting axis never depends on a rounded
        // ratio.
        let source_ratio = sw as u128 * mh as u128;
        let mode_ratio = mw as u128 * sh as u128;
        let (width, height) = if source_ratio <= mode_ratio {
            (usize::try_from(source_ratio / sh as u128).unwrap_or(mw).min(mw), mh)
        } else {
            (mw, usize::try_from(mode_ratio / sw as u128).unwrap_or(mh).min(mh))
        };
        let (width, height) = (width.max(1), height.max(1));
        Some(Self { x: (mw - width) / 2, y: (mh - height) / 2, width, height })
    }

    pub fn fills(&self, mw: usize, mh: usize) -> bool {
        self.x == 0 && self.y == 0 && self.width == mw && self.height == mh
    }
}

/// The source coordinate destination pixel `index` samples: its centre,
/// mapped back.
pub fn sample(index: usize, destination: usize, source: usize) -> usize {
    let numerator = (2 * index as u128 + 1) * source as u128;
    let coordinate = numerator / (2 * destination as u128);
    usize::try_from(coordinate).unwrap_or(0).min(source - 1)
}

/// The tables the decode resamples with.
#[derive(Clone, Debug)]
pub struct Scaler {
    pub placement: Placement,
    /// Source row of each destination row, non-decreasing.
    pub rows: Vec<u32>,
    /// Source pixel of each destination column.
    pub columns: Vec<u32>,
}

impl Scaler {
    pub fn new(sw: usize, sh: usize, p: Placement) -> Result<Self, Error> {
        if sw == 0
            || sh == 0
            || u32::try_from(sw).is_err()
            || u32::try_from(sh).is_err()
            || p.width == 0
            || p.height == 0
        {
            return Err(Error::new("Video frame geometry cannot be scaled"));
        }
        let table = |n: usize, source: usize| -> Vec<u32> {
            (0..n).map(|i| u32::try_from(sample(i, n, source)).unwrap_or(u32::MAX)).collect()
        };
        Ok(Self { placement: p, rows: table(p.height, sh), columns: table(p.width, sw) })
    }
}
