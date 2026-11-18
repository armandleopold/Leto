//! The engine driven by requests, as the desktop app and the bridge drive
//! it, against a stand-in host: asked for something, it answers with news.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use leto_client::{
    AudioSink, CameraSettings, Client, Counter, Service, Shell, VideoSink,
    engine::{CameraOffer, Event, Request, StreamEnd},
    input::{Input, InputAllowed, Key, Modifiers},
};
use leto_testhost::{FakeHost, Options, permission, quiet_voice};

/// How long any step may take before the test gives up.
const PATIENCE: Duration = Duration::from_secs(20);

/// What a small camera would send.
const CAMERA: CameraSettings =
    CameraSettings { width: 320, height: 240, fps: 30, bitrate_kbps: 500 };

/// A fresh settings folder per test.
struct Settings(PathBuf);

impl Settings {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name = format!(
            "leto-service-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        );
        let path = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&path);
        Self(path)
    }
}

impl Drop for Settings {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A shell that keeps the news in a queue and counts what a stream carries.
struct Listener {
    news: Mutex<mpsc::Sender<Event>>,
    counter: Counter,
    /// What `sinks` answers when set: this shell cannot show a stream.
    cannot_show: Option<&'static str>,
    /// This shell has a camera to send.
    camera: bool,
}

/// The counter's sinks have types without a name: boxed, they have one.
struct Pictures(Box<dyn VideoSink>);
struct Sound(Box<dyn AudioSink>);

impl Shell for Listener {
    type Video = Pictures;
    type Audio = Sound;

    fn tell(&self, event: Event) {
        let _ = self.news.lock().unwrap().send(event);
    }

    fn sinks(&self) -> Result<(Self::Video, Self::Audio), String> {
        match self.cannot_show {
            Some(why) => Err(why.to_owned()),
            None => Ok((
                Pictures(Box::new(self.counter.video())),
                Sound(Box::new(self.counter.audio())),
            )),
        }
    }

    fn sends_camera(&self) -> bool {
        self.camera
    }
}

struct Desk {
    service: Service<Listener>,
    news: mpsc::Receiver<Event>,
    _settings: Settings,
}

impl Desk {
    fn new() -> Self {
        Self::with(None)
    }

    fn with(cannot_show: Option<&'static str>) -> Self {
        Self::of(cannot_show, false)
    }

    /// A desk with a camera to send.
    fn with_camera() -> Self {
        Self::of(None, true)
    }

    fn of(cannot_show: Option<&'static str>, camera: bool) -> Self {
        let settings = Settings::new();
        let client = Arc::new(Client::open(&settings.0).unwrap());
        let (sender, news) = mpsc::channel();
        let listener =
            Listener { news: Mutex::new(sender), counter: Counter::default(), cannot_show, camera };
        Self { service: Service::new(client, listener), news, _settings: settings }
    }

    /// The next piece of news `wanted` picks, skipping the rest.
    fn wait<T>(&self, mut wanted: impl FnMut(Event) -> Option<T>) -> T {
        let start = Instant::now();
        loop {
            let left = PATIENCE.checked_sub(start.elapsed()).expect("the news never came");
            let event = self.news.recv_timeout(left).expect("the news never came");
            if let Some(found) = wanted(event) {
                return found;
            }
        }
    }

    /// Adds the stand-in host and pairs with it, as a person would.
    fn pair(&self, host: &FakeHost) -> leto_client::HostId {
        self.service.ask(Request::Add(host.address()));
        let added = self.wait(|event| match event {
            Event::Added(result) => Some(result.unwrap()),
            _ => None,
        });
        self.service.ask(Request::Pair { host: added.id.clone(), attempt: 1 });
        let pin = self.wait(|event| match event {
            Event::PinChosen { attempt: 1, pin } => Some(pin),
            _ => None,
        });
        host.type_pin(pin);
        let paired = self.wait(|event| match event {
            Event::PairingEnded { attempt: 1, result } => Some(result.unwrap()),
            _ => None,
        });
        assert!(paired.paired);
        added.id
    }
}

impl VideoSink for Pictures {
    fn setup(&mut self, setup: leto_client::VideoSetup) {
        self.0.setup(setup);
    }
    fn frame(&mut self, frame: leto_client::VideoFrame<'_>) -> leto_client::Decoded {
        self.0.frame(frame)
    }
    fn stop(&mut self) {
        self.0.stop();
    }
}

impl AudioSink for Sound {
    fn setup(&mut self, setup: leto_client::AudioSetup) {
        self.0.setup(setup);
    }
    fn packet(&mut self, opus: &[u8]) {
        self.0.packet(opus);
    }
    fn stop(&mut self) {
        self.0.stop();
    }
}

#[test]
fn a_pc_is_added_paired_and_its_apps_listed_with_their_covers() {
    let host = FakeHost::start();
    let desk = Desk::new();
    let id = desk.pair(&host);

    desk.service.ask(Request::Apps(id.clone()));
    let apps = desk.wait(|event| match event {
        Event::Apps { host, result } if host == id => Some(result.unwrap()),
        _ => None,
    });
    assert!(!apps.is_empty());
    // One piece of news per app, cover or not, in the order of the list.
    for app in &apps {
        let about = desk.wait(|event| match event {
            Event::Cover { host, app, .. } if host == id => Some(app),
            _ => None,
        });
        assert_eq!(about, app.id);
    }
}

#[test]
fn an_address_that_answers_nothing_is_told_in_a_sentence() {
    let desk = Desk::new();
    desk.service.ask(Request::Add("not an address".into()));
    let message = desk.wait(|event| match event {
        Event::Added(result) => Some(result.unwrap_err()),
        _ => None,
    });
    assert!(message.ends_with('.'), "{message}");
}

#[test]
fn a_stream_starts_takes_input_and_is_left() {
    let host = FakeHost::start();
    let desk = Desk::new();
    let id = desk.pair(&host);
    assert_eq!(desk.service.streaming(), None);

    desk.service.ask(Request::Play { host: id.clone(), app: 1, attempt: 7 });
    let (allowed, microphone) = desk.wait(|event| match event {
        Event::Playing { attempt: 7, input, microphone } => Some((input, microphone)),
        Event::PlayFailed { message, .. } => panic!("{message}"),
        _ => None,
    });
    assert_eq!(allowed, InputAllowed::ALL);
    assert!(microphone, "the stand-in PC takes a microphone");
    assert_eq!(desk.service.streaming(), Some(7));

    let key =
        Input::Key { key: Key::letter('L').unwrap(), down: true, modifiers: Modifiers::default() };
    desk.service.input([key]);
    let start = Instant::now();
    while host.inputs().is_empty() {
        assert!(start.elapsed() < PATIENCE, "the key never reached the host");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(host.inputs(), [key]);

    // Pictures reach the sinks the shell gave.
    let start = Instant::now();
    while desk.service.shell().counter.tally().frames == 0 {
        assert!(start.elapsed() < PATIENCE, "no picture arrived");
        std::thread::sleep(Duration::from_millis(10));
    }

    // The microphone goes the other way.
    let quiet = quiet_voice();
    let start = Instant::now();
    let mut frames = 0;
    while host.stream_report().is_none_or(|report| report.microphone_packets == 0) {
        assert!(start.elapsed() < PATIENCE, "the microphone never reached the host");
        assert!(desk.service.microphone(Duration::from_millis(20) * frames, &quiet));
        frames += 1;
        std::thread::sleep(Duration::from_millis(10));
    }

    desk.service.ask(Request::Leave);
    assert_eq!(desk.service.streaming(), None);
    assert!(!desk.service.microphone(Duration::ZERO, &quiet), "nobody is listening any more");
    // Left, not ended: the app keeps running on the PC.
    assert_eq!(host.running_app(), Some(1));
}

#[test]
fn a_pc_that_takes_no_microphone_says_so_and_gets_none() {
    let host = FakeHost::start_with(Options { microphone: false, ..Options::default() });
    let desk = Desk::new();
    let id = desk.pair(&host);
    desk.service.ask(Request::Play { host: id, app: 1, attempt: 3 });
    let microphone = desk.wait(|event| match event {
        Event::Playing { attempt: 3, microphone, .. } => Some(microphone),
        Event::PlayFailed { message, .. } => panic!("{message}"),
        _ => None,
    });
    assert!(!microphone);
    assert!(!desk.service.microphone(Duration::ZERO, &quiet_voice()));
}

#[test]
fn an_app_closed_on_the_pc_ends_the_stream_with_news() {
    let host = FakeHost::start();
    let desk = Desk::new();
    let id = desk.pair(&host);
    desk.service.ask(Request::Play { host: id, app: 1, attempt: 2 });
    desk.wait(|event| matches!(event, Event::Playing { attempt: 2, .. }).then_some(()));

    host.close_app();
    let why = desk.wait(|event| match event {
        Event::StreamEnded { attempt: 2, why } => Some(why),
        _ => None,
    });
    assert_eq!(why, StreamEnd::AppClosed);
}

/// Waits until `done` says so.
fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < PATIENCE, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_camera_goes_along_with_a_stream_and_stops_when_it_is_left() {
    let host = FakeHost::start();
    let desk = Desk::with_camera();
    let id = desk.pair(&host);
    // The stand-in PC would take it, and the engine waits for a stream.
    assert!(desk.service.camera_open(CAMERA).is_err());
    assert!(!host.camera_report().open);

    desk.service.ask(Request::Play { host: id, app: 1, attempt: 4 });
    // The news that the stream plays comes first: nothing waits for the
    // camera.
    let mut playing = false;
    let offer = desk.wait(|event| match event {
        Event::Playing { attempt: 4, .. } => {
            playing = true;
            None
        }
        Event::Camera { attempt: 4, offer } => Some(offer),
        Event::PlayFailed { message, .. } => panic!("{message}"),
        _ => None,
    });
    assert!(playing);
    assert_eq!(offer, Some(CameraOffer { max_width: 1920, max_height: 1080, max_fps: 30 }));
    assert!(!host.camera_report().open, "nothing is sent until the person says so");

    let camera = desk.service.camera_open(CAMERA).unwrap();
    assert_eq!(host.camera_report().asked, Some((320, 240, 30)));
    // Three packets of something: what the pictures are is not the
    // engine's business.
    assert_eq!(desk.service.camera(camera, Duration::ZERO, true, &[7; 3000]), Some(true));
    wait_for("the camera to arrive", || host.camera_report().packets == 3);
    assert_eq!(host.camera_report().refused, 0);

    desk.service.ask(Request::Leave);
    assert_eq!(desk.service.camera(camera, Duration::ZERO, true, &[7; 3000]), None);
    wait_for("the PC to learn that the camera stopped", || !host.camera_report().open);
}

#[test]
fn a_camera_stops_with_a_stream_that_ends_and_when_asked() {
    let host = FakeHost::start();
    let desk = Desk::with_camera();
    let id = desk.pair(&host);
    desk.service.ask(Request::Play { host: id, app: 1, attempt: 2 });
    desk.wait(|event| matches!(event, Event::Camera { attempt: 2, .. }).then_some(()));

    let first = desk.service.camera_open(CAMERA).unwrap();
    desk.service.camera_close(first);
    assert_eq!(desk.service.camera(first, Duration::ZERO, true, b"a picture"), None);
    wait_for("the PC to learn that the camera stopped", || !host.camera_report().open);

    // A camera that is asked for again takes the place of the one before,
    // and whoever still holds that one neither sends through it nor stops
    // it.
    let second = desk.service.camera_open(CAMERA).unwrap();
    let third = desk.service.camera_open(CAMERA).unwrap();
    assert_eq!(desk.service.camera(second, Duration::ZERO, true, b"a picture"), None);
    desk.service.camera_close(second);
    assert_eq!(desk.service.camera(third, Duration::ZERO, true, b"a picture"), Some(true));
    wait_for("the camera to arrive", || host.camera_report().packets == 1);
    assert!(host.camera_report().open);

    host.close_app();
    desk.wait(|event| matches!(event, Event::StreamEnded { attempt: 2, .. }).then_some(()));
    assert_eq!(desk.service.camera(third, Duration::ZERO, true, b"a picture"), None);
    wait_for("the PC to learn that the camera stopped", || !host.camera_report().open);
}

#[test]
fn a_pc_that_takes_no_camera_says_so_and_one_that_refuses_is_explained() {
    let stock = FakeHost::start_with(Options { camera: None, ..Options::default() });
    let desk = Desk::with_camera();
    let id = desk.pair(&stock);
    desk.service.ask(Request::Play { host: id, app: 1, attempt: 1 });
    let offer = desk.wait(|event| match event {
        Event::Camera { attempt: 1, offer } => Some(offer),
        Event::PlayFailed { message, .. } => panic!("{message}"),
        _ => None,
    });
    assert_eq!(offer, None);
    drop(desk);

    let host = FakeHost::start();
    let desk = Desk::with_camera();
    let id = desk.pair(&host);
    desk.service.ask(Request::Play { host: id, app: 1, attempt: 1 });
    desk.wait(|event| matches!(event, Event::Camera { attempt: 1, .. }).then_some(()));
    host.refuse_camera(true);
    let message = desk.service.camera_open(CAMERA).unwrap_err();
    assert!(message.contains("does not let this device send its camera"), "{message}");
    assert!(message.ends_with('.'), "{message}");
    assert!(!host.camera_report().open);
}

#[test]
fn a_shell_without_a_camera_hears_nothing_about_one() {
    let host = FakeHost::start();
    let desk = Desk::new();
    let id = desk.pair(&host);
    desk.service.ask(Request::Play { host: id, app: 1, attempt: 2 });
    desk.wait(|event| matches!(event, Event::Playing { attempt: 2, .. }).then_some(()));
    // Long enough for the news about a camera to have come, had the PC
    // been asked.
    host.close_app();
    desk.wait(|event| match event {
        Event::Camera { .. } => panic!("nobody has a camera to send"),
        Event::StreamEnded { attempt: 2, .. } => Some(()),
        _ => None,
    });
}

#[test]
fn a_shell_that_cannot_show_a_stream_says_why_and_starts_nothing() {
    let host = FakeHost::start();
    let desk = Desk::with(Some("This device cannot decode pictures."));
    let id = desk.pair(&host);
    desk.service.ask(Request::Play { host: id, app: 1, attempt: 1 });
    let message = desk.wait(|event| match event {
        Event::PlayFailed { attempt: 1, message } => Some(message),
        _ => None,
    });
    assert_eq!(message, "This device cannot decode pictures.");
    assert_eq!(host.running_app(), None);
}

#[test]
fn what_apollo_refuses_is_explained_with_the_box_to_tick() {
    let host = FakeHost::start_with(Options {
        permissions: Some(permission::DEFAULT),
        ..Options::default()
    });
    let desk = Desk::new();
    let id = desk.pair(&host);
    desk.service.ask(Request::Play { host: id, app: 1, attempt: 1 });
    let message = desk.wait(|event| match event {
        Event::PlayFailed { attempt: 1, message } => Some(message),
        _ => None,
    });
    assert!(message.contains("\"Launch Apps\""), "{message}");
}

#[test]
fn a_service_that_goes_away_leaves_its_stream() {
    let host = FakeHost::start();
    let desk = Desk::new();
    let id = desk.pair(&host);
    desk.service.ask(Request::Play { host: id, app: 1, attempt: 1 });
    desk.wait(|event| matches!(event, Event::Playing { attempt: 1, .. }).then_some(()));
    // A host only misses a client it has met. The engine says that the
    // stream plays once the host has answered on the control connection,
    // which is a moment before the host hears that its answer arrived.
    let start = Instant::now();
    while !host.stream_report().is_some_and(|report| report.connected) {
        assert!(start.elapsed() < PATIENCE, "the host never met the client");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(host.stream_report().is_some_and(|report| !report.client_left));

    drop(desk);
    let start = Instant::now();
    while !host.stream_report().is_some_and(|report| report.client_left) {
        assert!(start.elapsed() < PATIENCE, "the stream was never left");
        std::thread::sleep(Duration::from_millis(20));
    }
}
