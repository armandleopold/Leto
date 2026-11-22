//! What a host keeps from one launch to the next: who it is, and the
//! devices paired with it. A PC that is shared must not ask every device for
//! a PIN again each time it restarts.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU32, Ordering},
    thread,
    time::{Duration, Instant},
};

use leto_client::{Client, HostSummary};
use leto_host::server::{Host, Options};

/// A fresh folder per test, gone with it.
struct Folder(PathBuf);

impl Folder {
    fn new(what: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name = format!(
            "leto-kept-{what}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        );
        let path = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&path);
        Self(path)
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A host that keeps what it knows in `folder`, as a shared PC does: a
/// device is forgotten only when it asks for itself.
fn options(folder: &Folder, port: u16) -> Options {
    Options {
        store: Some(folder.0.clone()),
        port,
        forgets_for_anyone: false,
        camera: None,
        ..Options::default()
    }
}

/// The host again, on the port it had. The port may take a moment to be
/// free again.
fn restart(folder: &Folder, port: u16) -> Host {
    let start = Instant::now();
    loop {
        match Host::start(options(folder, port)) {
            Ok(host) => return host,
            Err(error) if start.elapsed() < Duration::from_secs(20) => {
                eprintln!("the host's port is taken for now: {error}");
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => panic!("the host did not start again: {error}"),
        }
    }
}

fn pair(client: &Client, host: &Host) -> HostSummary {
    let added = client.add(&host.address()).unwrap();
    let pin = client.new_pin().unwrap();
    host.type_pin(pin);
    client.pair(&added.id, pin).unwrap()
}

#[test]
fn a_host_that_restarts_is_the_same_host_with_the_same_devices() {
    let (kept, settings) = (Folder::new("host"), Folder::new("client"));
    let client = Client::open(&settings.0).unwrap();

    let host = Host::start(options(&kept, 0)).unwrap();
    let (port, id) = (host.port(), host.id().to_owned());
    let paired = pair(&client, &host);
    assert!(paired.paired);
    assert_eq!(client.apps(&paired.id).unwrap().len(), 2);
    drop(host);

    let host = restart(&kept, port);

    assert_eq!(host.id(), id);
    assert_eq!(host.paired_clients(), 1);
    // The client checks the certificate it pinned at pairing: a host with
    // another key would be refused here.
    assert_eq!(client.apps(&paired.id).unwrap().len(), 2);
    assert!(client.refresh().iter().any(|host| host.id == paired.id && host.paired));
}

#[test]
fn a_device_that_was_forgotten_stays_forgotten_after_a_restart() {
    let (kept, settings) = (Folder::new("forgotten"), Folder::new("forgetful"));
    let client = Client::open(&settings.0).unwrap();

    let host = Host::start(options(&kept, 0)).unwrap();
    let port = host.port();
    let paired = pair(&client, &host);
    assert_eq!(host.paired_clients(), 1);

    // The device asks the host to forget it, over the connection that shows
    // who it is.
    client.forget(&paired.id).unwrap();
    assert_eq!(host.paired_clients(), 0);
    drop(host);

    assert_eq!(restart(&kept, port).paired_clients(), 0);
}

#[test]
fn a_stranger_cannot_make_a_shared_pc_forget_its_devices() {
    let (kept, settings, other) =
        (Folder::new("strangers"), Folder::new("known"), Folder::new("stranger"));
    let client = Client::open(&settings.0).unwrap();
    let stranger = Client::open(&other.0).unwrap();

    let host = Host::start(options(&kept, 0)).unwrap();
    let paired = pair(&client, &host);

    // A pairing that fails ends with a plain request to forget, which anyone
    // on the network can send.
    let added = stranger.add(&host.address()).unwrap();
    let pin = stranger.new_pin().unwrap();
    let wrong = loop {
        let wrong = stranger.new_pin().unwrap();
        if wrong != pin {
            break wrong;
        }
    };
    host.type_pin(wrong);
    assert!(stranger.pair(&added.id, pin).is_err());

    assert_eq!(host.paired_clients(), 1);
    assert_eq!(client.apps(&paired.id).unwrap().len(), 2);
}

#[test]
fn a_host_without_a_folder_is_new_each_time() {
    let first = Host::start(Options { camera: None, ..Options::default() }).unwrap();
    let settings = Folder::new("new");
    let client = Client::open(&settings.0).unwrap();
    pair(&client, &first);
    assert_eq!(first.paired_clients(), 1);
    drop(first);

    let second = Host::start(Options { camera: None, ..Options::default() }).unwrap();
    assert_eq!(second.paired_clients(), 0);
}
