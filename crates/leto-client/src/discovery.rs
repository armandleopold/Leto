//! Finding hosts on the local network.
//!
//! Sunshine, Apollo and GameStream hosts announce themselves over mDNS as
//! `_nvstream._tcp`. Listening for those announcements is how the list of PCs
//! fills itself without anyone typing an address.

use std::{
    collections::BTreeSet,
    net::{IpAddr, Ipv4Addr, ToSocketAddrs},
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
