//! Video and audio for Leto.
//!
//! A host sends pictures as H.264 and sound as Opus. This crate turns them
//! into pixels and samples, and plays the samples on the speakers. It knows
//! nothing about the network: the client engine hands it encoded data, and
//! the app shows the pictures.
//!
//! Everything here runs on the processor. Decoding on the graphics card is
//! faster and comes later; this path stays as the one that works everywhere.

mod queue;
mod sound;
mod speaker;
mod video;

pub use sound::SoundDecoder;
pub use speaker::Speaker;
pub use video::{Picture, VideoDecoder};

/// What can go wrong with pictures and sound.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the picture could not be decoded: {0}")]
    Video(String),
    #[error("the sound could not be decoded: {0}")]
    Sound(String),
    #[error("the speakers could not be used: {0}")]
    Speaker(String),
}
