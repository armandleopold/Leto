//! Leto's log: what happened, in a file the person can send to whoever helps.
//!
//! Each program writes its own file in the `logs` folder beside its
//! settings, so that two of them running at once do not mix: `leto.txt` for
//! the app, `leto-cli.txt` for the command line, `leto-bridge.txt` for the
//! bridge. Each run adds to the file, starting with a line that names the
//! program and its version. A file that reached its limit is set aside as
//! `leto.previous.txt` and so on, when the program starts again. Nothing in
//! a log may tell a secret, so that the folder can be sent as
//! it is: no PIN, no key, no bridge code, no query of a request. The settings
//! folder around it holds this device's key, and is never to be sent.

use std::{
    env,
    fs::{self, File},
    io::{self, Write},
    panic,
    path::{Path, PathBuf},
};

/// Most a log grows. What comes after is lost until the program starts
/// again, which is better than filling the disk of someone who left Leto
/// running for a month.
const LIMIT: u64 = 10 * 1024 * 1024;

/// What a log holds unless `RUST_LOG` says otherwise: everything Leto says,
/// and what the libraries under it find worth telling.
const DEFAULT_FILTER: &str = "info,leto=debug";

/// The folder of the logs beside the settings in `settings`.
pub fn folder(settings: &Path) -> PathBuf {
    settings.join("logs")
}

/// The log `program` writes beside the settings in `settings`.
pub fn file(settings: &Path, program: &str) -> PathBuf {
    folder(settings).join(format!("{program}.txt"))
}

/// Starts the log of `program` beside the settings in `settings`, and
/// returns the file it writes. A bug that stops the program is written there
/// too. Messages also go to the terminal when `RUST_LOG` is set.
///
/// Without a file, because the folder cannot be written, messages still go
/// to the terminal when asked. Only the first call in a program does
/// anything.
pub fn start(program: &str, settings: &Path) -> Option<PathBuf> {
    let path = file(settings, program);
    let opened = open(program, &path)
        .inspect_err(|error| eprintln!("Leto cannot write its log in {}: {error}", path.display()))
        .ok();
    let kept = opened.is_some();

    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(DEFAULT_FILTER));
    builder.format_timestamp_millis().write_style(env_logger::WriteStyle::Never).target(
        env_logger::Target::Pipe(Box::new(Tee {
            written: opened.as_ref().and_then(|file| file.metadata().ok()).map_or(0, |m| m.len()),
            file: opened,
            terminal: env::var_os("RUST_LOG").is_some(),
        })),
    );
    builder.try_init().ok()?;

    let before = panic::take_hook();
    let name = program.to_owned();
    panic::set_hook(Box::new(move |info| {
        log::error!("{name} stopped on a bug: {info}");
        before(info);
    }));

    log::info!(
        "{program} {} on {} {}",
        env!("CARGO_PKG_VERSION"),
        env::consts::OS,
        env::consts::ARCH
    );
    kept.then_some(path)
}

/// Opens the log at `path` to add to it, first setting it aside if full.
fn open(program: &str, path: &Path) -> io::Result<File> {
    let folder = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(folder)?;
    if fs::metadata(path).is_ok_and(|file| file.len() >= LIMIT) {
        // Whatever happens to the old one, the new one starts empty.
        let _ = fs::rename(path, folder.join(format!("{program}.previous.txt")));
        let _ = fs::remove_file(path);
    }
    File::options().create(true).append(true).open(path)
}

/// Writes each message to the file, and to the terminal when asked.
struct Tee {
    /// Gone once full, or once it could not be written.
    file: Option<File>,
    written: u64,
    terminal: bool,
}

impl Write for Tee {
    fn write(&mut self, message: &[u8]) -> io::Result<usize> {
        if self.terminal {
            let _ = io::stderr().write_all(message);
        }
        if let Some(file) = &mut self.file {
            let length = message.len() as u64;
            let kept = if self.written + length <= LIMIT {
                file.write_all(message).is_ok()
            } else {
                let _ = file.write_all(b"The log is full: what follows is not kept.\n");
                false
            };
            if kept {
                self.written += length;
            } else {
                self.file = None;
            }
        }
        // A log that cannot be written must not stop the program.
        Ok(message.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn folder_for(name: &str) -> PathBuf {
        let folder = env::temp_dir().join(format!("leto-logs-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        folder
    }

    #[test]
    fn each_run_adds_to_the_log_until_it_is_full() {
        let settings = folder_for("runs");
        let path = file(&settings, "leto");
        open("leto", &path).unwrap().write_all(b"first run\n").unwrap();
        open("leto", &path).unwrap().write_all(b"second run\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "first run\nsecond run\n");

        open("leto", &path).unwrap().set_len(LIMIT).unwrap();
        open("leto", &path).unwrap().write_all(b"after\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "after\n");
        let before = folder(&settings).join("leto.previous.txt");
        assert_eq!(fs::metadata(before).unwrap().len(), LIMIT);
        fs::remove_dir_all(settings).unwrap();
    }

    #[test]
    fn a_full_log_says_so_and_stops_growing() {
        let settings = folder_for("full");
        let path = file(&settings, "leto");
        let mut tee =
            Tee { file: Some(open("leto", &path).unwrap()), written: LIMIT - 4, terminal: false };
        tee.write_all(b"one\n").unwrap();
        tee.write_all(b"two\n").unwrap();
        tee.write_all(b"three\n").unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "one\nThe log is full: what follows is not kept.\n"
        );
        fs::remove_dir_all(settings).unwrap();
    }
}
