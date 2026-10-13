//! Hand-written foreign declarations: the C library calls the standard
//! library does not wrap, OpenSSL, ALSA, the Linux DRM uapi, and Win32.
//!
//! This is one of the few crates allowed `unsafe`. Declarations here are
//! checked against the real C headers by the ABI probe (tests/abi), which
//! compares every struct size, field offset, and constant this crate defines
//! with what the platform headers say, for each target the project ships.
//! Code outside this crate calls the safe wrappers, never the raw items.

#![allow(non_camel_case_types, non_snake_case)]
// Converting between Rust sizes and the C ABI's fixed-width fields is this
// crate's job; each conversion is to the type the kernel or library declares,
// and the values are bounded by the structures they describe.
#![allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap, clippy::cast_sign_loss)]

/// Declares types C code only ever hands out by pointer: zero-sized,
/// unconstructible, and neither `Send`, `Sync`, nor `Unpin`.
#[allow(unused_macros)]
macro_rules! opaque {
    ($($name:ident),* $(,)?) => {$(
        #[repr(C)]
        #[derive(Debug)]
        pub struct $name {
            _data: [u8; 0],
            _marker: std::marker::PhantomData<(*mut u8, std::marker::PhantomPinned)>,
        }
    )*};
}

#[cfg(unix)]
pub mod posix;

#[cfg(feature = "openssl")]
pub mod openssl;

#[cfg(feature = "openssl")]
pub mod evp;

#[cfg(feature = "ssl")]
pub mod ssl;

#[cfg(windows)]
pub mod win32;

#[cfg(target_os = "linux")]
pub mod drm;

#[cfg(all(target_os = "linux", feature = "alsa"))]
pub mod alsa;
