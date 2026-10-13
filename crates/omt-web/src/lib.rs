//! omt-web: the appliance's HTTPS operator interface -- sessions and CSRF,
//! the dashboard and settings pages, bounded diagnostics, and the support
//! bundle -- served by a single-threaded poll loop over OpenSSL.

#![forbid(unsafe_code)]

pub mod app;
pub mod auth;
pub mod diagnostics;
pub mod http;
pub mod network;
pub mod playback;
pub mod settings;
pub mod state;
#[rustfmt::skip]
pub mod templates_gen;
pub mod tv;
pub mod zip;
