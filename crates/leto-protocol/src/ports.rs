//! Network ports used by a Moonlight-compatible host.
//!
//! Every port is derived from a single configurable base port, exactly as
//! Sunshine and Apollo do, so one setting moves the whole set. The offsets
//! below match Apollo's sources (`nvhttp.h`, `confighttp.h`, `stream.h`,
//! `rtsp.h`); the microphone offset comes from the microphone-enabled Apollo
//! build (Apollo pull request 1428).

use std::fmt;

use serde::Serialize;

/// The base port used by Sunshine, Apollo and Leto unless configured otherwise.
pub const DEFAULT_BASE_PORT: u16 = 47989;

/// Transport protocol of a service.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Tcp,
    Udp,
}

impl fmt::Display for Transport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Tcp => "TCP",
            Self::Udp => "UDP",
        })
    }
}

/// A network service exposed by the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Service {
    /// Pairing and app list over TLS.
    Https,
    /// Server information over plain HTTP. This is the base port.
    Http,
    /// The host's configuration web UI.
    WebUi,
    /// Video stream.
    Video,
    /// Control stream: input, statistics, rumble, clipboard.
    Control,
    /// Audio stream, host to client.
    Audio,
    /// Microphone stream, client to host. An extension: only hosts that
    /// advertise it listen on this port.
    Microphone,
    /// RTSP session setup.
    Rtsp,
}

impl Service {
    /// Every service, in ascending port order.
    pub const ALL: [Self; 8] = [
        Self::Https,
        Self::Http,
        Self::WebUi,
        Self::Video,
        Self::Control,
        Self::Audio,
        Self::Microphone,
        Self::Rtsp,
    ];

    /// Offset of this service's port from the base port.
    pub const fn offset(self) -> i32 {
        match self {
            Self::Https => -5,
            Self::Http => 0,
            Self::WebUi => 1,
            Self::Video => 9,
            Self::Control => 10,
            Self::Audio => 11,
            Self::Microphone => 12,
            Self::Rtsp => 21,
        }
    }

    /// Transport protocol of this service.
    pub const fn transport(self) -> Transport {
        match self {
            Self::Https | Self::Http | Self::WebUi | Self::Rtsp => Transport::Tcp,
            Self::Video | Self::Control | Self::Audio | Self::Microphone => Transport::Udp,
        }
    }

    /// Whether the service is part of the baseline protocol every
    /// Moonlight-compatible host offers, as opposed to an extension.
    pub const fn is_baseline(self) -> bool {
        !matches!(self, Self::Microphone)
    }

    /// Short human-readable name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Https => "HTTPS",
            Self::Http => "HTTP",
            Self::WebUi => "Web UI",
            Self::Video => "Video",
            Self::Control => "Control",
            Self::Audio => "Audio",
            Self::Microphone => "Microphone",
            Self::Rtsp => "RTSP",
        }
    }
}

/// The base port leaves no room for the lowest or highest derived port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BasePortOutOfRange {
    /// The rejected base port.
    pub base: u16,
}

impl fmt::Display for BasePortOutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "base port {} is out of range: it must be between {} and {}",
            self.base,
            PortMap::MIN_BASE,
            PortMap::MAX_BASE
        )
    }
}

impl std::error::Error for BasePortOutOfRange {}

/// The set of ports derived from one base port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortMap {
    base: u16,
}

impl PortMap {
    /// Lowest accepted base port: keeps every derived port at 1024 or above.
    pub const MIN_BASE: u16 = 1024 + 5;

    /// Highest accepted base port: keeps every derived port at 65535 or below.
    pub const MAX_BASE: u16 = 65535 - 21;

    /// Builds the port map for `base`, rejecting a base that would push a
    /// derived port outside the unprivileged range.
    pub const fn new(base: u16) -> Result<Self, BasePortOutOfRange> {
        if base < Self::MIN_BASE || base > Self::MAX_BASE {
            Err(BasePortOutOfRange { base })
        } else {
            Ok(Self { base })
        }
    }

    /// The base port this map was built from.
    pub const fn base(self) -> u16 {
        self.base
    }

    /// The port of `service`.
    pub const fn port(self, service: Service) -> u16 {
        // `new` guarantees the sum stays within 1024..=65535.
        (self.base as i32 + service.offset()) as u16
    }
}

impl Default for PortMap {
    fn default() -> Self {
        Self { base: DEFAULT_BASE_PORT }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_ports_match_sunshine_and_apollo() {
        let ports = PortMap::default();
        assert_eq!(ports.port(Service::Https), 47984);
        assert_eq!(ports.port(Service::Http), 47989);
        assert_eq!(ports.port(Service::WebUi), 47990);
        assert_eq!(ports.port(Service::Video), 47998);
        assert_eq!(ports.port(Service::Control), 47999);
        assert_eq!(ports.port(Service::Audio), 48000);
        assert_eq!(ports.port(Service::Microphone), 48001);
        assert_eq!(ports.port(Service::Rtsp), 48010);
    }

    #[test]
    fn services_are_listed_in_ascending_port_order() {
        let offsets: Vec<_> = Service::ALL.iter().map(|s| s.offset()).collect();
        let mut sorted = offsets.clone();
        sorted.sort_unstable();
        assert_eq!(offsets, sorted);
    }

    #[test]
    fn the_accepted_range_keeps_every_port_unprivileged_and_valid() {
        let low = PortMap::new(PortMap::MIN_BASE).expect("lowest base is accepted");
        assert_eq!(low.port(Service::Https), 1024);
        let high = PortMap::new(PortMap::MAX_BASE).expect("highest base is accepted");
        assert_eq!(high.port(Service::Rtsp), 65535);
    }

    #[test]
    fn a_base_outside_the_range_is_rejected() {
        assert_eq!(PortMap::new(1028), Err(BasePortOutOfRange { base: 1028 }));
        assert_eq!(PortMap::new(65515), Err(BasePortOutOfRange { base: 65515 }));
        assert!(PortMap::new(80).is_err());
    }

    #[test]
    fn only_the_microphone_is_an_extension() {
        let extensions: Vec<_> = Service::ALL.into_iter().filter(|s| !s.is_baseline()).collect();
        assert_eq!(extensions, [Service::Microphone]);
    }
}
