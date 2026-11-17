//! The client side of Leto: find hosts, pair with them, list their apps.
//!
//! This crate is the engine behind every native client shell. It speaks the
//! Moonlight protocol through `moonlight-common`, so it works with Sunshine
//! and Apollo hosts as they are. All calls block; a shell with an interface
//! runs them on worker threads.
//!
//! It does not compile for the web: browsers have no sockets. The web client
//! reaches hosts through a bridge instead.

pub mod discovery;
pub mod store;
mod transport;

use std::{
    collections::BTreeMap,
    io,
    path::Path,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

pub use leto_core::model::{
    AppSummary, HostId, HostKind, HostSummary, Pin, Reachability, join_address, parse_address,
};
use moonlight_common::{
    AppId, ServerState, ServerType,
    crypto::rustcrypto::RustCryptoBackend,
    high::std::MoonlightHost,
    http::{
        ClientIdentifier, ClientSecret, ServerIdentifier,
        pair::{PairPin, PairingCryptoBackend},
    },
};
use store::{Identity, Saved, SavedHost, Store};
use transport::Transport;

type Host = MoonlightHost<Transport>;
type ProtocolError = moonlight_common::error::Error;

/// What can go wrong, in words meant for the person using Leto.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("\"{0}\" is not an address Leto understands")]
    BadAddress(String),
    #[error("the PC did not answer")]
    Offline,
    #[error("the PIN did not match")]
    WrongPin,
    #[error("the PIN was not entered on the PC in time")]
    PinTimeout,
    #[error("another device is pairing with this PC right now")]
    PairingBusy,
    #[error("the PC refused the pairing")]
    PairingFailed,
    #[error("this device is not paired with the PC")]
    NotPaired,
    #[error("Leto does not know this PC")]
    UnknownHost,
    #[error("Leto could not read or write its settings: {0}")]
    Storage(#[from] io::Error),
    #[error("searching the network failed: {0}")]
    Discovery(String),
    #[error("the PC answered in a way Leto did not expect: {0}")]
    Protocol(String),
}

impl From<ProtocolError> for Error {
    fn from(error: ProtocolError) -> Self {
        match error {
            ProtocolError::ConnectionFailed | ProtocolError::ConnectionTimeout => Self::Offline,
            ProtocolError::PairingFailedWrongPin => Self::WrongPin,
            ProtocolError::PairingFailedAlreadyInProgress => Self::PairingBusy,
            ProtocolError::PairingFailed => Self::PairingFailed,
            ProtocolError::Unauthenticated
            | ProtocolError::NotPaired
            | ProtocolError::StatusCode { code: 401, .. } => Self::NotPaired,
            other => Self::Protocol(other.to_string()),
        }
    }
}

/// A host the client knows, saved or only seen on the network.
struct Entry {
    summary: HostSummary,
    /// The host's certificate from pairing, as PEM.
    certificate: Option<String>,
    /// Kept between runs: paired, or added by hand.
    saved: bool,
    /// The live connection state for this host, once it has answered.
    handle: Option<Arc<Host>>,
}

/// The client engine. One per app.
pub struct Client {
    store: Store,
    identifier: ClientIdentifier,
    secret: ClientSecret,
    hosts: Mutex<BTreeMap<HostId, Entry>>,
}

impl Client {
    /// Opens the client with its settings in the user's configuration folder.
    pub fn open_default() -> Result<Self, Error> {
        let folders = directories::ProjectDirs::from("re", "leto", "Leto").ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "this system has no home folder")
        })?;
        Self::open(folders.config_dir())
    }

    /// Opens the client with its settings in `directory`, creating the
    /// device's identity on first use.
    pub fn open(directory: &Path) -> Result<Self, Error> {
        let store = Store::in_directory(directory);
        let mut saved = store.load()?;

        let (identifier, secret) = match &saved.identity {
            Some(identity) => (
                ClientIdentifier::from_pem(parse_pem(&identity.certificate)?),
                ClientSecret::from_pem(parse_pem(&identity.private_key)?),
            ),
            None => {
                log::info!("creating this device's identity in {}", store.path().display());
                let (identifier, secret) = RustCryptoBackend.generate_client_identity()?;
                saved.identity = Some(Identity {
                    certificate: identifier.to_pem().to_string(),
                    private_key: secret.to_pem().to_string(),
                });
                store.save(&saved)?;
                (identifier, secret)
            }
        };

        let hosts = saved
            .hosts
            .into_iter()
            .map(|host| {
                let summary = HostSummary {
                    id: host.id.clone(),
                    name: host.name,
                    address: host.address,
                    port: host.port,
                    kind: host.kind,
                    paired: host.certificate.is_some(),
                    reachability: Reachability::Unknown,
                    busy: false,
                };
                let entry =
                    Entry { summary, certificate: host.certificate, saved: true, handle: None };
                (host.id, entry)
            })
            .collect();

        Ok(Self { store, identifier, secret, hosts: Mutex::new(hosts) })
    }

    /// Every host the client knows, sorted by name.
    pub fn hosts(&self) -> Vec<HostSummary> {
        let mut hosts: Vec<_> = self.lock().values().map(|entry| entry.summary.clone()).collect();
        hosts.sort_by(|a, b| {
            a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.id.cmp(&b.id))
        });
        hosts
    }

    /// One host by id.
    pub fn host(&self, id: &HostId) -> Option<HostSummary> {
        self.lock().get(id).map(|entry| entry.summary.clone())
    }

    /// Adds a host by address (`"192.168.1.20"`, `"pc.example.org:47989"`)
    /// and keeps it between runs.
    pub fn add(&self, address: &str) -> Result<HostSummary, Error> {
        let (address, port) =
            parse_address(address).ok_or_else(|| Error::BadAddress(address.trim().to_owned()))?;
        let summary = self.probe(&address, port)?;
        if let Some(entry) = self.lock().get_mut(&summary.id) {
            entry.saved = true;
        }
        self.persist()?;
        Ok(summary)
    }

    /// Listens on the local network for `duration` and returns the hosts that
    /// announced themselves and answered. They join [`Client::hosts`].
    pub fn discover(&self, duration: Duration) -> Result<Vec<HostSummary>, Error> {
        let announcements =
            discovery::listen(duration).map_err(|error| Error::Discovery(error.to_string()))?;
        let mut found: Vec<HostSummary> = Vec::new();
        for announcement in announcements {
            match self.probe(&announcement.address.to_string(), announcement.port) {
                Ok(summary) if found.iter().all(|host| host.id != summary.id) => {
                    found.push(summary)
                }
                Ok(_) => {}
                Err(error) => log::debug!("{} announced itself but: {error}", announcement.address),
            }
        }
        Ok(found)
    }

    /// Asks every known host whether it is there. Hosts that were only seen
    /// on the network and no longer answer are dropped.
    pub fn refresh(&self) -> Vec<HostSummary> {
        let known: Vec<(HostId, String, u16)> = self
            .lock()
            .values()
            .map(|entry| {
                (entry.summary.id.clone(), entry.summary.address.clone(), entry.summary.port)
            })
            .collect();

        for (id, address, port) in known {
            match self.probe(&address, port) {
                // The address now belongs to another PC: the one asked for is gone.
                Ok(summary) if summary.id != id => self.mark_offline(&id),
                Ok(_) => {}
                Err(error) => {
                    log::debug!("{address} did not answer: {error}");
                    self.mark_offline(&id);
                }
            }
        }
        self.hosts()
    }

    /// A fresh random PIN to show the person.
    pub fn new_pin(&self) -> Result<Pin, Error> {
        let mut bytes = [0u8; 4];
        RustCryptoBackend.random_bytes(&mut bytes)?;
        Ok(Pin::from_bytes(bytes))
    }

    /// Pairs with a host. Blocks until the person has typed `pin` on the
    /// host, which can take a while, or until the host gives up waiting.
    pub fn pair(&self, id: &HostId, pin: Pin) -> Result<HostSummary, Error> {
        let handle = self.handle(id)?;
        let [a, b, c, d] = pin.digits();
        let pin = PairPin::new(a, b, c, d).expect("a Pin holds four digits below ten");

        handle
            .pair(&self.identifier, &self.secret, device_name(), pin, RustCryptoBackend)
            .map_err(|error| match error {
                // The first request waits for the PIN, so a timeout means
                // nobody typed it.
                ProtocolError::ConnectionTimeout => Error::PinTimeout,
                other => Error::from(other),
            })?;

        let (_, _, server) = handle.identity().ok_or(Error::PairingFailed)?;
        let summary = {
            let mut hosts = self.lock();
            let entry = hosts.get_mut(id).ok_or(Error::UnknownHost)?;
            entry.certificate = Some(server.to_pem().to_string());
            entry.saved = true;
            entry.summary.paired = true;
            entry.summary.reachability = Reachability::Online;
            entry.summary.clone()
        };
        self.persist()?;
        Ok(summary)
    }

    /// The apps a paired host offers.
    pub fn apps(&self, id: &HostId) -> Result<Vec<AppSummary>, Error> {
        let handle = self.handle(id)?;
        if !handle.is_paired()? {
            return Err(Error::NotPaired);
        }
        // Ask again: the list changes when the host's owner edits it.
        handle.update()?;
        let apps = handle.app_list()?;
        Ok(apps
            .into_iter()
            .map(|app| AppSummary { id: app.id.0, title: app.title, hdr: app.is_hdr_supported })
            .collect())
    }

    /// The cover image of an app, as the bytes of an image file.
    pub fn app_image(&self, id: &HostId, app: u32) -> Result<Vec<u8>, Error> {
        Ok(self.handle(id)?.request_app_image(AppId(app))?)
    }

    /// Forgets a host, and tells it to forget this device when it can.
    pub fn forget(&self, id: &HostId) -> Result<(), Error> {
        let entry = self.lock().remove(id).ok_or(Error::UnknownHost)?;
        if let (true, Some(handle)) = (entry.summary.paired, entry.handle)
            && let Err(error) = handle.unpair()
        {
            log::debug!("{} was not told to forget this device: {error}", entry.summary.name);
        }
        self.persist()
    }

    /// Asks the host at `address` who it is and records the answer.
    fn probe(&self, address: &str, port: u16) -> Result<HostSummary, Error> {
        let mut handle = Host::new(address.to_owned(), port, None)?;
        handle.update()?;
        let id = HostId(handle.unique_id()?.to_string());

        // A host this device is paired with must prove it still is that host,
        // and gets to say whether it still knows this device.
        let certificate = self.lock().get(&id).and_then(|entry| entry.certificate.clone());
        let mut paired = false;
        if let Some(certificate) = &certificate {
            let server = ServerIdentifier::from_pem(parse_pem(certificate)?);
            match handle.set_identity(self.identifier.clone(), self.secret.clone(), server) {
                Ok(()) => paired = true,
                Err(ProtocolError::ConnectionFailed | ProtocolError::ConnectionTimeout) => {
                    return Err(Error::Offline);
                }
                Err(error) => {
                    log::warn!("{address} no longer accepts this device's pairing: {error}");
                    handle = Host::new(address.to_owned(), port, None)?;
                    handle.update()?;
                }
            }
        }

        let summary = HostSummary {
            id: id.clone(),
            name: handle.host_name()?,
            address: address.to_owned(),
            port,
            kind: match handle.version()?.server_type {
                ServerType::Apollo => HostKind::Apollo,
                ServerType::FoundationSunshine => HostKind::FoundationSunshine,
                ServerType::NvidiaGameStream => HostKind::GameStream,
                _ => HostKind::Sunshine,
            },
            paired,
            reachability: Reachability::Online,
            busy: handle.state()? == ServerState::Busy,
        };

        let mut hosts = self.lock();
        let entry = hosts.entry(id).or_insert_with(|| Entry {
            summary: summary.clone(),
            certificate: None,
            saved: false,
            handle: None,
        });
        entry.summary = summary.clone();
        entry.handle = Some(Arc::new(handle));
        Ok(summary)
    }

    /// The live handle of a host, asking the host first if there is none yet.
    fn handle(&self, id: &HostId) -> Result<Arc<Host>, Error> {
        let (handle, address, port) = {
            let hosts = self.lock();
            let entry = hosts.get(id).ok_or(Error::UnknownHost)?;
            (entry.handle.clone(), entry.summary.address.clone(), entry.summary.port)
        };
        if let Some(handle) = handle {
            return Ok(handle);
        }
        let summary = self.probe(&address, port)?;
        if summary.id != *id {
            return Err(Error::Offline);
        }
        self.lock().get(id).and_then(|entry| entry.handle.clone()).ok_or(Error::UnknownHost)
    }

    fn mark_offline(&self, id: &HostId) {
        let mut hosts = self.lock();
        let Some(entry) = hosts.get_mut(id) else { return };
        if entry.saved {
            entry.summary.reachability = Reachability::Offline;
            entry.summary.busy = false;
            entry.handle = None;
        } else {
            hosts.remove(id);
        }
    }

    /// Writes the saved hosts to disk.
    fn persist(&self) -> Result<(), Error> {
        let hosts = self
            .lock()
            .values()
            .filter(|entry| entry.saved)
            .map(|entry| SavedHost {
                id: entry.summary.id.clone(),
                name: entry.summary.name.clone(),
                address: entry.summary.address.clone(),
                port: entry.summary.port,
                kind: entry.summary.kind,
                certificate: entry.certificate.clone(),
            })
            .collect();
        let identity = Identity {
            certificate: self.identifier.to_pem().to_string(),
            private_key: self.secret.to_pem().to_string(),
        };
        self.store.save(&Saved { identity: Some(identity), hosts })?;
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<HostId, Entry>> {
        // The map stays consistent even if a thread panicked while holding it.
        self.hosts.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The name this device shows in the host's list of paired devices.
fn device_name() -> String {
    let name = gethostname::gethostname().to_string_lossy().trim().to_owned();
    if name.is_empty() { "Leto".to_owned() } else { name }
}

fn parse_pem(text: &str) -> Result<pem::Pem, Error> {
    pem::parse(text)
        .map_err(|error| Error::Storage(io::Error::new(io::ErrorKind::InvalidData, error)))
}
