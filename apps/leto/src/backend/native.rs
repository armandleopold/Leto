//! The backend of the desktop app: the client engine on worker threads.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};

use leto_client::{
    Client, Service, Shell,
    engine::{self, ImageFile},
    input::Input,
    service::{explain, sentence},
};
use leto_media::{Microphone, VOICE_FRAME};

use super::{
    Door, Picture, Unavailable, decode_cover, gamepad,
    stream::{Outlet, Screen, Sound},
};
use crate::state::{Event, Request};

pub struct Backend {
    /// The engine at work for this window, or why it could not start.
    service: Result<Arc<Service<Desk>>, String>,
    events: mpsc::Receiver<Event>,
}

/// What the engine works for on the desktop: this window.
struct Desk {
    events: mpsc::Sender<Event>,
    ctx: egui::Context,
    /// Where the stream on screen leaves its pictures.
    outlet: Mutex<Arc<Outlet>>,
    voice: Mutex<Voice>,
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

impl Desk {
    fn outlet(&self) -> Arc<Outlet> {
        Arc::clone(&self.outlet.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn voice(&self) -> MutexGuard<'_, Voice> {
        // Whether a microphone captures stays true even if a thread
        // panicked meanwhile.
        self.voice.lock().unwrap_or_else(PoisonError::into_inner)
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
            // Nobody listens any more, so nothing is captured any more: at
            // once, not when the window gets to hear of it.
            engine::Event::PlayFailed { attempt, .. }
            | engine::Event::StreamEnded { attempt, .. } => {
                let mut voice = self.voice();
                if voice.stream == Some(*attempt) {
                    *voice = Voice::default();
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
        let screen =
            Screen::new(&outlet, &self.ctx).map_err(|error| sentence(&error.to_string()))?;
        Ok((screen, Sound::new(&outlet)))
    }
}

impl Backend {
    pub fn new(ctx: &egui::Context) -> Self {
        let (sender, events) = mpsc::channel();
        let desk = Desk {
            events: sender,
            ctx: ctx.clone(),
            outlet: Mutex::default(),
            voice: Mutex::default(),
        };
        let service = Client::open_default()
            .map(|client| Arc::new(Service::new(Arc::new(client), desk)))
            .map_err(|error| explain(&error));
        match &service {
            Ok(service) => {
                // Gamepads go to the stream that is connected, whichever it is.
                let (asked, sent) = (Arc::clone(service), Arc::clone(service));
                gamepad::watch(move || asked.streaming(), move |input| sent.input([input]));
            }
            Err(error) => log::error!("the client engine did not start: {error}"),
        }
        Self { service, events }
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
            // No stream, or a new one: the microphone is off.
            *desk.voice() = Voice::default();
        }
        service.ask(request);
    }
}
