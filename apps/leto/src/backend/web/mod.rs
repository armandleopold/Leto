//! The backend of the web build: the bridge that served the page does the
//! work, and the browser shows the result.
//!
//! A page cannot open the sockets the Moonlight protocol needs. It keeps a
//! WebSocket to its bridge instead, in the language of
//! [`leto_core::bridge`]: requests, input and the microphone go up, news
//! and the stream come down. Pictures and sound arrive still encoded, and
//! the browser's own decoders turn them into something to see and hear.

mod camera;
mod capture;
mod codecs;
mod gamepad;
mod microphone;
mod screen;
mod speaker;

use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};

use js_sys::{ArrayBuffer, Uint8Array};
use leto_core::{
    bridge::{Down, Frame, PATH, Up},
    engine,
    input::Input,
};
use wasm_bindgen::{JsCast as _, closure::Closure};
use wasm_bindgen_futures::spawn_local;
use web_sys::{AudioContext, BinaryType, MessageEvent, WebSocket};

use self::{
    camera::Camera,
    gamepad::Gamepads,
    microphone::Microphone,
    screen::{Canvas, Screen, Shown},
    speaker::Speaker,
};
use super::{Door, Picture, Seen, Unavailable, decode_cover};
use crate::state::{Event, Request};

/// Where the browser keeps the key its bridge gave it.
const KEY: &str = "leto-bridge-key";

/// Milliseconds before trying again to reach a bridge that does not answer.
const TRY_AGAIN_AFTER: i32 = 2_000;

/// The rate PCs send sound at.
const SOUND_RATE: u32 = 48_000;

pub struct Backend {
    page: Rc<Page>,
}

/// What the page's callbacks and the app share. A page has one thread:
/// nothing here is ever used from two places at once.
struct Page {
    ctx: egui::Context,
    /// For the callbacks, which must not keep the page alive.
    this: Weak<Page>,
    inner: RefCell<Inner>,
}

struct Inner {
    door: Door,
    line: Option<Line>,
    /// News the app has not taken yet.
    events: Vec<Event>,
    stream: Option<Stream>,
    /// Made when first needed, then kept: a page gets few of each.
    canvas: Option<Rc<Canvas>>,
    speakers: Option<AudioContext>,
    /// Times the browser was asked for a microphone or a camera.
    asked: u32,
    /// The camera and the microphone while the person tries them out, with
    /// no PC involved: what they capture stays on the page.
    camera: Device<Camera>,
    ear: Device<Microphone>,
}

/// The WebSocket to the bridge, with what listens to it.
struct Line {
    socket: WebSocket,
    _opened: Closure<dyn FnMut()>,
    _heard: Closure<dyn FnMut(MessageEvent)>,
    _closed: Closure<dyn FnMut()>,
}

impl Drop for Line {
    fn drop(&mut self) {
        self.socket.set_onopen(None);
        self.socket.set_onmessage(None);
        self.socket.set_onclose(None);
        // Closing what is closed already is fine.
        let _ = self.socket.close();
    }
}

/// The stream the person asked for, from the asking on.
struct Stream {
    /// Tells it from a stream that was left.
    attempt: u64,
    screen: Option<Screen>,
    speaker: Option<Speaker>,
    /// Why this stream has no sound, when the bridge or the browser said.
    silent: Option<String>,
    /// The size of the pictures, as the app last heard it.
    told: Option<[usize; 2]>,
    /// The gamepads, from the moment the PC takes input.
    gamepads: Option<Gamepads>,
    /// The PC takes a microphone, and the stream is connected.
    listens: bool,
    /// The microphone of this browser, which only ever captures for a
    /// stream that is connected to a PC that takes one. It goes with the
    /// stream.
    voice: Device<Microphone>,
}

/// A microphone or a camera of this browser, from the asking on.
enum Device<T> {
    Off,
    /// The browser was asked for it, for this many-th time, and has not
    /// answered: it may be asking the person.
    Asked(u32),
    On(T),
    /// The browser gave none, and why.
    Refused(String),
}

/// What is left to do once the page's state is let go of.
enum Then {
    Nothing,
    Say(Up),
    /// The stream cannot go on, for a reason of the browser's.
    GiveUp {
        attempt: u64,
        message: String,
    },
}

impl Backend {
    pub fn new(ctx: &egui::Context) -> Self {
        crate::window::watch(ctx);
        let page = Rc::new_cyclic(|this| Page {
            ctx: ctx.clone(),
            this: this.clone(),
            inner: RefCell::new(Inner {
                door: Door::Opening { failed: false },
                line: None,
                events: Vec::new(),
                stream: None,
                canvas: None,
                speakers: None,
                asked: 0,
                camera: Device::Off,
                ear: Device::Off,
            }),
        });
        page.connect();
        Self { page }
    }

    /// Whether the bridge takes requests from this browser.
    pub fn door(&self) -> Door {
        self.page.inner.borrow().door.clone()
    }

    /// Gives the bridge the code it wants before letting a browser in.
    pub fn knock(&self, code: &str) {
        let digits: String = code.chars().filter(char::is_ascii_digit).collect();
        if let Door::Code { checking, refused } = &mut self.page.inner.borrow_mut().door {
            (*checking, *refused) = (true, false);
        }
        self.page.say(&Up::Code(digits));
    }

    /// The news that arrived since the last call.
    pub fn events(&self) -> Vec<Event> {
        std::mem::take(&mut self.page.inner.borrow_mut().events)
    }

    /// Asks the bridge for `request`. The outcome arrives as events.
    pub fn send(&self, request: Request) {
        {
            let mut inner = self.page.inner.borrow_mut();
            match request {
                Request::Play { attempt, .. } => {
                    if let Some(message) = screen::missing() {
                        inner.events.push(Event::PlayFailed { attempt, message });
                        self.page.ctx.request_repaint();
                        return;
                    }
                    inner.start_stream(attempt);
                }
                Request::Leave => inner.end_stream(),
                _ => {}
            }
        }
        self.page.say(&Up::Ask(request));
    }

    /// Captures the microphone for the PC being watched while `on`, and
    /// stops capturing when it is not. Returns how loud it is while it
    /// captures, from 0 to 1, or why it cannot, in words for the person.
    ///
    /// Asked every frame. The browser is asked for the microphone the first
    /// time, which is when it may ask the person. Until it answers nothing
    /// is captured, and this returns `None`.
    pub fn microphone(&self, on: bool) -> Result<Option<f32>, String> {
        let mut inner = self.page.inner.borrow_mut();
        let inner = &mut *inner;
        let Some(stream) = &mut inner.stream else { return Ok(None) };
        // Whatever it was: letting go of a microphone stops it.
        let voice = std::mem::replace(&mut stream.voice, Device::Off);
        if !on || !stream.listens {
            return Ok(None);
        }
        match voice {
            Device::Off => {
                let Some(speakers) = inner.speakers.clone() else {
                    return Err("This browser gave the page no sound, in or out.".to_owned());
                };
                // The microphone is heard at the pace of the page's sound.
                speaker::wake(&speakers);
                inner.asked += 1;
                let asked = inner.asked;
                stream.voice = Device::Asked(asked);
                let (page, listener) = (self.page.this.clone(), self.page.this.clone());
                spawn_local(async move {
                    let packet = move |packet: &[u8]| {
                        if let Some(page) = listener.upgrade() {
                            page.voice(packet);
                        }
                    };
                    let answer = Microphone::open(speakers, packet).await;
                    if let Some(page) = page.upgrade() {
                        page.microphone_answered(asked, answer);
                    }
                });
                Ok(None)
            }
            Device::Asked(asked) => {
                stream.voice = Device::Asked(asked);
                Ok(None)
            }
            Device::On(microphone) => match microphone.stopped() {
                Some(why) => Err(why.to_owned()),
                None => {
                    let level = microphone.level();
                    stream.voice = Device::On(microphone);
                    Ok(Some(level))
                }
            },
            Device::Refused(why) => Err(why),
        }
    }

    /// Captures the camera of this browser while `on`, for the person to
    /// look at, and stops capturing when it is not. Returns what it shows
    /// when that is news, or why it cannot be used, in words for the person.
    ///
    /// Asked every frame. The browser is asked for the camera the first
    /// time, which is when it may ask the person. Until it answers nothing
    /// is captured.
    pub fn camera(&self, on: bool) -> Result<Option<Seen>, String> {
        let mut inner = self.page.inner.borrow_mut();
        // Whatever it was: letting go of a camera stops it.
        let camera = std::mem::replace(&mut inner.camera, Device::Off);
        if !on {
            return Ok(None);
        }
        match camera {
            Device::Off => {
                inner.asked += 1;
                let asked = inner.asked;
                inner.camera = Device::Asked(asked);
                let page = self.page.this.clone();
                spawn_local(async move {
                    let answer = Camera::open().await;
                    if let Some(page) = page.upgrade() {
                        page.camera_answered(asked, answer);
                    }
                });
                Ok(None)
            }
            Device::Asked(asked) => {
                inner.camera = Device::Asked(asked);
                Ok(None)
            }
            Device::On(camera) => match camera.stopped() {
                Some(why) => Err(why.to_owned()),
                None => {
                    let seen = camera.picture();
                    inner.camera = Device::On(camera);
                    Ok(seen)
                }
            },
            Device::Refused(why) => Err(why),
        }
    }

    /// Listens to the microphone of this browser while `on`, for the person
    /// to see that it hears them, and stops when it is not. Returns how
    /// loud it is, from 0 to 1, or why it cannot be used.
    ///
    /// Asked every frame. It is captured exactly as it would be for a PC,
    /// so what works here works there, and nothing of it is sent.
    pub fn hear(&self, on: bool) -> Result<Option<f32>, String> {
        let mut inner = self.page.inner.borrow_mut();
        let ear = std::mem::replace(&mut inner.ear, Device::Off);
        if !on {
            return Ok(None);
        }
        match ear {
            Device::Off => {
                // The microphone is heard at the pace of the page's sound.
                let Some(speakers) = inner.speakers() else {
                    return Err("This browser gave the page no sound, in or out.".to_owned());
                };
                inner.asked += 1;
                let asked = inner.asked;
                inner.ear = Device::Asked(asked);
                let page = self.page.this.clone();
                spawn_local(async move {
                    let answer = Microphone::open(speakers, |_| {}).await;
                    if let Some(page) = page.upgrade() {
                        page.ear_answered(asked, answer);
                    }
                });
                Ok(None)
            }
            Device::Asked(asked) => {
                inner.ear = Device::Asked(asked);
                Ok(None)
            }
            Device::On(microphone) => match microphone.stopped() {
                Some(why) => Err(why.to_owned()),
                None => {
                    let level = microphone.level();
                    inner.ear = Device::On(microphone);
                    Ok(Some(level))
                }
            },
            Device::Refused(why) => Err(why),
        }
    }

    /// Passes what the person did on to the PC being watched, in order.
    pub fn input(&self, inputs: Vec<Input>) {
        if inputs.is_empty() {
            return;
        }
        if let Some(speakers) = &self.page.inner.borrow().speakers {
            // The person is clicking or typing: the moment a browser that
            // held the sound back lets it through.
            speaker::wake(speakers);
        }
        self.page.say(&Up::Input(inputs));
    }

    /// The size of the stream's pictures, when it is news to the app. The
    /// pictures themselves are on the canvas behind the window.
    pub fn take_picture(&self) -> Option<Picture> {
        let mut inner = self.page.inner.borrow_mut();
        let stream = inner.stream.as_mut()?;
        let size = stream.screen.as_ref()?.size()?;
        (stream.told.replace(size) != Some(size)).then_some(Picture::Behind(size))
    }

    /// Shows the pictures at `place`, in the app's points, or hides them.
    pub fn place_picture(&self, place: Option<egui::Rect>) {
        let inner = self.page.inner.borrow();
        let Some(canvas) = &inner.canvas else { return };
        // The page counts in CSS pixels, which are the app's points until
        // the person zooms the app.
        let zoom = self.page.ctx.zoom_factor();
        canvas.place(place.map(|place| {
            [place.left(), place.top(), place.width(), place.height()].map(|length| length * zoom)
        }));
    }

    /// Why the stream has no sound, when it has none.
    pub fn sound_problem(&self) -> Option<String> {
        let inner = self.page.inner.borrow();
        let stream = inner.stream.as_ref()?;
        stream.silent.clone().or_else(|| stream.speaker.as_ref()?.problem())
    }
}

impl Inner {
    /// The page's sound, made the first time and woken every time. Asked
    /// for while the browser knows the person just asked for something:
    /// that is when it gives a page its speakers.
    fn speakers(&mut self) -> Option<AudioContext> {
        if self.speakers.is_none() {
            match speaker::speakers(SOUND_RATE) {
                Ok(speakers) => self.speakers = Some(speakers),
                Err(error) => log::warn!("the browser gave no speakers: {error}"),
            }
        }
        let speakers = self.speakers.clone()?;
        speaker::wake(&speakers);
        Some(speakers)
    }

    fn start_stream(&mut self, attempt: u64) {
        self.end_stream();
        self.speakers();
        self.stream = Some(Stream {
            attempt,
            screen: None,
            speaker: None,
            silent: None,
            told: None,
            gamepads: None,
            listens: false,
            voice: Device::Off,
        });
    }

    fn end_stream(&mut self) {
        self.stream = None;
        if let Some(canvas) = &self.canvas {
            // The next stream does not start with the end of this one.
            canvas.place(None);
            canvas.clear();
        }
    }

    /// The canvas the pictures are drawn on.
    fn canvas(&mut self) -> Result<Rc<Canvas>, String> {
        if let Some(canvas) = &self.canvas {
            return Ok(Rc::clone(canvas));
        }
        let canvas = Rc::new(Canvas::behind_the_window()?);
        self.canvas = Some(Rc::clone(&canvas));
        Ok(canvas)
    }
}

impl Page {
    /// Opens a WebSocket to the bridge this page came from.
    fn connect(&self) {
        let socket = match address()
            .and_then(|address| WebSocket::new(&address).map_err(|error| codecs::words(&error)))
        {
            Ok(socket) => socket,
            Err(error) => {
                log::error!("the bridge cannot be reached from this page: {error}");
                self.close(Unavailable {
                    title: "This page cannot reach a bridge",
                    explanation: "Leto reaches your PCs through the bridge that serves this \
                                  page, over a WebSocket. This browser did not let the page \
                                  open one."
                        .to_owned(),
                });
                return;
            }
        };
        socket.set_binary_type(BinaryType::Arraybuffer);

        let page = self.this.clone();
        let opened = Closure::new(move || {
            if let Some(page) = page.upgrade() {
                page.say(&Up::Hello { key: stored_key() });
            }
        });
        let page = self.this.clone();
        let heard = Closure::new(move |message: MessageEvent| {
            if let Some(page) = page.upgrade() {
                page.heard(&message);
            }
        });
        let page = self.this.clone();
        let closed = Closure::new(move || {
            if let Some(page) = page.upgrade() {
                page.lost();
            }
        });
        socket.set_onopen(Some(opened.as_ref().unchecked_ref()));
        socket.set_onmessage(Some(heard.as_ref().unchecked_ref()));
        // An error closes the socket too: this hears of both.
        socket.set_onclose(Some(closed.as_ref().unchecked_ref()));
        self.inner.borrow_mut().line =
            Some(Line { socket, _opened: opened, _heard: heard, _closed: closed });
    }

    /// The bridge is gone, or never answered: try again in a moment.
    fn lost(&self) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.line = None;
            inner.end_stream();
            if matches!(inner.door, Door::Closed(_)) {
                return;
            }
            inner.door = Door::Opening { failed: true };
        }
        self.ctx.request_repaint();

        let page = self.this.clone();
        let again = Closure::once_into_js(move || {
            if let Some(page) = page.upgrade() {
                page.connect();
            }
        });
        if let Some(window) = web_sys::window() {
            // A browser that cannot set a timer has other worries.
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                again.unchecked_ref(),
                TRY_AGAIN_AFTER,
            );
        }
    }

    /// Nothing more can come of this page.
    fn close(&self, problem: Unavailable) {
        let mut inner = self.inner.borrow_mut();
        inner.line = None;
        inner.end_stream();
        inner.door = Door::Closed(problem);
        self.ctx.request_repaint();
    }

    /// Says `message` to the bridge, if it is listening.
    fn say(&self, message: &Up) {
        let inner = self.inner.borrow();
        let Some(line) = &inner.line else { return };
        if line.socket.ready_state() != WebSocket::OPEN {
            return;
        }
        match serde_json::to_string(message) {
            // A socket that just closed says so itself, in a moment.
            Ok(text) => drop(line.socket.send_with_str(&text)),
            Err(error) => log::error!("a message could not be written down: {error}"),
        }
    }

    /// Something arrived from the bridge.
    fn heard(&self, message: &MessageEvent) {
        let data = message.data();
        let then = if let Some(text) = data.as_string() {
            match serde_json::from_str::<Down>(&text) {
                Ok(said) => self.told(said),
                Err(error) => {
                    log::error!("the bridge said something this page does not understand: {error}");
                    self.close(Unavailable {
                        title: "This page and its bridge do not match",
                        explanation: "They are of different versions of Leto. Load the page \
                                      again. If that changes nothing, the bridge serves an old \
                                      web client: build it again with `cargo xtask web`."
                            .to_owned(),
                    });
                    Then::Nothing
                }
            }
        } else if let Ok(buffer) = data.dyn_into::<ArrayBuffer>() {
            self.given(&Uint8Array::new(&buffer).to_vec())
        } else {
            Then::Nothing
        };

        match then {
            Then::Nothing => {}
            Then::Say(message) => self.say(&message),
            Then::GiveUp { attempt, message } => {
                self.say(&Up::Ask(Request::Leave));
                let mut inner = self.inner.borrow_mut();
                inner.end_stream();
                inner.events.push(Event::PlayFailed { attempt, message });
                self.ctx.request_repaint();
            }
        }
    }

    /// The bridge said `said`.
    fn told(&self, said: Down) -> Then {
        let mut inner = self.inner.borrow_mut();
        match said {
            Down::CodeNeeded { refused } => {
                // Whatever key this browser had, the bridge has forgotten.
                forget_key();
                inner.door = Door::Code { refused, checking: false };
            }
            Down::Welcome { key, name } => {
                if let Some(key) = key {
                    store_key(&key);
                }
                log::info!("reaching PCs through the bridge on {name}");
                inner.door = Door::Open;
            }
            Down::News(event) => {
                if let engine::Event::PlayFailed { attempt, .. }
                | engine::Event::StreamEnded { attempt, .. } = &event
                    && inner.stream.as_ref().is_some_and(|stream| stream.attempt == *attempt)
                {
                    inner.end_stream();
                }
                if let engine::Event::Playing { attempt, microphone, .. } = &event
                    && let Some(stream) = &mut inner.stream
                    && stream.attempt == *attempt
                {
                    stream.listens = *microphone;
                    // The PC takes input from now on.
                    let page = self.this.clone();
                    stream.gamepads = Some(Gamepads::watch(move || {
                        if let Some(page) = page.upgrade() {
                            page.gamepads();
                        }
                    }));
                }
                inner.events.push(event.with_covers(|file| decode_cover(&file.0)));
            }
            Down::Pictures(format) => {
                let Some(attempt) = inner.stream.as_ref().map(|stream| stream.attempt) else {
                    // The pictures of a stream that was left.
                    return Then::Nothing;
                };
                let screen =
                    inner.canvas().and_then(|canvas| Screen::new(&format, &canvas, &self.ctx));
                match (screen, &mut inner.stream) {
                    (Ok(screen), Some(stream)) => stream.screen = Some(screen),
                    (Err(message), _) => return Then::GiveUp { attempt, message },
                    (Ok(_), None) => {}
                }
            }
            Down::Sound(format) => {
                let inner = &mut *inner;
                let Some(stream) = &mut inner.stream else { return Then::Nothing };
                let speaker = format.and_then(|format| match &inner.speakers {
                    Some(speakers) => Speaker::new(&format, speakers),
                    None => Err("This browser gave the page no speakers.".to_owned()),
                });
                match speaker {
                    Ok(speaker) => stream.speaker = Some(speaker),
                    Err(why) => stream.silent = Some(why),
                }
            }
        }
        self.ctx.request_repaint();
        Then::Nothing
    }

    /// The browser answered the `asked`-th request for the microphone.
    fn microphone_answered(&self, asked: u32, answer: Result<Microphone, String>) {
        let mut inner = self.inner.borrow_mut();
        if let Some(stream) = &mut inner.stream
            && matches!(stream.voice, Device::Asked(waiting) if waiting == asked)
        {
            stream.voice = match answer {
                Ok(microphone) => Device::On(microphone),
                Err(why) => Device::Refused(why),
            };
            self.ctx.request_repaint();
        }
        // Otherwise nobody waits for this microphone any more: the switch
        // went off or the stream ended while the browser was asking. It is
        // let go of here, unused.
    }

    /// The browser answered the `asked`-th request, which was for the
    /// camera the person tries out.
    fn camera_answered(&self, asked: u32, answer: Result<Camera, String>) {
        let mut inner = self.inner.borrow_mut();
        if matches!(inner.camera, Device::Asked(waiting) if waiting == asked) {
            inner.camera = match answer {
                Ok(camera) => Device::On(camera),
                Err(why) => Device::Refused(why),
            };
            self.ctx.request_repaint();
        }
        // Otherwise the person turned it off, or left, while the browser
        // was asking: the camera is let go of here, unused.
    }

    /// The same for the microphone the person tries out.
    fn ear_answered(&self, asked: u32, answer: Result<Microphone, String>) {
        let mut inner = self.inner.borrow_mut();
        if matches!(inner.ear, Device::Asked(waiting) if waiting == asked) {
            inner.ear = match answer {
                Ok(microphone) => Device::On(microphone),
                Err(why) => Device::Refused(why),
            };
            self.ctx.request_repaint();
        }
    }

    /// Sends a packet of the microphone to the bridge, for the PC.
    fn voice(&self, packet: &[u8]) {
        let inner = self.inner.borrow();
        let Some(line) = &inner.line else { return };
        if line.socket.ready_state() == WebSocket::OPEN {
            // A socket that just closed says so itself, in a moment.
            let _ = line.socket.send_with_u8_array(&Frame::Voice(packet).encode());
        }
    }

    /// Looks at the gamepads and tells the PC what they did.
    fn gamepads(&self) {
        let inputs = {
            let mut inner = self.inner.borrow_mut();
            let Some(gamepads) = inner.stream.as_mut().and_then(|stream| stream.gamepads.as_mut())
            else {
                return;
            };
            gamepads.look()
        };
        if !inputs.is_empty() {
            self.say(&Up::Input(inputs));
        }
    }

    /// The bridge handed over a piece of the stream.
    fn given(&self, message: &[u8]) -> Then {
        let mut inner = self.inner.borrow_mut();
        let Some(stream) = &mut inner.stream else { return Then::Nothing };
        match Frame::parse(message) {
            Some(Frame::Picture { keyframe, data, .. }) => {
                let Some(screen) = &mut stream.screen else { return Then::Nothing };
                match screen.show(keyframe, data) {
                    Shown::Yes => Then::Nothing,
                    Shown::KeyframeNeeded => Then::Say(Up::KeyframeNeeded),
                    Shown::Never(message) => Then::GiveUp { attempt: stream.attempt, message },
                }
            }
            Some(Frame::Sound(packet)) => {
                if let Some(speaker) = &mut stream.speaker {
                    speaker.play(packet);
                }
                Then::Nothing
            }
            // A microphone goes up, never down.
            Some(Frame::Voice(_)) | None => Then::Nothing,
        }
    }
}

/// Where the bridge listens for this page: where the page came from.
fn address() -> Result<String, String> {
    let location = web_sys::window().ok_or("no window")?.location();
    let host = location.host().map_err(|error| codecs::words(&error))?;
    let secure = location.protocol().is_ok_and(|protocol| protocol == "https:");
    Ok(format!("{}://{host}{PATH}", if secure { "wss" } else { "ws" }))
}

/// The browser's own storage. Browsers can be told to offer none.
fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

fn stored_key() -> Option<String> {
    storage()?.get_item(KEY).ok().flatten()
}

fn store_key(key: &str) {
    // Without storage the code is asked for again next time: no more.
    let stored = storage().map(|storage| storage.set_item(KEY, key));
    if !matches!(stored, Some(Ok(()))) {
        log::warn!("this browser keeps nothing: the bridge will ask for its code again");
    }
}

fn forget_key() {
    if let Some(storage) = storage() {
        let _ = storage.remove_item(KEY);
    }
}
