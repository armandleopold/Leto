//! Two ends of a tunnel on this machine: what goes in one comes out of the
//! other, and a door and a relay lead through it to ordinary sockets.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream, UdpSocket},
    sync::Arc,
    thread,
    time::Duration,
};

use boringtun::x25519::{PublicKey, StaticSecret};
use leto_tunnel::{Config, Door, Range, Relay, Tunnel};

const HOME: Ipv4Addr = Ipv4Addr::new(10, 66, 0, 1);
const LAPTOP: Ipv4Addr = Ipv4Addr::new(10, 66, 0, 2);
const WAIT: Duration = Duration::from_secs(5);

fn public(private: [u8; 32]) -> [u8; 32] {
    PublicKey::from(&StaticSecret::from(private)).to_bytes()
}

/// A home end that waits for the laptop, and the laptop's end, which goes
/// to it. `home_knows` is the key the home expects of the laptop.
fn ends_with(laptop_key: [u8; 32], home_knows: [u8; 32]) -> (Arc<Tunnel>, Arc<Tunnel>) {
    let home_key = [1; 32];
    let home = Tunnel::start(Config {
        private_key: home_key,
        address: HOME,
        mtu: 1420,
        listen_port: None,
        peer_public_key: home_knows,
        preshared_key: None,
        endpoint: None,
        allowed: vec![Range { address: LAPTOP, prefix: 32 }],
        keepalive: None,
    })
    .expect("the home end starts");
    let port = home.local_address().expect("the home end listens").port();
    let laptop = Tunnel::start(Config {
        private_key: laptop_key,
        address: LAPTOP,
        mtu: 1420,
        listen_port: None,
        peer_public_key: public(home_key),
        preshared_key: None,
        endpoint: Some(format!("127.0.0.1:{port}")),
        allowed: vec![Range { address: Ipv4Addr::new(10, 66, 0, 0), prefix: 24 }],
        keepalive: Some(25),
    })
    .expect("the laptop's end starts");
    (Arc::new(home), Arc::new(laptop))
}

fn ends() -> (Arc<Tunnel>, Arc<Tunnel>) {
    let laptop_key = [2; 32];
    ends_with(laptop_key, public(laptop_key))
}

/// A port nothing on this machine listens on now.
fn free_port() -> u16 {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap().port()
}

#[test]
fn datagrams_go_both_ways_and_say_where_they_came_from() {
    let (home, laptop) = ends();
    let at_home = home.bind(Some(7)).unwrap();
    let on_laptop = laptop.bind(None).unwrap();

    on_laptop.send_to(b"ping", SocketAddrV4::new(HOME, 7)).unwrap();
    let (payload, from) = at_home.recv_from(WAIT).expect("the datagram arrives");
    assert_eq!(payload, b"ping");
    assert_eq!(from, SocketAddrV4::new(LAPTOP, on_laptop.port()));

    at_home.send_to(b"pong", from).unwrap();
    let (payload, from) = on_laptop.recv_from(WAIT).expect("the answer arrives");
    assert_eq!((payload.as_slice(), from), (&b"pong"[..], SocketAddrV4::new(HOME, 7)));

    let status = laptop.status();
    assert!(status.handshake.is_some(), "{status:?}");
    assert!(status.sent > 0 && status.received > 0, "{status:?}");
}

#[test]
fn a_connection_carries_much_more_than_a_packet_both_ways() {
    let (home, laptop) = ends();
    let listener = home.listen(80).unwrap();
    let sent: Vec<u8> = (0..600_000u32).map(|n| (n % 251) as u8).collect();

    let echo = thread::spawn(move || {
        let connection = listener.accept(WAIT).expect("a connection arrives");
        let mut received = Vec::new();
        (&connection).read_to_end(&mut received).unwrap();
        (&connection).write_all(&received).unwrap();
        connection.shutdown_write();
        // Until the laptop has it all.
        let mut rest = Vec::new();
        let _ = (&connection).read_to_end(&mut rest);
    });

    let connection = laptop.connect(SocketAddrV4::new(HOME, 80), WAIT).expect("it connects");
    connection.set_read_timeout(Some(WAIT));
    (&connection).write_all(&sent).unwrap();
    connection.shutdown_write();
    let mut back = Vec::new();
    (&connection).read_to_end(&mut back).unwrap();
    assert_eq!(back.len(), sent.len());
    assert!(back == sent, "the bytes came back changed");
    drop(connection);
    echo.join().unwrap();
}

#[test]
fn a_door_leads_ordinary_sockets_through_the_tunnel() {
    let (home, laptop) = ends();
    let (tcp, udp) = (free_port(), free_port());
    let listener = home.listen(tcp).unwrap();
    let datagrams = home.bind(Some(udp)).unwrap();
    thread::spawn(move || {
        let connection = listener.accept(WAIT).expect("a connection arrives");
        let mut word = [0; 5];
        (&connection).read_exact(&mut word).unwrap();
        (&connection).write_all(&word).unwrap();
        let _ = (&connection).read_to_end(&mut Vec::new());
    });
    thread::spawn(move || {
        let (payload, from) = datagrams.recv_from(WAIT).expect("a datagram arrives");
        datagrams.send_to(&payload, from).unwrap();
    });

    let door = Door::open(Arc::clone(&laptop), HOME, &[tcp], &[udp]).unwrap();
    assert_eq!(door.target(), HOME);

    let mut stream = TcpStream::connect((door.address(), tcp)).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    stream.write_all(b"hello").unwrap();
    let mut word = [0; 5];
    stream.read_exact(&mut word).expect("the answer comes back through the door");
    assert_eq!(&word, b"hello");

    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    socket.set_read_timeout(Some(WAIT)).unwrap();
    socket.send_to(b"hi", (door.address(), udp)).unwrap();
    let mut answer = [0; 16];
    let (length, from) = socket.recv_from(&mut answer).expect("the datagram comes back");
    assert_eq!(&answer[..length], b"hi");
    assert_eq!(from, (door.address(), udp).into(), "it answers from the door's own port");
}

#[test]
fn a_relay_leads_from_the_tunnel_to_this_machine() {
    let (home, laptop) = ends();
    let service = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let tcp = service.local_addr().unwrap().port();
    let echo = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let udp = echo.local_addr().unwrap().port();
    thread::spawn(move || {
        let (mut stream, _) = service.accept().unwrap();
        let mut word = [0; 5];
        stream.read_exact(&mut word).unwrap();
        stream.write_all(&word).unwrap();
    });
    thread::spawn(move || {
        let mut buffer = [0; 16];
        let (length, from) = echo.recv_from(&mut buffer).unwrap();
        echo.send_to(&buffer[..length], from).unwrap();
    });
    let _relay = Relay::start(&home, Ipv4Addr::LOCALHOST, &[tcp], &[udp]).unwrap();

    let connection = laptop.connect(SocketAddrV4::new(HOME, tcp), WAIT).expect("it connects");
    connection.set_read_timeout(Some(WAIT));
    (&connection).write_all(b"hello").unwrap();
    let mut word = [0; 5];
    (&connection).read_exact(&mut word).expect("the service answers");
    assert_eq!(&word, b"hello");

    let datagrams = laptop.bind(None).unwrap();
    datagrams.send_to(b"hi", SocketAddrV4::new(HOME, udp)).unwrap();
    let (payload, from) = datagrams.recv_from(WAIT).expect("the service answers");
    assert_eq!((payload.as_slice(), from), (&b"hi"[..], SocketAddrV4::new(HOME, udp)));
}

#[test]
fn a_peer_that_does_not_know_this_key_lets_nothing_through() {
    let (home, laptop) = ends_with([2; 32], public([3; 32]));
    let _listener = home.listen(80).unwrap();
    let refused = laptop.connect(SocketAddrV4::new(HOME, 80), Duration::from_secs(1));
    assert!(refused.is_err());
    assert!(laptop.status().handshake.is_none());
    // Nothing came back: the handshakes are counted, and no refusal is
    // made up.
    let silence = laptop.status().silence;
    assert!(silence.tries >= 1, "{silence:?}");
    assert_eq!((silence.refused, silence.answered_once), (0, false));
}

#[test]
fn a_peer_that_answers_is_not_silent() {
    let (home, laptop) = ends();
    let at_home = home.bind(Some(7)).unwrap();
    laptop.bind(None).unwrap().send_to(b"ping", SocketAddrV4::new(HOME, 7)).unwrap();
    at_home.recv_from(WAIT).expect("the datagram arrives");
    let silence = laptop.status().silence;
    assert!(silence.answered_once);
    assert_eq!((silence.tries, silence.refused), (0, 0));
    assert_eq!(silence.explain("127.0.0.1", None), None);
}

/// The home end answers, but with a preshared key the laptop does not have:
/// the answer is refused, and that is told apart from no answer at all.
#[test]
fn an_answer_with_other_keys_is_told_apart_from_none() {
    let home_key = [1; 32];
    let laptop_key = [2; 32];
    let config = |private_key, address, peer, preshared_key, endpoint| Config {
        private_key,
        address,
        mtu: 1420,
        listen_port: None,
        peer_public_key: public(peer),
        preshared_key,
        endpoint,
        allowed: vec![Range { address: Ipv4Addr::new(10, 66, 0, 0), prefix: 24 }],
        keepalive: None,
    };
    let home = Tunnel::start(config(home_key, HOME, laptop_key, Some([7; 32]), None)).unwrap();
    let port = home.local_address().unwrap().port();
    let endpoint = Some(format!("127.0.0.1:{port}"));
    let laptop =
        Tunnel::start(config(laptop_key, LAPTOP, home_key, Some([8; 32]), endpoint)).unwrap();
    let deadline = std::time::Instant::now() + WAIT;
    while laptop.status().silence.refused == 0 && std::time::Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    let silence = laptop.status().silence;
    assert!(silence.refused >= 1, "{silence:?}");
    assert!(laptop.status().handshake.is_none());
}
