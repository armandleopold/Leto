//! The platforms Leto targets and what each one can do.

use std::fmt;

use serde::Serialize;

/// A platform Leto can run on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Windows,
    Linux,
    MacOs,
    Android,
    Ios,
    Web,
}

impl Platform {
    /// Every platform Leto targets.
    pub const ALL: [Self; 6] =
        [Self::Windows, Self::Linux, Self::MacOs, Self::Android, Self::Ios, Self::Web];

    /// The platform this binary was compiled for, or `None` on a platform
    /// Leto does not target.
    pub const fn current() -> Option<Self> {
        if cfg!(target_arch = "wasm32") {
            Some(Self::Web)
        } else if cfg!(target_os = "windows") {
            Some(Self::Windows)
        } else if cfg!(target_os = "android") {
            Some(Self::Android)
        } else if cfg!(target_os = "linux") {
            Some(Self::Linux)
        } else if cfg!(target_os = "macos") {
            Some(Self::MacOs)
        } else if cfg!(target_os = "ios") {
            Some(Self::Ios)
        } else {
            None
        }
    }

    /// Whether this platform can share its screen, that is, act as the host.
    ///
    /// Every platform can be a client. Only desktop systems can be a host.
    pub const fn can_host(self) -> bool {
        matches!(self, Self::Windows | Self::Linux | Self::MacOs)
    }

    /// Human-readable name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Windows => "Windows",
            Self::Linux => "Linux",
            Self::MacOs => "macOS",
            Self::Android => "Android",
            Self::Ios => "iOS",
            Self::Web => "Web",
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_desktop_systems_can_host() {
        let hosts: Vec<_> = Platform::ALL.into_iter().filter(|p| p.can_host()).collect();
        assert_eq!(hosts, [Platform::Windows, Platform::Linux, Platform::MacOs]);
    }

    #[test]
    fn the_build_platform_is_known() {
        assert!(Platform::current().is_some(), "tests run on an untargeted platform");
    }
}
