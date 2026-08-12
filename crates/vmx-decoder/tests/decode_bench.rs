// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// Decode throughput. This is a measurement, not a gate: it is `#[ignore]`d so
// the normal suite stays deterministic, and reported per worker count so a
// regression in single-core cost cannot hide behind the pool.
//
//   cargo test --release -p vmx-decoder --test decode_bench -- --ignored --nocapture
//
// Run this on each supported board. The per-board ceilings in
// `deploy/lib/board-profile.sh` are targets derived from core count and clock,
// not measurements, and this is what confirms or refutes them. The receiver
// gives the decoder three of the four cores every supported board has, so the
// 3-worker row is the one that decides a tier: if it cannot sustain the frame
// interval its ceiling promises, lower that board's profile.
//
// Only 1080p and 480p geometries are committed as vectors, so a 720p tier is
// bracketed rather than measured directly. Regenerating vectors needs the C
// reference encoder described in `tests/vectors/vmx/README.md`.

use std::path::PathBuf;
use std::time::Instant;
use vmx_decoder::{ColorSpace, Decoder, Dimensions};

const FRAMES: u32 = 60;

/// The frame intervals the shipped ceilings promise, for reading the results
/// against something rather than eyeballing them.
const BUDGETS: [(&str, f64); 2] = [("60 fps", 1000.0 / 60.0), ("30 fps", 1000.0 / 30.0)];

struct Case {
    label: &'static str,
    width: usize,
    height: usize,
    color_space: ColorSpace,
}

const CASES: [Case; 3] = [
    Case {
        label: "gradient-1920x1080-709",
        width: 1920,
        height: 1080,
        color_space: ColorSpace::Bt709,
    },
    Case {
        label: "flat-1920x1080-709",
        width: 1920,
        height: 1080,
        color_space: ColorSpace::Bt709,
    },
    Case {
        label: "gradient-720x480-601",
        width: 720,
        height: 480,
        color_space: ColorSpace::Bt601,
    },
];

fn vector(label: &str) -> Vec<u8> {
    let directory = std::env::var_os("VMX_VECTOR_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/vectors/vmx"),
        PathBuf::from,
    );
    let path = directory.join(format!("{label}.vmx"));
    std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
#[ignore = "throughput measurement, not a correctness gate"]
fn decode_throughput() {
    println!("\nBGRX decode, {FRAMES} frames per measurement");
    println!("The receiver uses 3 workers; that row decides a board's tier.\n");
    for case in &CASES {
        let Case {
            label,
            width,
            height,
            color_space,
        } = *case;
        println!("{label} ({width}x{height})");
        let compressed = vector(label);
        for workers in [1_usize, 3, 4] {
            let mut decoder = Decoder::new(Dimensions { width, height }, color_space, workers)
                .unwrap_or_else(|error| panic!("{error}"));
            let mut output = vec![0_u8; width * height * 4];

            for _ in 0..5 {
                decoder
                    .load(&compressed)
                    .unwrap_or_else(|error| panic!("{error}"));
                decoder
                    .decode_bgrx(&mut output, width * 4)
                    .unwrap_or_else(|error| panic!("{error}"));
            }
            let start = Instant::now();
            for _ in 0..FRAMES {
                decoder
                    .load(&compressed)
                    .unwrap_or_else(|error| panic!("{error}"));
                decoder
                    .decode_bgrx(&mut output, width * 4)
                    .unwrap_or_else(|error| panic!("{error}"));
            }
            let per_frame = start.elapsed().as_secs_f64() * 1000.0 / f64::from(FRAMES);
            let verdict: Vec<String> = BUDGETS
                .iter()
                .map(|(name, budget)| {
                    let mark = if per_frame <= *budget { "ok" } else { "OVER" };
                    format!("{name}: {mark}")
                })
                .collect();
            println!(
                "  {workers} worker(s): {per_frame:6.2} ms/frame  {:6.1} fps   {}",
                1000.0 / per_frame,
                verdict.join("  ")
            );
        }
        println!();
    }
}
