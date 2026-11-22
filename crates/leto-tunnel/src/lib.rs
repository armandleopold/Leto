//! A WireGuard tunnel inside Leto, for Leto's own connections to a PC.
//!
//! The system's WireGuard needs administrator rights to make a network card
//! of its own, and carries everything. This one needs neither: it is a UDP
//! socket like any other, and carries only what Leto hands it. WireGuard is
//! boringtun's, which only turns packets into packets; TCP inside the tunnel
//! is smoltcp's; UDP, the pictures and sound of a stream, is written by hand
//! in [`packet`] to stay out of a stack's way.
//!
//! The rest of Leto opens ordinary sockets: a [`Door`] is a local address
//! that leads through the tunnel to one address at the other end, port for
//! port. A [`Relay`] is the other end of a door, for a tunnel this side
//! answers, as the tests do.

use std::{
    collections::HashMap,
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, ToSocketAddrs, UdpSocket},
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicU16, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use boringtun::{
    noise::{Tunn, TunnResult, errors::WireGuardError},
    x25519,
};
use smoltcp::{iface::SocketHandle, socket::tcp};

pub mod config;
mod door;
pub mod packet;
mod stack;

pub use config::{Config, ConfigError, Range, public_key};
pub use door::{Door, Relay};
use stack::Stack;

/// The longest datagram the system hands over, and room for WireGuard's own
/// header around a packet of that size.
const DATAGRAM: usize = 65_536;
/// How often WireGuard's timers are looked at: they count in seconds.
const TIMERS: Duration = Duration::from_millis(100);
/// How long a thread waits for the system before looking whether to stop.
const PATIENCE: Duration = Duration::from_millis(200);
/// Datagrams kept for a port whose reader is late. Later ones are dropped,
/// as the network would.
const WAITING: usize = 4096;
/// What the tunnel's socket asks the system to hold of what arrives.
const SOCKET_BUFFER: usize = 4 * 1024 * 1024;
/// How long without an answer before the peer's name is looked up again:
/// its address at home may have changed.
const LOOK_UP_AGAIN: Duration = Duration::from_secs(30);
/// How long without an answer before the silence is explained: a handshake
/// is tried again every 5 seconds, so by then three went unanswered.
const SILENCE: Duration = Duration::from_secs(15);
/// How often a silence is written to the log.
const TELL_AGAIN: Duration = Duration::from_secs(30);
/// The first bytes of a WireGuard handshake initiation, and its length.
const INITIATION: ([u8; 4], usize) = ([1, 0, 0, 0], 148);

/// How the tunnel is doing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Status {
    /// Where the peer is, once known.
    pub endpoint: Option<SocketAddr>,
    /// How long ago the peer last proved who it is.
    pub handshake: Option<Duration>,
    /// Bytes sent and received inside the tunnel.
    pub sent: usize,
    pub received: usize,
    /// Why the peer cannot be reached, if it cannot.
    pub problem: Option<String>,
    /// What this end saw while the peer did not answer, or since it last
    /// did.
    pub silence: Silence,
}

/// What this end saw since the peer last answered, or since the tunnel
/// opened if it never did: what tells a network that drops WireGuard from a
/// server that does not know this end.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Silence {
    /// How long since the peer last answered, or since the tunnel opened.
    pub since: Duration,
    /// Whether the peer ever answered since the tunnel opened.
    pub answered_once: bool,
    /// Handshakes sent that got no answer.
    pub tries: u32,
    /// Datagrams that came back but were not the peer's, or not for these
    /// keys, and why the last one was refused.
    pub refused: u32,
    pub refusal: Option<String>,
    /// The system said nothing listens at the peer's port: an ICMP "port
    /// unreachable" came back. Windows tells it, Linux does not here.
    pub unreachable: bool,
    /// Why the system would not send the last datagram, if it would not.
    pub send_error: Option<String>,
}

impl Silence {
    /// Why the peer does not answer, in words for the person, once it has
    /// been silent long enough to say. `peer` is the peer as the file names
    /// it; `endpoint` where it was looked for.
    pub fn explain(&self, peer: &str, endpoint: Option<SocketAddr>) -> Option<String> {
        if self.since < SILENCE {
            return None;
        }
        let at = match endpoint {
            Some(endpoint) if endpoint.to_string() != peer => format!("{peer} ({endpoint})"),
            _ => peer.to_owned(),
        };
        let port =
            endpoint.map_or_else(|| "its".to_owned(), |endpoint| endpoint.port().to_string());
        let seconds = self.since.as_secs();
        Some(if let Some(error) = &self.send_error {
            format!("this device cannot send to {at}: {error}.")
        } else if self.refused > 0 {
            format!(
                "{at} answers, but the answer does not match this file's keys ({}). Check the \
                 server's public key and the preshared key in the file.",
                self.refusal.as_deref().unwrap_or("refused"),
            )
        } else if self.unreachable {
            format!(
                "nothing listens on UDP port {port} at {at}. Check the port in the file, and \
                 that the router at home forwards it to the WireGuard server."
            )
        } else {
            format!(
                "no answer from {at} after {} handshakes in {seconds} s. Either the server \
                 does not know this device's public key below, or the address is out of date, \
                 or this network or the router at home drops UDP port {port}.",
                self.tries,
            )
        })
    }
}

/// One end of a WireGuard tunnel. Stops when dropped.
pub struct Tunnel {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

struct Shared {
    config: Config,
    socket: UdpSocket,
    wireguard: Mutex<WireGuard>,
    stack: Mutex<Stack>,
    /// Told after each round of the stack: a connection may have news.
    changed: Condvar,
    /// Asks the stack for a round now.
    poke: Mutex<bool>,
    poked: Condvar,
    /// Who reads the datagrams that arrive at each port of this end.
    ports: Mutex<HashMap<u16, SyncSender<Datagram>>>,
    next_id: AtomicU16,
    stopped: AtomicBool,
    problem: Mutex<Option<String>>,
    watch: Mutex<Watch>,
}

/// What [`Silence`] is made from.
struct Watch {
    /// When the peer last answered, or when the tunnel opened.
    since: Instant,
    answered_once: bool,
    /// Where the peer last answered from.
    answered_from: Option<SocketAddr>,
    tries: u32,
    refused: u32,
    refusal: Option<String>,
    unreachable: bool,
    send_error: Option<String>,
    /// When the silence was last written to the log.
    told: Option<Instant>,
}

impl Watch {
    fn new() -> Self {
        Self {
            since: Instant::now(),
            answered_once: false,
            answered_from: None,
            tries: 0,
            refused: 0,
            refusal: None,
            unreachable: false,
            send_error: None,
            told: None,
        }
    }

    fn silence(&self) -> Silence {
        Silence {
            since: self.since.elapsed(),
            answered_once: self.answered_once,
            tries: self.tries,
            refused: self.refused,
            refusal: self.refusal.clone(),
            unreachable: self.unreachable,
            send_error: self.send_error.clone(),
        }
    }
}

struct WireGuard {
    tunnel: Tunn,
    endpoint: Option<SocketAddr>,
    looked_up: Option<Instant>,
    out: Vec<u8>,
}

/// One datagram that arrived through the tunnel.
struct Datagram {
    from: SocketAddrV4,
    payload: Vec<u8>,
}

impl Tunnel {
    /// Opens this end of the tunnel and greets the peer, if the file says
    /// where it is. A peer known by name is looked up now, then again when
    /// it stops answering.
    pub fn start(config: Config) -> io::Result<Self> {
        let endpoint = config.endpoint.as_deref().map(look_up).transpose()?;
        let any: SocketAddr = match endpoint {
            Some(SocketAddr::V6(_)) => (std::net::Ipv6Addr::UNSPECIFIED, 0).into(),
            _ => (Ipv4Addr::UNSPECIFIED, 0).into(),
        };
        let socket = open_socket(SocketAddr::new(any.ip(), config.listen_port.unwrap_or(0)))?;
        socket.set_read_timeout(Some(PATIENCE))?;

        let tunnel = Tunn::new(
            x25519::StaticSecret::from(config.private_key),
            x25519::PublicKey::from(config.peer_public_key),
            config.preshared_key,
            config.keepalive,
            // This end's index among its peers: it has one.
            0,
            None,
        );
        let mtu = usize::from(config.mtu);
        let shared = Arc::new(Shared {
            stack: Mutex::new(Stack::new(config.address, mtu)),
            config,
            socket,
            wireguard: Mutex::new(WireGuard {
                tunnel,
                endpoint,
                looked_up: endpoint.map(|_| Instant::now()),
                out: vec![0; DATAGRAM + 148],
            }),
            changed: Condvar::new(),
            poke: Mutex::new(false),
            poked: Condvar::new(),
            ports: Mutex::new(HashMap::new()),
            next_id: AtomicU16::new(1),
            stopped: AtomicBool::new(false),
            problem: Mutex::new(None),
            watch: Mutex::new(Watch::new()),
        });
        match (&shared.config.endpoint, endpoint) {
            (Some(name), Some(found)) => log::info!(
                "the tunnel's peer {name} is at {found}; this end sends from {}",
                describe_local(&shared.socket)
            ),
            _ => log::info!("the tunnel waits for its peer to come"),
        }
        shared.greet();

        let mut threads = Vec::new();
        for (name, work) in [
            ("leto: tunnel in", Shared::receive as fn(&Shared)),
            ("leto: tunnel rounds", Shared::rounds as fn(&Shared)),
        ] {
            let shared = Arc::clone(&shared);
            threads
                .push(thread::Builder::new().name(name.to_owned()).spawn(move || work(&shared))?);
        }
        Ok(Self { shared, threads })
    }

    pub fn config(&self) -> &Config {
        &self.shared.config
    }

    /// Where this end listens on the system's network.
    pub fn local_address(&self) -> io::Result<SocketAddr> {
        self.shared.socket.local_addr()
    }

    pub fn status(&self) -> Status {
        let wireguard = lock(&self.shared.wireguard);
        let (handshake, sent, received, _, _) = wireguard.tunnel.stats();
        Status {
            endpoint: wireguard.endpoint,
            handshake,
            sent,
            received,
            problem: lock(&self.shared.problem).clone(),
            silence: lock(&self.shared.watch).silence(),
        }
    }

    /// Opens a TCP connection to `to` through the tunnel.
    pub fn connect(&self, to: SocketAddrV4, timeout: Duration) -> io::Result<Connection> {
        let handle = lock(&self.shared.stack).connect(to)?;
        self.shared.wake();
        let deadline = Instant::now() + timeout;
        let mut stack = lock(&self.shared.stack);
        loop {
            match stack.socket(handle).state() {
                tcp::State::Established => break,
                tcp::State::Closed => {
                    stack.drop_now(handle);
                    return Err(io::ErrorKind::ConnectionRefused.into());
                }
                _ => {}
            }
            let now = Instant::now();
            if now >= deadline {
                stack.drop_now(handle);
                return Err(io::ErrorKind::TimedOut.into());
            }
            stack = self.shared.wait(stack, deadline - now);
        }
        drop(stack);
        Ok(Connection::new(Arc::clone(&self.shared), handle, to))
    }

    /// Waits for TCP connections to `port` of this end.
    pub fn listen(&self, port: u16) -> io::Result<Listener> {
        let mut stack = lock(&self.shared.stack);
        let waiting =
            (0..Listener::BACKLOG).map(|_| stack.listen(port)).collect::<io::Result<_>>()?;
        Ok(Listener { shared: Arc::clone(&self.shared), port, waiting: Mutex::new(waiting) })
    }

    /// Takes the UDP datagrams that arrive at `port` of this end, or at a
    /// port of its own.
    pub fn bind(&self, port: Option<u16>) -> io::Result<Datagrams> {
        let (sender, incoming) = mpsc::sync_channel(WAITING);
        let mut ports = lock(&self.shared.ports);
        let port = match port {
            Some(port) if ports.contains_key(&port) => {
                return Err(io::ErrorKind::AddrInUse.into());
            }
            Some(port) => port,
            None => (0..)
                .map(|_| self.shared.next_port())
                .find(|port| !ports.contains_key(port))
                .ok_or(io::ErrorKind::AddrInUse)?,
        };
        ports.insert(port, sender);
        Ok(Datagrams { shared: Arc::clone(&self.shared), port, incoming: Mutex::new(incoming) })
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::SeqCst);
        self.shared.wake();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

impl Shared {
    /// Starts a handshake, so that the tunnel is up by the time it is used.
    fn greet(&self) {
        let mut wireguard = lock(&self.wireguard);
        let WireGuard { tunnel, endpoint, out, .. } = &mut *wireguard;
        let Some(endpoint) = *endpoint else { return };
        if let TunnResult::WriteToNetwork(packet) = tunnel.format_handshake_initiation(out, false) {
            self.send(packet, endpoint);
        }
    }

    /// Hands one IP packet to the tunnel.
    fn send_ip(&self, packet: &[u8]) {
        let mut wireguard = lock(&self.wireguard);
        let WireGuard { tunnel, endpoint, out, .. } = &mut *wireguard;
        let Some(endpoint) = *endpoint else { return };
        match tunnel.encapsulate(packet, out) {
            TunnResult::WriteToNetwork(datagram) => self.send(datagram, endpoint),
            TunnResult::Err(error) => log::debug!("the tunnel did not take a packet: {error:?}"),
            _ => {}
        }
    }

    fn send(&self, datagram: &[u8], to: SocketAddr) {
        let result = self.socket.send_to(datagram, to);
        let mut watch = lock(&self.watch);
        match result {
            Ok(_) => {
                watch.send_error = None;
                if datagram.len() == INITIATION.1 && datagram.starts_with(&INITIATION.0) {
                    watch.tries += 1;
                    log::debug!("the tunnel sent handshake {} to {to}", watch.tries);
                }
            }
            Err(error) => {
                if watch.send_error.is_none() {
                    log::warn!("the tunnel could not send to {to}: {error}");
                }
                watch.send_error = Some(error.to_string());
            }
        }
    }

    /// The peer answered and proved who it is, from `from`.
    fn answered(&self, from: SocketAddr) {
        let mut watch = lock(&self.watch);
        if watch.answered_from != Some(from) || watch.since.elapsed() >= SILENCE {
            if watch.answered_once {
                log::info!("the tunnel's peer answers again, from {from}");
            } else {
                log::info!(
                    "the tunnel's peer answered from {from} after {} handshakes in {} ms",
                    watch.tries,
                    watch.since.elapsed().as_millis()
                );
            }
        }
        *watch = Watch { answered_once: true, answered_from: Some(from), ..Watch::new() };
    }

    /// Reads what the peer sends, until the tunnel stops.
    fn receive(&self) {
        let mut datagram = vec![0; DATAGRAM];
        let mut arrived = Vec::new();
        while !self.stopped.load(Ordering::SeqCst) {
            let (length, from) = match self.socket.recv_from(&mut datagram) {
                Ok(received) => received,
                // On Windows, a peer said nobody listens at its port.
                Err(error) if error.kind() == io::ErrorKind::ConnectionReset => {
                    let mut watch = lock(&self.watch);
                    if !watch.unreachable {
                        log::warn!("the tunnel's peer says nothing listens at its port");
                    }
                    watch.unreachable = true;
                    continue;
                }
                // Nothing came.
                Err(error) if is_passing(&error) => continue,
                Err(error) => {
                    log::debug!("the tunnel could not read: {error}");
                    thread::sleep(PATIENCE);
                    continue;
                }
            };
            let mut refused = None;
            let (proved, session) = {
                let mut wireguard = lock(&self.wireguard);
                let WireGuard { tunnel, endpoint, out, .. } = &mut *wireguard;
                let before = tunnel.stats().0;
                let mut first = Some(&datagram[..length]);
                loop {
                    let result = match first.take() {
                        Some(datagram) => tunnel.decapsulate(Some(from.ip()), datagram, out),
                        // What was waiting for the handshake to end.
                        None => tunnel.decapsulate(None, &[], out),
                    };
                    match result {
                        TunnResult::WriteToNetwork(answer) => {
                            // The peer proved who it is: this is where it is.
                            *endpoint = Some(from);
                            self.send(answer, from);
                            continue;
                        }
                        TunnResult::WriteToTunnelV4(packet, _) => {
                            *endpoint = Some(from);
                            arrived.push(packet.to_vec());
                        }
                        TunnResult::Err(error) => {
                            note_refusal(&error, from);
                            refused = Some(error);
                        }
                        TunnResult::WriteToTunnelV6(..) | TunnResult::Done => {}
                    }
                    break;
                }
                // A handshake that just ended: the peer is there.
                let after = tunnel.stats().0;
                (
                    after.is_some_and(|after| before.is_none_or(|before| after < before)),
                    after.is_some(),
                )
            };
            if proved {
                self.answered(from);
            } else if let Some(error) = refused
                && !session
            {
                // Without a session: late or doubled datagrams of one are no
                // news.
                let mut watch = lock(&self.watch);
                watch.refused += 1;
                watch.refusal = Some(format!("{error:?}"));
            }
            for packet in arrived.drain(..) {
                self.deliver(packet);
            }
        }
    }

    /// Takes one packet that came out of the tunnel to where it goes.
    fn deliver(&self, packet: Vec<u8>) {
        let Some(ip) = packet::read_ipv4(&packet) else { return };
        if ip.destination != self.config.address {
            return;
        }
        if ip.fragment {
            log::debug!("a packet from {} came in pieces, which Leto does not join", ip.source);
            return;
        }
        match ip.protocol {
            packet::UDP => {
                let Some(udp) = packet::read_udp(ip.payload) else { return };
                if let Some(reader) = lock(&self.ports).get(&udp.destination_port) {
                    // A reader that is late loses datagrams, as on a network.
                    let _ = reader.try_send(Datagram {
                        from: SocketAddrV4::new(ip.source, udp.source_port),
                        payload: udp.payload.to_vec(),
                    });
                }
            }
            packet::TCP => {
                lock(&self.stack).take(packet);
                self.wake();
            }
            _ => {}
        }
    }

    /// Keeps WireGuard's timers and the TCP connections going.
    fn rounds(&self) {
        let mut timers = Instant::now();
        let mut out = vec![0; 148];
        while !self.stopped.load(Ordering::SeqCst) {
            if timers.elapsed() >= TIMERS {
                timers = Instant::now();
                self.keep_up(&mut out);
            }
            let (outgoing, delay) = lock(&self.stack).poll();
            self.changed.notify_all();
            for packet in &outgoing {
                self.send_ip(packet);
            }
            if !outgoing.is_empty() {
                // What was sent may have made room, or more may be ready.
                continue;
            }
            let wait = delay.unwrap_or(TIMERS).min(TIMERS);
            let mut poke = lock(&self.poke);
            if !*poke {
                poke =
                    self.poked.wait_timeout(poke, wait).unwrap_or_else(PoisonError::into_inner).0;
            }
            *poke = false;
        }
        self.changed.notify_all();
    }

    /// WireGuard's timers: a new handshake when the keys are old, signs of
    /// life, and a peer known by name looked up again when it is silent.
    fn keep_up(&self, out: &mut [u8]) {
        self.tell_silence();
        let look_up_again = {
            let mut wireguard = lock(&self.wireguard);
            let WireGuard { tunnel, endpoint, looked_up, .. } = &mut *wireguard;
            let silent = tunnel.stats().0.is_none_or(|since| since > LOOK_UP_AGAIN);
            let due = looked_up.is_none_or(|at| at.elapsed() > LOOK_UP_AGAIN);
            if self.config.endpoint.is_some() && silent && due {
                *looked_up = Some(Instant::now());
                true
            } else {
                if let Some(endpoint) = *endpoint {
                    match tunnel.update_timers(out) {
                        TunnResult::WriteToNetwork(datagram) => self.send(datagram, endpoint),
                        TunnResult::Err(WireGuardError::ConnectionExpired) => {}
                        TunnResult::Err(error) => log::debug!("tunnel timers: {error:?}"),
                        _ => {}
                    }
                }
                false
            }
        };
        if look_up_again && let Some(name) = &self.config.endpoint {
            // Not while holding WireGuard: a look-up can take seconds.
            match look_up(name) {
                Ok(found) => {
                    let before = lock(&self.wireguard).endpoint.replace(found);
                    if before != Some(found) {
                        log::info!("the tunnel's peer {name} is now at {found}");
                    }
                    *lock(&self.problem) = None;
                    self.greet();
                }
                Err(error) => {
                    log::debug!("could not look up the tunnel's peer {name}: {error}");
                    *lock(&self.problem) = Some(format!("{name} cannot be found: {error}"));
                }
            }
        }
    }

    /// Writes to the log why the peer does not answer, now and then while it
    /// does not.
    fn tell_silence(&self) {
        let (endpoint, session) = {
            let wireguard = lock(&self.wireguard);
            (wireguard.endpoint, wireguard.tunnel.stats().0.is_some())
        };
        if session {
            return;
        }
        let silence = {
            let mut watch = lock(&self.watch);
            let silence = watch.silence();
            if silence.since < SILENCE || watch.told.is_some_and(|told| told.elapsed() < TELL_AGAIN)
            {
                return;
            }
            watch.told = Some(Instant::now());
            silence
        };
        let peer = self.config.endpoint.as_deref().unwrap_or("the peer");
        if let Some(why) = silence.explain(peer, endpoint) {
            log::warn!(
                "the tunnel is silent: {why} ({} refused, unreachable: {})",
                silence.refused,
                silence.unreachable
            );
        }
    }

    /// Asks the stack for a round now.
    fn wake(&self) {
        *lock(&self.poke) = true;
        self.poked.notify_one();
    }

    /// Waits for news of the connections, for at most `timeout`.
    fn wait<'a>(&self, stack: MutexGuard<'a, Stack>, timeout: Duration) -> MutexGuard<'a, Stack> {
        self.changed.wait_timeout(stack, timeout).unwrap_or_else(PoisonError::into_inner).0
    }

    fn next_port(&self) -> u16 {
        // Ports the system would give, kept apart from the ones of TCP.
        49152 + self.next_id.fetch_add(1, Ordering::Relaxed) % 8192
    }

    fn stopped(&self) -> io::Result<()> {
        if self.stopped.load(Ordering::SeqCst) {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "the tunnel was closed"));
        }
        Ok(())
    }
}

/// A TCP connection through the tunnel. Reads and writes from two threads at
/// once, as a `TcpStream` does, through `&Connection`.
pub struct Connection {
    shared: Arc<Shared>,
    handle: SocketHandle,
    peer: SocketAddrV4,
    read_timeout: Mutex<Option<Duration>>,
    write_timeout: Mutex<Option<Duration>>,
}

impl Connection {
    fn new(shared: Arc<Shared>, handle: SocketHandle, peer: SocketAddrV4) -> Self {
        Self {
            shared,
            handle,
            peer,
            read_timeout: Mutex::new(None),
            write_timeout: Mutex::new(None),
        }
    }

    pub fn peer(&self) -> SocketAddrV4 {
        self.peer
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) {
        *lock(&self.read_timeout) = timeout;
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) {
        *lock(&self.write_timeout) = timeout;
    }

    /// Says nothing more will be written. Reading goes on.
    pub fn shutdown_write(&self) {
        lock(&self.shared.stack).socket(self.handle).close();
        self.shared.wake();
    }

    /// Waits until `ready` says the connection can go on, as a read or a
    /// write would.
    fn wait_for<T>(
        &self,
        timeout: Option<Duration>,
        mut ready: impl FnMut(&mut tcp::Socket<'static>) -> Option<io::Result<T>>,
    ) -> io::Result<T> {
        let deadline = timeout.map(|timeout| Instant::now() + timeout);
        let mut stack = lock(&self.shared.stack);
        loop {
            self.shared.stopped()?;
            if let Some(result) = ready(stack.socket(self.handle)) {
                drop(stack);
                // An answer to send: room was made, or data is to go.
                self.shared.wake();
                return result;
            }
            let wait = match deadline {
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        return Err(io::ErrorKind::TimedOut.into());
                    }
                    (deadline - now).min(PATIENCE)
                }
                None => PATIENCE,
            };
            stack = self.shared.wait(stack, wait);
        }
    }
}

impl Read for &Connection {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let timeout = *lock(&self.read_timeout);
        self.wait_for(timeout, |socket| {
            if socket.can_recv() {
                return Some(
                    socket.recv_slice(buffer).map_err(|_| io::ErrorKind::ConnectionReset.into()),
                );
            }
            // The other side said it is done, or the connection is gone.
            (!socket.may_recv()).then_some(Ok(0))
        })
    }
}

impl Write for &Connection {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }
        let timeout = *lock(&self.write_timeout);
        self.wait_for(timeout, |socket| {
            if !socket.may_send() {
                return Some(Err(io::ErrorKind::BrokenPipe.into()));
            }
            socket
                .can_send()
                .then(|| socket.send_slice(data).map_err(|_| io::ErrorKind::BrokenPipe.into()))
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Read for Connection {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        (&*self).read(buffer)
    }
}

impl Write for Connection {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        (&*self).write(data)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&*self).flush()
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        lock(&self.shared.stack).release(self.handle);
        self.shared.wake();
    }
}

/// TCP connections arriving at one port of this end of the tunnel.
pub struct Listener {
    shared: Arc<Shared>,
    port: u16,
    waiting: Mutex<Vec<SocketHandle>>,
}

impl Listener {
    /// Connections that may arrive at once.
    const BACKLOG: usize = 8;

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The next connection, waiting at most `timeout`.
    pub fn accept(&self, timeout: Duration) -> io::Result<Connection> {
        let deadline = Instant::now() + timeout;
        let mut waiting = lock(&self.waiting);
        let mut stack = lock(&self.shared.stack);
        loop {
            self.shared.stopped()?;
            for slot in waiting.iter_mut() {
                let socket = stack.socket(*slot);
                match socket.state() {
                    tcp::State::Established | tcp::State::CloseWait => {
                        let peer = match socket.remote_endpoint() {
                            Some(endpoint) => match endpoint.addr {
                                smoltcp::wire::IpAddress::Ipv4(ip) => {
                                    SocketAddrV4::new(ip, endpoint.port)
                                }
                            },
                            None => continue,
                        };
                        let accepted = std::mem::replace(slot, stack.listen(self.port)?);
                        drop(stack);
                        self.shared.wake();
                        return Ok(Connection::new(Arc::clone(&self.shared), accepted, peer));
                    }
                    // A connection that failed before it was taken.
                    tcp::State::Closed => {
                        stack.drop_now(*slot);
                        *slot = stack.listen(self.port)?;
                    }
                    _ => {}
                }
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(io::ErrorKind::TimedOut.into());
            }
            stack = self.shared.wait(stack, (deadline - now).min(PATIENCE));
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let mut waiting = lock(&self.waiting);
        let mut stack = lock(&self.shared.stack);
        for handle in waiting.drain(..) {
            stack.drop_now(handle);
        }
    }
}

/// UDP datagrams to and from one port of this end of the tunnel.
pub struct Datagrams {
    shared: Arc<Shared>,
    port: u16,
    incoming: Mutex<Receiver<Datagram>>,
}

impl Datagrams {
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Sends one datagram. One the tunnel cannot carry whole is an error.
    pub fn send_to(&self, payload: &[u8], to: SocketAddrV4) -> io::Result<()> {
        self.shared.stopped()?;
        let source = SocketAddrV4::new(self.shared.config.address, self.port);
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        let packet = packet::write_udp(source, to, id, payload)
            .filter(|packet| packet.len() <= usize::from(self.shared.config.mtu))
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "a datagram too large for the tunnel")
            })?;
        self.shared.send_ip(&packet);
        Ok(())
    }

    /// The next datagram and where it came from, waiting at most `timeout`.
    pub fn recv_from(&self, timeout: Duration) -> io::Result<(Vec<u8>, SocketAddrV4)> {
        match lock(&self.incoming).recv_timeout(timeout) {
            Ok(datagram) => Ok((datagram.payload, datagram.from)),
            Err(RecvTimeoutError::Timeout) => Err(io::ErrorKind::TimedOut.into()),
            Err(RecvTimeoutError::Disconnected) => Err(io::ErrorKind::NotConnected.into()),
        }
    }
}

impl Drop for Datagrams {
    fn drop(&mut self) {
        lock(&self.shared.ports).remove(&self.port);
    }
}

/// The socket of the tunnel on the system's network, with room for a burst:
/// a picture of a stream arrives as a hundred datagrams at once, and the
/// system's usual buffer, 64 KB on Windows, would drop most of them.
fn open_socket(address: SocketAddr) -> io::Result<UdpSocket> {
    let domain = socket2::Domain::for_address(address);
    let socket = socket2::Socket::new(domain, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
    // The system may give less, as Linux does without being told it may.
    for size in [SOCKET_BUFFER, SOCKET_BUFFER / 4] {
        if socket.set_recv_buffer_size(size).is_ok() {
            break;
        }
    }
    let _ = socket.set_send_buffer_size(SOCKET_BUFFER / 4);
    socket.bind(&address.into())?;
    Ok(socket.into())
}

/// Finds where a peer written `name:port` is.
fn look_up(name: &str) -> io::Result<SocketAddr> {
    let mut found: Vec<SocketAddr> = name.to_socket_addrs()?.collect();
    // IPv4 first: the most common way home, and what the socket is.
    found.sort_by_key(SocketAddr::is_ipv6);
    if found.len() > 1 {
        log::debug!("the tunnel's peer {name} has the addresses {found:?}");
    }
    found.into_iter().next().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no address"))
}

/// Where this end's socket is, for the log.
fn describe_local(socket: &UdpSocket) -> String {
    socket.local_addr().map_or_else(|error| format!("an unknown port ({error})"), |a| a.to_string())
}

/// An error a receiving socket gives that only means to go on.
fn is_passing(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock
            | io::ErrorKind::TimedOut
            | io::ErrorKind::Interrupted
            | io::ErrorKind::ConnectionReset
    )
}

/// A datagram WireGuard refused: someone else's, a duplicate, or a peer with
/// other keys.
fn note_refusal(error: &WireGuardError, from: SocketAddr) {
    log::debug!("the tunnel refused a datagram from {from}: {error:?}");
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn silent(seconds: u64) -> Silence {
        Silence { since: Duration::from_secs(seconds), tries: 4, ..Silence::default() }
    }

    #[test]
    fn a_silence_is_explained_only_once_it_lasted() {
        let at = Some(SocketAddr::from(([203, 0, 113, 7], 51821)));
        assert_eq!(silent(5).explain("home.example.org:51821", at), None);
        let why = silent(20).explain("home.example.org:51821", at).unwrap();
        assert!(why.contains("home.example.org:51821 (203.0.113.7:51821)"), "{why}");
        assert!(why.contains("after 4 handshakes in 20 s"), "{why}");
        assert!(why.contains("public key"), "{why}");
        assert!(why.contains("UDP port 51821"), "{why}");
    }

    #[test]
    fn what_came_back_says_more_than_nothing() {
        let at = Some(SocketAddr::from(([203, 0, 113, 7], 51821)));
        let refused = Silence { refused: 2, refusal: Some("InvalidAeadTag".into()), ..silent(20) };
        assert!(refused.explain("x:51821", at).unwrap().contains("InvalidAeadTag"));
        let closed = Silence { unreachable: true, ..silent(20) };
        assert!(closed.explain("x:51821", at).unwrap().contains("nothing listens"));
        let blocked = Silence { send_error: Some("access denied".into()), ..closed };
        assert!(blocked.explain("x:51821", at).unwrap().contains("cannot send"));
        // A peer written as an address is not repeated.
        let plain = silent(20).explain("203.0.113.7:51821", at).unwrap();
        assert!(!plain.contains("(203.0.113.7"), "{plain}");
    }
}
