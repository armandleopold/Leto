//! The pictures of a stream in a browser.
//!
//! The browser decodes them, and they are drawn on a canvas of their own
//! that sits behind the canvas the app draws on: the app leaves a hole over
//! it. A picture goes from the decoder to the screen without passing
//! through the app, which need not draw anything for it to show.

use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
    time::Duration,
};

use js_sys::{Object, Uint8Array};
use leto_core::bridge::VideoFormat;
use wasm_bindgen::{JsCast as _, JsValue, closure::Closure};
use web_sys::HtmlCanvasElement;

use super::codecs::{self, EncodedVideoChunk, Surface, VideoDecoder, VideoFrame, object, words};
use crate::{
    backend::flow::{Pictures, Verdict},
    stats::Counts,
};

/// Failures in a row, with no picture in between, after which a decoder is
/// not given another chance.
const CHANCES: u32 = 3;

/// The canvas behind the window.
pub struct Canvas {
    element: HtmlCanvasElement,
    surface: Surface,
    /// Where it is on the page: left, top, width and height in CSS pixels.
    place: Cell<Option<[f32; 4]>>,
}

impl Canvas {
    /// Adds the canvas to the page, hidden, behind the app's.
    pub fn behind_the_window() -> Result<Self, String> {
        let missing = || "The page is not the one Leto came with.".to_owned();
        let document =
            web_sys::window().and_then(|window| window.document()).ok_or_else(missing)?;
        let window = document.get_element_by_id(crate::CANVAS_ID).ok_or_else(missing)?;
        let parent = window.parent_node().ok_or_else(missing)?;
        let element = document
            .create_element("canvas")
            .ok()
            .and_then(|element| element.dyn_into::<HtmlCanvasElement>().ok())
            .ok_or_else(missing)?;
        element.set_id("leto_picture");
        // Of two fixed elements, the one that comes first is behind.
        parent.insert_before(&element, Some(&window)).map_err(|error| words(&error))?;

        // Nothing shows through the pictures, and they need not wait for
        // the rest of the page to be drawn.
        let options = object(&[("alpha", false.into()), ("desynchronized", true.into())]);
        let surface = element
            .get_context_with_context_options("2d", &options)
            .ok()
            .flatten()
            .and_then(|surface| surface.dyn_into::<Surface>().ok())
            .ok_or("This browser cannot draw on the page.")?;
        let canvas = Self { element, surface, place: Cell::new(Some([0.0; 4])) };
        canvas.place(None);
        Ok(canvas)
    }

    /// Shows the canvas at `place`, or hides it.
    pub fn place(&self, place: Option<[f32; 4]>) {
        if self.place.replace(place) == place {
            return;
        }
        let style = match place {
            Some([left, top, width, height]) => format!(
                "position:fixed;left:{left}px;top:{top}px;width:{width}px;height:{height}px;\
                 background:#000"
            ),
            None => "display:none".to_owned(),
        };
        // Setting an attribute with a valid name cannot fail.
        let _ = self.element.set_attribute("style", &style);
    }

    /// Forgets the last picture.
    pub fn clear(&self) {
        let (width, height) = (self.element.width(), self.element.height());
        self.surface.clear(0.0, 0.0, f64::from(width), f64::from(height));
    }

    fn draw(&self, frame: &VideoFrame) {
        let (width, height) = (frame.width(), frame.height());
        if self.element.width() != width || self.element.height() != height {
            self.element.set_width(width);
            self.element.set_height(height);
        }
        if let Err(error) = self.surface.draw(frame, 0.0, 0.0) {
            log::debug!("a picture could not be drawn: {}", words(&error));
        }
    }
}

/// What the decoder's callbacks leave for the screen to find.
#[derive(Default)]
struct Status {
    /// Width and height of the pictures shown.
    size: Cell<Option<[usize; 2]>>,
    /// The decoder reported an error, and closed itself.
    failed: Cell<bool>,
    /// Failures since the last picture came out.
    failures: Cell<u32>,
    /// What happened to the pictures so far.
    counts: Cell<Counts>,
    /// When each picture still in the decoder was given to it, by its
    /// timestamp, in seconds.
    given_at: RefCell<VecDeque<(f64, f64)>>,
}

impl Status {
    /// How long the picture given with `timestamp` took to come out of the
    /// decoder, when its giving was noted.
    fn took(&self, timestamp: f64) -> Option<Duration> {
        let mut given_at = self.given_at.borrow_mut();
        // Pictures come out in the order they went in.
        while let Some(&(given, at)) = given_at.front() {
            given_at.pop_front();
            if given == timestamp {
                return Some(Duration::from_secs_f64((now() - at).max(0.0)));
            }
        }
        None
    }
}

/// What became of a picture given to the screen.
pub enum Shown {
    /// It was shown, or will be.
    Yes,
    /// It could not be, and nothing can until the bridge sends a keyframe.
    KeyframeNeeded,
    /// It could not be, and nothing ever will: why, for the person.
    Never(String),
}

/// Decodes the pictures of one stream and shows them on the canvas.
pub struct Screen {
    decoder: VideoDecoder,
    callbacks: Object,
    format: Object,
    /// The format, for the person.
    codec: String,
    rule: Pictures,
    status: Rc<Status>,
    /// Pictures given to the decoder so far.
    given: u32,
    /// Microseconds between two pictures.
    step: f64,
    _output: Closure<dyn FnMut(VideoFrame)>,
    _error: Closure<dyn FnMut(JsValue)>,
}

impl Screen {
    pub fn new(
        format: &VideoFormat,
        canvas: &Rc<Canvas>,
        ctx: &egui::Context,
    ) -> Result<Self, String> {
        let status = Rc::new(Status::default());

        let (shown, canvas, ctx) = (Rc::clone(&status), Rc::clone(canvas), ctx.clone());
        let output = Closure::new(move |frame: VideoFrame| {
            canvas.draw(&frame);
            let size = [frame.width() as usize, frame.height() as usize];
            let took = shown.took(frame.timestamp());
            frame.close();
            let mut counts = shown.counts.get();
            counts.decoded(took);
            // Drawn on the canvas as it comes out.
            *counts.shown.get_or_insert(0) += 1;
            shown.counts.set(counts);
            shown.failures.set(0);
            if shown.size.replace(Some(size)) != Some(size) {
                // The app makes room for pictures of this size.
                ctx.request_repaint();
            }
        });
        let failed = Rc::clone(&status);
        let error = Closure::new(move |error: JsValue| {
            log::warn!("the browser could not decode a picture: {}", words(&error));
            failed.failed.set(true);
        });
        let callbacks =
            object(&[("output", output.as_ref().clone()), ("error", error.as_ref().clone())]);

        // Leto asks PCs for BT.601 in limited range. A stream that says
        // what it is itself is believed over this.
        let colors = object(&[
            ("primaries", "smpte170m".into()),
            ("transfer", "smpte170m".into()),
            ("matrix", "smpte170m".into()),
            ("fullRange", false.into()),
        ]);
        let config = object(&[
            ("codec", format.codec.as_str().into()),
            ("codedWidth", format.width.into()),
            ("codedHeight", format.height.into()),
            ("colorSpace", colors.into()),
            // Each picture comes out as soon as it is decoded.
            ("optimizeForLatency", true.into()),
        ]);

        let decoder = start(&callbacks, &config)
            .map_err(|error| cannot_decode(&format.codec, &words(&error)))?;
        Ok(Self {
            decoder,
            callbacks,
            format: config,
            codec: format.codec.clone(),
            rule: Pictures::new(),
            status,
            given: 0,
            step: 1_000_000.0 / f64::from(format.fps.max(1)),
            _output: output,
            _error: error,
        })
    }

    /// Width and height of the pictures on the canvas, once there is one.
    pub fn size(&self) -> Option<[usize; 2]> {
        self.status.size.get()
    }

    /// What happened to the pictures so far.
    pub fn counts(&self) -> Counts {
        self.status.counts.get()
    }

    /// Takes the next picture of the stream.
    pub fn show(&mut self, keyframe: bool, data: &[u8]) -> Shown {
        let mut counts = self.status.counts.get();
        // The bridge passes on what arrives, and says nothing of what did
        // not.
        counts.received(data.len(), None, None);
        self.status.counts.set(counts);
        if self.status.failed.replace(false) {
            if let Err(why) = self.start_over(true) {
                return Shown::Never(why);
            }
        } else if self.rule.behind(self.decoder.waiting(), now()) {
            // What waits in the decoder is the past: let go of it.
            log::debug!("the decoder is behind: skipping to a fresh keyframe");
            if let Err(why) = self.start_over(false) {
                return Shown::Never(why);
            }
        }
        match self.rule.judge(keyframe) {
            Verdict::Decode => {}
            Verdict::Skip { ask: true } => return Shown::KeyframeNeeded,
            Verdict::Skip { ask: false } => return Shown::Yes,
        }

        let timestamp = f64::from(self.given) * self.step;
        let picture = object(&[
            ("type", if keyframe { "key" } else { "delta" }.into()),
            ("timestamp", timestamp.into()),
            ("data", Uint8Array::from(data).into()),
        ]);
        let mut given_at = self.status.given_at.borrow_mut();
        // A decoder that loses pictures leaves their times behind: a few
        // seconds of them are plenty.
        if given_at.len() >= 240 {
            given_at.pop_front();
        }
        given_at.push_back((timestamp, now()));
        drop(given_at);
        self.given = self.given.wrapping_add(1);
        let given = EncodedVideoChunk::new(&picture).and_then(|chunk| self.decoder.decode(&chunk));
        if let Err(error) = given {
            log::warn!("the browser did not take a picture: {}", words(&error));
            self.status.failed.set(true);
        }
        Shown::Yes
    }

    /// Replaces the decoder by one that knows nothing. `failure` when the
    /// old one gave up, as opposed to being let go of.
    fn start_over(&mut self, failure: bool) -> Result<(), String> {
        if failure {
            let failures = self.status.failures.get() + 1;
            self.status.failures.set(failures);
            if failures >= CHANCES {
                return Err(cannot_decode(&self.codec, "it failed on every picture"));
            }
        }
        // Closing a decoder that closed itself is an error, and no news.
        let _ = self.decoder.close();
        self.decoder = start(&self.callbacks, &self.format)
            .map_err(|error| cannot_decode(&self.codec, &words(&error)))?;
        self.rule.lose();
        Ok(())
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let _ = self.decoder.close();
    }
}

/// Seconds on a clock that only goes forward.
fn now() -> f64 {
    let clock = web_sys::window().and_then(|window| window.performance());
    clock.map_or(0.0, |clock| clock.now() / 1000.0)
}

fn start(callbacks: &Object, format: &Object) -> Result<VideoDecoder, JsValue> {
    let decoder = VideoDecoder::new(callbacks)?;
    decoder.configure(format)?;
    Ok(decoder)
}

fn cannot_decode(codec: &str, why: &str) -> String {
    format!("This browser cannot decode the pictures of the stream ({codec}): {why}.")
}

/// Why no stream can be shown at all, when that is so.
pub fn missing() -> Option<String> {
    (!codecs::has("VideoDecoder")).then(|| {
        "This browser cannot show a stream: it has no video decoder that a page may use. A \
         recent Chrome, Edge, Firefox or Safari has one, and so does the desktop app."
            .to_owned()
    })
}
