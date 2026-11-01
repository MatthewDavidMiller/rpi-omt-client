//! Decode throughput. A measurement, not a gate, so it is not one of the
//! suites; it reports per worker count so a regression in single-core cost
//! cannot hide behind the pool.
//!
//!   cargo run --release -p omt-vmx --bin bench_vmx
//!
//! Run this on each supported board. The per-board ceilings in
//! deploy/lib/board-profile.sh are targets derived from core count and
//! clock, and this is what confirms or refutes them. The receiver gives the
//! decoder three of the four cores every supported board has, so the
//! 3-worker row decides a tier: if it cannot sustain the frame interval its
//! ceiling promises, lower that board's profile.
//!
//! Each case is measured for the three outputs the receiver can scan out:
//! BGRX, planar YUV 4:2:2 (what ships on Pi 4 and Pi 5), and BGRX placed
//! into a 1280x720 mode. `VMX_VECTOR_DIR` overrides where the vectors are
//! read from, for a run on a board without the checkout.

#![allow(
    clippy::print_stdout,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::many_single_char_names
)]

use std::process::ExitCode;
use std::time::Instant;

use omt_vmx::{ColorSpace, Decoder, Placement, PlaneMut, Status};

const FRAMES: i32 = 60;

const CASES: [(&str, usize, usize, ColorSpace); 5] = [
    ("gradient-1920x1080-709", 1920, 1080, ColorSpace::Bt709),
    ("flat-1920x1080-709", 1920, 1080, ColorSpace::Bt709),
    ("gradient-720x480-601", 720, 480, ColorSpace::Bt601),
    // Small, but every block is dense, and edges is all row 0: per-block
    // cost at both ends of the sparsity the transform's fast path depends on.
    ("noise-128x80-709", 128, 80, ColorSpace::Bt709),
    ("edges-320x176-709", 320, 176, ColorSpace::Bt709),
];

#[derive(Clone, Copy)]
enum Output {
    Bgrx,
    Yuv422p,
    Placed720,
}

fn samples(destination: usize, source: usize) -> Vec<u32> {
    (0..destination)
        .map(|i| (((2 * i + 1) * source) / (2 * destination)).min(source - 1) as u32)
        .collect()
}

struct Buffers {
    bgrx: Vec<u8>,
    planes: [Vec<u8>; 3],
    placed: Vec<u8>,
    columns: Vec<u32>,
    rows: Vec<u32>,
}

fn decode(d: &mut Decoder, b: &mut Buffers, output: Output, width: usize) -> Result<(), Status> {
    match output {
        Output::Bgrx => d.decode_bgrx(&mut b.bgrx, width * 4),
        Output::Yuv422p => {
            let [y, u, v] = &mut b.planes;
            d.decode_yuv422p([
                PlaneMut { data: y, stride: width },
                PlaneMut { data: u, stride: width / 2 },
                PlaneMut { data: v, stride: width / 2 },
            ])
        }
        Output::Placed720 => {
            let p = Placement { x: 0, y: 0, columns: &b.columns, rows: &b.rows };
            d.decode_bgrx_placed(&mut b.placed, 1280 * 4, &p)
        }
    }
}

fn main() -> ExitCode {
    let dir = std::env::var("VMX_VECTOR_DIR").unwrap_or_else(|_| "tests/vectors/vmx".to_owned());
    // BENCH_ONLY=<label>[:<output>[:<workers>]] narrows the run, for profiling.
    let only = std::env::var("BENCH_ONLY").ok();
    println!("\nDecode, {FRAMES} frames per measurement");
    println!("The receiver uses 3 workers; that row decides a board's tier.\n");
    for (label, width, height, color_space) in CASES {
        if only.as_deref().is_some_and(|o| o.split(':').next() != Some(label)) {
            continue;
        }
        println!("{label} ({width}x{height})");
        let path = format!("{dir}/{label}.vmx");
        let compressed = match omt_common::fsio::read_bounded(&path, omt_vmx::MAX_COMPRESSED_BYTES)
        {
            Ok(Some(data)) if !data.is_empty() => data,
            _ => {
                eprintln!("cannot read {path}");
                return ExitCode::FAILURE;
            }
        };
        let mut buffers = Buffers {
            bgrx: vec![0; width * height * 4],
            planes: [
                vec![0; width * height],
                vec![0; width * height / 2],
                vec![0; width * height / 2],
            ],
            placed: vec![0; 1280 * 720 * 4],
            columns: samples(1280, width),
            rows: samples(720, height),
        };
        for (name, output) in [
            ("bgrx", Output::Bgrx),
            ("yuv422p", Output::Yuv422p),
            ("placed-720", Output::Placed720),
        ] {
            if only.as_deref().is_some_and(|o| o.split(':').nth(1).is_some_and(|n| n != name)) {
                continue;
            }
            for workers in [1, 3, 4] {
                if only
                    .as_deref()
                    .is_some_and(|o| o.split(':').nth(2).is_some_and(|w| w != workers.to_string()))
                {
                    continue;
                }
                let Ok(mut decoder) = Decoder::new(width, height, color_space, workers) else {
                    eprintln!("cannot create a decoder");
                    return ExitCode::FAILURE;
                };
                let mut start = Instant::now();
                for frame in -5..FRAMES {
                    if frame == 0 {
                        start = Instant::now();
                    }
                    let result = decoder
                        .load(&compressed)
                        .and_then(|()| decode(&mut decoder, &mut buffers, output, width));
                    if let Err(e) = result {
                        eprintln!("decode failed: {e}");
                        return ExitCode::FAILURE;
                    }
                }
                let per_frame = start.elapsed().as_secs_f64() * 1000.0 / f64::from(FRAMES);
                println!(
                    "  {name:<10} {workers} worker(s): {per_frame:6.2} ms/frame  {:6.1} fps   60 fps: {}  30 fps: {}",
                    1000.0 / per_frame,
                    if per_frame <= 1000.0 / 60.0 { "ok" } else { "OVER" },
                    if per_frame <= 1000.0 / 30.0 { "ok" } else { "OVER" },
                );
            }
        }
        println!();
    }
    ExitCode::SUCCESS
}
