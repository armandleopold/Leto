//! Leto asks Apollo's own pages to change what it never writes itself: the
//! name of a device, what it may do, whether it stays paired.
//!
//! The Apollo here is the stand-in host, with a stand-in for its pages on
//! the port after its own.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU32, Ordering},
};

use leto_client::Client;
use leto_host::{
    apollo::{Apollo, EVERY_RIGHT, RIGHTS},
    pages::{Error, Pages},
};
use leto_testhost::{ApolloPages, FakeHost, Options, permission};

/// A fresh folder per test, gone with it.
struct Folder(PathBuf);

impl Folder {
    fn new(what: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name = format!(
            "leto-pages-{what}-{}-{}",
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

/// An Apollo paired with `devices` devices, each with what Apollo gives a
/// new one, its folder, and its pages: any port that has the next one free.
fn apollo(devices: usize) -> (FakeHost, Folder, ApolloPages, Vec<Folder>) {
    loop {
        let host =
            FakeHost::start_with(Options { camera: None, microphone: false, ..Options::default() });
        let settings: Vec<Folder> = (0..devices).map(|_| Folder::new("device")).collect();
        for settings in &settings {
            let client = Client::open(&settings.0).unwrap();
            let added = client.add(&host.address()).unwrap();
            let pin = client.new_pin().unwrap();
            host.type_pin(pin);
            client.pair(&added.id, pin).unwrap();
        }
        let folder = Folder::new("apollo");
        host.write_apollo_folder(&folder.0, Some(permission::DEFAULT)).unwrap();
        // The next port may be another test's: another host, then.
        if let Ok(pages) = ApolloPages::start(&folder.0, "owner", "secret") {
            return (host, folder, pages, settings);
        }
    }
}

#[test]
fn only_the_name_and_the_password_of_the_pages_sign_in() {
    let (_host, folder, pages, _devices) = apollo(0);
    let apollo = Apollo::read(&folder.0).unwrap();
    assert_eq!(apollo.pages_port(), pages.port());

    assert!(matches!(Pages::sign_in(&apollo, "owner", "wrong"), Err(Error::Refused)));
    assert!(matches!(Pages::sign_in(&apollo, "someone", "secret"), Err(Error::Refused)));
    assert_eq!(pages.signed_in(), 0);

    // Apollo does not mind the case of the name.
    let signed_in = Pages::sign_in(&apollo, "Owner", "secret").unwrap();
    assert_eq!(pages.signed_in(), 1);
    assert!(signed_in.clients().unwrap().is_empty());
    // Nothing that was given at sign-in shows when the pages are printed.
    assert!(!format!("{signed_in:?}").contains("stand-in-pass"));
}

#[test]
fn pages_that_show_another_certificate_are_not_spoken_to() {
    let (_host, folder, _pages, _devices) = apollo(0);
    // Another Apollo's folder, with the port of this one: its certificate
    // is not the one the pages show.
    let (other_host, other_folder, other_pages, _) = apollo(0);
    drop(other_pages);
    let port = Apollo::read(&folder.0).unwrap().port();
    fs::write(other_folder.0.join("sunshine.conf"), format!("port = {port}\n")).unwrap();
    let other = Apollo::read(&other_folder.0).unwrap();

    assert!(matches!(Pages::sign_in(&other, "owner", "secret"), Err(Error::Unreachable(_))));
    drop(other_host);
}

#[test]
fn nothing_answers_where_no_pages_are() {
    let (_host, folder, pages, _devices) = apollo(0);
    let apollo = Apollo::read(&folder.0).unwrap();
    drop(pages);
    assert!(matches!(Pages::sign_in(&apollo, "owner", "secret"), Err(Error::Unreachable(_))));
}

#[test]
fn what_a_device_may_do_is_changed_by_apollo_and_read_back_from_its_file() {
    let (_host, folder, _pages, _devices) = apollo(2);
    let apollo = Apollo::read(&folder.0).unwrap();
    let pages = Pages::sign_in(&apollo, "owner", "secret").unwrap();

    let mut clients = pages.clients().unwrap();
    assert_eq!(clients.len(), 2);
    assert!(clients.iter().all(|client| client.permissions == permission::DEFAULT));
    // The same devices as the file names, known by the same words.
    let in_file = apollo.devices().unwrap();
    assert_eq!(in_file.len(), 2);
    for (client, device) in clients.iter().zip(&in_file) {
        assert_eq!((&client.id, &client.name), (&device.id, &device.name));
        assert!(!device.trusted());
    }

    // The first one gets the keyboard and the mouse, and another name.
    let [(_, _), (_, _), (_, input)] = RIGHTS;
    let wanted = permission::DEFAULT | permission::MOUSE | permission::KEYBOARD;
    assert_eq!(
        input
            .iter()
            .filter(|right| right.held_by(wanted))
            .map(|right| right.name)
            .collect::<Vec<_>>(),
        ["Mouse", "Keyboard"]
    );
    clients[0].permissions = wanted;
    clients[0].name = "Laptop".to_owned();
    pages.update(&clients[0]).unwrap();

    let after = pages.clients().unwrap();
    assert_eq!((after[0].name.as_str(), after[0].permissions), ("Laptop", wanted));
    assert_eq!(after[1], clients[1]);
    // Apollo wrote its own file: the companion reads the change there.
    let in_file = apollo.devices().unwrap();
    assert_eq!(in_file[0].name, "Laptop");
    assert!(in_file[0].trusted() && !in_file[1].trusted());

    // More than Apollo has a word for is not kept.
    clients[1].permissions = u32::MAX;
    pages.update(&clients[1]).unwrap();
    assert_eq!(pages.clients().unwrap()[1].permissions, EVERY_RIGHT);

    // A device Apollo does not know is not changed, and that is said.
    let mut unknown = clients[0].clone();
    unknown.id = "no-such-device".to_owned();
    assert!(matches!(pages.update(&unknown), Err(Error::Unexpected(_))));
}

#[test]
fn a_device_is_forgotten_or_its_stream_ended_from_the_pages() {
    let (_host, folder, stand_in, _devices) = apollo(2);
    let apollo = Apollo::read(&folder.0).unwrap();
    let pages = Pages::sign_in(&apollo, "owner", "secret").unwrap();
    let clients = pages.clients().unwrap();
    assert!(clients.iter().all(|client| !client.connected));

    // The second one streams, and is told to stop.
    stand_in.connect(&clients[1].id);
    let listed = pages.clients().unwrap();
    assert!(!listed[0].connected && listed[1].connected);
    pages.disconnect(&clients[1].id).unwrap();
    assert_eq!(stand_in.disconnected(), [clients[1].id.clone()]);
    assert!(pages.clients().unwrap().iter().all(|client| !client.connected));
    // One that does not stream has nothing to end.
    assert!(pages.disconnect(&clients[0].id).is_err());

    pages.unpair(&clients[0].id).unwrap();
    let left = pages.clients().unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, clients[1].id);
    assert_eq!(apollo.devices().unwrap().len(), 1);
    assert!(pages.unpair(&clients[0].id).is_err());
}

#[test]
fn a_pin_typed_in_leto_reaches_apollo_with_a_name_for_the_device() {
    let (_host, folder, stand_in, _devices) = apollo(0);
    let apollo = Apollo::read(&folder.0).unwrap();
    let pages = Pages::sign_in(&apollo, "owner", "secret").unwrap();

    // No device shows this PIN.
    assert!(!pages.pair("0000", "Nobody").unwrap());
    stand_in.await_pin("4821");
    assert!(pages.pair("4821", "Tablet").unwrap());
    assert_eq!(
        stand_in.typed(),
        [("0000".to_owned(), "Nobody".to_owned()), ("4821".to_owned(), "Tablet".to_owned())]
    );
}

#[test]
fn a_sign_in_taken_over_elsewhere_asks_to_sign_in_again() {
    let (_host, folder, stand_in, _devices) = apollo(1);
    let apollo = Apollo::read(&folder.0).unwrap();
    let pages = Pages::sign_in(&apollo, "owner", "secret").unwrap();
    assert_eq!(pages.clients().unwrap().len(), 1);

    // Someone signs in from a browser: Apollo keeps one sign-in.
    let browser = Pages::sign_in(&apollo, "owner", "secret").unwrap();
    assert!(matches!(pages.clients(), Err(Error::SignedOut)));
    assert_eq!(browser.clients().unwrap().len(), 1);

    stand_in.sign_out();
    assert!(matches!(browser.clients(), Err(Error::SignedOut)));
    assert!(matches!(browser.unpair("anything"), Err(Error::SignedOut)));
}
