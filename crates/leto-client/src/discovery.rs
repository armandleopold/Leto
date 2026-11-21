//! Finding hosts on the local network.
//!
//! Sunshine, Apollo and GameStream hosts announce themselves over mDNS as
//! `_nvstream._tcp`. Listening for those announcements is how the list of PCs
//! fills itself without anyone typing an address.

use std::{
    collections::BTreeSet,
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use mdns_sd::{ServiceDaemon, ServiceEvent};

/// The service type with the domain mDNS wants.
const SERVICE: &str = "_nvstream._tcp.local.";

/// An address a host announced. It still has to be asked who it is.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Announcement {
    pub address: IpAddr,
    pub port: u16,
}

/// Listens for `duration` and returns every address announced meanwhile.
pub fn listen(duration: Duration) -> Result<Vec<Announcement>, mdns_sd::Error> {
    let daemon = ServiceDaemon::new()?;
    let events = daemon.browse(SERVICE)?;
    let deadline = Instant::now() + duration;

    let mut found = BTreeSet::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let Ok(event) = events.recv_timeout(left) else { break };
        if let ServiceEvent::ServiceResolved(service) = event {
            let port = service.get_port();
            // IPv4 first: it is what hosts are set up for in practice.
            for address in service.get_addresses_v4() {
                found.insert(Announcement { address: IpAddr::V4(*address), port });
            }
        }
    }

    // Best effort: the daemon's thread ends on its own if this fails.
    let _ = daemon.stop_browse(SERVICE);
    let _ = daemon.shutdown();
    log::debug!("{} addresses announced a PC on the network", found.len());
    Ok(found.into_iter().collect())
}

/// The addresses nearby that this device's system knows `name` by, as far
/// as it finds them within `patience`.
///
/// A PC says its name when asked who it is, and the system can often turn
/// that name into the PC's address on the local network: from the router,
/// or by asking the network as Windows and macOS do for the names of
/// computers. It is a second way to the address of a PC that Leto only
/// knows from further away, through a VPN for example, where announcements
/// do not arrive: a firewall that keeps them from Leto lets the system ask.
///
/// Only addresses of a local network are returned. A name can mean
/// something else on the internet, and that is nobody to ask.
pub fn by_name(name: &str, patience: Duration) -> Vec<Ipv4Addr> {
    if !is_computer_name(name) {
        return Vec::new();
    }
    let mut asked = vec![name.to_owned()];
    if !name.to_ascii_lowercase().ends_with(".local") {
        // The name mDNS gives a computer.
        asked.push(format!("{name}.local"));
    }
    let (sender, answers) = mpsc::channel();
    for name in asked {
        let sender = sender.clone();
        // Not waited for beyond `patience`: a name nobody knows can take the
        // system many seconds.
        let asking = thread::Builder::new().name("leto: Name".to_owned()).spawn(move || {
            if let Ok(addresses) = (name.as_str(), 0).to_socket_addrs() {
                let _ = sender.send(addresses.map(|address| address.ip()).collect::<Vec<_>>());
            }
        });
        if let Err(error) = asking {
            log::error!("could not start a worker thread: {error}");
        }
    }
    drop(sender);

    let deadline = Instant::now() + patience;
    let mut found = BTreeSet::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let Ok(addresses) = answers.recv_timeout(left) else { break };
        found.extend(addresses.into_iter().filter_map(|address| match address {
            IpAddr::V4(address) if is_nearby(address) => Some(address),
            _ => None,
        }));
    }
    found.into_iter().collect()
}

/// A local network this device is on, as far as Leto looks around on it:
/// the addresses that share the first numbers of this device's own.
///
/// Never more than the 254 that share its first three numbers, whatever
/// the size of the network: looking around means asking each of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Around {
    /// The first address of them all, which is none of a device's.
    first: u32,
    /// How many of the leading bits the addresses share: 24 to 30.
    shared: u32,
    /// This device's own.
    own: Ipv4Addr,
}

impl Around {
    /// The addresses around `own`, on a network whose addresses share
    /// their first `shared` bits. `None` for a network of two, as a VPN
    /// makes between this device and one other: there is nothing to look
    /// for around.
    pub fn new(own: Ipv4Addr, shared: u32) -> Option<Self> {
        if shared > 30 {
            return None;
        }
        let shared = shared.max(24);
        let first = u32::from(own) & (u32::MAX << (32 - shared));
        Some(Self { first, shared, own })
    }

    /// Whether `address` is one of them, this device's own included.
    pub fn holds(&self, address: Ipv4Addr) -> bool {
        u32::from(address) & (u32::MAX << (32 - self.shared)) == self.first
    }

    /// Every address another device can have there.
    pub fn addresses(&self) -> impl Iterator<Item = Ipv4Addr> + use<> {
        let (first, own) = (self.first, self.own);
        let last = first | (u32::MAX >> self.shared);
        // The first and the last belong to the network itself.
        (first + 1..last).map(Ipv4Addr::from).filter(move |address| *address != own)
    }
}

/// The local networks this device is on, to look around on. Only networks
/// of private addresses: Leto asks nothing of strangers on the internet.
pub fn networks() -> Vec<Around> {
    let interfaces = if_addrs::get_if_addrs().unwrap_or_else(|error| {
        log::debug!("could not list this device's networks: {error}");
        Vec::new()
    });
    let found: BTreeSet<Around> = interfaces
        .iter()
        .filter(|interface| !interface.is_loopback())
        .filter_map(|interface| match &interface.addr {
            if_addrs::IfAddr::V4(four) if four.ip.is_private() => {
                Around::new(four.ip, u32::from(four.netmask).leading_ones())
            }
            _ => None,
        })
        .collect();
    found.into_iter().collect()
}

/// Of `addresses`, those where something takes a connection on `port`
/// within `patience`. All are tried at once, so it takes `patience` and no
/// longer. Nothing is sent: the connection is closed as soon as it is made.
pub fn listening(addresses: &[Ipv4Addr], port: u16, patience: Duration) -> Vec<Ipv4Addr> {
    thread::scope(|scope| {
        let trying: Vec<_> = addresses
            .iter()
            .filter_map(|&address| {
                // Small: each only waits for one connection.
                let trying = thread::Builder::new()
                    .name("leto: Around".to_owned())
                    .stack_size(64 * 1024)
                    .spawn_scoped(scope, move || {
                        let there = SocketAddr::from((address, port));
                        TcpStream::connect_timeout(&there, patience).is_ok()
                    });
                match trying {
                    Ok(trying) => Some((address, trying)),
                    Err(error) => {
                        log::debug!("could not start a thread to look around: {error}");
                        None
                    }
                }
            })
            .collect();
        trying
            .into_iter()
            .filter_map(|(address, trying)| trying.join().unwrap_or(false).then_some(address))
            .collect()
    })
}

/// Whether `name` can be the name of a computer on a network: letters,
/// digits and hyphens, in parts no longer than 63 joined by dots. A PC can
/// call itself anything, and an address is not a name.
fn is_computer_name(name: &str) -> bool {
    let part = |part: &str| {
        (1..=63).contains(&part.len())
            && part.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            && !part.starts_with('-')
            && !part.ends_with('-')
    };
    name.len() <= 253 && name.split('.').all(part) && name.parse::<IpAddr>().is_err()
}

/// Whether `address` is one of a local network, or of this device itself.
const fn is_nearby(address: Ipv4Addr) -> bool {
    address.is_private() || address.is_link_local() || address.is_loopback()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_addresses_around_a_device_are_those_another_device_can_have() {
        let own: Ipv4Addr = "192.168.1.37".parse().expect("an address");
        let around = Around::new(own, 24).expect("a network of many");
        let addresses: Vec<_> = around.addresses().collect();
        assert_eq!(addresses.len(), 253, "254 devices at most, less this one");
        assert_eq!(addresses.first(), Some(&Ipv4Addr::new(192, 168, 1, 1)));
        assert_eq!(addresses.last(), Some(&Ipv4Addr::new(192, 168, 1, 254)));
        assert!(!addresses.contains(&own));
        assert!(around.holds(own) && around.holds(Ipv4Addr::new(192, 168, 1, 200)));
        assert!(!around.holds(Ipv4Addr::new(192, 168, 2, 37)));
        assert!(!around.holds(Ipv4Addr::new(10, 8, 0, 3)));

        // A large network is not asked whole: only what is nearest.
        let large = Around::new(Ipv4Addr::new(10, 3, 7, 9), 8).expect("a network of many");
        assert_eq!(large.addresses().count(), 253);
        assert!(large.holds(Ipv4Addr::new(10, 3, 7, 200)));
        assert!(!large.holds(Ipv4Addr::new(10, 3, 8, 9)));

        // A small one has what it has, and one of two has nothing around.
        let small = Around::new(Ipv4Addr::new(127, 0, 0, 2), 30).expect("a network of four");
        assert_eq!(small.addresses().collect::<Vec<_>>(), [Ipv4Addr::LOCALHOST]);
        assert_eq!(Around::new(own, 31), None);
        assert_eq!(Around::new(own, 32), None);
    }

    #[test]
    fn what_takes_a_connection_is_told_from_what_does_not() {
        let open = std::net::TcpListener::bind("127.0.0.1:0").expect("a port to listen on");
        let port = open.local_addr().expect("its address").port();
        let here = [Ipv4Addr::LOCALHOST];
        assert_eq!(listening(&here, port, Duration::from_secs(5)), here);
        // Nothing listens once it is closed.
        drop(open);
        assert!(listening(&here, port, Duration::from_secs(5)).is_empty());
        assert!(listening(&[], port, Duration::from_secs(5)).is_empty());
    }

    #[test]
    fn the_name_of_a_computer_is_told_from_what_a_pc_can_call_itself() {
        for name in ["armand-desktop", "SALON", "pc7", "salon.local", "tour.home.arpa"] {
            assert!(is_computer_name(name), "{name}");
        }
        let long = "a".repeat(64);
        for name in ["", "Armand's PC", "le salon", "-pc", "pc-", "a..b", "192.168.1.20", &long] {
            assert!(!is_computer_name(name), "{name}");
        }
    }

    #[test]
    fn only_addresses_of_a_local_network_are_nearby() {
        for address in ["192.168.1.20", "10.8.0.3", "172.16.4.1", "169.254.7.7", "127.0.0.1"] {
            assert!(is_nearby(address.parse().expect("an address")), "{address}");
        }
        for address in ["8.8.8.8", "172.32.0.1", "100.64.0.1", "0.0.0.0", "255.255.255.255"] {
            assert!(!is_nearby(address.parse().expect("an address")), "{address}");
        }
    }

    #[test]
    fn the_system_is_asked_for_the_addresses_of_a_name() {
        // The one name every system knows.
        let found = by_name("localhost", Duration::from_secs(5));
        assert_eq!(found, [Ipv4Addr::LOCALHOST]);
        // What is no name is not asked about.
        assert!(by_name("not a name", Duration::from_secs(5)).is_empty());
    }
}
