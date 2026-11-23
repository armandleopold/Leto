//! What an Apollo or a Sunshine installed on this PC knows, read from its
//! own folder and never written.
//!
//! A Leto host that runs beside one of them shows the same certificate and
//! takes the same devices: nobody pairs twice. That takes three things from
//! the folder where Apollo keeps its settings: its certificate and its
//! private key (`credentials/cacert.pem` and `credentials/cakey.pem`), and
//! the devices it is paired with (`sunshine_state.json`). `sunshine.conf`
//! may move each of them, and says the port when it is not 47989. Read in
//! `config.cpp` and `nvhttp.cpp` of Apollo, at commit `adc5c5a`.
//!
//! The private key stays in memory, inside the TLS setup made from it. It
//! is never logged and never copied anywhere.

use std::{
    env, fmt, fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use rustls::{
    DigitallySignedStruct, DistinguishedName, ServerConfig, SignatureScheme,
    client::danger::HandshakeSignatureValid,
    crypto::{WebPkiSupportedAlgorithms, verify_tls12_signature, verify_tls13_signature},
    pki_types::{
        CertificateDer, PrivateKeyDer, PrivatePkcs1KeyDer, PrivatePkcs8KeyDer, PrivateSec1KeyDer,
        UnixTime,
    },
    server::danger::{ClientCertVerified, ClientCertVerifier},
};
use serde_json::Value;

/// The port of an Apollo whose settings name none.
pub const DEFAULT_PORT: u16 = 47989;

/// What Apollo lets a device do, as it keeps it for each one (`PERM` in its
/// `crypto.h`): the two that say the owner of the PC trusts the device with
/// the PC itself.
const MOUSE: u32 = 1 << 11;
const KEYBOARD: u32 = 1 << 12;

/// Every permission: what a device has when nothing says less, as on a
/// Sunshine, which keeps none.
const EVERYTHING: u32 = u32::MAX;

/// Every permission Apollo has a word for (`_all` in its `crypto.h`).
pub const EVERY_RIGHT: u32 = 0x071F_1F00;

/// One thing Apollo lets a device do, or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Right {
    /// Where Apollo keeps it among a device's permissions.
    pub bit: u32,
    pub name: &'static str,
    /// The permissions that bring this one with them: a device that may
    /// start apps may watch a stream, and one that may watch may list.
    pub comes_with: u32,
}

impl Right {
    /// Whether a device with these `permissions` has this one, said or
    /// brought by another.
    pub const fn held_by(&self, permissions: u32) -> bool {
        permissions & (self.bit | self.comes_with) != 0
    }

    /// Whether another of these `permissions` brings this one, so that
    /// taking it away alone changes nothing.
    pub const fn brought_to(&self, permissions: u32) -> bool {
        permissions & self.comes_with != 0
    }
}

const LIST: u32 = 1 << 24;
const VIEW: u32 = 1 << 25;
const LAUNCH: u32 = 1 << 26;

/// What Apollo lets the owner of the PC decide for each device, in the
/// groups and the order of its own page (`pin.html`).
pub const RIGHTS: [(&str, &[Right]); 3] = [
    (
        "Apps",
        &[
            Right { bit: LIST, name: "List the apps", comes_with: VIEW | LAUNCH },
            Right { bit: VIEW, name: "Watch a stream", comes_with: LAUNCH },
            Right { bit: LAUNCH, name: "Start apps", comes_with: 0 },
        ],
    ),
    (
        "On the PC",
        &[
            Right { bit: 1 << 16, name: "Write the clipboard", comes_with: 0 },
            Right { bit: 1 << 17, name: "Read the clipboard", comes_with: 0 },
            Right { bit: 1 << 20, name: "Run commands", comes_with: 0 },
        ],
    ),
    (
        "Input",
        &[
            Right { bit: 1 << 8, name: "Gamepad", comes_with: 0 },
            Right { bit: 1 << 9, name: "Touch", comes_with: 0 },
            Right { bit: 1 << 10, name: "Pen", comes_with: 0 },
            Right { bit: MOUSE, name: "Mouse", comes_with: 0 },
            Right { bit: KEYBOARD, name: "Keyboard", comes_with: 0 },
        ],
    ),
];

/// Why what Apollo keeps could not be read.
#[derive(Debug)]
pub enum Error {
    /// No folder of an Apollo or a Sunshine where they usually are.
    NotFound,
    /// A file of the folder could not be read: which one, and why.
    Unreadable(PathBuf, io::Error),
    /// A file was read and is not what Apollo writes: which one.
    Unexpected(PathBuf),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "no Apollo or Sunshine was found on this computer"),
            Self::Unreadable(path, error) => {
                write!(f, "{} could not be read: {error}", path.display())
            }
            Self::Unexpected(path) => {
                write!(f, "{} is not what Apollo or Sunshine writes", path.display())
            }
        }
    }
}

impl std::error::Error for Error {}

/// A device Apollo is paired with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// What Apollo knows it by. Empty for a device of an old Sunshine.
    pub id: String,
    /// The name its owner gave it on Apollo's pages.
    pub name: String,
    /// Its certificate, DER encoded: what it shows when it connects.
    pub certificate: Vec<u8>,
    /// What Apollo lets it do.
    pub permissions: u32,
}

impl Device {
    /// Whether the owner of the PC lets this device use its keyboard and its
    /// mouse on the PC. A Leto host beside Apollo takes the camera of such
    /// a device, and of no other: Apollo has no word for cameras, and this
    /// is how its owner says which devices are trusted with the PC.
    pub fn trusted(&self) -> bool {
        self.permissions & (MOUSE | KEYBOARD) == MOUSE | KEYBOARD
    }
}

/// An Apollo or a Sunshine installed on this PC.
pub struct Apollo {
    folder: PathBuf,
    port: u16,
    state: PathBuf,
    /// Apollo's certificate, DER encoded.
    certificate: Vec<u8>,
    tls: Arc<ServerConfig>,
}

impl fmt::Debug for Apollo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Apollo").field("folder", &self.folder).field("port", &self.port).finish()
    }
}

impl Apollo {
    /// Reads what the Apollo or the Sunshine installed on this computer
    /// keeps, from where they usually keep it.
    pub fn find() -> Result<Self, Error> {
        let folder = usual_folders()
            .into_iter()
            .find(|folder| folder.join("sunshine_state.json").is_file())
            .ok_or(Error::NotFound)?;
        Self::read(&folder)
    }

    /// Reads what an Apollo or a Sunshine keeps in `folder`: the one with
    /// its `sunshine.conf` and `sunshine_state.json`.
    pub fn read(folder: &Path) -> Result<Self, Error> {
        let settings = Settings::read(&folder.join("sunshine.conf"));
        let place = |named: Option<&String>, usual: &str| match named {
            Some(named) => folder.join(named),
            None => folder.join(usual),
        };
        let certificate_file = place(settings.certificate.as_ref(), "credentials/cacert.pem");
        let key_file = place(settings.key.as_ref(), "credentials/cakey.pem");
        let state = place(settings.state.as_ref(), "sunshine_state.json");

        let certificate = read_pem(&certificate_file)?;
        if certificate.tag() != "CERTIFICATE" {
            return Err(Error::Unexpected(certificate_file));
        }
        let key = read_pem(&key_file)?;
        let key = match key.tag() {
            "PRIVATE KEY" => PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.into_contents())),
            "RSA PRIVATE KEY" => {
                PrivateKeyDer::Pkcs1(PrivatePkcs1KeyDer::from(key.into_contents()))
            }
            "EC PRIVATE KEY" => PrivateKeyDer::Sec1(PrivateSec1KeyDer::from(key.into_contents())),
            _ => return Err(Error::Unexpected(key_file)),
        };
        let certificate = certificate.into_contents();

        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = AnyDevice { algorithms: provider.signature_verification_algorithms };
        let tls = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|_| Error::Unexpected(key_file.clone()))?
            .with_client_cert_verifier(Arc::new(verifier))
            .with_single_cert(vec![CertificateDer::from(certificate.clone())], key)
            .map_err(|_| Error::Unexpected(key_file))?;

        let apollo = Self {
            folder: folder.to_owned(),
            port: settings.port.unwrap_or(DEFAULT_PORT),
            state,
            certificate,
            tls: Arc::new(tls),
        };
        // Read once now, so that a folder that is not an Apollo's is refused
        // here and not at the first device that connects.
        apollo.devices()?;
        Ok(apollo)
    }

    /// The folder all of this was read from.
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// The port Apollo counts its other ports from.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The port of Apollo's own pages: the one after its own.
    pub fn pages_port(&self) -> u16 {
        self.port.saturating_add(1)
    }

    /// The certificate Apollo shows, to its devices and on its pages, DER
    /// encoded.
    pub fn certificate(&self) -> &[u8] {
        &self.certificate
    }

    /// A TLS setup that shows Apollo's certificate and asks a device for its
    /// own. Any certificate passes the handshake, as on Apollo: whether it
    /// is a paired device's is looked up afterwards, in [`Self::devices`].
    pub fn tls(&self) -> Arc<ServerConfig> {
        Arc::clone(&self.tls)
    }

    /// The devices Apollo is paired with, as its file says now: a device
    /// paired or removed a moment ago is there, or gone.
    pub fn devices(&self) -> Result<Vec<Device>, Error> {
        let text =
            fs::read(&self.state).map_err(|error| Error::Unreadable(self.state.clone(), error))?;
        let unexpected = || Error::Unexpected(self.state.clone());
        let tree: Value = serde_json::from_slice(&text).map_err(|_| unexpected())?;
        let root = tree.get("root").ok_or_else(unexpected)?;

        let mut devices = Vec::new();
        let mut keep = |id: &str, name: &str, certificate: &str, permissions: u32| {
            // A certificate Apollo could not read back is no device.
            if let Ok(certificate) = pem::parse(certificate) {
                devices.push(Device {
                    id: id.to_owned(),
                    name: name.to_owned(),
                    certificate: certificate.into_contents(),
                    permissions,
                });
            }
        };
        // What Apollo and today's Sunshine write.
        for device in root.get("named_devices").and_then(Value::as_array).into_iter().flatten() {
            let text = |field: &str| device.get(field).and_then(Value::as_str).unwrap_or("");
            // Apollo reads a missing permission as every one, and so does a
            // Sunshine, which writes none.
            let permissions = device
                .get("perm")
                .and_then(Value::as_u64)
                .map_or(EVERYTHING, |permissions| permissions as u32);
            keep(text("uuid"), text("name"), text("cert"), permissions);
        }
        // What a Sunshine wrote before it named devices.
        for device in root.get("devices").and_then(Value::as_array).into_iter().flatten() {
            for certificate in device.get("certs").and_then(Value::as_array).into_iter().flatten() {
                keep("", "", certificate.as_str().unwrap_or(""), EVERYTHING);
            }
        }
        Ok(devices)
    }
}

/// Where an Apollo or a Sunshine usually keeps its settings.
fn usual_folders() -> Vec<PathBuf> {
    let mut folders = Vec::new();
    if cfg!(windows) {
        // Beside the program itself, in its `config` folder.
        for programs in ["ProgramW6432", "ProgramFiles"] {
            if let Some(programs) = env::var_os(programs) {
                for name in ["Apollo", "Sunshine"] {
                    folders.push(Path::new(&programs).join(name).join("config"));
                }
            }
        }
    } else if cfg!(target_os = "macos") {
        if let Some(home) = env::var_os("HOME") {
            for name in ["Apollo", "Sunshine"] {
                folders.push(Path::new(&home).join("Library/Application Support").join(name));
            }
        }
    } else {
        let settings = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| Path::new(&home).join(".config")));
        if let Some(settings) = settings {
            for name in ["apollo", "sunshine"] {
                folders.push(settings.join(name));
            }
        }
    }
    folders
}

/// What `sunshine.conf` says of the things read here.
#[derive(Default)]
struct Settings {
    port: Option<u16>,
    certificate: Option<String>,
    key: Option<String>,
    state: Option<String>,
}

impl Settings {
    /// A file that is missing, as on an Apollo nobody changed a setting of,
    /// says nothing.
    fn read(file: &Path) -> Self {
        let mut settings = Self::default();
        let Ok(text) = fs::read_to_string(file) else { return settings };
        for line in text.lines() {
            let Some((name, value)) = line.split_once('=') else { continue };
            let value = value.trim();
            if value.is_empty() {
                continue;
            }
            match name.trim() {
                "port" => settings.port = value.parse().ok(),
                "cert" => settings.certificate = Some(value.to_owned()),
                "pkey" => settings.key = Some(value.to_owned()),
                "file_state" => settings.state = Some(value.to_owned()),
                _ => {}
            }
        }
        settings
    }
}

fn read_pem(file: &Path) -> Result<pem::Pem, Error> {
    let text = fs::read(file).map_err(|error| Error::Unreadable(file.to_owned(), error))?;
    pem::parse(text).map_err(|_| Error::Unexpected(file.to_owned()))
}

/// Like Apollo, take any certificate during the TLS handshake and decide
/// afterwards, from the list of paired devices, whose it is.
#[derive(Debug)]
struct AnyDevice {
    algorithms: WebPkiSupportedAlgorithms,
}

impl ClientCertVerifier for AnyDevice {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_say_what_was_moved_and_nothing_else() {
        let folder = env::temp_dir().join(format!("leto-apollo-conf-{}", std::process::id()));
        fs::create_dir_all(&folder).unwrap();
        let file = folder.join("sunshine.conf");
        fs::write(
            &file,
            "sunshine_name = Salon\nport = 48989\npkey =\n file_state = other.json \n",
        )
        .unwrap();
        let settings = Settings::read(&file);
        assert_eq!(settings.port, Some(48989));
        assert_eq!(settings.state.as_deref(), Some("other.json"));
        assert!(settings.key.is_none() && settings.certificate.is_none());

        let none = Settings::read(&folder.join("missing.conf"));
        assert!(none.port.is_none() && none.state.is_none());
        fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn a_device_is_trusted_when_it_may_use_the_keyboard_and_the_mouse() {
        let device = |permissions| Device {
            id: String::new(),
            name: String::new(),
            certificate: vec![],
            permissions,
        };
        // What Apollo gives a new device: list the apps and watch.
        assert!(!device(0x0300_0000).trusted());
        assert!(!device(MOUSE).trusted());
        assert!(device(MOUSE | KEYBOARD).trusted());
        assert!(device(EVERYTHING).trusted());
    }

    #[test]
    fn a_right_is_held_when_said_or_brought_by_another() {
        let [(_, apps), (_, on_the_pc), (_, input)] = RIGHTS;
        let [list, view, launch] = apps else { panic!("three rights about apps") };
        // What Apollo gives a new device: list the apps and watch.
        let new = 0x0300_0000;
        assert!(list.held_by(new) && view.held_by(new) && !launch.held_by(new));
        assert!(list.brought_to(new) && !view.brought_to(new));
        // Starting apps brings the two others.
        assert!(list.held_by(LAUNCH) && view.held_by(LAUNCH) && view.brought_to(LAUNCH));
        assert!(!list.held_by(0) && !list.held_by(MOUSE));
        // Every right is one Apollo has a word for, and no two are the same.
        let all: Vec<u32> =
            apps.iter().chain(on_the_pc).chain(input).map(|right| right.bit).collect();
        assert_eq!(all.len(), 11);
        assert_eq!(all.iter().fold(0, |every, bit| every | bit) & !EVERY_RIGHT, 0);
        assert_eq!(all.iter().map(|bit| bit.count_ones()).sum::<u32>(), 11);
        assert_eq!(all.iter().fold(0, |every, bit| every | bit).count_ones(), 11);
    }
}
