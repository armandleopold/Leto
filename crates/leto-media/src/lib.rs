//! Video and audio for Leto.
//!
//! A host sends pictures as H.264, H.265 or AV1 and sound as Opus. This
//! crate turns them into pixels and samples, and plays the samples on the
//! speakers. The other way, it captures the microphone and encodes it as
//! Opus for a host that takes one, and captures the camera as pixels, which
//! it encodes as H.264 for a host that takes one. It knows nothing about the
//! network: the client engine hands it encoded data and takes encoded data
//! from it, and the app shows the pictures.
//!
//! Everything here runs on the processor, except pictures from a PC on
//! Windows: Windows decodes them on the graphics card when it can, and that
//! is the only way to H.265 and AV1. OpenH264 on the processor stays as the
//! H.264 decoder that works everywhere.

mod camera;
// Pictures from Windows' decoders come in NV12.
#[cfg(any(windows, test))]
mod convert;
mod encode;
mod feed;
mod microphone;
mod queue;
mod sound;
mod speaker;
mod video;
mod voice;

pub use camera::Camera;
pub use encode::{Encoded, PictureEncoder, Sending};
pub use feed::{CameraFeed, Carrier, Fed, Feeding, Limits};
pub use microphone::Microphone;
pub use sound::SoundDecoder;
pub use speaker::Speaker;
pub use video::{Picture, VideoDecoder, decodable};
pub use voice::{VOICE_FRAME, VOICE_RATE, VoiceEncoder};

/// What can go wrong with pictures and sound.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the picture could not be decoded: {0}")]
    Video(String),
    #[error("the sound could not be decoded: {0}")]
    Sound(String),
    #[error("the speakers could not be used: {0}")]
    Speaker(String),
    #[error("the microphone could not be used: {0}")]
    Microphone(String),
    #[error("the camera could not be used: {0}")]
    Camera(String),
}
