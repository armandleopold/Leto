//! What Leto asks of the system itself, beyond what a window gets: every key
//! for the PC being watched, Windows' own shortcuts too, what the screen a
//! stream fills is like, and a pane inside the window for pictures drawn
//! straight on the screen.
//!
//! Only Windows is asked for now. Elsewhere these find nothing, and a stream
//! gets the keys its window gets.
//!
//! Talking to Windows takes `unsafe`: only the modules for Windows have any.

#![allow(unsafe_code)]

pub mod display;
pub mod keyboard;
pub mod pane;
