//! The backend of the desktop app: the client engine on worker threads.

use std::{
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

#[cfg(test)]
use leto_client::Permission;
use leto_client::{
    Client, Ended, Error, HostId, InputError, Session, StreamSettings, input::Input,
};

use super::{
    Unavailable,
    stream::{Outlet, Screen, Sound, sentence},
};
use crate::state::{AppsProblem, Event, Request, StreamEnd};

/// How long a search listens for PCs announcing themselves.
const LISTEN_TIME: Duration = Duration::from_secs(2);

/// Covers are shown small: anything wider is scaled down to this.
const COVER_WIDTH: u32 = 360;

pub struct Backend {
    /// The engine, or why it could not start.
    client: Result<Arc<Client>, String>,
    reporter: Reporter,
    events: mpsc::Receiver<Event>,
    /// Counts the app lists asked for. A worker loading the covers of a list
    /// nobody looks at any more notices and stops.
    apps_asked: Arc<AtomicU64>,
    watching: Arc<Mutex<Watching>>,
}

/// The stream the person is watching, or waiting for.
#[derive(Default)]
struct Watching {
    /// The attempt on screen. A stream that connects for another one is
    /// stopped at once: the person has left in the meantime.
    wanted: Option<u64>,
    session: Option<Session>,
    outlet: Arc<Outlet>,
}

/// Sends events to the interface and wakes it up.
#[derive(Clone)]
struct Reporter {
    events: mpsc::Sender<Event>,
    ctx: egui::Context,
}

impl Reporter {
    fn send(&self, event: Event) {
        // The interface is gone when the app is closing.
        let _ = self.events.send(event);
        self.ctx.request_repaint();
    }
}

impl Backend {
    pub fn new(ctx: &egui::Context) -> Self {
        let (sender, events) = mpsc::channel();
        let client = Client::open_default().map(Arc::new).map_err(|error| explain(&error));
        if let Err(error) = &client {
            log::error!("the client engine did not start: {error}");
        }
        Self {
            client,
            reporter: Reporter { events: sender, ctx: ctx.clone() },
            events,
            apps_asked: Arc::new(AtomicU64::new(0)),
            watching: Arc::default(),
        }
    }

    pub fn unavailable(&self) -> Option<Unavailable> {
        let explanation = self.client.as_ref().err()?.clone();
        Some(Unavailable { title: "Leto could not start", explanation })
    }

    /// The news that arrived since the last call.
    pub fn events(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    /// The latest picture of the stream that nobody has shown yet.
    pub fn take_picture(&self) -> Option<egui::ColorImage> {
        let outlet = Arc::clone(&lock(&self.watching).outlet);
        outlet.take_picture()
    }

    /// Why the stream has no sound, when it has none.
    pub fn sound_problem(&self) -> Option<String> {
        let outlet = Arc::clone(&lock(&self.watching).outlet);
        outlet.sound_problem()
    }

    /// Passes what the person did on to the PC being watched, in order.
    pub fn input(&self, inputs: Vec<Input>) {
        if inputs.is_empty() {
            return;
        }
        let watching = lock(&self.watching);
        let Some(session) = &watching.session else { return };
        for input in inputs {
            match session.send(input) {
                // The interface says what the PC does not accept.
                Ok(()) | Err(InputError::NotAllowed) => {}
                // The news that the stream ended is on its way.
                Err(InputError::NotConnected) => break,
            }
        }
    }

    /// Starts `request` on a worker thread. The outcome arrives as events.
    pub fn send(&self, request: Request) {
        let Ok(client) = &self.client else { return };
        let client = Arc::clone(client);
        let report = self.reporter.clone();
        let apps_asked = Arc::clone(&self.apps_asked);
        let watching = Arc::clone(&self.watching);
        // The stream to stop, for `Request::Leave`.
        let mut left = None;
        match &request {
            Request::Apps(_) => {
                apps_asked.fetch_add(1, Ordering::SeqCst);
            }
            // Which stream is wanted changes at once, not when a worker gets
            // to it: the next request may already be about another one.
            Request::Play { attempt, .. } => {
                let mut watching = lock(&watching);
                watching.wanted = Some(*attempt);
                watching.outlet = Arc::default();
            }
            Request::Leave => {
                let mut watching = lock(&watching);
                watching.wanted = None;
                left = watching.session.take();
            }
            _ => {}
        }
        let name = format!("leto: {request:?}");
        let spawned = thread::Builder::new().name(name).spawn(move || match request {
            Request::Search => search(&client, &report),
            Request::Add(address) => {
                let added = client.add(&address).map_err(|error| explain(&error));
                report.send(Event::Hosts(client.hosts()));
                report.send(Event::Added(added));
            }
            Request::Pair { host, attempt } => pair(&client, &report, &host, attempt),
            Request::Apps(host) => apps(&client, &report, &host, &apps_asked),
            Request::Forget(host) => {
                if let Err(error) = client.forget(&host) {
                    report.send(Event::Notice(explain(&error)));
                }
                report.send(Event::Hosts(client.hosts()));
            }
            Request::Play { host, app, attempt } => {
                play(&client, &report, &watching, &host, app, attempt);
            }
            Request::Leave => {
                // Winding a stream down takes a moment: not on the
                // interface's thread.
                if let Some(session) = left {
                    session.stop();
                }
            }
            Request::Quit(host) => {
                let result = client.quit_app(&host).map(|_| ()).map_err(|error| explain(&error));
                report.send(Event::Hosts(client.hosts()));
                report.send(Event::QuitEnded { host, result });
            }
        });
        if let Err(error) = spawned {
            log::error!("could not start a worker thread: {error}");
        }
    }
}

fn search(client: &Client, report: &Reporter) {
    // Listen on the network while the saved PCs are being checked.
    let listened = thread::scope(|scope| {
        let listening = scope.spawn(|| client.discover(LISTEN_TIME));
        client.refresh();
        report.send(Event::Hosts(client.hosts()));
        listening.join()
    });
    let network_searched = match listened {
        Ok(Ok(_)) => true,
        Ok(Err(error)) => {
            log::warn!("could not search the network: {error}");
            false
        }
        Err(_) => false,
    };
    report.send(Event::Hosts(client.hosts()));
    report.send(Event::SearchFinished { network_searched });
}

fn pair(client: &Client, report: &Reporter, host: &HostId, attempt: u64) {
    let result = client.new_pin().and_then(|pin| {
        report.send(Event::PinChosen { attempt, pin });
        client.pair(host, pin)
    });
    report.send(Event::Hosts(client.hosts()));
    report.send(Event::PairingEnded { attempt, result: result.map_err(|error| explain(&error)) });
}

fn apps(client: &Client, report: &Reporter, host: &HostId, apps_asked: &AtomicU64) {
    let mine = apps_asked.load(Ordering::SeqCst);
    let apps = match client.apps(host) {
        Ok(apps) => apps,
        Err(error) => {
            let needs_pairing = matches!(error, Error::NotPaired);
            if needs_pairing {
                // The list of PCs should say so too.
                client.refresh();
                report.send(Event::Hosts(client.hosts()));
            }
            let problem = AppsProblem { message: explain(&error), needs_pairing };
            report.send(Event::Apps { host: host.clone(), result: Err(problem) });
            return;
        }
    };
    report.send(Event::Apps { host: host.clone(), result: Ok(apps.clone()) });

    for app in apps {
        if apps_asked.load(Ordering::SeqCst) != mine {
            return;
        }
        let image = match client.app_image(host, app.id) {
            Ok(bytes) => decode_cover(&bytes),
            Err(error) => {
                log::debug!("no cover for {}: {error}", app.title);
                None
            }
        };
        report.send(Event::Cover { host: host.clone(), app: app.id, image });
    }
}

fn play(
    client: &Arc<Client>,
    report: &Reporter,
    watching: &Mutex<Watching>,
    host: &HostId,
    app: u32,
    attempt: u64,
) {
    let outlet = Arc::clone(&lock(watching).outlet);
    let started = Screen::new(&outlet, &report.ctx)
        .map_err(|error| sentence(&error.to_string()))
        .and_then(|screen| {
            let ended = {
                let (client, report) = (Arc::clone(client), report.clone());
                move |reason| {
                    let why = match reason {
                        Ended::ByHost => StreamEnd::AppClosed,
                        Ended::ConnectionLost => StreamEnd::ConnectionLost,
                        Ended::HostError(code) => StreamEnd::HostError(code),
                    };
                    report.send(Event::Hosts(client.hosts()));
                    report.send(Event::StreamEnded { attempt, why });
                }
            };
            let settings = StreamSettings::default();
            client
                .play(host, app, settings, screen, Sound::new(&outlet), ended)
                .map_err(|error| explain(&error))
        });
    // The PC is running the app now, or still runs another one.
    report.send(Event::Hosts(client.hosts()));

    match started {
        Ok(session) => {
            let mut watching = lock(watching);
            if watching.wanted == Some(attempt) {
                let input = session.input_allowed();
                watching.session = Some(session);
                drop(watching);
                report.send(Event::Playing { attempt, input });
            } else {
                // The person left while the PC was starting the app.
                drop(watching);
                session.stop();
            }
        }
        Err(message) => report.send(Event::PlayFailed { attempt, message }),
    }
}

fn lock(watching: &Mutex<Watching>) -> MutexGuard<'_, Watching> {
    // What is being watched stays true even if a thread panicked meanwhile.
    watching.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Turns the bytes of an image file into pixels, scaled down for the grid.
fn decode_cover(bytes: &[u8]) -> Option<egui::ColorImage> {
    let image = image::load_from_memory(bytes).ok()?.into_rgba8();
    let image = if image.width() > COVER_WIDTH {
        let height = u64::from(image.height()) * u64::from(COVER_WIDTH) / u64::from(image.width());
        let height = u32::try_from(height).ok()?.max(1);
        image::imageops::resize(&image, COVER_WIDTH, height, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    let size = [usize::try_from(image.width()).ok()?, usize::try_from(image.height()).ok()?];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()))
}

/// Says what went wrong and what to do about it, for the person.
fn explain(error: &Error) -> String {
    match error {
        Error::BadAddress(address) => {
            format!("\"{address}\" does not look like an address. Try something like 192.168.1.20.")
        }
        Error::Offline => "The PC did not answer. Check that it is on, that Apollo or Sunshine \
                           is running on it, and that the address is right."
            .to_owned(),
        Error::WrongPin => "The code did not match. Try again with a new code.".to_owned(),
        Error::PinTimeout => "Nobody typed the code on the PC in time.".to_owned(),
        Error::PairingBusy => {
            "Another device is pairing with this PC right now. Try again in a moment.".to_owned()
        }
        Error::PairingFailed => "The PC refused the pairing.".to_owned(),
        Error::NotPaired => {
            "This PC no longer knows this device. Pair again to continue.".to_owned()
        }
        Error::UnknownHost => "Leto no longer knows this PC.".to_owned(),
        Error::OtherAppRunning => {
            "Another app is running on this PC. Close it first, then start this one.".to_owned()
        }
        Error::NotAllowed(permission) => format!(
            "This PC does not let this device {permission}. On the PC, open Apollo's PIN page \
             and, under Device Management, allow \"{}\" for this device.",
            permission.apollo_name()
        ),
        Error::Unsupported(reason) => format!("This PC cannot stream to Leto yet: {reason}."),
        Error::Storage(error) => format!("Leto could not read or write its settings: {error}"),
        Error::Discovery(_) | Error::Protocol(_) => sentence(&error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explanations_are_full_sentences() {
        let errors = [
            Error::BadAddress("my pc".into()),
            Error::Offline,
            Error::WrongPin,
            Error::PinTimeout,
            Error::PairingBusy,
            Error::PairingFailed,
            Error::NotPaired,
            Error::UnknownHost,
            Error::OtherAppRunning,
            Error::NotAllowed(Permission::LaunchApps),
            Error::Unsupported("the host is too old".into()),
            Error::Discovery("no network interface".into()),
            Error::Protocol("unexpected answer".into()),
        ];
        for error in errors {
            let text = explain(&error);
            assert!(text.starts_with(|letter: char| letter.is_uppercase() || letter == '"'));
            assert!(text.ends_with('.'), "{text}");
        }
    }

    #[test]
    fn a_wide_cover_is_scaled_down_and_a_broken_one_is_dropped() {
        let wide = image::RgbaImage::from_pixel(720, 1080, image::Rgba([255, 107, 53, 255]));
        let mut file = std::io::Cursor::new(Vec::new());
        wide.write_to(&mut file, image::ImageFormat::Png).expect("a PNG can be written to memory");

        let cover = decode_cover(file.get_ref()).expect("a valid PNG decodes");
        assert_eq!(cover.size, [360, 540]);
        assert_eq!(cover.pixels[0], egui::Color32::from_rgb(255, 107, 53));

        assert!(decode_cover(b"not an image").is_none());
    }
}
