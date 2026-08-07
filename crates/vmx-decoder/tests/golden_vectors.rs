// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
//
// Conformance: every committed stream must decode to exactly the bytes the
// Open Media Transport reference decoder produced before it was removed. The
// expected images are pinned by digest so the fixtures stay small; see
// tests/vectors/vmx/vectors.json.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use vmx_decoder::{ColorSpace, DecodeError, Decoder, Dimensions};

#[derive(Deserialize)]
struct Vector {
    label: String,
    width: usize,
    height: usize,
    color_space: i32,
    stream: String,
    uyvy_sha256: String,
    bgrx_sha256: String,
}

#[derive(Deserialize)]
struct Vectors {
    vectors: Vec<Vector>,
}

fn directory() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/vectors/vmx")
}

fn load() -> Vectors {
    let text = std::fs::read_to_string(directory().join("vectors.json"))
        .unwrap_or_else(|error| panic!("vector index: {error}"));
    let parsed: Vectors =
        serde_json::from_str(&text).unwrap_or_else(|error| panic!("vector index: {error}"));
    assert!(!parsed.vectors.is_empty(), "vector index is empty");
    parsed
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn decoder(vector: &Vector, workers: usize) -> Decoder {
    Decoder::new(
        Dimensions {
            width: vector.width,
            height: vector.height,
        },
        ColorSpace::resolve(vector.color_space, vector.height),
        workers,
    )
    .unwrap_or_else(|error| panic!("{}: {error}", vector.label))
}

#[test]
fn decodes_every_vector_bit_exactly() {
    for vector in load().vectors {
        let compressed = std::fs::read(directory().join(&vector.stream))
            .unwrap_or_else(|error| panic!("{}: {error}", vector.label));
        // Worker counts must not change a single output byte.
        for workers in [1_usize, 2, 3, 8] {
            let mut decoder = decoder(&vector, workers);
            decoder
                .load(&compressed)
                .unwrap_or_else(|error| panic!("{}: {error}", vector.label));

            let mut uyvy = vec![0_u8; vector.width * vector.height * 2];
            decoder
                .decode_uyvy(&mut uyvy, vector.width * 2)
                .unwrap_or_else(|error| panic!("{}: {error}", vector.label));
            assert_eq!(
                digest(&uyvy),
                vector.uyvy_sha256,
                "{} UYVY with {workers} workers",
                vector.label
            );

            decoder
                .load(&compressed)
                .unwrap_or_else(|error| panic!("{}: {error}", vector.label));
            let mut bgrx = vec![0_u8; vector.width * vector.height * 4];
            decoder
                .decode_bgrx(&mut bgrx, vector.width * 4)
                .unwrap_or_else(|error| panic!("{}: {error}", vector.label));
            assert_eq!(
                digest(&bgrx),
                vector.bgrx_sha256,
                "{} BGRX with {workers} workers",
                vector.label
            );
        }
    }
}

#[test]
fn repeated_lifecycles_are_stable() {
    let vectors = load().vectors;
    let vector = vectors
        .iter()
        .find(|candidate| candidate.width == 1920)
        .unwrap_or_else(|| panic!("no 1080p vector"));
    let compressed =
        std::fs::read(directory().join(&vector.stream)).unwrap_or_else(|error| panic!("{error}"));
    let mut decoder = decoder(vector, 4);
    let mut output = vec![0_u8; vector.width * vector.height * 4];
    for _ in 0..8 {
        decoder
            .load(&compressed)
            .unwrap_or_else(|error| panic!("{error}"));
        decoder
            .decode_bgrx(&mut output, vector.width * 4)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(digest(&output), vector.bgrx_sha256);
    }
}

#[test]
fn rejects_malformed_and_truncated_streams() {
    let vectors = load().vectors;
    let vector = &vectors[0];
    let compressed =
        std::fs::read(directory().join(&vector.stream)).unwrap_or_else(|error| panic!("{error}"));
    let mut decoder = decoder(vector, 1);

    assert_eq!(decoder.load(&[]), Err(DecodeError::Empty));
    assert_eq!(decoder.load(&[1, 2, 3]), Err(DecodeError::Truncated));
    assert_eq!(
        decoder.load(&[9, 0, 0, 0, 0]),
        Err(DecodeError::InvalidFormat)
    );
    assert_eq!(
        decoder.load(&[2, 0, 0, 0, 0]),
        Err(DecodeError::UnsupportedFormat)
    );

    // A stream that claims the wrong geometry never reaches the bit reader.
    // The slice count trails the envelope, which the extended form widens.
    let slice_count_offset = if compressed[0] == 3 { 4 } else { 2 };
    let mut wrong = compressed.clone();
    wrong[slice_count_offset] = wrong[slice_count_offset].wrapping_add(1);
    assert_eq!(decoder.load(&wrong), Err(DecodeError::SliceCount));

    // Every truncation of a valid stream is rejected or decodes without a
    // panic, an out-of-bounds access, or an unbounded allocation.
    let mut output = vec![0_u8; vector.width * vector.height * 4];
    for length in 1..compressed.len() {
        if decoder.load(&compressed[..length]).is_ok() {
            let _outcome = decoder.decode_bgrx(&mut output, vector.width * 4);
        }
    }

    // Bit flips anywhere in the payload must stay inside the same envelope.
    for position in (5..compressed.len()).step_by(17) {
        let mut damaged = compressed.clone();
        damaged[position] ^= 0xA5;
        if decoder.load(&damaged).is_ok() {
            let _outcome = decoder.decode_bgrx(&mut output, vector.width * 4);
        }
    }
}

#[test]
fn rejects_undersized_destinations() {
    let vectors = load().vectors;
    let vector = &vectors[0];
    let compressed =
        std::fs::read(directory().join(&vector.stream)).unwrap_or_else(|error| panic!("{error}"));
    let mut decoder = decoder(vector, 1);
    decoder
        .load(&compressed)
        .unwrap_or_else(|error| panic!("{error}"));

    let mut short = vec![0_u8; vector.width * vector.height * 4 - 1];
    assert_eq!(
        decoder.decode_bgrx(&mut short, vector.width * 4),
        Err(DecodeError::OutputSize)
    );
    let mut full = vec![0_u8; vector.width * vector.height * 4];
    assert_eq!(
        decoder.decode_bgrx(&mut full, vector.width * 4 - 1),
        Err(DecodeError::OutputSize)
    );
}

#[test]
fn rejects_unsupported_geometry() {
    for (width, height, workers) in [
        (8, 32, 1),
        (1922, 1080, 1),
        (64, 8, 1),
        (64, 1082, 1),
        (65, 32, 1),
        (64, 32, 0),
        (64, 32, 9),
    ] {
        assert!(
            Decoder::new(Dimensions { width, height }, ColorSpace::Bt709, workers).is_err(),
            "{width}x{height} with {workers} workers"
        );
    }
}
