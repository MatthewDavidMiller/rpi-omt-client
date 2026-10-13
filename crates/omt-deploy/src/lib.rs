//! The deployer core shared by the CLI and the terminal application: the
//! first-party SSH client, validation, the embedded capsule, and the
//! operations. Nothing here draws or prompts; progress is reported through
//! a callback and failures as errors.

#![forbid(unsafe_code)]
// The protocol code keeps the RFCs' own names (p, g, e, f, k, x for
// Diffie-Hellman; l and r for Blowfish halves) and mirrors the protocol's
// per-channel flags, which reads better than renaming them away.
#![allow(clippy::many_single_char_names, clippy::struct_excessive_bools)]

pub mod capsule;
pub mod core;
pub mod https;
pub mod jobs;
pub mod ops;
pub mod sd_card;
pub mod ssh;
pub mod sys;
pub mod tools;
