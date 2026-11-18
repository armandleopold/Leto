//! The camera of a browser, for the person to look at or for a PC.
//!
//! The browser plays what the camera sees in a `<video>` that nobody sees.
//! For the person, Leto copies it into its own window through a canvas,
//! some thirty times a second. For a PC, `film` encodes what it plays.

use std::{cell::Cell, rc::Rc};

use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    CanvasRenderingContext2d, HtmlCanvasElement, HtmlVideoElement, MediaStreamConstraints,
};

use super::{
    capture::{self, Kind, Tracks},
    codecs::object,
};
use crate::backend::Seen;

/// What Leto would like from a camera. The browser gives what is closest.
const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
pub const RATE: u32 = 30;

/// The longest side of the pictures handed to the window: more than it
/// shows them at, and little enough to copy many times a second.
const LONGEST: u32 = 960;

/// Milliseconds between two pictures handed to the window, at least.
const BETWEEN: f64 = 30.0;

/// What a `<video>` says once it has a picture to show.
pub const HAS_A_PICTURE: u16 = 2;

/// Captures the browser's camera.
///
/// Capturing stops when this is dropped, and the browser's sign that a page
/// is watching goes out.
pub struct Camera {
    _tracks: Tracks,
    video: HtmlVideoElement,
    canvas: HtmlCanvasElement,
    surface: CanvasRenderingContext2d,
    /// Why it no longer captures, when it stopped on its own.
    stopped: Rc<Cell<Option<&'static str>>>,
    /// When the window was last handed a picture, on the page's clock.
    handed: Cell<f64>,
}

impl Camera {
    /// Asks the browser for the camera, which may ask the person.
    pub async fn open() -> Result<Self, String> {
        let ideal = |value: u32| JsValue::from(object(&[("ideal", value.into())]));
        let picture = object(&[
            ("width", ideal(WIDTH)),
            ("height", ideal(HEIGHT)),
            ("frameRate", ideal(RATE)),
        ]);
        let wanted = MediaStreamConstraints::new();
        wanted.set_video(&picture);

        let stopped = Rc::new(Cell::new(None));
        // From here on, leaving this function lets go of the camera.
        let (tracks, stream) = capture::ask(Kind::Camera, &wanted, &stopped).await?;

        let not_opened = |error: JsValue| capture::not_opened(Kind::Camera, &error);
        let missing = || "The page is not the one Leto came with.".to_owned();
        let document =
            web_sys::window().and_then(|window| window.document()).ok_or_else(missing)?;
        let window = document.get_element_by_id(crate::CANVAS_ID).ok_or_else(missing)?;
        let parent = window.parent_node().ok_or_else(missing)?;
        let create = |tag: &str| document.create_element(tag).map_err(not_opened);

        let video: HtmlVideoElement = create("video")?.unchecked_into();
        video.set_muted(true);
        // On a phone it would otherwise take the whole screen.
        video.set_attribute("playsinline", "").map_err(not_opened)?;
        // On the page, because some browsers only play what is on it, and
        // where nobody sees it.
        let hidden = "position:fixed;left:0;top:0;width:1px;height:1px;opacity:0;\
                      pointer-events:none";
        video.set_attribute("style", hidden).map_err(not_opened)?;
        video.set_src_object(Some(&stream));
        parent.append_child(&video).map_err(not_opened)?;

        let canvas: HtmlCanvasElement = create("canvas")?.unchecked_into();
        // Its pixels are read all the time: the browser keeps them where
        // reading is fast.
        let options = object(&[("willReadFrequently", true.into())]);
        let surface = canvas
            .get_context_with_context_options("2d", &options)
            .ok()
            .flatten()
            .and_then(|surface| surface.dyn_into::<CanvasRenderingContext2d>().ok())
            .ok_or("This browser cannot draw on the page.")?;

        // Made before the video is asked to play: dropping it takes the
        // video off the page whatever happens next.
        let camera =
            Self { _tracks: tracks, video, canvas, surface, stopped, handed: Cell::new(0.0) };
        let playing = camera.video.play().map_err(not_opened)?;
        JsFuture::from(playing).await.map_err(not_opened)?;
        Ok(camera)
    }

    /// What the camera shows now, when the window was not handed a picture
    /// a moment ago already.
    pub fn picture(&self) -> Option<Seen> {
        let clock = web_sys::window().and_then(|window| window.performance())?;
        let now = clock.now();
        if now - self.handed.get() < BETWEEN {
            return None;
        }
        let (width, height) = (self.video.video_width(), self.video.video_height());
        if width == 0 || height == 0 || self.video.ready_state() < HAS_A_PICTURE {
            return None;
        }
        self.handed.set(now);

        let scale = (f64::from(LONGEST) / f64::from(width.max(height))).min(1.0);
        let scaled = |length: u32| ((f64::from(length) * scale).round() as u32).max(1);
        let (shown_width, shown_height) = (scaled(width), scaled(height));
        if (self.canvas.width(), self.canvas.height()) != (shown_width, shown_height) {
            self.canvas.set_width(shown_width);
            self.canvas.set_height(shown_height);
        }
        let (across, down) = (f64::from(shown_width), f64::from(shown_height));
        self.surface
            .draw_image_with_html_video_element_and_dw_and_dh(&self.video, 0.0, 0.0, across, down)
            .ok()?;
        let pixels = self.surface.get_image_data(0.0, 0.0, across, down).ok()?.data();
        let shown = [shown_width as usize, shown_height as usize];
        Some(Seen {
            picture: egui::ColorImage::from_rgba_unmultiplied(shown, &pixels),
            size: [width as usize, height as usize],
        })
    }

    /// Why the camera no longer captures, in words for the person, when it
    /// stopped on its own.
    pub fn stopped(&self) -> Option<&'static str> {
        self.stopped.get()
    }

    /// What plays the camera: its pictures can be taken from there.
    pub fn video(&self) -> &HtmlVideoElement {
        &self.video
    }

    /// The size the camera captures at, once it gave a picture.
    pub fn size(&self) -> Option<(u32, u32)> {
        let size = (self.video.video_width(), self.video.video_height());
        (size.0 > 0 && size.1 > 0).then_some(size)
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        let _ = self.video.pause();
        self.video.set_src_object(None);
        self.video.remove();
    }
}
