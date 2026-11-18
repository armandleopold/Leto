//! What an interface asks of the client engine, and what it hears back.
//!
//! The desktop app says these things to the engine inside the same program.
//! The web client says the same things to the bridge, as text, which is why
//! everything here can be written down and read back.

use std::fmt;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

use crate::{
    input::InputAllowed,
    model::{AppSummary, HostId, HostSummary, Pin},
};

/// Work for the engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    /// Check the saved PCs and look for others on the network.
    Search,
    /// Add the PC at this address.
    Add(String),
    Pair {
        host: HostId,
        attempt: u64,
    },
    /// Load the apps of a PC, then their covers.
    Apps(HostId),
    Forget(HostId),
    /// Start an app on a PC, or go back to it if it is running, and stream it.
    Play {
        host: HostId,
        app: u32,
        attempt: u64,
    },
    /// Stop watching. The app keeps running on the PC.
    Leave,
    /// Close the app running on a PC.
    Quit(HostId),
}

/// News from the engine.
///
/// `Image` is what a cover is made of. The engine hands covers over as the
/// files the PC keeps ([`ImageFile`]). A shell turns them into what it draws
/// with [`Event::with_covers`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event<Image = ImageFile> {
    /// The list of PCs changed.
    Hosts(Vec<HostSummary>),
    /// A search ended. `network_searched` is false when this device could
    /// not listen for PCs announcing themselves.
    SearchFinished {
        network_searched: bool,
    },
    Added(Result<HostSummary, String>),
    /// The PIN the person has to type on the PC.
    PinChosen {
        attempt: u64,
        pin: Pin,
    },
    PairingEnded {
        attempt: u64,
        result: Result<HostSummary, String>,
    },
    Apps {
        host: HostId,
        result: Result<Vec<AppSummary>, AppsProblem>,
    },
    /// The cover of an app, or `None` when the PC has no usable picture.
    Cover {
        host: HostId,
        app: u32,
        image: Option<Image>,
    },
    /// The stream is connected: pictures and sound are on their way, and
    /// the PC accepts `input` from this device. `microphone` says whether
    /// it takes this device's microphone too.
    Playing {
        attempt: u64,
        input: InputAllowed,
        microphone: bool,
    },
    /// The stream did not start.
    PlayFailed {
        attempt: u64,
        message: String,
    },
    /// A stream that was playing ended without the person asking.
    StreamEnded {
        attempt: u64,
        why: StreamEnd,
    },
    /// The attempt to close the app running on a PC is over.
    QuitEnded {
        host: HostId,
        result: Result<(), String>,
    },
    /// Something the person should know that belongs to no screen.
    Notice(String),
}

impl<Image> Event<Image> {
    /// The same news with covers made of something else. `convert` returns
    /// `None` for a cover it cannot use, which is then a missing cover.
    pub fn with_covers<Other>(self, convert: impl FnOnce(Image) -> Option<Other>) -> Event<Other> {
        match self {
            Self::Cover { host, app, image } => {
                Event::Cover { host, app, image: image.and_then(convert) }
            }
            Self::Hosts(hosts) => Event::Hosts(hosts),
            Self::SearchFinished { network_searched } => Event::SearchFinished { network_searched },
            Self::Added(result) => Event::Added(result),
            Self::PinChosen { attempt, pin } => Event::PinChosen { attempt, pin },
            Self::PairingEnded { attempt, result } => Event::PairingEnded { attempt, result },
            Self::Apps { host, result } => Event::Apps { host, result },
            Self::Playing { attempt, input, microphone } => {
                Event::Playing { attempt, input, microphone }
            }
            Self::PlayFailed { attempt, message } => Event::PlayFailed { attempt, message },
            Self::StreamEnded { attempt, why } => Event::StreamEnded { attempt, why },
            Self::QuitEnded { host, result } => Event::QuitEnded { host, result },
            Self::Notice(message) => Event::Notice(message),
        }
    }
}

/// Why a stream ended on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamEnd {
    /// The app closed on the PC.
    AppClosed,
    /// The stream broke. The app may still be running.
    ConnectionLost,
    /// The PC ended the stream with this error code.
    HostError(u32),
}

/// Why the apps of a PC could not be listed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppsProblem {
    pub message: String,
    /// The PC no longer knows this device: pairing again fixes it.
    pub needs_pairing: bool,
}

/// An image as the PC keeps it: the bytes of a PNG or JPEG file.
///
/// Written down as base64, so that it travels as text with the rest.
#[derive(Clone, PartialEq, Eq)]
pub struct ImageFile(pub Vec<u8>);

impl fmt::Debug for ImageFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ImageFile({} bytes)", self.0.len())
    }
}

impl Serialize for ImageFile {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for ImageFile {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        STANDARD.decode(text.as_bytes()).map(Self).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn there_and_back<T: Serialize + for<'de> Deserialize<'de>>(value: &T) -> T {
        let text = serde_json::to_string(value).expect("these types have no map keys to refuse");
        serde_json::from_str(&text).expect("what was just written reads back")
    }

    #[test]
    fn a_request_survives_being_written_down() {
        let requests = [
            Request::Search,
            Request::Add("192.168.1.20:47989".into()),
            Request::Pair { host: HostId("salon".into()), attempt: 3 },
            Request::Play { host: HostId("salon".into()), app: 881_448_767, attempt: 4 },
            Request::Leave,
        ];
        for request in requests {
            assert_eq!(there_and_back(&request), request);
        }
    }

    #[test]
    fn news_survives_being_written_down_failures_included() {
        let events: [Event; 6] = [
            Event::Added(Err("The PC did not answer.".into())),
            Event::Playing { attempt: 2, input: InputAllowed::ALL, microphone: true },
            Event::PinChosen { attempt: 1, pin: Pin::from_bytes([0, 9, 0, 5]) },
            Event::Apps {
                host: HostId("salon".into()),
                result: Err(AppsProblem { message: "Pair again.".into(), needs_pairing: true }),
            },
            Event::StreamEnded { attempt: 2, why: StreamEnd::HostError(0x8003_0023) },
            Event::QuitEnded { host: HostId("salon".into()), result: Ok(()) },
        ];
        for event in events {
            assert_eq!(there_and_back(&event), event);
        }
    }

    #[test]
    fn a_cover_travels_as_text() {
        let file = ImageFile((0..=255).collect());
        let cover: Event =
            Event::Cover { host: HostId("salon".into()), app: 7, image: Some(file.clone()) };
        let text = serde_json::to_string(&cover).expect("a cover can be written down");
        assert!(text.is_ascii());
        assert_eq!(serde_json::from_str::<Event>(&text).expect("and read back"), cover);

        // What is not base64 is refused, not read as an empty picture.
        let broken = text.replace("AAEC", "*");
        assert!(serde_json::from_str::<Event>(&broken).is_err());
    }

    #[test]
    fn a_shell_makes_covers_of_what_it_draws_with() {
        let host = HostId("salon".into());
        let cover: Event =
            Event::Cover { host: host.clone(), app: 7, image: Some(ImageFile(vec![1, 2, 3])) };
        assert_eq!(
            cover.clone().with_covers(|file| Some(file.0.len())),
            Event::Cover { host: host.clone(), app: 7, image: Some(3) }
        );
        // A file the shell cannot read is a missing cover.
        assert_eq!(
            cover.with_covers(|_| None::<usize>),
            Event::Cover { host, app: 7, image: None }
        );
        // Everything else passes through as it is.
        let other: Event = Event::Notice("Hello.".into());
        assert_eq!(other.with_covers(|_| None::<()>), Event::Notice("Hello.".into()));
    }
}
