//! The microphone: the system's default sound input.

use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicBool, AtomicU32, Ordering},
};

use cpal::{
    BufferSize, ErrorKind, FromSample, SampleFormat, SizedSample, StreamConfig,
    SupportedBufferSize, SupportedStreamConfig, SupportedStreamConfigRange,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};

use crate::{
    Error,
    voice::{VOICE_RATE, VoiceEncoder},
};

/// Captures the system's default input and encodes it for a PC.
///
/// Capturing starts when this is opened and stops when it is dropped:
/// nothing is listened to in between two of them.
pub struct Microphone {
    /// How loud the last frame was, as the bits of an `f32`.
    level: Arc<AtomicU32>,
    /// Set by the input when it can no longer capture.
    lost: Arc<AtomicBool>,
    /// Capturing stops when this is dropped.
    _stream: cpal::Stream,
}

/// Takes each frame of the microphone, encoded.
type Packets = Box<dyn FnMut(&[u8]) + Send>;

/// What the input's thread works with.
struct Capture {
    voice: VoiceEncoder,
    /// What was captured, as the encoder takes it.
    samples: Vec<f32>,
    packet: Packets,
    level: Arc<AtomicU32>,
}

impl Microphone {
    /// Opens the default input and hands `packet` every frame of it, encoded
    /// as [`VoiceEncoder`] does, on a thread of the system's.
    pub fn open(packet: impl FnMut(&[u8]) + Send + 'static) -> Result<Self, Error> {
        let device = cpal::default_host()
            .default_input_device()
            .ok_or_else(|| Error::Microphone("this device has no microphone".to_owned()))?;
        // The rate a PC takes when the microphone has it, which most do.
        // Otherwise whatever the microphone prefers, brought to that rate.
        let offered = device.supported_input_configs().map_err(microphone_error)?;
        let chosen = match choose(offered) {
            Some(chosen) => chosen,
            None => device.default_input_config().map_err(microphone_error)?,
        };
        let (rate, channels) = (chosen.sample_rate(), usize::from(chosen.channels()));

        let level = Arc::new(AtomicU32::new(0));
        let lost = Arc::new(AtomicBool::new(false));
        let capture = Arc::new(Mutex::new(Capture {
            voice: VoiceEncoder::new(rate, channels)?,
            samples: Vec::new(),
            packet: Box::new(packet),
            level: Arc::clone(&level),
        }));

        // Ask for small chunks to keep the delay low. Not every input takes
        // the hint, so fall back to whatever it prefers.
        let mut config = chosen.config();
        let small = rate / 100;
        let hints = match chosen.buffer_size() {
            SupportedBufferSize::Range { min, max } if (*min..=*max).contains(&small) => {
                vec![BufferSize::Fixed(small), BufferSize::Default]
            }
            _ => vec![BufferSize::Default],
        };
        let mut failure = None;
        for hint in hints {
            config.buffer_size = hint;
            match start(&device, &chosen, config, &capture, &lost) {
                Ok(stream) => {
                    log::info!(
                        "the microphone captures {channels} channels as {} at {rate} Hz",
                        chosen.sample_format(),
                    );
                    return Ok(Self { level, lost, _stream: stream });
                }
                Err(error) => failure = Some(error),
            }
        }
        Err(failure.unwrap_or_else(|| Error::Microphone("the microphone did not open".to_owned())))
    }

    /// How loud the last frame was, from 0 for silence to 1 for as loud as
    /// a microphone gets.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    /// Whether the microphone stopped capturing on its own, for example
    /// because it was unplugged.
    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::Relaxed)
    }
}

/// Picks the configuration to capture a voice with among those the input
/// offers at the rate a PC takes: as few channels as it has, in the sample
/// format that loses the least. `None` when it offers nothing at that rate.
fn choose(
    offered: impl IntoIterator<Item = SupportedStreamConfigRange>,
) -> Option<SupportedStreamConfig> {
    offered
        .into_iter()
        .filter_map(|range| {
            let format = rank(range.sample_format())?;
            let channels = range.channels();
            if channels == 0 {
                return None;
            }
            Some(((channels, format), range.try_with_sample_rate(VOICE_RATE)?))
        })
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, config)| config)
}

/// The sample formats Leto captures, the one that loses the least first.
fn rank(format: SampleFormat) -> Option<u8> {
    match format {
        SampleFormat::F32 => Some(0),
        SampleFormat::I32 => Some(1),
        SampleFormat::I16 => Some(2),
        SampleFormat::U16 => Some(3),
        _ => None,
    }
}

fn start(
    device: &cpal::Device,
    chosen: &SupportedStreamConfig,
    config: StreamConfig,
    capture: &Arc<Mutex<Capture>>,
    lost: &Arc<AtomicBool>,
) -> Result<cpal::Stream, Error> {
    let stream = match chosen.sample_format() {
        SampleFormat::F32 => build::<f32>(device, config, capture, lost),
        SampleFormat::I32 => build::<i32>(device, config, capture, lost),
        SampleFormat::I16 => build::<i16>(device, config, capture, lost),
        SampleFormat::U16 => build::<u16>(device, config, capture, lost),
        other => return Err(Error::Microphone(format!("{other} samples are not supported"))),
    }
    .map_err(microphone_error)?;
    stream.play().map_err(microphone_error)?;
    Ok(stream)
}

fn build<T>(
    device: &cpal::Device,
    config: StreamConfig,
    capture: &Arc<Mutex<Capture>>,
    lost: &Arc<AtomicBool>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let capture = Arc::clone(capture);
    let lost = Arc::clone(lost);
    device.build_input_stream(
        config,
        move |captured: &[T], _: &cpal::InputCallbackInfo| {
            // Only this thread ever takes the lock, and what it guards is as
            // good as before if it panicked while holding it.
            let mut capture = capture.lock().unwrap_or_else(PoisonError::into_inner);
            let Capture { voice, samples, packet, level } = &mut *capture;
            samples.clear();
            samples.extend(captured.iter().map(|sample| f32::from_sample_(*sample)));
            voice.push(samples, packet);
            level.store(voice.level().to_bits(), Ordering::Relaxed);
        },
        move |error| match error.kind() {
            // Hiccups: the input captures on.
            ErrorKind::Xrun | ErrorKind::RealtimeDenied | ErrorKind::DeviceChanged => {
                log::debug!("microphone: {error}");
            }
            _ => {
                log::warn!("microphone: {error}");
                lost.store(true, Ordering::Relaxed);
            }
        },
        None,
    )
}

fn microphone_error(error: cpal::Error) -> Error {
    Error::Microphone(match error.kind() {
        ErrorKind::PermissionDenied => {
            "the system does not let Leto listen to it. Allow it in the system's privacy settings"
                .to_owned()
        }
        ErrorKind::DeviceBusy => "another app is using it".to_owned(),
        _ => error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(
        channels: u16,
        min: u32,
        max: u32,
        format: SampleFormat,
    ) -> SupportedStreamConfigRange {
        SupportedStreamConfigRange::new(channels, min, max, SupportedBufferSize::Unknown, format)
    }

    fn chosen(offered: &[SupportedStreamConfigRange]) -> Option<(u16, u32, SampleFormat)> {
        choose(offered.iter().cloned())
            .map(|config| (config.channels(), config.sample_rate(), config.sample_format()))
    }

    #[test]
    fn the_fewest_channels_of_floats_at_the_rate_of_the_pc_are_preferred() {
        let offered = [
            offer(2, 44_100, 192_000, SampleFormat::F32),
            offer(1, 8_000, 96_000, SampleFormat::I16),
            offer(1, 8_000, 96_000, SampleFormat::F32),
            offer(8, 44_100, 192_000, SampleFormat::F32),
        ];
        assert_eq!(chosen(&offered), Some((1, 48_000, SampleFormat::F32)));
        // Two channels are mixed into one rather than captured at another
        // rate.
        assert_eq!(chosen(&offered[..1]), Some((2, 48_000, SampleFormat::F32)));
    }

    #[test]
    fn a_microphone_without_the_rate_or_a_known_format_is_left_to_its_own_choice() {
        let offered = [
            offer(1, 16_000, 16_000, SampleFormat::I16),
            offer(2, 44_100, 44_100, SampleFormat::F32),
            offer(1, 8_000, 96_000, SampleFormat::U8),
        ];
        assert_eq!(chosen(&offered), None);
    }
}
