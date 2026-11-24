//! One Leto at a time. The app started while another runs asks that one to
//! show its window, and ends without opening one of its own: the person
//! who opened Leto again only wanted to see it, even put away beside the
//! clock.
//!
//! The first one holds a lock on a file in a folder, the settings folder of
//! the person, which the system takes back when the program ends however it
//! ends: a crash leaves nothing behind that would keep the next one from
//! starting. It listens on this computer's own address, and writes beside
//! the lock where. The one that finds the lock taken connects there and
//! asks.
//!
//! Anything on this computer may connect and ask for the window too. That
//! is all it may ask, and it is no more than a click on Leto's icon does.

use std::{
    fs::{self, File, TryLockError},
    io::{self, BufRead, BufReader, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::Path,
    thread,
    time::{Duration, Instant},
};

/// The file whose lock says a Leto runs.
const LOCK: &str = "leto.lock";
/// The file beside it that says where the one that runs listens.
const WHERE: &str = "leto.instance";
/// What the one started again asks, and what the one that runs answers.
const ASK: &str = "show";
const ANSWER: &str = "shown";
/// How long the one started again tries: the one that holds the lock may
/// have started a moment before it, and not be listening yet.
const PATIENCE: Duration = Duration::from_secs(3);
/// How long it waits for the window to be shown before it ends anyway.
const ANSWER_PATIENCE: Duration = Duration::from_secs(2);

/// What [`claim`] found.
#[derive(Debug)]
pub enum Claim {
    /// No other Leto runs: this one is the one, while this lives.
    First(First),
    /// Another Leto runs, and has been asked to show its window.
    Other,
}

/// The Leto that runs. Others started meanwhile are heard once
/// [`First::listen`] is called, and until it is, they wait.
#[derive(Debug)]
pub struct First {
    // Held, never read: its lock ends with it.
    _lock: File,
    listener: TcpListener,
}

impl First {
    /// Calls `show` on a thread of its own each time another Leto is
    /// started, for as long as this program runs.
    pub fn listen(self, show: impl Fn() + Send + 'static) {
        let Self { _lock: lock, listener } = self;
        let spawned = thread::Builder::new().name("leto-instance".into()).spawn(move || {
            let _lock = lock;
            for asker in listener.incoming() {
                let Ok(asker) = asker else { continue };
                if heard(&asker) {
                    show();
                    let _ = (&asker).write_all(format!("{ANSWER}\n").as_bytes());
                }
            }
        });
        if let Err(problem) = spawned {
            log::warn!("Leto started again will not be heard: {problem}");
        }
    }
}

/// Whether `asker` asked for the window.
fn heard(asker: &TcpStream) -> bool {
    let _ = asker.set_read_timeout(Some(ANSWER_PATIENCE));
    let mut line = String::new();
    read_line(asker, &mut line) && line.trim() == ASK
}

/// Reads a line of a few bytes at most into `line`: what connects may say
/// anything, or nothing. Returns whether it could.
fn read_line(from: &TcpStream, line: &mut String) -> bool {
    BufReader::new(io::Read::take(from, 64)).read_line(line).is_ok()
}

/// Becomes the one Leto that runs with `folder` as its settings, or asks
/// the one that already does to show its window.
///
/// # Errors
///
/// When the folder cannot be written in, or this computer refuses to
/// listen on its own address, or another Leto holds the lock but cannot be
/// reached. A Leto that cannot tell should start anyway: two of them are
/// better than none.
pub fn claim(folder: &Path) -> io::Result<Claim> {
    fs::create_dir_all(folder)?;
    let lock = File::options().create(true).truncate(false).write(true).open(folder.join(LOCK))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            ask(&folder.join(WHERE))?;
            return Ok(Claim::Other);
        }
        Err(TryLockError::Error(problem)) => return Err(problem),
    }
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = listener.local_addr()?.port();
    // Written in whole, then put in place: the one started again never
    // reads half of it.
    let written = folder.join(format!("{WHERE}.{}", std::process::id()));
    fs::write(&written, format!("{port} {}\n", std::process::id()))?;
    fs::rename(&written, folder.join(WHERE))?;
    Ok(Claim::First(First { _lock: lock, listener }))
}

/// Asks the Leto that says where it listens in `place` to show its window.
fn ask(place: &Path) -> io::Result<()> {
    let until = Instant::now() + PATIENCE;
    loop {
        match ask_once(place) {
            Ok(()) => return Ok(()),
            Err(problem) if Instant::now() >= until => return Err(problem),
            Err(_) => thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn ask_once(place: &Path) -> io::Result<()> {
    let text = fs::read_to_string(place)?;
    let mut words = text.split_whitespace();
    let unreadable = || io::Error::new(io::ErrorKind::InvalidData, "unreadable instance file");
    let port: u16 = words.next().and_then(|word| word.parse().ok()).ok_or_else(unreadable)?;
    let process: u32 = words.next().and_then(|word| word.parse().ok()).ok_or_else(unreadable)?;
    let mut asked = TcpStream::connect_timeout(
        &SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        Duration::from_millis(500),
    )?;
    // The person just started this one: it may bring a window to the
    // front, and lets the one that runs bring its own.
    let_to_front(process);
    asked.write_all(format!("{ASK}\n").as_bytes())?;
    // The window is shown once the answer comes. Without one, it most
    // likely was anyway: this one ends all the same.
    asked.set_read_timeout(Some(ANSWER_PATIENCE))?;
    let mut answer = String::new();
    let _ = read_line(&asked, &mut answer);
    Ok(())
}

/// Lets the program `process` bring its window in front of the others,
/// which Windows keeps for the one the person is using. Elsewhere the
/// system decides.
fn let_to_front(process: u32) {
    #[cfg(windows)]
    // SAFETY: takes a number and no pointer; a wrong one only fails.
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(process);
    }
    #[cfg(not(windows))]
    let _ = process;
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use std::path::PathBuf;

    use super::*;

    fn folder(name: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("leto-instance-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        folder
    }

    #[test]
    fn the_second_leto_shows_the_first_and_ends() {
        let folder = folder("second");
        let Claim::First(first) = claim(&folder).expect("the folder is ours") else {
            panic!("no other Leto runs with this folder");
        };
        let shown = Arc::new(AtomicUsize::new(0));
        first.listen({
            let shown = shown.clone();
            move || {
                shown.fetch_add(1, Ordering::SeqCst);
            }
        });

        for times in 1..=2 {
            let claim = claim(&folder).expect("the first one answers");
            assert!(matches!(claim, Claim::Other), "{claim:?}");
            assert_eq!(shown.load(Ordering::SeqCst), times);
        }
    }

    #[test]
    fn the_lock_goes_with_the_first_leto() {
        let folder = folder("gone");
        let first = claim(&folder).expect("the folder is ours");
        assert!(matches!(first, Claim::First(_)));
        drop(first);
        // As when the first one ended, or crashed: the next one is the one.
        assert!(matches!(claim(&folder).expect("the lock is free"), Claim::First(_)));
    }

    #[test]
    fn a_second_leto_waits_for_the_first_to_listen() {
        let folder = folder("early");
        let Claim::First(first) = claim(&folder).expect("the folder is ours") else {
            panic!("no other Leto runs with this folder");
        };
        let shown = Arc::new(AtomicUsize::new(0));
        let listening = thread::spawn({
            let shown = shown.clone();
            move || {
                // The first one is still opening its window.
                thread::sleep(Duration::from_millis(500));
                first.listen(move || {
                    shown.fetch_add(1, Ordering::SeqCst);
                });
            }
        });
        assert!(matches!(claim(&folder).expect("the first one answers"), Claim::Other));
        assert_eq!(shown.load(Ordering::SeqCst), 1);
        listening.join().expect("the thread does not panic");
    }

    #[test]
    fn nonsense_asks_for_nothing() {
        let folder = folder("nonsense");
        let Claim::First(first) = claim(&folder).expect("the folder is ours") else {
            panic!("no other Leto runs with this folder");
        };
        let shown = Arc::new(AtomicUsize::new(0));
        first.listen({
            let shown = shown.clone();
            move || {
                shown.fetch_add(1, Ordering::SeqCst);
            }
        });
        let text = fs::read_to_string(folder.join(WHERE)).expect("the first one wrote it");
        let port: u16 =
            text.split_whitespace().next().and_then(|w| w.parse().ok()).expect("a port");
        let mut other = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("it listens");
        other.write_all(b"GET / HTTP/1.1\r\n\r\n").expect("it reads");
        drop(other);
        assert!(matches!(claim(&folder).expect("it still answers"), Claim::Other));
        assert_eq!(shown.load(Ordering::SeqCst), 1);
    }
}
