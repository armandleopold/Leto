//! The Leto app on the desktop and, compiled to WebAssembly, in the browser.
//! Everything is in the library (`src/lib.rs`).

// Hide the console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
fn main() -> eframe::Result {
    leto::run_desktop()
}

#[cfg(target_arch = "wasm32")]
fn main() {
    leto::start_web();
}

// Android starts the app from `apps/leto-android`, never from here.
#[cfg(target_os = "android")]
fn main() {}
