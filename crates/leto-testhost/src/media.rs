//! What the stand-in host shows and plays: a test card and a melody.
//!
//! Both are made to be judged at a glance. The card has a sun that passes
//! behind a moon, so stutter shows, a marker that crosses the top edge once a
//! second, so dropped frames show, and the eight classic colour bars, so
//! swapped or washed-out colours show. The melody repeats every two seconds,
//! so a gap or a crackle is easy to hear.

use openh264::{
    OpenH264API,
    encoder::{BitRate, Encoder, EncoderConfig, FrameRate, FrameType, RateControlMode},
    formats::YUVSource,
};
use opus_rs::{Application, OpusEncoder};

/// The largest picture the encoder accepts.
const MAX_WIDTH: u32 = 3840;
const MAX_HEIGHT: u32 = 2160;

/// A colour as the encoder wants it: luma and two chroma values.
#[derive(Clone, Copy)]
struct Yuv(u8, u8, u8);

impl Yuv {
    /// Converts from red, green and blue with the BT.601 matrix in limited
    /// range, what a host sends for standard dynamic range.
    fn from_rgb(red: u8, green: u8, blue: u8) -> Self {
        let (r, g, b) = (f32::from(red), f32::from(green), f32::from(blue));
        let y = 16.0 + 0.257 * r + 0.504 * g + 0.098 * b;
        let u = 128.0 - 0.148 * r - 0.291 * g + 0.439 * b;
        let v = 128.0 + 0.439 * r - 0.368 * g - 0.071 * b;
        Self(y.round() as u8, u.round() as u8, v.round() as u8)
    }
}

/// A picture in the planar 4:2:0 layout: full-size luma, quarter-size chroma.
#[derive(Clone)]
struct Planes {
    width: usize,
    height: usize,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
}

impl Planes {
    fn new(width: usize, height: usize, colour: Yuv) -> Self {
        Self {
            width,
            height,
            y: vec![colour.0; width * height],
            u: vec![colour.1; width * height / 4],
            v: vec![colour.2; width * height / 4],
        }
    }

    /// Fills the rectangle from (`left`, `top`), `width` by `height`.
    fn rectangle(&mut self, left: usize, top: usize, width: usize, height: usize, colour: Yuv) {
        let right = (left + width).min(self.width);
        let bottom = (top + height).min(self.height);
        if left >= right || top >= bottom {
            return;
        }
        for row in top..bottom {
            self.y[row * self.width + left..row * self.width + right].fill(colour.0);
        }
        let half = self.width / 2;
        for row in top / 2..bottom.div_ceil(2).min(self.height / 2) {
            let (from, to) = (row * half + left / 2, row * half + right.div_ceil(2).min(half));
            self.u[from..to].fill(colour.1);
            self.v[from..to].fill(colour.2);
        }
    }

    /// Fills the disc around (`x`, `y`).
    fn disc(&mut self, x: f32, y: f32, radius: f32, colour: Yuv) {
        let top = (y - radius).floor().max(0.0) as usize;
        let bottom = ((y + radius).ceil().max(0.0) as usize).min(self.height);
        for row in top..bottom {
            let distance = row as f32 + 0.5 - y;
            let reach = (radius * radius - distance * distance).max(0.0).sqrt();
            let left = (x - reach).round().max(0.0) as usize;
            let right = ((x + reach).round().max(0.0) as usize).min(self.width);
            if left < right {
                self.rectangle(left, row, right - left, 1, colour);
            }
        }
    }

    fn copy_from(&mut self, other: &Self) {
        self.y.copy_from_slice(&other.y);
        self.u.copy_from_slice(&other.u);
        self.v.copy_from_slice(&other.v);
    }
}

impl YUVSource for Planes {
    fn dimensions(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    fn strides(&self) -> (usize, usize, usize) {
        (self.width, self.width / 2, self.width / 2)
    }

    fn y(&self) -> &[u8] {
        &self.y
    }

    fn u(&self) -> &[u8] {
        &self.u
    }

    fn v(&self) -> &[u8] {
        &self.v
    }
}

/// One encoded picture.
pub(crate) struct EncodedFrame<'a> {
    pub keyframe: bool,
    /// An H.264 access unit with start codes.
    pub data: &'a [u8],
}

/// The moving test card, encoded as H.264.
pub(crate) struct TestCard {
    encoder: Encoder,
    fps: u32,
    background: Planes,
    canvas: Planes,
    number: u64,
    data: Vec<u8>,
}

impl TestCard {
    /// A card of `width` by `height`, rounded down to even numbers and to
    /// what the encoder accepts.
    pub fn new(width: u32, height: u32, fps: u32, bitrate_kbps: u32) -> Option<Self> {
        let width = (width.clamp(64, MAX_WIDTH) & !1) as usize;
        let height = (height.clamp(64, MAX_HEIGHT) & !1) as usize;

        // A host never skips a frame to save bits, and sends a keyframe only
        // at the start and when the client asks for one. Without rate control
        // the encoder keeps one quality throughout, and the card, all flat
        // colours, stays far below any bit rate asked for.
        //
        // The two last settings save bits by leaving alone what looks
        // unchanged, which leaves a trace behind the sun. Without them the
        // brightness of the ground is exact again once the sun has passed. A
        // tint of a few levels can remain: that is this encoder, not the
        // client showing the card.
        let config = EncoderConfig::new()
            .bitrate(BitRate::from_bps(bitrate_kbps.clamp(500, 20_000) * 1000))
            .max_frame_rate(FrameRate::from_hz(fps as f32))
            .rate_control_mode(RateControlMode::Off)
            .skip_frames(false)
            .scene_change_detect(false)
            .background_detection(false)
            .adaptive_quantization(false);
        let encoder = Encoder::with_api_config(OpenH264API::from_source(), config).ok()?;

        let ground = Yuv::from_rgb(0x15, 0x13, 0x12);
        let mut background = Planes::new(width, height, ground);
        // The classic bars: white, yellow, cyan, green, magenta, red, blue,
        // black, at three quarters of full brightness.
        let bars = [
            (191, 191, 191),
            (191, 191, 0),
            (0, 191, 191),
            (0, 191, 0),
            (191, 0, 191),
            (191, 0, 0),
            (0, 0, 191),
            (0, 0, 0),
        ];
        let bar_height = height / 10;
        for (index, (red, green, blue)) in bars.into_iter().enumerate() {
            let left = index * width / bars.len();
            let right = (index + 1) * width / bars.len();
            background.rectangle(
                left,
                height - bar_height,
                right - left,
                bar_height,
                Yuv::from_rgb(red, green, blue),
            );
        }

        Some(Self {
            encoder,
            fps: fps.max(1),
            canvas: background.clone(),
            background,
            number: 0,
            data: Vec::new(),
        })
    }

    /// Draws and encodes the next picture. `keyframe` asks for one that can
    /// be decoded on its own.
    pub fn next(&mut self, keyframe: bool) -> Option<EncodedFrame<'_>> {
        let (width, height) = (self.canvas.width as f32, self.canvas.height as f32);
        let seconds = self.number as f32 / self.fps as f32;
        self.canvas.copy_from(&self.background);

        // The sun goes round behind the moon every eight seconds.
        let angle = seconds * std::f32::consts::TAU / 8.0;
        let radius = height / 7.0;
        let centre = (width / 2.0, height * 0.42);
        let sun = (centre.0 + angle.cos() * width / 4.0, centre.1 + angle.sin() * height / 9.0);
        self.canvas.disc(sun.0, sun.1, radius, Yuv::from_rgb(0xFF, 0x6B, 0x35));
        self.canvas.disc(centre.0, centre.1, radius * 0.9, Yuv::from_rgb(0xF2, 0xED, 0xE4));

        // The marker crosses the top edge once a second, one step a frame.
        let step = self.number % u64::from(self.fps);
        let marker = (self.canvas.width / self.fps as usize).max(2);
        self.canvas.rectangle(
            step as usize * self.canvas.width / self.fps as usize,
            0,
            marker,
            (self.canvas.height / 40).max(2),
            Yuv::from_rgb(0xF2, 0xED, 0xE4),
        );

        if keyframe {
            self.encoder.force_intra_frame();
        }
        self.number += 1;
        let encoded = self.encoder.encode(&self.canvas).ok()?;
        let kind = encoded.frame_type();
        if matches!(kind, FrameType::Skip | FrameType::Invalid) {
            return None;
        }
        self.data.clear();
        encoded.write_vec(&mut self.data);
        Some(EncodedFrame {
            keyframe: matches!(kind, FrameType::IDR | FrameType::I),
            data: &self.data,
        })
    }
}

/// The notes of the melody, in hertz: an arpeggio up and down.
const NOTES: [f32; 8] = [220.0, 277.18, 329.63, 440.0, 554.37, 440.0, 329.63, 277.18];
/// How long each note lasts, in seconds.
const NOTE: f32 = 0.25;

/// The melody, encoded as Opus in stereo.
pub(crate) struct Melody {
    encoder: OpusEncoder,
    rate: u32,
    /// Samples in one packet, per channel.
    samples: usize,
    position: u64,
    pcm: Vec<f32>,
    packet: Vec<u8>,
}

impl Melody {
    pub fn new(rate: u32, bitrate: i32, samples: usize) -> Option<Self> {
        // Low delay and a constant bit rate, as a host encodes game sound.
        let mut encoder = OpusEncoder::new(rate as i32, 2, Application::RestrictedLowDelay).ok()?;
        encoder.bitrate_bps = bitrate;
        encoder.use_cbr = true;
        Some(Self {
            encoder,
            rate,
            samples,
            position: 0,
            pcm: vec![0.0; samples * 2],
            packet: vec![0; 1500],
        })
    }

    /// Encodes the next packet of sound.
    pub fn next(&mut self) -> Option<&[u8]> {
        for index in 0..self.samples {
            let time = (self.position + index as u64) as f32 / self.rate as f32;
            let note = (time / NOTE) as usize % NOTES.len();
            let into_note = time % NOTE;
            // A quick rise and a slow fall, so notes do not click.
            let envelope = (into_note / 0.01).min(1.0) * (1.0 - into_note / NOTE).powi(2);
            let phase = time * NOTES[note] * std::f32::consts::TAU;
            let sample = (phase.sin() + 0.3 * (phase * 2.0).sin()) * envelope * 0.25;
            self.pcm[index * 2] = sample;
            self.pcm[index * 2 + 1] = sample;
        }
        self.position += self.samples as u64;
        let written = self.encoder.encode(&self.pcm, self.samples, &mut self.packet).ok()?;
        Some(&self.packet[..written])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_starts_with_a_keyframe_and_makes_one_on_request() {
        let mut card = TestCard::new(320, 180, 30, 2000).expect("the encoder starts");
        assert!(card.next(false).expect("a first picture").keyframe);
        assert!(!card.next(false).expect("a second picture").keyframe);
        assert!(card.next(true).expect("a requested picture").keyframe);
    }

    #[test]
    fn the_sun_leaves_no_trace_on_the_ground() {
        use openh264::decoder::Decoder;

        let (width, height, fps) = (640.0f32, 360.0f32, 30.0f32);
        let mut card = TestCard::new(640, 360, 30, 2000).expect("the encoder starts");
        let mut decoder = Decoder::new().expect("the decoder starts");
        let ground = i32::from(Yuv::from_rgb(0x15, 0x13, 0x12).0);

        // Three quarters of the sun's way round.
        let last = 6 * 30 - 1;
        for number in 0..=last {
            let data = card.next(false).expect("a picture").data.to_vec();
            let decoded = decoder.decode(&data).expect("it decodes").expect("a picture comes out");
            if number < last {
                continue;
            }
            let angle = number as f32 / fps * std::f32::consts::TAU / 8.0;
            let moon = (width / 2.0, height * 0.42);
            let sun = (moon.0 + angle.cos() * width / 4.0, moon.1 + angle.sin() * height / 9.0);
            let (stride, _, _) = decoded.strides();
            // Between the marker's edge and the colour bars, away from the discs.
            for row in 40..300 {
                for column in 0..640 {
                    let away = |disc: (f32, f32)| {
                        (column as f32 - disc.0).hypot(row as f32 - disc.1) > height / 7.0 + 12.0
                    };
                    if away(sun) && away(moon) {
                        let luma = i32::from(decoded.y()[row * stride + column]);
                        assert!((luma - ground).abs() <= 2, "{luma} at {column}, {row}");
                    }
                }
            }
        }
    }

    #[test]
    fn an_odd_size_is_rounded_down() {
        let card = TestCard::new(321, 181, 30, 2000).expect("the encoder starts");
        assert_eq!((card.canvas.width, card.canvas.height), (320, 180));
    }

    #[test]
    fn every_sound_packet_has_the_same_size() {
        let mut melody = Melody::new(48_000, 96_000, 240).expect("the encoder starts");
        for _ in 0..400 {
            assert_eq!(melody.next().expect("a packet").len(), 60);
        }
    }
}
