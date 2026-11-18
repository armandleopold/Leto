//! The client side of Leto: find hosts, pair with them, list their apps and
//! stream one.
//!
//! This crate is the engine behind every native client shell. It speaks the
//! Moonlight protocol through `moonlight-common`, so it works with Sunshine
//! and Apollo hosts as they are. All calls block; a shell with an interface
//! runs them on worker threads.
//!
//! A stream arrives still encoded. See [`session`] for where it goes. A
//! shell that would rather ask and be told than call and wait uses
//! [`service`].
//!
//! The camera of this device goes the other way, to a PC that takes one:
//! see [`Client::send_camera`].
//!
//! It does not compile for the web: browsers have no sockets. The web client
//! reaches hosts through a bridge instead.

mod camera;
mod describe;
pub mod discovery;
pub mod service;
pub mod session;
pub mod store;
mod transport;

use std::{
    collections::BTreeMap,
    fmt, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    thread,
    time::Duration,
};

pub use camera::{CameraLink, CameraRefusal, CameraSettings};
pub use leto_core::{
    engine, input,
    model::{
        AppSummary, HostId, HostKind, HostSummary, Pin, Reachability, join_address, parse_address,
    },
};
use moonlight_common::{
    AppId, ServerState, ServerType,
    crypto::rustcrypto::RustCryptoBackend,
    high::std::MoonlightHost,
    http::{
        ClientIdentifier, ClientSecret, ServerIdentifier,
        pair::{PairPin, PairingCryptoBackend},
        server_info::ApolloPermissions,
    },
    stream::{AesIv, AesKey, std::MoonlightStream},
};
pub use service::{Service, Shell};
pub use session::{
    AudioSetup, AudioSink, Counter, Decoded, Ended, InputError, Session, StreamSettings, Tally,
    VideoFrame, VideoSetup, VideoSink,
};
use store::{Identity, Saved, SavedHost, Store};
use transport::Transport;

/// Where Leto keeps its settings on this system: the user's configuration
/// folder.
pub fn default_directory() -> Result<PathBuf, Error> {
    let folders = directories::ProjectDirs::from("re", "leto", "Leto")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "this system has no home folder"))?;
    Ok(folders.config_dir().to_owned())
}

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
    #[error("another app is running on the PC")]
    OtherAppRunning,
    #[error(
        "the PC does not let this device {0}: allow \"{name}\" for it on Apollo's PIN page, \
         under Device Management",
        name = .0.apollo_name()
    )]
    NotAllowed(Permission),
    #[error("the PC cannot stream with these settings: {0}")]
    Unsupported(String),
    #[error("the PC does not take the camera of this device: {0}")]
    Camera(CameraRefusal),
    #[error("Leto could not read or write its settings: {0}")]
    Storage(#[from] io::Error),
    #[error("searching the network failed: {0}")]
    Discovery(String),
    #[error("the PC answered in a way Leto did not expect: {0}")]
    Protocol(String),
}

/// Something the owner of a PC allows or refuses device by device.
///
/// Apollo does it: the first device paired with it may do everything, and
/// every device paired after that may only list the apps and watch, until the
/// owner gives it more on Apollo's PIN page, under "Device Management".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    ListApps,
    ViewStreams,
    LaunchApps,
}

impl Permission {
    /// The name Apollo gives this permission on its page.
    pub const fn apollo_name(self) -> &'static str {
        match self {
            Self::ListApps => "List Apps",
            Self::ViewStreams => "View Streams",
            Self::LaunchApps => "Launch Apps",
        }
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ListApps => "list its apps",
            Self::ViewStreams => "watch what is running",
            Self::LaunchApps => "start or close apps",
        })
    }
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
            // Apollo's answer to a device that may not start or close apps.
            ProtocolError::StatusCode { code: 403, .. } => Self::NotAllowed(Permission::LaunchApps),
            ProtocolError::SettingHdrNotSupported | ProtocolError::ServerVersionNotSupported(_) => {
                Self::Unsupported(error.to_string())
            }
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
    /// Shared with the streams, which outlive the calls that start them.
    hosts: Arc<Mutex<BTreeMap<HostId, Entry>>>,
}

impl Client {
    /// Opens the client with its settings in the user's configuration folder.
    pub fn open_default() -> Result<Self, Error> {
        Self::open(&default_directory()?)
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
                    running_app: None,
                };
                let entry =
                    Entry { summary, certificate: host.certificate, saved: true, handle: None };
                (host.id, entry)
            })
            .collect();

        Ok(Self { store, identifier, secret, hosts: Arc::new(Mutex::new(hosts)) })
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

        // All at once: a PC that is off takes seconds to give up on.
        thread::scope(|scope| {
            for (id, address, port) in known {
                scope.spawn(move || match self.probe(&address, port) {
                    // The address now belongs to another PC: the one asked for is gone.
                    Ok(summary) if summary.id != id => self.mark_offline(&id),
                    Ok(_) => {}
                    Err(error) => {
                        log::debug!("{address} did not answer: {error}");
                        self.mark_offline(&id);
                    }
                });
            }
        });
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
        // Apollo would answer with a single app named "Permission Denied".
        if let Some(allowed) = handle.apollo_permissions()?
            && !allowed.intersects(
                ApolloPermissions::LIST | ApolloPermissions::VIEW | ApolloPermissions::LAUNCH,
            )
        {
            return Err(Error::NotAllowed(Permission::ListApps));
        }
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

    /// Starts `app` on a paired host, or goes back to it when it is already
    /// running, and connects the stream. Blocks until pictures can flow.
    ///
    /// The stream arrives on threads of its own: pictures at `video`, sound at
    /// `audio`. `ended` is called once if the stream ends without
    /// [`Session::stop`], for example when the app closes on the PC.
    pub fn play(
        &self,
        id: &HostId,
        app: u32,
        settings: StreamSettings,
        video: impl VideoSink,
        audio: impl AudioSink,
        ended: impl FnOnce(Ended) + Send + 'static,
    ) -> Result<Session, Error> {
        let handle = self.handle(id)?;
        if !handle.is_paired()? {
            return Err(Error::NotPaired);
        }
        // A host runs one app at a time. Resuming would show the other one.
        handle.update()?;
        let running = handle.current_game()?;
        if running != 0 && running != app {
            return Err(Error::OtherAppRunning);
        }
        // A device that may only watch can join the app that is running, and
        // nothing more.
        if let Some(allowed) = handle.apollo_permissions()? {
            let (needed, missing) = if running == app {
                (ApolloPermissions::VIEW | ApolloPermissions::LAUNCH, Permission::ViewStreams)
            } else {
                (ApolloPermissions::LAUNCH, Permission::LaunchApps)
            };
            if !allowed.intersects(needed) {
                return Err(Error::NotAllowed(missing));
            }
        }

        let mut protocol = settings.to_protocol();
        protocol
            .adjust_for_server(
                handle.version()?,
                &handle.gfe_version()?,
                handle.server_codec_mode_support()?,
            )
            .map_err(|error| Error::Unsupported(error.to_string()))?;

        // A fresh key for every stream. The host learns it over the paired
        // HTTPS connection, and both sides encrypt the stream with it.
        let key = AesKey::new_random(&RustCryptoBackend)?;
        let key_id = AesIv::new_random(&RustCryptoBackend)?;
        let mut config = handle.start_stream(
            AppId(app),
            &protocol,
            key,
            key_id,
            MoonlightStream::launch_query_parameters(),
        )?;
        // The protocol library asks for the microphone stream of the one
        // kind of host it knows to take one. Leto asks the host instead.
        config.foundation_enable_mic =
            config.version.is_sunshine_like() && describe::takes_microphone(&config);
        if config.foundation_enable_mic {
            // The library also keeps microphone encryption to that kind of
            // host, and every host that takes a microphone wants it
            // encrypted. Nothing else in a stream depends on the kind.
            config.version.server_type = ServerType::FoundationSunshine;
        }
        let witness = {
            let mut hosts = self.lock();
            let entry = hosts.get_mut(id).ok_or(Error::UnknownHost)?;
            entry.summary.busy = true;
            entry.summary.running_app = Some(app);
            Witness {
                hosts: Arc::clone(&self.hosts),
                id: id.clone(),
                app,
                address: entry.summary.address.clone(),
                port: entry.summary.port,
                identifier: self.identifier.clone(),
                secret: self.secret.clone(),
                certificate: entry.certificate.clone(),
            }
        };
        let ended = move |reason| ended(witness.confirm(reason));
        Ok(Session::connect(config, protocol, video, audio, ended)?)
    }

    /// The camera a paired host takes from this device, when it takes one.
    /// No Sunshine or Apollo does. Blocks for a few seconds at most.
    pub fn camera_offer(&self, id: &HostId) -> Result<Option<engine::CameraOffer>, Error> {
        Ok(self.leto_api(id)?.camera_offer())
    }

    /// Asks a paired host to take the camera of this device, and opens the
    /// way there. `sending` is what the pictures will be like: within what
    /// [`Client::camera_offer`] said. Blocks until the host has answered.
    ///
    /// The pictures travel sealed with a key of their own, which the host
    /// made and told over the paired HTTPS connection.
    pub fn send_camera(&self, id: &HostId, sending: CameraSettings) -> Result<CameraLink, Error> {
        self.leto_api(id)?.send_camera(sending)
    }

    /// Leto's own API on a paired host, which few have.
    fn leto_api(&self, id: &HostId) -> Result<camera::LetoApi, Error> {
        let (address, port, certificate) = {
            let hosts = self.lock();
            let entry = hosts.get(id).ok_or(Error::UnknownHost)?;
            (entry.summary.address.clone(), entry.summary.port, entry.certificate.clone())
        };
        camera::LetoApi::of(
            &address,
            port,
            &self.identifier,
            &self.secret,
            &certificate.ok_or(Error::NotPaired)?,
        )
    }

    /// Closes the app running on a paired host.
    pub fn quit_app(&self, id: &HostId) -> Result<HostSummary, Error> {
        let (address, port, certificate) = {
            let hosts = self.lock();
            let entry = hosts.get(id).ok_or(Error::UnknownHost)?;
            (entry.summary.address.clone(), entry.summary.port, entry.certificate.clone())
        };
        // Closing needs a handle of its own: the shared one cannot be changed.
        let mut handle = paired_handle(
            &address,
            port,
            &self.identifier,
            &self.secret,
            &certificate.ok_or(Error::NotPaired)?,
        )?;
        handle.cancel()?;
        self.probe(&address, port)
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
            running_app: Some(handle.current_game()?).filter(|app| *app != 0),
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
            entry.summary.running_app = None;
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

/// What a stream needs to find out why it ended, after the call that started
/// it has returned.
struct Witness {
    hosts: Arc<Mutex<BTreeMap<HostId, Entry>>>,
    id: HostId,
    app: u32,
    address: String,
    port: u16,
    identifier: ClientIdentifier,
    secret: ClientSecret,
    certificate: Option<String>,
}

impl Witness {
    /// Corrects the reason a stream gave for its ending by asking the host.
    ///
    /// The stream cannot tell a closed app from a lost connection:
    /// `moonlight-common` drops the host's goodbye (see `docs/inventory.md`).
    /// The host can: it says over HTTPS which app it is running.
    fn confirm(self, reason: Ended) -> Ended {
        if reason != Ended::ConnectionLost {
            return reason;
        }
        let running = self.certificate.as_deref().ok_or(Error::NotPaired).and_then(|certificate| {
            let handle = paired_handle(
                &self.address,
                self.port,
                &self.identifier,
                &self.secret,
                certificate,
            )?;
            Ok(handle.current_game()?)
        });
        match running {
            Ok(running) if running != self.app => {
                let mut hosts = self.hosts.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(entry) = hosts.get_mut(&self.id) {
                    entry.summary.busy = running != 0;
                    entry.summary.running_app = Some(running).filter(|app| *app != 0);
                }
                Ended::ByHost
            }
            Ok(_) => Ended::ConnectionLost,
            Err(error) => {
                log::debug!("{} did not say why the stream ended: {error}", self.address);
                Ended::ConnectionLost
            }
        }
    }
}

/// A connection of its own to a host this device is paired with, for what
/// the shared one cannot do.
fn paired_handle(
    address: &str,
    port: u16,
    identifier: &ClientIdentifier,
    secret: &ClientSecret,
    certificate: &str,
) -> Result<Host, Error> {
    let server = ServerIdentifier::from_pem(parse_pem(certificate)?);
    let handle = Host::new(address.to_owned(), port, None)?;
    handle.update()?;
    handle.set_identity(identifier.clone(), secret.clone(), server)?;
    Ok(handle)
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
