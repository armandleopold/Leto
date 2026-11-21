use std::{
    collections::{HashMap, VecDeque},
    convert::Infallible,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use bytes::Bytes;
use rusty_enet::{
    Address, Event, Host, HostSettings, MTU_MAX, PacketReceived, Peer, PeerID, ReadWrite, Socket,
    SocketOptions,
};
use sans_io_time::Instant;
use tracing::{debug, trace};

use crate::{
    error::Error,
    stream::{control::PacketSendError, proto::runtime::Transmit},
};

// TODO: dynamically set timeout, see https://github.com/jabuwu/rusty_enet/issues/4
// TODO: this seems interesting: https://github.com/zpl-c/enet/blob/8647b6eaea881c86471ae29f732620d299fc20d7/include/enet.h#L296-L488

impl From<rusty_enet::error::HostNewError<ReadWrite<SocketAddr, Infallible>>> for Error {
    fn from(value: rusty_enet::error::HostNewError<ReadWrite<SocketAddr, Infallible>>) -> Self {
        match value {
            rusty_enet::error::HostNewError::BadParameter(parameter) => {
                Error::Other(parameter.into())
            }
            rusty_enet::error::HostNewError::FailedToInitializeSocket(_) => unreachable!(),
        }
    }
}

impl From<rusty_enet::error::PeerSendError> for Error {
    fn from(value: rusty_enet::error::PeerSendError) -> Self {
        use rusty_enet::error::PeerSendError;

        match value {
            PeerSendError::NotConnected => Error::PacketSend(PacketSendError::PeerNotConnected),
            value => Error::Other(value.into()),
        }
    }
}

impl From<rusty_enet::error::NoAvailablePeers> for Error {
    fn from(value: rusty_enet::error::NoAvailablePeers) -> Self {
        Self::Other(value.into())
    }
}

pub struct EnetConfig {
    pub peer_count: usize,
    pub channel_limit: usize,
    pub incoming_bandwidth: Option<usize>,
    pub outgoing_bandwidth: Option<usize>,
}

#[derive(Debug)]
pub enum EnetEvent {
    Connect {
        peer: PeerID,
        data: u32,
    },
    Receive {
        peer: PeerID,
        channel_id: u8,
        data: Vec<u8>,
    },
    Disconnect {
        peer: PeerID,
        #[allow(unused)]
        data: u32,
    },
}

/// Leto: the commands of ENet by their number, as they cross the network:
/// how long each is, and where it says how much data follows it.
/// From `protocol.h` of ENet.
fn command_layout(number: u8) -> Option<(usize, Option<usize>)> {
    Some(match number {
        // An acknowledgement.
        1 => (8, None),
        2 => (48, None),
        3 => (44, None),
        4 => (8, None),
        5 => (4, None),
        6 => (6, Some(4)),
        7 | 9 => (8, Some(6)),
        8 | 12 => (24, Some(6)),
        10 => (12, None),
        11 => (16, None),
        _ => return None,
    })
}

/// Leto: the times at which what `datagram` acknowledges was sent, as the
/// sender stamped them: the low 16 bits of its clock in milliseconds.
/// Nothing for a datagram that is compressed or does not read as ENet's.
fn acknowledged_sent_times(datagram: &[u8]) -> Vec<u16> {
    const SENT_TIME: u16 = 1 << 15;
    const COMPRESSED: u16 = 1 << 14;
    const ACKNOWLEDGE: u8 = 1;

    let word = |at: usize| {
        Some(u16::from_be_bytes([
            *datagram.get(at)?,
            *datagram.get(at + 1)?,
        ]))
    };
    let mut times = Vec::new();
    let Some(peer) = word(0) else {
        return times;
    };
    if peer & COMPRESSED != 0 {
        return times;
    }
    let mut at = if peer & SENT_TIME != 0 { 4 } else { 2 };
    while let Some(command) = datagram.get(at) {
        let number = command & 0x0f;
        let Some((size, data_length_at)) = command_layout(number) else {
            break;
        };
        if at + size > datagram.len() {
            break;
        }
        if number == ACKNOWLEDGE
            && let Some(sent) = word(at + 6)
        {
            times.push(sent);
        }
        let data = match data_length_at.map(|offset| word(at + offset)) {
            None => 0,
            Some(Some(length)) => usize::from(length),
            Some(None) => break,
        };
        at += size + data;
    }
    times
}

/// Leto: the way to a peer and back, as a mean that follows what is
/// measured.
///
/// ENet keeps one too, in whole milliseconds: each measure moves it by an
/// eighth of the difference, rounded down. A difference under 8 ms moves
/// it by nothing, so once a few slow answers have raised it, it comes down
/// to 7 ms above the truth and stays there: a host 1 ms away reads 8 ms
/// for the rest of the stream. This mean weighs each measure as ENet does
/// and keeps the fractions.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct RoundTrip {
    milliseconds: Option<f32>,
}

impl RoundTrip {
    /// One more measure, in milliseconds.
    fn measured(&mut self, milliseconds: u16) {
        let measure = f32::from(milliseconds);
        self.milliseconds = Some(match self.milliseconds {
            Some(mean) => mean + (measure - mean) / 8.0,
            None => measure,
        });
    }

    fn mean(self) -> Option<Duration> {
        self.milliseconds
            .map(|milliseconds| Duration::from_secs_f32(milliseconds / 1000.0))
    }
}

/// Leto: a measure longer than this is not one: the stamp it comes from
/// was not this host's, or is from another turn of the 16-bit clock.
const LONGEST_ROUND_TRIP_MS: u16 = 10_000;

pub struct EnetHost {
    /// Leto: when ENet's clock started, and the way to each peer and back
    /// as its acknowledgements tell it. See [`RoundTrip`].
    start: Instant,
    round_trips: HashMap<SocketAddr, RoundTrip>,
    last_now: Arc<Mutex<Instant>>,
    pub(crate) enet: Host<Io<SocketAddr>>,
    events: VecDeque<EnetEvent>,
}

impl EnetHost {
    pub fn new(now: Instant, config: EnetConfig) -> Self {
        let last_now = Arc::new(Mutex::new(now));

        // This unwrap is safe because those settings don't fail, which would be the only error source
        #[allow(clippy::unwrap_used)]
        let enet = Host::new(
            Io::<SocketAddr>::default(),
            HostSettings {
                peer_limit: config.peer_count,
                channel_limit: config.channel_limit,
                incoming_bandwidth_limit: config.incoming_bandwidth.map(|x| x as u32),
                outgoing_bandwidth_limit: config.outgoing_bandwidth.map(|x| x as u32),
                time: {
                    let start = now;
                    let last_now = last_now.clone();
                    Box::new(move || {
                        let last_now = {
                            // This is allowed because we:
                            // 1. only we have got access to this mutex
                            // 2. don't panic inside this implementation
                            #[allow(clippy::unwrap_used)]
                            let last_now = last_now.lock().unwrap();

                            *last_now
                        };

                        last_now - start
                    })
                },
                ..Default::default()
            },
        )
        .unwrap();

        Self {
            start: now,
            round_trips: HashMap::new(),
            last_now,
            enet,
            events: Default::default(),
        }
    }

    /// Leto: the way to the peer at `addr` and back, when it acknowledged
    /// something already. See [`RoundTrip`].
    pub fn round_trip(&self, addr: SocketAddr) -> Option<Duration> {
        self.round_trips.get(&addr)?.mean()
    }

    pub fn connect(
        &mut self,
        addr: SocketAddr,
        channel_count: usize,
        data: u32,
    ) -> Result<PeerID, Error> {
        debug!(remote_addr = ?addr, connect_data = ?data, "enet starting connect");

        let peer = self.enet.connect(addr, channel_count, data)?;

        Ok(peer.id())
    }

    pub fn disconnect_later(&mut self, id: PeerID, data: u32) -> Result<(), Error> {
        self.enet
            .get_peer_mut(id)
            .ok_or(Error::PacketSend(PacketSendError::PeerNotFound))?
            .disconnect_later(data);

        Ok(())
    }
    pub fn disconnect_now(&mut self, id: PeerID, data: u32) -> Result<(), Error> {
        self.enet
            .get_peer_mut(id)
            .ok_or(Error::PacketSend(PacketSendError::PeerNotFound))?
            .disconnect_now(data);

        Ok(())
    }

    pub fn peer_mut(&mut self, id: PeerID) -> Option<&mut Peer<impl Socket<Address = SocketAddr>>> {
        self.enet.get_peer_mut(id)
    }
    pub fn peer(&self, id: PeerID) -> Option<&Peer<impl Socket<Address = SocketAddr>>> {
        self.enet.get_peer(id)
    }

    pub fn poll_transmit(&mut self) -> Option<Transmit> {
        self.enet
            .socket_mut()
            .outbound
            .pop_front()
            .map(|(destination, data)| Transmit { destination, data })
    }

    pub fn poll_timeout(&self) -> Instant {
        self.last_now() + Duration::from_millis(50)
    }

    pub fn poll_event(&mut self) -> Option<EnetEvent> {
        self.events.pop_front()
    }

    pub fn handle_receive(&mut self, now: Instant, addr: SocketAddr, data: &[u8]) {
        // Leto: each acknowledgement says when what it answers was sent, by
        // the clock ENet stamped it with: the milliseconds since `start`.
        let clock = ((now - self.start).as_millis() % u128::from(u32::MAX)) as u16;
        for sent in acknowledged_sent_times(data) {
            let took = clock.wrapping_sub(sent);
            if took <= LONGEST_ROUND_TRIP_MS {
                self.round_trips.entry(addr).or_default().measured(took);
            }
        }

        self.enet
            .socket_mut()
            .inbound
            .push_back((addr, data.to_vec()));

        self.handle_timeout(now);
    }

    pub fn handle_timeout(&mut self, now: Instant) {
        self.set_last_now(now);

        self.service();
    }

    /// Must be called after each modification to update all state
    pub fn service(&mut self) {
        // The error is infallible and cannot be constructed
        // -> we are allowed to unwrap
        #[allow(clippy::unwrap_used)]
        while let Some(event) = self.enet.service().unwrap() {
            trace!(event = ?event, "enet service event");

            let event = match event {
                Event::Connect { peer, data } => {
                    debug!(peer_id = ?peer.id(), connect_data = ?data, "enet peer connected");

                    EnetEvent::Connect {
                        peer: peer.id(),
                        data,
                    }
                }
                Event::Receive {
                    peer,
                    channel_id,
                    packet,
                } => {
                    trace!(peer_id = ?peer.id(), channel_id = ?channel_id, packet = ?packet, "enet received packet");

                    EnetEvent::Receive {
                        peer: peer.id(),
                        channel_id,
                        data: packet.data().to_vec(),
                    }
                }
                Event::Disconnect { peer, data } => {
                    debug!(peer_id = ?peer.id(), disconnect_data = ?data, "enet peer disconnected");

                    EnetEvent::Disconnect {
                        peer: peer.id(),
                        data,
                    }
                }
            };

            self.events.push_back(event);
        }
    }

    fn set_last_now(&self, now: Instant) {
        // This is allowed because we:
        // 1. only we have got access to this mutex
        // 2. don't panic inside this implementation
        #[allow(clippy::unwrap_used)]
        let mut last_now = self.last_now.lock().unwrap();

        *last_now = now;
    }
    fn last_now(&self) -> Instant {
        // This is allowed because we:
        // 1. only we have got access to this mutex
        // 2. don't panic inside this implementation
        #[allow(clippy::unwrap_used)]
        let last_now = self.last_now.lock().unwrap();

        *last_now
    }
}

#[derive(Debug)]
pub(crate) struct Io<A> {
    inbound: VecDeque<(A, Vec<u8>)>,
    outbound: VecDeque<(A, Bytes)>,
}

impl<A> Default for Io<A> {
    fn default() -> Self {
        Self {
            inbound: Default::default(),
            outbound: Default::default(),
        }
    }
}

impl<A: Address + 'static> Socket for Io<A> {
    type Address = A;
    type Error = Infallible;

    fn init(&mut self, _socket_options: SocketOptions) -> Result<(), Self::Error> {
        // NOTE: this implementation must not become fallable
        Ok(())
    }

    fn send(&mut self, address: A, buffer: &[u8]) -> Result<usize, Infallible> {
        self.outbound
            .push_back((address, Bytes::copy_from_slice(buffer)));
        Ok(buffer.len())
    }

    fn receive(
        &mut self,
        buffer: &mut [u8; MTU_MAX],
    ) -> Result<Option<(A, PacketReceived)>, Infallible> {
        if let Some((address, inbound)) = self.inbound.pop_front() {
            let bytes = inbound.len();
            if bytes <= MTU_MAX {
                #[cfg(feature = "std")]
                {
                    use std::io::{Cursor, copy};
                    copy(&mut Cursor::new(inbound), &mut Cursor::new(&mut buffer[..]))
                        .expect("Buffer copy should not fail.");
                }
                #[cfg(not(feature = "std"))]
                unsafe {
                    use core::ptr::copy_nonoverlapping;
                    copy_nonoverlapping(inbound.as_ptr(), buffer.as_mut_ptr(), bytes);
                }
                Ok(Some((address, PacketReceived::Complete(bytes))))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An acknowledgement of something sent at `sent`.
    fn acknowledgement(sent: u16) -> Vec<u8> {
        let mut command = vec![1, 0xff, 0, 1, 0, 7];
        command.extend(sent.to_be_bytes());
        command
    }

    #[test]
    fn acknowledgements_are_found_among_the_other_commands() {
        // The peer and the time the datagram was sent at.
        let mut datagram = vec![0x80, 0x01, 0x12, 0x34];
        datagram.extend(acknowledgement(1000));
        // A ping, then five bytes sent reliably, with the flag that asks
        // for an acknowledgement.
        datagram.extend([5, 0xff, 0, 2]);
        datagram.extend([0x86, 0, 0, 3, 0, 5, 1, 2, 3, 4, 5]);
        datagram.extend(acknowledgement(1003));
        // Three bytes sent unreliably, and one more acknowledgement.
        datagram.extend([7, 1, 0, 0, 0, 9, 0, 3, 9, 9, 9]);
        datagram.extend(acknowledgement(65_535));
        assert_eq!(acknowledged_sent_times(&datagram), [1000, 1003, 65_535]);

        // Without the time the datagram was sent at.
        let mut datagram = vec![0x00, 0x01];
        datagram.extend(acknowledgement(7));
        assert_eq!(acknowledged_sent_times(&datagram), [7]);
    }

    #[test]
    fn what_does_not_read_as_enet_acknowledges_nothing() {
        assert_eq!(acknowledged_sent_times(&[]), []);
        assert_eq!(acknowledged_sent_times(&[0x80]), []);
        // Compressed.
        let mut datagram = vec![0x40, 0x01];
        datagram.extend(acknowledgement(7));
        assert_eq!(acknowledged_sent_times(&datagram), []);
        // An acknowledgement cut short, a command nobody knows, and data
        // that says it is longer than the datagram.
        assert_eq!(acknowledged_sent_times(&[0, 1, 1, 0xff, 0, 1, 0, 7, 0]), []);
        assert_eq!(
            acknowledged_sent_times(&[0, 1, 13, 0, 0, 1, 0, 7, 0, 7]),
            []
        );
        let mut datagram = vec![0, 1, 6, 0, 0, 3, 0xff, 0xff, 1, 2];
        datagram.extend(acknowledgement(7));
        assert_eq!(acknowledged_sent_times(&datagram), []);
    }

    #[test]
    fn the_mean_comes_down_where_enets_stays_seven_milliseconds_above() {
        // A slow start, then a host 1 ms away.
        let measures = [40_u16, 60, 35]
            .into_iter()
            .chain(std::iter::repeat_n(1, 200));
        let mut enet = None::<u32>;
        let mut ours = RoundTrip::default();
        for measure in measures {
            // ENet's rule, in whole milliseconds.
            let measure_ms = u32::from(measure);
            enet = Some(match enet {
                Some(mean) if measure_ms >= mean => mean + (measure_ms - mean) / 8,
                Some(mean) => mean - (mean - measure_ms) / 8,
                None => measure_ms,
            });
            ours.measured(measure);
        }
        assert_eq!(enet, Some(8));
        let ours = ours.mean().expect("a mean");
        assert!(ours < Duration::from_micros(1100), "{ours:?}");
        assert!(ours >= Duration::from_millis(1), "{ours:?}");
    }
}
