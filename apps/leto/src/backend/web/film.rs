//! The camera of a browser, on its way to the PC being watched.
//!
//! The browser plays its camera in a `<video>` (see `camera`). Some thirty
//! times a second, what that shows is handed to the browser's own H.264
//! encoder, and each encoded picture goes to whoever opened this: the page,
//! which sends it to its bridge, which passes it on to the PC.
//!
//! A timer of the page sets the pace. Browsers slow the timers of a tab
//! nobody looks at: from there, a PC gets a picture a second at most.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use js_sys::{Reflect, Uint8Array};
use leto_core::{
    bridge::LONGEST_CAMERA_PICTURE,
    engine::{CameraOffer, CameraSettings},
};
use wasm_bindgen::{JsCast as _, JsValue, closure::Closure};
use wasm_bindgen_futures::JsFuture;

use super::{
    camera::{Camera, HAS_A_PICTURE, RATE},
    codecs::{self, EncodedVideoChunk, VideoEncoder, VideoFrame, object, words},
};
use crate::backend::flow::{Shutter, camera_codec};

const NOT_ENCODED: &str = "The browser could not encode the camera.";
const NO_TIMER: &str = "The browser gave the page no timer to take pictures by.";

/// What the callbacks share.
struct Shared {
    shutter: RefCell<Shutter>,
    /// Why nothing is encoded any more, when the encoder gave up.
    failed: Cell<Option<&'static str>>,
}

/// Captures the browser's camera and encodes it for a PC.
///
/// Nothing is encoded before [`Self::start`]. Capturing stops when this is
/// dropped, and the browser's sign that a page is watching goes out.
pub struct Film {
    camera: Camera,
    encoder: VideoEncoder,
    sending: CameraSettings,
    shared: Rc<Shared>,
    /// The timer that takes the pictures, once started.
    timer: Cell<Option<i32>>,
    take: Closure<dyn FnMut()>,
    _encoded: Closure<dyn FnMut(EncodedVideoChunk)>,
    _failed: Closure<dyn FnMut(JsValue)>,
}

impl Film {
    /// Asks the browser for the camera, which may ask the person, and gets
    /// ready to encode it the way a PC that made `offer` takes it.
    ///
    /// Once started, `picture` is handed every picture: when it was taken,
    /// in milliseconds since the first, whether it decodes on its own, and
    /// the H.264 of it with start codes, as
    /// [`leto_core::bridge::Frame::Camera`] carries them. `room` says
    /// whether the way they leave by has room for more: while it has none,
    /// no picture is taken.
    pub async fn open(
        offer: CameraOffer,
        room: impl Fn() -> bool + 'static,
        mut picture: impl FnMut(u32, bool, &[u8]) + 'static,
    ) -> Result<Self, String> {
        // Before the person is asked for a camera that would be of no use.
        if !codecs::has("VideoEncoder") {
            return Err("This browser has no picture encoder that a page may use.".to_owned());
        }
        // From here on, leaving this function lets go of the camera.
        let camera = Camera::open().await?;
        let (width, height) = camera.size().ok_or("The camera gave no picture.")?;
        let sending = offer.fit(width as usize, height as usize, RATE).ok_or_else(|| {
            format!("The pictures of the camera, {width} by {height}, cannot be sent as they are.")
        })?;

        let format = object(&[
            ("codec", camera_codec(sending.width, sending.height, sending.fps).into()),
            // The browser scales what the camera gives down to this.
            ("width", sending.width.into()),
            ("height", sending.height.into()),
            ("bitrate", (sending.bitrate_kbps * 1000).into()),
            ("framerate", sending.fps.into()),
            // Each picture leaves as soon as it is made: none is held back
            // for the sake of the next.
            ("latencyMode", "realtime".into()),
            // With start codes and, in the pictures that decode on their
            // own, what a decoder has to know first: what a PC takes.
            ("avc", object(&[("format", "annexb".into())]).into()),
        ]);
        let cannot = |error: JsValue| {
            format!(
                "This browser cannot encode the camera: {}.",
                words(&error).trim_end_matches('.')
            )
        };
        let answer = JsFuture::from(VideoEncoder::supports(&format).map_err(cannot)?)
            .await
            .map_err(cannot)?;
        let supported = Reflect::get(&answer, &JsValue::from_str("supported")).ok();
        if supported.and_then(|supported| supported.as_bool()) != Some(true) {
            return Err("This browser cannot encode the camera the way a PC takes it.".to_owned());
        }

        let shared = Rc::new(Shared { shutter: RefCell::default(), failed: Cell::new(None) });
        let encoded = {
            let shared = Rc::clone(&shared);
            Closure::new(move |chunk: EncodedVideoChunk| {
                let length = chunk.length();
                let copied = (length > 0 && length as usize <= LONGEST_CAMERA_PICTURE)
                    .then(|| Uint8Array::new_with_length(length))
                    .filter(|bytes| chunk.copy_to(bytes).is_ok());
                let Some(bytes) = copied else {
                    // Left out, and the following ones build on it.
                    shared.shutter.borrow_mut().lose();
                    return;
                };
                // In milliseconds, in a number that starts over after seven
                // weeks.
                let at = (chunk.timestamp() / 1000.0) as u64 as u32;
                picture(at, chunk.kind() == "key", &bytes.to_vec());
            })
        };
        let failed = {
            let shared = Rc::clone(&shared);
            Closure::new(move |error: JsValue| {
                log::warn!("the browser could not encode the camera: {}", words(&error));
                shared.failed.set(Some(NOT_ENCODED));
            })
        };
        let callbacks =
            object(&[("output", encoded.as_ref().clone()), ("error", failed.as_ref().clone())]);
        let encoder = VideoEncoder::new(&callbacks)
            .and_then(|encoder| encoder.configure(&format).map(|()| encoder))
            .map_err(cannot)?;

        let take = {
            let (video, encoder, shared) =
                (camera.video().clone(), encoder.clone(), Rc::clone(&shared));
            Closure::new(move || {
                if shared.failed.get().is_some() || video.ready_state() < HAS_A_PICTURE {
                    return;
                }
                let Some(now) = now() else { return };
                let taken = shared.shutter.borrow_mut().take(now, encoder.waiting(), room());
                let Some(taken) = taken else { return };
                // In microseconds, whole ones.
                let when = object(&[("timestamp", (taken.at * 1000.0).round().into())]);
                let how = object(&[("keyFrame", taken.keyframe.into())]);
                let given = VideoFrame::shown_by(&video, &when).and_then(|frame| {
                    let given = encoder.encode(&frame, &how);
                    frame.close();
                    given
                });
                if let Err(error) = given {
                    log::warn!(
                        "the browser did not take a picture of the camera: {}",
                        words(&error)
                    );
                    shared.failed.set(Some(NOT_ENCODED));
                }
            })
        };

        Ok(Self {
            camera,
            encoder,
            sending,
            shared,
            timer: Cell::new(None),
            take,
            _encoded: encoded,
            _failed: failed,
        })
    }

    /// What the PC is to be told about the pictures that will leave.
    pub fn sending(&self) -> CameraSettings {
        self.sending
    }

    /// Starts taking pictures, at the pace the PC was told.
    pub fn start(&self) -> Result<(), String> {
        if self.started() {
            return Ok(());
        }
        let every = (1000 / self.sending.fps.max(1)) as i32;
        let timer = web_sys::window()
            .and_then(|window| {
                window
                    .set_interval_with_callback_and_timeout_and_arguments_0(
                        self.take.as_ref().unchecked_ref(),
                        every,
                    )
                    .ok()
            })
            .ok_or(NO_TIMER)?;
        self.timer.set(Some(timer));
        Ok(())
    }

    /// Whether pictures are being taken.
    pub fn started(&self) -> bool {
        self.timer.get().is_some()
    }

    /// Why no more pictures will leave, in words for the person, when the
    /// camera or the encoder stopped on its own.
    pub fn stopped(&self) -> Option<&'static str> {
        self.camera.stopped().or(self.shared.failed.get())
    }
}

impl Drop for Film {
    fn drop(&mut self) {
        if let (Some(timer), Some(window)) = (self.timer.take(), web_sys::window()) {
            window.clear_interval_with_handle(timer);
        }
        // Closing an encoder that closed itself is an error, and no news.
        let _ = self.encoder.close();
    }
}

/// The page's clock, in milliseconds.
fn now() -> Option<f64> {
    Some(web_sys::window()?.performance()?.now())
}
