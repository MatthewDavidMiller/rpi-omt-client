//! Link directives for the libraries this crate declares.
//!
//! Nothing is compiled here; the build script only tells rustc which system
//! libraries to link, and how. `OMT_OPENSSL_STATIC=1` links OpenSSL
//! statically (the static Linux and Windows deployers), and
//! `OMT_OPENSSL_DIR` names a non-system OpenSSL prefix (the pinned Windows
//! build from scripts/build-openssl.sh).

use std::env;

fn main() {
    println!("cargo:rerun-if-env-changed=OMT_OPENSSL_STATIC");
    println!("cargo:rerun-if-env-changed=OMT_OPENSSL_DIR");
    let windows = env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows");
    if env::var_os("CARGO_FEATURE_OPENSSL").is_some() {
        let kind =
            if env::var("OMT_OPENSSL_STATIC").is_ok_and(|v| v == "1") { "static" } else { "dylib" };
        if let Ok(dir) = env::var("OMT_OPENSSL_DIR") {
            println!("cargo:rustc-link-search=native={dir}/lib64");
            println!("cargo:rustc-link-search=native={dir}/lib");
        }
        if env::var_os("CARGO_FEATURE_SSL").is_some() {
            println!("cargo:rustc-link-lib={kind}=ssl");
        }
        println!("cargo:rustc-link-lib={kind}=crypto");
        if windows {
            // What a static libcrypto needs from the platform.
            for lib in ["ws2_32", "crypt32", "bcrypt", "gdi32", "user32", "advapi32"] {
                println!("cargo:rustc-link-lib=dylib={lib}");
            }
        } else {
            println!("cargo:rustc-link-lib=dylib=pthread");
        }
    }
    if env::var_os("CARGO_FEATURE_ALSA").is_some() {
        println!("cargo:rustc-link-lib=dylib=asound");
    }
}
