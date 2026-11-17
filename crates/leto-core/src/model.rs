//! The things the interface shows: hosts and their apps.
//!
//! These types carry no networking and compile on every platform, so every
//! shell (graphical app, command line, web) describes a PC the same way.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The port a host answers on unless its owner changed it.
pub const DEFAULT_HOST_PORT: u16 = 47989;

/// Identifies a host across address changes: the unique id it reports itself.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HostId(pub String);

impl fmt::Display for HostId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The software running on a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HostKind {
    Apollo,
    Sunshine,
    FoundationSunshine,
    /// Nvidia's discontinued GameStream service.
    GameStream,
}

impl HostKind {
    /// Name for people.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Apollo => "Apollo",
            Self::Sunshine => "Sunshine",
            Self::FoundationSunshine => "Foundation Sunshine",
            Self::GameStream => "GameStream",
        }
    }
}

/// Whether a host can be reached right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reachability {
    /// Not checked yet since the app started.
    Unknown,
    Online,
    Offline,
}

/// One PC, as shown in the list of PCs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostSummary {
    pub id: HostId,
    /// The name the PC gives itself.
    pub name: String,
    /// IP address or host name, without the port.
    pub address: String,
    pub port: u16,
    pub kind: HostKind,
    /// This device holds a pairing with the host.
    pub paired: bool,
    pub reachability: Reachability,
    /// An app is running on the host and a stream can be resumed.
    pub busy: bool,
}

/// One app a host offers to stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSummary {
    pub id: u32,
    pub title: String,
    pub hdr: bool,
}

/// The four digits typed on the host to approve a pairing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin([u8; 4]);

impl Pin {
    /// Builds a PIN from any four bytes by keeping the last digit of each.
    pub const fn from_bytes(bytes: [u8; 4]) -> Self {
        Self([bytes[0] % 10, bytes[1] % 10, bytes[2] % 10, bytes[3] % 10])
    }

    /// The four digits, each from 0 to 9.
    pub const fn digits(self) -> [u8; 4] {
        self.0
    }
}

/// The text was not four digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidPin;

impl fmt::Display for InvalidPin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a PIN is four digits")
    }
}

impl std::error::Error for InvalidPin {}

impl std::str::FromStr for Pin {
    type Err = InvalidPin;

    fn from_str(text: &str) -> Result<Self, InvalidPin> {
        let bytes: [u8; 4] = text.trim().as_bytes().try_into().map_err(|_| InvalidPin)?;
        if bytes.iter().all(u8::is_ascii_digit) {
            Ok(Self(bytes.map(|byte| byte - b'0')))
        } else {
            Err(InvalidPin)
        }
    }
}

impl fmt::Display for Pin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c, d] = self.0;
        write!(f, "{a}{b}{c}{d}")
    }
}

/// Splits `"host"`, `"host:port"`, `"[v6]"` or `"[v6]:port"` into address and
/// port. The port defaults to [`DEFAULT_HOST_PORT`]. Returns `None` for an
/// empty address or a port that is not a number.
pub fn parse_address(input: &str) -> Option<(String, u16)> {
    let input = input.trim();
    let (address, port) = if let Some(rest) = input.strip_prefix('[') {
        // Bracketed IPv6, with or without a port.
        let (address, after) = rest.split_once(']')?;
        match after.strip_prefix(':') {
            Some(port) => (address, Some(port)),
            None if after.is_empty() => (address, None),
            None => return None,
        }
    } else {
        match input.rsplit_once(':') {
            // More than one colon and no brackets: a bare IPv6 address.
            Some((before, _)) if before.contains(':') => (input, None),
            Some((address, port)) => (address, Some(port)),
            None => (input, None),
        }
    };
    if address.is_empty() || address.contains(char::is_whitespace) {
        return None;
    }
    let port = match port {
        Some(port) => port.parse().ok().filter(|port| *port != 0)?,
        None => DEFAULT_HOST_PORT,
    };
    Some((address.to_owned(), port))
}

/// Joins an address and a port the way URLs want it, with brackets for IPv6.
pub fn join_address(address: &str, port: u16) -> String {
    if address.contains(':') { format!("[{address}]:{port}") } else { format!("{address}:{port}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_without_port_gets_the_default() {
        assert_eq!(parse_address("192.168.1.20"), Some(("192.168.1.20".into(), 47989)));
        assert_eq!(parse_address("  salon-pc  "), Some(("salon-pc".into(), 47989)));
    }

    #[test]
    fn address_with_port() {
        assert_eq!(parse_address("192.168.1.20:48989"), Some(("192.168.1.20".into(), 48989)));
        assert_eq!(parse_address("pc.example.org:1234"), Some(("pc.example.org".into(), 1234)));
    }

    #[test]
    fn ipv6_addresses() {
        assert_eq!(parse_address("fe80::1"), Some(("fe80::1".into(), 47989)));
        assert_eq!(parse_address("[fe80::1]"), Some(("fe80::1".into(), 47989)));
        assert_eq!(parse_address("[fe80::1]:5000"), Some(("fe80::1".into(), 5000)));
    }

    #[test]
    fn rejects_nonsense() {
        assert_eq!(parse_address(""), None);
        assert_eq!(parse_address("   "), None);
        assert_eq!(parse_address("pc:"), None);
        assert_eq!(parse_address("pc:abc"), None);
        assert_eq!(parse_address("pc:0"), None);
        assert_eq!(parse_address("pc:70000"), None);
        assert_eq!(parse_address("my pc"), None);
        assert_eq!(parse_address("[fe80::1]x"), None);
    }

    #[test]
    fn a_pin_is_always_four_digits() {
        assert_eq!(Pin::from_bytes([0, 9, 10, 255]).to_string(), "0905");
        assert_eq!(Pin::from_bytes([1, 2, 3, 4]).digits(), [1, 2, 3, 4]);
    }

    #[test]
    fn a_pin_is_read_from_text() {
        assert_eq!("0905".parse::<Pin>().map(|pin| pin.digits()), Ok([0, 9, 0, 5]));
        assert_eq!(" 1234\n".parse::<Pin>().map(|pin| pin.to_string()), Ok("1234".into()));
        assert_eq!("123".parse::<Pin>(), Err(InvalidPin));
        assert_eq!("12345".parse::<Pin>(), Err(InvalidPin));
        assert_eq!("12a4".parse::<Pin>(), Err(InvalidPin));
        assert_eq!("１２３４".parse::<Pin>(), Err(InvalidPin));
    }

    #[test]
    fn joins_with_brackets_for_ipv6() {
        assert_eq!(join_address("192.168.1.20", 47989), "192.168.1.20:47989");
        assert_eq!(join_address("fe80::1", 47989), "[fe80::1]:47989");
    }
}
