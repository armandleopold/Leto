//! What the client remembers between runs: its own identity and its hosts.
//!
//! Everything lives in one JSON file. The private key in it is what the hosts
//! trust, so the file is created readable by its owner only where the system
//! allows that to be said.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use leto_core::model::{HostId, HostKind};
use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "client.json";

/// The certificate and private key this device presents to hosts, as PEM.
#[derive(Clone, Serialize, Deserialize)]
pub struct Identity {
    pub certificate: String,
    pub private_key: String,
}

/// A host the client keeps: one it paired with or one added by hand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedHost {
    pub id: HostId,
    pub name: String,
    pub address: String,
    pub port: u16,
    pub kind: HostKind,
    /// The host's certificate from pairing, as PEM. `None` until paired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate: Option<String>,
}

/// The content of the file.
#[derive(Default, Clone, Serialize, Deserialize)]
pub struct Saved {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    #[serde(default)]
    pub hosts: Vec<SavedHost>,
}

/// Reads and writes the file.
#[derive(Debug, Clone)]
pub struct Store {
    path: PathBuf,
}

impl Store {
    /// A store in `directory`. Nothing is read or created yet.
    pub fn in_directory(directory: &Path) -> Self {
        Self { path: directory.join(FILE_NAME) }
    }

    /// The file this store reads and writes.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads the file. A missing file is an empty store.
    pub fn load(&self) -> io::Result<Saved> {
        match fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Saved::default()),
            Err(error) => Err(error),
        }
    }

    /// Writes the file, replacing it in one step so a crash cannot leave half
    /// of it behind.
    pub fn save(&self, saved: &Saved) -> io::Result<()> {
        if let Some(directory) = self.path.parent() {
            fs::create_dir_all(directory)?;
        }
        let json = serde_json::to_vec_pretty(saved)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let temporary = self.path.with_extension("json.tmp");
        write_private(&temporary, &json)?;
        fs::rename(&temporary, &self.path)
    }
}

/// Writes a file only its owner can read, on systems with Unix permissions.
/// On Windows the file inherits the profile folder's protection.
pub fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
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

    fn temporary_directory(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("leto-store-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        directory
    }

    #[test]
    fn a_missing_file_is_an_empty_store() {
        let store = Store::in_directory(&temporary_directory("missing"));
        let saved = store.load().expect("a missing file loads");
        assert!(saved.identity.is_none());
        assert!(saved.hosts.is_empty());
    }

    #[test]
    fn what_is_saved_comes_back() {
        let directory = temporary_directory("roundtrip");
        let store = Store::in_directory(&directory);
        let host = SavedHost {
            id: HostId("abc".into()),
            name: "Salon".into(),
            address: "192.168.1.20".into(),
            port: 47989,
            kind: HostKind::Apollo,
            certificate: Some("PEM".into()),
        };
        let saved = Saved {
            identity: Some(Identity { certificate: "C".into(), private_key: "K".into() }),
            hosts: vec![host.clone()],
        };
        store.save(&saved).expect("the store saves");

        let loaded = store.load().expect("the store loads");
        assert_eq!(loaded.hosts, vec![host]);
        assert_eq!(loaded.identity.map(|identity| identity.private_key), Some("K".into()));
        let _ = fs::remove_dir_all(&directory);
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_private() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = temporary_directory("private");
        let store = Store::in_directory(&directory);
        store.save(&Saved::default()).expect("the store saves");
        let mode = fs::metadata(store.path()).expect("the file exists").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_damaged_file_is_an_error_not_a_silent_reset() {
        let directory = temporary_directory("damaged");
        fs::create_dir_all(&directory).expect("the directory is created");
        let store = Store::in_directory(&directory);
        fs::write(store.path(), b"{ not json").expect("the file is written");
        assert!(store.load().is_err());
        let _ = fs::remove_dir_all(&directory);
    }
}
