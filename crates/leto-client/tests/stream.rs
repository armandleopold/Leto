//! The client streaming from a stand-in host: launch, pictures and sound,
//! leaving, resuming, and the app closing on the host.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicU32, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use leto_client::{Client, Counter, Ended, Error, HostSummary, StreamSettings, Tally};
use leto_testhost::{FakeHost, Options};

/// A small picture, so the tests spend their time on the protocol.
const SMALL: StreamSettings =
    StreamSettings { width: 320, height: 180, fps: 30, bitrate_kbps: 2000 };

/// A fresh settings folder per test.
struct Settings(PathBuf);

impl Settings {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name = format!(
            "leto-stream-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        );
        let path = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&path);
        Self(path)
    }

    fn open(&self) -> Client {
        Client::open(&self.0).unwrap()
    }
}

impl Drop for Settings {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn pair(client: &Client, host: &FakeHost) -> HostSummary {
    let added = client.add(&host.address()).unwrap();
    let pin = client.new_pin().unwrap();
    host.type_pin(pin);
    client.pair(&added.id, pin).unwrap()
}

/// Waits until `done` says so, and fails after ten seconds.
fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_stream(counter: &Counter) -> Tally {
    wait_for("pictures and sound", || {
        let tally = counter.tally();
        tally.frames >= 15 && tally.audio_packets >= 40
    });
    counter.tally()
}

#[test]
fn a_stream_delivers_pictures_and_sound() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let client = settings.open();
    let paired = pair(&client, &host);
    let counter = Counter::default();

    let session =
        client.play(&paired.id, 1, SMALL, counter.video(), counter.audio(), |_| {}).unwrap();
    let tally = wait_for_stream(&counter);

    let video = tally.setup.expect("the video was set up");
    assert_eq!((video.width, video.height, video.fps), (320, 180, 30));
    assert!(tally.keyframes >= 1, "the stream starts with a keyframe");
    assert_eq!(tally.frames_missing, 0, "nothing is lost on the loopback interface");
    let audio = tally.audio_setup.expect("the sound was set up");
    assert_eq!((audio.sample_rate, audio.channels, audio.samples_per_packet), (48_000, 2, 240));
    // Sixty bytes per packet: 96 kbit/s in packets of five milliseconds.
    assert_eq!(tally.audio_bytes, tally.audio_packets * 60);

    let report = host.stream_report().expect("the host is streaming");
    assert!(report.connected);
    assert_eq!((report.width, report.height, report.fps), (320, 180, 30));
    assert!(report.video_encrypted && report.audio_encrypted && report.control_encrypted);
    assert_eq!(host.running_app(), Some(1));
    assert_eq!(client.host(&paired.id).unwrap().running_app, Some(1));

    assert!(!session.is_over());
    session.stop();
    wait_for("the stream to wind down", || counter.tally().stopped);
}

#[test]
fn a_host_that_never_encrypts_the_handshake_streams_too() {
    let settings = Settings::new();
    let host = FakeHost::start_with(Options { encrypt_rtsp: false, ..Options::default() });
    let client = settings.open();
    let paired = pair(&client, &host);
    let counter = Counter::default();

    let session =
        client.play(&paired.id, 1, SMALL, counter.video(), counter.audio(), |_| {}).unwrap();
    wait_for_stream(&counter);
    session.stop();
}

#[test]
fn leaving_keeps_the_app_running_and_playing_again_resumes_it() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let client = settings.open();
    let paired = pair(&client, &host);

    let first = Counter::default();
    let session = client.play(&paired.id, 1, SMALL, first.video(), first.audio(), |_| {}).unwrap();
    wait_for_stream(&first);
    session.stop();

    wait_for("the host to notice the client left", || {
        host.stream_report().is_some_and(|report| report.client_left)
    });
    assert_eq!(host.running_app(), Some(1), "the app keeps running");
    let seen = client.refresh().into_iter().find(|seen| seen.id == paired.id).unwrap();
    assert!(seen.busy);
    assert_eq!(seen.running_app, Some(1));

    let second = Counter::default();
    let session =
        client.play(&paired.id, 1, SMALL, second.video(), second.audio(), |_| {}).unwrap();
    let tally = wait_for_stream(&second);
    assert!(tally.keyframes >= 1, "a resumed stream starts with a keyframe");
    session.stop();
}

#[test]
fn another_app_cannot_start_while_one_is_running() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let client = settings.open();
    let paired = pair(&client, &host);
    let counter = Counter::default();
    let session =
        client.play(&paired.id, 1, SMALL, counter.video(), counter.audio(), |_| {}).unwrap();
    wait_for_stream(&counter);
    session.stop();

    let other = Counter::default();
    let refused = client.play(&paired.id, 881_448_767, SMALL, other.video(), other.audio(), |_| {});
    assert!(matches!(refused, Err(Error::OtherAppRunning)));

    // Closing the running app makes room.
    let after = client.quit_app(&paired.id).unwrap();
    assert!(!after.busy);
    assert_eq!(host.running_app(), None);
    let session =
        client.play(&paired.id, 881_448_767, SMALL, other.video(), other.audio(), |_| {}).unwrap();
    wait_for_stream(&other);
    assert_eq!(host.running_app(), Some(881_448_767));
    session.stop();
}

#[test]
fn the_client_is_told_when_the_app_closes_on_the_host() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let client = settings.open();
    let paired = pair(&client, &host);
    let counter = Counter::default();
    let (ended, endings) = mpsc::channel();

    let session = client
        .play(&paired.id, 1, SMALL, counter.video(), counter.audio(), move |reason| {
            let _ = ended.send(reason);
        })
        .unwrap();
    wait_for_stream(&counter);

    host.close_app();

    assert_eq!(endings.recv_timeout(Duration::from_secs(10)), Ok(Ended::ByHost));
    wait_for("the stream to wind down", || session.is_over());
    assert_eq!(client.host(&paired.id).unwrap().running_app, None);
}

#[test]
fn a_stream_that_breaks_while_the_app_runs_is_a_lost_connection() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let client = settings.open();
    let paired = pair(&client, &host);
    let counter = Counter::default();
    let (ended, endings) = mpsc::channel();

    let _session = client
        .play(&paired.id, 1, SMALL, counter.video(), counter.audio(), move |reason| {
            let _ = ended.send(reason);
        })
        .unwrap();
    wait_for_stream(&counter);

    host.drop_stream();

    assert_eq!(endings.recv_timeout(Duration::from_secs(10)), Ok(Ended::ConnectionLost));
    assert_eq!(client.host(&paired.id).unwrap().running_app, Some(1));
}

#[test]
fn leaving_is_not_reported_as_an_ending() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let client = settings.open();
    let paired = pair(&client, &host);
    let counter = Counter::default();
    let (ended, endings) = mpsc::channel::<Ended>();

    let session = client
        .play(&paired.id, 1, SMALL, counter.video(), counter.audio(), move |reason| {
            let _ = ended.send(reason);
        })
        .unwrap();
    wait_for_stream(&counter);
    session.stop();

    assert!(endings.recv_timeout(Duration::from_millis(500)).is_err());
}

#[test]
fn an_unpaired_device_cannot_start_a_stream() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let client = settings.open();
    let added = client.add(&host.address()).unwrap();
    let counter = Counter::default();

    let refused = client.play(&added.id, 1, SMALL, counter.video(), counter.audio(), |_| {});

    assert!(matches!(refused, Err(Error::NotPaired)));
    assert_eq!(host.running_app(), None);
}
