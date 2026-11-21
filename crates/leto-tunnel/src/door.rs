//! Doors between this machine's network and the tunnel.
//!
//! A [`Door`] is a local address, `127.77.0.1` for the first, that leads
//! through the tunnel to one address at the other end, port for port: what
//! connects to `127.77.0.1:47989` reaches `10.8.0.3:47989`. The rest of Leto
//! opens ordinary sockets to it. A [`Relay`] is the other way round: what
//! arrives in the tunnel at some ports goes on to an address of this
//! machine's network.

use std::{
    collections::HashMap,
    io::{self, Read, Write},
    net::{Ipv4Addr, Shutdown, SocketAddr, SocketAddrV4, TcpListener, TcpStream, UdpSocket},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crate::{Connection, Datagrams, Listener, PATIENCE, Tunnel, lock};

/// How long a connection through the tunnel may take to open.
const OPENING: Duration = Duration::from_secs(5);
/// A UDP flow nobody used for this long is forgotten.
const IDLE: Duration = Duration::from_secs(120);
/// What is read at once from either side.
const CHUNK: usize = 64 * 1024;

/// The next local address to give a door, among `127.77.0.1` to
/// `127.77.0.250`.
static NEXT_DOOR: AtomicU8 = AtomicU8::new(1);

/// A local address that leads through the tunnel to `target`, port for port.
/// Closes when dropped.
pub struct Door {
    shared: Arc<DoorShared>,
}

struct DoorShared {
    tunnel: Arc<Tunnel>,
    local: Ipv4Addr,
    target: Ipv4Addr,
    stopped: AtomicBool,
    /// The ports open, so that one is opened once.
    open: Mutex<Vec<(bool, u16)>>,
}

impl Door {
    /// Opens a door to `target` for the TCP ports and UDP ports given.
    pub fn open(
        tunnel: Arc<Tunnel>,
        target: Ipv4Addr,
        tcp_ports: &[u16],
        udp_ports: &[u16],
    ) -> io::Result<Self> {
        let local = free_address(tcp_ports.first().copied())?;
        let door = Self {
            shared: Arc::new(DoorShared {
                tunnel,
                local,
                target,
                stopped: AtomicBool::new(false),
                open: Mutex::new(Vec::new()),
            }),
        };
        // The first port is the door's reason to be. Another port that is
        // taken, or below 1024 where some systems want an administrator,
        // only leaves that one closed.
        let mut first = true;
        for (port, tcp) in tcp_ports
            .iter()
            .map(|port| (*port, true))
            .chain(udp_ports.iter().map(|port| (*port, false)))
        {
            let opened = if tcp { door.forward_tcp(port) } else { door.forward_udp(port) };
            match opened {
                Ok(()) => {}
                Err(error) if first => return Err(error),
                Err(error) if port < 1024 => {
                    log::debug!("the door at {local} lets nothing through at port {port}: {error}");
                }
                Err(error) => {
                    log::warn!("the door at {local} lets nothing through at port {port}: {error}");
                }
            }
            first = false;
        }
        log::debug!("a door at {local} leads through the tunnel to {target}");
        Ok(door)
    }

    /// The local address that leads to the target.
    pub fn address(&self) -> Ipv4Addr {
        self.shared.local
    }

    pub fn target(&self) -> Ipv4Addr {
        self.shared.target
    }

    /// Lets TCP through at one more port.
    pub fn forward_tcp(&self, port: u16) -> io::Result<()> {
        if !self.claim(false, port) {
            return Ok(());
        }
        let listener = TcpListener::bind((self.shared.local, port))?;
        let shared = Arc::clone(&self.shared);
        spawn("leto: door", move || {
            for local in listener.incoming() {
                if shared.stopped.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(local) = local else { continue };
                let shared = Arc::clone(&shared);
                let started = spawn("leto: door connection", move || {
                    let to = SocketAddrV4::new(shared.target, port);
                    match shared.tunnel.connect(to, OPENING) {
                        Ok(far) => pump(local, far),
                        Err(error) => log::debug!("no way through the tunnel to {to}: {error}"),
                    }
                });
                if let Err(error) = started {
                    log::error!("could not start a worker thread: {error}");
                }
            }
        })
    }

    /// Lets UDP through at one more port.
    pub fn forward_udp(&self, port: u16) -> io::Result<()> {
        if !self.claim(true, port) {
            return Ok(());
        }
        let socket = Arc::new(UdpSocket::bind((self.shared.local, port))?);
        socket.set_read_timeout(Some(PATIENCE))?;
        let shared = Arc::clone(&self.shared);
        spawn("leto: door datagrams", move || {
            let to = SocketAddrV4::new(shared.target, port);
            // One way through the tunnel for each local sender, so that the
            // answers find their way back.
            let mut flows: HashMap<SocketAddr, (Arc<Datagrams>, Instant)> = HashMap::new();
            let mut datagram = vec![0; CHUNK];
            while !shared.stopped.load(Ordering::SeqCst) {
                let (length, sender) = match socket.recv_from(&mut datagram) {
                    Ok(received) => received,
                    Err(_) => {
                        flows.retain(|_, (_, used)| used.elapsed() < IDLE);
                        continue;
                    }
                };
                let flow = match flows.get_mut(&sender) {
                    Some((flow, used)) => {
                        *used = Instant::now();
                        Arc::clone(flow)
                    }
                    None => {
                        let Ok(flow) = shared.tunnel.bind(None).map(Arc::new) else { continue };
                        let (back, socket, shared) =
                            (Arc::clone(&flow), Arc::clone(&socket), Arc::clone(&shared));
                        let answers = spawn("leto: door answers", move || {
                            answer(&shared, &back, &socket, to, sender)
                        });
                        if answers.is_err() {
                            continue;
                        }
                        flows.insert(sender, (Arc::clone(&flow), Instant::now()));
                        flow
                    }
                };
                if let Err(error) = flow.send_to(&datagram[..length], to) {
                    log::debug!("a datagram to {to} did not go through the tunnel: {error}");
                }
            }
        })
    }

    /// Notes a port as open. False if it was already.
    fn claim(&self, udp: bool, port: u16) -> bool {
        let mut open = lock(&self.shared.open);
        if open.contains(&(udp, port)) {
            return false;
        }
        open.push((udp, port));
        true
    }
}

impl Drop for Door {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::SeqCst);
        // Each listener waits for a connection: one each, to see the end.
        for (udp, port) in lock(&self.shared.open).iter() {
            if !udp {
                let _ = TcpStream::connect_timeout(
                    &SocketAddr::from((self.shared.local, *port)),
                    PATIENCE,
                );
            }
        }
    }
}

/// Passes what `to` answers through the tunnel back to the local sender,
/// until the flow is forgotten or the door closes.
fn answer(
    shared: &DoorShared,
    flow: &Arc<Datagrams>,
    socket: &UdpSocket,
    to: SocketAddrV4,
    sender: SocketAddr,
) {
    let mut last = Instant::now();
    // The door's loop holds the other reference while the flow is in use.
    while !shared.stopped.load(Ordering::SeqCst)
        && (Arc::strong_count(flow) > 1 || last.elapsed() < IDLE)
    {
        match flow.recv_from(PATIENCE) {
            Ok((payload, from)) if from == to => {
                last = Instant::now();
                let _ = socket.send_to(&payload, sender);
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                if Arc::strong_count(flow) == 1 {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

/// What arrives in a tunnel at some ports, passed on to an address of this
/// machine's network, port for port. Closes when dropped.
pub struct Relay {
    stopped: Arc<AtomicBool>,
}

impl Relay {
    /// Passes TCP at `tcp_ports` and UDP at `udp_ports` of the tunnel's
    /// address on to the same ports of `target`.
    pub fn start(
        tunnel: &Arc<Tunnel>,
        target: Ipv4Addr,
        tcp_ports: &[u16],
        udp_ports: &[u16],
    ) -> io::Result<Self> {
        let stopped = Arc::new(AtomicBool::new(false));
        for port in tcp_ports {
            let listener = tunnel.listen(*port)?;
            let stopped = Arc::clone(&stopped);
            let to = SocketAddrV4::new(target, *port);
            spawn("leto: relay", move || relay_tcp(&listener, to, &stopped))?;
        }
        for port in udp_ports {
            let datagrams = Arc::new(tunnel.bind(Some(*port))?);
            let stopped = Arc::clone(&stopped);
            let to = SocketAddrV4::new(target, *port);
            spawn("leto: relay datagrams", move || relay_udp(&datagrams, to, &stopped))?;
        }
        Ok(Self { stopped })
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
    }
}

fn relay_tcp(listener: &Listener, to: SocketAddrV4, stopped: &AtomicBool) {
    while !stopped.load(Ordering::SeqCst) {
        match listener.accept(PATIENCE) {
            Ok(far) => {
                let started = spawn("leto: relay connection", move || {
                    match TcpStream::connect_timeout(&to.into(), OPENING) {
                        Ok(local) => pump(local, far),
                        Err(error) => log::debug!("the relay could not reach {to}: {error}"),
                    }
                });
                if started.is_err() {
                    break;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
    }
}

fn relay_udp(datagrams: &Arc<Datagrams>, to: SocketAddrV4, stopped: &Arc<AtomicBool>) {
    // One local socket for each sender in the tunnel, so that the answers
    // find their way back.
    let mut flows: HashMap<SocketAddrV4, Arc<UdpSocket>> = HashMap::new();
    while !stopped.load(Ordering::SeqCst) {
        let (payload, sender) = match datagrams.recv_from(PATIENCE) {
            Ok(received) => received,
            Err(error) if error.kind() == io::ErrorKind::TimedOut => continue,
            Err(_) => break,
        };
        let socket = match flows.get(&sender) {
            Some(socket) => Arc::clone(socket),
            None => {
                let Ok(socket) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) else { continue };
                if socket.set_read_timeout(Some(PATIENCE)).is_err() {
                    continue;
                }
                let socket = Arc::new(socket);
                let (back, datagrams, stopped) =
                    (Arc::clone(&socket), Arc::clone(datagrams), Arc::clone(stopped));
                let answers = spawn("leto: relay answers", move || {
                    let mut buffer = vec![0; CHUNK];
                    while !stopped.load(Ordering::SeqCst) {
                        match back.recv_from(&mut buffer) {
                            Ok((length, from)) if from == SocketAddr::V4(to) => {
                                let _ = datagrams.send_to(&buffer[..length], sender);
                            }
                            Ok(_) => {}
                            Err(_) => {}
                        }
                    }
                });
                if answers.is_err() {
                    continue;
                }
                flows.insert(sender, Arc::clone(&socket));
                socket
            }
        };
        let _ = socket.send_to(&payload, to);
    }
}

/// Passes bytes both ways between a local connection and one through the
/// tunnel, until both sides are done.
fn pump(local: TcpStream, far: Connection) {
    let _ = local.set_nodelay(true);
    let far = Arc::new(far);
    let Ok(local_reader) = local.try_clone() else { return };
    let outward = {
        let far = Arc::clone(&far);
        thread::Builder::new().name("leto: door out".to_owned()).spawn(move || {
            let mut local = local_reader;
            let mut chunk = vec![0; CHUNK];
            loop {
                match local.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(length) => {
                        if (&*far).write_all(&chunk[..length]).is_err() {
                            break;
                        }
                    }
                }
            }
            far.shutdown_write();
        })
    };
    let mut chunk = vec![0; CHUNK];
    let mut local_writer = local;
    loop {
        match (&*far).read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(length) => {
                if local_writer.write_all(&chunk[..length]).is_err() {
                    break;
                }
            }
        }
    }
    let _ = local_writer.shutdown(Shutdown::Write);
    if let Ok(outward) = outward {
        let _ = outward.join();
    }
}

/// A local address for a new door, where `port` is free. macOS has only
/// `127.0.0.1` unless told otherwise, so that one comes last.
fn free_address(port: Option<u16>) -> io::Result<Ipv4Addr> {
    loop {
        let number = NEXT_DOOR.fetch_add(1, Ordering::SeqCst);
        if number == 0 || number > 250 {
            break;
        }
        let address = Ipv4Addr::new(127, 77, 0, number);
        let Some(port) = port else { return Ok(address) };
        match TcpListener::bind((address, port)) {
            // Free: the door binds it again in a moment.
            Ok(_) => return Ok(address),
            Err(error) if error.kind() == io::ErrorKind::AddrNotAvailable => break,
            Err(_) => {}
        }
    }
    Ok(Ipv4Addr::LOCALHOST)
}

fn spawn(name: &str, work: impl FnOnce() + Send + 'static) -> io::Result<()> {
    thread::Builder::new().name(name.to_owned()).spawn(work).map(|_| ())
}
