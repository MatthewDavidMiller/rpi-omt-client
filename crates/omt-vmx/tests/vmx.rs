//! VMX conformance: every committed stream must decode to exactly the bytes
//! the Open Media Transport reference decoder produced, pinned by digest in
//! tests/vectors/vmx/vectors.json. Also the kernel differential checks,
//! which on AArch64 hold the assembly to the portable kernels bit for bit.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// Fixture construction truncates random words into samples on purpose.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::many_single_char_names
)]

use omt_common::json::{self, Value};
use omt_vmx::kernels;
use omt_vmx::{ColorSpace, Decoder, Placement, PlaneMut, Status};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/vectors/vmx/");

struct Vector {
    label: String,
    width: usize,
    height: usize,
    color_space: i32,
    stream: Vec<u8>,
    uyvy: String,
    bgrx: String,
}

fn vectors() -> Vec<Vector> {
    let raw =
        omt_common::fsio::read_bounded(format!("{DIR}vectors.json"), 1 << 20).unwrap().unwrap();
    let root = json::parse(&raw).unwrap();
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap().to_owned();
    root.get("vectors")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .map(|v| Vector {
            label: text(v, "label"),
            width: v.get("width").and_then(Value::as_u64).unwrap() as usize,
            height: v.get("height").and_then(Value::as_u64).unwrap() as usize,
            color_space: v.get("color_space").and_then(Value::as_i64).unwrap() as i32,
            stream: omt_common::fsio::read_bounded(
                format!("{DIR}{}", text(v, "stream")),
                omt_vmx::MAX_COMPRESSED_BYTES,
            )
            .unwrap()
            .unwrap(),
            uyvy: text(v, "uyvy_sha256"),
            bgrx: text(v, "bgrx_sha256"),
        })
        .collect()
}

fn make(v: &Vector, workers: usize) -> Decoder {
    Decoder::new(v.width, v.height, ColorSpace::resolve(v.color_space, v.height), workers).unwrap()
}

fn sha(data: &[u8]) -> String {
    omt_crypto::sha256_hex(data).unwrap()
}

#[test]
#[cfg_attr(miri, ignore = "exhaustive; covered under ASan and on AArch64")]
fn decodes_every_vector_bit_exactly() {
    let all = vectors();
    assert!(!all.is_empty());
    for v in &all {
        for workers in [1, 2, 3, 8] {
            let mut d = make(v, workers);
            let mut uyvy = vec![0_u8; v.width * v.height * 2];
            d.load(&v.stream).unwrap();
            d.decode_uyvy(&mut uyvy, v.width * 2).unwrap();
            assert_eq!(sha(&uyvy), v.uyvy, "{} UYVY with {workers} workers", v.label);
            let mut bgrx = vec![0_u8; v.width * v.height * 4];
            d.load(&v.stream).unwrap();
            d.decode_bgrx(&mut bgrx, v.width * 4).unwrap();
            assert_eq!(sha(&bgrx), v.bgrx, "{} BGRX with {workers} workers", v.label);
        }
    }
}

#[test]
fn repeated_lifecycles_are_stable() {
    let all = vectors();
    let v = all.iter().find(|v| v.width == 1920).unwrap();
    let mut d = make(v, 4);
    let mut out = vec![0_u8; v.width * v.height * 4];
    for _ in 0..8 {
        d.load(&v.stream).unwrap();
        d.decode_bgrx(&mut out, v.width * 4).unwrap();
        assert_eq!(sha(&out), v.bgrx);
    }
}

#[test]
fn rejects_malformed_and_truncated_streams() {
    let all = vectors();
    let v = &all[0];
    let mut d = make(v, 1);
    assert_eq!(d.load(&[]), Err(Status::Empty));
    assert_eq!(d.load(&[1, 2, 3]), Err(Status::Truncated));
    assert_eq!(d.load(&[9, 0, 0, 0, 0]), Err(Status::InvalidFormat));
    assert_eq!(d.load(&[2, 0, 0, 0, 0]), Err(Status::UnsupportedFormat));
    let at = if v.stream[0] == 3 { 4 } else { 2 };
    let mut wrong = v.stream.clone();
    wrong[at] += 1;
    assert_eq!(d.load(&wrong), Err(Status::SliceCount));
    let mut out = vec![0_u8; v.width * v.height * 4];
    for length in 1..v.stream.len() {
        if d.load(&v.stream[..length]).is_ok() {
            let _ = d.decode_bgrx(&mut out, v.width * 4);
        }
    }
    for pos in (5..v.stream.len()).step_by(17) {
        let mut wrong = v.stream.clone();
        wrong[pos] ^= 0xA5;
        if d.load(&wrong).is_ok() {
            let _ = d.decode_bgrx(&mut out, v.width * 4);
        }
    }
}

#[test]
fn rejects_undersized_destinations() {
    let all = vectors();
    let v = &all[0];
    let mut d = make(v, 1);
    d.load(&v.stream).unwrap();
    let len = v.width * v.height * 4;
    let mut out = vec![0_u8; len];
    assert_eq!(d.decode_bgrx(&mut out[..len - 1], v.width * 4), Err(Status::OutputSize));
    assert_eq!(d.decode_bgrx(&mut out, v.width * 4 - 1), Err(Status::OutputSize));
}

/// Pixel-centre nearest-neighbour samples, the rule the receiver's scaler
/// builds its tables with.
fn samples(destination: usize, source: usize) -> Vec<u32> {
    (0..destination)
        .map(|i| (((2 * i + 1) * source) / (2 * destination)).min(source - 1) as u32)
        .collect()
}

/// A placed decode is a full decode followed by a gather: every placed pixel
/// matches, and nothing outside the placed area is touched.
#[test]
fn placed_decodes_match_decode_then_gather() {
    for v in &vectors() {
        let mut full = vec![0_u8; v.width * v.height * 4];
        let mut d = make(v, 3);
        d.load(&v.stream).unwrap();
        d.decode_bgrx(&mut full, v.width * 4).unwrap();
        // Shrunk into the top-left, enlarged and letterboxed, and placed at
        // its own size off-centre.
        let shapes = [
            [v.width, v.height, 0, 0, v.width * 2 / 3 + 1, v.height * 2 / 3 + 1],
            [v.width * 3, v.height * 2 + 9, 7, 5, v.width * 2 + 3, v.height * 2],
            [v.width + 6, v.height + 2, 3, 1, v.width, v.height],
        ];
        for (c, [mode_w, mode_h, px, py, pw, ph]) in shapes.into_iter().enumerate() {
            for workers in [1, 4] {
                let columns = samples(pw, v.width);
                let rows = samples(ph, v.height);
                let p = Placement { x: px, y: py, columns: &columns, rows: &rows };
                let stride = mode_w * 4 + 12;
                let len = stride * mode_h;
                let mut expected = vec![0xA5_u8; len];
                let mut actual = vec![0xA5_u8; len];
                for (y, &r) in rows.iter().enumerate() {
                    for (x, &col) in columns.iter().enumerate() {
                        let dst = (py + y) * stride + (px + x) * 4;
                        let src = r as usize * v.width * 4 + col as usize * 4;
                        expected[dst..dst + 4].copy_from_slice(&full[src..src + 4]);
                    }
                }
                let mut placed = make(v, workers);
                placed.load(&v.stream).unwrap();
                placed.decode_bgrx_placed(&mut actual, stride, &p).unwrap();
                assert!(expected == actual, "{} shape {c} with {workers} workers", v.label);
            }
        }
    }
}

/// Every index a worker follows is checked before any worker runs.
#[test]
fn placed_decodes_reject_bad_placements() {
    let all = vectors();
    let v = &all[0];
    let mut d = make(v, 2);
    d.load(&v.stream).unwrap();
    let mut columns = samples(v.width, v.width);
    let mut rows = samples(v.height, v.height);
    let stride = v.width * 4;
    let len = stride * v.height;
    let mut out = vec![0_u8; len];
    let p = Placement { x: 0, y: 0, columns: &columns, rows: &rows };
    assert_eq!(d.decode_bgrx_placed(&mut out, stride, &p), Ok(()));
    assert_eq!(d.decode_bgrx_placed(&mut out[..len - 1], stride, &p), Err(Status::OutputSize));
    assert_eq!(d.decode_bgrx_placed(&mut out, stride - 4, &p), Err(Status::OutputSize));
    let shifted = Placement { x: 1, ..p };
    assert_eq!(d.decode_bgrx_placed(&mut out, stride, &shifted), Err(Status::OutputSize));
    let shifted = Placement { y: usize::MAX, ..p };
    assert_eq!(d.decode_bgrx_placed(&mut out, stride, &shifted), Err(Status::OutputSize));
    let empty = Placement { columns: &[], ..p };
    assert_eq!(d.decode_bgrx_placed(&mut out, stride, &empty), Err(Status::InvalidDimensions));
    columns[v.width - 1] = v.width as u32;
    let p = Placement { x: 0, y: 0, columns: &columns, rows: &rows };
    assert_eq!(d.decode_bgrx_placed(&mut out, stride, &p), Err(Status::InvalidDimensions));
    columns[v.width - 1] = 0;
    rows[1] = rows[0] + 3;
    rows[2] = rows[0];
    let p = Placement { x: 0, y: 0, columns: &columns, rows: &rows };
    assert_eq!(d.decode_bgrx_placed(&mut out, stride, &p), Err(Status::InvalidDimensions));
    rows[2] = rows[1];
    rows[v.height - 1] = v.height as u32;
    let p = Placement { x: 0, y: 0, columns: &columns, rows: &rows };
    assert_eq!(d.decode_bgrx_placed(&mut out, stride, &p), Err(Status::InvalidDimensions));
}

/// Planar 4:2:2 is the same samples UYVY carries, so re-interleaved it must
/// be the UYVY the conformance vectors pin; padding past each row is left
/// alone.
#[test]
fn planar_output_is_the_pinned_uyvy() {
    for v in &vectors() {
        let (w, h, half) = (v.width, v.height, v.width / 2);
        let mut uyvy = vec![0_u8; w * h * 2];
        for workers in [1, 4] {
            let mut d = make(v, workers);
            d.load(&v.stream).unwrap();
            d.decode_uyvy(&mut uyvy, w * 2).unwrap();
            let strides = [w + 32, half + 16, half + 16];
            let mut planes: Vec<Vec<u8>> = strides.iter().map(|s| vec![0x5A; s * h]).collect();
            d.load(&v.stream).unwrap();
            {
                let [a, b, c] = &mut planes[..] else { unreachable!() };
                d.decode_yuv422p([
                    PlaneMut { data: a, stride: strides[0] },
                    PlaneMut { data: b, stride: strides[1] },
                    PlaneMut { data: c, stride: strides[2] },
                ])
                .unwrap();
            }
            let mut interleaved = vec![0_u8; w * h * 2];
            for y in 0..h {
                let luma = &planes[0][y * strides[0]..];
                let cb = &planes[1][y * strides[1]..];
                let cr = &planes[2][y * strides[2]..];
                for x in 0..half {
                    let o = &mut interleaved[y * w * 2 + x * 4..];
                    o[..4].copy_from_slice(&[cb[x], luma[2 * x], cr[x], luma[2 * x + 1]]);
                }
                assert!(
                    luma[w] == 0x5A && cb[half] == 0x5A && cr[half] == 0x5A,
                    "{} padding",
                    v.label
                );
            }
            assert!(uyvy == interleaved, "{} with {workers} workers", v.label);
            // Every plane is held to its own extent.
            for p in 0..3 {
                let mut bad: Vec<Vec<u8>> = planes.clone();
                bad[p].pop();
                let [a, b, c] = &mut bad[..] else { unreachable!() };
                let r = d.decode_yuv422p([
                    PlaneMut { data: a, stride: strides[0] },
                    PlaneMut { data: b, stride: strides[1] },
                    PlaneMut { data: c, stride: strides[2] },
                ]);
                assert_eq!(r, Err(Status::OutputSize));
                let mut narrow = strides;
                narrow[p] = if p == 0 { w } else { half } - 1;
                let [a, b, c] = &mut planes[..] else { unreachable!() };
                let r = d.decode_yuv422p([
                    PlaneMut { data: a, stride: narrow[0] },
                    PlaneMut { data: b, stride: narrow[1] },
                    PlaneMut { data: c, stride: narrow[2] },
                ]);
                assert_eq!(r, Err(Status::OutputSize));
            }
        }
    }
}

#[test]
fn rejects_unsupported_geometry() {
    for (w, h, workers) in [
        (8, 32, 1),
        (1922, 1080, 1),
        (64, 8, 1),
        (64, 1082, 1),
        (65, 32, 1),
        (64, 32, 0),
        (64, 32, 9),
    ] {
        assert_eq!(
            Decoder::new(w, h, ColorSpace::Bt709, workers).map(|_| ()),
            Err(Status::InvalidDimensions),
            "{w}x{h} with {workers}"
        );
    }
}

fn xorshift(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

/// On AArch64 this pins the assembly kernels to the portable ones;
/// elsewhere the dispatch resolves to the portable kernel and the check is
/// trivially true, which is why the suite also runs under emulated AArch64.
#[test]
fn idct_kernels_agree_bit_for_bit() {
    let mut state = 0x2545_F491_4F6C_DD1D_u64;
    for round in 0..2000 {
        let mut block = [0_i16; 64];
        for (i, b) in block.iter_mut().enumerate() {
            let raw = xorshift(&mut state);
            *b = match round % 4 {
                0 => (raw % 512) as i16 - 256,
                1 => {
                    if i == 0 {
                        (raw % 4096) as i16 - 2048
                    } else {
                        0
                    }
                }
                2 => raw as u16 as i16,
                _ => match raw % 8 {
                    0 => i16::MAX,
                    1 => i16::MIN,
                    _ => 0,
                },
            };
        }
        let matrix = kernels::decode_matrix(round % 25);
        for bias in [0, 128] {
            let (mut expected, mut actual) = ([0_u8; 64], [0_u8; 64]);
            kernels::idct_scalar(&block, &matrix, &mut expected, 8, bias);
            kernels::idct(&block, &matrix, &mut actual, 8, bias, 8);
            assert_eq!(expected, actual, "round {round} bias {bias}");
        }
    }
    // One coefficient at a time, at every position, under every quality, at
    // the values that saturate each stage of the row and column passes.
    for quality in 0..kernels::QUALITY_COUNT {
        let matrix = kernels::decode_matrix(quality);
        for position in 0..64 {
            for value in [i16::MIN, -2048, -1, 1, 2047, i16::MAX] {
                let mut block = [0_i16; 64];
                block[position] = value;
                let (mut expected, mut actual) = ([0_u8; 64], [0_u8; 64]);
                kernels::idct_scalar(&block, &matrix, &mut expected, 8, 128);
                kernels::idct(&block, &matrix, &mut actual, 8, 128, (position / 8 + 1) as u8);
                assert_eq!(expected, actual, "quality {quality} position {position} value {value}");
            }
        }
    }
    // Every row bound: coefficients confined to rows 0..rows-1, so a kernel
    // that skips the empty rows must still produce the full transform.
    for rows in 1..=8_u8 {
        for round in 0..400 {
            let mut block = [0_i16; 64];
            for b in block.iter_mut().take(usize::from(rows) * 8) {
                let raw = xorshift(&mut state);
                *b = if round % 2 == 1 { raw as u16 as i16 } else { (raw % 512) as i16 - 256 };
            }
            let matrix = kernels::decode_matrix(round % kernels::QUALITY_COUNT);
            let (mut expected, mut actual) = ([0_u8; 64], [0_u8; 64]);
            kernels::idct_scalar(&block, &matrix, &mut expected, 8, 128);
            kernels::idct(&block, &matrix, &mut actual, 8, 128, rows);
            assert_eq!(expected, actual, "rows {rows} round {round}");
        }
    }
}

#[test]
fn convert_kernels_agree_bit_for_bit() {
    let mut state = 0x1234_5678_9ABC_DEF1_u64;
    for width in [2, 4, 6, 8, 14, 16, 18, 30, 32, 34, 62, 64, 96, 130] {
        let pairs = width / 2;
        let mut luma = [0_u8; 256];
        let mut blue = [0_u8; 128];
        let mut red = [0_u8; 128];
        for b in &mut luma[..width] {
            *b = (xorshift(&mut state) >> 24) as u8;
        }
        for b in &mut blue[..pairs] {
            *b = (xorshift(&mut state) >> 24) as u8;
        }
        for b in &mut red[..pairs] {
            *b = (xorshift(&mut state) >> 24) as u8;
        }
        for c in [&kernels::YUV_RGB_709, &kernels::YUV_RGB_601] {
            let (mut expected, mut actual) = ([0_u8; 1024], [0_u8; 1024]);
            kernels::bgra_row_scalar(&luma, &blue, &red, width, &mut expected, c);
            kernels::bgra_row(&luma, &blue, &red, width, &mut actual, c);
            assert_eq!(expected[..width * 4], actual[..width * 4], "width {width}");
        }
    }
    for l in [0, 16, 128, 235, 255] {
        for cv in [0_u8, 1, 128, 254, 255] {
            let (luma, blue, red) = ([l; 64], [cv; 32], [255 - cv; 32]);
            let (mut expected, mut actual) = ([0_u8; 256], [0_u8; 256]);
            kernels::bgra_row_scalar(&luma, &blue, &red, 64, &mut expected, &kernels::YUV_RGB_709);
            kernels::bgra_row(&luma, &blue, &red, 64, &mut actual, &kernels::YUV_RGB_709);
            assert_eq!(expected, actual);
        }
    }
    // Every (Y, U, V) under both coefficient sets. The AArch64 kernel's
    // rounding is only the scalar one for the channel values these
    // coefficients can produce (see convert_aarch64.s), so this is the
    // proof, not a sample.
    if cfg!(target_arch = "aarch64") {
        let luma: Vec<u8> = (0..=255).collect();
        let mut mismatches = 0;
        for c in [&kernels::YUV_RGB_709, &kernels::YUV_RGB_601] {
            for u in 0..=255_u8 {
                for v in 0..=255_u8 {
                    let (blue, red) = ([u; 128], [v; 128]);
                    let (mut expected, mut actual) = ([0_u8; 1024], [0_u8; 1024]);
                    kernels::bgra_row_scalar(&luma, &blue, &red, 256, &mut expected, c);
                    kernels::bgra_row(&luma, &blue, &red, 256, &mut actual, c);
                    mismatches += usize::from(expected != actual);
                }
            }
        }
        assert_eq!(mismatches, 0, "{mismatches} rows differ");
    }
}
