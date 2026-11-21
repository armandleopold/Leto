//! The commands about streams: play an app and say what arrived, close it.
//!
//! `play` shows nothing and plays nothing: it measures. It tells whether a
//! PC streams to Leto at all, whether Leto can decode its pictures, and how
//! well they arrive, without the graphical app. Asked to, it also sends the
//! PC this device's microphone and its camera, and tells how that went.

use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use leto_client::{
    AppSummary, CameraLink, CameraSettings, Client, Counter, Decoded, Ended, HostId, Session,
    StreamSettings, VideoFrame, VideoSetup, VideoSink,
    engine::{CameraOffer, Codec},
};
use leto_media::{
    CameraFeed, Carrier, Encoded, Feeding, Limits, Microphone, Picture, Sending, VOICE_FRAME,
    VideoDecoder,
};
use serde::Serialize;

use crate::{pcs::find, to_json};

/// How often the microphone is asked how loud it is.
const LISTEN_EVERY: Duration = Duration::from_millis(50);

/// How long a camera gets to open and the PC to agree to take it.
const CAMERA_PATIENCE: Duration = Duration::from_secs(10);

/// The stream to ask the PC for, and who decodes it.
#[derive(Debug, Clone, Copy)]
pub struct Asked {
    pub settings: StreamSettings,
    /// The graphics card may decode.
    pub hardware: bool,
}

/// What of this device goes to the PC while it streams.
#[derive(Debug, Default, Clone, Copy)]
pub struct Back {
    pub microphone: bool,
    pub camera: Option<WhichCamera>,
}

/// The camera that is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhichCamera {
    /// The one of this device.
    System,
    /// A test card, where there is none.
    Card,
}

/// What a stream delivered while it was watched.
#[derive(Debug, Serialize)]
struct Report {
    pc: String,
    app: String,
    /// What was asked for.
    width: u32,
    height: u32,
    fps: u32,
    /// Seconds from asking the PC to the stream being connected.
    connected_in: f64,
    /// Seconds the stream was watched.
    watched: f64,
    pictures: u64,
    keyframes: u64,
    /// Pictures the PC made that never arrived.
    pictures_missing: u64,
    /// Pictures Leto decoded, and their size.
    pictures_decoded: u64,
    decoded_width: usize,
    decoded_height: usize,
    /// Pictures that arrived and could not be decoded.
    pictures_undecodable: u64,
    /// The codec the PC sent, whether the graphics card decoded it, and why
    /// nothing could, if nothing could.
    codec: Option<&'static str>,
    decoded_on_graphics_card: bool,
    decoder_problem: Option<String>,
    video_kbps: u64,
    sound_packets: u64,
    sound_kbps: u64,
    /// The PC takes the microphone of this device.
    takes_microphone: bool,
    /// Moments of this device's microphone that left for the PC, of twenty
    /// milliseconds each, when it was asked for.
    microphone_frames: Option<u64>,
    /// How loud the loudest of them was, from 0 to 1.
    microphone_loudest: Option<f32>,
    /// Why the microphone that was asked for was not sent.
    microphone_problem: Option<String>,
    /// The PC takes the camera of this device.
    takes_camera: bool,
    /// Pictures of this device's camera that left for the PC, when it was
    /// asked for, and how they were sent.
    camera_pictures: Option<u64>,
    camera_width: Option<u32>,
    camera_height: Option<u32>,
    camera_fps: Option<u32>,
    camera_kbps: Option<u64>,
    /// Why the camera that was asked for was not sent, or stopped.
    camera_problem: Option<String>,
    /// Why the stream ended, when it ended before the time was up.
    ended: Option<String>,
}

pub fn play(
    client: &Arc<Client>,
    pc: &str,
    app: &str,
    seconds: u64,
    asked: Asked,
    back: Back,
    json: bool,
) -> Result<String, String> {
    let Asked { settings, hardware } = asked;
    let host = find(client, pc, false)?;
    let apps = client.apps(&host.id).map_err(|error| error.to_string())?;
    let app = pick_app(&apps, app)?;

    let counter = Counter::default();
    let decoding = Arc::new(Mutex::new(Decoding::default()));
    let video = Decoder {
        counter: counter.video(),
        decoder: None,
        hardware,
        picture: Picture::default(),
        decoding: Arc::clone(&decoding),
    };
    let (ended, endings) = mpsc::channel();
    let asked = Instant::now();
    let session = client
        .play(&host.id, app.id, settings, video, counter.audio(), move |reason| {
            let _ = ended.send(reason);
        })
        .map_err(|error| error.to_string())?;
    let session = Arc::new(session);
    let connected = Instant::now();
    eprintln!("Streaming {} from {} for {seconds} seconds…", app.title, host.name);
    let takes_microphone = session.takes_microphone();
    let listening = back.microphone.then(|| Listening::start(&session));
    // Few PCs take a camera, and one that takes none may be slow to say so.
    // The stream is not kept waiting for it: only a camera to send is.
    let mut asking = Some({
        let (client, id) = (Arc::clone(client), host.id.clone());
        thread::spawn(move || client.camera_offer(&id).ok().flatten())
    });
    let mut answered = move || asking.take().and_then(|asking| asking.join().ok()).flatten();
    let mut takes_camera = None;
    let filming = back.camera.map(|which| {
        let offer = answered();
        takes_camera = Some(offer.is_some());
        Filming::start(client, &host.id, offer, which)
    });

    // Until the time is up, or the stream ends on its own.
    let mut loudest = 0f32;
    let ended = loop {
        let left = Duration::from_secs(seconds).saturating_sub(connected.elapsed());
        if left.is_zero() {
            break None;
        }
        if let Ok(reason) = endings.recv_timeout(left.min(LISTEN_EVERY)) {
            break Some(reason);
        }
        if let Some(Ok(listening)) = &listening {
            loudest = loudest.max(listening.microphone.level());
        }
    };
    let watched = connected.elapsed().as_secs_f64();
    let tally = counter.tally();
    let decoding = decoding.lock().unwrap_or_else(PoisonError::into_inner).clone();
    // The microphone stops with the stream. Leaving a stream is letting go
    // of it.
    let voice = listening
        .map(|listening| listening.map(|listening| listening.frames.load(Ordering::Relaxed)));
    // So does the camera, and the PC is told before this returns.
    let film = filming.map(|filming| filming.map(Filming::stop));
    let takes_camera = takes_camera.unwrap_or_else(|| answered().is_some());
    drop(session);

    let kbps = |bytes: u64| (bytes as f64 * 8.0 / 1000.0 / watched.max(0.001)).round() as u64;
    let report = Report {
        pc: host.name.clone(),
        app: app.title.clone(),
        width: settings.width,
        height: settings.height,
        fps: settings.fps,
        connected_in: (connected - asked).as_secs_f64(),
        watched,
        pictures: tally.frames,
        keyframes: tally.keyframes,
        pictures_missing: tally.frames_missing,
        pictures_decoded: decoding.decoded,
        decoded_width: decoding.width,
        decoded_height: decoding.height,
        pictures_undecodable: decoding.failed,
        codec: decoding.codec.map(Codec::name),
        decoded_on_graphics_card: decoding.on_graphics_card,
        decoder_problem: decoding.problem.clone(),
        video_kbps: kbps(tally.video_bytes),
        sound_packets: tally.audio_packets,
        sound_kbps: kbps(tally.audio_bytes),
        takes_microphone,
        microphone_frames: voice.as_ref().and_then(|voice| voice.as_ref().ok().copied()),
        microphone_loudest: voice.as_ref().and_then(|voice| voice.as_ref().ok().map(|_| loudest)),
        microphone_problem: voice.and_then(Result::err),
        takes_camera,
        camera_pictures: filmed(&film).map(|film| film.pictures),
        camera_width: filmed(&film).map(|film| film.sending.width),
        camera_height: filmed(&film).map(|film| film.sending.height),
        camera_fps: filmed(&film).map(|film| film.sending.fps),
        camera_kbps: filmed(&film).map(|film| kbps(film.bytes)),
        camera_problem: match &film {
            Some(Ok(film)) => film.problem.clone(),
            Some(Err(problem)) => Some(problem.clone()),
            None => None,
        },
        ended: ended.map(|reason| match reason {
            Ended::ByHost => "the app closed on the PC".to_owned(),
            Ended::ConnectionLost => "the connection was lost".to_owned(),
            Ended::HostError(code) => format!("the PC ended the stream with error {code:#x}"),
        }),
    };
    if json {
        return to_json(&report);
    }

    let mut lines = vec![
        format!(
            "Streamed {} from {} for {:.1} s (connected in {:.1} s).",
            report.app, report.pc, report.watched, report.connected_in
        ),
        format!(
            "Video  asked for {}x{} at {} fps: {} pictures ({:.1} per second), keyframes: {}, \
             missing: {}, {} kbit/s",
            report.width,
            report.height,
            report.fps,
            report.pictures,
            report.pictures as f64 / report.watched.max(0.001),
            report.keyframes,
            report.pictures_missing,
            report.video_kbps
        ),
        match &report.decoder_problem {
            Some(problem) => format!(
                "       in {}, which this device cannot decode: {problem}",
                report.codec.unwrap_or("an unknown codec")
            ),
            None => format!(
                "       in {}, decoded {} of them as {}x{} on the {}, could not decode {}",
                report.codec.unwrap_or("an unknown codec"),
                report.pictures_decoded,
                report.decoded_width,
                report.decoded_height,
                if report.decoded_on_graphics_card { "graphics card" } else { "processor" },
                report.pictures_undecodable
            ),
        },
        format!("Sound  {} packets, {} kbit/s", report.sound_packets, report.sound_kbps),
        match (&report.microphone_problem, report.microphone_frames, report.takes_microphone) {
            (Some(problem), _, _) => format!("Voice  not sent: {problem}"),
            (None, Some(frames), _) => format!(
                "Voice  sent {frames} moments of this device's microphone, the loudest at {:.0}% \
                 of what it can capture",
                report.microphone_loudest.unwrap_or_default() * 100.0
            ),
            (None, None, true) => {
                "Voice  the PC takes a microphone: `--microphone` sends it this device's".to_owned()
            }
            (None, None, false) => "Voice  the PC takes no microphone".to_owned(),
        },
    ];
    lines.extend(match (&report.camera_problem, &film, report.takes_camera) {
        (Some(problem), Some(Err(_)), _) => vec![format!("Camera not sent: {problem}")],
        (problem, Some(Ok(film)), _) => {
            let mut lines = vec![format!(
                "Camera sent {} pictures of this device's camera as {}x{} at {} fps, {} kbit/s",
                film.pictures,
                film.sending.width,
                film.sending.height,
                film.sending.fps,
                kbps(film.bytes)
            )];
            lines.extend(problem.iter().map(|problem| format!("       it stopped: {problem}")));
            lines
        }
        (_, _, true) => {
            vec!["Camera the PC takes a camera: `--camera` sends it this device's".to_owned()]
        }
        (_, _, false) => vec!["Camera the PC takes no camera".to_owned()],
    });
    lines.push(match &report.ended {
        Some(reason) => format!("The stream ended early: {reason}."),
        None => format!(
            "{} keeps running on the PC. `leto-cli quit {}` closes it.",
            report.app, report.pc
        ),
    });
    Ok(lines.join("\n"))
}

pub fn quit(client: &Client, pc: &str, json: bool) -> Result<String, String> {
    let host = find(client, pc, false)?;
    let host = client.quit_app(&host.id).map_err(|error| error.to_string())?;
    if json {
        return to_json(&host);
    }
    Ok(format!("No app is running on {} now.", host.name))
}

/// The microphone of this device, sent to a PC for as long as this is kept.
struct Listening {
    microphone: Microphone,
    /// Moments that left for the PC.
    frames: Arc<AtomicU64>,
}

impl Listening {
    /// Starts sending the microphone to the PC of `session`, or says why
    /// not.
    fn start(session: &Arc<Session>) -> Result<Self, String> {
        if !session.takes_microphone() {
            return Err("the PC takes no microphone".to_owned());
        }
        let frames = Arc::new(AtomicU64::new(0));
        let (session, sent) = (Arc::clone(session), Arc::clone(&frames));
        let mut number = 0u32;
        let microphone = Microphone::open(None, move |frame| {
            if session.send_microphone(VOICE_FRAME * number, frame) {
                sent.fetch_add(1, Ordering::Relaxed);
            }
            number = number.wrapping_add(1);
        })
        .map_err(|error| error.to_string())?;
        Ok(Self { microphone, frames })
    }
}

/// What a camera sent to a PC.
struct Film {
    sending: Sending,
    pictures: u64,
    bytes: u64,
    /// Why it stopped before it was asked to.
    problem: Option<String>,
}

/// What was sent of a camera that was asked for and got going.
fn filmed(film: &Option<Result<Film, String>>) -> Option<&Film> {
    film.as_ref().and_then(|film| film.as_ref().ok())
}

/// The camera of this device, sent to a PC for as long as this is kept.
struct Filming {
    feed: CameraFeed,
    sending: Sending,
}

impl Filming {
    /// Starts sending a camera to the PC that made `offer`, or says why
    /// not. Waits until pictures leave.
    fn start(
        client: &Arc<Client>,
        host: &HostId,
        offer: Option<CameraOffer>,
        which: WhichCamera,
    ) -> Result<Self, String> {
        let offer = offer.ok_or("the PC takes no camera")?;
        let limits =
            Limits { width: offer.max_width, height: offer.max_height, fps: offer.max_fps };
        let carrier = ToPc { client: Arc::clone(client), host: host.clone(), link: None };
        let (told, news) = mpsc::channel();
        let told = move || {
            let _ = told.send(());
        };
        let feed = match which {
            WhichCamera::System => CameraFeed::open(None, limits, carrier, told),
            WhichCamera::Card => CameraFeed::test_card(limits, carrier, told),
        };
        let asked = Instant::now();
        loop {
            match feed.state() {
                Feeding::Sending(sending) => return Ok(Self { feed, sending }),
                Feeding::Starting if asked.elapsed() < CAMERA_PATIENCE => {
                    let _ = news.recv_timeout(LISTEN_EVERY);
                }
                Feeding::Starting => return Err("the camera gave no picture".to_owned()),
                Feeding::Over => return Err("the PC stopped taking it".to_owned()),
                Feeding::Lost(problem) | Feeding::Refused(problem) => return Err(problem),
            }
        }
    }

    /// Stops sending, tells the PC, and says what left.
    fn stop(self) -> Film {
        let problem = match self.feed.state() {
            Feeding::Starting | Feeding::Sending(_) => None,
            Feeding::Over => Some("the PC stopped taking it".to_owned()),
            Feeding::Lost(problem) | Feeding::Refused(problem) => Some(problem),
        };
        let fed = self.feed.fed();
        self.feed.stop();
        Film { sending: self.sending, pictures: fed.pictures, bytes: fed.bytes, problem }
    }
}

/// Carries the pictures of a camera to a PC.
struct ToPc {
    client: Arc<Client>,
    host: HostId,
    /// The way there, once the PC agreed.
    link: Option<CameraLink>,
}

impl Carrier for ToPc {
    fn open(&mut self, sending: Sending) -> Result<(), String> {
        let settings = CameraSettings {
            width: sending.width,
            height: sending.height,
            fps: sending.fps,
            bitrate_kbps: sending.bitrate_kbps,
        };
        let link = self.client.send_camera(&self.host, settings).map_err(|e| e.to_string())?;
        self.link = Some(link);
        Ok(())
    }

    fn carry(&mut self, at: Duration, picture: Encoded<'_>) -> Option<bool> {
        self.link.as_mut().map(|link| link.send(at, picture.key_frame, picture.data))
    }
}

impl Drop for ToPc {
    /// The PC knows that the camera stopped before the command ends.
    fn drop(&mut self) {
        if let Some(link) = self.link.take() {
            link.close();
        }
    }
}

/// How decoding went.
#[derive(Debug, Default, Clone)]
struct Decoding {
    decoded: u64,
    failed: u64,
    width: usize,
    height: usize,
    codec: Option<Codec>,
    on_graphics_card: bool,
    /// Why there was no decoder.
    problem: Option<String>,
}

/// Counts the pictures that arrive and decodes them as the app would,
/// asking for a keyframe when one cannot be decoded.
struct Decoder<V> {
    counter: V,
    /// Made once the PC says what it sends.
    decoder: Option<VideoDecoder>,
    /// It may decode on the graphics card.
    hardware: bool,
    picture: Picture,
    decoding: Arc<Mutex<Decoding>>,
}

impl<V: VideoSink> VideoSink for Decoder<V> {
    fn setup(&mut self, setup: VideoSetup) {
        let VideoSetup { codec, width, height, fps } = setup;
        let made = VideoDecoder::for_stream(codec, width, height, fps, self.hardware);
        let mut decoding = self.decoding.lock().unwrap_or_else(PoisonError::into_inner);
        decoding.codec = Some(codec);
        match made {
            Ok(decoder) => {
                decoding.on_graphics_card = decoder.on_graphics_card();
                self.decoder = Some(decoder);
            }
            Err(error) => decoding.problem = Some(error.to_string()),
        }
        drop(decoding);
        self.counter.setup(setup);
    }

    fn frame(&mut self, frame: VideoFrame<'_>) -> Decoded {
        let Some(decoder) = &mut self.decoder else {
            self.counter.frame(frame);
            return Decoded::Ok;
        };
        let outcome = decoder.decode(frame.data, &mut self.picture);
        let on_graphics_card = decoder.on_graphics_card();
        self.counter.frame(frame);
        let mut decoding = self.decoding.lock().unwrap_or_else(PoisonError::into_inner);
        // A graphics card that fails H.264 hands it to the processor.
        decoding.on_graphics_card = on_graphics_card;
        match outcome {
            Ok(true) => {
                decoding.decoded += 1;
                decoding.width = self.picture.width;
                decoding.height = self.picture.height;
                Decoded::Ok
            }
            Ok(false) => Decoded::Ok,
            Err(_) => {
                decoding.failed += 1;
                Decoded::NeedsKeyframe
            }
        }
    }

    fn stop(&mut self) {
        self.counter.stop();
    }
}

/// Finds the app a person means by its title or its id.
fn pick_app<'a>(apps: &'a [AppSummary], wanted: &str) -> Result<&'a AppSummary, String> {
    let wanted = wanted.trim();
    let by_id = wanted.parse::<u32>().ok();
    apps.iter()
        .find(|app| Some(app.id) == by_id || app.title.to_lowercase() == wanted.to_lowercase())
        .ok_or_else(|| {
            format!("the PC has no app called \"{wanted}\". `leto-cli apps` lists the ones it has.")
        })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use leto_client::{Pin, engine::Codecs};
    use leto_testhost::{FakeHost, HOST_NAME};

    use super::*;
    use crate::pcs::pair;

    const SMALL: Asked = Asked {
        settings: StreamSettings {
            width: 320,
            height: 180,
            fps: 30,
            bitrate_kbps: 2000,
            virtual_display: None,
            optimize_games: true,
            sound_on_pc: false,
            codecs: Codecs::of(Codec::H264),
        },
        hardware: true,
    };

    #[test]
    fn an_app_is_played_measured_and_closed() {
        let folder = std::env::temp_dir().join(format!("leto-cli-{}-play", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let host = FakeHost::start();
        let client = Arc::new(Client::open(&folder).unwrap());
        let pin: Pin = "4821".parse().unwrap();
        host.type_pin(pin);
        pair(&client, &host.address(), Some(pin), false).unwrap();
        let nothing = Back::default();

        let error = play(&client, HOST_NAME, "Solitaire", 1, SMALL, nothing, false).unwrap_err();
        assert!(error.contains("no app called \"Solitaire\""), "{error}");

        // By title, whatever the letter case.
        let json = play(&client, HOST_NAME, "desktop", 2, SMALL, nothing, true).unwrap();
        let report: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(report["app"], "Desktop");
        assert!(report["pictures"].as_u64().unwrap() >= 30, "{json}");
        assert!(report["keyframes"].as_u64().unwrap() >= 1, "{json}");
        assert_eq!(report["pictures_decoded"], report["pictures"], "{json}");
        assert_eq!(report["pictures_undecodable"], 0, "{json}");
        // The stand-in PC sends H.264 alone.
        assert_eq!(report["codec"], "H.264", "{json}");
        assert_eq!(report["decoder_problem"], serde_json::Value::Null, "{json}");
        assert_eq!(
            (&report["decoded_width"], &report["decoded_height"]),
            (&320.into(), &180.into())
        );
        assert!(report["sound_packets"].as_u64().unwrap() >= 200, "{json}");
        // The stand-in PC takes a microphone, and nobody asked to send one.
        assert_eq!(report["takes_microphone"], true, "{json}");
        assert_eq!(report["microphone_frames"], serde_json::Value::Null, "{json}");
        // The same goes for the camera.
        assert_eq!(report["takes_camera"], true, "{json}");
        assert_eq!(report["camera_pictures"], serde_json::Value::Null, "{json}");
        assert!(!host.camera_report().open);
        assert_eq!(report["ended"], serde_json::Value::Null);
        assert_eq!(host.running_app(), Some(1));

        // By id, in plain text. The app was left running, so this resumes it.
        let text = play(&client, HOST_NAME, "1", 1, SMALL, nothing, false).unwrap();
        assert!(text.contains("Video  asked for 320x180 at 30 fps"), "{text}");
        assert!(text.contains("Voice  the PC takes a microphone"), "{text}");
        assert!(text.contains("Camera the PC takes a camera"), "{text}");
        assert!(
            text.ends_with("Desktop keeps running on the PC. `leto-cli quit Salon` closes it.")
        );

        assert_eq!(quit(&client, HOST_NAME, false).unwrap(), "No app is running on Salon now.");
        assert_eq!(host.running_app(), None);
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_camera_is_sent_to_a_pc_that_takes_one_and_stops_with_the_stream() {
        let folder = std::env::temp_dir().join(format!("leto-cli-{}-camera", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let host = FakeHost::start();
        let client = Arc::new(Client::open(&folder).unwrap());
        let pin: Pin = "4821".parse().unwrap();
        host.type_pin(pin);
        pair(&client, &host.address(), Some(pin), false).unwrap();
        let card = Back { microphone: false, camera: Some(WhichCamera::Card) };

        let json = play(&client, HOST_NAME, "desktop", 2, SMALL, card, true).unwrap();
        let report: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(report["takes_camera"], true, "{json}");
        assert_eq!(report["camera_problem"], serde_json::Value::Null, "{json}");
        // More than the first: pictures went on leaving. How many depends on
        // how well this computer keeps time, which the test card goes by: on
        // GitHub's macOS, whose naps run late by up to 170 ms, 15 left in
        // these two seconds. leto-media tests the camera's pace exactly.
        let sent = report["camera_pictures"].as_u64().unwrap();
        assert!(sent >= 2, "{json}");
        assert_eq!(
            (&report["camera_width"], &report["camera_height"], &report["camera_fps"]),
            (&640.into(), &360.into(), &30.into())
        );

        // What left arrived, decoded, and the PC knows that it is over.
        let seen = host.camera_report();
        assert!(!seen.open);
        assert_eq!(seen.asked, Some((640, 360, 30)));
        assert_eq!(seen.refused, 0);
        // On a computer that is busy with other tests a packet may be lost
        // on the way, and the pictures until the next key frame with it.
        assert!(seen.pictures >= sent / 2 && seen.key_frames >= 1, "{seen:?} of {sent}");
        assert_eq!(seen.size, Some((640, 360)));

        // The owner of the PC says no: the stream goes on without a camera.
        host.refuse_camera(true);
        let text = play(&client, HOST_NAME, "1", 1, SMALL, card, false).unwrap();
        assert!(
            text.contains("Camera not sent: the PC does not take the camera of this device"),
            "{text}"
        );
        assert!(text.contains("Video  asked for 320x180 at 30 fps"), "{text}");
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_pc_that_takes_no_camera_is_sent_none() {
        let folder = std::env::temp_dir().join(format!("leto-cli-{}-stock", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let host = FakeHost::start_with(leto_testhost::Options {
            camera: None,
            ..leto_testhost::Options::default()
        });
        let client = Arc::new(Client::open(&folder).unwrap());
        let pin: Pin = "4821".parse().unwrap();
        host.type_pin(pin);
        pair(&client, &host.address(), Some(pin), false).unwrap();

        let text = play(&client, HOST_NAME, "1", 1, SMALL, Back::default(), false).unwrap();
        assert!(text.contains("Camera the PC takes no camera"), "{text}");
        let card = Back { microphone: false, camera: Some(WhichCamera::Card) };
        let text = play(&client, HOST_NAME, "1", 1, SMALL, card, false).unwrap();
        assert!(text.contains("Camera not sent: the PC takes no camera"), "{text}");
        let _ = std::fs::remove_dir_all(&folder);
    }
}
