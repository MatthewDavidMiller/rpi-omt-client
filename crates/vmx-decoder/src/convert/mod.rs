// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// The planar output stages, and the choice of which implementation runs.
//
// Selection is made at compile time for the same reason the inverse DCT makes
// it that way: NEON is mandatory in ARMv8-A, so on the appliance's target the
// hand-written kernel is always the right one and anywhere else the portable
// kernel is. The dispatch costs nothing and leaves no untested branch.
//
// Only the BGRX stage has a hand-written kernel. It is the one the appliance
// runs -- the presenter scans out XRGB8888 -- and at 1080p it writes 8 MiB per
// frame, which the on-hardware decode bench showed dominating the frame budget
// once the entropy decode is spread over the worker pool.

mod scalar;

#[cfg(target_arch = "aarch64")]
mod neon;

/// One slice's worth of decoded 4:2:2 planes.
pub struct Planes<'a> {
    pub luma: &'a [u8],
    pub luma_stride: usize,
    pub blue: &'a [u8],
    pub red: &'a [u8],
    pub chroma_stride: usize,
}

/// The destination rectangle for one slice.
#[derive(Clone, Copy)]
pub struct Target {
    pub stride: usize,
    pub width: usize,
    pub height: usize,
}

/// The source rows for one output row, already narrowed to the visible width.
pub(crate) struct Rows<'a> {
    pub luma: &'a [u8],
    pub blue: &'a [u8],
    pub red: &'a [u8],
}

impl<'a> Planes<'a> {
    /// Narrows every plane to one row, or `None` if any is short.
    fn row(&self, row: usize, width: usize) -> Option<Rows<'a>> {
        let pairs = width / 2;
        Some(Rows {
            luma: self.luma.get(row * self.luma_stride..)?.get(..width)?,
            blue: self.blue.get(row * self.chroma_stride..)?.get(..pairs)?,
            red: self.red.get(row * self.chroma_stride..)?.get(..pairs)?,
        })
    }
}

/// Narrows both ends of one row, or `None` if either is short.
///
/// Bounds are checked once per row here, so neither kernel's inner loop
/// carries a check and neither can be handed a rectangle it would run past.
fn row_pair<'a, 'b>(
    planes: &Planes<'a>,
    destination: &'b mut [u8],
    target: Target,
    row: usize,
    bytes_per_pixel: usize,
) -> Option<(Rows<'a>, &'b mut [u8])> {
    let source = planes.row(row, target.width)?;
    let out = destination
        .get_mut(row * target.stride..)?
        .get_mut(..target.width * bytes_per_pixel)?;
    Some((source, out))
}

/// `VMX_PlanarToUYVY`: interleave one slice of 4:2:2 planes into packed UYVY.
///
/// # Errors
/// Returns an error when a source plane or the destination is shorter than the
/// declared rectangle, so a truncated decode cannot look successful.
pub fn planar_to_uyvy(
    planes: &Planes<'_>,
    target: Target,
    destination: &mut [u8],
) -> Result<(), ()> {
    for row in 0..target.height {
        let Some((source, out)) = row_pair(planes, destination, target, row, 2) else {
            return Err(());
        };
        scalar::uyvy_row(&source, out);
    }
    Ok(())
}

/// `VMX_YUV4224ToBGRA`: convert one slice of 4:2:2 planes into packed BGRX.
///
/// The reference kernel takes the fourth byte of each pixel from an alpha
/// plane. VMX1 carries no alpha, and the only consumer here is a DRM scanout
/// that reads the buffer as XRGB8888, so the byte is the constant this decoder
/// always fed it.
///
/// # Errors
/// Returns an error when a source plane or the destination is shorter than the
/// declared rectangle, so a truncated decode cannot look successful.
pub fn yuv422_to_bgra(
    planes: &Planes<'_>,
    target: Target,
    destination: &mut [u8],
    coefficients: &[i16; 5],
) -> Result<(), ()> {
    for row in 0..target.height {
        let Some((source, out)) = row_pair(planes, destination, target, row, 4) else {
            return Err(());
        };
        #[cfg(target_arch = "aarch64")]
        neon::bgra_row(&source, out, coefficients);
        #[cfg(not(target_arch = "aarch64"))]
        scalar::bgra_row(&source, out, coefficients);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Planes, Target, yuv422_to_bgra};
    use crate::tables::{YUV_RGB_601, YUV_RGB_709};

    #[test]
    fn uyvy_refuses_a_short_luma_plane() {
        let planes = Planes {
            luma: &[0_u8; 8],
            luma_stride: 8,
            blue: &[0_u8; 4],
            red: &[0_u8; 4],
            chroma_stride: 4,
        };
        let mut out = [0_u8; 32];
        assert!(
            super::planar_to_uyvy(
                &planes,
                Target {
                    stride: 16,
                    width: 8,
                    height: 2,
                },
                &mut out,
            )
            .is_err()
        );
    }

    #[test]
    fn bgra_refuses_a_short_destination() {
        let planes = Planes {
            luma: &[0_u8; 16],
            luma_stride: 8,
            blue: &[0_u8; 8],
            red: &[0_u8; 8],
            chroma_stride: 4,
        };
        let mut out = [0_u8; 16];
        assert!(
            yuv422_to_bgra(
                &planes,
                Target {
                    stride: 32,
                    width: 8,
                    height: 2,
                },
                &mut out,
                &YUV_RGB_709,
            )
            .is_err()
        );
    }

    /// The hand-written kernel has to agree with the portable one on every
    /// byte, over widths that exercise the vector body, every tail length, and
    /// widths shorter than one vector step.
    #[test]
    fn kernels_agree_bit_for_bit() {
        let mut state = 0x1234_5678_9ABC_DEF1_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            u8::try_from((state >> 24) & 0xFF).unwrap_or(0)
        };

        for width in [2_usize, 4, 6, 8, 14, 16, 18, 30, 32, 34, 62, 64, 96, 130] {
            let pairs = width / 2;
            let luma: Vec<u8> = (0..width).map(|_| next()).collect();
            let blue: Vec<u8> = (0..pairs).map(|_| next()).collect();
            let red: Vec<u8> = (0..pairs).map(|_| next()).collect();
            let planes = Planes {
                luma: &luma,
                luma_stride: width,
                blue: &blue,
                red: &red,
                chroma_stride: pairs,
            };
            let target = Target {
                stride: width * 4,
                width,
                height: 1,
            };
            for coefficients in [&YUV_RGB_709, &YUV_RGB_601] {
                let mut expected = vec![0_u8; width * 4];
                let source = planes
                    .row(0, width)
                    .unwrap_or_else(|| panic!("width {width}: short row"));
                super::scalar::bgra_row(&source, &mut expected, coefficients);

                let mut actual = vec![0_u8; width * 4];
                yuv422_to_bgra(&planes, target, &mut actual, coefficients)
                    .unwrap_or_else(|()| panic!("width {width}: conversion refused"));
                assert_eq!(actual, expected, "width {width}: kernels disagree");
            }
        }
    }

    /// Saturation is where a lane-wise translation is easiest to get wrong, so
    /// the extremes of both planes are pinned rather than left to the sampler.
    #[test]
    fn kernels_agree_at_the_saturating_extremes() {
        let width = 64_usize;
        let pairs = width / 2;
        for luma_value in [0_u8, 16, 128, 235, 255] {
            for chroma in [0_u8, 1, 128, 254, 255] {
                let luma = vec![luma_value; width];
                let blue = vec![chroma; pairs];
                let red = vec![255 - chroma; pairs];
                let planes = Planes {
                    luma: &luma,
                    luma_stride: width,
                    blue: &blue,
                    red: &red,
                    chroma_stride: pairs,
                };
                let target = Target {
                    stride: width * 4,
                    width,
                    height: 1,
                };
                let mut expected = vec![0_u8; width * 4];
                let source = planes.row(0, width).unwrap_or_else(|| panic!("short row"));
                super::scalar::bgra_row(&source, &mut expected, &YUV_RGB_709);

                let mut actual = vec![0_u8; width * 4];
                yuv422_to_bgra(&planes, target, &mut actual, &YUV_RGB_709)
                    .unwrap_or_else(|()| panic!("conversion refused"));
                assert_eq!(
                    actual, expected,
                    "luma {luma_value}, chroma {chroma}: kernels disagree"
                );
            }
        }
    }
}
