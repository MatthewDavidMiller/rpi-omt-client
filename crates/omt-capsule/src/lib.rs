//! The manifest-v3 capsule as compiled into this binary: every member of
//! deploy/manifest-v3.txt, the ARM64 image archive included. Linked only into
//! the deployer binaries, so the rest of the deployer builds and tests
//! without the image.

#![forbid(unsafe_code)]

include!(concat!(env!("OUT_DIR"), "/capsule.rs"));
