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
//! H.264 decoder that works everywhere. Decoded pictures stay as the
//! decoders give them, in planes, and a [`Card`] turns them into pixels on
//! the graphics card as the window draws them. On Windows, a picture
//! decoded on the graphics card can stay there until it is drawn: a
//! [`Kept`], which is being tried.

pub mod cable;
mod camera;
// Drawing goes through OpenGL's C interface.
#[allow(unsafe_code)]
mod card;
mod convert;
mod encode;
mod endpoints;
mod feed;
// Direct3D and the driver's sharing with OpenGL are C interfaces.
#[allow(unsafe_code)]
mod kept;
// Direct3D is a C interface.
mod microphone;
mod queue;
mod sound;
mod speaker;
#[cfg(windows)]
#[allow(unsafe_code)]
mod straight;
mod video;
mod voice;

pub use camera::{Camera, camera_names, look_for_cameras};
pub use card::Card;
pub use convert::Planes;
pub use encode::{Encoded, PictureEncoder, Sending};
pub use endpoints::{microphones, speakers};
pub use feed::{CameraFeed, Carrier, Fed, Feeding, Limits};
pub use kept::Kept;
pub use microphone::Microphone;
pub use sound::SoundDecoder;
pub use speaker::Speaker;
pub use video::{Pane, Picture, VideoDecoder, decodable};
pub use voice::{PCM_FRAME, VOICE_FRAME, VOICE_RATE, VoiceCoding, VoiceEncoder, pcm24_samples};

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
