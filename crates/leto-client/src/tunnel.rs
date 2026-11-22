//! The way to a PC through the WireGuard tunnel Leto runs itself.
//!
//! The person gives Leto a WireGuard file, as their WireGuard app or server
//! writes one, and Leto keeps it beside its settings. The tunnel opens the
//! first time a PC it leads to is looked for. Each address of a PC that the
//! tunnel covers then has a door, a local address that leads through the
//! tunnel to it (`leto_tunnel::Door`), and the door is one more way to try
//! in the race for the nearest: at home the local network still answers
//! first.

use std::{
    collections::HashMap,
    fs, io,
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use leto_core::model::TunnelSummary;
use leto_tunnel::{Config, Door, Tunnel};

use crate::{Error, store};

/// The file beside the settings. It holds a private key, as `client.json`
/// does, and is kept as private.
pub const FILE_NAME: &str = "wireguard.conf";

/// How long after a tunnel failed to open before trying again.
const RETRY: Duration = Duration::from_secs(10);

/// The tunnel of this device, opened when first needed.
pub(crate) struct Ways {
    path: PathBuf,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    config: Option<Config>,
    tunnel: Option<Arc<Tunnel>>,
    /// The doors open, by the address and the port of the PC they lead to.
    doors: HashMap<(Ipv4Addr, u16), Arc<Door>>,
    problem: Option<String>,
    failed: Option<Instant>,
}

impl Ways {
    /// Reads the WireGuard file kept in `directory`, if there is one. A file
    /// that cannot be used is reported, not fatal: the PCs stay reachable
    /// directly.
    pub(crate) fn open(directory: &Path) -> Self {
        let path = directory.join(FILE_NAME);
        let mut state = State::default();
        match fs::read_to_string(&path) {
            Ok(text) => match Config::parse(&text) {
                Ok(config) => state.config = Some(config),
                Err(error) => {
                    log::warn!("the WireGuard file {} cannot be used: {error}", path.display());
                    state.problem = Some(error.to_string());
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => log::warn!("could not read {}: {error}", path.display()),
        }
        Self { path, state: Mutex::new(state) }
    }

    /// Whether the tunnel, if there is one, leads to `address`.
    pub(crate) fn covers(&self, address: &str) -> bool {
        let Ok(IpAddr::V4(target)) = address.parse::<IpAddr>() else { return false };
        usable(target)
            && self.lock().config.as_ref().is_some_and(|config| config.covers(target.into()))
    }

    /// Whether `address` is inside the tunnel itself, as a PC that is one of
    /// its peers, rather than somewhere the other end leads on to, as a
    /// network at home. See [`inside`].
    pub(crate) fn inside(&self, address: &str) -> bool {
        let Ok(IpAddr::V4(target)) = address.parse::<IpAddr>() else { return false };
        self.lock().config.as_ref().is_some_and(|config| inside(config, target))
    }

    /// The local address that leads through the tunnel to `address`, for a
    /// PC whose port is `port`. None when there is no tunnel, it does not
    /// cover the address, or it cannot open.
    pub(crate) fn door(&self, address: &str, port: u16) -> Option<Ipv4Addr> {
        let Ok(IpAddr::V4(target)) = address.parse::<IpAddr>() else { return None };
        if !usable(target) {
            return None;
        }
        let mut state = self.lock();
        let config = state.config.as_ref()?;
        if !config.covers(IpAddr::V4(target)) {
            return None;
        }
        if let Some(door) = state.doors.get(&(target, port)) {
            return Some(door.address());
        }
        let tunnel = state.tunnel()?;
        match Door::open(tunnel, target, &tcp_ports(port), &udp_ports(port)) {
            Ok(door) => {
                let local = door.address();
                state.doors.insert((target, port), Arc::new(door));
                Some(local)
            }
            Err(error) => {
                log::warn!("could not open a door to {target} through the tunnel: {error}");
                None
            }
        }
    }

    /// What the tunnel is and how it does, if there is one.
    pub(crate) fn summary(&self) -> Option<TunnelSummary> {
        let state = self.lock();
        let Some(config) = &state.config else {
            // A file that could not be used still says so.
            return state.problem.as_ref().map(|problem| TunnelSummary {
                address: String::new(),
                endpoint: None,
                covers: Vec::new(),
                public_key: String::new(),
                running: false,
                handshake: None,
                problem: Some(problem.clone()),
            });
        };
        let status = state.tunnel.as_ref().map(|tunnel| tunnel.status());
        Some(TunnelSummary {
            address: config.address.to_string(),
            endpoint: config.endpoint.clone(),
            covers: config.allowed.iter().map(ToString::to_string).collect(),
            public_key: leto_tunnel::public_key(config.private_key),
            running: state.tunnel.is_some(),
            handshake: status.as_ref().and_then(|status| status.handshake).map(|ago| ago.as_secs()),
            problem: status.and_then(|status| status.problem).or_else(|| state.problem.clone()),
        })
    }

    /// Uses the tunnel the WireGuard file `text` describes, instead of any
    /// other, and keeps the file. The tunnel opens at once, to tell whether
    /// it can.
    pub(crate) fn set(&self, text: &str) -> Result<TunnelSummary, Error> {
        let config = Config::parse(text).map_err(|error| Error::Tunnel(error.to_string()))?;
        store::write_private(&self.path, text.as_bytes())?;
        {
            let mut state = self.lock();
            // The doors and the tunnel of the old file close.
            *state = State { config: Some(config), ..State::default() };
            let _ = state.tunnel();
        }
        log::info!("Leto now has a WireGuard tunnel, kept in {}", self.path.display());
        self.summary().ok_or_else(|| Error::Tunnel("the tunnel was removed meanwhile".to_owned()))
    }

    /// Forgets the tunnel and its file.
    pub(crate) fn remove(&self) -> Result<(), Error> {
        *self.lock() = State::default();
        match fs::remove_file(&self.path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error.into()),
            _ => {
                log::info!("Leto's WireGuard tunnel was removed");
                Ok(())
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl State {
    /// The tunnel, opened if it is not yet.
    fn tunnel(&mut self) -> Option<Arc<Tunnel>> {
        if let Some(tunnel) = &self.tunnel {
            return Some(Arc::clone(tunnel));
        }
        if self.failed.is_some_and(|failed| failed.elapsed() < RETRY) {
            return None;
        }
        let config = self.config.clone()?;
        let endpoint = config.endpoint.clone().unwrap_or_else(|| "nowhere yet".to_owned());
        match Tunnel::start(config) {
            Ok(tunnel) => {
                log::info!("opened the WireGuard tunnel to {endpoint}");
                let tunnel = Arc::new(tunnel);
                self.tunnel = Some(Arc::clone(&tunnel));
                self.problem = None;
                self.failed = None;
                Some(tunnel)
            }
            Err(error) => {
                log::warn!("could not open the WireGuard tunnel to {endpoint}: {error}");
                self.problem = Some(format!("{endpoint} cannot be reached: {error}"));
                self.failed = Some(Instant::now());
                None
            }
        }
    }
}

/// An address a door may lead to: not one of this device's own.
/// Whether `target` is inside the tunnel `config` describes: in one of the
/// ranges it covers that also holds this device's own address in it, as
/// `10.8.0.0/24` for `10.8.0.2`. A range wider than `/16`, as `0.0.0.0/0`
/// that sends everything through, says nothing of the tunnel's own
/// addresses: then those that share this device's first three numbers are.
fn inside(config: &Config, target: Ipv4Addr) -> bool {
    let own = config.address;
    if target == own || !usable(target) {
        return false;
    }
    let ranges = config.allowed.iter().filter(|range| range.prefix >= 16 && range.contains(own));
    let mut ranges = ranges.peekable();
    if ranges.peek().is_none() {
        return own.octets()[..3] == target.octets()[..3] && config.covers(IpAddr::V4(target));
    }
    ranges.any(|range| range.contains(target))
}

fn usable(target: Ipv4Addr) -> bool {
    !(target.is_loopback() || target.is_unspecified() || target.is_broadcast())
}

/// The TCP ports of a PC whose own port is `port`, as Sunshine and Apollo
/// place them: HTTPS, HTTP, Leto's API and RTSP. The main port first.
fn tcp_ports(port: u16) -> Vec<u16> {
    [Some(port), port.checked_sub(5), port.checked_add(13), port.checked_add(21)]
        .into_iter()
        .flatten()
        .collect()
}

/// Its UDP ports: the ones a sleeping PC's card listens on, then video,
/// control, sound and the microphone or the camera.
fn udp_ports(port: u16) -> Vec<u16> {
    let moved = [9, 10, 11, 13].into_iter().filter_map(|offset| port.checked_add(offset));
    [9, 47009].into_iter().chain(moved).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_door_lets_through_the_ports_apollo_uses() {
        assert_eq!(tcp_ports(47989), [47989, 47984, 48002, 48010]);
        assert_eq!(udp_ports(47989), [9, 47009, 47998, 47999, 48000, 48002]);
        assert_eq!(tcp_ports(65530), [65530, 65525]);
    }

    fn config(allowed: &str) -> Config {
        let text = format!(
            "[Interface]\nPrivateKey = {key}\nAddress = 10.8.0.2/24\n\n[Peer]\nPublicKey = \
             {key}\nAllowedIPs = {allowed}\nEndpoint = home.example.org:51820\n",
            key = "YMx4QwN5c1EG5dFl8uKCC4Rzl6QpTwP2O4j9Gs8VW2I="
        );
        Config::parse(&text).expect("a usable file")
    }

    #[test]
    fn a_pc_inside_the_tunnel_is_told_from_one_at_home_behind_it() {
        let both = config("10.8.0.0/24, 192.168.1.0/24");
        assert!(inside(&both, Ipv4Addr::new(10, 8, 0, 3)));
        assert!(!inside(&both, Ipv4Addr::new(192, 168, 1, 15)));
        assert!(!inside(&both, Ipv4Addr::new(10, 8, 0, 2)), "this device's own");

        // Everything through the tunnel: its own addresses share three numbers.
        let everything = config("0.0.0.0/0");
        assert!(inside(&everything, Ipv4Addr::new(10, 8, 0, 3)));
        assert!(!inside(&everything, Ipv4Addr::new(192, 168, 1, 15)));

        // A tunnel that only leads home has no PC inside it.
        let home = config("192.168.1.0/24");
        assert!(!inside(&home, Ipv4Addr::new(10, 8, 0, 3)));
    }

    #[test]
    fn no_file_no_tunnel_and_a_broken_one_says_why() {
        let directory = std::env::temp_dir().join(format!("leto-ways-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("a folder");
        let ways = Ways::open(&directory);
        assert_eq!(ways.summary(), None);
        assert_eq!(ways.door("10.8.0.3", 47989), None);

        fs::write(directory.join(FILE_NAME), "[Interface]\nAddress = 10.8.0.2\n").expect("a file");
        let summary = Ways::open(&directory).summary().expect("the problem is told");
        assert!(summary.problem.expect("a problem").contains("PrivateKey"));
        assert!(matches!(ways.set("not a file"), Err(Error::Tunnel(_))));
        let _ = fs::remove_dir_all(&directory);
    }
}
