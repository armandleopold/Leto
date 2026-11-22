//! Leto on the PC that is shared.
//!
//! For now it is the companion: started beside an Apollo or a Sunshine, it
//! takes the camera of the devices they are paired with. See `leto-host`,
//! the crate, for what it does and `docs/return-channel.md` for why.

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
    thread,
    time::Duration,
};

use clap::{Parser, Subcommand};
use leto_host::{Companion, Options, apollo::Apollo, camera::Seen};

#[derive(Parser)]
#[command(name = "leto-host", version, about = "Leto on the PC that is shared.")]
struct Arguments {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Take the camera of your devices, beside the Apollo or the Sunshine
    /// of this PC, until Ctrl+C.
    Companion {
        /// The folder where Apollo keeps its settings, when it is not where
        /// Apollo usually installs itself.
        #[arg(long, value_name = "FOLDER")]
        apollo: Option<PathBuf>,
        /// Keep what the camera sees in this PNG file, written again every
        /// two seconds, to look at.
        #[arg(long, value_name = "FILE")]
        save: Option<PathBuf>,
        /// Answer this computer alone, not the network.
        #[arg(long)]
        local: bool,
    },
}

fn main() -> ExitCode {
    log::set_logger(&Printer).ok();
    log::set_max_level(log::LevelFilter::Info);
    match Arguments::parse().command {
        Command::Companion { apollo, save, local } => {
            companion(apollo.as_deref(), save.as_deref(), local)
        }
    }
}

fn companion(folder: Option<&Path>, save: Option<&Path>, local: bool) -> ExitCode {
    let apollo = match folder {
        Some(folder) => Apollo::read(folder),
        None => Apollo::find(),
    };
    let apollo = match apollo {
        Ok(apollo) => apollo,
        Err(error) => {
            eprintln!("leto-host: {error}.");
            eprintln!(
                "Apollo keeps its settings in its `config` folder. If it is elsewhere, name it \
                 with --apollo. If Windows refuses to let it be read, start this from a terminal \
                 opened as administrator."
            );
            return ExitCode::FAILURE;
        }
    };
    let mut options = Options::default();
    if local {
        options.interface = "127.0.0.1".to_owned();
    }
    let companion = match Companion::start(apollo, &options) {
        Ok(companion) => companion,
        Err(error) => {
            eprintln!("leto-host: the port for the camera could not be opened: {error}.");
            return ExitCode::FAILURE;
        }
    };

    println!(
        "Beside the Apollo of {}, on port {}.",
        companion.apollo().folder().display(),
        companion.port()
    );
    match companion.apollo().devices() {
        Ok(devices) => {
            let trusted: Vec<_> =
                devices.iter().filter(|device| device.trusted()).map(|d| d.name.as_str()).collect();
            println!(
                "Apollo is paired with {} device(s). {} may send a camera, the ones Apollo lets \
                 use the keyboard and the mouse: {}",
                devices.len(),
                trusted.len(),
                if trusted.is_empty() { "none".to_owned() } else { trusted.join(", ") }
            );
        }
        Err(error) => eprintln!("leto-host: {error}."),
    }
    println!("Turn the camera on in Leto during a stream. Ctrl+C stops this.");

    let mut open = false;
    let mut size = None;
    let mut saved = 0;
    loop {
        thread::sleep(Duration::from_secs(1));
        let report = companion.cameras().report();
        if report.open && !open {
            let name = companion.sender().filter(|name| !name.is_empty());
            println!("{} turned its camera on.", name.as_deref().unwrap_or("A device"));
            size = None;
        }
        if !report.open && open {
            println!(
                "The camera is off again: {} pictures arrived, {} were lost.",
                report.pictures, report.lost
            );
        }
        open = report.open;
        if report.open && report.size != size && report.size.is_some() {
            size = report.size;
            if let Some((width, height)) = size {
                println!("Its pictures arrive: {width} by {height}.");
            }
        }
        // Every other second, and only a picture that is new.
        if let (Some(file), Some(seen)) = (save, companion.cameras().seen())
            && report.pictures != saved
            && report.pictures % 2 == saved % 2
        {
            saved = report.pictures;
            if let Err(error) = write(file, &seen) {
                eprintln!("leto-host: {} could not be written: {error}.", file.display());
            }
        }
    }
}

fn write(file: &Path, seen: &Seen) -> image::ImageResult<()> {
    image::save_buffer(
        file,
        &seen.rgba,
        seen.width as u32,
        seen.height as u32,
        image::ColorType::Rgba8,
    )
}

/// Warnings and what is worth knowing, on the terminal.
struct Printer;

impl log::Log for Printer {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info && metadata.target().starts_with("leto")
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!("leto-host: {}", record.args());
        }
    }

    fn flush(&self) {}
}
