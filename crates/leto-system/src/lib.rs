//! What Leto asks of the system itself, beyond what a window gets: every key
//! for the PC being watched, Windows' own shortcuts too, what the screen a
//! stream fills is like, a clock fine enough for a stream, and a pane inside
//! the window for pictures drawn straight on the screen. On Android, the
//! clipboard.
//!
//! Only Windows is asked for the rest for now. Elsewhere these find nothing, and a stream
//! gets the keys its window gets.
//!
//! Talking to Windows, or to Android's Java side, takes `unsafe`: only the
//! modules for them have any.

#![allow(unsafe_code)]

pub mod bars;
#[cfg(target_os = "android")]
pub mod camera;
#[cfg(target_os = "android")]
pub mod gamepad;
pub mod clipboard;
pub mod clock;
pub mod display;
pub mod keyboard;
pub mod orientation;
pub mod pane;
pub mod permission;

#[cfg(target_os = "android")]
mod java;
