//! What a host keeps from one launch to the next: who it is, and the
//! devices it is paired with.
//!
//! Three files in one folder. `host-key.pem` is the host's private key,
//! which only the person who shares the PC may read. `host-cert.pem` is the
//! certificate that goes with it, the one every paired device remembers:
//! lose it and every device has to pair again. `host.json` holds the id the
//! host gives itself and the certificates of the paired devices.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

const KEY: &str = "host-key.pem";
const CERTIFICATE: &str = "host-cert.pem";
const STATE: &str = "host.json";

/// What `host.json` holds.
#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    /// The id the host reports. Clients tell hosts apart by it.
    #[serde(default)]
    id: String,
    /// The certificates of the paired devices, as PEM.
    #[serde(default)]
    devices: Vec<String>,
}

/// The folder a host keeps its files in.
#[derive(Debug, Clone)]
pub(crate) struct Store {
    folder: PathBuf,
}

/// What a host that ran before left behind.
pub(crate) struct Kept {
    pub certificate: pem::Pem,
    pub key: pem::Pem,
    pub id: String,
    /// The certificates of the paired devices, DER encoded.
    pub devices: Vec<Vec<u8>>,
}

impl Store {
    pub fn new(folder: &Path) -> Self {
        Self { folder: folder.to_owned() }
    }

    /// What was kept, or `None` for a host that starts for the first time.
    /// Files that are there and cannot be read are an error: starting over
    /// would silently unpair every device.
    pub fn load(&self) -> io::Result<Option<Kept>> {
        let key = match fs::read_to_string(self.folder.join(KEY)) {
            Ok(key) => key,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let certificate = fs::read_to_string(self.folder.join(CERTIFICATE))?;
        let state = match fs::read(self.folder.join(STATE)) {
            Ok(state) => serde_json::from_slice(&state).map_err(io::Error::other)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => State::default(),
            Err(error) => return Err(error),
        };
        let State { id, devices } = state;
        // Never the text of a file in an error: one of them is a key.
        let unreadable = |name: &str| io::Error::other(format!("{name} is not a PEM file"));
        Ok(Some(Kept {
            certificate: pem::parse(certificate).map_err(|_| unreadable(CERTIFICATE))?,
            key: pem::parse(key).map_err(|_| unreadable(KEY))?,
            id,
            devices: devices
                .iter()
                .filter_map(|device| pem::parse(device).ok())
                .map(|device| device.into_contents())
                .collect(),
        }))
    }

    /// Keeps who the host is. Done once, when it first starts.
    pub fn keep_identity(&self, certificate: &pem::Pem, key: &pem::Pem) -> io::Result<()> {
        fs::create_dir_all(&self.folder)?;
        write_private(&self.folder.join(KEY), pem::encode(key).as_bytes())?;
        fs::write(self.folder.join(CERTIFICATE), pem::encode(certificate))
    }

    /// Keeps the id and the paired devices, whenever they change.
    pub fn keep_devices(&self, id: &str, devices: &[Vec<u8>]) -> io::Result<()> {
        let state = State {
            id: id.to_owned(),
            devices: devices
                .iter()
                .map(|device| pem::encode(&pem::Pem::new("CERTIFICATE", device.clone())))
                .collect(),
        };
        let text = serde_json::to_vec_pretty(&state).map_err(io::Error::other)?;
        fs::create_dir_all(&self.folder)?;
        // Whole or not at all: a file cut short would unpair every device.
        let draft = self.folder.join(format!("{STATE}.new"));
        fs::write(&draft, text)?;
        fs::rename(draft, self.folder.join(STATE))
    }
}

/// Writes a file that only its owner may read, where the system has such a
/// thing. On Windows it is as private as the folder it is in.
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write as _;

    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("leto-host-store-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        folder
    }

    #[test]
    fn a_host_finds_again_who_it_is_and_its_devices() {
        let folder = folder("again");
        let store = Store::new(&folder);
        assert!(store.load().expect("an empty folder reads").is_none());

        let certificate = pem::Pem::new("CERTIFICATE", vec![1, 2, 3]);
        let key = pem::Pem::new("PRIVATE KEY", vec![4, 5, 6]);
        store.keep_identity(&certificate, &key).expect("the identity is kept");
        // Without devices yet, the identity alone is enough.
        let kept = store.load().expect("the folder reads").expect("a host was here");
        assert_eq!((kept.certificate, kept.key), (certificate, key));
        assert!(kept.id.is_empty() && kept.devices.is_empty());

        store.keep_devices("an id", &[vec![7, 8], vec![9]]).expect("the devices are kept");
        let kept = store.load().expect("the folder reads").expect("a host was here");
        assert_eq!(kept.id, "an id");
        assert_eq!(kept.devices, [vec![7, 8], vec![9]]);
        let _ = fs::remove_dir_all(&folder);
    }

    #[cfg(unix)]
    #[test]
    fn only_the_owner_reads_the_key() {
        use std::os::unix::fs::PermissionsExt as _;

        let folder = folder("private");
        let store = Store::new(&folder);
        let certificate = pem::Pem::new("CERTIFICATE", vec![1]);
        store.keep_identity(&certificate, &pem::Pem::new("PRIVATE KEY", vec![2])).expect("kept");
        let mode = fs::metadata(folder.join(KEY)).expect("the key is there").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_damaged_key_is_an_error_that_does_not_quote_it() {
        let folder = folder("damaged");
        fs::create_dir_all(&folder).expect("a folder");
        fs::write(folder.join(KEY), "secret words that are no key").expect("written");
        fs::write(folder.join(CERTIFICATE), "neither is this").expect("written");
        let error = Store::new(&folder).load().err().expect("nothing to start from");
        assert!(!error.to_string().contains("secret"), "{error}");
        let _ = fs::remove_dir_all(&folder);
    }
}
