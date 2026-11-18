//! The speakers: the system's default sound output.

use std::{
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use cpal::{
    BufferSize, ErrorKind, FromSample, SampleFormat, SizedSample, StreamConfig,
    SupportedBufferSize, SupportedStreamConfig, SupportedStreamConfigRange,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};

use crate::{Error, queue::Queue};

/// How long to wait before trying again to open speakers that failed.
const RETRY_EVERY: Duration = Duration::from_secs(2);

/// Plays stereo sound on the system's default output.
///
/// Follows the system when the default output changes, and opens the output
/// again when it was lost, for example when headphones were unplugged.
pub struct Speaker {
    sample_rate: u32,
    output: Option<Output>,
    last_try: Instant,
}

/// An open output and the sound waiting for it.
struct Output {
    queue: Arc<Mutex<Queue>>,
    /// Set by the output when it can no longer play.
    lost: Arc<AtomicBool>,
    /// Playing stops when this is dropped.
    _stream: cpal::Stream,
}

impl Speaker {
    /// Opens the default output for stereo sound at `sample_rate`.
    pub fn open(sample_rate: u32) -> Result<Self, Error> {
        let output = Output::open(sample_rate)?;
        Ok(Self { sample_rate, output: Some(output), last_try: Instant::now() })
    }

    /// Plays stereo samples (left, right, left, right…) after the ones
    /// already waiting. Never blocks: sound that cannot be played is dropped.
    pub fn play(&mut self, stereo: &[f32]) {
        if self.output.as_ref().is_some_and(|output| output.lost.load(Ordering::Relaxed)) {
            log::warn!("the sound output was lost, looking for another");
            self.output = None;
        }
        if self.output.is_none() && self.last_try.elapsed() >= RETRY_EVERY {
            self.last_try = Instant::now();
            match Output::open(self.sample_rate) {
                Ok(output) => self.output = Some(output),
                Err(error) => log::debug!("still no sound output: {error}"),
            }
        }
        if let Some(output) = &self.output {
            lock(&output.queue).push(stereo);
        }
    }
}

impl Output {
    fn open(sample_rate: u32) -> Result<Self, Error> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or_else(|| Error::Speaker("this device has no sound output".to_owned()))?;
        let offered = device.supported_output_configs().map_err(speaker_error)?;
        let chosen = choose(offered, sample_rate).ok_or_else(|| {
            Error::Speaker(format!("the sound output cannot play at {sample_rate} Hz"))
        })?;

        let queue = Arc::new(Mutex::new(Queue::new(sample_rate, usize::from(chosen.channels()))));
        let lost = Arc::new(AtomicBool::new(false));

        // Ask for small chunks to keep the delay low. Not every output takes
        // the hint, so fall back to whatever it prefers.
        let mut config = chosen.config();
        let small = sample_rate / 100;
        let hints = match chosen.buffer_size() {
            SupportedBufferSize::Range { min, max } if (*min..=*max).contains(&small) => {
                vec![BufferSize::Fixed(small), BufferSize::Default]
            }
            _ => vec![BufferSize::Default],
        };
        let mut failure = None;
        for hint in hints {
            config.buffer_size = hint;
            match start(&device, &chosen, config, &queue, &lost) {
                Ok(stream) => {
                    log::info!(
                        "sound plays on {} channels as {} at {sample_rate} Hz",
                        chosen.channels(),
                        chosen.sample_format(),
                    );
                    return Ok(Self { queue, lost, _stream: stream });
                }
                Err(error) => failure = Some(error),
            }
        }
        Err(failure.unwrap_or_else(|| Error::Speaker("the sound output did not open".to_owned())))
    }
}

/// Picks the configuration to play stereo at `sample_rate` among those the
/// output offers: two channels when it can, in the sample format that loses
/// the least.
fn choose(
    offered: impl IntoIterator<Item = SupportedStreamConfigRange>,
    sample_rate: u32,
) -> Option<SupportedStreamConfig> {
    offered
        .into_iter()
        .filter_map(|range| {
            let format = match range.sample_format() {
                SampleFormat::F32 => 0,
                SampleFormat::I32 => 1,
                SampleFormat::I16 => 2,
                SampleFormat::U16 => 3,
                _ => return None,
            };
            let channels = match range.channels() {
                0 => return None,
                2 => (0, 0),
                1 => (2, 0),
                more => (1, more),
            };
            Some(((channels, format), range.try_with_sample_rate(sample_rate)?))
        })
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, config)| config)
}

fn start(
    device: &cpal::Device,
    chosen: &SupportedStreamConfig,
    config: StreamConfig,
    queue: &Arc<Mutex<Queue>>,
    lost: &Arc<AtomicBool>,
) -> Result<cpal::Stream, Error> {
    let stream = match chosen.sample_format() {
        SampleFormat::F32 => build::<f32>(device, config, queue, lost),
        SampleFormat::I32 => build::<i32>(device, config, queue, lost),
        SampleFormat::I16 => build::<i16>(device, config, queue, lost),
        SampleFormat::U16 => build::<u16>(device, config, queue, lost),
        other => return Err(Error::Speaker(format!("{other} samples are not supported"))),
    }
    .map_err(speaker_error)?;
    stream.play().map_err(speaker_error)?;
    Ok(stream)
}

fn build<T>(
    device: &cpal::Device,
    config: StreamConfig,
    queue: &Arc<Mutex<Queue>>,
    lost: &Arc<AtomicBool>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample + FromSample<f32>,
{
    let queue = Arc::clone(queue);
    let lost = Arc::clone(lost);
    let mut samples = Vec::<f32>::new();
    device.build_output_stream(
        config,
        move |out: &mut [T], _: &cpal::OutputCallbackInfo| {
            samples.resize(out.len(), 0.0);
            lock(&queue).take(&mut samples);
            for (slot, sample) in out.iter_mut().zip(&samples) {
                *slot = T::from_sample(*sample);
            }
        },
        move |error| match error.kind() {
            // Hiccups: the output plays on.
            ErrorKind::Xrun | ErrorKind::RealtimeDenied | ErrorKind::DeviceChanged => {
                log::debug!("sound output: {error}");
            }
            _ => {
                log::warn!("sound output: {error}");
                lost.store(true, Ordering::Relaxed);
            }
        },
        None,
    )
}

fn speaker_error(error: cpal::Error) -> Error {
    Error::Speaker(error.to_string())
}

fn lock(queue: &Mutex<Queue>) -> MutexGuard<'_, Queue> {
    // The queue is only samples: it is as good as before if a thread
    // panicked while holding it.
    queue.lock().unwrap_or_else(PoisonError::into_inner)
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

    fn chosen(offered: &[SupportedStreamConfigRange]) -> Option<(u16, SampleFormat)> {
        choose(offered.iter().cloned(), 48_000)
            .map(|config| (config.channels(), config.sample_format()))
    }

    #[test]
    fn two_channels_of_floats_are_preferred() {
        let offered = [
            offer(8, 44_100, 192_000, SampleFormat::F32),
            offer(2, 44_100, 192_000, SampleFormat::I16),
            offer(2, 44_100, 192_000, SampleFormat::F32),
            offer(1, 44_100, 192_000, SampleFormat::F32),
        ];
        assert_eq!(chosen(&offered), Some((2, SampleFormat::F32)));
    }

    #[test]
    fn more_channels_beat_one_and_fewer_beat_more() {
        let offered = [
            offer(1, 48_000, 48_000, SampleFormat::F32),
            offer(8, 48_000, 48_000, SampleFormat::I16),
            offer(6, 48_000, 48_000, SampleFormat::I16),
        ];
        assert_eq!(chosen(&offered), Some((6, SampleFormat::I16)));
        assert_eq!(chosen(&offered[..1]), Some((1, SampleFormat::F32)));
    }

    #[test]
    fn an_output_that_cannot_play_at_the_rate_or_in_a_known_format_is_not_chosen() {
        let offered = [
            offer(2, 44_100, 44_100, SampleFormat::F32),
            offer(2, 8_000, 96_000, SampleFormat::U8),
        ];
        assert_eq!(chosen(&offered), None);
    }
}
