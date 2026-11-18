//! The sound of a stream in a browser: decoded by the browser, then played
//! piece after piece on the page's speakers.

use std::{cell::Cell, rc::Rc};

use js_sys::{Float32Array, Uint8Array};
use leto_core::bridge::AudioFormat;
use wasm_bindgen::{JsValue, closure::Closure};
use web_sys::{AudioContext, AudioContextOptions, AudioContextState};

use super::codecs::{self, AudioData, AudioDecoder, EncodedAudioChunk, object, words};
use crate::backend::flow::Pace;

/// Leto plays stereo, like the desktop app.
const CHANNELS: u32 = 2;

/// The page's speakers, at the rate PCs send sound at.
///
/// A browser gives a page its speakers once the person has done something
/// on it. Asking for a stream is something: call this from there.
pub fn speakers(sample_rate: u32) -> Result<AudioContext, String> {
    let options = AudioContextOptions::new();
    options.set_sample_rate(sample_rate as f32);
    // As little waiting as the device allows: this sound belongs to a
    // picture.
    options.set_latency_hint(&JsValue::from_str("interactive"));
    AudioContext::new_with_context_options(&options).map_err(|error| words(&error))
}

/// Asks the browser for speakers it is holding back. It gives them when the
/// person has just clicked or pressed something.
pub fn wake(speakers: &AudioContext) {
    if speakers.state() == AudioContextState::Suspended {
        // The answer comes later, and shows in the state.
        let _ = speakers.resume();
    }
}

/// Decodes the sound of one stream and plays it.
pub struct Speaker {
    decoder: AudioDecoder,
    speakers: AudioContext,
    /// Packets given to the decoder so far.
    given: u64,
    /// Microseconds of sound in a packet.
    step: f64,
    failed: Rc<Cell<bool>>,
    _output: Closure<dyn FnMut(AudioData)>,
    _error: Closure<dyn FnMut(JsValue)>,
}

impl Speaker {
    pub fn new(format: &AudioFormat, speakers: &AudioContext) -> Result<Self, String> {
        if !codecs::has("AudioDecoder") {
            return Err("This browser has no sound decoder that a page may use.".to_owned());
        }
        let failed = Rc::new(Cell::new(false));

        let context = speakers.clone();
        let mut pace = Pace::new();
        let mut samples = Float32Array::new_with_length(0);
        let output = Closure::new(move |sound: AudioData| {
            if let Err(error) = play(&context, &mut pace, &mut samples, &sound) {
                log::debug!("a piece of sound could not be played: {}", words(&error));
            }
            sound.close();
        });
        let gave_up = Rc::clone(&failed);
        let error = Closure::new(move |error: JsValue| {
            log::warn!("the browser could not decode the sound: {}", words(&error));
            gave_up.set(true);
        });

        let callbacks =
            object(&[("output", output.as_ref().clone()), ("error", error.as_ref().clone())]);
        let config = object(&[
            ("codec", "opus".into()),
            ("sampleRate", format.sample_rate.into()),
            ("numberOfChannels", format.channels.into()),
        ]);
        let decoder = AudioDecoder::new(&callbacks)
            .and_then(|decoder| decoder.configure(&config).map(|()| decoder))
            .map_err(|error| format!("This browser cannot decode the sound: {}.", words(&error)))?;

        let step =
            1_000_000.0 * f64::from(format.samples_per_packet) / f64::from(format.sample_rate);
        Ok(Self {
            decoder,
            speakers: speakers.clone(),
            given: 0,
            step,
            failed,
            _output: output,
            _error: error,
        })
    }

    /// Takes the next packet of the stream.
    pub fn play(&mut self, packet: &[u8]) {
        if self.failed.get() {
            return;
        }
        let piece = object(&[
            // Every Opus packet decodes on its own.
            ("type", "key".into()),
            ("timestamp", (self.given as f64 * self.step).into()),
            ("duration", self.step.into()),
            ("data", Uint8Array::from(packet).into()),
        ]);
        self.given += 1;
        let given = EncodedAudioChunk::new(&piece).and_then(|chunk| self.decoder.decode(&chunk));
        if let Err(error) = given {
            log::warn!("the browser did not take a piece of sound: {}", words(&error));
            self.failed.set(true);
        }
    }

    /// Why nothing is heard, when that is so.
    pub fn problem(&self) -> Option<String> {
        if self.failed.get() {
            Some("The browser could not decode the sound of this stream.".to_owned())
        } else if self.speakers.state() == AudioContextState::Suspended {
            Some("The browser holds the sound back until you click on the picture.".to_owned())
        } else {
            None
        }
    }
}

impl Drop for Speaker {
    fn drop(&mut self) {
        // Closing a decoder that closed itself is an error, and no news.
        let _ = self.decoder.close();
    }
}

/// Plays one decoded piece on `speakers`, when `pace` says.
fn play(
    speakers: &AudioContext,
    pace: &mut Pace,
    samples: &mut Float32Array,
    sound: &AudioData,
) -> Result<(), JsValue> {
    let (frames, rate) = (sound.frames(), sound.sample_rate());
    if frames == 0 || rate <= 0.0 {
        return Ok(());
    }
    let length = f64::from(frames) / f64::from(rate);
    let Some(at) = pace.place(speakers.current_time(), length) else {
        // Too much is waiting to be heard already.
        return Ok(());
    };

    let buffer = speakers.create_buffer(CHANNELS, frames, rate)?;
    if samples.length() != frames {
        *samples = Float32Array::new_with_length(frames);
    }
    for channel in 0..CHANNELS {
        // Sound with one channel goes to both ears.
        let from = channel.min(sound.channels().saturating_sub(1));
        let plane = object(&[("planeIndex", from.into()), ("format", "f32-planar".into())]);
        sound.copy_to(samples, &plane)?;
        buffer.copy_to_channel_with_f32_array(samples, channel as i32)?;
    }
    let source = speakers.create_buffer_source()?;
    source.set_buffer(Some(&buffer));
    source.connect_with_audio_node(&speakers.destination())?;
    source.start_with_when(at)
}
