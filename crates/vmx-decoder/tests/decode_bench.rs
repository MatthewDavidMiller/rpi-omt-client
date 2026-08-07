// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// 1080p decode throughput. This is a measurement, not a gate: it is `#[ignore]`d
// so the normal suite stays deterministic, and reported per worker count so a
// regression in single-core cost cannot hide behind the pool.
//
//   cargo test --release -p vmx-decoder --test decode_bench -- --ignored --nocapture
//
// The appliance's budget is one 1920x1080 frame every 16.7 ms on a Pi 5, which
// has four Cortex-A76 cores and gives the decoder three of them.

use std::path::PathBuf;
use std::time::Instant;
use vmx_decoder::{ColorSpace, Decoder, Dimensions};

const WIDTH: usize = 1920;
const HEIGHT: usize = 1080;
const FRAMES: u32 = 60;

fn vector(label: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/vectors/vmx")
        .join(format!("{label}.vmx"));
    std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
#[ignore = "throughput measurement, not a correctness gate"]
fn decode_throughput() {
    println!("\n1920x1080 BGRX decode, {FRAMES} frames per measurement\n");
    for label in ["gradient-1920x1080-709", "flat-1920x1080-709"] {
        let compressed = vector(label);
        for workers in [1_usize, 3, 4] {
            let mut decoder = Decoder::new(
                Dimensions {
                    width: WIDTH,
                    height: HEIGHT,
                },
                ColorSpace::Bt709,
                workers,
            )
            .unwrap_or_else(|error| panic!("{error}"));
            let mut output = vec![0_u8; WIDTH * HEIGHT * 4];

            for _ in 0..5 {
                decoder
                    .load(&compressed)
                    .unwrap_or_else(|error| panic!("{error}"));
                decoder
                    .decode_bgrx(&mut output, WIDTH * 4)
                    .unwrap_or_else(|error| panic!("{error}"));
            }
            let start = Instant::now();
            for _ in 0..FRAMES {
                decoder
                    .load(&compressed)
                    .unwrap_or_else(|error| panic!("{error}"));
                decoder
                    .decode_bgrx(&mut output, WIDTH * 4)
                    .unwrap_or_else(|error| panic!("{error}"));
            }
            let per_frame = start.elapsed().as_secs_f64() * 1000.0 / f64::from(FRAMES);
            println!(
                "  {label:<24} {workers} worker(s): {per_frame:6.2} ms/frame  {:6.1} fps",
                1000.0 / per_frame
            );
        }
        println!();
    }
}
