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

use leto_client::{
    Client, Counter, Decoded, Ended, Error, HostKind, HostSummary, Permission, StreamSettings,
    Tally, VideoFrame, VideoSetup, VideoSink,
    engine::{Codec, Codecs},
};
use leto_testhost::{CLIENT_MISSED_WITHIN, FakeHost, Options, permission};

/// A small picture, so the tests spend their time on the protocol.
const SMALL: StreamSettings = StreamSettings {
    width: 320,
    height: 180,
    fps: 30,
    bitrate_kbps: 2000,
    virtual_display: false,
    optimize_games: true,
    sound_on_pc: false,
    codecs: Codecs::of(Codec::H264),
};

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

/// A picture that leaves the PC late is not waited for: the engine gives it
/// up after a tenth of a second, asks for a keyframe and hands over nothing
/// that builds on the picture it gave up. This is the way from one end to
/// the other, whatever the clocks of the computer that runs it are like.
#[test]
fn after_a_picture_that_came_too_late_the_stream_goes_on_from_a_keyframe() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let client = settings.open();
    let paired = pair(&client, &host);
    let counter = Counter::default();
    let session =
        client.play(&paired.id, 1, SMALL, counter.video(), counter.audio(), |_| {}).unwrap();
    let before = wait_for_stream(&counter);

    // Far longer than the engine waits, and than a busy computer stalls.
    host.hold_a_picture(Duration::from_millis(500));
    wait_for("the stream to go on", || {
        let tally = counter.tally();
        tally.frames >= before.frames + 15 && tally.keyframes > before.keyframes
    });

    let tally = counter.tally();
    assert_eq!(tally.frames_orphaned, 0, "a picture that follows a missing one is a keyframe");
    let report = host.stream_report().expect("the host is streaming");
    assert!(report.keyframes_requested >= 1, "{report:?}");
    drop(session);
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
    // Not that none is missing: a computer busy enough sends one late, and
    // the engine stops waiting for a picture after a tenth of a second.
    assert_eq!(tally.frames_orphaned, 0, "a picture that follows a missing one is a keyframe");
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

/// A screen too slow for the stream: it takes longer over each picture
/// than the PC takes to make the next. Not as long as the engine waits for
/// a picture before it gives it up, a tenth of a second: a screen that slow
/// loses the pictures that wait whenever it is a little late.
struct SlowScreen(mpsc::Sender<usize>);

impl VideoSink for SlowScreen {
    fn setup(&mut self, _setup: VideoSetup) {}

    fn frame(&mut self, frame: VideoFrame<'_>) -> Decoded {
        let _ = self.0.send(frame.waiting);
        thread::sleep(Duration::from_millis(50));
        Decoded::Ok
    }
}

#[test]
fn a_screen_that_falls_behind_is_told_how_many_pictures_wait() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let client = settings.open();
    let paired = pair(&client, &host);
    let (sender, told) = mpsc::channel();

    let session =
        client.play(&paired.id, 1, SMALL, SlowScreen(sender), Counter::default().audio(), |_| {});
    let session = session.unwrap();
    // Thirty pictures a second, twenty taken at most: they pile up. How high
    // depends on the clocks of the computer this runs on, which can make the
    // engine give pictures up: that some wait, and are counted, does not.
    let waiting: Vec<usize> =
        (0..20).map(|_| told.recv_timeout(Duration::from_secs(10)).unwrap()).collect();
    session.stop();

    let most = waiting.iter().copied().max().unwrap_or(0);
    assert!(most >= 2, "pictures wait behind a slow screen, and it is told: {waiting:?}");
}

#[test]
fn the_best_codec_both_sides_have_is_chosen_av1_first() {
    let every = Codecs::of(Codec::Av1).with(Codec::H265).with(Codec::H264);
    let h265 = Codecs::of(Codec::H265).with(Codec::H264);
    let h264 = Codecs::of(Codec::H264);
    // What the PC encodes, what this device decodes, and what they settle on.
    for (encodes, decodes, chosen) in [
        (every, every, Codec::Av1),
        (every, h265, Codec::H265),
        (every, h264, Codec::H264),
        (h265, every, Codec::H265),
        (h264, every, Codec::H264),
    ] {
        let settings = Settings::new();
        let host = FakeHost::start_with(Options { encodes, ..Options::default() });
        let client = settings.open();
        let paired = pair(&client, &host);
        let counter = Counter::default();
        let asked = StreamSettings { codecs: decodes, ..SMALL };
        let session =
            client.play(&paired.id, 1, asked, counter.video(), counter.audio(), |_| {}).unwrap();
        wait_for("the video to be set up", || counter.tally().setup.is_some());

        let told = counter.tally().setup.unwrap().codec;
        assert_eq!(told, chosen, "{encodes:?} encoded, {decodes:?} decoded");
        assert_eq!(host.stream_report().unwrap().codec, chosen, "what the PC was asked");
        session.stop();
        wait_for("the stream to wind down", || counter.tally().stopped);
    }
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

    // Nobody tells the host: it has to miss the client, which takes it
    // seconds at best.
    let start = Instant::now();
    while !host.stream_report().is_some_and(|report| report.client_left) {
        assert!(start.elapsed() < CLIENT_MISSED_WITHIN, "the host never noticed the client left");
        thread::sleep(Duration::from_millis(20));
    }
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

#[test]
fn a_device_that_may_only_watch_cannot_start_or_close_an_app() {
    // What Apollo gives every device but the first one paired with it.
    let settings = Settings::new();
    let host = FakeHost::start_with(Options {
        permissions: Some(permission::DEFAULT),
        ..Options::default()
    });
    let client = settings.open();
    let paired = pair(&client, &host);
    assert_eq!(paired.kind, HostKind::Apollo);
    assert_eq!(client.apps(&paired.id).unwrap().len(), 2);

    let counter = Counter::default();
    let refused = client.play(&paired.id, 1, SMALL, counter.video(), counter.audio(), |_| {});
    assert!(matches!(refused, Err(Error::NotAllowed(Permission::LaunchApps))));
    assert_eq!(host.running_app(), None);

    // The owner of the PC ticks the boxes.
    host.set_permissions(Some(permission::ALL));
    let session =
        client.play(&paired.id, 1, SMALL, counter.video(), counter.audio(), |_| {}).unwrap();
    wait_for_stream(&counter);
    session.stop();

    // Watching the app that runs takes less than starting one.
    host.set_permissions(Some(permission::DEFAULT));
    let again = Counter::default();
    let session = client.play(&paired.id, 1, SMALL, again.video(), again.audio(), |_| {}).unwrap();
    wait_for_stream(&again);
    session.stop();

    assert!(matches!(client.quit_app(&paired.id), Err(Error::NotAllowed(Permission::LaunchApps))));
    assert_eq!(host.running_app(), Some(1), "the app keeps running");
}

#[test]
fn a_device_that_may_do_nothing_is_told_so_instead_of_shown_a_fake_app() {
    let settings = Settings::new();
    let host = FakeHost::start_with(Options { permissions: Some(0), ..Options::default() });
    let client = settings.open();
    let paired = pair(&client, &host);

    assert!(matches!(client.apps(&paired.id), Err(Error::NotAllowed(Permission::ListApps))));

    let counter = Counter::default();
    let refused = client.play(&paired.id, 1, SMALL, counter.video(), counter.audio(), |_| {});
    assert!(matches!(refused, Err(Error::NotAllowed(Permission::LaunchApps))));
}

/// One second of a 440 Hz tone as a microphone would send it: Opus frames of
/// twenty milliseconds, one channel at 48 kHz.
fn tone() -> Vec<Vec<u8>> {
    use opus_rs::{Application, OpusEncoder};

    let mut encoder = OpusEncoder::new(48_000, 1, Application::RestrictedLowDelay).unwrap();
    encoder.bitrate_bps = 48_000;
    let mut packet = vec![0u8; 1500];
    (0..50u32)
        .map(|frame| {
            let samples: Vec<f32> = (0..960u32)
                .map(|index| {
                    let time = (frame * 960 + index) as f32 / 48_000.0;
                    0.5 * (time * 440.0 * std::f32::consts::TAU).sin()
                })
                .collect();
            let written = encoder.encode(&samples, 960, &mut packet).unwrap();
            packet[..written].to_vec()
        })
        .collect()
}

#[test]
fn the_microphone_reaches_a_host_that_takes_one() {
    for encrypt_rtsp in [true, false] {
        let settings = Settings::new();
        let host = FakeHost::start_with(Options { encrypt_rtsp, ..Options::default() });
        let client = settings.open();
        let paired = pair(&client, &host);
        let counter = Counter::default();

        let session =
            client.play(&paired.id, 1, SMALL, counter.video(), counter.audio(), |_| {}).unwrap();
        wait_for_stream(&counter);
        assert!(session.takes_microphone());
        let report = host.stream_report().expect("the host is streaming");
        assert!(report.microphone, "the host listens, to an encrypted microphone");
        assert_eq!(report.microphone_packets, 0, "nothing is sent until asked");

        for (number, frame) in tone().iter().enumerate() {
            assert!(session.send_microphone(Duration::from_millis(20) * number as u32, frame));
            thread::sleep(Duration::from_millis(2));
        }
        wait_for("the microphone to arrive", || {
            host.stream_report().is_some_and(|report| report.microphone_packets == 50)
        });

        // What the host heard is the tone: as loud, and at the same pitch.
        let heard = host.heard();
        assert_eq!(heard.len(), 48_000);
        let settled = &heard[4_800..];
        let mean = settled.iter().map(|sample| sample * sample).sum::<f32>() / settled.len() as f32;
        let amplitude = (mean * 2.0).sqrt();
        assert!((0.4..0.6).contains(&amplitude), "heard at {amplitude} instead of 0.5");
        let rises = settled.windows(2).filter(|pair| pair[0] < 0.0 && pair[1] >= 0.0).count();
        let pitch = rises as f32 / (settled.len() as f32 / 48_000.0);
        assert!((435.0..445.0).contains(&pitch), "heard at {pitch} Hz instead of 440");

        session.stop();
        wait_for("the stream to wind down", || counter.tally().stopped);
        // The host remembers what the last stream said.
        assert_eq!(host.heard().len(), 48_000);
    }
}

#[test]
fn a_host_that_knows_nothing_of_microphones_streams_as_before() {
    let settings = Settings::new();
    let host = FakeHost::start_with(Options { microphone: false, ..Options::default() });
    let client = settings.open();
    let paired = pair(&client, &host);
    let counter = Counter::default();

    let session =
        client.play(&paired.id, 1, SMALL, counter.video(), counter.audio(), |_| {}).unwrap();
    let tally = wait_for_stream(&counter);
    assert!(tally.keyframes >= 1);

    assert!(!session.takes_microphone());
    assert!(!session.send_microphone(Duration::ZERO, &tone()[0]));
    let report = host.stream_report().expect("the host is streaming");
    assert!(!report.microphone);
    assert_eq!(report.microphone_packets, 0);
    assert!(host.heard().is_empty());
}
