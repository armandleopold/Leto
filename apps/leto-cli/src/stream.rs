//! The commands about streams: play an app and say what arrived, close it.
//!
//! `play` shows nothing and plays nothing: it measures. It tells whether a
//! PC streams to Leto at all, whether Leto can decode its pictures, and how
//! well they arrive, without the graphical app. Asked to, it also sends the
//! PC this device's microphone, and tells how that went.

use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use leto_client::{
    AppSummary, Client, Counter, Decoded, Ended, Session, StreamSettings, VideoFrame, VideoSetup,
    VideoSink,
};
use leto_media::{Microphone, Picture, VOICE_FRAME, VideoDecoder};
use serde::Serialize;

use crate::{pcs::find, to_json};

/// How often the microphone is asked how loud it is.
const LISTEN_EVERY: Duration = Duration::from_millis(50);

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
    /// Why the stream ended, when it ended before the time was up.
    ended: Option<String>,
}

pub fn play(
    client: &Client,
    pc: &str,
    app: &str,
    seconds: u64,
    settings: StreamSettings,
    microphone: bool,
    json: bool,
) -> Result<String, String> {
    let host = find(client, pc, false)?;
    let apps = client.apps(&host.id).map_err(|error| error.to_string())?;
    let app = pick_app(&apps, app)?;

    let counter = Counter::default();
    let decoding = Arc::new(Mutex::new(Decoding::default()));
    let video = Decoder {
        counter: counter.video(),
        decoder: VideoDecoder::new().map_err(|error| error.to_string())?,
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
    let listening = microphone.then(|| Listening::start(&session));

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
    let decoding = *decoding.lock().unwrap_or_else(PoisonError::into_inner);
    // The microphone stops with the stream. Leaving a stream is letting go
    // of it.
    let voice = listening
        .map(|listening| listening.map(|listening| listening.frames.load(Ordering::Relaxed)));
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
        video_kbps: kbps(tally.video_bytes),
        sound_packets: tally.audio_packets,
        sound_kbps: kbps(tally.audio_bytes),
        takes_microphone,
        microphone_frames: voice.as_ref().and_then(|voice| voice.as_ref().ok().copied()),
        microphone_loudest: voice.as_ref().and_then(|voice| voice.as_ref().ok().map(|_| loudest)),
        microphone_problem: voice.and_then(Result::err),
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
        format!(
            "       decoded {} of them as {}x{}, could not decode {}",
            report.pictures_decoded,
            report.decoded_width,
            report.decoded_height,
            report.pictures_undecodable
        ),
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
        let microphone = Microphone::open(move |frame| {
            if session.send_microphone(VOICE_FRAME * number, frame) {
                sent.fetch_add(1, Ordering::Relaxed);
            }
            number = number.wrapping_add(1);
        })
        .map_err(|error| error.to_string())?;
        Ok(Self { microphone, frames })
    }
}

/// How decoding went.
#[derive(Debug, Default, Clone, Copy)]
struct Decoding {
    decoded: u64,
    failed: u64,
    width: usize,
    height: usize,
}

/// Counts the pictures that arrive and decodes them as the app would,
/// asking for a keyframe when one cannot be decoded.
struct Decoder<V> {
    counter: V,
    decoder: VideoDecoder,
    picture: Picture,
    decoding: Arc<Mutex<Decoding>>,
}

impl<V: VideoSink> VideoSink for Decoder<V> {
    fn setup(&mut self, setup: VideoSetup) {
        self.counter.setup(setup);
    }

    fn frame(&mut self, frame: VideoFrame<'_>) -> Decoded {
        let outcome = self.decoder.decode(frame.data, &mut self.picture);
        self.counter.frame(frame);
        let mut decoding = self.decoding.lock().unwrap_or_else(PoisonError::into_inner);
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

    use leto_client::Pin;
    use leto_testhost::{FakeHost, HOST_NAME};

    use super::*;
    use crate::pcs::pair;

    const SMALL: StreamSettings =
        StreamSettings { width: 320, height: 180, fps: 30, bitrate_kbps: 2000 };

    #[test]
    fn an_app_is_played_measured_and_closed() {
        let folder = std::env::temp_dir().join(format!("leto-cli-{}-play", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let host = FakeHost::start();
        let client = Client::open(&folder).unwrap();
        let pin: Pin = "4821".parse().unwrap();
        host.type_pin(pin);
        pair(&client, &host.address(), Some(pin), false).unwrap();

        let error = play(&client, HOST_NAME, "Solitaire", 1, SMALL, false, false).unwrap_err();
        assert!(error.contains("no app called \"Solitaire\""), "{error}");

        // By title, whatever the letter case.
        let json = play(&client, HOST_NAME, "desktop", 2, SMALL, false, true).unwrap();
        let report: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(report["app"], "Desktop");
        assert!(report["pictures"].as_u64().unwrap() >= 30, "{json}");
        assert!(report["keyframes"].as_u64().unwrap() >= 1, "{json}");
        assert_eq!(report["pictures_decoded"], report["pictures"], "{json}");
        assert_eq!(report["pictures_undecodable"], 0, "{json}");
        assert_eq!(
            (&report["decoded_width"], &report["decoded_height"]),
            (&320.into(), &180.into())
        );
        assert!(report["sound_packets"].as_u64().unwrap() >= 200, "{json}");
        // The stand-in PC takes a microphone, and nobody asked to send one.
        assert_eq!(report["takes_microphone"], true, "{json}");
        assert_eq!(report["microphone_frames"], serde_json::Value::Null, "{json}");
        assert_eq!(report["ended"], serde_json::Value::Null);
        assert_eq!(host.running_app(), Some(1));

        // By id, in plain text. The app was left running, so this resumes it.
        let text = play(&client, HOST_NAME, "1", 1, SMALL, false, false).unwrap();
        assert!(text.contains("Video  asked for 320x180 at 30 fps"), "{text}");
        assert!(text.contains("Voice  the PC takes a microphone"), "{text}");
        assert!(
            text.ends_with("Desktop keeps running on the PC. `leto-cli quit Salon` closes it.")
        );

        assert_eq!(quit(&client, HOST_NAME, false).unwrap(), "No app is running on Salon now.");
        assert_eq!(host.running_app(), None);
        let _ = std::fs::remove_dir_all(&folder);
    }
}
