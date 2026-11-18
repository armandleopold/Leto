//! The commands about streams: play an app and say what arrived, close it.
//!
//! `play` shows nothing and plays nothing: it measures. It tells whether a
//! PC streams to Leto at all, and how well, without the graphical app.

use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use leto_client::{AppSummary, Client, Counter, Ended, StreamSettings};
use serde::Serialize;

use crate::{pcs::find, to_json};

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
    video_kbps: u64,
    sound_packets: u64,
    sound_kbps: u64,
    /// Why the stream ended, when it ended before the time was up.
    ended: Option<String>,
}

pub fn play(
    client: &Client,
    pc: &str,
    app: &str,
    seconds: u64,
    settings: StreamSettings,
    json: bool,
) -> Result<String, String> {
    let host = find(client, pc, false)?;
    let apps = client.apps(&host.id).map_err(|error| error.to_string())?;
    let app = pick_app(&apps, app)?;

    let counter = Counter::default();
    let (ended, endings) = mpsc::channel();
    let asked = Instant::now();
    let session = client
        .play(&host.id, app.id, settings, counter.video(), counter.audio(), move |reason| {
            let _ = ended.send(reason);
        })
        .map_err(|error| error.to_string())?;
    let connected = Instant::now();
    eprintln!("Streaming {} from {} for {seconds} seconds…", app.title, host.name);

    // Until the time is up, or the stream ends on its own.
    let ended = endings.recv_timeout(Duration::from_secs(seconds)).ok();
    let watched = connected.elapsed().as_secs_f64();
    let tally = counter.tally();
    session.stop();

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
        video_kbps: kbps(tally.video_bytes),
        sound_packets: tally.audio_packets,
        sound_kbps: kbps(tally.audio_bytes),
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
        format!("Sound  {} packets, {} kbit/s", report.sound_packets, report.sound_kbps),
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

        let error = play(&client, HOST_NAME, "Solitaire", 1, SMALL, false).unwrap_err();
        assert!(error.contains("no app called \"Solitaire\""), "{error}");

        // By title, whatever the letter case.
        let json = play(&client, HOST_NAME, "desktop", 2, SMALL, true).unwrap();
        let report: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(report["app"], "Desktop");
        assert!(report["pictures"].as_u64().unwrap() >= 30, "{json}");
        assert!(report["keyframes"].as_u64().unwrap() >= 1, "{json}");
        assert!(report["sound_packets"].as_u64().unwrap() >= 200, "{json}");
        assert_eq!(report["ended"], serde_json::Value::Null);
        assert_eq!(host.running_app(), Some(1));

        // By id, in plain text. The app was left running, so this resumes it.
        let text = play(&client, HOST_NAME, "1", 1, SMALL, false).unwrap();
        assert!(text.contains("Video  asked for 320x180 at 30 fps"), "{text}");
        assert!(
            text.ends_with("Desktop keeps running on the PC. `leto-cli quit Salon` closes it.")
        );

        assert_eq!(quit(&client, HOST_NAME, false).unwrap(), "No app is running on Salon now.");
        assert_eq!(host.running_app(), None);
        let _ = std::fs::remove_dir_all(&folder);
    }
}
