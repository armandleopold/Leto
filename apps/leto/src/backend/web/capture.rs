//! Asking the browser for the microphone or the camera of this device.

use std::{cell::Cell, rc::Rc};

use js_sys::Reflect;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::JsFuture;
use web_sys::{MediaStream, MediaStreamConstraints, MediaStreamTrack};

use super::codecs::words;

/// What a page asks the browser for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Microphone,
    Camera,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Microphone => "microphone",
            Self::Camera => "camera",
        }
    }

    /// What is said when the browser takes it back.
    fn stopped(self) -> &'static str {
        match self {
            Self::Microphone => "The microphone stopped. It may have been unplugged.",
            Self::Camera => "The camera stopped. It may have been unplugged.",
        }
    }
}

/// The tracks of a device the browser handed over. It captures until they
/// are stopped, which dropping this does: the browser's sign that a page
/// listens or watches goes out.
pub struct Tracks {
    tracks: Vec<MediaStreamTrack>,
    _ended: Closure<dyn FnMut()>,
}

impl Drop for Tracks {
    fn drop(&mut self) {
        for track in &self.tracks {
            track.set_onended(None);
            track.stop();
        }
    }
}

/// Asks the browser for what `wanted` describes, which may ask the person.
/// `stopped` hears of it if the browser takes it back.
pub async fn ask(
    kind: Kind,
    wanted: &MediaStreamConstraints,
    stopped: &Rc<Cell<Option<&'static str>>>,
) -> Result<(Tracks, MediaStream), String> {
    let name = kind.name();
    let nothing = || format!("This browser gives this page no {name}.");
    let devices = web_sys::window().ok_or_else(nothing)?.navigator().media_devices();
    // Missing on a page that did not come over HTTPS.
    let devices = devices.ok().filter(|devices| !devices.is_undefined()).ok_or_else(nothing)?;

    let asked =
        devices.get_user_media_with_constraints(wanted).map_err(|error| refused(kind, &error))?;
    let stream: MediaStream =
        JsFuture::from(asked).await.map_err(|error| refused(kind, &error))?.unchecked_into();

    let gone = Rc::clone(stopped);
    let ended = Closure::new(move || gone.set(Some(kind.stopped())));
    let tracks = match kind {
        Kind::Microphone => stream.get_audio_tracks(),
        Kind::Camera => stream.get_video_tracks(),
    };
    let tracks: Vec<MediaStreamTrack> = tracks.iter().map(JsCast::unchecked_into).collect();
    for track in &tracks {
        track.set_onended(Some(ended.as_ref().unchecked_ref()));
    }
    let tracks = Tracks { tracks, _ended: ended };
    if tracks.tracks.is_empty() {
        return Err(format!("This device has no {name}."));
    }
    Ok((tracks, stream))
}

/// Why the browser gave nothing, in words for the person.
fn refused(kind: Kind, error: &JsValue) -> String {
    let name = kind.name();
    let error_name =
        Reflect::get(error, &JsValue::from_str("name")).ok().and_then(|name| name.as_string());
    match error_name.as_deref() {
        Some("NotAllowedError" | "SecurityError") => format!(
            "The browser does not let this page use the {name}. Allow it in the browser's \
             settings for this page."
        ),
        Some("NotFoundError" | "OverconstrainedError") => format!("This device has no {name}."),
        Some("NotReadableError" | "AbortError") => {
            format!("The {name} could not be opened. Another app may be using it.")
        }
        _ => not_opened(kind, error),
    }
}

pub fn not_opened(kind: Kind, error: &JsValue) -> String {
    format!("The {} could not be opened: {}.", kind.name(), words(error).trim_end_matches('.'))
}
