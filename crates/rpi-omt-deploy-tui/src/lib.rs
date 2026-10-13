//! The terminal deployer: state and input (app), the cell buffer (screen),
//! the views (ui), and the terminal itself (term). Split from the binary so
//! the tests drive the same code the operator does.

#![forbid(unsafe_code)]
// Screen coordinates are bounded to 1000 cells a side (Screen::begin), and
// the focus and cursor indexes to a form's rows and FIELD_LIMIT; the casts
// between them cannot wrap.
// The app mirrors the form's independent toggles as plain flags.
#![allow(clippy::struct_excessive_bools)]
#![allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap, clippy::cast_sign_loss)]

pub mod app;
pub mod screen;
pub mod term;
pub mod ui;
