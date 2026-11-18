//! Opus to samples.

use opus_decoder::OpusMultistreamDecoder;

use crate::Error;

/// The longest an Opus packet can last, in seconds.
const LONGEST_PACKET: f32 = 0.12;

/// Decodes the sound of one stream, in order.
pub struct SoundDecoder {
    decoder: OpusMultistreamDecoder,
    channels: usize,
    samples: Vec<f32>,
}

impl SoundDecoder {
    /// A decoder for sound at `sample_rate` with `channels` channels, carried
    /// in `streams` Opus streams of which `coupled_streams` are stereo pairs.
    /// `mapping` says which decoded channel goes to which output channel.
    ///
    /// Plain stereo is one coupled stream with the mapping `[0, 1]`.
    pub fn new(
        sample_rate: u32,
        channels: usize,
        streams: usize,
        coupled_streams: usize,
        mapping: &[u8],
    ) -> Result<Self, Error> {
        let mapping = mapping
            .get(..channels)
            .ok_or_else(|| Error::Sound(format!("{channels} channels need a mapping that long")))?;
        let decoder =
            OpusMultistreamDecoder::new(sample_rate, channels, streams, coupled_streams, mapping)
                .map_err(|error| Error::Sound(error.to_string()))?;
        let longest = (sample_rate as f32 * LONGEST_PACKET).ceil() as usize;
        Ok(Self { decoder, channels, samples: vec![0.0; longest * channels] })
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Decodes one packet. The samples of all channels come out interleaved,
    /// between -1 and 1.
    pub fn decode(&mut self, packet: &[u8]) -> Result<&[f32], Error> {
        let per_channel = self
            .decoder
            .decode_float(packet, &mut self.samples, false)
            .map_err(|error| Error::Sound(error.to_string()))?;
        Ok(&self.samples[..per_channel * self.channels])
    }
}

#[cfg(test)]
mod tests {
    use opus_rs::{Application, OpusEncoder};

    use super::*;

    const RATE: u32 = 48_000;
    /// Five milliseconds, the packet length hosts use.
    const PACKET: usize = 240;

    /// How loud `samples` are: the root of their mean square.
    fn loudness(samples: impl Iterator<Item = f32>) -> f32 {
        let (sum, count) =
            samples.fold((0.0, 0), |(sum, count), sample| (sum + sample * sample, count + 1));
        (sum / count.max(1) as f32).sqrt()
    }

    #[test]
    fn a_tone_comes_out_as_loud_as_it_went_in_on_the_side_it_went_in() {
        let mut encoder =
            OpusEncoder::new(RATE as i32, 2, Application::RestrictedLowDelay).expect("encoder");
        encoder.bitrate_bps = 96_000;
        let mut decoder = SoundDecoder::new(RATE, 2, 1, 1, &[0, 1]).expect("decoder");

        let mut left = Vec::new();
        let mut right = Vec::new();
        let mut packet = vec![0u8; 1500];
        for number in 0..100 {
            // A 440 Hz tone at half volume on the left, silence on the right.
            let pcm: Vec<f32> = (0..PACKET)
                .flat_map(|index| {
                    let time = (number * PACKET + index) as f32 / RATE as f32;
                    [0.5 * (time * 440.0 * std::f32::consts::TAU).sin(), 0.0]
                })
                .collect();
            let written = encoder.encode(&pcm, PACKET, &mut packet).expect("a packet encodes");
            let decoded = decoder.decode(&packet[..written]).expect("a packet decodes");
            assert_eq!(decoded.len(), PACKET * 2);
            left.extend(decoded.iter().step_by(2));
            right.extend(decoded.iter().skip(1).step_by(2));
        }

        // The first packets fade in. A sine at half volume has a loudness of
        // 0.5 / sqrt(2).
        let settled = PACKET * 10;
        let left = loudness(left[settled..].iter().copied());
        let right = loudness(right[settled..].iter().copied());
        assert!((left - 0.3536).abs() < 0.03, "left is {left}");
        assert!(right < 0.02, "right is {right}");
    }

    #[test]
    fn a_broken_packet_is_an_error() {
        let mut decoder = SoundDecoder::new(RATE, 2, 1, 1, &[0, 1]).expect("decoder");
        // Two frames, the first one said to be longer than the whole packet.
        assert!(decoder.decode(&[0xFE, 0xF0, 0x01]).is_err());
    }

    #[test]
    fn a_mapping_shorter_than_the_channels_is_refused() {
        assert!(SoundDecoder::new(RATE, 2, 1, 1, &[0]).is_err());
    }
}
