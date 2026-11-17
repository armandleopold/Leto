//! Finding hosts on the local network.
//!
//! Sunshine, Apollo and GameStream hosts announce themselves over mDNS as
//! `_nvstream._tcp`. Listening for those announcements is how the list of PCs
//! fills itself without anyone typing an address.

use std::{
    collections::BTreeSet,
    net::IpAddr,
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
    Ok(found.into_iter().collect())
}
