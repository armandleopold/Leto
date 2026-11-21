//! IPv4 and UDP as they travel inside the tunnel. The pictures and sound of
//! a stream are UDP, written and read here without a TCP/IP stack in the
//! way; TCP is smoltcp's.

use std::net::{Ipv4Addr, SocketAddrV4};

pub const IPV4_HEADER: usize = 20;
pub const UDP_HEADER: usize = 8;
pub const TCP: u8 = 6;
pub const UDP: u8 = 17;

/// An IPv4 packet, read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ipv4<'a> {
    pub source: Ipv4Addr,
    pub destination: Ipv4Addr,
    pub protocol: u8,
    /// A piece of a larger packet, which Leto does not put back together.
    pub fragment: bool,
    pub payload: &'a [u8],
}

/// Reads an IPv4 packet. None for anything else.
pub fn read_ipv4(packet: &[u8]) -> Option<Ipv4<'_>> {
    let first = *packet.first()?;
    if first >> 4 != 4 || packet.len() < IPV4_HEADER {
        return None;
    }
    let header = usize::from(first & 0x0f) * 4;
    let total = usize::from(u16::from_be_bytes([packet[2], packet[3]]));
    if header < IPV4_HEADER || total < header || total > packet.len() {
        return None;
    }
    let flags = u16::from_be_bytes([packet[6], packet[7]]);
    Some(Ipv4 {
        source: Ipv4Addr::new(packet[12], packet[13], packet[14], packet[15]),
        destination: Ipv4Addr::new(packet[16], packet[17], packet[18], packet[19]),
        protocol: packet[9],
        // More fragments, or an offset.
        fragment: flags & 0x2000 != 0 || flags & 0x1fff != 0,
        payload: &packet[header..total],
    })
}

/// A UDP datagram, read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Udp<'a> {
    pub source_port: u16,
    pub destination_port: u16,
    pub payload: &'a [u8],
}

/// Reads the UDP datagram an IPv4 packet carries.
pub fn read_udp(segment: &[u8]) -> Option<Udp<'_>> {
    if segment.len() < UDP_HEADER {
        return None;
    }
    let length = usize::from(u16::from_be_bytes([segment[4], segment[5]]));
    if length < UDP_HEADER || length > segment.len() {
        return None;
    }
    Some(Udp {
        source_port: u16::from_be_bytes([segment[0], segment[1]]),
        destination_port: u16::from_be_bytes([segment[2], segment[3]]),
        payload: &segment[UDP_HEADER..length],
    })
}

/// Writes an IPv4 packet carrying one UDP datagram, numbered `id`. None if
/// the payload is too large for one packet.
pub fn write_udp(
    source: SocketAddrV4,
    destination: SocketAddrV4,
    id: u16,
    payload: &[u8],
) -> Option<Vec<u8>> {
    let udp_length = u16::try_from(UDP_HEADER + payload.len()).ok()?;
    let total = u16::try_from(IPV4_HEADER + usize::from(udp_length)).ok()?;
    let mut packet = Vec::with_capacity(usize::from(total));
    // Version 4, header of five words, no special service.
    packet.extend_from_slice(&[0x45, 0]);
    packet.extend_from_slice(&total.to_be_bytes());
    packet.extend_from_slice(&id.to_be_bytes());
    // No flags, no offset: a whole packet, which routers may cut.
    packet.extend_from_slice(&[0, 0]);
    // A lifetime of 64 hops, UDP, the checksum to come.
    packet.extend_from_slice(&[64, UDP, 0, 0]);
    packet.extend_from_slice(&source.ip().octets());
    packet.extend_from_slice(&destination.ip().octets());
    let header_sum = !fold(sum(&packet[..IPV4_HEADER]));
    packet[10..12].copy_from_slice(&header_sum.to_be_bytes());

    packet.extend_from_slice(&source.port().to_be_bytes());
    packet.extend_from_slice(&destination.port().to_be_bytes());
    packet.extend_from_slice(&udp_length.to_be_bytes());
    packet.extend_from_slice(&[0, 0]);
    packet.extend_from_slice(payload);
    // Over a pseudo header of both addresses, the protocol and the length.
    let pseudo = sum(&source.ip().octets())
        + sum(&destination.ip().octets())
        + u32::from(UDP)
        + u32::from(udp_length);
    let mut udp_sum = !fold(pseudo + sum(&packet[IPV4_HEADER..]));
    // Zero would say there is no checksum.
    if udp_sum == 0 {
        udp_sum = 0xffff;
    }
    packet[IPV4_HEADER + 6..IPV4_HEADER + 8].copy_from_slice(&udp_sum.to_be_bytes());
    Some(packet)
}

/// The sum of the 16-bit words of `bytes`, an odd last byte padded.
fn sum(bytes: &[u8]) -> u32 {
    let mut words = bytes.chunks_exact(2);
    let mut total: u32 =
        words.by_ref().map(|word| u32::from(u16::from_be_bytes([word[0], word[1]]))).sum();
    if let [last] = words.remainder() {
        total += u32::from(*last) << 8;
    }
    total
}

/// A sum folded into 16 bits, the carries added back, as IP does.
fn fold(mut total: u32) -> u16 {
    while total > 0xffff {
        total = (total & 0xffff) + (total >> 16);
    }
    // At most 0xffff after the loop.
    u16::try_from(total).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FROM: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(10, 8, 0, 2), 50_000);
    const TO: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(10, 8, 0, 3), 47998);

    #[test]
    fn a_datagram_written_reads_back_the_same() {
        let packet = write_udp(FROM, TO, 7, b"picture").expect("a small datagram");
        let ip = read_ipv4(&packet).expect("an IPv4 packet");
        assert_eq!((ip.source, ip.destination, ip.protocol), (*FROM.ip(), *TO.ip(), UDP));
        assert!(!ip.fragment);
        let udp = read_udp(ip.payload).expect("a UDP datagram");
        assert_eq!((udp.source_port, udp.destination_port), (FROM.port(), TO.port()));
        assert_eq!(udp.payload, b"picture");
    }

    #[test]
    fn the_checksums_are_those_a_system_checks() {
        let packet = write_udp(FROM, TO, 7, b"odd").expect("a small datagram");
        // A header with its checksum sums to all ones.
        assert_eq!(fold(sum(&packet[..IPV4_HEADER])), 0xffff);
        let udp = &packet[IPV4_HEADER..];
        let pseudo = sum(&FROM.ip().octets())
            + sum(&TO.ip().octets())
            + u32::from(UDP)
            + u32::try_from(udp.len()).expect("a short datagram");
        assert_eq!(fold(pseudo + sum(udp)), 0xffff);
    }

    #[test]
    fn a_piece_of_a_packet_is_told_apart() {
        let mut packet = write_udp(FROM, TO, 7, &[0; 100]).expect("a small datagram");
        packet[6] = 0x20;
        assert!(read_ipv4(&packet).expect("an IPv4 packet").fragment);
        packet[6] = 0;
        packet[7] = 0x10;
        assert!(read_ipv4(&packet).expect("an IPv4 packet").fragment);
    }

    #[test]
    fn what_is_not_a_packet_is_none() {
        assert_eq!(read_ipv4(&[]), None);
        assert_eq!(read_ipv4(&[0x60; 40]), None, "IPv6");
        let mut packet = write_udp(FROM, TO, 7, b"x").expect("a small datagram");
        packet[3] = 200;
        assert_eq!(read_ipv4(&packet), None, "longer than what arrived");
        assert_eq!(read_udp(&[0; 4]), None);
        assert!(write_udp(FROM, TO, 7, &vec![0; 70_000]).is_none());
    }
}
