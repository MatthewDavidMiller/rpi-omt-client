//! The shared foundation every other crate builds on: errors, bounded
//! buffers, strict JSON and XML, Unicode NFC, file and process I/O for paths
//! and programs outside this process's control, operating-system randomness,
//! and UTC calendar arithmetic.
//!
//! Every reader here takes an explicit ceiling. Unbounded input handling is a
//! defect in this project even where growth looks impossible.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::disallowed_methods))]

pub mod buf;
pub mod error;
#[cfg(unix)]
pub mod fsio;
pub mod json;
pub mod nfc;
#[rustfmt::skip] // generated; tools/gen/gen_nfc_tables.py --check holds it
mod nfc_tables;
#[cfg(unix)]
pub mod proc;
pub mod rand;
pub mod text;
pub mod timefmt;
pub mod xml;

pub use error::{Error, Result};

/// The build's version, as `scripts/detect-version.sh` resolved it.
pub const VERSION: &str = env!("OMT_BUILD_VERSION");
