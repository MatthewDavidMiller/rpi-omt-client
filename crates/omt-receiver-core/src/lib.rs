//! Receiver policy that has no hardware behind it: the decode ceiling, the
//! playback status projection the Web frontend reads, and detail sanitizing.

#![forbid(unsafe_code)]

mod ceiling;
mod status;

pub use ceiling::{CEILING_MAX_SHAPES, Shape, VideoCeiling};
pub use status::{
    AudioState, Connector, DETAIL_LIMIT, HEARTBEAT_MS, PlaybackStatus, VIDEO_STATE_COUNT,
    VideoState, durable_status_parent, sanitize_detail,
};
