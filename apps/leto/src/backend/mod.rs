//! What does the work the interface asks for.
//!
//! The desktop app runs the client engine itself, on worker threads. A
//! browser page cannot open sockets, so the web build has a backend that
//! only explains what is missing, until the bridge exists.

#[cfg(not(target_arch = "wasm32"))]
mod gamepad;
#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
mod stream;
#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(not(target_arch = "wasm32"))]
pub use native::Backend;
#[cfg(target_arch = "wasm32")]
pub use web::Backend;

/// Why the backend can do nothing, in words for the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unavailable {
    pub title: &'static str,
    pub explanation: String,
}
