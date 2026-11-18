//! The certificate the bridge shows browsers.
//!
//! A browser only lets a page decode video, or use a microphone and a
//! camera, when the page came over HTTPS. No authority vouches for an
//! address on a home network, so the bridge vouches for itself: the browser
//! warns the first time, and the person accepts once. The certificate is
//! kept between runs so that once stays once.

use std::{
    fs, io,
    net::{IpAddr, UdpSocket},
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use leto_client::store::write_private;
use rcgen::{CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, KeyPair};
use ring::digest;
use rustls::{
    ServerConfig,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject as _},
};
use serde::{Deserialize, Serialize};

/// The file that keeps the certificate and its key.
const FILE_NAME: &str = "certificate.json";

/// How long a certificate is good for. Some systems refuse longer ones.
const VALID_FOR: Duration = Duration::from_secs(730 * 24 * 3600);

/// A certificate this close to its end is replaced when the bridge starts.
const RENEWED_BEFORE: Duration = Duration::from_secs(30 * 24 * 3600);

/// A certificate is dated a little in the past, for devices whose clock is
/// behind.
const BACKDATED: Duration = Duration::from_secs(24 * 3600);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    /// The certificate, as PEM.
    pub certificate: String,
    /// Its private key, as PEM.
    private_key: String,
    /// When the certificate stops being good, in seconds since 1970.
    expires: u64,
}

impl Identity {
    /// The identity kept in `directory`, or a new one when there is none or
    /// the old one is about to run out.
    pub fn load_or_create(directory: &Path, now: SystemTime) -> io::Result<Self> {
        let file = directory.join(FILE_NAME);
        match fs::read(&file) {
            Ok(bytes) => {
                let saved: Self = serde_json::from_slice(&bytes)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                if saved.good_at(now + RENEWED_BEFORE) {
                    return Ok(saved);
                }
                log::info!("the bridge's certificate is running out: making a new one");
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }

        let identity = Self::create(&names(), now)?;
        fs::create_dir_all(directory)?;
        let json = serde_json::to_vec_pretty(&identity)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let temporary = file.with_extension("json.tmp");
        write_private(&temporary, &json)?;
        fs::rename(&temporary, &file)?;
        Ok(identity)
    }

    /// A new certificate for a bridge reached under `names`: host names and
    /// addresses.
    fn create(names: &[String], now: SystemTime) -> io::Result<Self> {
        let failed = |error: rcgen::Error| io::Error::other(error.to_string());
        let mut params = CertificateParams::new(names.to_vec()).map_err(failed)?;
        let mut name = DistinguishedName::new();
        name.push(DnType::CommonName, "Leto bridge");
        params.distinguished_name = name;
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.not_before = date(now - BACKDATED)?;
        params.not_after = date(now + VALID_FOR)?;
        let key = KeyPair::generate().map_err(failed)?;
        let certificate = params.self_signed(&key).map_err(failed)?;
        Ok(Self {
            certificate: certificate.pem(),
            private_key: key.serialize_pem(),
            expires: seconds(now + VALID_FOR),
        })
    }

    fn good_at(&self, time: SystemTime) -> bool {
        seconds(time) < self.expires
    }

    /// What a browser shows as the certificate's SHA-256 fingerprint, for
    /// whoever wants to check that the warning is about this bridge.
    pub fn fingerprint(&self) -> io::Result<String> {
        let certificate = self.der()?;
        let hash = digest::digest(&digest::SHA256, &certificate);
        let pairs: Vec<String> = hash.as_ref().iter().map(|byte| format!("{byte:02X}")).collect();
        Ok(pairs.join(":"))
    }

    fn der(&self) -> io::Result<CertificateDer<'static>> {
        CertificateDer::from_pem_slice(self.certificate.as_bytes())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))
    }

    /// How the bridge answers HTTPS with this identity.
    pub fn tls(&self) -> io::Result<Arc<ServerConfig>> {
        let invalid = |error: String| io::Error::new(io::ErrorKind::InvalidData, error);
        let key = PrivateKeyDer::from_pem_slice(self.private_key.as_bytes())
            .map_err(|error| invalid(error.to_string()))?;
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|error| invalid(error.to_string()))?
            .with_no_client_auth()
            .with_single_cert(vec![self.der()?], key)
            .map_err(|error| invalid(error.to_string()))?;
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Arc::new(config))
    }
}

fn seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

fn date(time: SystemTime) -> io::Result<time::OffsetDateTime> {
    let seconds = i64::try_from(seconds(time)).map_err(io::Error::other)?;
    time::OffsetDateTime::from_unix_timestamp(seconds).map_err(io::Error::other)
}

/// The names this computer is reached under, as far as it knows them.
fn names() -> Vec<String> {
    let mut names = vec!["localhost".to_owned(), "127.0.0.1".to_owned()];
    let host = computer_name();
    // A certificate only takes names made of these.
    let plain =
        host.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.');
    if plain && !host.is_empty() && host != "localhost" {
        names.push(format!("{host}.local"));
        names.push(host);
    }
    if let Some(address) = local_address() {
        names.push(address.to_string());
    }
    names
}

/// What this computer calls itself.
pub fn computer_name() -> String {
    gethostname::gethostname().to_string_lossy().trim().to_owned()
}

/// The address other devices of the network reach this computer at, when it
/// has one.
pub fn local_address() -> Option<IpAddr> {
    // Asking the system which of its addresses leads out says it. Nothing
    // is sent: the destination is an address reserved for documentation.
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let address = socket.local_addr().ok()?.ip();
    (!address.is_unspecified() && !address.is_loopback()).then_some(address)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn folder(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("leto-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        path
    }

    #[test]
    fn the_certificate_is_made_once_and_kept() {
        let folder = folder("identity-kept");
        let now = SystemTime::now();
        let first = Identity::load_or_create(&folder, now).unwrap();
        let again = Identity::load_or_create(&folder, now + Duration::from_secs(3600)).unwrap();
        assert_eq!(first.certificate, again.certificate);
        // It can answer HTTPS, and it has a fingerprint to read out.
        first.tls().unwrap();
        let fingerprint = first.fingerprint().unwrap();
        assert_eq!(fingerprint.len(), 32 * 3 - 1);
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_certificate_about_to_run_out_is_replaced() {
        let folder = folder("identity-renewed");
        let now = SystemTime::now();
        let first = Identity::load_or_create(&folder, now).unwrap();
        let later = now + VALID_FOR - RENEWED_BEFORE + Duration::from_secs(1);
        let renewed = Identity::load_or_create(&folder, later).unwrap();
        assert_ne!(first.certificate, renewed.certificate);
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_damaged_file_is_an_error_not_a_silent_new_certificate() {
        let folder = folder("identity-damaged");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join(FILE_NAME), "{").unwrap();
        assert!(Identity::load_or_create(&folder, SystemTime::now()).is_err());
        let _ = fs::remove_dir_all(&folder);
    }
}
