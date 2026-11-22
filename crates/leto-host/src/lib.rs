//! The host side of Leto: what runs on the PC that is shared.
//!
//! It starts small, as the companion of `docs/return-channel.md`: beside an
//! Apollo or a Sunshine that streams the screen as before, it takes what
//! neither of them takes. Today that is the camera of a client; its
//! microphone is next. Screen, sound and input come after, until nothing is
//! left for Apollo to do: see `docs/roadmap.md`.
//!
//! - [`apollo`] reads what the Apollo installed on the PC knows, so that the
//!   companion shows the same certificate and takes the same devices.
//! - [`camera`] answers Leto's API and takes the pictures of a camera.
//! - [`Companion`] puts the two together.

pub mod apollo;
pub mod camera;

use std::{
    io,
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use apollo::Apollo;
use camera::{Access, Cameras};

/// How many devices are answered at once. A request takes a moment, and
/// only paired devices have any business here: more than this at a time is
/// not them.
const AT_ONCE: usize = 16;

/// How a [`Companion`] is set up.
#[derive(Debug, Clone)]
pub struct Options {
    /// The address of this computer the companion listens on: every one, or
    /// this computer alone for a test.
    pub interface: String,
    /// The largest picture of a camera the companion takes.
    pub largest: (u32, u32),
}

impl Default for Options {
    fn default() -> Self {
        Self { interface: Ipv4Addr::UNSPECIFIED.to_string(), largest: (1920, 1080) }
    }
}

/// Leto beside an Apollo: takes the cameras of the devices Apollo is paired
/// with, for as long as it lives.
pub struct Companion {
    apollo: Arc<Apollo>,
    cameras: Arc<Cameras>,
    stop: Arc<AtomicBool>,
    threads: Vec<thread::JoinHandle<()>>,
}

impl Companion {
    /// Opens Leto's API and the camera's port, thirteen above Apollo's own.
    /// Fails when either is taken, by another companion for one.
    pub fn start(apollo: Apollo, options: &Options) -> io::Result<Self> {
        let (cameras, listener) =
            Cameras::open(&options.interface, apollo.port(), options.largest)?;
        let apollo = Arc::new(apollo);
        let stop = Arc::new(AtomicBool::new(false));

        let receiving = thread::Builder::new().name("leto-host camera".to_owned()).spawn({
            let (cameras, stop) = (Arc::clone(&cameras), Arc::clone(&stop));
            move || cameras.receive(&stop)
        })?;
        let answering = thread::Builder::new().name("leto-host api".to_owned()).spawn({
            let (apollo, cameras, stop) =
                (Arc::clone(&apollo), Arc::clone(&cameras), Arc::clone(&stop));
            move || answer(&listener, &apollo, &cameras, &stop)
        })?;
        Ok(Self { apollo, cameras, stop, threads: vec![receiving, answering] })
    }

    /// The port of Leto's API and of the camera.
    pub fn port(&self) -> u16 {
        self.cameras.port()
    }

    /// What arrives of the cameras.
    pub fn cameras(&self) -> &Cameras {
        &self.cameras
    }

    /// The same, to keep and to watch from another thread.
    pub fn watch(&self) -> Arc<Cameras> {
        Arc::clone(&self.cameras)
    }

    /// The Apollo this companion stands beside.
    pub fn apollo(&self) -> &Apollo {
        &self.apollo
    }

    /// The name Apollo's owner gave the device whose camera is being sent.
    pub fn sender(&self) -> Option<String> {
        let certificate = self.cameras.sender()?;
        let devices = self.apollo.devices().ok()?;
        devices.into_iter().find(|device| device.certificate == certificate).map(|d| d.name)
    }
}

impl Drop for Companion {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Both threads wait for as long as it takes: each is sent something
        // of the companion's own to see that it is over.
        self.cameras.wake();
        let here = SocketAddr::from((Ipv4Addr::LOCALHOST, self.cameras.port()));
        let _ = TcpStream::connect_timeout(&here, Duration::from_secs(1));
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// Answers the devices that connect to Leto's API until `stop` is set.
fn answer(listener: &TcpListener, apollo: &Arc<Apollo>, cameras: &Arc<Cameras>, stop: &AtomicBool) {
    let busy = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let Ok(stream) = stream else {
            // Nothing a device does causes this. Not in a tight loop, in
            // case it lasts.
            thread::sleep(Duration::from_millis(50));
            continue;
        };
        if busy.fetch_add(1, Ordering::SeqCst) >= AT_ONCE {
            busy.fetch_sub(1, Ordering::SeqCst);
            continue;
        }
        let (apollo, cameras, busy) = (Arc::clone(apollo), Arc::clone(cameras), Arc::clone(&busy));
        let spawned = thread::Builder::new().name("leto-host device".to_owned()).spawn({
            let busy = Arc::clone(&busy);
            move || {
                camera::handle(stream, apollo.tls(), |device| access(&apollo, device), &cameras);
                busy.fetch_sub(1, Ordering::SeqCst);
            }
        });
        if spawned.is_err() {
            busy.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// What Apollo knows of the device that showed this certificate, as its
/// file says at this moment.
fn access(apollo: &Apollo, certificate: &[u8]) -> Access {
    let devices = match apollo.devices() {
        Ok(devices) => devices,
        Err(error) => {
            log::warn!("Apollo's devices could not be read, so nobody is let in: {error}");
            return Access::Stranger;
        }
    };
    match devices.iter().find(|device| device.certificate == certificate) {
        Some(device) => Access::Paired { camera: device.trusted() },
        None => Access::Stranger,
    }
}
