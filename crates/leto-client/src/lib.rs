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
//! A PC away from home can be reached through a WireGuard tunnel Leto runs
//! itself: see [`Client::use_tunnel`].
//!
//! It does not compile for the web: browsers have no sockets. The web client
//! reaches hosts through a bridge instead.

mod camera;
mod describe;
pub mod discovery;
pub mod logs;
pub mod service;
pub mod session;
pub mod store;
mod transport;
mod tunnel;
pub mod wake;

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, io,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc},
    thread,
    time::{Duration, Instant},
};

pub use camera::{CameraLink, CameraRefusal, CameraSettings};
pub use leto_core::{
    engine, input,
    model::{
        AppSummary, HostId, HostKind, HostSummary, Pin, Reachability, TunnelSummary, join_address,
        parse_address,
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
    stream::{AesIv, AesKey},
};
pub use service::{Service, Shell};
pub use session::{
    AudioSetup, AudioSink, Counter, Decoded, Ended, InputError, Session, StreamSettings, Tally,
    VideoFrame, VideoSetup, VideoSink,
};
use store::{Identity, Saved, SavedAddress, SavedHost, Store};
use transport::Transport;

/// The folder [`default_directory`] gives, when the system has none to
/// offer: an Android app is handed its own as it starts. Only the first
/// call counts, and it must come before anything reads the settings.
pub fn use_directory(folder: PathBuf) {
    let _ = CHOSEN_DIRECTORY.set(folder);
}

static CHOSEN_DIRECTORY: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Where Leto keeps its settings on this system: the user's configuration
/// folder, or the one given to [`use_directory`].
pub fn default_directory() -> Result<PathBuf, Error> {
    if let Some(folder) = CHOSEN_DIRECTORY.get() {
        return Ok(folder.clone());
    }
    let folders = directories::ProjectDirs::from("re", "leto", "Leto")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "this system has no home folder"))?;
    Ok(folders.config_dir().to_owned())
}

type Host = MoonlightHost<Transport>;

/// How long [`Client::wake_up`] waits between two looks at a host it woke,
/// and between two signals.
const WAKE_LOOKS: Duration = Duration::from_secs(2);
const WAKE_AGAIN: Duration = Duration::from_secs(10);

/// How many addresses Leto keeps for one PC. The ones that answered least
/// recently go first.
const ADDRESSES_KEPT: usize = 4;
/// How often, at most, this device's system is asked where the names of
/// the PCs lead: a name that leads nowhere is slow to say so.
const NAMES_ASKED_EVERY: Duration = Duration::from_secs(60);
/// How long an address a PC's name led to has to answer. It is nearby.
const NEARBY_PATIENCE: Duration = Duration::from_secs(1);
/// How long an address around this device has to take a connection.
const AROUND_PATIENCE: Duration = Duration::from_millis(300);
/// How long the direct ways to a PC have to answer before the one through
/// Leto's tunnel asks: at home, the local network answers within it.
const TUNNEL_HEAD_START: Duration = Duration::from_millis(150);
/// The largest video packet through the tunnel, which must carry it whole
/// with its own headers: what Moonlight uses for a PC far away.
const TUNNEL_PACKET: u32 = 1024;
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
    #[error(
        "Leto cannot wake this PC yet: the PC tells the address of its network card once it is \
         on and paired"
    )]
    CannotWake,
    #[error("the signal to wake the PC could not be sent: {0}")]
    WakeNotSent(String),
    #[error(
        "the PC did not wake up within {0} seconds. Wake-on-LAN may be off in its BIOS or in \
         the settings of its network card, or this device is on another network"
    )]
    DidNotWake(u64),
    #[error("the WireGuard file cannot be used: {0}")]
    Tunnel(String),
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
    /// Every address and port the host was reached at, the one that answered
    /// last first. The summary has that one.
    addresses: Vec<(String, u16)>,
    /// The host's certificate from pairing, as PEM.
    certificate: Option<String>,
    /// The address of its network card, to wake it with. A host tells a
    /// device it is paired with.
    card: Option<wake::Card>,
    /// The local door that leads to the host through Leto's tunnel, when
    /// that is the way it answered at.
    door: Option<Ipv4Addr>,
    /// Kept between runs: paired, or added by hand.
    saved: bool,
    /// The live connection state for this host, once it has answered.
    handle: Option<Arc<Host>>,
}

impl Entry {
    /// Where to connect to the host: its door when it answered through
    /// Leto's tunnel.
    fn reached_at(&self) -> String {
        self.door.map_or_else(|| self.summary.address.clone(), |door| door.to_string())
    }
}

/// The client engine. One per app.
pub struct Client {
    store: Store,
    identifier: ClientIdentifier,
    secret: ClientSecret,
    /// Shared with the streams, which outlive the calls that start them.
    hosts: Arc<Mutex<BTreeMap<HostId, Entry>>>,
    /// Held while the file is written: hosts can be saved from several
    /// threads at once.
    saving: Mutex<()>,
    /// The way through Leto's WireGuard tunnel, if it has one.
    ways: Arc<tunnel::Ways>,
    /// When the system was last asked where the names of the PCs lead.
    names_asked: Mutex<Option<Instant>>,
    /// The local networks Leto looked around on since it started.
    looked_around: Mutex<BTreeSet<discovery::Around>>,
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
                let addresses = std::iter::once((host.address.clone(), host.port))
                    .chain(
                        host.other_addresses.into_iter().map(|other| (other.address, other.port)),
                    )
                    .collect();
                let card = host.card.as_deref().and_then(wake::parse_card);
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
                    wakeable: card.is_some(),
                    tunnel: false,
                };
                let entry = Entry {
                    summary,
                    addresses,
                    certificate: host.certificate,
                    card,
                    door: None,
                    saved: true,
                    handle: None,
                };
                (host.id, entry)
            })
            .collect();

        Ok(Self {
            store,
            identifier,
            secret,
            hosts: Arc::new(Mutex::new(hosts)),
            saving: Mutex::new(()),
            ways: Arc::new(tunnel::Ways::open(directory)),
            names_asked: Mutex::new(None),
            looked_around: Mutex::new(BTreeSet::new()),
        })
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
    /// and keeps it between runs. A host Leto knows already keeps its other
    /// addresses too.
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
    ///
    /// Meanwhile the PCs Leto keeps are looked for by their names, and a
    /// paired PC that answers only from further away is looked for at the
    /// addresses around this device: both find the address nearby of a PC
    /// known only through a VPN.
    pub fn discover(&self, duration: Duration) -> Result<Vec<HostSummary>, Error> {
        let (announcements, named) = thread::scope(|scope| {
            let naming = scope.spawn(|| self.where_names_lead(duration));
            let looking = scope.spawn(|| self.look_around_on(&discovery::networks()));
            let announcements = discovery::listen(duration);
            let _ = looking.join();
            (announcements, naming.join().unwrap_or_default())
        });
        self.ask_nearby(&named);
        let announcements = announcements.map_err(|error| Error::Discovery(error.to_string()))?;
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

    /// The addresses nearby that the names of the PCs Leto keeps lead to,
    /// by this device's system, and that Leto does not know them at yet:
    /// each with the PC it would be and the port to ask on. Asked once in a
    /// while only.
    fn where_names_lead(&self, patience: Duration) -> Vec<(HostId, String, u16)> {
        {
            let mut asked = self.names_asked.lock().unwrap_or_else(PoisonError::into_inner);
            if asked.is_some_and(|asked| asked.elapsed() < NAMES_ASKED_EVERY) {
                return Vec::new();
            }
            *asked = Some(Instant::now());
        }
        let kept: Vec<_> = self
            .lock()
            .iter()
            .filter(|(_, entry)| entry.saved)
            .map(|(id, entry)| (id.clone(), entry.summary.name.clone(), entry.addresses.clone()))
            .collect();
        // All at once: a name nobody knows takes all the patience there is.
        thread::scope(|scope| {
            let asking: Vec<_> = kept
                .iter()
                .map(|(id, name, known)| {
                    scope.spawn(move || {
                        let mut ports: Vec<u16> = known.iter().map(|(_, port)| *port).collect();
                        ports.sort_unstable();
                        ports.dedup();
                        let mut led = Vec::new();
                        for address in discovery::by_name(name, patience) {
                            let address = address.to_string();
                            for &port in &ports {
                                if !known.contains(&(address.clone(), port)) {
                                    led.push((id.clone(), address.clone(), port));
                                }
                            }
                        }
                        led
                    })
                })
                .collect();
            let led: Vec<_> =
                asking.into_iter().flat_map(|asking| asking.join().unwrap_or_default()).collect();
            log::debug!(
                "the names of the {} PCs kept lead to {} addresses nearby that are new",
                kept.len(),
                led.len()
            );
            led
        })
    }

    /// Asks at the addresses the names of PCs led to, and keeps those where
    /// the PC answers: for one that is paired, with the proof that it is.
    fn ask_nearby(&self, named: &[(HostId, String, u16)]) {
        if named.is_empty() {
            return;
        }
        let answers = self.ask_every_way(
            named.iter().map(|(_, address, port)| (address.clone(), *port)).collect(),
        );
        let deadline = Instant::now() + NEARBY_PATIENCE;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            let answer = match answers.recv_timeout(left) {
                Ok(Ok(answer)) => answer,
                Ok(Err(_)) => continue,
                Err(_) => break,
            };
            let id = answer.summary.id.clone();
            let expected = named.iter().any(|(expected, address, port)| {
                *expected == id
                    && *address == answer.summary.address
                    && *port == answer.summary.port
            });
            let paired = self.lock().get(&id).is_some_and(|entry| entry.certificate.is_some());
            if !expected {
                // Another PC has that name here: found, as one that
                // announced itself.
                self.record(answer);
            } else if answer.summary.paired || !paired {
                log::info!(
                    "{} is also at {}, where its name leads on this network",
                    answer.summary.name,
                    join_address(&answer.summary.address, answer.summary.port)
                );
                self.record(answer);
            } else {
                log::debug!(
                    "{} did not prove to be {}",
                    join_address(&answer.summary.address, answer.summary.port),
                    answer.summary.name
                );
            }
        }
    }

    /// Looks on `networks`, the local ones this device is on, for the PCs
    /// that are paired and answer now, but not on that network: as one
    /// reached through a VPN while this device is at home beside it. Every
    /// address around this device is asked for a connection on the PC's
    /// port, and what takes one is asked who it is. Only a PC that proves
    /// to be one Leto paired with is kept, at that address too, and the
    /// nearest address is then the one used.
    ///
    /// Each network is looked around on once while Leto runs, and only
    /// while such a PC answers: it is some 250 connections tried, which a
    /// network that is not the person's own may notice.
    #[doc(hidden)]
    pub fn look_around_on(&self, networks: &[discovery::Around]) {
        for network in networks {
            let (sought, ports) = {
                let hosts = self.lock();
                let sought: BTreeMap<HostId, Vec<u16>> = hosts
                    .iter()
                    .filter(|(_, entry)| {
                        let reached = entry.summary.address.parse::<std::net::Ipv4Addr>();
                        entry.saved
                            && entry.certificate.is_some()
                            && entry.summary.reachability == Reachability::Online
                            && !reached.is_ok_and(|reached| network.holds(reached))
                    })
                    .map(|(id, entry)| {
                        (id.clone(), entry.addresses.iter().map(|(_, port)| *port).collect())
                    })
                    .collect();
                let ports: BTreeSet<u16> = sought.values().flatten().copied().collect();
                (sought, ports)
            };
            if sought.is_empty() {
                continue;
            }
            let new =
                self.looked_around.lock().unwrap_or_else(PoisonError::into_inner).insert(*network);
            if !new {
                continue;
            }
            let addresses: Vec<_> = network.addresses().collect();
            let mut taking = Vec::new();
            for port in ports {
                let open = discovery::listening(&addresses, port, AROUND_PATIENCE);
                log::debug!(
                    "looking around for {} PCs that answer from further away: {} of the {} \
                     addresses around this device take a connection on port {port}",
                    sought.len(),
                    open.len(),
                    addresses.len()
                );
                taking.extend(open.into_iter().map(|address| (address.to_string(), port)));
            }
            if taking.is_empty() {
                continue;
            }
            let answers = self.ask_every_way(taking);
            let deadline = Instant::now() + NEARBY_PATIENCE;
            while let Some(left) = deadline.checked_duration_since(Instant::now()) {
                let answer = match answers.recv_timeout(left) {
                    Ok(Ok(answer)) => answer,
                    Ok(Err(_)) => continue,
                    Err(_) => break,
                };
                // Nothing else found this way is kept: not a PC Leto does
                // not know, and not one that does not prove who it is.
                if sought.contains_key(&answer.summary.id) && answer.summary.paired {
                    log::info!(
                        "{} is also at {}, among the addresses around this device",
                        answer.summary.name,
                        join_address(&answer.summary.address, answer.summary.port)
                    );
                    self.record(answer);
                }
            }
        }
    }

    /// Asks every known host whether it is there, at every address it was
    /// reached at. Hosts that were only seen on the network and no longer
    /// answer are dropped.
    pub fn refresh(&self) -> Vec<HostSummary> {
        let known: Vec<HostId> = self.lock().keys().cloned().collect();

        // All at once: a PC that is off takes seconds to give up on.
        thread::scope(|scope| {
            for id in known {
                scope.spawn(move || {
                    if let Err(error) = self.reach(&id) {
                        log::debug!("{} did not answer: {error}", self.for_log(&id));
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
        log::info!("pairing with {}", self.for_log(id));
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
        // A paired device is told the address of the PC's network card.
        let card = handle
            .update()
            .ok()
            .and_then(|()| handle.mac().ok().flatten())
            .map(|card| card.to_bytes());
        let summary = {
            let mut hosts = self.lock();
            let entry = hosts.get_mut(id).ok_or(Error::UnknownHost)?;
            entry.certificate = Some(server.to_pem().to_string());
            entry.card = card.or(entry.card);
            entry.saved = true;
            entry.summary.paired = true;
            entry.summary.wakeable = entry.card.is_some();
            entry.summary.reachability = Reachability::Online;
            entry.summary.clone()
        };
        self.persist()?;
        log::info!("paired with {}", self.for_log(id));
        Ok(summary)
    }

    /// Sends a host the signal that wakes it from sleep, or starts it when it
    /// is off and its network card listens. Returns at once: see
    /// [`Client::wake_up`] to wait for it.
    pub fn wake(&self, id: &HostId) -> Result<(), Error> {
        let (card, addresses) = {
            let hosts = self.lock();
            let entry = hosts.get(id).ok_or(Error::UnknownHost)?;
            (entry.card.ok_or(Error::CannotWake)?, entry.addresses.clone())
        };
        // Through Leto's tunnel too, to the PC's addresses at home: a router
        // there that still knows the card passes the packet on.
        let doors = addresses.iter().filter_map(|(address, port)| {
            Some((self.ways.door(address, *port)?.to_string(), *port))
        });
        let every = addresses.iter().cloned().chain(doors).collect::<Vec<_>>();
        let sent =
            wake::send(card, &every).map_err(|error| Error::WakeNotSent(error.to_string()))?;
        log::info!("sent {} the signal to wake up, {sent} times", self.for_log(id));
        Ok(())
    }

    /// Wakes a host and waits until it answers, for at most `patience`. A
    /// host that answers already is left as it is. The signal goes again
    /// every few seconds, in case one was lost on the way.
    pub fn wake_up(&self, id: &HostId, patience: Duration) -> Result<HostSummary, Error> {
        if let Ok(summary) = self.reach(id) {
            return Ok(summary);
        }
        let start = Instant::now();
        let mut sent: Option<Instant> = None;
        while start.elapsed() < patience {
            if sent.is_none_or(|sent| sent.elapsed() >= WAKE_AGAIN) {
                self.wake(id)?;
                sent = Some(Instant::now());
            }
            match self.reach(id) {
                Ok(summary) => {
                    log::info!("{} woke up after {:?}", self.for_log(id), start.elapsed());
                    return Ok(summary);
                }
                Err(_) => thread::sleep(WAKE_LOOKS),
            }
        }
        self.mark_offline(id);
        Err(Error::DidNotWake(patience.as_secs()))
    }

    /// The name and address of a host, for the log.
    fn for_log(&self, id: &HostId) -> String {
        self.lock().get(id).map_or_else(
            || "a PC Leto does not know".to_owned(),
            |entry| {
                let host = &entry.summary;
                format!("{} at {}:{} ({:?})", host.name, host.address, host.port, host.kind)
            },
        )
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
        if self.lock().get(id).is_some_and(|entry| entry.door.is_some()) {
            protocol.packet_size = TUNNEL_PACKET;
        }

        // A fresh key for every stream. The host learns it over the paired
        // HTTPS connection, and both sides encrypt the stream with it.
        let key = AesKey::new_random(&RustCryptoBackend)?;
        let key_id = AesIv::new_random(&RustCryptoBackend)?;
        if settings.virtual_display {
            log::info!(
                "asking {} for a screen of its own, {}×{} at {} pictures per second, {} kbit/s",
                self.for_log(id),
                settings.width,
                settings.height,
                settings.fps,
                settings.bitrate_kbps
            );
        }
        let mut config = handle.start_stream(
            AppId(app),
            &protocol,
            key,
            key_id,
            &settings.launch_parameters(),
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
                address: entry.reached_at(),
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
            (entry.reached_at(), entry.summary.port, entry.certificate.clone())
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
        let (address, reached_at, port, certificate) = {
            let hosts = self.lock();
            let entry = hosts.get(id).ok_or(Error::UnknownHost)?;
            let summary = &entry.summary;
            (summary.address.clone(), entry.reached_at(), summary.port, entry.certificate.clone())
        };
        // Closing needs a handle of its own: the shared one cannot be changed.
        let mut handle = paired_handle(
            &reached_at,
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

    /// The WireGuard tunnel Leto runs itself, if it was given one.
    pub fn tunnel(&self) -> Option<TunnelSummary> {
        self.ways.summary()
    }

    /// Uses the tunnel that `text` describes, the content of a WireGuard file
    /// as WireGuard apps and servers write it, instead of any other, and
    /// keeps it. Leto then also reaches each PC through it, at the addresses
    /// it leads to, and keeps the nearest way. Only Leto's own connections
    /// go through it, and it needs no administrator rights.
    ///
    /// The file's keys should be a peer of their own: the same keys in use
    /// elsewhere at the same time, as by the WireGuard app on this device,
    /// would take the tunnel from each other.
    pub fn use_tunnel(&self, text: &str) -> Result<TunnelSummary, Error> {
        let summary = self.ways.set(text)?;
        self.forget_doors();
        Ok(summary)
    }

    /// Stops using the tunnel and forgets its file.
    pub fn remove_tunnel(&self) -> Result<(), Error> {
        self.ways.remove()?;
        self.forget_doors();
        Ok(())
    }

    /// The hosts reached through doors that closed are asked anew next time.
    fn forget_doors(&self) {
        for entry in self.lock().values_mut().filter(|entry| entry.door.is_some()) {
            entry.door = None;
            entry.handle = None;
            entry.summary.tunnel = false;
        }
    }

    /// Asks the host at `address` who it is and records the answer. Where
    /// Leto's tunnel leads there too, both ways at once, and the first to
    /// answer is kept.
    fn probe(&self, address: &str, port: u16) -> Result<HostSummary, Error> {
        if !self.ways.covers(address) {
            let answer = self.asker().ask(address, port, None)?;
            return Ok(self.record(answer));
        }
        let mut failure = None;
        for answer in self.ask_every_way(vec![(address.to_owned(), port)]) {
            match answer {
                Ok(answer) => return Ok(self.record(answer)),
                Err(error) => {
                    failure.get_or_insert(error);
                }
            }
        }
        Err(failure.unwrap_or(Error::Offline))
    }

    /// Asks at each address at once, directly and, where Leto's tunnel leads,
    /// through it, each way in a thread of its own. The answers come as they
    /// arrive.
    fn ask_every_way(
        &self,
        addresses: Vec<(String, u16)>,
    ) -> mpsc::Receiver<Result<Answer, Error>> {
        let (sender, answers) = mpsc::channel();
        for (address, port) in addresses {
            let ways = if self.ways.covers(&address) { &[false, true][..] } else { &[false][..] };
            for &through_tunnel in ways {
                let (asker, sender, address) = (self.asker(), sender.clone(), address.clone());
                // Not waited for: an address that leads nowhere takes seconds
                // to give up on, and the nearest one has answered long before.
                let asking = thread::Builder::new().name("leto: Ask".to_owned()).spawn(move || {
                    let door = match through_tunnel {
                        // Opening the tunnel can take a moment: here too.
                        true => match asker.ways.door(&address, port) {
                            Some(door) => {
                                thread::sleep(TUNNEL_HEAD_START);
                                Some(door)
                            }
                            None => return,
                        },
                        false => None,
                    };
                    // Nobody listens any more once another way answered.
                    let _ = sender.send(asker.ask(&address, port, door));
                });
                if let Err(error) = asking {
                    log::error!("could not start a worker thread: {error}");
                }
            }
        }
        answers
    }

    /// Asks a known host at every address it was reached at, all at once,
    /// and keeps the first to answer: the nearest way there. That is how a
    /// PC reached through a VPN is still streamed over the local network
    /// when both lead to it, as Moonlight prefers its address there. Leto's
    /// own tunnel starts a moment later, so that it is used only when no
    /// direct way answers by then.
    fn reach(&self, id: &HostId) -> Result<HostSummary, Error> {
        let (addresses, paired) = {
            let hosts = self.lock();
            let entry = hosts.get(id).ok_or(Error::UnknownHost)?;
            (entry.addresses.clone(), entry.certificate.is_some())
        };
        let answers = self.ask_every_way(addresses);

        // An answer that does not prove the pairing is kept for when no
        // address gives a better one.
        let mut unproven = None;
        let mut failure = Error::Offline;
        for answer in answers {
            match answer {
                // That address now belongs to another PC.
                Ok(answer) if answer.summary.id != *id => {
                    self.record(answer);
                }
                Ok(answer) if answer.summary.paired || !paired => return Ok(self.record(answer)),
                Ok(answer) => {
                    unproven.get_or_insert(answer);
                }
                Err(error) => failure = error,
            }
        }
        unproven.map(|answer| self.record(answer)).ok_or(failure)
    }

    /// What asking a host needs, for a thread of its own.
    fn asker(&self) -> Asker {
        Asker {
            hosts: Arc::clone(&self.hosts),
            identifier: self.identifier.clone(),
            secret: self.secret.clone(),
            ways: Arc::clone(&self.ways),
        }
    }

    /// Keeps what a host answered, and the address it answered at as the one
    /// to use. A saved host that learns an address is saved again.
    fn record(&self, answer: Answer) -> HostSummary {
        let Answer { mut summary, handle, local, card, door } = answer;
        let learned = {
            let mut hosts = self.lock();
            let entry = hosts.entry(summary.id.clone()).or_insert_with(|| Entry {
                summary: summary.clone(),
                addresses: Vec::new(),
                certificate: None,
                card: None,
                door: None,
                saved: false,
                handle: None,
            });
            let before = entry.addresses.clone();
            let card_before = entry.card;
            // A card is kept even while the host does not tell it, as when
            // it no longer knows this device: it sleeps the same.
            entry.card = card.or(entry.card);
            summary.wakeable = entry.card.is_some();
            let reached = (summary.address.clone(), summary.port);
            entry.addresses.retain(|known| *known != reached);
            entry.addresses.insert(0, reached);
            // The host's own idea of its address, on the port it answered.
            if let Some(local) = local
                && !entry
                    .addresses
                    .iter()
                    .any(|(known, port)| *known == local && *port == summary.port)
            {
                entry.addresses.push((local, summary.port));
            }
            entry.addresses.truncate(ADDRESSES_KEPT);
            if before.len() > 1 && before.first() != entry.addresses.first() {
                log::info!(
                    "{} answers first at {}, of the {} addresses Leto knows for it",
                    summary.name,
                    join_address(&summary.address, summary.port),
                    entry.addresses.len()
                );
            }
            entry.summary = summary.clone();
            entry.door = door;
            entry.handle = Some(Arc::new(handle));
            entry.saved && (entry.addresses != before || entry.card != card_before)
        };
        if learned && let Err(error) = self.persist() {
            log::warn!("could not keep what Leto learned of {}: {error}", summary.name);
        }
        summary
    }
    /// The live handle of a host, asking the host first if there is none yet.
    fn handle(&self, id: &HostId) -> Result<Arc<Host>, Error> {
        let handle = self.lock().get(id).ok_or(Error::UnknownHost)?.handle.clone();
        if let Some(handle) = handle {
            return Ok(handle);
        }
        self.reach(id)?;
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
        // From taking what to save until it is saved, so that the file ends
        // with the latest.
        let _saving = self.saving.lock().unwrap_or_else(PoisonError::into_inner);
        let hosts = self
            .lock()
            .values()
            .filter(|entry| entry.saved)
            .map(|entry| SavedHost {
                id: entry.summary.id.clone(),
                name: entry.summary.name.clone(),
                address: entry.summary.address.clone(),
                port: entry.summary.port,
                other_addresses: entry
                    .addresses
                    .iter()
                    .filter(|(address, port)| {
                        (address, *port) != (&entry.summary.address, entry.summary.port)
                    })
                    .map(|(address, port)| SavedAddress { address: address.clone(), port: *port })
                    .collect(),
                kind: entry.summary.kind,
                certificate: entry.certificate.clone(),
                card: entry.card.map(wake::card_text),
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

/// What asking a host who it is needs, owned so that it can go to a thread
/// of its own.
#[derive(Clone)]
struct Asker {
    hosts: Arc<Mutex<BTreeMap<HostId, Entry>>>,
    identifier: ClientIdentifier,
    secret: ClientSecret,
    ways: Arc<tunnel::Ways>,
}

/// What a host answered, not yet recorded.
struct Answer {
    summary: HostSummary,
    handle: Host,
    /// The address the host gives itself on its own network.
    local: Option<String>,
    /// The address of its network card, when it tells.
    card: Option<wake::Card>,
    /// The door through Leto's tunnel it answered at, if that was the way.
    door: Option<Ipv4Addr>,
}

impl Asker {
    /// Asks the host at `address` who it is. A host this device is paired
    /// with must prove it still is that host.
    /// Through the tunnel, `door` is where to connect.
    fn ask(&self, address: &str, port: u16, door: Option<Ipv4Addr>) -> Result<Answer, Error> {
        let reach_at = door.map_or_else(|| address.to_owned(), |door| door.to_string());
        let mut handle = Host::new(reach_at.clone(), port, None)?;
        handle.update()?;
        let id = HostId(handle.unique_id()?.to_string());

        // A host this device is paired with must prove it still is that host,
        // and gets to say whether it still knows this device.
        let certificate = self
            .hosts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .and_then(|entry| entry.certificate.clone());
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
                    handle = Host::new(reach_at, port, None)?;
                    handle.update()?;
                }
            }
        }

        let summary = HostSummary {
            id,
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
            // Known once recorded.
            wakeable: false,
            tunnel: door.is_some(),
        };
        // Only to a device it is paired with: the others get zeros.
        let card =
            if paired { handle.mac().ok().flatten().map(|card| card.to_bytes()) } else { None };
        // Sunshine and Apollo give the address of theirs that the question
        // came in on: their address on the local network when asked from
        // further away, for example through a forwarded port.
        let local =
            handle.local_ip().ok().filter(|ip| !ip.is_unspecified()).map(|ip| ip.to_string());
        Ok(Answer { summary, handle, local, card, door })
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
