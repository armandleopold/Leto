//! Waking a PC that sleeps or is off, with the "magic packet" its network
//! card listens for even then (Wake-on-LAN).
//!
//! A sleeping PC answers nothing, so the packet goes everywhere it could
//! land: to every address the PC was reached at, and to every local network
//! this device is on, as a broadcast, in case the router has forgotten which
//! card has the address. Each on the ports that are open on the way: the
//! usual 9, the 47009 that Moonlight's Internet Hosting Tool opens, and the
//! ports of the streaming software itself, which a router may forward. The
//! addresses and the ports are the ones Moonlight uses, in moonlight-qt's
//! `NvComputer::wake` (GPL-3.0, as Leto).

use std::{
    collections::BTreeSet,
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6, ToSocketAddrs, UdpSocket},
};

use leto_core::model::DEFAULT_HOST_PORT;

/// The address of a network card: what the packet asks to wake.
pub type Card = [u8; 6];

/// Ports the packet goes to on every address, whatever the PC's own.
const FIXED_PORTS: [u16; 2] = [9, 47009];
/// Ports of the streaming software, for a PC on [`DEFAULT_HOST_PORT`]. A
/// PC on another port has them moved by as much.
const STREAMING_PORTS: [u16; 5] = [47998, 47999, 48000, 48002, 48010];

/// What a network card wakes up for: six bytes of 0xFF, then its own address
/// sixteen times.
pub fn magic_packet(card: Card) -> [u8; 102] {
    let mut packet = [0xFF; 102];
    for copy in packet[6..].chunks_exact_mut(6) {
        copy.copy_from_slice(&card);
    }
    packet
}

/// Reads a card's address as hosts write it, `00:11:22:33:44:55`, dashes
/// allowed. The zero address, which hosts give devices they do not trust,
/// is none.
pub fn parse_card(text: &str) -> Option<Card> {
    let mut card = [0u8; 6];
    let mut parts = text.trim().split([':', '-']);
    for byte in &mut card {
        let part = parts.next()?;
        if part.len() != 2 {
            return None;
        }
        *byte = u8::from_str_radix(part, 16).ok()?;
    }
    (parts.next().is_none() && card != [0; 6]).then_some(card)
}

/// Writes a card's address as hosts do.
pub fn card_text(card: Card) -> String {
    card.iter().map(|byte| format!("{byte:02X}")).collect::<Vec<_>>().join(":")
}

/// Where the packet goes, for a PC reached at `known`, from a device whose
/// networks have the broadcast addresses `broadcasts` and the IPv6 scopes
/// `scopes`.
pub fn targets(
    known: &[(IpAddr, u16)],
    broadcasts: &[Ipv4Addr],
    scopes: &[u32],
) -> Vec<SocketAddr> {
    let mut targets = BTreeSet::new();
    let ports_from = |base: u16| {
        let moved = STREAMING_PORTS
            .into_iter()
            .filter_map(move |port| (port - DEFAULT_HOST_PORT).checked_add(base));
        FIXED_PORTS.into_iter().chain(moved)
    };
    for (address, base) in known {
        targets.extend(ports_from(*base).map(|port| SocketAddr::new(*address, port)));
    }

    // Nobody knows at which port a broadcast is awaited: at those of every
    // port the PC was reached at.
    let mut bases: BTreeSet<u16> = known.iter().map(|(_, base)| *base).collect();
    if bases.is_empty() {
        bases.insert(DEFAULT_HOST_PORT);
    }
    let everyone = std::iter::once(IpAddr::V4(Ipv4Addr::BROADCAST))
        .chain(broadcasts.iter().map(|broadcast| IpAddr::V4(*broadcast)));
    for address in everyone {
        for base in &bases {
            targets.extend(ports_from(*base).map(|port| SocketAddr::new(address, port)));
        }
    }
    // IPv6 has no broadcast: every device on a link listens to ff02::1.
    let all_nodes = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1);
    for scope in scopes {
        for base in &bases {
            targets.extend(
                ports_from(*base)
                    .map(|port| SocketAddr::V6(SocketAddrV6::new(all_nodes, port, 0, *scope))),
            );
        }
    }
    targets.into_iter().collect()
}

/// Sends the packet that wakes `card` to every target there is for a PC
/// reached at `known`, as addresses or names with their ports. Returns how
/// many packets left. None leaving is an error.
pub fn send(card: Card, known: &[(String, u16)]) -> io::Result<usize> {
    let resolved: Vec<(IpAddr, u16)> = known
        .iter()
        .flat_map(|(address, port)| {
            // A name that leads nowhere today leaves the others.
            let found = (address.as_str(), *port).to_socket_addrs().map_or_else(
                |error| {
                    log::debug!("could not look up {address} to wake it: {error}");
                    Vec::new()
                },
                Iterator::collect,
            );
            found.into_iter().map(|found: SocketAddr| (found.ip(), *port))
        })
        .collect();
    let (broadcasts, scopes) = networks();
    let packet = magic_packet(card);

    let four = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    four.set_broadcast(true)?;
    let six = UdpSocket::bind((Ipv6Addr::UNSPECIFIED, 0)).ok();
    let mut sent = 0;
    let mut failure = None;
    for target in targets(&resolved, &broadcasts, &scopes) {
        let socket = match (target, &six) {
            (SocketAddr::V4(_), _) => &four,
            (SocketAddr::V6(_), Some(six)) => six,
            (SocketAddr::V6(_), None) => continue,
        };
        match socket.send_to(&packet, target) {
            Ok(_) => sent += 1,
            // A network that is down or a route that is missing leaves the
            // others.
            Err(error) => failure = Some(error),
        }
    }
    match (sent, failure) {
        (0, Some(error)) => Err(error),
        (0, None) => Err(io::Error::other("there was nowhere to send it")),
        (sent, _) => Ok(sent),
    }
}

/// The broadcast addresses and the IPv6 scopes of the networks this device
/// is on, loopback left out.
fn networks() -> (Vec<Ipv4Addr>, Vec<u32>) {
    let interfaces = if_addrs::get_if_addrs().unwrap_or_else(|error| {
        log::debug!("could not list this device's networks: {error}");
        Vec::new()
    });
    let mut broadcasts = Vec::new();
    let mut scopes = BTreeSet::new();
    for interface in interfaces.iter().filter(|interface| !interface.is_loopback()) {
        match &interface.addr {
            if_addrs::IfAddr::V4(four) => broadcasts.extend(four.broadcast),
            if_addrs::IfAddr::V6(six) if six.ip.segments()[0] & 0xffc0 == 0xfe80 => {
                scopes.extend(interface.index);
            }
            if_addrs::IfAddr::V6(_) => {}
        }
    }
    broadcasts.sort();
    broadcasts.dedup();
    (broadcasts, scopes.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARD: Card = [0x00, 0x11, 0x22, 0x33, 0x44, 0xAB];

    #[test]
    fn the_packet_is_six_bytes_of_ff_then_the_card_sixteen_times() {
        let packet = magic_packet(CARD);
        assert_eq!(packet[..6], [0xFF; 6]);
        for copy in packet[6..].chunks(6) {
            assert_eq!(copy, CARD);
        }
        assert_eq!(packet.len(), 6 + 16 * 6);
    }

    #[test]
    fn a_card_is_read_as_hosts_write_it() {
        assert_eq!(parse_card("00:11:22:33:44:ab"), Some(CARD));
        assert_eq!(parse_card(" 00-11-22-33-44-AB "), Some(CARD));
        assert_eq!(card_text(CARD), "00:11:22:33:44:AB");
        // What a host tells a device it does not trust.
        assert_eq!(parse_card("00:00:00:00:00:00"), None);
        assert_eq!(parse_card("00:11:22:33:44"), None);
        assert_eq!(parse_card("00:11:22:33:44:55:66"), None);
        assert_eq!(parse_card("0:11:22:33:44:55"), None);
        assert_eq!(parse_card("zz:11:22:33:44:55"), None);
    }

    #[test]
    fn the_packet_goes_to_every_address_and_every_network_on_the_pcs_ports() {
        let lan = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20));
        let vpn = IpAddr::V4(Ipv4Addr::new(10, 8, 0, 3));
        let targets = targets(
            &[(lan, DEFAULT_HOST_PORT), (vpn, 50_000)],
            &[Ipv4Addr::new(192, 168, 1, 255)],
            &[3],
        );
        let has = |address: IpAddr, port| targets.contains(&SocketAddr::new(address, port));

        for port in [9, 47009, 47998, 47999, 48000, 48002, 48010] {
            assert!(has(lan, port), "{lan}:{port}");
        }
        // Another port moves the streaming software's ports with it.
        assert!(has(vpn, 9) && has(vpn, 50_009) && has(vpn, 50_021));
        assert!(!has(vpn, 47998));
        // Every network hears it, at the ports of both.
        let everyone = IpAddr::V4(Ipv4Addr::BROADCAST);
        let home = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 255));
        for broadcast in [everyone, home] {
            assert!(has(broadcast, 9) && has(broadcast, 48010) && has(broadcast, 50_021));
        }
        let all_nodes = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1);
        assert!(targets.contains(&SocketAddr::V6(SocketAddrV6::new(all_nodes, 9, 0, 3))));
        // Each once.
        let unique: BTreeSet<_> = targets.iter().collect();
        assert_eq!(unique.len(), targets.len());
    }

    #[test]
    fn a_port_near_the_end_moves_no_port_past_it() {
        let pc = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let ports: BTreeSet<u16> =
            targets(&[(pc, 65_525)], &[], &[]).iter().map(SocketAddr::port).collect();
        // 65525 moved by 9 or 10 still fits; moved by more, it does not.
        assert_eq!(ports, BTreeSet::from([9, 47009, 65_534, 65_535]));
    }

    #[test]
    fn the_packet_reaches_a_card_listening_on_this_device() {
        let listening = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("a port to listen on");
        let wait = Some(std::time::Duration::from_secs(5));
        listening.set_read_timeout(wait).expect("a socket waits as long as asked");
        let port = listening.local_addr().expect("the socket has an address").port();
        // The PC's port, such that its video port is the one listening.
        let base = port - (47998 - DEFAULT_HOST_PORT);
        let sent = send(CARD, &[("127.0.0.1".to_owned(), base)]).expect("the packet leaves");
        assert!(sent > 0);
        let mut received = [0u8; 200];
        let (length, _) = listening.recv_from(&mut received).expect("the packet arrives");
        assert_eq!(received[..length], magic_packet(CARD));
    }
}
