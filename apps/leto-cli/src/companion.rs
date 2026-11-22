//! `leto-cli companion`: Leto on the PC that is shared, with no window.
//! The Leto app does the same behind "This PC".

use std::{path::Path, thread, time::Duration};

use leto_host::{Companion, Options, apollo::Apollo, camera::Seen};

/// Takes the camera of the devices the Apollo of this PC is paired with,
/// until stopped.
pub fn run(folder: Option<&Path>, save: Option<&Path>, local: bool) -> Result<String, String> {
    let apollo = match folder {
        Some(folder) => Apollo::read(folder),
        None => Apollo::find(),
    };
    let apollo = apollo.map_err(|error| {
        format!(
            "{error}. Apollo keeps its settings in its `config` folder: if it is elsewhere, \
             name it with --apollo. If Windows refuses to let it be read, start this from a \
             terminal opened as administrator"
        )
    })?;
    let mut options = Options::default();
    if local {
        options.interface = "127.0.0.1".to_owned();
    }
    let companion = Companion::start(apollo, &options)
        .map_err(|error| format!("the port for the camera could not be opened: {error}"))?;

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
        Err(error) => eprintln!("error: {error}."),
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
                eprintln!("error: {} could not be written: {error}.", file.display());
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
