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
use leto_testhost::{COVER_PNG, FakeHost, HOST_ID, HOST_NAME, Options};

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

/// Sunshine and Apollo keep an attempt that never got its PIN, and would go
/// on with its certificate when another device pairs: Armand's phone and
/// laptop were refused that way on 2026-11-21.
#[test]
fn an_attempt_left_without_its_pin_does_not_spoil_the_next_one() {
    let host = FakeHost::start_with(Options {
        pin_patience: Duration::from_millis(300),
        ..Options::default()
    });
    let (first, second) = (Settings::new(), Settings::new());

    let left = first.open();
    let added = left.add(&host.address()).unwrap();
    assert!(left.pair(&added.id, left.new_pin().unwrap()).is_err());

    let client = second.open();
    assert!(pair(&client, &host).paired);
    assert_eq!(host.paired_clients(), 1);
}

#[test]
fn a_wrong_pin_does_not_spoil_the_next_attempt() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let client = settings.open();
    let added = client.add(&host.address()).unwrap();
    host.type_pin(Pin::from_bytes([1, 2, 3, 4]));
    assert!(client.pair(&added.id, Pin::from_bytes([4, 3, 2, 1])).is_err());

    assert!(pair(&client, &host).paired);
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

#[test]
fn a_pc_known_at_two_addresses_is_reached_at_the_nearest() {
    let settings = Settings::new();
    // One PC, two ways there: one far away, as through a VPN, one near.
    let far = FakeHost::start_with(Options {
        answer_after: Duration::from_millis(400),
        ..Options::default()
    });
    let near = FakeHost::start();
    let client = settings.open();

    client.add(&near.address()).unwrap();
    let id = client.add(&far.address()).unwrap().id;
    assert_eq!(client.hosts().len(), 1, "the same PC twice: {:?}", client.hosts());
    assert_eq!(client.host(&id).unwrap().port, far.port());

    let refreshed = client.refresh();
    assert_eq!(refreshed.len(), 1);
    assert_eq!(refreshed[0].port, near.port(), "the nearest address is the one to use");

    // Both are remembered, and the nearest is found again after a restart.
    let client = settings.open();
    assert_eq!(client.refresh()[0].port, near.port());

    // When the near way is gone, the far one still leads to the PC.
    drop(near);
    let refreshed = client.refresh();
    assert_eq!(refreshed[0].reachability, Reachability::Online);
    assert_eq!(refreshed[0].port, far.port());
}

#[test]
fn a_paired_pc_stays_paired_at_each_of_its_addresses() {
    let settings = Settings::new();
    let host = FakeHost::start();
    let client = settings.open();
    let id = pair(&client, &host).id;

    let again = client.add(&format!("localhost:{}", host.port())).unwrap();
    assert_eq!(again.id, id);
    assert!(again.paired, "the PC proved who it is at that address too");
    assert_eq!(client.hosts().len(), 1);

    let client = settings.open();
    let refreshed = client.refresh();
    assert_eq!(refreshed.len(), 1);
    assert!(refreshed[0].paired);
    assert_eq!(client.apps(&id).unwrap().len(), 2);
    let saved = fs::read_to_string(settings.0.join("client.json")).unwrap();
    assert!(saved.contains("other_addresses"), "{saved}");
}

/// A PC that Leto only knows from further away, as through a VPN, is found
/// nearby by its name: this device's system knows where the name leads,
/// and the PC proves there that it is the one Leto paired with. From then
/// on the nearest address is the one used.
#[test]
fn a_pc_known_from_further_away_is_found_nearby_by_its_name() {
    // Another address of this machine stands for the one further away.
    const AFAR: &str = "127.77.0.4";
    assert!(
        std::net::UdpSocket::bind((AFAR, 0)).is_ok(),
        "this machine has no local address {AFAR}: on macOS, add it with \
         `sudo ifconfig lo0 alias {AFAR}`"
    );
    let settings = Settings::new();
    // The one name every system knows where to find.
    let host = FakeHost::start_with(Options {
        name: "localhost".to_owned(),
        interface: "0.0.0.0".to_owned(),
        ..Options::default()
    });
    let client = settings.open();
    let added = client.add(&format!("{AFAR}:{}", host.port())).unwrap();
    let pin = client.new_pin().unwrap();
    host.type_pin(pin);
    let paired = client.pair(&added.id, pin).unwrap();
    assert_eq!(paired.address, AFAR);

    // Nothing announces itself here, and where that cannot even be listened
    // for the names are asked all the same.
    let _ = client.discover(Duration::from_secs(2));

    let found = client.hosts();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].address, "127.0.0.1");
    assert!(found[0].paired);
    // Both are kept, and the PC is still reached when one is gone.
    let saved = fs::read_to_string(settings.0.join("client.json")).unwrap();
    assert!(saved.contains(AFAR) && saved.contains("127.0.0.1"), "{saved}");
    assert_eq!(client.apps(&paired.id).unwrap().len(), 2);
}

/// A PC that Leto only knows from further away, as through a VPN, and
/// whose name leads nowhere, is found by asking the addresses around this
/// device: what takes a connection there is asked who it is, and the PC
/// proves that it is the one Leto paired with.
#[test]
fn a_pc_known_from_further_away_is_found_among_the_addresses_around() {
    use leto_client::discovery::Around;

    // Another address of this machine stands for the one further away.
    const AFAR: &str = "127.77.0.4";
    assert!(
        std::net::UdpSocket::bind((AFAR, 0)).is_ok(),
        "this machine has no local address {AFAR}: on macOS, add it with \
         `sudo ifconfig lo0 alias {AFAR}`"
    );
    let settings = Settings::new();
    let host = FakeHost::start_on("0.0.0.0");
    let client = settings.open();
    let added = client.add(&format!("{AFAR}:{}", host.port())).unwrap();

    // A network of four around this machine, with one address for another
    // device: 127.0.0.1, where the PC also listens.
    let around = [Around::new("127.0.0.2".parse().unwrap(), 30).unwrap()];

    // A PC that is not paired is not looked for.
    client.look_around_on(&around);
    assert_eq!(client.host(&added.id).unwrap().address, AFAR);

    let pin = client.new_pin().unwrap();
    host.type_pin(pin);
    let paired = client.pair(&added.id, pin).unwrap();
    assert_eq!(paired.address, AFAR);

    client.look_around_on(&around);
    let found = client.hosts();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].address, "127.0.0.1");
    assert!(found[0].paired);
    // Both are kept, and the PC is still reached at either.
    let saved = fs::read_to_string(settings.0.join("client.json")).unwrap();
    assert!(saved.contains(AFAR) && saved.contains("127.0.0.1"), "{saved}");
    assert_eq!(client.apps(&paired.id).unwrap().len(), 2);
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
