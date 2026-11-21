//! Waking a PC that is off: the client learns the address of its network
//! card when pairing, keeps it, and sends the packet that wakes the card
//! once the PC no longer answers.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    fs,
    net::{Ipv4Addr, UdpSocket},
    path::PathBuf,
    sync::atomic::{AtomicU32, Ordering},
    time::{Duration, Instant},
};

use leto_client::{
    Client, Error, Reachability,
    wake::{magic_packet, parse_card},
};
use leto_core::model::DEFAULT_HOST_PORT;
use leto_testhost::{FakeHost, HOST_CARD};

/// A fresh settings folder per test.
struct Settings(PathBuf);

impl Settings {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name =
            format!("leto-wake-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst));
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

/// Listens where the streaming software of a PC on `base` would: one of the
/// ports the packet goes to on the PC's own address. The first that is free.
fn listen_like_the_pc(base: u16) -> UdpSocket {
    let socket = [47998, 47999, 48000, 48002, 48010]
        .into_iter()
        .filter_map(|port: u16| (port - DEFAULT_HOST_PORT).checked_add(base))
        .find_map(|port| UdpSocket::bind((Ipv4Addr::LOCALHOST, port)).ok())
        .expect("one of the PC's ports is free");
    socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    socket
}

#[test]
fn a_paired_pc_is_woken_at_the_address_it_was_reached_at() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let client = settings.open();
    let added = client.add(&host.address()).unwrap();
    assert!(!added.wakeable, "a PC tells its card only to a device it paired with");
    let pin = client.new_pin().unwrap();
    host.type_pin(pin);
    let paired = client.pair(&added.id, pin).unwrap();
    assert!(paired.wakeable);
    let saved = fs::read_to_string(settings.0.join("client.json")).unwrap();
    assert!(saved.contains(HOST_CARD), "{saved}");

    // The PC goes to sleep, and Leto starts again.
    let base = host.port();
    drop(host);
    let client = settings.open();
    assert!(client.host(&paired.id).unwrap().wakeable);
    let listening = listen_like_the_pc(base);

    client.wake(&paired.id).unwrap();

    let mut received = [0u8; 200];
    let (length, _) = listening.recv_from(&mut received).expect("the packet arrives");
    assert_eq!(received[..length], magic_packet(parse_card(HOST_CARD).unwrap()));
}

#[test]
fn a_pc_never_paired_cannot_be_woken() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let client = settings.open();
    let id = client.add(&host.address()).unwrap().id;
    drop(host);

    assert!(matches!(client.wake(&id), Err(Error::CannotWake)));
    let saved = fs::read_to_string(settings.0.join("client.json")).unwrap();
    assert!(!saved.contains("card"), "zeros are not a card: {saved}");
}

#[test]
fn a_pc_that_answers_is_not_woken_and_one_that_stays_off_says_so() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let client = settings.open();
    let added = client.add(&host.address()).unwrap();
    let pin = client.new_pin().unwrap();
    host.type_pin(pin);
    let id = client.pair(&added.id, pin).unwrap().id;

    let awake = client.wake_up(&id, Duration::from_secs(5)).unwrap();
    assert_eq!(awake.reachability, Reachability::Online);

    drop(host);
    let started = Instant::now();
    let woken = client.wake_up(&id, Duration::from_millis(500));
    assert!(matches!(woken, Err(Error::DidNotWake(_))), "{woken:?}");
    assert!(started.elapsed() < Duration::from_secs(10), "it gave up in time");
    assert_eq!(client.host(&id).unwrap().reachability, Reachability::Offline);
}
