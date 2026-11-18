//! The browser's own decoders, and its encoder for the microphone, as far
//! as Leto uses them.
//!
//! This is WebCodecs. `web-sys` only offers it behind a flag that makes its
//! API unstable, for the whole build, so the few calls Leto makes are
//! declared here.

use js_sys::{Float32Array, Object, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    /// Turns encoded pictures into [`VideoFrame`]s.
    pub type VideoDecoder;

    /// `callbacks` has `output`, called with each [`VideoFrame`], and
    /// `error`.
    #[wasm_bindgen(constructor, catch)]
    pub fn new(callbacks: &Object) -> Result<VideoDecoder, JsValue>;

    #[wasm_bindgen(method, catch)]
    pub fn configure(this: &VideoDecoder, format: &Object) -> Result<(), JsValue>;

    #[wasm_bindgen(method, catch)]
    pub fn decode(this: &VideoDecoder, picture: &EncodedVideoChunk) -> Result<(), JsValue>;

    /// Forgets the pictures it was given and its format.
    #[wasm_bindgen(method, catch)]
    pub fn reset(this: &VideoDecoder) -> Result<(), JsValue>;

    #[wasm_bindgen(method, catch)]
    pub fn close(this: &VideoDecoder) -> Result<(), JsValue>;

    /// The pictures it was given and has not started on.
    #[wasm_bindgen(method, getter, js_name = decodeQueueSize)]
    pub fn waiting(this: &VideoDecoder) -> u32;

    /// One encoded picture.
    pub type EncodedVideoChunk;

    #[wasm_bindgen(constructor, catch)]
    pub fn new(picture: &Object) -> Result<EncodedVideoChunk, JsValue>;

    /// One decoded picture. It holds memory of the decoder's until closed.
    pub type VideoFrame;

    #[wasm_bindgen(method, getter, js_name = displayWidth)]
    pub fn width(this: &VideoFrame) -> u32;

    #[wasm_bindgen(method, getter, js_name = displayHeight)]
    pub fn height(this: &VideoFrame) -> u32;

    #[wasm_bindgen(method)]
    pub fn close(this: &VideoFrame);

    /// What a canvas is drawn on with: `CanvasRenderingContext2D`, as far
    /// as the pictures of a stream need it.
    #[wasm_bindgen(js_name = CanvasRenderingContext2D)]
    pub type Surface;

    /// Draws a decoded picture at its own size.
    #[wasm_bindgen(method, js_name = drawImage, catch)]
    pub fn draw(this: &Surface, frame: &VideoFrame, x: f64, y: f64) -> Result<(), JsValue>;

    /// Empties a rectangle.
    #[wasm_bindgen(method, js_name = clearRect)]
    pub fn clear(this: &Surface, x: f64, y: f64, width: f64, height: f64);

    /// Turns encoded sound into [`AudioData`].
    pub type AudioDecoder;

    #[wasm_bindgen(constructor, catch)]
    pub fn new(callbacks: &Object) -> Result<AudioDecoder, JsValue>;

    #[wasm_bindgen(method, catch)]
    pub fn configure(this: &AudioDecoder, format: &Object) -> Result<(), JsValue>;

    #[wasm_bindgen(method, catch)]
    pub fn decode(this: &AudioDecoder, packet: &EncodedAudioChunk) -> Result<(), JsValue>;

    #[wasm_bindgen(method, catch)]
    pub fn close(this: &AudioDecoder) -> Result<(), JsValue>;

    /// One encoded piece of sound.
    pub type EncodedAudioChunk;

    #[wasm_bindgen(constructor, catch)]
    pub fn new(packet: &Object) -> Result<EncodedAudioChunk, JsValue>;

    /// Its size in bytes.
    #[wasm_bindgen(method, getter, js_name = byteLength)]
    pub fn length(this: &EncodedAudioChunk) -> u32;

    /// Copies it out. `bytes` must have room for all of it.
    #[wasm_bindgen(method, js_name = copyTo, catch)]
    pub fn copy_to(this: &EncodedAudioChunk, bytes: &Uint8Array) -> Result<(), JsValue>;

    /// Turns sound into encoded pieces.
    #[derive(Clone)]
    pub type AudioEncoder;

    /// `callbacks` has `output`, called with each [`EncodedAudioChunk`], and
    /// `error`.
    #[wasm_bindgen(constructor, catch)]
    pub fn new(callbacks: &Object) -> Result<AudioEncoder, JsValue>;

    #[wasm_bindgen(method, catch)]
    pub fn configure(this: &AudioEncoder, format: &Object) -> Result<(), JsValue>;

    #[wasm_bindgen(method, catch)]
    pub fn encode(this: &AudioEncoder, sound: &AudioData) -> Result<(), JsValue>;

    #[wasm_bindgen(method, catch)]
    pub fn close(this: &AudioEncoder) -> Result<(), JsValue>;

    /// One piece of sound that is not encoded: what a decoder makes, and
    /// what an encoder takes.
    pub type AudioData;

    /// `sound` has the samples and says what they are.
    #[wasm_bindgen(constructor, catch)]
    pub fn new(sound: &Object) -> Result<AudioData, JsValue>;

    /// Samples per channel.
    #[wasm_bindgen(method, getter, js_name = numberOfFrames)]
    pub fn frames(this: &AudioData) -> u32;

    #[wasm_bindgen(method, getter, js_name = numberOfChannels)]
    pub fn channels(this: &AudioData) -> u32;

    #[wasm_bindgen(method, getter, js_name = sampleRate)]
    pub fn sample_rate(this: &AudioData) -> f32;

    /// Copies one channel out, in the format `options` asks for.
    #[wasm_bindgen(method, js_name = copyTo, catch)]
    pub fn copy_to(
        this: &AudioData,
        samples: &Float32Array,
        options: &Object,
    ) -> Result<(), JsValue>;

    #[wasm_bindgen(method)]
    pub fn close(this: &AudioData);
}

/// Whether this browser has something called `name`, such as a decoder.
pub fn has(name: &str) -> bool {
    Reflect::has(&js_sys::global(), &JsValue::from_str(name)).unwrap_or(false)
}

/// A JavaScript object with these fields: what the browser's constructors
/// take their options in.
pub fn object(fields: &[(&str, JsValue)]) -> Object {
    let object = Object::new();
    for (name, value) in fields {
        // Setting a field of a plain object cannot fail.
        let _ = Reflect::set(&object, &JsValue::from_str(name), value);
    }
    object
}

/// What went wrong, in the browser's words.
pub fn words(error: &JsValue) -> String {
    let message = Reflect::get(error, &JsValue::from_str("message")).ok();
    message.and_then(|message| message.as_string()).unwrap_or_else(|| format!("{error:?}"))
}
