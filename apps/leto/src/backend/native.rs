//! The backend of the desktop app: the client engine on worker threads.

use std::{
    cell::RefCell,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError, Weak,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

use leto_client::{
    CameraSettings, Client, Service, Shell,
    engine::{self, Decodable, ImageFile},
    input::Input,
    service::{explain, sentence},
};
use leto_media::{
    Camera, CameraFeed, Carrier, Encoded, Feeding, Limits, Microphone, Sending, VOICE_FRAME,
};

use super::{
    Door, Filming, Pace, Picture, Seen, Unavailable, decode_cover, gamepad, pads,
    stream::{Outlet, Screen, Sound},
};
use crate::{
    state::{Event, Request},
    stats::Counts,
};

pub struct Backend {
    /// The engine at work for this window, or why it could not start.
    service: Result<Arc<Service<Desk>>, String>,
    events: mpsc::Receiver<Event>,
    ctx: egui::Context,
    /// The camera and the microphone while the person tries them out.
    tried: RefCell<Tried>,
    /// The person has A and B change places on the gamepads, and X and Y.
    swapped: Arc<AtomicBool>,
}

/// The camera and the microphone of this device, tried out with no PC
/// involved: what they capture goes to the window and nowhere else.
#[derive(Default)]
struct Tried {
    camera: Option<Camera>,
    /// The picture the camera's pixels are taken into.
    picture: leto_media::Picture,
    microphone: Option<Microphone>,
}

/// Set to `card` in the environment, the camera is a test card: for trying
/// the app on a device that has none.
const CAMERA: &str = "LETO_CAMERA";

/// Whether the camera is the test card.
fn is_card() -> bool {
    std::env::var_os(CAMERA).is_some_and(|camera| camera == "card")
}

/// What the engine works for on the desktop: this window.
struct Desk {
    events: mpsc::Sender<Event>,
    ctx: egui::Context,
    /// Where the stream on screen leaves its pictures.
    outlet: Mutex<Arc<Outlet>>,
    voice: Mutex<Voice>,
    film: Mutex<Film>,
    /// The person lets the graphics card decode the pictures.
    hardware: AtomicBool,
}

/// The microphone of this device, which only ever captures for a stream
/// that is connected to a PC that takes one.
#[derive(Default)]
struct Voice {
    /// The stream whose PC takes a microphone, while it is connected.
    stream: Option<u64>,
    /// The microphone, while the person has it on.
    microphone: Option<Microphone>,
}

/// The camera of this device, which only ever captures for a stream that is
/// connected to a PC that takes one.
#[derive(Default)]
struct Film {
    /// The stream the person asked for last.
    stream: Option<u64>,
    /// How much of a camera the PC of that stream takes, once it said that
    /// it takes one.
    limits: Option<Limits>,
    /// The camera on its way there, while the person has it on.
    feed: Option<CameraFeed>,
}

/// Carries the pictures of the camera to the PC being watched.
struct ToPc {
    /// The engine outlives no camera: it must not be kept alive by one.
    service: Weak<Service<Desk>>,
    /// The number the engine gave this camera, once the PC agreed.
    camera: Option<u64>,
}

impl Carrier for ToPc {
    fn open(&mut self, sending: Sending) -> Result<(), String> {
        let service = self.service.upgrade().ok_or("Leto is closing.")?;
        let settings = CameraSettings {
            width: sending.width,
            height: sending.height,
            fps: sending.fps,
            bitrate_kbps: sending.bitrate_kbps,
        };
        self.camera = Some(service.camera_open(settings)?);
        Ok(())
    }

    fn carry(&mut self, at: Duration, picture: Encoded<'_>) -> Option<bool> {
        self.service.upgrade()?.camera(self.camera?, at, picture.key_frame, picture.data)
    }
}

impl Drop for ToPc {
    /// However the feed ended, the PC is told that the camera stopped.
    fn drop(&mut self) {
        if let (Some(service), Some(camera)) = (self.service.upgrade(), self.camera) {
            service.camera_close(camera);
        }
    }
}

impl Desk {
    fn outlet(&self) -> Arc<Outlet> {
        Arc::clone(&self.outlet.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn voice(&self) -> MutexGuard<'_, Voice> {
        // Whether a microphone captures stays true even if a thread
        // panicked meanwhile.
        self.voice.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn film(&self) -> MutexGuard<'_, Film> {
        // The same goes for a camera.
        self.film.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Shell for Desk {
    type Video = Screen;
    type Audio = Sound;

    fn tell(&self, event: engine::Event) {
        match &event {
            engine::Event::Playing { attempt, microphone: true, .. } => {
                self.voice().stream = Some(*attempt);
            }
            // News of a stream the person left says nothing about the one
            // on screen.
            engine::Event::Camera { attempt, offer } => {
                let mut film = self.film();
                if film.stream == Some(*attempt) {
                    film.limits = offer.map(|offer| Limits {
                        width: offer.max_width,
                        height: offer.max_height,
                        fps: offer.max_fps,
                    });
                }
            }
            // Nobody listens or looks any more, so nothing is captured any
            // more: at once, not when the window gets to hear of it.
            engine::Event::PlayFailed { attempt, .. }
            | engine::Event::StreamEnded { attempt, .. } => {
                let mut voice = self.voice();
                if voice.stream == Some(*attempt) {
                    *voice = Voice::default();
                }
                drop(voice);
                let mut film = self.film();
                if film.stream == Some(*attempt) {
                    *film = Film::default();
                }
            }
            _ => {}
        }
        // Covers are decoded here, on the engine's thread, not while drawing.
        let event = event.with_covers(|file: ImageFile| decode_cover(&file.0));
        // The interface is gone when the app is closing.
        let _ = self.events.send(event);
        self.ctx.request_repaint();
    }

    fn sinks(&self) -> Result<(Screen, Sound), String> {
        let outlet = self.outlet();
        let screen = Screen::new(&outlet, &self.ctx, self.hardware.load(Ordering::Relaxed));
        Ok((screen, Sound::new(&outlet)))
    }

    fn sends_camera(&self) -> bool {
        true
    }
}

impl Backend {
    pub fn new(ctx: &egui::Context) -> Self {
        crate::window::watch(ctx);
        let (sender, events) = mpsc::channel();
        let desk = Desk {
            events: sender,
            ctx: ctx.clone(),
            outlet: Mutex::default(),
            voice: Mutex::default(),
            film: Mutex::default(),
            hardware: AtomicBool::new(true),
        };
        // Finding out what this device decodes takes a moment on Windows,
        // better spent now than when the person asks for a stream.
        let _ =
            std::thread::Builder::new().name("leto-decoders".into()).spawn(leto_media::decodable);
        let service = Client::open_default()
            .map(|client| Arc::new(Service::new(Arc::new(client), desk)))
            .map_err(|error| explain(&error));
        let swapped = Arc::new(AtomicBool::new(false));
        match &service {
            Ok(service) => {
                // Gamepads go to the stream that is connected, whichever it
                // is, as the person chose.
                let (asked, sent) = (Arc::clone(service), Arc::clone(service));
                let chosen = Arc::clone(&swapped);
                gamepad::watch(
                    move || asked.streaming(),
                    move |input| {
                        sent.input([pads::as_chosen(input, chosen.load(Ordering::Relaxed))]);
                    },
                );
            }
            Err(error) => log::error!("the client engine did not start: {error}"),
        }
        Self { service, events, ctx: ctx.clone(), tried: RefCell::default(), swapped }
    }

    /// The engine here can run a WireGuard tunnel of its own.
    pub fn has_tunnel(&self) -> bool {
        true
    }

    /// Open when the engine started, closed for good when it did not.
    pub fn door(&self) -> Door {
        match &self.service {
            Ok(_) => Door::Open,
            Err(explanation) => Door::Closed(Unavailable {
                title: "Leto could not start",
                explanation: explanation.clone(),
            }),
        }
    }

    /// Nobody asks the desktop app for a code.
    pub fn knock(&self, _code: &str) {}

    /// The news that arrived since the last call.
    pub fn events(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    /// The latest picture of the stream that nobody has shown yet.
    pub fn take_picture(&self) -> Option<Picture> {
        let outlet = self.service.as_ref().ok()?.shell().outlet();
        outlet.take_picture().map(Picture::Pixels)
    }

    /// The window draws the pictures itself: nothing to place.
    pub fn place_picture(&self, _place: Option<egui::Rect>) {}

    /// Why the stream has no sound, when it has none.
    pub fn sound_problem(&self) -> Option<String> {
        let outlet = self.service.as_ref().ok()?.shell().outlet();
        outlet.sound_problem()
    }

    /// Why the stream has no pictures, when it has none.
    pub fn picture_problem(&self) -> Option<String> {
        self.service.as_ref().ok()?.shell().outlet().picture_problem()
    }

    /// Makes as many pictures of the stream into pixels as the person sees.
    pub fn pace(&self, pace: Pace) {
        if let Ok(service) = &self.service {
            service.shell().outlet().set_pace(pace);
        }
    }

    /// Silences the stream, or lets it be heard again.
    pub fn mute(&self, mute: bool) {
        if let Ok(service) = &self.service {
            service.shell().outlet().set_muted(mute);
        }
    }

    /// Has A and B change places on the gamepads, and X and Y, or not.
    pub fn swap_gamepad_buttons(&self, swap: bool) {
        self.swapped.store(swap, Ordering::Relaxed);
    }

    /// The codecs this device decodes, on its graphics card and at all.
    pub fn decodable(&self) -> Decodable {
        leto_media::decodable()
    }

    /// Lets the graphics card decode the pictures of the next streams, or
    /// leaves them to the processor.
    pub fn allow_graphics_card(&self, allowed: bool) {
        if let Ok(service) = &self.service {
            service.shell().hardware.store(allowed, Ordering::Relaxed);
        }
    }

    /// What happened to the pictures of the stream so far.
    pub fn counts(&self) -> Option<Counts> {
        Some(self.service.as_ref().ok()?.shell().outlet().counts())
    }

    /// How long a message takes to reach the PC and come back, once the
    /// stream has measured it.
    pub fn round_trip(&self) -> Option<Duration> {
        self.service.as_ref().ok()?.round_trip()
    }

    /// Captures the microphone for the PC being watched while `on`, and
    /// stops capturing when it is not. Returns how loud it is while it
    /// captures, from 0 to 1, or why it cannot, in words for the person.
    ///
    /// Asked every frame. The microphone is opened the first time, which is
    /// when the system may ask the person whether Leto may listen.
    pub fn microphone(&self, on: bool) -> Result<Option<f32>, String> {
        let Ok(service) = &self.service else { return Ok(None) };
        let mut voice = service.shell().voice();
        if !on || voice.stream.is_none() {
            voice.microphone = None;
            return Ok(None);
        }
        if voice.microphone.as_ref().is_some_and(Microphone::is_lost) {
            voice.microphone = None;
            return Err("The microphone stopped. It may have been unplugged.".to_owned());
        }
        if voice.microphone.is_none() {
            // The engine outlives no microphone: it must not be kept alive
            // by one.
            let listener = Arc::downgrade(service);
            let mut frames = 0u32;
            let opened = Microphone::open(move |frame| {
                if let Some(service) = listener.upgrade() {
                    service.microphone(VOICE_FRAME * frames, frame);
                }
                frames = frames.wrapping_add(1);
            });
            voice.microphone = Some(opened.map_err(|error| sentence(&error.to_string()))?);
        }
        Ok(voice.microphone.as_ref().map(Microphone::level))
    }

    /// Sends the camera of this device to the PC being watched while `on`,
    /// and stops capturing when it is not. Returns how that is going while
    /// it captures, or why it cannot, in words for the person.
    ///
    /// Asked every frame. The camera is opened the first time, and the PC
    /// asked once it gave a picture: until then nothing leaves.
    pub fn film(&self, on: bool) -> Result<Option<Filming>, String> {
        let Ok(service) = &self.service else { return Ok(None) };
        let mut film = service.shell().film();
        let Some(limits) = film.limits.filter(|_| on) else {
            film.feed = None;
            return Ok(None);
        };
        let feed = film.feed.get_or_insert_with(|| {
            let carrier = ToPc { service: Arc::downgrade(service), camera: None };
            let ctx = self.ctx.clone();
            let news = move || ctx.request_repaint();
            if is_card() {
                CameraFeed::test_card(limits, carrier, news)
            } else {
                CameraFeed::open(limits, carrier, news)
            }
        });
        let problem = match feed.state() {
            Feeding::Starting => return Ok(Some(Filming::Opening)),
            Feeding::Sending(sending) => {
                return Ok(Some(Filming::Sending([sending.width, sending.height])));
            }
            Feeding::Over => "The stream ended, and the camera with it.".to_owned(),
            Feeding::Lost(why) => sentence(&why),
            Feeding::Refused(why) => why,
        };
        film.feed = None;
        Err(problem)
    }

    /// Captures the camera of this device while `on`, for the person to
    /// look at, and stops capturing when it is not. Returns what it shows
    /// when that is news, or why it cannot be used, in words for the person.
    ///
    /// Asked every frame. The camera is opened the first time, which takes
    /// it a moment: until then there is no picture.
    pub fn camera(&self, on: bool) -> Result<Option<Seen>, String> {
        let mut tried = self.tried.borrow_mut();
        let Tried { camera, picture, .. } = &mut *tried;
        if !on {
            *camera = None;
            return Ok(None);
        }
        let capturing = camera.get_or_insert_with(|| {
            let ctx = self.ctx.clone();
            let news = move || ctx.request_repaint();
            if is_card() { Camera::test_card(news) } else { Camera::open(news) }
        });
        match capturing.take(picture) {
            Ok(true) => {
                let size = [picture.width, picture.height];
                let pixels = egui::ColorImage::from_rgba_premultiplied(size, &picture.rgba);
                Ok(Some(Seen { picture: pixels, size }))
            }
            Ok(false) => Ok(None),
            Err(error) => {
                *camera = None;
                Err(sentence(&error.to_string()))
            }
        }
    }

    /// Listens to the microphone of this device while `on`, for the person
    /// to see that it hears them, and stops when it is not. Returns how
    /// loud it is, from 0 to 1, or why it cannot be used.
    ///
    /// Asked every frame. Nothing of what it hears is kept or sent.
    pub fn hear(&self, on: bool) -> Result<Option<f32>, String> {
        let mut tried = self.tried.borrow_mut();
        if !on {
            tried.microphone = None;
            return Ok(None);
        }
        if tried.microphone.as_ref().is_some_and(Microphone::is_lost) {
            tried.microphone = None;
            return Err("The microphone stopped. It may have been unplugged.".to_owned());
        }
        if tried.microphone.is_none() {
            let opened = Microphone::open(|_| {});
            tried.microphone = Some(opened.map_err(|error| sentence(&error.to_string()))?);
        }
        Ok(tried.microphone.as_ref().map(Microphone::level))
    }

    /// Passes what the person did on to the PC being watched, in order.
    pub fn input(&self, inputs: Vec<Input>) {
        if let Ok(service) = &self.service {
            service.input(inputs);
        }
    }

    /// Starts `request` on a worker thread. The outcome arrives as events.
    pub fn send(&self, request: Request) {
        let Ok(service) = &self.service else { return };
        let desk = service.shell();
        if matches!(request, Request::Play { .. }) {
            // A new stream leaves its pictures in a place of its own: what
            // an earlier one left is not for this one.
            *desk.outlet.lock().unwrap_or_else(PoisonError::into_inner) = Arc::default();
        }
        if matches!(request, Request::Play { .. } | Request::Leave) {
            // No stream, or a new one: the microphone is off, and so is the
            // camera.
            *desk.voice() = Voice::default();
            let stream = match &request {
                Request::Play { attempt, .. } => Some(*attempt),
                _ => None,
            };
            *desk.film() = Film { stream, ..Film::default() };
        }
        service.ask(request);
    }
}
