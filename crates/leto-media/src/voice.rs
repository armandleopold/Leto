//! From what a microphone captures to what a PC takes.
//!
//! A PC that takes a microphone wants one channel at 48 kHz, as Opus frames
//! of twenty milliseconds. A microphone gives whatever it is built for:
//! often two channels, often 44.1 kHz, sometimes 16 kHz on a headset. This
//! mixes the channels into one, brings the samples to 48 kHz, cuts them into
//! frames and encodes each.

use std::time::Duration;

use opus_rs::{Application, OpusEncoder};

use crate::Error;

/// The rate a PC takes a microphone at, in samples per second.
pub const VOICE_RATE: u32 = 48_000;
/// How long one frame of the microphone lasts.
pub const VOICE_FRAME: Duration = Duration::from_millis(20);
/// Samples in one frame.
const FRAME: usize = (VOICE_RATE as usize / 1000) * VOICE_FRAME.as_millis() as usize;

/// Bits per second. Plenty for a voice on one channel, and little next to
/// the pictures going the other way.
const BIT_RATE: i32 = 48_000;
/// No Opus frame is longer than this.
const LARGEST_PACKET: usize = 1275;

/// Encodes what a microphone captures, in order.
pub struct VoiceEncoder {
    encoder: OpusEncoder,
    channels: usize,
    /// How far the capture advances for each sample at 48 kHz.
    step: f64,
    /// Where the next sample at 48 kHz falls, counted in captured samples
    /// from `before`.
    position: f64,
    /// The last captured sample of the call before, all channels mixed.
    before: f32,
    /// Samples at 48 kHz that do not fill a frame yet.
    waiting: Vec<f32>,
    packet: Vec<u8>,
    level: f32,
}

impl VoiceEncoder {
    /// An encoder for a microphone that captures `channels` channels at
    /// `rate` samples per second.
    pub fn new(rate: u32, channels: usize) -> Result<Self, Error> {
        if rate == 0 || channels == 0 {
            return Err(Error::Microphone("the microphone says it captures nothing".to_owned()));
        }
        // The low-delay mode: the same kind of frames a host sends its own
        // sound in, which every Opus decoder is at home with.
        let mut encoder = OpusEncoder::new(VOICE_RATE as i32, 1, Application::RestrictedLowDelay)
            .map_err(|error| Error::Microphone(error.to_owned()))?;
        encoder.bitrate_bps = BIT_RATE;
        Ok(Self {
            encoder,
            channels,
            step: f64::from(rate) / f64::from(VOICE_RATE),
            position: 1.0,
            before: 0.0,
            waiting: Vec::with_capacity(FRAME * 2),
            packet: vec![0; LARGEST_PACKET],
            level: 0.0,
        })
    }

    /// Takes what was captured since the last call, the samples of all
    /// channels interleaved, and hands each frame it completes to `packet`.
    pub fn push(&mut self, captured: &[f32], mut packet: impl FnMut(&[u8])) {
        let moments = captured.len() / self.channels;
        let moment = |index: usize| {
            let start = index * self.channels;
            captured[start..start + self.channels].iter().sum::<f32>() / self.channels as f32
        };

        // A sample at 48 kHz falls between two captured ones, and takes of
        // each what its distance to them says. `position` counts from the
        // last sample of the call before, so the samples of this call are
        // at 1 and up, and what falls behind the last of them waits for the
        // next call.
        while self.position <= moments as f64 {
            let index = self.position.floor() as usize;
            let between = (self.position - index as f64) as f32;
            let earlier = if index == 0 { self.before } else { moment(index - 1) };
            // On the last sample itself there is no later one, and none is
            // needed.
            let later = if index < moments { moment(index) } else { earlier };
            self.waiting.push(earlier + (later - earlier) * between);
            self.position += self.step;
        }
        if moments > 0 {
            self.before = moment(moments - 1);
            self.position -= moments as f64;
        }

        let mut frames = self.waiting.chunks_exact(FRAME);
        for frame in &mut frames {
            self.level = loudness(frame);
            match self.encoder.encode(frame, FRAME, &mut self.packet) {
                Ok(length) => packet(&self.packet[..length]),
                Err(error) => log::debug!("a moment of the microphone was not encoded: {error}"),
            }
        }
        let left = frames.remainder().len();
        let done = self.waiting.len() - left;
        self.waiting.drain(..done);
    }

    /// How loud the last frame was, from 0 for silence to 1 for as loud as a
    /// microphone gets.
    pub fn level(&self) -> f32 {
        self.level
    }
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

#[cfg(test)]
mod tests {
    use opus_decoder::OpusDecoder;

    use super::*;

    /// One second of a 440 Hz tone at half volume, as a microphone with
    /// `channels` channels at `rate` would capture it, in pieces of `piece`
    /// samples per channel. Returns the packets it made.
    fn capture(rate: u32, channels: usize, piece: usize) -> (Vec<Vec<u8>>, f32) {
        let mut voice = VoiceEncoder::new(rate, channels).expect("an encoder");
        let captured: Vec<f32> = (0..rate)
            .flat_map(|index| {
                let time = index as f32 / rate as f32;
                let sample = 0.5 * (time * 440.0 * std::f32::consts::TAU).sin();
                std::iter::repeat_n(sample, channels)
            })
            .collect();
        let mut packets = Vec::new();
        for piece in captured.chunks(piece * channels) {
            voice.push(piece, |packet| packets.push(packet.to_vec()));
        }
        (packets, voice.level())
    }

    /// What a PC hears of `packets`: how loud, and at which pitch in hertz.
    fn listen(packets: &[Vec<u8>]) -> (f32, f32) {
        let mut decoder = OpusDecoder::new(VOICE_RATE, 1).expect("a decoder");
        let mut heard = Vec::new();
        let mut samples = vec![0f32; FRAME];
        for packet in packets {
            let decoded = decoder.decode_float(packet, &mut samples, false).expect("it decodes");
            assert_eq!(decoded, FRAME, "every frame lasts twenty milliseconds");
            heard.extend_from_slice(&samples);
        }
        // The first frames fade in.
        let settled = &heard[FRAME * 5..];
        let rises = settled.windows(2).filter(|pair| pair[0] < 0.0 && pair[1] >= 0.0).count();
        (loudness(settled), rises as f32 * VOICE_RATE as f32 / settled.len() as f32)
    }

    #[test]
    fn a_voice_at_the_rate_of_the_pc_is_cut_into_frames_as_it_is() {
        let (packets, level) = capture(48_000, 1, 480);
        assert_eq!(packets.len(), 50);
        let (loud, pitch) = listen(&packets);
        assert!((loud - 0.5).abs() < 0.05, "heard at {loud}");
        assert!((pitch - 440.0).abs() < 3.0, "heard at {pitch} Hz");
        assert!((level - 0.5).abs() < 0.02, "measured at {level}");
    }

    #[test]
    fn a_microphone_at_another_rate_or_with_more_channels_sounds_the_same() {
        // A usual microphone, a headset, a studio one, and pieces of a size
        // that never falls on a frame.
        for (rate, channels, piece) in
            [(44_100, 2, 441), (16_000, 1, 160), (96_000, 2, 1024), (44_100, 1, 333)]
        {
            let (packets, _) = capture(rate, channels, piece);
            // A second of sound, give or take the frame being filled.
            assert!((49..=50).contains(&packets.len()), "{} packets at {rate} Hz", packets.len());
            let (loud, pitch) = listen(&packets);
            assert!((loud - 0.5).abs() < 0.05, "heard at {loud} from {rate} Hz");
            assert!((pitch - 440.0).abs() < 3.0, "heard at {pitch} Hz from {rate} Hz");
        }
    }

    #[test]
    fn silence_is_silent_and_nothing_makes_nothing() {
        let mut voice = VoiceEncoder::new(48_000, 2).expect("an encoder");
        let mut packets = 0;
        voice.push(&[], |_| packets += 1);
        voice.push(&[0.0; 960], |_| packets += 1);
        assert_eq!(packets, 0, "half a frame is not a frame");
        voice.push(&[0.0; 960], |_| packets += 1);
        assert_eq!(packets, 1);
        assert_eq!(voice.level(), 0.0);

        assert!(VoiceEncoder::new(0, 1).is_err());
        assert!(VoiceEncoder::new(48_000, 0).is_err());
    }
}
