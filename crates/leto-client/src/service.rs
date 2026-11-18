//! The engine at the service of an interface: asked with [`Request`]s, it
//! works on threads of its own and answers with [`Event`]s.
//!
//! The desktop app drives the engine this way, and so does the bridge on
//! behalf of each browser. A PC is therefore found, paired and streamed the
//! same from either, and says the same things when something goes wrong.

use std::{
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use leto_core::{
    engine::{AppsProblem, Event, ImageFile, Request, StreamEnd},
    input::Input,
};

use crate::{
    AudioSink, Client, Ended, Error, HostId, InputError, Session, StreamSettings, VideoSink,
};

/// How long a search listens for PCs announcing themselves.
const LISTEN_TIME: Duration = Duration::from_secs(2);

/// Whoever the engine works for: where its news goes, and where the streams
/// it starts end up.
pub trait Shell: Send + Sync + 'static {
    type Video: VideoSink;
    type Audio: AudioSink;

    /// News for the interface. Called from the engine's threads.
    fn tell(&self, event: Event);

    /// Where the pictures and the sound of a stream about to start go, or
    /// why this shell cannot show a stream, in words for the person.
    fn sinks(&self) -> Result<(Self::Video, Self::Audio), String>;
}

/// The stream the person is watching, or waiting for.
#[derive(Default)]
struct Watching {
    /// The attempt on screen. A stream that connects for another one is
    /// stopped at once: the person has left in the meantime.
    wanted: Option<u64>,
    session: Option<Session>,
}

/// The engine, working for one interface.
pub struct Service<S: Shell> {
    client: Arc<Client>,
    shell: Arc<S>,
    /// Counts the app lists asked for. A worker loading the covers of a list
    /// nobody looks at any more notices and stops.
    apps_asked: Arc<AtomicU64>,
    watching: Arc<Mutex<Watching>>,
}

impl<S: Shell> Service<S> {
    /// Several services may share one engine: they then share its PCs and
    /// its pairings, and each has a stream of its own.
    pub fn new(client: Arc<Client>, shell: S) -> Self {
        Self {
            client,
            shell: Arc::new(shell),
            apps_asked: Arc::new(AtomicU64::new(0)),
            watching: Arc::default(),
        }
    }

    pub fn shell(&self) -> &S {
        &self.shell
    }

    /// Starts `request` on a worker thread. The outcome arrives as events.
    pub fn ask(&self, request: Request) {
        let client = Arc::clone(&self.client);
        let shell = Arc::clone(&self.shell);
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
            Request::Play { attempt, .. } => lock(&watching).wanted = Some(*attempt),
            Request::Leave => {
                let mut watching = lock(&watching);
                watching.wanted = None;
                left = watching.session.take();
            }
            _ => {}
        }
        let name = format!("leto: {request:?}");
        let spawned = thread::Builder::new().name(name).spawn(move || match request {
            Request::Search => search(&client, &*shell),
            Request::Add(address) => {
                let added = client.add(&address).map_err(|error| explain(&error));
                shell.tell(Event::Hosts(client.hosts()));
                shell.tell(Event::Added(added));
            }
            Request::Pair { host, attempt } => pair(&client, &*shell, &host, attempt),
            Request::Apps(host) => apps(&client, &*shell, &host, &apps_asked),
            Request::Forget(host) => {
                if let Err(error) = client.forget(&host) {
                    shell.tell(Event::Notice(explain(&error)));
                }
                shell.tell(Event::Hosts(client.hosts()));
            }
            Request::Play { host, app, attempt } => {
                play(&client, &shell, &watching, &host, app, attempt);
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
                shell.tell(Event::Hosts(client.hosts()));
                shell.tell(Event::QuitEnded { host, result });
            }
        });
        if let Err(error) = spawned {
            log::error!("could not start a worker thread: {error}");
        }
    }

    /// Passes what the person did on to the PC being watched, in order.
    pub fn input(&self, inputs: impl IntoIterator<Item = Input>) {
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

    /// The attempt whose stream is connected, when one is.
    pub fn streaming(&self) -> Option<u64> {
        let watching = lock(&self.watching);
        watching.session.as_ref().and(watching.wanted)
    }
}

impl<S: Shell> Drop for Service<S> {
    /// Whoever was watching is gone: so is the stream. The app keeps running
    /// on the PC.
    fn drop(&mut self) {
        let mut watching = lock(&self.watching);
        watching.wanted = None;
        if let Some(session) = watching.session.take() {
            drop(watching);
            let stopping =
                thread::Builder::new().name("leto: Leave".to_owned()).spawn(|| session.stop());
            if let Err(error) = stopping {
                log::error!("could not start a worker thread: {error}");
            }
        }
    }
}

fn search(client: &Client, shell: &impl Shell) {
    // Listen on the network while the saved PCs are being checked.
    let listened = thread::scope(|scope| {
        let listening = scope.spawn(|| client.discover(LISTEN_TIME));
        client.refresh();
        shell.tell(Event::Hosts(client.hosts()));
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
    shell.tell(Event::Hosts(client.hosts()));
    shell.tell(Event::SearchFinished { network_searched });
}

fn pair(client: &Client, shell: &impl Shell, host: &HostId, attempt: u64) {
    let result = client.new_pin().and_then(|pin| {
        shell.tell(Event::PinChosen { attempt, pin });
        client.pair(host, pin)
    });
    shell.tell(Event::Hosts(client.hosts()));
    shell.tell(Event::PairingEnded { attempt, result: result.map_err(|error| explain(&error)) });
}

fn apps(client: &Client, shell: &impl Shell, host: &HostId, apps_asked: &AtomicU64) {
    let mine = apps_asked.load(Ordering::SeqCst);
    let apps = match client.apps(host) {
        Ok(apps) => apps,
        Err(error) => {
            let needs_pairing = matches!(error, Error::NotPaired);
            if needs_pairing {
                // The list of PCs should say so too.
                client.refresh();
                shell.tell(Event::Hosts(client.hosts()));
            }
            let problem = AppsProblem { message: explain(&error), needs_pairing };
            shell.tell(Event::Apps { host: host.clone(), result: Err(problem) });
            return;
        }
    };
    shell.tell(Event::Apps { host: host.clone(), result: Ok(apps.clone()) });

    for app in apps {
        if apps_asked.load(Ordering::SeqCst) != mine {
            return;
        }
        let image = match client.app_image(host, app.id) {
            Ok(bytes) => Some(ImageFile(bytes)),
            Err(error) => {
                log::debug!("no cover for {}: {error}", app.title);
                None
            }
        };
        shell.tell(Event::Cover { host: host.clone(), app: app.id, image });
    }
}

fn play<S: Shell>(
    client: &Arc<Client>,
    shell: &Arc<S>,
    watching: &Mutex<Watching>,
    host: &HostId,
    app: u32,
    attempt: u64,
) {
    let started = shell.sinks().and_then(|(video, audio)| {
        let ended = {
            let (client, shell) = (Arc::clone(client), Arc::clone(shell));
            move |reason| {
                let why = match reason {
                    Ended::ByHost => StreamEnd::AppClosed,
                    Ended::ConnectionLost => StreamEnd::ConnectionLost,
                    Ended::HostError(code) => StreamEnd::HostError(code),
                };
                shell.tell(Event::Hosts(client.hosts()));
                shell.tell(Event::StreamEnded { attempt, why });
            }
        };
        client
            .play(host, app, StreamSettings::default(), video, audio, ended)
            .map_err(|error| explain(&error))
    });
    // The PC is running the app now, or still runs another one.
    shell.tell(Event::Hosts(client.hosts()));

    match started {
        Ok(session) => {
            let mut watching = lock(watching);
            if watching.wanted == Some(attempt) {
                let input = session.input_allowed();
                watching.session = Some(session);
                drop(watching);
                shell.tell(Event::Playing { attempt, input });
            } else {
                // The person left while the PC was starting the app.
                drop(watching);
                session.stop();
            }
        }
        Err(message) => shell.tell(Event::PlayFailed { attempt, message }),
    }
}

fn lock(watching: &Mutex<Watching>) -> MutexGuard<'_, Watching> {
    // What is being watched stays true even if a thread panicked meanwhile.
    watching.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Says what went wrong and what to do about it, for the person.
pub fn explain(error: &Error) -> String {
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

/// Makes a sentence of an error message: a capital first, a full stop last.
pub fn sentence(text: &str) -> String {
    let mut letters = text.chars();
    let first = letters.next().map(|letter| letter.to_uppercase().to_string());
    format!("{}{}.", first.unwrap_or_default(), letters.as_str().trim_end_matches('.'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Permission;

    #[test]
    fn sentences_get_a_capital_and_one_full_stop() {
        assert_eq!(sentence("the PC did not answer"), "The PC did not answer.");
        assert_eq!(sentence("It may have been disconnected."), "It may have been disconnected.");
    }

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
}
