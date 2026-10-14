//! The receiver's Linux adapters: the OMT channel, the frame handoff, DRM/KMS
//! scanout, ALSA audio, HDMI connector selection, discovery over a central
//! server or Avahi on D-Bus, and the playback supervisor the omt-receiver
//! binary runs. Playback behaviour is derived from the MIT-licensed Open
//! Media Transport projects; see third_party/omt/PROVENANCE.md.

#![forbid(unsafe_code)]

pub mod audio;
pub mod channel;
pub mod connector;
pub mod dbus;
pub mod discovery;
pub mod handoff;
mod mdns;
pub mod play;
pub mod scale;
pub mod trace;
pub mod video;
