//! The camera of a device on its way to a stand-in PC that takes one, as
//! Leto's own host will: asked for over the PC's own API, sent sealed, and
//! seen on the other side.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU32, Ordering},
    thread,
    time::{Duration, Instant},
};

use leto_client::{CameraRefusal, CameraSettings, Client, Error, HostSummary, engine::CameraOffer};
use leto_media::{Picture, PictureEncoder, Sending};
use leto_testhost::{FakeHost, Options};

/// A small picture, so the tests spend their time on the way there.
const SMALL: CameraSettings =
    CameraSettings { width: 320, height: 240, fps: 30, bitrate_kbps: 500 };

const RED: [u8; 3] = [200, 40, 40];
const BLUE: [u8; 3] = [40, 60, 200];

/// A fresh settings folder per test.
struct Settings(PathBuf);

impl Settings {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name = format!(
            "leto-camera-test-{}-{}",
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
fn a_pc_says_what_camera_it_takes_and_most_take_none() {
    let settings = Settings::new();
    let client = settings.open();

    let host = FakeHost::start_with(Options { camera: Some((1280, 720)), ..Options::default() });
    let paired = pair(&client, &host);
    assert_eq!(
        client.camera_offer(&paired.id).unwrap(),
        Some(CameraOffer { max_width: 1280, max_height: 720, max_fps: 30 })
    );

    // Every Sunshine and Apollo: nothing answers where Leto's API would be.
    let stock = FakeHost::start_with(Options {
        camera: None,
        id: "0b1e7a52-3c1d-4f0a-9d6e-5a2f8c4b7e10".to_owned(),
        ..Options::default()
    });
    let paired = pair(&client, &stock);
    assert_eq!(client.camera_offer(&paired.id).unwrap(), None);
    assert!(client.send_camera(&paired.id, SMALL).is_err());
}

#[test]
fn the_pictures_of_a_camera_arrive_and_show_what_it_saw() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let client = settings.open();
    let paired = pair(&client, &host);
    assert!(!host.camera_report().open, "nothing is taken until asked");

    let mut link = client.send_camera(&paired.id, SMALL).unwrap();
    let report = host.camera_report();
    assert!(report.open);
    assert_eq!(report.asked, Some((320, 240, 30)));

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
        let at = Duration::from_millis(33) * number;
        assert!(link.send(at, encoded.key_frame, encoded.data));
        sent += 1;
        thread::sleep(Duration::from_millis(2));
    }
    assert!(sent >= 10, "the encoder left out most pictures");
    wait_for("the camera to arrive", || host.camera_report().pictures == sent);

    let report = host.camera_report();
    assert_eq!((report.lost, report.refused), (0, 0));
    assert!(report.key_frames >= 1);
    assert_eq!(report.size, Some((320, 240)));
    let seen = host.seen().expect("the host keeps the last picture");
    let pixel = |column: usize, row: usize| {
        let at = (row * seen.width + column) * 4;
        &seen.rgba[at..at + 3]
    };
    assert!(close(pixel(80, 120), RED), "left: {:?}", pixel(80, 120));
    assert!(close(pixel(240, 120), BLUE), "right: {:?}", pixel(240, 120));

    // The PC is told when the camera stops, and shows it no longer.
    link.close();
    assert!(!host.camera_report().open);
    assert!(host.seen().is_none());
}

#[test]
fn a_camera_that_is_dropped_tells_the_pc_too() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let client = settings.open();
    let paired = pair(&client, &host);

    let link = client.send_camera(&paired.id, SMALL).unwrap();
    assert!(host.camera_report().open);
    drop(link);
    wait_for("the PC to learn that the camera stopped", || !host.camera_report().open);
}

#[test]
fn a_camera_turned_off_and_on_again_at_once_stays_on() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let client = settings.open();
    let paired = pair(&client, &host);

    // Asking again replaces what was agreed, and the goodbye of the first
    // camera arrives after the ask of the second: it must not end that one.
    let first = client.send_camera(&paired.id, SMALL).unwrap();
    let mut second = client.send_camera(&paired.id, SMALL).unwrap();
    first.close();
    assert!(host.camera_report().open, "a late goodbye ended the camera that followed");
    assert!(second.send(Duration::ZERO, true, b"something"));
    wait_for("the camera to arrive", || host.camera_report().packets == 1);
    assert_eq!(host.camera_report().refused, 0);

    second.close();
    assert!(!host.camera_report().open);
}

#[test]
fn a_pc_refuses_what_its_owner_does_not_allow_and_what_it_cannot_take() {
    let settings = Settings::new();
    let host = FakeHost::start_with(Options { camera: Some((1280, 720)), ..Options::default() });
    let client = settings.open();
    let paired = pair(&client, &host);

    host.refuse_camera(true);
    let refused = client.send_camera(&paired.id, SMALL);
    assert!(matches!(refused, Err(Error::Camera(CameraRefusal::NotAllowed))));
    assert!(!host.camera_report().open);
    host.refuse_camera(false);

    let large = CameraSettings { width: 1920, height: 1080, ..SMALL };
    let refused = client.send_camera(&paired.id, large);
    assert!(matches!(refused, Err(Error::Camera(CameraRefusal::Unsuitable))));

    assert!(client.send_camera(&paired.id, SMALL).is_ok());
}

#[test]
fn one_device_sends_at_a_time() {
    let (first_settings, second_settings) = (Settings::new(), Settings::new());
    let host = FakeHost::start();
    let (first, second) = (first_settings.open(), second_settings.open());
    let (paired, also_paired) = (pair(&first, &host), pair(&second, &host));

    let link = first.send_camera(&paired.id, SMALL).unwrap();
    let refused = second.send_camera(&also_paired.id, SMALL);
    assert!(matches!(refused, Err(Error::Camera(CameraRefusal::Busy))));

    link.close();
    assert!(second.send_camera(&also_paired.id, SMALL).is_ok());
}

#[test]
fn only_a_paired_device_asks_and_only_the_pc_it_knows_is_asked() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let client = settings.open();

    // Not paired yet: there is no certificate to recognise the PC by.
    let added = client.add(&host.address()).unwrap();
    assert!(matches!(client.camera_offer(&added.id), Err(Error::NotPaired)));
    assert!(matches!(client.send_camera(&added.id, SMALL), Err(Error::NotPaired)));

    let paired = pair(&client, &host);
    assert!(client.camera_offer(&paired.id).unwrap().is_some());

    // A PC that forgot this device answers it nothing.
    host.forget_clients();
    assert_eq!(client.camera_offer(&paired.id).unwrap(), None);
    assert!(matches!(client.send_camera(&paired.id, SMALL), Err(Error::NotPaired)));

    // Something else that answers at the address of the PC is told nothing:
    // not even what this device would like to send.
    let pin = client.new_pin().unwrap();
    host.type_pin(pin);
    client.pair(&paired.id, pin).unwrap();
    assert!(client.camera_offer(&paired.id).unwrap().is_some());
    host.become_impostor();
    assert_eq!(client.camera_offer(&paired.id).unwrap(), None);
    assert!(client.send_camera(&paired.id, SMALL).is_err());
    assert!(!host.camera_report().open);
}
