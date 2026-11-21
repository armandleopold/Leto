//! A PC that is only reachable through WireGuard: Leto opens the tunnel
//! itself, from a WireGuard file, and finds, pairs with and streams from the
//! PC through it.
//!
//! The other end of the tunnel is in this process too: an end that waits,
//! as a WireGuard server at home does, and a relay from it to a stand-in
//! host that listens on this machine only, at the ports of a real one.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    fs,
    net::{Ipv4Addr, TcpListener},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use leto_client::{
    Client, Counter, Error, StreamSettings,
    engine::{Codec, Codecs},
};
use leto_testhost::{FakeHost, Options};
use leto_tunnel::{Config, Relay, Tunnel};

/// Addresses kept for examples, which no network leads to: the PC is there
/// for Leto, and only the tunnel knows the way.
const PC: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 1);
const LAPTOP: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 2);

/// Thirty-two bytes of 1, and of 2, as WireGuard files write keys.
const HOME_KEY: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";
const LAPTOP_KEY: &str = "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI=";

const SMALL: StreamSettings = StreamSettings {
    width: 320,
    height: 180,
    fps: 30,
    bitrate_kbps: 2000,
    virtual_display: None,
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
            "leto-tunnel-test-{}-{}",
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

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < Duration::from_secs(20), "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

/// A port with room around it for the others of a host: five below, twenty
/// one above.
fn free_port() -> u16 {
    loop {
        let port = TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
        let around = [port - 5, port + 13, port + 21];
        if port < 65_000
            && around.iter().all(|port| TcpListener::bind(("127.0.0.1", *port)).is_ok())
        {
            return port;
        }
    }
}

fn public_key(private: &str) -> String {
    let file = format!(
        "[Interface]\nPrivateKey = {private}\nAddress = {PC}\n\
         [Peer]\nPublicKey = {HOME_KEY}\nAllowedIPs = {LAPTOP}/32\n"
    );
    leto_tunnel::public_key(Config::parse(&file).unwrap().private_key)
}

/// A home with a PC behind WireGuard: the end of the tunnel that waits, and
/// what it lets through to the PC.
struct Home {
    host: FakeHost,
    port: u16,
    tunnel: Arc<Tunnel>,
    _relay: Relay,
}

impl Home {
    fn new() -> Self {
        let port = free_port();
        let host = FakeHost::start_with(Options { port, real_ports: true, ..Options::default() });
        let file = format!(
            "[Interface]\nPrivateKey = {HOME_KEY}\nAddress = {PC}/24\n\n\
             [Peer]\nPublicKey = {}\nAllowedIPs = {LAPTOP}/32\n",
            public_key(LAPTOP_KEY)
        );
        let tunnel = Arc::new(Tunnel::start(Config::parse(&file).unwrap()).unwrap());
        // What a firewall at home would let through to the PC.
        let tcp = [port, port - 5, port + 13, port + 21];
        let udp = [port + 9, port + 10, port + 11, port + 13];
        let relay = Relay::start(&tunnel, Ipv4Addr::LOCALHOST, &tcp, &udp).unwrap();
        Self { host, port, tunnel, _relay: relay }
    }

    /// The WireGuard file the owner of this home hands to the laptop.
    fn file(&self) -> String {
        let endpoint = self.tunnel.local_address().unwrap().port();
        format!(
            "[Interface]\nPrivateKey = {LAPTOP_KEY}\nAddress = {LAPTOP}/24\nDNS = 192.0.2.1\n\n\
             [Peer]\nPublicKey = {}\nAllowedIPs = 192.0.2.0/24\n\
             Endpoint = 127.0.0.1:{endpoint}\nPersistentKeepalive = 25\n",
            public_key(HOME_KEY)
        )
    }

    fn address(&self) -> String {
        format!("{PC}:{}", self.port)
    }
}

#[test]
fn a_pc_behind_wireguard_is_found_paired_and_streamed_through_leto_s_tunnel() {
    let settings = Settings::new();
    let home = Home::new();
    let client = Client::open(&settings.0).unwrap();

    // Without the tunnel nothing leads to the PC.
    assert!(client.tunnel().is_none());
    assert!(client.add(&home.address()).is_err());

    let tunnel = client.use_tunnel(&home.file()).unwrap();
    assert_eq!(tunnel.address, LAPTOP.to_string());
    assert_eq!(tunnel.covers, ["192.0.2.0/24"]);
    assert_eq!(tunnel.public_key, public_key(LAPTOP_KEY));
    assert!(tunnel.running && tunnel.problem.is_none(), "{tunnel:?}");

    // The PC is known by its address at home, and reached through the
    // tunnel.
    let added = client.add(&home.address()).unwrap();
    assert_eq!(added.address, PC.to_string());
    assert!(added.tunnel);
    let pin = client.new_pin().unwrap();
    home.host.type_pin(pin);
    let paired = client.pair(&added.id, pin).unwrap();
    assert!(paired.paired && paired.tunnel);
    assert!(client.tunnel().unwrap().handshake.is_some());
    assert_eq!(client.apps(&paired.id).unwrap().len(), 2);

    // Pictures and sound come through it, and what the person does goes
    // back.
    let counter = Counter::default();
    let session =
        client.play(&paired.id, 1, SMALL, counter.video(), counter.audio(), |_| {}).unwrap();
    wait_for("pictures and sound", || {
        let tally = counter.tally();
        tally.frames >= 15 && tally.audio_packets >= 40
    });
    let report = home.host.stream_report().expect("the host is streaming");
    assert!(report.connected);
    assert_eq!((report.width, report.height), (320, 180));
    session.stop();
    wait_for("the stream to wind down", || counter.tally().stopped);

    // The tunnel is kept: a Leto that starts again still has it, and
    // still knows the PC it paired with through it.
    drop(client);
    let client = Client::open(&settings.0).unwrap();
    let kept = client.tunnel().expect("the tunnel is kept");
    assert_eq!(kept.address, LAPTOP.to_string());
    let again = client.add(&home.address()).unwrap();
    assert!(again.tunnel && again.paired, "{again:?}");

    // Removed, nothing leads to the PC's address at home any more.
    client.remove_tunnel().unwrap();
    assert!(client.tunnel().is_none());
    assert!(!settings.0.join("wireguard.conf").exists());
    assert!(client.add(&home.address()).is_err());
}

#[test]
fn a_file_that_is_no_wireguard_file_is_refused_and_changes_nothing() {
    let settings = Settings::new();
    let client = Client::open(&settings.0).unwrap();
    let refused = client.use_tunnel("[Interface]\nAddress = 10.8.0.2/24\n");
    assert!(matches!(refused, Err(Error::Tunnel(_))), "{refused:?}");
    assert!(client.tunnel().is_none());
    assert!(!settings.0.join("wireguard.conf").exists());
}
