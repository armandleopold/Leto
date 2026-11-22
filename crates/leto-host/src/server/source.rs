//! Where the pictures and the sound of a stream come from.
//!
//! The host asks a [`Screen`] for the pictures of each stream and a
//! [`Sound`] for what goes with them, already encoded. On a PC that is
//! shared they are its screen and what it plays. The test card and its
//! melody ([`Card`], [`Tune`]) are what a host without either streams, and
//! what the tests of the clients watch.

use std::{fmt, time::Duration};

use leto_core::engine::Codec;

use super::media::{Hands, Melody, TestCard};
use crate::camera::Seen;

/// How long the sound of one packet lasts. Hosts send five milliseconds at
/// a time unless a client asks otherwise, and Leto's clients do not.
pub const SOUND_PACKET: Duration = Duration::from_millis(5);
/// Samples a second, of each of the two channels.
pub const SOUND_RATE: u32 = 48_000;
/// Bits a second of the encoded sound. Every packet has the same size: the
/// recovery packets that go with them are computed over equal lengths.
pub const SOUND_BITRATE: u32 = 96_000;
/// The size of every packet of sound, in bytes.
pub const SOUND_PACKET_BYTES: usize =
    SOUND_BITRATE as usize / 8 * SOUND_PACKET.as_millis() as usize / 1000;
/// Samples of each channel in one packet.
pub const SOUND_PACKET_SAMPLES: usize =
    SOUND_RATE as usize * SOUND_PACKET.as_millis() as usize / 1000;

/// What a client asked of the pictures of its stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asked {
    pub width: u32,
    pub height: u32,
    /// Pictures a second.
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub codec: Codec,
}

/// One encoded picture.
#[derive(Debug, Clone, Copy)]
pub struct Picture<'a> {
    /// It can be decoded on its own.
    pub keyframe: bool,
    /// The picture as the codec's stream carries it, with start codes for
    /// H.264 and H.265.
    pub data: &'a [u8],
    /// How long the PC took over it, from the moment the screen changed to
    /// the encoded picture: clients show it. `None` where nobody measured.
    pub took: Option<Duration>,
}

/// What the client of a stream is doing, for a source that shows it.
#[derive(Clone, Copy, Default)]
pub struct Watching<'a> {
    pub(crate) hands: Hands,
    pub(crate) camera: Option<&'a Seen>,
}

impl fmt::Debug for Watching<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Watching").finish_non_exhaustive()
    }
}

/// The pictures of one stream.
pub trait Pictures: Send {
    /// Waits for the next picture and encodes it. `keyframe` asks for one
    /// that can be decoded on its own. `None` when there is none this time:
    /// nothing changed on the screen, or the encoder kept it. Never waits
    /// much longer than a tenth of a second, so that a stream that ends is
    /// noticed.
    fn next(&mut self, keyframe: bool, watching: Watching<'_>) -> Option<Picture<'_>>;
}

/// Something to stream the pictures of.
pub trait Screen: fmt::Debug + Send + Sync {
    /// Starts the pictures of one stream. `None` when there is nothing to
    /// show, or no way to encode it as `asked`.
    fn open(&self, asked: Asked) -> Option<Box<dyn Pictures>>;
}

/// The sound of one stream.
pub trait Packets: Send {
    /// Waits until the next [`SOUND_PACKET`] of sound is there, and gives it
    /// encoded: Opus, two channels at [`SOUND_RATE`], in exactly
    /// [`SOUND_PACKET_BYTES`] bytes. `None` when there is none this time.
    fn next(&mut self) -> Option<&[u8]>;
}

/// Something to stream the sound of.
pub trait Sound: fmt::Debug + Send + Sync {
    /// Starts the sound of one stream. `None` when there is none to send.
    fn open(&self) -> Option<Box<dyn Packets>>;
}

/// The test card: a sun that passes behind a moon, colour bars, and what
/// the client does with its keyboard, mouse, gamepad, microphone and
/// camera. It is H.264 whatever was asked.
#[derive(Debug, Clone, Copy, Default)]
pub struct Card;

impl Screen for Card {
    fn open(&self, asked: Asked) -> Option<Box<dyn Pictures>> {
        let card = TestCard::new(asked.width, asked.height, asked.fps, asked.bitrate_kbps)?;
        Some(Box::new(card))
    }
}

/// The melody that goes with the test card: it repeats every two seconds.
#[derive(Debug, Clone, Copy, Default)]
pub struct Tune;

impl Sound for Tune {
    fn open(&self) -> Option<Box<dyn Packets>> {
        let melody = Melody::new(SOUND_RATE, SOUND_BITRATE as i32, SOUND_PACKET_SAMPLES)?;
        Some(Box::new(melody))
    }
}
