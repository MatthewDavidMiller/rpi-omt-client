//! Stamps the build's version into this one crate.
//!
//! The scripts pass the value `scripts/detect-version.sh` resolves as
//! `OMT_VERSION`. Only this crate rebuilds when it changes; a bare `cargo build` falls back to
//! the VERSION file.

// The VERSION file is the repository's own one-line file, not outside input.
#![allow(clippy::disallowed_methods)]

use std::env;
use std::fs;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-env-changed=OMT_VERSION");
    let version = if let Ok(v) = env::var("OMT_VERSION") {
        v
    } else {
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../VERSION");
        println!("cargo:rerun-if-changed={}", file.display());
        match fs::read_to_string(&file) {
            Ok(text) => format!("v{}", text.trim().trim_start_matches('v')),
            Err(_) => "unknown".to_owned(),
        }
    };
    println!("cargo:rustc-env=OMT_BUILD_VERSION={version}");
}
