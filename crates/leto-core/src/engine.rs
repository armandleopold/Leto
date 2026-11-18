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
    /// Whether the PC takes this device's camera, and what it takes. Told
    /// a moment after [`Event::Playing`], to a shell that has a camera to
    /// send: finding out must not keep the pictures waiting.
    Camera {
        attempt: u64,
        offer: Option<CameraOffer>,
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
            Self::Camera { attempt, offer } => Event::Camera { attempt, offer },
            Self::PlayFailed { attempt, message } => Event::PlayFailed { attempt, message },
            Self::StreamEnded { attempt, why } => Event::StreamEnded { attempt, why },
            Self::QuitEnded { host, result } => Event::QuitEnded { host, result },
            Self::Notice(message) => Event::Notice(message),
        }
    }
}

/// The camera a PC takes from the device that watches it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CameraOffer {
    /// The largest picture, in pixels.
    pub max_width: u32,
    pub max_height: u32,
    /// The most pictures a second.
    pub max_fps: u32,
}

/// What 1280 by 720 at thirty pictures a second gets, in kilobits a second.
/// Other sizes and paces get as much per pixel.
const CAMERA_BITRATE: u64 = 2500;

/// No camera is sent with less, or with more, in kilobits a second.
const CAMERA_BITRATE_RANGE: (u64, u64) = (300, 8000);

/// The smallest picture worth sending, each way.
pub const SMALLEST_PICTURE: usize = 32;

/// How often a picture may be halved to fit. A camera sixteen times too
/// large each way does not exist.
pub const HALVINGS: u32 = 4;

/// The size of a picture halved `halvings` times, even each way: what an
/// H.264 encoder takes.
pub fn halved(width: usize, height: usize, halvings: u32) -> (usize, usize) {
    ((width >> halvings) & !1, (height >> halvings) & !1)
}

impl CameraOffer {
    /// How pictures of `width` by `height`, taken `fps` times a second, are
    /// sent to the PC that made this offer. They are halved until they fit,
    /// and an odd size loses its last row or column. `None` for a picture
    /// that is too small to send, or that halving does not bring down to
    /// what the PC takes.
    ///
    /// Every client sends by this rule, whatever encodes its pictures.
    pub fn fit(&self, width: usize, height: usize, fps: u32) -> Option<CameraSettings> {
        let halvings = (0..=HALVINGS).find(|halvings| {
            (width >> halvings) <= self.max_width as usize
                && (height >> halvings) <= self.max_height as usize
        })?;
        let (width, height) = halved(width, height, halvings);
        let fps = fps.min(self.max_fps);
        if width < SMALLEST_PICTURE || height < SMALLEST_PICTURE || fps == 0 {
            return None;
        }
        let pixels = (width * height) as u64 * u64::from(fps);
        let bitrate = CAMERA_BITRATE * pixels / (1280 * 720 * 30);
        Some(CameraSettings {
            width: width as u32,
            height: height as u32,
            fps,
            bitrate_kbps: bitrate.clamp(CAMERA_BITRATE_RANGE.0, CAMERA_BITRATE_RANGE.1) as u32,
        })
    }
}

/// What a device says it will send of its camera.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CameraSettings {
    pub width: u32,
    pub height: u32,
    /// Pictures a second.
    pub fps: u32,
    pub bitrate_kbps: u32,
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
    fn a_camera_is_sent_as_large_and_as_fast_as_the_pc_takes() {
        let fit = |width, height, fps, largest: (u32, u32), fastest| {
            CameraOffer { max_width: largest.0, max_height: largest.1, max_fps: fastest }
                .fit(width, height, fps)
                .map(|sending| (sending.width, sending.height, sending.fps, sending.bitrate_kbps))
        };
        let full_hd = (1920, 1080);
        // What Leto captures at when the camera has it.
        assert_eq!(fit(1280, 720, 30, full_hd, 30), Some((1280, 720, 30, 2500)));
        assert_eq!(fit(640, 480, 30, full_hd, 30), Some((640, 480, 30, 833)));
        assert_eq!(fit(1920, 1080, 30, full_hd, 30), Some((1920, 1080, 30, 5625)));
        // A PC that takes less gets less.
        assert_eq!(fit(1920, 1080, 30, (1280, 720), 30), Some((960, 540, 30, 1406)));
        assert_eq!(fit(3840, 2160, 60, (1280, 720), 30), Some((960, 540, 30, 1406)));
        assert_eq!(fit(1280, 720, 30, full_hd, 15), Some((1280, 720, 15, 1250)));
        // An odd size loses its last row and column.
        assert_eq!(fit(161, 91, 30, full_hd, 30), Some((160, 90, 30, 300)));
        // The bit rate stays within what a camera is worth.
        assert_eq!(fit(160, 120, 10, full_hd, 30), Some((160, 120, 10, 300)));
        assert_eq!(fit(3840, 2160, 60, (3840, 2160), 60), Some((3840, 2160, 60, 8000)));
        // Nothing to send: no picture, no pace, or one that no halving
        // brings down to what the PC takes.
        assert_eq!(fit(0, 0, 30, full_hd, 30), None);
        assert_eq!(fit(40, 30, 30, full_hd, 30), None);
        assert_eq!(fit(1280, 720, 0, full_hd, 30), None);
        assert_eq!(fit(1280, 720, 30, full_hd, 0), None);
        assert_eq!(fit(1280, 720, 30, (16, 16), 30), None);
        assert_eq!(fit(100_000, 100_000, 30, (1280, 720), 30), None);
    }

    #[test]
    fn news_survives_being_written_down_failures_included() {
        let offer = CameraOffer { max_width: 1920, max_height: 1080, max_fps: 30 };
        let events: [Event; 8] = [
            Event::Added(Err("The PC did not answer.".into())),
            Event::Playing { attempt: 2, input: InputAllowed::ALL, microphone: true },
            Event::Camera { attempt: 2, offer: Some(offer) },
            Event::Camera { attempt: 2, offer: None },
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
