//! The WireGuard file a person already has, in the format of `wg-quick`,
//! which the WireGuard apps and servers such as wg-easy write.

use std::{
    fmt,
    net::{IpAddr, Ipv4Addr},
    str::FromStr,
};

use base64::{Engine, engine::general_purpose::STANDARD};

/// What most WireGuard setups use, for an IPv4 or IPv6 path of 1500 bytes.
pub const DEFAULT_MTU: u16 = 1420;

/// A range of IPv4 addresses, as `10.8.0.0/24`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub address: Ipv4Addr,
    pub prefix: u8,
}

impl Range {
    pub fn contains(&self, ip: Ipv4Addr) -> bool {
        let mask = u32::MAX.checked_shl(32 - u32::from(self.prefix)).unwrap_or(0);
        u32::from(ip) & mask == u32::from(self.address) & mask
    }
}

impl FromStr for Range {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, ()> {
        let (address, prefix) = text.split_once('/').unwrap_or((text, "32"));
        let address = address.trim().parse().map_err(|_| ())?;
        let prefix = prefix.trim().parse().map_err(|_| ())?;
        if prefix > 32 {
            return Err(());
        }
        Ok(Self { address, prefix })
    }
}

impl fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.address, self.prefix)
    }
}

/// What is wrong with a WireGuard file. Never says a key.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("line {line}: {what}")]
    /// `what` is never what the file says: a damaged line can hold a key.
    Line { line: usize, what: &'static str },
    #[error("the file has no {0}")]
    Missing(&'static str),
    #[error("Leto takes a tunnel to one peer, and the file has {0}")]
    Peers(usize),
    #[error("the file gives this device no IPv4 address in the tunnel")]
    NoAddress,
    #[error("the tunnel leads to no IPv4 address: AllowedIPs has none")]
    NoRoute,
}

/// One tunnel: this device's end, and the one peer it leads to.
#[derive(Clone, PartialEq, Eq)]
pub struct Config {
    pub private_key: [u8; 32],
    /// This device's address inside the tunnel. Leto uses IPv4 inside it.
    pub address: Ipv4Addr,
    pub mtu: u16,
    /// Where this end listens, when the other one comes to it.
    pub listen_port: Option<u16>,
    pub peer_public_key: [u8; 32],
    pub preshared_key: Option<[u8; 32]>,
    /// The peer's address and port on the internet, as `home.example.org:51820`.
    /// None when the peer comes to this end.
    pub endpoint: Option<String>,
    /// The addresses the tunnel leads to.
    pub allowed: Vec<Range>,
    /// Seconds between two signs of life, which keep a router's door open.
    pub keepalive: Option<u16>,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Not the keys: a log must never tell them.
        f.debug_struct("Config")
            .field("address", &self.address)
            .field("mtu", &self.mtu)
            .field("listen_port", &self.listen_port)
            .field("endpoint", &self.endpoint)
            .field("allowed", &self.allowed)
            .field("keepalive", &self.keepalive)
            .finish_non_exhaustive()
    }
}

impl Config {
    /// Reads a WireGuard file. IPv6 inside the tunnel and what only the
    /// system's WireGuard does with the file (DNS, scripts, routing tables)
    /// are left out.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let mut section = Section::None;
        let mut peers = 0;
        let mut private_key = None;
        let mut address = None;
        let mut mtu = DEFAULT_MTU;
        let mut listen_port = None;
        let mut peer_public_key = None;
        let mut preshared_key = None;
        let mut endpoint = None;
        let mut allowed = Vec::new();
        let mut keepalive = None;

        for (index, line) in text.lines().enumerate() {
            let line_number = index + 1;
            let wrong = |what: &'static str| ConfigError::Line { line: line_number, what };
            let line = line.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with('[') {
                section = match line.to_ascii_lowercase().as_str() {
                    "[interface]" => Section::Interface,
                    "[peer]" => {
                        peers += 1;
                        Section::Peer
                    }
                    // Said without the line: a file whose lines ran together
                    // has its keys on this one.
                    _ => return Err(wrong("a section that is not WireGuard's")),
                };
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(wrong("a line that is not `Name = value`"));
            };
            let (key, value) = (key.trim().to_ascii_lowercase(), value.trim());
            match (section, key.as_str()) {
                (Section::None, _) => {
                    return Err(wrong("a setting before [Interface] or [Peer]"));
                }
                (Section::Interface, "privatekey") => {
                    private_key = Some(read_key(value).ok_or_else(|| not_a_key("PrivateKey"))?);
                }
                (Section::Interface, "address") => {
                    // The first IPv4 one: IPv6 inside the tunnel is not used.
                    address = address
                        .or_else(|| value.split(',').find_map(|one| one.parse::<Range>().ok()));
                }
                (Section::Interface, "mtu") => {
                    mtu = value
                        .parse()
                        .ok()
                        .filter(|mtu| (576..=9000).contains(mtu))
                        .ok_or_else(|| wrong("an MTU Leto cannot use"))?;
                }
                (Section::Interface, "listenport") => {
                    listen_port =
                        Some(value.parse().map_err(|_| wrong("a ListenPort that is no port"))?);
                }
                (Section::Peer, _) if peers > 1 => {}
                (Section::Peer, "publickey") => {
                    peer_public_key = Some(read_key(value).ok_or_else(|| not_a_key("PublicKey"))?);
                }
                (Section::Peer, "presharedkey") => {
                    preshared_key = Some(read_key(value).ok_or_else(|| not_a_key("PresharedKey"))?);
                }
                (Section::Peer, "endpoint") => {
                    // Shown and logged later: nothing but a name and a port
                    // may get there from a line that ran into the next.
                    let named = value.rsplit_once(':').is_some_and(|(host, port)| {
                        port.parse::<u16>().is_ok()
                            && !host.is_empty()
                            && !host.contains(|c: char| c.is_whitespace() || c == '=')
                    });
                    if !named {
                        return Err(wrong("an Endpoint that is not `name:port`"));
                    }
                    endpoint = Some(value.to_owned());
                }
                (Section::Peer, "allowedips") => {
                    // IPv6 ranges are left out with the rest of IPv6.
                    allowed.extend(value.split(',').filter_map(|one| one.parse::<Range>().ok()));
                }
                (Section::Peer, "persistentkeepalive") => {
                    keepalive = match value {
                        "off" | "0" => None,
                        _ => Some(value.parse().map_err(|_| {
                            wrong("a PersistentKeepalive that is not a number of seconds")
                        })?),
                    };
                }
                // DNS, Table, PreUp, PostUp, FwMark and the like: what the
                // system's WireGuard does with the file, not a tunnel's.
                // The name is not logged: on a damaged line it can be a key.
                _ => log::debug!("line {line_number} of the WireGuard file is left out"),
            }
        }

        if peers > 1 {
            return Err(ConfigError::Peers(peers));
        }
        let config = Self {
            private_key: private_key.ok_or(ConfigError::Missing("PrivateKey"))?,
            address: address.ok_or(ConfigError::NoAddress)?.address,
            mtu,
            listen_port,
            peer_public_key: peer_public_key.ok_or(ConfigError::Missing("[Peer] PublicKey"))?,
            preshared_key,
            endpoint,
            allowed,
            keepalive,
        };
        if config.allowed.is_empty() {
            return Err(ConfigError::NoRoute);
        }
        Ok(config)
    }

    /// Whether the tunnel leads to `ip`.
    pub fn covers(&self, ip: IpAddr) -> bool {
        match ip {
            IpAddr::V4(ip) => self.allowed.iter().any(|range| range.contains(ip)),
            IpAddr::V6(_) => false,
        }
    }

    /// The file again, for a tunnel built in code, as the tests do.
    pub fn to_text(&self) -> String {
        let mut text = format!(
            "[Interface]\nPrivateKey = {}\nAddress = {}/32\nMTU = {}\n",
            STANDARD.encode(self.private_key),
            self.address,
            self.mtu
        );
        if let Some(port) = self.listen_port {
            text.push_str(&format!("ListenPort = {port}\n"));
        }
        text.push_str(&format!(
            "\n[Peer]\nPublicKey = {}\n",
            STANDARD.encode(self.peer_public_key)
        ));
        if let Some(key) = self.preshared_key {
            text.push_str(&format!("PresharedKey = {}\n", STANDARD.encode(key)));
        }
        if let Some(endpoint) = &self.endpoint {
            text.push_str(&format!("Endpoint = {endpoint}\n"));
        }
        let allowed: Vec<String> = self.allowed.iter().map(Range::to_string).collect();
        text.push_str(&format!("AllowedIPs = {}\n", allowed.join(", ")));
        if let Some(keepalive) = self.keepalive {
            text.push_str(&format!("PersistentKeepalive = {keepalive}\n"));
        }
        text
    }
}

#[derive(Debug, Clone, Copy)]
enum Section {
    None,
    Interface,
    Peer,
}

/// A key that is not one, said without what was written.
fn not_a_key(name: &str) -> ConfigError {
    ConfigError::Missing(match name {
        "PrivateKey" => "valid PrivateKey",
        "PublicKey" => "valid PublicKey",
        _ => "valid PresharedKey",
    })
}

/// A key as WireGuard writes it: 32 bytes in base64.
fn read_key(text: &str) -> Option<[u8; 32]> {
    STANDARD.decode(text).ok()?.try_into().ok()
}

/// The public key that goes with a private one, as WireGuard writes it.
pub fn public_key(private_key: [u8; 32]) -> String {
    let secret = boringtun::x25519::StaticSecret::from(private_key);
    STANDARD.encode(boringtun::x25519::PublicKey::from(&secret).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=";
    const PEER: &str = "xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=";

    fn wg_easy() -> String {
        format!(
            "[Interface]\nPrivateKey = {KEY}\nAddress = 10.8.0.2/24, fdcc:ad94:bacf:61a4::cafe:2/112\n\
             DNS = 1.1.1.1\nMTU = 1420\n\n[Peer]\nPublicKey = {PEER}\n\
             PresharedKey = {KEY}\nAllowedIPs = 0.0.0.0/0, ::/0\n\
             PersistentKeepalive = 25\nEndpoint = home.example.org:51820\n"
        )
    }

    #[test]
    fn a_file_from_wg_easy_is_read() {
        let config = Config::parse(&wg_easy()).expect("a valid file");
        assert_eq!(config.address, Ipv4Addr::new(10, 8, 0, 2));
        assert_eq!(config.mtu, 1420);
        assert_eq!(config.endpoint.as_deref(), Some("home.example.org:51820"));
        assert_eq!(config.allowed, [Range { address: Ipv4Addr::UNSPECIFIED, prefix: 0 }]);
        assert_eq!(config.keepalive, Some(25));
        assert!(config.preshared_key.is_some());
        assert!(config.covers("192.168.1.20".parse().expect("an address")));
        assert!(!config.covers("::1".parse().expect("an address")));
        // Read back the same.
        assert_eq!(Config::parse(&config.to_text()), Ok(config));
    }

    #[test]
    fn a_tunnel_covers_only_its_ranges() {
        let text = format!(
            "[Interface]\nPrivateKey={KEY}\nAddress=10.8.0.2/32\n[peer]\npublickey={PEER}\n\
             AllowedIPs = 10.8.0.0/24 , 192.168.1.0/24 # home\n"
        );
        let config = Config::parse(&text).expect("a valid file");
        assert_eq!(config.endpoint, None);
        assert_eq!(config.mtu, DEFAULT_MTU);
        for (ip, covered) in [("10.8.0.3", true), ("192.168.1.20", true), ("192.168.2.1", false)] {
            assert_eq!(config.covers(ip.parse().expect("an address")), covered, "{ip}");
        }
    }

    #[test]
    fn what_is_wrong_is_said_without_the_keys() {
        let missing = wg_easy().replace("PrivateKey", "# PrivateKey");
        assert_eq!(Config::parse(&missing), Err(ConfigError::Missing("PrivateKey")));
        let two = format!("{}\n[Peer]\nPublicKey = {PEER}\nAllowedIPs = 10.0.0.0/8\n", wg_easy());
        assert_eq!(Config::parse(&two), Err(ConfigError::Peers(2)));
        let six = wg_easy().replace("Address = 10.8.0.2/24, ", "Address = ");
        assert_eq!(Config::parse(&six), Err(ConfigError::NoAddress));
        let bad = wg_easy().replace(PEER, "not-a-key");
        let error = Config::parse(&bad).expect_err("a wrong key");
        assert_eq!(error, ConfigError::Missing("valid PublicKey"));
        let debug = format!("{:?}", Config::parse(&wg_easy()).expect("a valid file"));
        assert!(!debug.contains(KEY) && !debug.contains(PEER), "{debug}");
    }

    /// A file damaged on its way here, with its lines run together or cut
    /// anywhere, has its keys where anything is expected. Nothing of it is
    /// said back: the message goes to a window, a terminal and a log.
    #[test]
    fn a_damaged_file_is_refused_without_a_word_of_it() {
        let file = wg_easy();
        let mut damaged = vec![
            file.replace('\n', ""),
            file.replace('\n', " "),
            file.replace("[Interface]\n", "[Interface]"),
            file.replace("PrivateKey = ", "[").replace("[Peer]", "]"),
            file.replace("PrivateKey = ", "MTU = "),
            file.replace("PrivateKey = ", "ListenPort = "),
            file.replace("PublicKey = ", "PersistentKeepalive = "),
            file.replace("PrivateKey = ", ""),
            file.replace("[Interface]\n", ""),
        ];
        // Any two lines run together.
        let ends = file.match_indices('\n').map(|(at, _)| at);
        damaged.extend(ends.map(|at| format!("{}{}", &file[..at], &file[at + 1..])));
        // And cut after any character.
        damaged.extend((0..file.len()).filter(|&at| file.is_char_boundary(at)).map(|at| {
            let (start, end) = file.split_at(at);
            format!("{start}\n{end}")
        }));

        for text in damaged {
            let said = match Config::parse(&text) {
                // What a tunnel shows of its file: where the other end is.
                Ok(config) => format!("{config:?} {:?}", config.endpoint),
                Err(error) => format!("{error} {error:?}"),
            };
            for secret in [KEY, PEER] {
                // No piece of a key long enough to help guessing it.
                for piece in secret.as_bytes().windows(6) {
                    let piece = std::str::from_utf8(piece).expect("base64 is ASCII");
                    assert!(!said.contains(piece), "{said} says {piece}");
                }
            }
        }
    }
}
