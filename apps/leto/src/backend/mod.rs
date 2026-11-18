//! What does the work the interface asks for.
//!
//! The desktop app runs the client engine itself, on worker threads. A
//! browser page cannot open sockets, so the web build asks the bridge that
//! served the page, which runs the same engine next to the PC.

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

/// A picture of the stream, as the backend hands it to the interface.
// Each build has one way to show pictures, and never makes the other.
#[allow(dead_code)]
pub enum Picture {
    /// Pixels for the window to draw.
    Pixels(egui::ColorImage),
    /// The backend shows the pictures itself, behind the window, and only
    /// says how large they are. The interface leaves a hole for them and
    /// says where it is with `Backend::place_picture`.
    Behind([usize; 2]),
}
