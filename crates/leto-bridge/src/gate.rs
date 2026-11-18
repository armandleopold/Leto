//! Who may use the bridge.
//!
//! Whoever reaches the bridge reaches the PCs it is paired with, keyboard
//! and mouse included. So a browser has to be known to it. The first time,
//! the person types into the browser the code the bridge shows where it
//! runs, and the browser gets a key to keep. From then on the key is enough.
//!
//! The code only proves that someone can read the bridge's terminal, so it
//! is short. That makes guessing the thing to prevent: a wrong code costs
//! everyone a pause, and a few wrong codes replace the code by a new one.

use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use leto_client::store::write_private;
use leto_core::bridge::CODE_DIGITS;
use ring::{
    digest,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};

/// The file that remembers which browsers are known.
const FILE_NAME: &str = "browsers.json";

/// How long nobody can try a code after a wrong one.
const PAUSE: Duration = Duration::from_secs(2);

/// Wrong codes before the code is replaced by a new one.
const TRIES_PER_CODE: u32 = 5;

/// A key is this many random bytes, written as twice as many hex digits.
const KEY_BYTES: usize = 32;

/// What is kept about a browser: never its key, only what proves it has it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct Browser {
    /// SHA-256 of the key, in hex.
    key_hash: String,
    /// When the code was typed, in seconds since 1970.
    added: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    browsers: Vec<Browser>,
}

/// Called with each new code, for whoever shows it to the person.
pub type Announce = Box<dyn Fn(&str) + Send + Sync>;

pub struct Gate {
    file: PathBuf,
    random: SystemRandom,
    announce: Announce,
    state: Mutex<State>,
}

struct State {
    code: String,
    /// Wrong codes tried since this code was drawn.
    wrong: u32,
    /// Nobody may try a code before then.
    paused_until: Option<Instant>,
    browsers: BTreeSet<Browser>,
}

impl Gate {
    /// Opens the gate with what it remembers in `directory`, and draws the
    /// first code. `announce` is not called for that one: read it with
    /// [`Gate::code`].
    pub fn open(directory: &Path, announce: Announce) -> io::Result<Self> {
        let file = directory.join(FILE_NAME);
        let saved: Saved = match fs::read(&file) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Saved::default(),
            Err(error) => return Err(error),
        };
        let random = SystemRandom::new();
        let code = new_code(&random)?;
        let state = State {
            code,
            wrong: 0,
            paused_until: None,
            browsers: saved.browsers.into_iter().collect(),
        };
        Ok(Self { file, random, announce, state: Mutex::new(state) })
    }

    /// The code to show the person, digits only.
    pub fn code(&self) -> String {
        self.lock().code.clone()
    }

    /// How many browsers are known.
    pub fn browsers(&self) -> usize {
        self.lock().browsers.len()
    }

    /// Whether `key` is one this gate gave out.
    pub fn knows(&self, key: &str) -> bool {
        let hash = hash(key);
        self.lock().browsers.iter().any(|browser| browser.key_hash == hash)
    }

    /// Tries the code a person typed. The right one gives the browser its
    /// key. Spaces between the digits do not count.
    pub fn try_code(&self, typed: &str) -> Option<String> {
        self.try_code_at(typed, Instant::now())
    }

    fn try_code_at(&self, typed: &str, now: Instant) -> Option<String> {
        let typed: String = typed.chars().filter(|letter| !letter.is_whitespace()).collect();
        let mut state = self.lock();
        if state.paused_until.is_some_and(|until| now < until) {
            // Not even looked at: guessing faster gains nothing.
            return None;
        }
        if !same(typed.as_bytes(), state.code.as_bytes()) {
            state.paused_until = Some(now + PAUSE);
            state.wrong += 1;
            if state.wrong >= TRIES_PER_CODE {
                match new_code(&self.random) {
                    Ok(code) => {
                        state.code = code;
                        state.wrong = 0;
                        let code = state.code.clone();
                        drop(state);
                        (self.announce)(&code);
                    }
                    Err(error) => log::error!("could not draw a new code: {error}"),
                }
            }
            return None;
        }

        let key = match new_key(&self.random) {
            Ok(key) => key,
            Err(error) => {
                log::error!("could not make a key for a browser: {error}");
                return None;
            }
        };
        let added = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
        state.browsers.insert(Browser { key_hash: hash(&key), added });
        if let Err(error) = self.save(&state) {
            // The browser is known until the bridge stops, then asked again.
            log::warn!("could not remember a browser in {}: {error}", self.file.display());
        }
        // A code lets one browser in. The next one gets a code of its own.
        match new_code(&self.random) {
            Ok(code) => {
                state.code = code;
                state.wrong = 0;
                let code = state.code.clone();
                drop(state);
                (self.announce)(&code);
            }
            Err(error) => log::error!("could not draw a new code: {error}"),
        }
        Some(key)
    }

    /// Forgets every browser: each one needs the code again.
    pub fn forget_browsers(&self) -> io::Result<()> {
        let mut state = self.lock();
        state.browsers.clear();
        self.save(&state)
    }

    fn save(&self, state: &State) -> io::Result<()> {
        if let Some(directory) = self.file.parent() {
            fs::create_dir_all(directory)?;
        }
        let saved = Saved { browsers: state.browsers.iter().cloned().collect() };
        let json = serde_json::to_vec_pretty(&saved)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let temporary = self.file.with_extension("json.tmp");
        write_private(&temporary, &json)?;
        fs::rename(&temporary, &self.file)
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // The gate stays shut the same way even if a thread panicked.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The code the way a person reads it best: in two groups.
pub fn spaced(code: &str) -> String {
    let (first, second) = code.split_at(code.len() / 2);
    format!("{first} {second}")
}

fn new_code(random: &SystemRandom) -> io::Result<String> {
    let mut code = String::with_capacity(CODE_DIGITS);
    while code.len() < CODE_DIGITS {
        let mut bytes = [0_u8; 16];
        random
            .fill(&mut bytes)
            .map_err(|_| io::Error::other("the system gave no random numbers"))?;
        // Bytes from 250 up would make the digits 0 to 5 more likely.
        let digits =
            bytes.iter().filter(|byte| **byte < 250).map(|byte| char::from(b'0' + byte % 10));
        code.extend(digits.take(CODE_DIGITS - code.len()));
    }
    Ok(code)
}

fn new_key(random: &SystemRandom) -> io::Result<String> {
    let mut bytes = [0_u8; KEY_BYTES];
    random.fill(&mut bytes).map_err(|_| io::Error::other("the system gave no random numbers"))?;
    Ok(hex(&bytes))
}

fn hash(key: &str) -> String {
    hex(digest::digest(&digest::SHA256, key.as_bytes()).as_ref())
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Compares without stopping at the first difference, so that the time it
/// takes says nothing about how much of a guess was right.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0, |differences, (a, b)| differences | (a ^ b)) == 0
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    };

    use super::*;

    struct Folder(PathBuf);

    impl Folder {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let name = format!(
                "leto-gate-test-{}-{}",
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

    fn quiet() -> Announce {
        Box::new(|_| {})
    }

    /// A code that is certainly not `code`.
    fn wrong(code: &str) -> String {
        let first = if code.starts_with('0') { '1' } else { '0' };
        format!("{first}{}", &code[1..])
    }

    #[test]
    fn a_code_is_eight_digits_and_reads_in_two_groups() {
        let folder = Folder::new();
        let gate = Gate::open(&folder.0, quiet()).unwrap();
        let code = gate.code();
        assert_eq!(code.len(), 8);
        assert!(code.bytes().all(|byte| byte.is_ascii_digit()));
        assert_eq!(spaced("12345678"), "1234 5678");
    }

    #[test]
    fn the_right_code_gives_a_key_that_is_known_from_then_on() {
        let folder = Folder::new();
        let gate = Gate::open(&folder.0, quiet()).unwrap();
        assert!(!gate.knows("anything"));

        // Typed the way it is shown, in two groups.
        let key = gate.try_code(&spaced(&gate.code())).unwrap();
        assert_eq!(key.len(), 64);
        assert!(gate.knows(&key));
        assert!(!gate.knows(&key[1..]));
        assert_eq!(gate.browsers(), 1);

        // Still known after the bridge restarts.
        let reopened = Gate::open(&folder.0, quiet()).unwrap();
        assert!(reopened.knows(&key));
        // The file keeps what proves the key, never the key.
        let file = fs::read_to_string(folder.0.join(FILE_NAME)).unwrap();
        assert!(!file.contains(&key));
    }

    #[test]
    fn a_code_lets_one_browser_in() {
        let folder = Folder::new();
        let announced = Arc::new(Mutex::new(Vec::new()));
        let heard = Arc::clone(&announced);
        let gate = Gate::open(
            &folder.0,
            Box::new(move |code| heard.lock().unwrap().push(code.to_owned())),
        )
        .unwrap();
        let first = gate.code();
        assert!(gate.try_code(&first).is_some());

        let second = gate.code();
        assert_ne!(first, second);
        assert_eq!(*announced.lock().unwrap(), std::slice::from_ref(&second));
        // Whoever saw the first code typed cannot use it again.
        assert!(gate.try_code(&first).is_none());
    }

    #[test]
    fn a_wrong_code_costs_a_pause_even_to_the_right_one() {
        let folder = Folder::new();
        let gate = Gate::open(&folder.0, quiet()).unwrap();
        let code = gate.code();
        let start = Instant::now();

        assert!(gate.try_code_at(&wrong(&code), start).is_none());
        assert!(gate.try_code_at(&code, start + PAUSE / 2).is_none());
        assert!(gate.try_code_at(&code, start + PAUSE).is_some());
    }

    #[test]
    fn a_few_wrong_codes_replace_the_code() {
        let folder = Folder::new();
        let announced = Arc::new(Mutex::new(Vec::new()));
        let heard = Arc::clone(&announced);
        let gate = Gate::open(
            &folder.0,
            Box::new(move |code| heard.lock().unwrap().push(code.to_owned())),
        )
        .unwrap();
        let code = gate.code();
        let mut now = Instant::now();

        for _ in 0..TRIES_PER_CODE - 1 {
            assert!(gate.try_code_at(&wrong(&code), now).is_none());
            now += PAUSE;
        }
        assert_eq!(gate.code(), code);
        assert!(announced.lock().unwrap().is_empty());

        assert!(gate.try_code_at(&wrong(&code), now).is_none());
        now += PAUSE;
        let replaced = gate.code();
        assert_ne!(replaced, code);
        assert_eq!(*announced.lock().unwrap(), std::slice::from_ref(&replaced));
        // The old code is worth nothing, the new one works.
        assert!(gate.try_code_at(&code, now).is_none());
        now += PAUSE;
        assert!(gate.try_code_at(&replaced, now).is_some());
    }

    #[test]
    fn forgotten_browsers_need_the_code_again() {
        let folder = Folder::new();
        let gate = Gate::open(&folder.0, quiet()).unwrap();
        let key = gate.try_code(&gate.code()).unwrap();
        gate.forget_browsers().unwrap();
        assert!(!gate.knows(&key));
        assert!(!Gate::open(&folder.0, quiet()).unwrap().knows(&key));
    }

    #[test]
    fn a_damaged_file_is_an_error_not_an_open_gate() {
        let folder = Folder::new();
        fs::create_dir_all(&folder.0).unwrap();
        fs::write(folder.0.join(FILE_NAME), "{ not json").unwrap();
        assert!(Gate::open(&folder.0, quiet()).is_err());
    }
}
