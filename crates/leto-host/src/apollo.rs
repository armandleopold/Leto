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

        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = AnyDevice { algorithms: provider.signature_verification_algorithms };
        let tls = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|_| Error::Unexpected(key_file.clone()))?
            .with_client_cert_verifier(Arc::new(verifier))
            .with_single_cert(vec![CertificateDer::from(certificate.into_contents())], key)
            .map_err(|_| Error::Unexpected(key_file))?;

        let apollo = Self {
            folder: folder.to_owned(),
            port: settings.port.unwrap_or(DEFAULT_PORT),
            state,
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
        let mut keep = |name: &str, certificate: &str, permissions: u32| {
            // A certificate Apollo could not read back is no device.
            if let Ok(certificate) = pem::parse(certificate) {
                devices.push(Device {
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
            keep(text("name"), text("cert"), permissions);
        }
        // What a Sunshine wrote before it named devices.
        for device in root.get("devices").and_then(Value::as_array).into_iter().flatten() {
            for certificate in device.get("certs").and_then(Value::as_array).into_iter().flatten() {
                keep("", certificate.as_str().unwrap_or(""), EVERYTHING);
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
        let device = |permissions| Device { name: String::new(), certificate: vec![], permissions };
        // What Apollo gives a new device: list the apps and watch.
        assert!(!device(0x0300_0000).trusted());
        assert!(!device(MOUSE).trusted());
        assert!(device(MOUSE | KEYBOARD).trusted());
        assert!(device(EVERYTHING).trusted());
    }
}
