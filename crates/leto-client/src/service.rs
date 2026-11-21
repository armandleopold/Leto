//! The engine at the service of an interface: asked with [`Request`]s, it
//! works on threads of its own and answers with [`Event`]s.
//!
//! The desktop app drives the engine this way, and so does the bridge on
//! behalf of each browser. A PC is therefore found, paired and streamed the
//! same from either, and says the same things when something goes wrong.

use std::{
    iter,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use leto_core::{
    engine::{AppsProblem, Event, ImageFile, Request, StreamEnd},
    input::Input,
};

use crate::{
    AudioSink, CameraLink, CameraRefusal, CameraSettings, Client, Ended, Error, HostId, InputError,
    Session, StreamSettings, VideoSink,
};

/// How long a search listens for PCs announcing themselves.
const LISTEN_TIME: Duration = Duration::from_secs(2);
/// How long a PC asked to wake up may take to answer: a PC that was off
/// starts its system and its streaming software first.
pub const WAKE_PATIENCE: Duration = Duration::from_secs(120);

/// How long a lost stream waits before it is asked for again: longer each
/// time, then the last for as long as it takes.
const AGAIN_AFTER: [Duration; 4] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(3),
    Duration::from_secs(5),
];

/// How long a stream that connected may take to bring its first picture
/// before it is asked for again. A PC sends one as soon as a stream starts:
/// the streams measured had theirs within a second and a half.
const FIRST_PICTURE_PATIENCE: Duration = Duration::from_secs(4);

/// How many times in a row a stream without a first picture is asked for
/// again by itself. After that it waits: a PC that has nothing to show has
/// nothing more for being asked again.
const FIRST_PICTURE_TRIES: u32 = 2;

/// How often a lost stream, waiting to be asked for again, checks that the
/// person still wants it.
const STILL_WANTED_EVERY: Duration = Duration::from_millis(100);

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

    /// Whether this shell can send a camera to a PC. The engine only finds
    /// out whether a PC takes one, and tells with [`Event::Camera`], for a
    /// shell that can.
    fn sends_camera(&self) -> bool {
        false
    }
}

/// The stream the person is watching, or waiting for.
#[derive(Default)]
struct Watching {
    /// The attempt on screen. A stream that connects for another one is
    /// stopped at once: the person has left in the meantime.
    wanted: Option<u64>,
    /// The PC the stream comes from.
    host: Option<HostId>,
    session: Option<Session>,
    /// How many streams connected so far: each has its number.
    connected: u64,
    /// The way of this device's camera to that PC, while it sends one, and
    /// its number.
    camera: Option<(u64, CameraLink)>,
    /// How many cameras were sent so far: each has its number.
    cameras: u64,
}

impl Watching {
    /// The attempt whose stream plays. One that ended by itself keeps its
    /// session until the person leaves it, and plays no more.
    fn playing(&self) -> Option<u64> {
        self.session.as_ref().filter(|session| !session.is_over()).and(self.wanted)
    }
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
            Request::Play { attempt, .. } => {
                let mut watching = lock(&watching);
                watching.wanted = Some(*attempt);
                // A camera goes with one stream, and that one is over. The
                // next camera does not go to the PC it came from either.
                watching.camera = None;
                watching.host = None;
            }
            Request::Leave => {
                let mut watching = lock(&watching);
                watching.wanted = None;
                watching.camera = None;
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
            Request::Play { host, app, attempt, screen, choices } => {
                // A screen this device knows is filled as it is, unless the
                // person chose otherwise.
                let settings = StreamSettings::chosen(screen, choices);
                let without_picture = AtomicU32::new(0);
                let stream = Stream {
                    client,
                    shell,
                    watching,
                    host,
                    app,
                    attempt,
                    settings,
                    without_picture,
                };
                play(&Arc::new(stream));
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
            Request::Wake(host) => {
                let result = client.wake_up(&host, WAKE_PATIENCE).map_err(|error| explain(&error));
                shell.tell(Event::Hosts(client.hosts()));
                shell.tell(Event::WakeEnded { host, result });
            }
            Request::Tunnel => {
                shell.tell(Event::Tunnel { summary: client.tunnel(), refused: None })
            }
            Request::UseTunnel(file) => {
                let refused = client.use_tunnel(&file.0).err().map(|error| explain(&error));
                shell.tell(Event::Tunnel { summary: client.tunnel(), refused });
                // The PCs it leads to are looked for through it.
                search(&client, &*shell);
            }
            Request::RemoveTunnel => {
                let refused = client.remove_tunnel().err().map(|error| explain(&error));
                shell.tell(Event::Tunnel { summary: client.tunnel(), refused });
                search(&client, &*shell);
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

    /// Passes a frame of this device's microphone on to the PC being
    /// watched, as [`Session::send_microphone`] takes it. Returns whether it
    /// left: nothing does without a stream, or to a PC that takes none.
    pub fn microphone(&self, at: Duration, frame: &[u8]) -> bool {
        let watching = lock(&self.watching);
        watching.session.as_ref().is_some_and(|session| session.send_microphone(at, frame))
    }

    /// Asks the PC being watched to take this device's camera, which will
    /// send pictures like `sending`: within what [`Event::Camera`] said the
    /// PC takes. Blocks until the PC has answered, so not for a thread that
    /// draws. When the PC does not agree, says why in words for the person.
    ///
    /// Returns the number of this camera, to send its pictures and to stop
    /// it with. It stops with the stream too, and when the camera is asked
    /// for again.
    pub fn camera_open(&self, sending: CameraSettings) -> Result<u64, String> {
        const NO_STREAM: &str = "The camera goes along with a stream, and none is playing.";
        let (attempt, host) = {
            let watching = lock(&self.watching);
            match (watching.playing(), &watching.host) {
                (Some(attempt), Some(host)) => (attempt, host.clone()),
                _ => return Err(NO_STREAM.to_owned()),
            }
        };
        let link = self.client.send_camera(&host, sending).map_err(|error| explain(&error))?;
        let mut watching = lock(&self.watching);
        if watching.playing() == Some(attempt) {
            watching.cameras += 1;
            watching.camera = Some((watching.cameras, link));
            Ok(watching.cameras)
        } else {
            // The person left while the PC was answering. Dropping the way
            // there tells the PC.
            Err(NO_STREAM.to_owned())
        }
    }

    /// Passes a picture of the camera of this number on to the PC that
    /// agreed to take it, as [`CameraLink::send`] takes it. Says whether
    /// all of it left, or `None` when that camera is not being sent any
    /// more: it was stopped, or the stream is over.
    pub fn camera(
        &self,
        camera: u64,
        at: Duration,
        key_frame: bool,
        picture: &[u8],
    ) -> Option<bool> {
        let mut watching = lock(&self.watching);
        let (_, link) = watching.camera.as_mut().filter(|(number, _)| *number == camera)?;
        Some(link.send(at, key_frame, picture))
    }

    /// Stops sending the camera of this number, and tells the PC. Does not
    /// wait. A camera that was asked for since goes on.
    pub fn camera_close(&self, camera: u64) {
        let mut watching = lock(&self.watching);
        if watching.camera.as_ref().is_some_and(|(number, _)| *number == camera) {
            watching.camera = None;
        }
    }

    /// The attempt whose stream is connected, when one is.
    pub fn streaming(&self) -> Option<u64> {
        let watching = lock(&self.watching);
        watching.session.as_ref().and(watching.wanted)
    }

    /// How long a message takes to reach the PC streamed from and come
    /// back, when a stream plays and has measured it.
    pub fn round_trip(&self) -> Option<Duration> {
        let watching = lock(&self.watching);
        watching.session.as_ref().filter(|session| !session.is_over())?.round_trip()
    }
}

impl<S: Shell> Drop for Service<S> {
    /// Whoever was watching is gone: so is the stream. The app keeps running
    /// on the PC.
    fn drop(&mut self) {
        let mut watching = lock(&self.watching);
        watching.wanted = None;
        watching.camera = None;
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

/// A stream the person asked for: all it takes to ask the PC for it, again
/// if it is lost.
struct Stream<S: Shell> {
    client: Arc<Client>,
    shell: Arc<S>,
    watching: Arc<Mutex<Watching>>,
    host: HostId,
    app: u32,
    attempt: u64,
    settings: StreamSettings,
    /// How many times in a row it connected and brought no picture.
    without_picture: AtomicU32,
}

/// Why a stream did not start.
enum NotStarted {
    /// The shell cannot show it, in words for the person.
    Shell(String),
    Client(Error),
}

/// Whether asking again later may change `error`: a PC that did not
/// answer, or answered in a way Leto did not expect, as one still busy with
/// the stream it lost does. Every other answer stays.
fn passing(error: &Error) -> bool {
    matches!(error, Error::Offline | Error::Protocol(_) | Error::Discovery(_))
}

impl NotStarted {
    /// In words for the person; the log gets the details.
    fn explain(&self) -> String {
        match self {
            Self::Shell(words) => words.clone(),
            Self::Client(error) => explain(error),
        }
    }
}

fn play<S: Shell>(stream: &Arc<Stream<S>>) {
    if let Err(not_started) = start(stream) {
        let message = not_started.explain();
        stream.shell.tell(Event::PlayFailed { attempt: stream.attempt, message });
    }
}

/// Asks the PC for `stream`, and tells once it plays.
fn start<S: Shell>(stream: &Arc<Stream<S>>) -> Result<(), NotStarted> {
    let Stream { client, shell, watching, host, app, attempt, settings, .. } = &**stream;
    let attempt = *attempt;
    let (video, audio) = shell.sinks().map_err(NotStarted::Shell)?;
    let ended = {
        let stream = Arc::clone(stream);
        move |reason| ended(&stream, reason)
    };
    let started = client.play(host, *app, *settings, video, audio, ended);
    // The PC is running the app now, or still runs another one.
    shell.tell(Event::Hosts(client.hosts()));
    let session = started.map_err(NotStarted::Client)?;

    let mut watching = lock(watching);
    if watching.wanted != Some(attempt) {
        // The person left while the PC was starting the app.
        drop(watching);
        session.stop();
        return Ok(());
    }
    let (input, microphone) = (session.input_allowed(), session.takes_microphone());
    let before = watching.session.replace(session);
    watching.host = Some(host.clone());
    watching.connected += 1;
    let number = watching.connected;
    drop(watching);
    // Winding down a stream waits for its threads, and one of them may be
    // waiting to say that it ended.
    drop(before);
    shell.tell(Event::Playing { attempt, input, microphone });
    if shell.sends_camera() {
        // After the news that the stream plays: a PC without Leto's API can
        // take seconds not to answer.
        let offer = client.camera_offer(host).unwrap_or_else(|error| {
            log::debug!("no camera for this stream: {error}");
            None
        });
        shell.tell(Event::Camera { attempt, offer });
    }
    let waiting = {
        let stream = Arc::clone(stream);
        thread::Builder::new()
            .name("leto: first picture".to_owned())
            .spawn(move || first_picture(&stream, number))
    };
    if let Err(error) = waiting {
        log::error!("could not start a worker thread: {error}");
    }
    Ok(())
}

/// Waits for the first picture of the stream that connected as `number`,
/// and says in the log what came from the PC when none does. A stream that
/// stays without one is given up and asked for again, as the person would
/// by leaving it and starting it once more.
fn first_picture<S: Shell>(stream: &Stream<S>, number: u64) {
    let since = Instant::now();
    let mut waits = true;
    loop {
        let waited = since.elapsed();
        let watching = lock(&stream.watching);
        if watching.wanted != Some(stream.attempt) || watching.connected != number {
            return;
        }
        let Some(session) = watching.session.as_ref().filter(|session| !session.is_over()) else {
            return;
        };
        let arrived = session.arrived();
        if arrived.pictures > 0 {
            log::info!(
                "the first picture came {:.1} s after the stream connected, in {} packets",
                waited.as_secs_f32(),
                arrived.packets
            );
            stream.without_picture.store(0, Ordering::SeqCst);
            return;
        }
        if waits && waited >= FIRST_PICTURE_PATIENCE {
            waits = false;
            let again = stream.without_picture.fetch_add(1, Ordering::SeqCst) < FIRST_PICTURE_TRIES;
            log::warn!(
                "no picture {:.0} s after the stream connected: {} packets of video came from \
                 the PC, which was asked {} times for a picture to start from; the stream {}",
                waited.as_secs_f32(),
                arrived.packets,
                arrived.keyframes_asked,
                if again { "is asked for again" } else { "waits on" }
            );
            if again {
                // Heard as a lost stream, which is asked for again.
                session.give_up();
                return;
            }
        }
        drop(watching);
        thread::sleep(STILL_WANTED_EVERY);
    }
}

/// `stream` ended without the person asking. A stream that lost its PC
/// while the person watches it is asked for again until they leave.
fn ended<S: Shell>(stream: &Arc<Stream<S>>, reason: Ended) {
    let (shell, attempt) = (&stream.shell, stream.attempt);
    let wanted = lock(&stream.watching).wanted == Some(attempt);
    let why = match reason {
        Ended::ByHost => StreamEnd::AppClosed,
        Ended::ConnectionLost => StreamEnd::ConnectionLost,
        Ended::HostError(code) => StreamEnd::HostError(code),
    };
    let again = wanted && why == StreamEnd::ConnectionLost;
    if again {
        log::info!("the stream was lost, and is asked for again until the person leaves it");
        // Before the camera's way goes: a shell stops the camera it sends
        // rather than find that way gone, and sends it again once the
        // stream is back.
        shell.tell(Event::Reconnecting { attempt });
    }
    {
        // The camera went along with this stream.
        let mut watching = lock(&stream.watching);
        if watching.wanted == Some(attempt) {
            watching.camera = None;
        }
    }
    shell.tell(Event::Hosts(stream.client.hosts()));
    if again {
        let stream = Arc::clone(stream);
        let spawned = thread::Builder::new()
            .name("leto: reconnect".to_owned())
            .spawn(move || reconnect(&stream));
        match spawned {
            Ok(_) => return,
            Err(error) => log::error!("could not start a worker thread: {error}"),
        }
    }
    shell.tell(Event::StreamEnded { attempt, why });
}

/// Asks the PC for a lost stream again and again, until it plays, the
/// person leaves it, or the PC refuses it for good.
fn reconnect<S: Shell>(stream: &Arc<Stream<S>>) {
    // What is left of the lost stream goes first: its pictures and its
    // sound make way for the next ones.
    let lost = {
        let mut watching = lock(&stream.watching);
        if watching.wanted != Some(stream.attempt) {
            return;
        }
        watching.session.take()
    };
    if let Some(lost) = lost {
        lost.stop();
    }
    let last = AGAIN_AFTER[AGAIN_AFTER.len() - 1];
    let waits = AGAIN_AFTER.into_iter().chain(iter::repeat(last));
    for (tried, wait) in (1_u64..).zip(waits) {
        if !still_wanted_after(stream, wait) {
            log::info!("the person left the lost stream");
            return;
        }
        match start(stream) {
            Ok(()) => {
                log::info!("the lost stream is back, asked for {tried} times");
                return;
            }
            Err(NotStarted::Client(error)) if passing(&error) => {
                log::debug!("the lost stream is not back yet: {error}");
            }
            Err(not_started) => {
                let message = not_started.explain();
                if still_wanted_after(stream, Duration::ZERO) {
                    stream.shell.tell(Event::PlayFailed { attempt: stream.attempt, message });
                }
                return;
            }
        }
    }
}

/// Waits `wait`, unless the person leaves `stream` meanwhile. Says whether
/// they still want it.
fn still_wanted_after<S: Shell>(stream: &Stream<S>, wait: Duration) -> bool {
    let until = Instant::now() + wait;
    loop {
        if lock(&stream.watching).wanted != Some(stream.attempt) {
            return false;
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        thread::sleep(left.min(STILL_WANTED_EVERY));
    }
}

fn lock(watching: &Mutex<Watching>) -> MutexGuard<'_, Watching> {
    // What is being watched stays true even if a thread panicked meanwhile.
    watching.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Says what went wrong and what to do about it, for the person. The log
/// gets the details too, which the words leave out.
pub fn explain(error: &Error) -> String {
    let words = words(error);
    log::warn!("{words} ({error:?})");
    words
}

fn words(error: &Error) -> String {
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
        Error::Camera(CameraRefusal::NotTaken) => {
            "This PC takes no camera. It needs Leto on the PC, which is not there yet.".to_owned()
        }
        Error::Camera(CameraRefusal::NotAllowed) => {
            "This PC does not let this device send its camera. Its owner can allow it on the PC."
                .to_owned()
        }
        Error::Camera(CameraRefusal::Busy) => {
            "Another device is sending its camera to this PC. Turn that one off first.".to_owned()
        }
        Error::Camera(CameraRefusal::Unsuitable) => {
            "This PC cannot take the pictures of this camera.".to_owned()
        }
        Error::Storage(error) => format!("Leto could not read or write its settings: {error}"),
        Error::CannotWake => "Leto cannot wake this PC yet. It learns how the next time the PC \
                              is on, once it is paired."
            .to_owned(),
        Error::WakeNotSent(why) => {
            format!("Leto could not send the signal that wakes this PC: {why}.")
        }
        Error::DidNotWake(seconds) => format!(
            "The PC did not wake up within {seconds} seconds. Check that Wake-on-LAN is on in \
             its BIOS and in the settings of its network card, and that this device is on the \
             same network as the PC."
        ),
        Error::Tunnel(why) => format!(
            "Leto cannot use this WireGuard file: {why}. Paste the whole file the WireGuard \
             app exports for this device."
        ),
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
            Error::Camera(CameraRefusal::NotTaken),
            Error::Camera(CameraRefusal::NotAllowed),
            Error::Camera(CameraRefusal::Busy),
            Error::Camera(CameraRefusal::Unsuitable),
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
