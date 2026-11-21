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
    model::{AppSummary, HostId, HostSummary, Pin, TunnelSummary},
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
        /// The screen the stream is shown on, when the shell knows it: the
        /// PC is asked for a screen of its own just like it. Without it, a
        /// stream is Full HD at 60 pictures per second.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        screen: Option<ScreenMode>,
        /// What the person chose for their streams. A shell that offers no
        /// choice sends the defaults.
        #[serde(default)]
        choices: Choices,
    },
    /// Stop watching. The app keeps running on the PC.
    Leave,
    /// Close the app running on a PC.
    Quit(HostId),
    /// Wake a PC that sleeps or is off, and wait for it to answer.
    Wake(HostId),
    /// Say how Leto's own WireGuard tunnel is doing.
    Tunnel,
    /// Use the tunnel that this WireGuard file describes instead of any
    /// other, and keep it.
    UseTunnel(WireGuardFile),
    /// Stop using the tunnel and forget its file.
    RemoveTunnel,
}

/// The text of a WireGuard file, as WireGuard apps and servers write it. It
/// holds a private key, so it never shows in a log.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WireGuardFile(pub String);

impl fmt::Debug for WireGuardFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WireGuardFile(…)")
    }
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
    /// The PC asked to wake up answers, or the engine gave up waiting.
    WakeEnded {
        host: HostId,
        result: Result<HostSummary, String>,
    },
    /// How Leto's own WireGuard tunnel is doing, or that it has none.
    /// `refused` says why a file the person gave was not used.
    Tunnel {
        summary: Option<TunnelSummary>,
        refused: Option<String>,
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
            Self::WakeEnded { host, result } => Event::WakeEnded { host, result },
            Self::Tunnel { summary, refused } => Event::Tunnel { summary, refused },
            Self::Notice(message) => Event::Notice(message),
        }
    }
}

/// A screen of this device, as a stream should fill it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenMode {
    /// Pixels across and down.
    pub width: u32,
    pub height: u32,
    /// Pictures per second the screen shows.
    pub refresh_hz: u32,
    /// How large the system draws text and windows on it, in percent: 100
    /// as designed, 150 half as large again.
    pub scale_percent: u32,
}

/// What the person chose for their streams, besides the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Choices {
    /// Video bit rate, in kilobits per second. `None`: as much as the
    /// picture's size and pace call for.
    pub bitrate_kbps: Option<u32>,
    /// A PC that can make a screen of its own for the stream, as Apollo
    /// does, is asked to.
    pub virtual_display: bool,
    /// The PC may change a game's settings to suit the stream.
    pub optimize_games: bool,
    /// The PC's own speakers keep playing. Otherwise only this device does.
    pub sound_on_pc: bool,
    /// The codecs this device decodes well enough for a stream. The PC
    /// picks the best of them it can encode: AV1, then H.265, then H.264.
    pub codecs: Codecs,
}

impl Default for Choices {
    fn default() -> Self {
        Self {
            bitrate_kbps: None,
            virtual_display: true,
            optimize_games: true,
            sound_on_pc: false,
            codecs: Codecs::default(),
        }
    }
}

/// A way pictures are compressed. H.264 by default, which every PC sends
/// and every device decodes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Codec {
    #[default]
    H264,
    H265,
    Av1,
}

impl Codec {
    /// Every codec, the one that compresses best first: at the same bit
    /// rate, AV1 looks better than H.265, which looks better than H.264.
    pub const BEST_FIRST: [Self; 3] = [Self::Av1, Self::H265, Self::H264];

    pub const fn name(self) -> &'static str {
        match self {
            Self::H264 => "H.264",
            Self::H265 => "H.265",
            Self::Av1 => "AV1",
        }
    }

    const fn bit(self) -> u8 {
        match self {
            Self::H264 => 1,
            Self::H265 => 1 << 1,
            Self::Av1 => 1 << 2,
        }
    }
}

/// Some codecs. By default H.264 alone, which every PC sends and every
/// device decodes.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "Vec<Codec>", into = "Vec<Codec>")]
pub struct Codecs(u8);

impl Codecs {
    pub const NONE: Self = Self(0);

    pub const fn of(codec: Codec) -> Self {
        Self(codec.bit())
    }

    #[must_use]
    pub const fn with(self, codec: Codec) -> Self {
        Self(self.0 | codec.bit())
    }

    /// These codecs and those.
    #[must_use]
    pub const fn and(self, others: Self) -> Self {
        Self(self.0 | others.0)
    }

    pub const fn contains(self, codec: Codec) -> bool {
        self.0 & codec.bit() != 0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The codecs, the one that compresses best first.
    pub fn best_first(self) -> impl Iterator<Item = Codec> {
        Codec::BEST_FIRST.into_iter().filter(move |codec| self.contains(*codec))
    }

    /// The codec that compresses best, of these.
    pub fn best(self) -> Option<Codec> {
        self.best_first().next()
    }
}

impl Default for Codecs {
    fn default() -> Self {
        Self::of(Codec::H264)
    }
}

/// The codecs a device decodes: on its graphics card, and at all, on the
/// graphics card or the processor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decodable {
    pub hardware: Codecs,
    pub any: Codecs,
}

impl Decodable {
    /// H.264 on the processor, which every device decodes.
    pub const SOFTWARE: Self = Self { hardware: Codecs::NONE, any: Codecs::of(Codec::H264) };
}

impl std::fmt::Debug for Codecs {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_list().entries(self.best_first()).finish()
    }
}

impl From<Vec<Codec>> for Codecs {
    fn from(codecs: Vec<Codec>) -> Self {
        codecs.into_iter().fold(Self::NONE, Self::with)
    }
}

impl From<Codecs> for Vec<Codec> {
    fn from(codecs: Codecs) -> Self {
        codecs.best_first().collect()
    }
}

/// The video bit rates a person can choose, in kilobits per second.
/// Moonlight offers the same.
pub const BITRATES_KBPS: (u32, u32) = (500, 150_000);

/// The video bit rate a stream of `width` by `height` pixels at `fps`
/// pictures per second calls for, in kilobits per second.
///
/// It grows with the pixels, from the 20 Mbit/s of Full HD at 60 pictures
/// per second. Beyond 60 it grows more slowly than the pace: each picture
/// differs less from the one before. Never under 2 Mbit/s.
pub fn bitrate_for(width: u32, height: u32, fps: u32) -> u32 {
    let pixels = f64::from(width) * f64::from(height) / (1920.0 * 1080.0);
    let pace = f64::from(fps) / 60.0;
    let pace = if pace > 1.0 { pace.sqrt() } else { pace };
    let bitrate = (20_000.0 * pixels * pace).round() as u32;
    bitrate.clamp(2_000, BITRATES_KBPS.1)
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
            Request::Play {
                host: HostId("salon".into()),
                app: 881_448_767,
                attempt: 4,
                screen: None,
                choices: Choices::default(),
            },
            Request::Play {
                host: HostId("salon".into()),
                app: 1,
                attempt: 5,
                screen: Some(ScreenMode {
                    width: 2560,
                    height: 1440,
                    refresh_hz: 144,
                    scale_percent: 125,
                }),
                choices: Choices {
                    bitrate_kbps: Some(50_000),
                    virtual_display: false,
                    optimize_games: false,
                    sound_on_pc: true,
                    codecs: Codecs::of(Codec::H265).with(Codec::H264),
                },
            },
            Request::Leave,
            Request::Tunnel,
            Request::UseTunnel(WireGuardFile("[Interface]\nPrivateKey = x\n".into())),
            Request::RemoveTunnel,
        ];
        for request in requests {
            assert_eq!(there_and_back(&request), request);
        }
    }

    #[test]
    fn a_wireguard_file_never_shows_in_a_log() {
        let request = Request::UseTunnel(WireGuardFile("PrivateKey = secret".into()));
        let shown = format!("{request:?}");
        assert!(!shown.contains("secret"), "{shown}");
        // Written down for the bridge, it is all there.
        let text = serde_json::to_string(&request).expect("written");
        assert_eq!(text, r#"{"UseTunnel":"PrivateKey = secret"}"#);
    }

    #[test]
    fn a_play_request_from_a_shell_that_knows_no_screen_still_reads() {
        // What a page of an older bridge says: no screen, no choices.
        let text = r#"{"Play":{"host":"salon","app":1,"attempt":2}}"#;
        let request: Request = serde_json::from_str(text).expect("an older request reads");
        assert_eq!(
            request,
            Request::Play {
                host: HostId("salon".into()),
                app: 1,
                attempt: 2,
                screen: None,
                choices: Choices::default()
            }
        );
    }

    #[test]
    fn codecs_read_and_write_as_a_list_and_the_best_comes_first() {
        let codecs = Codecs::of(Codec::H264).with(Codec::Av1);
        assert_eq!(serde_json::to_string(&codecs).expect("codecs write"), r#"["Av1","H264"]"#);
        let read: Codecs = serde_json::from_str(r#"["H264","H265"]"#).expect("codecs read");
        assert_eq!(read.best(), Some(Codec::H265));
        assert!(!read.contains(Codec::Av1));
        assert_eq!(Codecs::NONE.best(), None);
        // An older page says nothing of codecs: H.264, as before.
        let choices: Choices = serde_json::from_str("{}").expect("no choice reads");
        assert_eq!(choices.codecs, Codecs::of(Codec::H264));
    }

    #[test]
    fn a_choice_left_out_is_its_default() {
        // A page that knows only of the bit rate.
        let choices: Choices =
            serde_json::from_str(r#"{"bitrate_kbps":8000}"#).expect("part of the choices reads");
        assert_eq!(choices, Choices { bitrate_kbps: Some(8000), ..Choices::default() });
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
