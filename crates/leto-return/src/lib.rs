//! The part of the return channel that is Leto's own.
//!
//! A stream flows from a PC to the device that watches it. The return
//! channel flows the other way: the microphone and the camera of that device,
//! which the PC then offers to its apps as its own. The microphone follows a
//! protocol that some hosts already speak, and lives with the rest of the
//! stream there. The camera is new: this crate holds its packets
//! ([`camera`]), those of a microphone sent the same way to a host that has
//! no protocol for one ([`voice`]), and what a client and a host say to each
//! other before either flows ([`api`]). The design
//! and its reasons are in `docs/return-channel.md`.
//!
//! Both ends use this crate, so that they cannot disagree: the clients to
//! send, the hosts to receive. It opens no connection and compiles on every
//! platform Leto targets.

pub mod api;
pub mod camera;
pub mod voice;
