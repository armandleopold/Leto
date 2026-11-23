//! A stand-in for a Sunshine or Apollo host.
//!
//! The tests of the clients run against it, and `cargo run -p leto-testhost
//! --example serve` keeps one running to try a client by hand without a
//! gaming PC. It captures no screen and runs no app: what it streams is a
//! test card and a melody.
//!
//! It is Leto's own host ([`leto_host::server`]) with the test card for a
//! screen, so a client is tried against the code a shared PC runs. What is
//! here is what only tests want: a host that starts or stops the test, and
//! a few sounds to send it, and a stand-in for the pages of an Apollo
//! ([`ApolloPages`]), which a Leto that shares a PC beside one asks.

// Test support: a failed assumption should stop the test loudly.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::ops::Deref;

mod pages;

pub use leto_host::server::{
    App, CLIENT_MISSED_WITHIN, COVER_PNG, CameraReport, HOST_CARD, HOST_ID, HOST_NAME, Host,
    Options, ScreenAsked, Seen, StreamReport, permission,
};

pub use pages::ApolloPages;

/// Twenty milliseconds of silence, as a client's microphone sends them: one
/// Opus frame, one channel at 48 kHz.
pub fn quiet_voice() -> Vec<u8> {
    use opus_rs::{Application, OpusEncoder};

    let mut encoder = OpusEncoder::new(48_000, 1, Application::RestrictedLowDelay)
        .expect("an encoder for one channel at 48 kHz");
    let mut packet = vec![0u8; 1500];
    let written =
        encoder.encode(&[0.0; 960], 960, &mut packet).expect("silence is not hard to encode");
    packet.truncate(written);
    packet
}
/// A running stand-in host. Stops when dropped.
pub struct FakeHost(Host);

impl FakeHost {
    /// A host reachable on the loopback interface only.
    pub fn start() -> Self {
        Self::start_with(Options::default())
    }

    /// A host listening on `interface`, for example `"0.0.0.0"` for all.
    pub fn start_on(interface: &str) -> Self {
        Self::start_with(Options { interface: interface.to_owned(), ..Options::default() })
    }

    pub fn start_with(options: Options) -> Self {
        Self(Host::start(options).expect("the stand-in host starts"))
    }
}

impl Deref for FakeHost {
    type Target = Host;

    fn deref(&self) -> &Host {
        &self.0
    }
}
