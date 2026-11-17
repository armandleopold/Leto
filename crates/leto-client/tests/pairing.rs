//! The client against a stand-in host: adding, pairing, listing apps, and
//! what happens when the host is not who it claims to be.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU32, Ordering},
    thread,
    time::Duration,
};

use leto_client::{Client, Error, HostKind, Pin, Reachability};
use leto_testhost::{COVER_PNG, FakeHost, HOST_ID, HOST_NAME};

/// A fresh settings folder per test.
struct Settings(PathBuf);

impl Settings {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name =
            format!("leto-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst));
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

/// Adds the host and pairs with it, typing the right PIN on the host.
fn pair(client: &Client, host: &FakeHost) -> leto_client::HostSummary {
    let added = client.add(&host.address()).unwrap();
    let pin = client.new_pin().unwrap();
    host.type_pin(pin);
    client.pair(&added.id, pin).unwrap()
}

#[test]
fn a_host_added_by_address_shows_who_it_is() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let client = settings.open();

    let added = client.add(&host.address()).unwrap();

    assert_eq!(added.name, HOST_NAME);
    assert_eq!(added.id.0, HOST_ID);
    assert_eq!(added.kind, HostKind::Sunshine);
    assert_eq!(added.reachability, Reachability::Online);
    assert!(!added.paired);
    assert!(!added.busy);
    assert_eq!(client.hosts(), vec![added]);
}

#[test]
fn pairing_with_the_right_pin_gives_access_to_the_apps() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let client = settings.open();

    let paired = pair(&client, &host);

    assert!(paired.paired);
    assert_eq!(host.paired_clients(), 1);

    let apps = client.apps(&paired.id).unwrap();
    let titles: Vec<_> = apps.iter().map(|app| (app.title.as_str(), app.hdr)).collect();
    assert_eq!(titles, vec![("Desktop", true), ("Steam Big Picture", false)]);
    assert_eq!(client.app_image(&paired.id, apps[0].id).unwrap(), COVER_PNG);
}

#[test]
fn the_pin_can_be_typed_while_the_client_waits() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let client = settings.open();
    let added = client.add(&host.address()).unwrap();
    let pin = client.new_pin().unwrap();

    thread::scope(|scope| {
        scope.spawn(|| {
            thread::sleep(Duration::from_millis(300));
            host.type_pin(pin);
        });
        assert!(client.pair(&added.id, pin).unwrap().paired);
    });
}

#[test]
fn a_wrong_pin_is_reported_and_pairs_nothing() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let client = settings.open();
    let added = client.add(&host.address()).unwrap();

    host.type_pin(Pin::from_bytes([1, 2, 3, 4]));
    let result = client.pair(&added.id, Pin::from_bytes([4, 3, 2, 1]));

    assert!(matches!(result, Err(Error::WrongPin)), "got {result:?}");
    assert_eq!(host.paired_clients(), 0);
    assert!(!client.host(&added.id).unwrap().paired);
    assert!(matches!(client.apps(&added.id), Err(Error::NotPaired)));
}

#[test]
fn the_apps_of_an_unpaired_host_are_out_of_reach() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let client = settings.open();
    let added = client.add(&host.address()).unwrap();

    assert!(matches!(client.apps(&added.id), Err(Error::NotPaired)));
}

#[test]
fn a_pairing_survives_a_restart_of_the_client() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let id = pair(&settings.open(), &host).id;

    let client = settings.open();
    let remembered = client.host(&id).unwrap();
    assert!(remembered.paired);
    assert_eq!(remembered.reachability, Reachability::Unknown);

    let refreshed = client.refresh();
    assert_eq!(refreshed.len(), 1);
    assert_eq!(refreshed[0].reachability, Reachability::Online);
    assert!(refreshed[0].paired);
    assert_eq!(client.apps(&id).unwrap().len(), 2);
    // The same device identity was reused: the host still knows one client.
    assert_eq!(host.paired_clients(), 1);
}

#[test]
fn an_impostor_with_another_certificate_learns_nothing() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let client = settings.open();
    let id = pair(&client, &host).id;

    // Same address, same name, same id, but not the key of the paired host.
    // It even "knows" the client's certificate, as a real attacker could.
    host.become_impostor();

    let refreshed = client.refresh();
    assert_eq!(refreshed.len(), 1);
    assert!(!refreshed[0].paired, "the client must not trust a host with another certificate");
    assert!(client.apps(&id).is_err());
    assert!(client.app_image(&id, 1).is_err());
}

#[test]
fn a_host_that_forgot_this_device_shows_as_unpaired() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let client = settings.open();
    let id = pair(&client, &host).id;

    host.forget_clients();

    let refreshed = client.refresh();
    assert!(!refreshed[0].paired);
    assert_eq!(refreshed[0].reachability, Reachability::Online);
    assert!(matches!(client.apps(&id), Err(Error::NotPaired)));
}

#[test]
fn a_saved_host_that_is_switched_off_shows_as_offline() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let client = settings.open();
    let id = pair(&client, &host).id;

    drop(host);

    let refreshed = client.refresh();
    assert_eq!(refreshed.len(), 1);
    assert_eq!(refreshed[0].reachability, Reachability::Offline);
    // Still paired as far as the client knows: it comes back when the PC does.
    assert!(refreshed[0].paired);
    assert!(matches!(client.apps(&id), Err(Error::Offline)));
}

#[test]
fn adding_an_address_nobody_answers_on_fails_cleanly() {
    let settings = Settings::new();
    let client = settings.open();

    // Port 9 on the loopback interface: nothing listens there.
    assert!(matches!(client.add("127.0.0.1:9"), Err(Error::Offline)));
    assert!(matches!(client.add("not an address"), Err(Error::BadAddress(_))));
    assert!(client.hosts().is_empty());
}

#[test]
fn forgetting_a_host_removes_it_on_both_sides() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let client = settings.open();
    let id = pair(&client, &host).id;

    client.forget(&id).unwrap();

    assert!(client.hosts().is_empty());
    assert!(settings.open().hosts().is_empty());
    assert_eq!(host.paired_clients(), 0);
}

/// The address this machine uses on its network, if it has one.
fn network_address() -> Option<std::net::IpAddr> {
    // Connecting a UDP socket sends nothing. It only picks the outgoing route.
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    socket.local_addr().ok().map(|address| address.ip()).filter(|ip| !ip.is_loopback())
}

/// Needs multicast on the local network, which build machines often lack.
/// Run it by hand: `cargo test -p leto-client -- --ignored`.
#[test]
#[ignore = "needs multicast networking"]
fn a_host_announced_on_the_network_is_found() {
    let address = network_address().expect("this machine is on a network");
    let settings = Settings::new();
    let host = FakeHost::start_on("0.0.0.0");
    let client = settings.open();

    let daemon = mdns_sd::ServiceDaemon::new().unwrap();
    let service = mdns_sd::ServiceInfo::new(
        "_nvstream._tcp.local.",
        "Salon",
        "salon.local.",
        address,
        host.port(),
        None,
    )
    .unwrap();
    daemon.register(service).unwrap();

    let found = client.discover(Duration::from_secs(3)).unwrap();
    let _ = daemon.shutdown();

    assert_eq!(found.len(), 1, "found {found:?}");
    assert_eq!(found[0].name, HOST_NAME);
    assert_eq!(found[0].address, address.to_string());
    assert_eq!(client.hosts(), found);
}
