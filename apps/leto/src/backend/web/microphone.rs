//! The microphone of a browser, for the PC being watched.
//!
//! The browser captures it and brings it to the rate of the page's sound.
//! A small script on the browser's sound thread (`web/microphone.js`) cuts
//! it into pieces of twenty milliseconds, the browser's own encoder makes
//! an Opus packet of each, and the packets go to whoever opened it.

use std::{cell::Cell, rc::Rc};

use js_sys::{Array, Float32Array, Reflect, Uint8Array};
use leto_core::bridge::{VOICE_PACKET, VOICE_RATE};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    AudioContext, AudioWorkletNode, AudioWorkletNodeOptions, ChannelCountMode, MediaStream,
    MediaStreamAudioSourceNode, MediaStreamConstraints, MediaStreamTrack, MessageEvent,
};

use super::codecs::{self, AudioData, AudioEncoder, EncodedAudioChunk, object, words};

/// The script that gathers what the microphone captures, next to the page,
/// and the name it gives itself.
const GATHERER_FILE: &str = "microphone.js";
const GATHERER: &str = "leto-microphone";

/// Samples in one piece.
const PIECE: u32 = VOICE_RATE / 1000 * VOICE_PACKET.as_millis() as u32;

/// Bits per second, as in the desktop app: plenty for a voice.
const BIT_RATE: u32 = 48_000;

const STOPPED: &str = "The microphone stopped. It may have been unplugged.";
const NOT_ENCODED: &str = "The browser could not encode the microphone.";

/// Captures the browser's microphone and encodes it.
///
/// Capturing stops when this is dropped, and the browser's sign that a page
/// is listening goes out.
pub struct Microphone {
    _tracks: Tracks,
    source: MediaStreamAudioSourceNode,
    gatherer: AudioWorkletNode,
    encoder: AudioEncoder,
    level: Rc<Cell<f32>>,
    /// Why it no longer captures, when it stopped on its own.
    stopped: Rc<Cell<Option<&'static str>>>,
    gathered: Closure<dyn FnMut(MessageEvent)>,
    _encoded: Closure<dyn FnMut(EncodedAudioChunk)>,
    _failed: Closure<dyn FnMut(JsValue)>,
}

/// The tracks of a microphone the browser handed over. It captures until
/// they are stopped, which dropping this does.
struct Tracks {
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

impl Microphone {
    /// Asks the browser for the microphone, which may ask the person, and
    /// hands `packet` every packet of it from then on: Opus, one channel,
    /// as [`leto_core::bridge::Frame::Voice`] carries them.
    ///
    /// `speakers` is the page's sound, which the microphone joins.
    pub async fn open(
        speakers: AudioContext,
        mut packet: impl FnMut(&[u8]) + 'static,
    ) -> Result<Self, String> {
        if !codecs::has("AudioEncoder") {
            return Err("This browser has no sound encoder that a page may use.".to_owned());
        }
        if speakers.sample_rate() != VOICE_RATE as f32 {
            // The pieces would be of another length than a PC takes.
            return Err("This browser plays sound at a rate Leto cannot send.".to_owned());
        }

        let stopped = Rc::new(Cell::new(None));
        // From here on, leaving this function lets go of the microphone.
        let (tracks, stream) = capture(&stopped).await?;
        let source =
            speakers.create_media_stream_source(&stream).map_err(|error| not_opened(&error))?;

        let encoded = Closure::new(move |chunk: EncodedAudioChunk| {
            let bytes = Uint8Array::new_with_length(chunk.length());
            if chunk.copy_to(&bytes).is_ok() {
                packet(&bytes.to_vec());
            }
        });
        let gave_up = Rc::clone(&stopped);
        let failed = Closure::new(move |error: JsValue| {
            log::warn!("the browser could not encode the microphone: {}", words(&error));
            gave_up.set(Some(NOT_ENCODED));
        });
        let callbacks =
            object(&[("output", encoded.as_ref().clone()), ("error", failed.as_ref().clone())]);
        let format = object(&[
            ("codec", "opus".into()),
            ("sampleRate", VOICE_RATE.into()),
            ("numberOfChannels", 1.into()),
            ("bitrate", BIT_RATE.into()),
            // One packet for each piece, as it is: what a PC takes.
            (
                "opus",
                object(&[
                    ("format", "opus".into()),
                    ("frameDuration", (VOICE_PACKET.as_micros() as u32).into()),
                ])
                .into(),
            ),
        ]);
        let encoder = AudioEncoder::new(&callbacks)
            .and_then(|encoder| encoder.configure(&format).map(|()| encoder))
            .map_err(|error| {
                format!("This browser cannot encode the microphone: {}.", words(&error))
            })?;

        let options = AudioWorkletNodeOptions::new();
        options.set_number_of_inputs(1);
        options.set_number_of_outputs(1);
        options.set_output_channel_count(&Array::of1(&1.into()));
        // However many channels the microphone has, the browser mixes them
        // into one.
        options.set_channel_count(1);
        options.set_channel_count_mode(ChannelCountMode::Explicit);
        options.set_processor_options(Some(&object(&[("piece", PIECE.into())])));
        let gatherer = match AudioWorkletNode::new_with_options(&speakers, GATHERER, &options) {
            Ok(gatherer) => gatherer,
            // The page's sound does not know the script yet: it is loaded
            // the first time.
            Err(_) => {
                let loading = speakers
                    .audio_worklet()
                    .and_then(|worklet| worklet.add_module(GATHERER_FILE))
                    .map_err(|error| not_opened(&error))?;
                JsFuture::from(loading).await.map_err(|error| not_opened(&error))?;
                AudioWorkletNode::new_with_options(&speakers, GATHERER, &options)
                    .map_err(|error| not_opened(&error))?
            }
        };

        let level = Rc::new(Cell::new(0.0));
        let gathered = {
            let (encoder, level, stopped) =
                (encoder.clone(), Rc::clone(&level), Rc::clone(&stopped));
            let step = VOICE_PACKET.as_micros() as f64;
            let mut pieces = 0u32;
            Closure::new(move |message: MessageEvent| {
                let Ok(piece) = message.data().dyn_into::<Float32Array>() else { return };
                level.set(loudness(&piece.to_vec()));
                let sound = object(&[
                    ("format", "f32-planar".into()),
                    ("sampleRate", VOICE_RATE.into()),
                    ("numberOfChannels", 1.into()),
                    ("numberOfFrames", piece.length().into()),
                    ("timestamp", (f64::from(pieces) * step).into()),
                    ("data", piece.into()),
                ]);
                pieces = pieces.wrapping_add(1);
                let given = AudioData::new(&sound).and_then(|sound| {
                    let given = encoder.encode(&sound);
                    sound.close();
                    given
                });
                if let Err(error) = given {
                    log::warn!(
                        "the browser did not take a piece of the microphone: {}",
                        words(&error)
                    );
                    stopped.set(Some(NOT_ENCODED));
                }
            })
        };

        let microphone = Self {
            _tracks: tracks,
            source,
            gatherer,
            encoder,
            level,
            stopped,
            gathered,
            _encoded: encoded,
            _failed: failed,
        };
        let wired = (|| {
            let port = microphone.gatherer.port()?;
            port.set_onmessage(Some(microphone.gathered.as_ref().unchecked_ref()));
            microphone.source.connect_with_audio_node(&microphone.gatherer)?;
            // On to the speakers, so that the browser has a reason to run
            // the script. The script sends them nothing.
            microphone.gatherer.connect_with_audio_node(&speakers.destination())?;
            Ok(())
        })();
        // Dropping it undoes what was wired.
        wired.map(|()| microphone).map_err(|error: JsValue| not_opened(&error))
    }

    /// How loud the last piece was, from 0 for silence to 1 for as loud as
    /// a microphone gets.
    pub fn level(&self) -> f32 {
        self.level.get()
    }

    /// Why the microphone no longer captures, in words for the person, when
    /// it stopped on its own.
    pub fn stopped(&self) -> Option<&'static str> {
        self.stopped.get()
    }
}

impl Drop for Microphone {
    fn drop(&mut self) {
        if let Ok(port) = self.gatherer.port() {
            port.set_onmessage(None);
            // The script stops on any message.
            let _ = port.post_message(&JsValue::NULL);
        }
        let _ = self.source.disconnect();
        let _ = self.gatherer.disconnect();
        // Closing an encoder that closed itself is an error, and no news.
        let _ = self.encoder.close();
    }
}

/// Asks the browser for a microphone. `stopped` hears of it if the browser
/// takes it back.
async fn capture(
    stopped: &Rc<Cell<Option<&'static str>>>,
) -> Result<(Tracks, MediaStream), String> {
    let nothing = || "This browser gives this page no microphone.".to_owned();
    let devices = web_sys::window().ok_or_else(nothing)?.navigator().media_devices();
    // Missing on a page that did not come over HTTPS.
    let devices = devices.ok().filter(|devices| !devices.is_undefined()).ok_or_else(nothing)?;

    // A voice for people on the PC: the browser takes out the echo of what
    // the page plays and the noise of the room, as it does for a call.
    let voice = object(&[
        ("channelCount", 1.into()),
        ("echoCancellation", true.into()),
        ("noiseSuppression", true.into()),
        ("autoGainControl", true.into()),
    ]);
    let wanted = MediaStreamConstraints::new();
    wanted.set_audio(&voice);
    let asked =
        devices.get_user_media_with_constraints(&wanted).map_err(|error| refused(&error))?;
    let stream: MediaStream =
        JsFuture::from(asked).await.map_err(|error| refused(&error))?.unchecked_into();

    let gone = Rc::clone(stopped);
    let ended = Closure::new(move || gone.set(Some(STOPPED)));
    let tracks: Vec<MediaStreamTrack> =
        stream.get_audio_tracks().iter().map(JsCast::unchecked_into).collect();
    for track in &tracks {
        track.set_onended(Some(ended.as_ref().unchecked_ref()));
    }
    let tracks = Tracks { tracks, _ended: ended };
    if tracks.tracks.is_empty() {
        return Err("This device has no microphone.".to_owned());
    }
    Ok((tracks, stream))
}

/// Why the browser gave no microphone, in words for the person.
fn refused(error: &JsValue) -> String {
    let name =
        Reflect::get(error, &JsValue::from_str("name")).ok().and_then(|name| name.as_string());
    match name.as_deref() {
        Some("NotAllowedError" | "SecurityError") => {
            "The browser does not let this page use the microphone. Allow it in the browser's \
             settings for this page."
                .to_owned()
        }
        Some("NotFoundError" | "OverconstrainedError") => {
            "This device has no microphone.".to_owned()
        }
        Some("NotReadableError" | "AbortError") => {
            "The microphone could not be opened. Another app may be using it.".to_owned()
        }
        _ => not_opened(error),
    }
}

fn not_opened(error: &JsValue) -> String {
    format!("The microphone could not be opened: {}.", words(error).trim_end_matches('.'))
}

/// How loud `samples` are: 1 for a tone that reaches the limits.
fn loudness(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let mean = samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32;
    // A tone at the limits has a mean square of a half.
    (mean * 2.0).sqrt().min(1.0)
}
