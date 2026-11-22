//! The companion beside an Apollo: a Leto client streams from the Apollo it
//! is paired with, and sends its camera to the companion, which reads from
//! Apollo's folder who that client is.
//!
//! The Apollo here is the stand-in host, set up as every Apollo is: it
//! takes no camera.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use leto_client::{
    AudioSink, CameraRefusal, CameraSettings, Client, Counter, Error, HostSummary, Service, Shell,
    VideoSink,
    engine::{CameraOffer, Choices, Event, Request},
};
use leto_host::{Companion, Options as CompanionOptions, apollo::Apollo};
use leto_media::{Picture, PictureEncoder, Sending};
use leto_testhost::{FakeHost, Options, permission};

const SMALL: CameraSettings =
    CameraSettings { width: 320, height: 240, fps: 30, bitrate_kbps: 500 };

const RED: [u8; 3] = [200, 40, 40];
const BLUE: [u8; 3] = [40, 60, 200];

/// A fresh folder per test, gone with it.
struct Folder(PathBuf);

impl Folder {
    fn new(what: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name = format!(
            "leto-companion-{what}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        );
        let path = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&path);
        Self(path)
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// An Apollo, with any port that has the one thirteen above it free.
fn apollo() -> FakeHost {
    loop {
        let host =
            FakeHost::start_with(Options { camera: None, microphone: false, ..Options::default() });
        let above = host.port().checked_add(13);
        if above.is_some_and(|port| std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()) {
            return host;
        }
    }
}

fn pair(client: &Client, host: &FakeHost) -> HostSummary {
    let added = client.add(&host.address()).unwrap();
    let pin = client.new_pin().unwrap();
    host.type_pin(pin);
    client.pair(&added.id, pin).unwrap()
}

/// A companion beside `host`, reading the folder `host` writes as an Apollo
/// whose devices have these permissions.
fn companion(host: &FakeHost, folder: &Folder, permissions: Option<u32>) -> Companion {
    host.write_apollo_folder(&folder.0, permissions).unwrap();
    assert_eq!(Apollo::read(&folder.0).unwrap().port(), host.port());
    let options = CompanionOptions { interface: "127.0.0.1".to_owned(), largest: (1280, 720) };
    // The port `apollo()` found free may have gone to a test running beside
    // this one in the meantime: it gives it back when it ends.
    let start = Instant::now();
    loop {
        match Companion::start(Apollo::read(&folder.0).unwrap(), &options) {
            Ok(companion) => return companion,
            Err(error) if start.elapsed() < Duration::from_secs(20) => {
                eprintln!("the companion's port is taken for now: {error}");
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => panic!("the companion did not start: {error}"),
        }
    }
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

/// What a camera would see: one colour on the left, another on the right.
fn halves(width: usize, height: usize, left: [u8; 3], right: [u8; 3]) -> Picture {
    let mut rgba = Vec::with_capacity(width * height * 4);
    for _ in 0..height {
        for column in 0..width {
            let [red, green, blue] = if column < width / 2 { left } else { right };
            rgba.extend_from_slice(&[red, green, blue, 255]);
        }
    }
    Picture { width, height, rgba }
}

fn close(found: &[u8], wanted: [u8; 3]) -> bool {
    found.iter().zip(wanted).all(|(found, wanted)| found.abs_diff(wanted) <= 12)
}

#[test]
fn a_device_paired_with_apollo_sends_its_camera_to_the_companion() {
    let (settings, folder) = (Folder::new("settings"), Folder::new("apollo"));
    let host = apollo();
    let client = Client::open(&settings.0).unwrap();
    let paired = pair(&client, &host);
    // Apollo alone takes no camera.
    assert_eq!(client.camera_offer(&paired.id).unwrap(), None);

    let companion = companion(&host, &folder, Some(permission::ALL));
    assert_eq!(companion.port(), host.port() + 13);
    assert_eq!(
        client.camera_offer(&paired.id).unwrap(),
        Some(CameraOffer { max_width: 1280, max_height: 720, max_fps: 30 })
    );

    let mut link = client.send_camera(&paired.id, SMALL).unwrap();
    assert!(companion.cameras().report().open);
    assert_eq!(companion.sender().as_deref(), Some("Device 1"));

    let mut encoder = PictureEncoder::new(Sending {
        width: SMALL.width,
        height: SMALL.height,
        fps: SMALL.fps,
        bitrate_kbps: SMALL.bitrate_kbps,
    })
    .unwrap();
    let picture = halves(320, 240, RED, BLUE);
    let mut sent = 0u64;
    for number in 0..20u32 {
        let Some(encoded) = encoder.encode(&picture, false).unwrap() else { continue };
        assert!(link.send(Duration::from_millis(33) * number, encoded.key_frame, encoded.data));
        sent += 1;
        thread::sleep(Duration::from_millis(2));
    }
    wait_for("the camera to arrive", || companion.cameras().report().pictures == sent);

    let seen = companion.cameras().seen().expect("a picture");
    assert_eq!((seen.width, seen.height), (320, 240));
    let at = |column: usize| {
        let start = (120 * seen.width + column) * 4;
        &seen.rgba[start..start + 3]
    };
    assert!(close(at(80), RED), "left of the picture: {:?}", at(80));
    assert!(close(at(240), BLUE), "right of the picture: {:?}", at(240));

    drop(link);
    wait_for("the camera to close", || !companion.cameras().report().open);
    assert_eq!(companion.sender(), None);
}

#[test]
fn only_the_devices_apollo_trusts_with_the_pc_send_a_camera() {
    let (settings, folder) = (Folder::new("settings"), Folder::new("apollo"));
    let host = apollo();
    let client = Client::open(&settings.0).unwrap();
    let paired = pair(&client, &host);

    // What Apollo gives a device paired after the first: list and watch.
    let companion = companion(&host, &folder, Some(permission::DEFAULT));
    assert!(matches!(
        client.send_camera(&paired.id, SMALL),
        Err(Error::Camera(CameraRefusal::NotAllowed))
    ));
    assert!(!companion.cameras().report().open);

    // Its owner ticks the keyboard and the mouse on Apollo's page: the
    // companion reads the file again at the next ask, with no restart.
    let trusted = permission::DEFAULT | permission::KEYBOARD | permission::MOUSE;
    host.write_apollo_folder(&folder.0, Some(trusted)).unwrap();
    let link = client.send_camera(&paired.id, SMALL).unwrap();
    assert!(companion.cameras().report().open);
    drop(link);
}

#[test]
fn a_device_apollo_does_not_know_is_told_nothing() {
    let (settings, other_settings) = (Folder::new("settings"), Folder::new("other"));
    let folder = Folder::new("apollo");
    let host = apollo();
    let client = Client::open(&settings.0).unwrap();
    let paired = pair(&client, &host);

    // The folder as it was before this device paired: Apollo knows nobody.
    host.forget_clients();
    let companion = companion(&host, &folder, None);
    assert_eq!(client.camera_offer(&paired.id).unwrap(), None);
    assert!(client.send_camera(&paired.id, SMALL).is_err());
    assert!(!companion.cameras().report().open);

    // On a Sunshine, which keeps no permissions, a paired device sends.
    let other = Client::open(&other_settings.0).unwrap();
    let paired = pair(&other, &host);
    host.write_apollo_folder(&folder.0, None).unwrap();
    assert!(other.camera_offer(&paired.id).unwrap().is_some());
    let link = other.send_camera(&paired.id, SMALL).unwrap();
    assert!(companion.cameras().report().open);
    drop(link);
}

#[test]
fn a_folder_that_is_not_an_apollo_s_is_refused_with_the_file_that_is_wrong() {
    let folder = Folder::new("apollo");
    fs::create_dir_all(&folder.0).unwrap();
    let missing = Apollo::read(&folder.0).unwrap_err().to_string();
    assert!(missing.contains("cacert.pem"), "{missing}");

    let host = apollo();
    host.write_apollo_folder(&folder.0, None).unwrap();
    fs::write(folder.0.join("sunshine_state.json"), "not what Apollo writes").unwrap();
    let wrong = Apollo::read(&folder.0).unwrap_err().to_string();
    assert!(wrong.contains("sunshine_state.json"), "{wrong}");
}

#[test]
fn a_device_sends_its_microphone_to_a_companion_that_has_someone_to_hand_it_to() {
    let (settings, folder) = (Folder::new("settings"), Folder::new("apollo"));
    let host = apollo();
    let client = Client::open(&settings.0).unwrap();
    let paired = pair(&client, &host);
    let companion = companion(&host, &folder, Some(permission::ALL));

    // Nobody on the PC to hand a microphone to: none is taken.
    assert!(!client.return_offer(&paired.id).unwrap().microphone);
    assert!(matches!(
        client.send_microphone(&paired.id),
        Err(Error::Camera(CameraRefusal::NotTaken))
    ));

    let frames = companion.hear();
    let offer = client.return_offer(&paired.id).unwrap();
    assert!(offer.microphone);
    assert!(offer.camera.is_some());

    // The camera is asked for apart, and goes on beside the microphone.
    let camera = client.send_camera(&paired.id, SMALL).unwrap();
    let mut link = client.send_microphone(&paired.id).unwrap();
    assert!(companion.cameras().report().open);
    assert!(companion.cameras().voice_report().open);
    assert_eq!(companion.speaker().as_deref(), Some("Device 1"));

    for number in 0..10u8 {
        assert!(link.send(Duration::from_millis(20) * u32::from(number), &[number; 40]));
    }
    for number in 0..10u8 {
        let frame = frames.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(frame, vec![number; 40], "frame {number}");
    }
    let report = companion.cameras().voice_report();
    assert_eq!((report.frames, report.refused), (10, 0));
    // None of it was taken for a piece of a picture.
    assert_eq!(companion.cameras().report().packets, 0);

    // The device says that the microphone stopped: the camera stays.
    drop(link);
    wait_for("the microphone to close", || !companion.cameras().voice_report().open);
    assert_eq!(companion.speaker(), None);
    assert!(companion.cameras().report().open);
    drop(camera);
    wait_for("the camera to close", || !companion.cameras().report().open);

    // Whoever listened is gone: the next frame ends the microphone.
    let mut link = client.send_microphone(&paired.id).unwrap();
    drop(frames);
    assert!(link.send(Duration::ZERO, &[1; 40]));
    wait_for("the companion to stop taking a microphone", || {
        !client.return_offer(&paired.id).unwrap().microphone
    });
}

#[test]
fn only_the_devices_apollo_trusts_with_the_pc_send_a_microphone() {
    let (settings, folder) = (Folder::new("settings"), Folder::new("apollo"));
    let host = apollo();
    let client = Client::open(&settings.0).unwrap();
    let paired = pair(&client, &host);
    let companion = companion(&host, &folder, Some(permission::DEFAULT));
    let _frames = companion.hear();
    assert!(matches!(
        client.send_microphone(&paired.id),
        Err(Error::Camera(CameraRefusal::NotAllowed))
    ));
    assert!(!companion.cameras().voice_report().open);
}

/// An interface that keeps the news in a queue and counts what a stream
/// carries.
struct Listener {
    news: Mutex<mpsc::Sender<Event>>,
    counter: Counter,
}

struct Pictures(Box<dyn VideoSink>);
struct Sound(Box<dyn AudioSink>);

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

impl Shell for Listener {
    type Video = Pictures;
    type Audio = Sound;

    fn tell(&self, event: Event) {
        let _ = self.news.lock().unwrap().send(event);
    }

    fn sinks(&self) -> Result<(Self::Video, Self::Audio), String> {
        Ok((Pictures(Box::new(self.counter.video())), Sound(Box::new(self.counter.audio()))))
    }
}

fn news<T>(news: &mpsc::Receiver<Event>, mut wanted: impl FnMut(Event) -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        let left = Duration::from_secs(20).checked_sub(start.elapsed()).expect("no such news");
        if let Some(found) = wanted(news.recv_timeout(left).expect("no such news")) {
            return found;
        }
    }
}

#[test]
fn an_interface_learns_that_the_companion_takes_the_microphone_and_sends_it() {
    let (settings, folder) = (Folder::new("settings"), Folder::new("apollo"));
    let host = apollo();
    let client = Arc::new(Client::open(&settings.0).unwrap());
    let paired = pair(&client, &host);
    let companion = companion(&host, &folder, Some(permission::ALL));
    let frames = companion.hear();

    let (told, heard) = mpsc::channel();
    let shell = Listener { news: Mutex::new(told), counter: Counter::default() };
    let service = Service::new(Arc::clone(&client), shell);
    service.ask(Request::Play {
        host: paired.id.clone(),
        app: 1,
        attempt: 1,
        screen: None,
        choices: Choices::default(),
    });
    // Apollo takes no microphone inside its stream. The companion beside it
    // does, which is told a moment later.
    let inside = news(&heard, |event| match event {
        Event::Playing { attempt: 1, microphone, .. } => Some(microphone),
        _ => None,
    });
    assert!(!inside);
    let beside = news(&heard, |event| match event {
        Event::Microphone { attempt: 1, taken } => Some(taken),
        _ => None,
    });
    assert!(beside);
    // Nothing was asked of the companion yet: the microphone is off.
    assert!(!companion.cameras().voice_report().open);

    // The first frames ask for the way, and the ones after it arrive.
    let mut number = 0u32;
    wait_for("the microphone to arrive", || {
        number += 1;
        service.microphone(Duration::from_millis(20) * number, &[7; 40]);
        frames.try_recv().is_ok_and(|frame| frame == [7; 40])
    });
    assert!(companion.cameras().voice_report().open);

    // It stops with the stream.
    service.ask(Request::Leave);
    wait_for("the microphone to close", || !companion.cameras().voice_report().open);
    assert!(!service.microphone(Duration::ZERO, &[7; 40]));
}
