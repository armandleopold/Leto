//! TCP inside the tunnel. smoltcp keeps the connections; its packets come
//! from and go to the tunnel through two queues.

use std::{
    collections::VecDeque,
    io,
    net::{Ipv4Addr, SocketAddrV4},
    time::{Duration, Instant},
};

use smoltcp::{
    iface::{Config, Interface, SocketHandle, SocketSet},
    phy::{self, DeviceCapabilities, Medium},
    socket::tcp,
    time::Instant as StackInstant,
    wire::{HardwareAddress, IpAddress, IpCidr, IpEndpoint},
};

/// Room for what is on its way in each direction of one connection.
const BUFFER: usize = 256 * 1024;
/// How long a connection that was let go may take to close politely.
const CLOSING: Duration = Duration::from_secs(10);
/// Ports this end gives its own connections.
const PORTS: std::ops::RangeInclusive<u16> = 49152..=65535;

/// The packets between smoltcp and the tunnel.
struct Queues {
    incoming: VecDeque<Vec<u8>>,
    outgoing: VecDeque<Vec<u8>>,
    mtu: usize,
}

impl phy::Device for Queues {
    type RxToken<'a> = Incoming;
    type TxToken<'a> = Outgoing<'a>;

    fn receive(&mut self, _now: StackInstant) -> Option<(Incoming, Outgoing<'_>)> {
        let packet = self.incoming.pop_front()?;
        Some((Incoming(packet), Outgoing(&mut self.outgoing)))
    }

    fn transmit(&mut self, _now: StackInstant) -> Option<Outgoing<'_>> {
        Some(Outgoing(&mut self.outgoing))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut capabilities = DeviceCapabilities::default();
        capabilities.medium = Medium::Ip;
        capabilities.max_transmission_unit = self.mtu;
        capabilities
    }
}

struct Incoming(Vec<u8>);

impl phy::RxToken for Incoming {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.0)
    }
}

struct Outgoing<'a>(&'a mut VecDeque<Vec<u8>>);

impl phy::TxToken for Outgoing<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut packet = vec![0; len];
        let result = f(&mut packet);
        self.0.push_back(packet);
        result
    }
}

/// The TCP connections of one end of the tunnel.
pub(crate) struct Stack {
    interface: Interface,
    queues: Queues,
    pub(crate) sockets: SocketSet<'static>,
    started: Instant,
    next_port: u16,
    /// Connections let go of, closing.
    closing: Vec<(SocketHandle, Instant)>,
}

impl Stack {
    pub(crate) fn new(address: Ipv4Addr, mtu: usize) -> Self {
        let mut queues = Queues { incoming: VecDeque::new(), outgoing: VecDeque::new(), mtu };
        let mut config = Config::new(HardwareAddress::Ip);
        config.random_seed = seed();
        let started = Instant::now();
        let mut interface = Interface::new(config, &mut queues, StackInstant::from_millis(0));
        interface.update_ip_addrs(|addresses| {
            // One address: the list has room for it.
            let _ = addresses.push(IpCidr::new(IpAddress::Ipv4(address), 32));
        });
        // Everything goes into the tunnel, which knows only one way.
        let _ = interface.routes_mut().add_default_ipv4_route(address);
        let span = u64::from(PORTS.end() - PORTS.start());
        let first = PORTS.start() + u16::try_from(seed() % span).unwrap_or(0);
        Self {
            interface,
            queues,
            sockets: SocketSet::new(Vec::new()),
            started,
            next_port: first,
            closing: Vec::new(),
        }
    }

    fn now(&self) -> StackInstant {
        let elapsed = i64::try_from(self.started.elapsed().as_millis()).unwrap_or(i64::MAX);
        StackInstant::from_millis(elapsed)
    }

    /// A packet from the tunnel.
    pub(crate) fn take(&mut self, packet: Vec<u8>) {
        self.queues.incoming.push_back(packet);
    }

    /// Lets smoltcp do its work. Returns the packets for the tunnel, and how
    /// soon it wants to be asked again.
    pub(crate) fn poll(&mut self) -> (Vec<Vec<u8>>, Option<Duration>) {
        let now = self.now();
        self.interface.poll(now, &mut self.queues, &mut self.sockets);
        self.reap();
        let outgoing = self.queues.outgoing.drain(..).collect();
        let delay = self
            .interface
            .poll_delay(now, &self.sockets)
            .map(|delay| Duration::from_micros(delay.total_micros()));
        (outgoing, delay)
    }

    /// Opens a connection to `to`. It is established once a later poll says.
    pub(crate) fn connect(&mut self, to: SocketAddrV4) -> io::Result<SocketHandle> {
        let mut socket = new_socket();
        let port = self.free_port();
        socket
            .connect(
                self.interface.context(),
                IpEndpoint::new(IpAddress::Ipv4(*to.ip()), to.port()),
                port,
            )
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
        Ok(self.sockets.add(socket))
    }

    /// Waits for one connection on `port`.
    pub(crate) fn listen(&mut self, port: u16) -> io::Result<SocketHandle> {
        let mut socket = new_socket();
        socket
            .listen(port)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
        Ok(self.sockets.add(socket))
    }

    pub(crate) fn socket(&mut self, handle: SocketHandle) -> &mut tcp::Socket<'static> {
        self.sockets.get_mut::<tcp::Socket<'static>>(handle)
    }

    /// Lets go of a connection: it closes politely, then is forgotten.
    pub(crate) fn release(&mut self, handle: SocketHandle) {
        self.socket(handle).close();
        self.closing.push((handle, Instant::now()));
    }

    /// Forgets at once a connection that never was.
    pub(crate) fn drop_now(&mut self, handle: SocketHandle) {
        self.socket(handle).abort();
        self.closing.push((handle, Instant::now() - CLOSING));
    }

    /// Forgets the connections that finished closing, or took too long.
    fn reap(&mut self) {
        let mut index = 0;
        while index < self.closing.len() {
            let (handle, since) = self.closing[index];
            let state = self.socket(handle).state();
            if matches!(state, tcp::State::Closed | tcp::State::TimeWait) {
                self.sockets.remove(handle);
                self.closing.swap_remove(index);
            } else {
                if since.elapsed() >= CLOSING {
                    // Removed on the next round, once the reset has gone.
                    self.socket(handle).abort();
                }
                index += 1;
            }
        }
    }

    fn free_port(&mut self) -> u16 {
        let port = self.next_port;
        self.next_port = if port == *PORTS.end() { *PORTS.start() } else { port + 1 };
        port
    }
}

fn new_socket() -> tcp::Socket<'static> {
    let mut socket = tcp::Socket::new(
        tcp::SocketBuffer::new(vec![0; BUFFER]),
        tcp::SocketBuffer::new(vec![0; BUFFER]),
    );
    // Requests and answers are sent whole: no waiting to gather more.
    socket.set_nagle_enabled(false);
    socket.set_congestion_control(tcp::CongestionControl::Reno);
    socket
}

/// Something different each time, for the first sequence numbers and ports.
fn seed() -> u64 {
    let since =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let nanos = u64::try_from(since.as_nanos() & u128::from(u64::MAX)).unwrap_or_default();
    nanos ^ u64::from(std::process::id()).rotate_left(32)
}
