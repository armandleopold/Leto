//! How a browser and the bridge talk, over one WebSocket.
//!
//! A browser page cannot reach a PC itself, so the web client asks the
//! bridge, which runs the client engine next to the PC. What they say to
//! each other is the engine's own vocabulary ([`crate::engine`]) wrapped in
//! [`Up`] and [`Down`], as JSON text messages. Pictures and sound are too
//! large for text and travel as binary messages, one [`Frame`] each. So
//! does the browser's microphone, the other way.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{
    engine::{Event, Request},
    input::Input,
};

/// Where the bridge answers WebSockets, next to the pages it serves.
pub const PATH: &str = "/link";

/// Digits in the code that makes a browser known to the bridge.
pub const CODE_DIGITS: usize = 8;

/// The rate a browser sends its microphone at, in samples per second, on
/// one channel: what a PC takes, so that the bridge passes it on as it is.
pub const VOICE_RATE: u32 = 48_000;

/// How much of the microphone one [`Frame::Voice`] carries.
pub const VOICE_PACKET: Duration = Duration::from_millis(20);

/// What a browser says to the bridge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Up {
    /// The first thing a browser says: the key the bridge gave it earlier,
    /// if it has one.
    Hello {
        key: Option<String>,
    },
    /// The code the person read where the bridge runs.
    Code(String),
    Ask(Request),
    /// What the person did, for the PC being watched, in order.
    Input(Vec<Input>),
    /// A picture could not be decoded. The following ones are useless until
    /// one comes that decodes on its own.
    KeyframeNeeded,
}

/// What the bridge says to a browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Down {
    /// The bridge does not know this browser: the person has to type the
    /// code. `refused` says the code just tried was not the right one.
    CodeNeeded {
        refused: bool,
    },
    /// The browser may ask. `key` comes with a code that was just accepted:
    /// the browser keeps it and says it in [`Up::Hello`] from then on.
    /// `name` is what the computer running the bridge calls itself.
    Welcome {
        key: Option<String>,
        name: String,
    },
    News(Event),
    /// The pictures about to arrive.
    Pictures(VideoFormat),
    /// The sound about to arrive, or why the stream will have none, in
    /// words for the person.
    Sound(Result<AudioFormat, String>),
}

/// What a browser needs to know to decode the pictures of a stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoFormat {
    /// The codec the way browsers name it, such as `avc1.640028`.
    pub codec: String,
    pub width: u32,
    pub height: u32,
    /// Pictures per second.
    pub fps: u32,
}

/// What a browser needs to know to decode the sound of a stream: Opus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u32,
    /// Samples per channel in one packet.
    pub samples_per_packet: u32,
}

const PICTURE: u8 = 1;
const SOUND: u8 = 2;
const VOICE: u8 = 3;
const KEYFRAME: u8 = 1;
/// Bytes before the data of a picture: kind, flags and number.
const PICTURE_HEAD: usize = 6;

/// One binary message: a piece of the stream, still encoded. Pictures and
/// sound come from the bridge, the microphone goes to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame<'a> {
    Picture {
        /// Counts up by one per picture the PC made.
        number: u32,
        /// A picture that decodes on its own.
        keyframe: bool,
        /// An H.264 access unit with start codes.
        data: &'a [u8],
    },
    /// One Opus packet.
    Sound(&'a [u8]),
    /// The browser's microphone for the PC being watched: one Opus packet
    /// of [`VOICE_PACKET`], one channel at [`VOICE_RATE`]. They follow each
    /// other without a gap.
    Voice(&'a [u8]),
}

impl<'a> Frame<'a> {
    /// The message that carries this frame.
    pub fn encode(&self) -> Vec<u8> {
        match *self {
            Self::Picture { number, keyframe, data } => {
                let mut message = Vec::with_capacity(PICTURE_HEAD + data.len());
                message.push(PICTURE);
                message.push(if keyframe { KEYFRAME } else { 0 });
                message.extend_from_slice(&number.to_be_bytes());
                message.extend_from_slice(data);
                message
            }
            Self::Sound(packet) => tagged(SOUND, packet),
            Self::Voice(packet) => tagged(VOICE, packet),
        }
    }

    /// Reads a message. `None` for one this version does not understand.
    pub fn parse(message: &'a [u8]) -> Option<Self> {
        match *message.first()? {
            PICTURE => {
                let flags = *message.get(1)?;
                let number = u32::from_be_bytes(message.get(2..PICTURE_HEAD)?.try_into().ok()?);
                let data = message.get(PICTURE_HEAD..)?;
                Some(Self::Picture { number, keyframe: flags & KEYFRAME != 0, data })
            }
            SOUND => Some(Self::Sound(message.get(1..)?)),
            VOICE => Some(Self::Voice(message.get(1..)?)),
            _ => None,
        }
    }
}

/// A message that is its kind, then `packet`.
fn tagged(kind: u8, packet: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(1 + packet.len());
    message.push(kind);
    message.extend_from_slice(packet);
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{input::GamepadState, model::HostId};

    #[test]
    fn frames_come_back_as_they_were_sent() {
        let picture =
            Frame::Picture { number: 0x0102_0304, keyframe: true, data: &[0, 0, 1, 0x65] };
        let message = picture.encode();
        assert_eq!(message, [1, 1, 1, 2, 3, 4, 0, 0, 1, 0x65]);
        assert_eq!(Frame::parse(&message), Some(picture));

        let later = Frame::Picture { number: 7, keyframe: false, data: &[] };
        assert_eq!(Frame::parse(&later.encode()), Some(later));

        let sound = Frame::Sound(&[0xFC, 1, 2]);
        assert_eq!(sound.encode(), [2, 0xFC, 1, 2]);
        assert_eq!(Frame::parse(&sound.encode()), Some(sound));

        let voice = Frame::Voice(&[0xF8, 3, 4]);
        assert_eq!(voice.encode(), [3, 0xF8, 3, 4]);
        assert_eq!(Frame::parse(&voice.encode()), Some(voice));
    }

    #[test]
    fn a_message_cut_short_or_unknown_is_not_a_frame() {
        assert_eq!(Frame::parse(&[]), None);
        assert_eq!(Frame::parse(&[1, 1, 0, 0]), None);
        assert_eq!(Frame::parse(&[9, 1, 2, 3]), None);
    }

    #[test]
    fn both_sides_read_what_the_other_writes() {
        let up = [
            Up::Hello { key: None },
            Up::Hello { key: Some("3f9a".into()) },
            Up::Code("1234 5678".into()),
            Up::Ask(Request::Apps(HostId("salon".into()))),
            Up::Input(vec![
                Input::MouseMove { dx: -3, dy: 4 },
                Input::Gamepad {
                    number: 1,
                    state: GamepadState {
                        left_trigger: 0.5,
                        left_stick: [-1.0, 0.25],
                        ..GamepadState::default()
                    },
                },
            ]),
            Up::KeyframeNeeded,
        ];
        for message in up {
            let text = serde_json::to_string(&message).expect("messages are plain data");
            assert_eq!(serde_json::from_str::<Up>(&text).expect("and read back"), message);
        }

        let down = [
            Down::CodeNeeded { refused: true },
            Down::Welcome { key: Some("3f9a".into()), name: "salon".into() },
            Down::News(Event::Notice("Hello.".into())),
            Down::Pictures(VideoFormat {
                codec: "avc1.640028".into(),
                width: 1920,
                height: 1080,
                fps: 60,
            }),
            Down::Sound(Ok(AudioFormat {
                sample_rate: 48_000,
                channels: 2,
                samples_per_packet: 240,
            })),
            Down::Sound(Err("The PC sends six channels.".into())),
        ];
        for message in down {
            let text = serde_json::to_string(&message).expect("messages are plain data");
            assert_eq!(serde_json::from_str::<Down>(&text).expect("and read back"), message);
        }
    }

    #[test]
    fn what_is_not_a_message_is_refused() {
        assert!(serde_json::from_str::<Up>("{\"Ask\":\"Everything\"}").is_err());
        assert!(serde_json::from_str::<Up>("\"Hello\"").is_err());
        assert!(serde_json::from_str::<Up>("not json").is_err());
    }
}
