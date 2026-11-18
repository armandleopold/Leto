//! Shared building blocks for every Leto app.
//!
//! This crate stays free of platform code and heavy dependencies so that it
//! compiles everywhere Leto runs: Windows, Linux, macOS, Android, iOS and the
//! web (WebAssembly).

pub mod brand;
pub mod engine;
pub mod input;
pub mod model;
pub mod platform;

/// Version of the Leto workspace, shared by every app.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
