//! The browser deployer: the state model (app), the loopback HTTP server
//! (http, server), the folder picker's view of the file system (fs), and the
//! About document (about). Split from the binary so the tests drive the
//! same code the operator does.

#![forbid(unsafe_code)]
// The app mirrors the form's independent toggles as plain flags.
#![allow(clippy::struct_excessive_bools)]
// Log sequence numbers index a deque bounded by LOG_CAPACITY.
#![allow(clippy::cast_possible_truncation)]

pub mod about;
pub mod app;
pub mod fs;
pub mod http;
pub mod server;
