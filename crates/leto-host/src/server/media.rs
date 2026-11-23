//! What the stand-in host shows and plays: a test card and a melody.
//!
//! Both are made to be judged at a glance. The card has a sun that passes
//! behind a moon, so stutter shows, a marker that crosses the top edge once a
//! second, so dropped frames show, and the eight classic colour bars, so
//! swapped or washed-out colours show. The melody repeats every two seconds,
//! so a gap or a crackle is easy to hear.
//!
//! The card also shows what the client does: a pointer where its mouse is, a
//! square for every key held, a dial for the first gamepad, a meter for its
//! microphone and, in a corner, what its camera sees.

use std::time::{Duration, Instant};

use leto_core::input::{GamepadButtons, Input};
use openh264::{
    OpenH264API,
    encoder::{BitRate, Encoder, EncoderConfig, FrameRate, FrameType, RateControlMode},
    formats::YUVSource,
};
use opus_rs::{Application, OpusEncoder};

use super::source::{Packets, Picture, Pictures, SOUND_PACKET, Watching};
use crate::camera::Seen;

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

/// What the person at the client is doing, as far as the card shows it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Hands {
    /// Where the pointer is, from 0 to 1 across and down the picture. None
    /// until the mouse was used.
    pub pointer: Option<(f32, f32)>,
    pub mouse_buttons_held: u32,
    pub keys_held: u32,
    /// The left stick of the first gamepad, when one is plugged in.
    pub stick: Option<[f32; 2]>,
    pub gamepad_button_held: bool,
    /// How loud the client's microphone was when it was last heard, from 0
    /// to 1, when the host listens to it.
    pub voice: Option<f32>,
    /// When the microphone was last heard.
    pub voice_heard: Option<Instant>,
}

/// How long without the microphone before its meter falls by half.
const VOICE_FALLS: Duration = Duration::from_millis(50);

impl Hands {
    /// How loud the microphone is at `now`: what was last heard, falling by
    /// half for every moment without it.
    pub fn voice_at(&self, now: Instant) -> Option<f32> {
        let silent = self.voice_heard.map_or(Duration::ZERO, |heard| now.duration_since(heard));
        let halvings = (silent.as_millis() / VOICE_FALLS.as_millis()).min(32) as i32;
        Some(self.voice? * 0.5f32.powi(halvings))
    }

    /// Takes `input` into account, on a picture `width` by `height`.
    pub fn follow(&mut self, input: Input, width: u32, height: u32) {
        match input {
            Input::Key { down: true, .. } => self.keys_held += 1,
            Input::Key { down: false, .. } => self.keys_held = self.keys_held.saturating_sub(1),
            Input::MouseMove { dx, dy } => {
                let (x, y) = self.pointer.unwrap_or((0.5, 0.5));
                self.pointer = Some((
                    (x + f32::from(dx) / width.max(1) as f32).clamp(0.0, 1.0),
                    (y + f32::from(dy) / height.max(1) as f32).clamp(0.0, 1.0),
                ));
            }
            Input::MousePosition { x, y, width, height } => {
                self.pointer = Some((
                    (f32::from(x) / f32::from(width.max(1))).clamp(0.0, 1.0),
                    (f32::from(y) / f32::from(height.max(1))).clamp(0.0, 1.0),
                ));
            }
            Input::MouseButton { down: true, .. } => self.mouse_buttons_held += 1,
            Input::MouseButton { down: false, .. } => {
                self.mouse_buttons_held = self.mouse_buttons_held.saturating_sub(1);
            }
            Input::Scroll { .. } => {}
            Input::GamepadArrived { number: 0, .. } => self.stick = Some([0.0, 0.0]),
            Input::Gamepad { number: 0, state } => {
                self.stick = Some(state.left_stick);
                self.gamepad_button_held = state.buttons != GamepadButtons::NONE;
            }
            Input::GamepadLeft { number: 0 } => {
                self.stick = None;
                self.gamepad_button_held = false;
            }
            Input::GamepadArrived { .. } | Input::Gamepad { .. } | Input::GamepadLeft { .. } => {}
        }
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
    /// When the picture before this one was due.
    due: Option<Instant>,
}

impl Pictures for TestCard {
    fn next(&mut self, keyframe: bool, watching: Watching<'_>) -> Option<Picture<'_>> {
        // One picture every so often, as a screen has them.
        let interval = Duration::from_secs_f64(1.0 / f64::from(self.fps));
        self.due = Some(match self.due {
            Some(due) => pace(due, interval),
            None => Instant::now(),
        });
        let frame = self.draw(keyframe, watching.hands, watching.camera)?;
        Some(Picture { keyframe: frame.keyframe, data: frame.data, took: None })
    }
}

/// Sleeps until `due + interval` and returns that instant. After a stall it
/// starts counting again from now instead of rushing to catch up.
pub(crate) fn pace(due: Instant, interval: Duration) -> Instant {
    let next = due + interval;
    let now = Instant::now();
    if next > now {
        std::thread::sleep(next - now);
        next
    } else if now - next > interval * 4 {
        now
    } else {
        next
    }
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
            due: None,
        })
    }

    /// Draws and encodes the next picture, with what the client's `hands` are
    /// doing and what its `camera` sees. `keyframe` asks for a picture that
    /// can be decoded on its own.
    pub fn draw(
        &mut self,
        keyframe: bool,
        hands: Hands,
        camera: Option<&Seen>,
    ) -> Option<EncodedFrame<'_>> {
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
        if let Some(seen) = camera {
            self.draw_camera(seen);
        }
        self.draw_hands(hands);

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

impl TestCard {
    /// What the client's camera sees, in the top right corner: a quarter of
    /// the card each way at most, in the camera's own shape, as the client
    /// sent it and not as a mirror would show it.
    fn draw_camera(&mut self, seen: &Seen) {
        let (card_width, card_height) = (self.canvas.width, self.canvas.height);
        if seen.width == 0 || seen.height == 0 || seen.rgba.len() < seen.width * seen.height * 4 {
            return;
        }
        let across = card_width as f32 / 4.0 / seen.width as f32;
        let down = card_height as f32 / 4.0 / seen.height as f32;
        let scale = across.min(down);
        // Even numbers, so that colours fall on their own squares.
        let width = ((seen.width as f32 * scale) as usize & !1).max(2);
        let height = ((seen.height as f32 * scale) as usize & !1).max(2);
        let unit = (card_height / 30).max(4);
        let left = card_width.saturating_sub(width + unit) & !1;
        let top = (unit * 2) & !1;
        if left < 2 || top + height + 2 > card_height {
            return;
        }

        let frame = Yuv::from_rgb(0xF2, 0xED, 0xE4);
        self.canvas.rectangle(left - 2, top - 2, width + 4, height + 4, frame);
        let half = card_width / 2;
        for row in 0..height {
            let from = row * seen.height / height * seen.width;
            for column in 0..width {
                let at = (from + column * seen.width / width) * 4;
                let Yuv(y, u, v) =
                    Yuv::from_rgb(seen.rgba[at], seen.rgba[at + 1], seen.rgba[at + 2]);
                self.canvas.y[(top + row) * card_width + left + column] = y;
                if row % 2 == 0 && column % 2 == 0 {
                    let colour = (top + row) / 2 * half + (left + column) / 2;
                    self.canvas.u[colour] = u;
                    self.canvas.v[colour] = v;
                }
            }
        }
    }

    fn draw_hands(&mut self, hands: Hands) {
        let (width, height) = (self.canvas.width as f32, self.canvas.height as f32);
        let moon = Yuv::from_rgb(0xF2, 0xED, 0xE4);
        let sun = Yuv::from_rgb(0xFF, 0x6B, 0x35);
        let shade = Yuv::from_rgb(0x3A, 0x35, 0x32);
        let unit = (self.canvas.height / 30).max(4);

        // A square for every key held, in a row under the top left corner.
        for index in 0..hands.keys_held.min(12) as usize {
            self.canvas.rectangle(unit + index * unit * 3 / 2, unit * 2, unit, unit, moon);
        }

        // The gamepad's dial, above the colour bars on the left: the dot
        // follows the left stick and lights up while a button is held.
        if let Some([x, y]) = hands.stick {
            let radius = height / 12.0;
            let centre = (radius * 1.5, height * 0.9 - radius * 1.5);
            self.canvas.disc(centre.0, centre.1, radius, shade);
            let dot = (centre.0 + x * radius * 0.6, centre.1 - y * radius * 0.6);
            let colour = if hands.gamepad_button_held { sun } else { moon };
            self.canvas.disc(dot.0, dot.1, radius * 0.3, colour);
        }

        // The microphone's meter, above the colour bars on the right: it
        // rises with the voice.
        if let Some(voice) = hands.voice_at(Instant::now()) {
            let tall = self.canvas.height / 6;
            let (left, top) =
                (self.canvas.width - unit * 3, self.canvas.height * 9 / 10 - tall - unit);
            self.canvas.rectangle(left, top, unit, tall, shade);
            let lit = ((tall as f32 * voice.clamp(0.0, 1.0)) as usize).min(tall);
            self.canvas.rectangle(left, top + tall - lit, unit, lit, sun);
        }

        // The pointer, over everything else. It lights up while a mouse
        // button is held.
        if let Some((x, y)) = hands.pointer {
            let radius = (height / 60.0).max(3.0);
            let colour = if hands.mouse_buttons_held > 0 { sun } else { moon };
            self.canvas.disc(x * width, y * height, radius + 2.0, shade);
            self.canvas.disc(x * width, y * height, radius, colour);
        }
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
    /// When the packet before this one was due.
    due: Option<Instant>,
}

impl Packets for Melody {
    fn next(&mut self) -> Option<&[u8]> {
        self.due = Some(match self.due {
            Some(due) => pace(due, SOUND_PACKET),
            None => Instant::now(),
        });
        self.encode()
    }
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
            due: None,
        })
    }

    /// Encodes the next packet of sound.
    pub fn encode(&mut self) -> Option<&[u8]> {
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
    fn the_meter_of_the_microphone_falls_while_nothing_is_heard() {
        let heard = Instant::now();
        let hands = Hands { voice: Some(0.8), voice_heard: Some(heard), ..Hands::default() };
        assert_eq!(hands.voice_at(heard), Some(0.8));
        assert_eq!(hands.voice_at(heard + VOICE_FALLS - Duration::from_millis(1)), Some(0.8));
        assert_eq!(hands.voice_at(heard + VOICE_FALLS), Some(0.4));
        assert_eq!(hands.voice_at(heard + VOICE_FALLS * 2), Some(0.2));
        // However long the silence: nothing is left, and nothing overflows.
        let later = hands.voice_at(heard + Duration::from_secs(3600)).unwrap_or(1.0);
        assert!((0.0..1e-6).contains(&later), "{later}");
        // A clock that went back is no reason to fail.
        assert_eq!(hands.voice_at(heard - Duration::from_millis(1)), Some(0.8));
        assert_eq!(Hands::default().voice_at(heard), None);
    }

    #[test]
    fn the_card_starts_with_a_keyframe_and_makes_one_on_request() {
        let mut card = TestCard::new(320, 180, 30, 2000).expect("the encoder starts");
        let hands = Hands::default();
        assert!(card.draw(false, hands, None).expect("a first picture").keyframe);
        assert!(!card.draw(false, hands, None).expect("a second picture").keyframe);
        assert!(card.draw(true, hands, None).expect("a requested picture").keyframe);
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
            let data = card.draw(false, Hands::default(), None).expect("a picture").data.to_vec();
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
            assert_eq!(melody.encode().expect("a packet").len(), 60);
        }
    }
}
