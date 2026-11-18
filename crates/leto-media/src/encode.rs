//! Pixels to H.264, in software: how the pictures of a camera leave.
//!
//! A host shows them on a PC as a webcam's. They are sent small enough for a
//! home connection and for the host, and with a key frame every two seconds,
//! since nothing that is lost on the way is sent again.

use openh264::{
    OpenH264API,
    encoder::{
        BitRate, Complexity, Encoder, EncoderConfig, FrameRate, FrameType, IntraFramePeriod,
        RateControlMode, VuiConfig,
    },
    formats::{RgbaSliceU8, YUVBuffer},
};

use crate::{Error, Picture};

/// A key frame this often, in seconds. A host that lost a picture shows the
/// camera again that soon.
const KEY_FRAME_EVERY: u32 = 2;

/// What 1280 by 720 at thirty pictures a second gets, in kilobits a second.
/// Other sizes and paces get as much per pixel.
const BITRATE: u64 = 2500;

/// No camera is sent with less, or with more, in kilobits a second.
const BITRATE_RANGE: (u64, u64) = (300, 8000);

/// The smallest picture worth sending, each way.
const SMALLEST: usize = 32;

/// How often a picture may be halved to fit. A camera sixteen times too
/// large each way does not exist.
const HALVINGS: u32 = 4;

/// How the pictures of a camera are sent: their size, their pace and how
/// much the encoder may spend on them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sending {
    pub width: u32,
    pub height: u32,
    /// Pictures a second.
    pub fps: u32,
    pub bitrate_kbps: u32,
}

impl Sending {
    /// How pictures of `width` by `height`, taken `fps` times a second, are
    /// sent to a host that takes nothing larger than `largest` nor faster
    /// than `fastest`. They are halved until they fit, and an odd size
    /// loses its last row or column. `None` for a picture that is too small
    /// to send, or that halving does not bring down to what the host takes.
    pub fn fit(
        width: usize,
        height: usize,
        fps: u32,
        largest: (u32, u32),
        fastest: u32,
    ) -> Option<Self> {
        let halvings = (0..=HALVINGS).find(|halvings| {
            (width >> halvings) <= largest.0 as usize && (height >> halvings) <= largest.1 as usize
        })?;
        let (width, height) = halved(width, height, halvings);
        let fps = fps.min(fastest);
        if width < SMALLEST || height < SMALLEST || fps == 0 {
            return None;
        }
        let pixels = (width * height) as u64 * u64::from(fps);
        let bitrate = BITRATE * pixels / (1280 * 720 * 30);
        Some(Self {
            width: width as u32,
            height: height as u32,
            fps,
            bitrate_kbps: bitrate.clamp(BITRATE_RANGE.0, BITRATE_RANGE.1) as u32,
        })
    }
}

/// The size of a picture halved `halvings` times, even each way.
fn halved(width: usize, height: usize, halvings: u32) -> (usize, usize) {
    ((width >> halvings) & !1, (height >> halvings) & !1)
}

/// One encoded picture.
#[derive(Debug, Clone, Copy)]
pub struct Encoded<'a> {
    /// It decodes on its own.
    pub key_frame: bool,
    /// An H.264 access unit with start codes.
    pub data: &'a [u8],
}

/// Encodes the pictures of one camera, in order.
pub struct PictureEncoder {
    encoder: Encoder,
    sending: Sending,
    /// The picture as the encoder wants it.
    planes: YUVBuffer,
    /// A picture brought down to the size that is sent.
    fitted: Vec<u8>,
    data: Vec<u8>,
    /// The first picture went in: it is a key frame without being asked.
    started: bool,
}

impl PictureEncoder {
    pub fn new(sending: Sending) -> Result<Self, Error> {
        let (width, height) = (sending.width as usize, sending.height as usize);
        if width % 2 != 0 || height % 2 != 0 || width < SMALLEST || height < SMALLEST {
            return Err(Error::Video(format!("{width} by {height} is no size to encode at")));
        }
        // What cameras show is what this encoder is tuned for by default.
        // It keeps to the bit rate by spending less on each picture, and by
        // leaving one out when that is not enough, which it needs to be
        // allowed to. The colours are said to be what they are, BT.601 in
        // limited range.
        let config = EncoderConfig::new()
            .bitrate(BitRate::from_bps(sending.bitrate_kbps.saturating_mul(1000)))
            .max_frame_rate(FrameRate::from_hz(sending.fps as f32))
            .rate_control_mode(RateControlMode::Bitrate)
            .complexity(Complexity::Low)
            .intra_frame_period(IntraFramePeriod::from_num_frames(sending.fps * KEY_FRAME_EVERY))
            .vui(VuiConfig::bt601());
        let encoder = Encoder::with_api_config(OpenH264API::from_source(), config)
            .map_err(|error| Error::Video(error.to_string()))?;
        Ok(Self {
            encoder,
            sending,
            planes: YUVBuffer::new(width, height),
            fitted: Vec::new(),
            data: Vec::new(),
            started: false,
        })
    }

    /// How this encoder sends.
    pub fn sending(&self) -> Sending {
        self.sending
    }

    /// Encodes the next picture. With `key_frame` it is made to decode on
    /// its own, as the first one and one every two seconds are anyway.
    /// `None` when the encoder left it out to keep to its bit rate.
    ///
    /// The picture has the size this encoder was made for, give or take the
    /// halving and the odd row that [`Sending::fit`] takes off. One of
    /// another size is an error: its camera changed, and needs a new
    /// encoder.
    pub fn encode(
        &mut self,
        picture: &Picture,
        key_frame: bool,
    ) -> Result<Option<Encoded<'_>>, Error> {
        let (width, height) = (self.sending.width as usize, self.sending.height as usize);
        if picture.rgba.len() != picture.width * picture.height * 4 {
            return Err(Error::Video("a picture of the camera is incomplete".to_owned()));
        }
        let halvings = (0..=HALVINGS)
            .find(|halvings| halved(picture.width, picture.height, *halvings) == (width, height))
            .ok_or_else(|| {
                Error::Video(format!(
                    "the camera went from {width} by {height} to {} by {}",
                    picture.width, picture.height
                ))
            })?;
        let pixels = if halvings == 0 && (picture.width, picture.height) == (width, height) {
            &picture.rgba
        } else {
            shrink(picture, halvings, (width, height), &mut self.fitted);
            &self.fitted
        };
        self.planes.read_rgba8(RgbaSliceU8::new(pixels, (width, height)));

        if key_frame && self.started {
            self.encoder.force_intra_frame();
        }
        self.started = true;
        let encoded =
            self.encoder.encode(&self.planes).map_err(|error| Error::Video(error.to_string()))?;
        let kind = encoded.frame_type();
        if matches!(kind, FrameType::Skip | FrameType::Invalid) {
            return Ok(None);
        }
        self.data.clear();
        encoded.write_vec(&mut self.data);
        Ok(Some(Encoded {
            key_frame: matches!(kind, FrameType::IDR | FrameType::I),
            data: &self.data,
        }))
    }
}

/// Brings `picture` down to `size` by halving it `halvings` times: every
/// pixel that is left is the mean of the ones it stands for.
fn shrink(picture: &Picture, halvings: u32, size: (usize, usize), out: &mut Vec<u8>) {
    let (width, height) = size;
    let step = 1usize << halvings;
    let area = (step * step) as u32;
    out.resize(width * height * 4, 255);
    for (row, line) in out.chunks_exact_mut(width * 4).enumerate() {
        for (column, pixel) in line.chunks_exact_mut(4).enumerate() {
            let mut sum = [0u32; 3];
            for below in 0..step {
                let from = ((row * step + below) * picture.width + column * step) * 4;
                for source in picture.rgba[from..from + step * 4].chunks_exact(4) {
                    for (sum, value) in sum.iter_mut().zip(source) {
                        *sum += u32::from(*value);
                    }
                }
            }
            for (value, sum) in pixel.iter_mut().zip(sum) {
                *value = (sum / area) as u8;
            }
            pixel[3] = 255;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::VideoDecoder;

    /// The left half in one colour, the right half in another.
    fn halves(width: usize, height: usize, left: [u8; 3], right: [u8; 3]) -> Picture {
        let mut rgba = Vec::with_capacity(width * height * 4);
        for _ in 0..height {
            for column in 0..width {
                let [red, green, blue] = if column < width / 2 { left } else { right };
                rgba.extend([red, green, blue, 255]);
            }
        }
        Picture { width, height, rgba }
    }

    fn pixel(picture: &Picture, column: usize, row: usize) -> [u8; 3] {
        let at = (row * picture.width + column) * 4;
        [picture.rgba[at], picture.rgba[at + 1], picture.rgba[at + 2]]
    }

    fn close(found: [u8; 3], wanted: [u8; 3]) -> bool {
        found.iter().zip(wanted).all(|(found, wanted)| found.abs_diff(wanted) <= 8)
    }

    const ORANGE: [u8; 3] = [255, 107, 53];
    const CREAM: [u8; 3] = [242, 237, 228];
    const BLUE: [u8; 3] = [20, 40, 200];

    fn sending(width: usize, height: usize) -> Sending {
        Sending::fit(width, height, 30, (1920, 1080), 30).expect("a size that can be sent")
    }

    #[test]
    fn what_the_camera_saw_decodes_to_the_same_colours() {
        let mut encoder = PictureEncoder::new(sending(320, 180)).expect("the encoder starts");
        let mut decoder = VideoDecoder::new().expect("the decoder starts");
        let mut shown = Picture::default();

        let first = halves(320, 180, ORANGE, CREAM);
        let encoded = encoder.encode(&first, false).expect("it encodes").expect("a picture");
        assert!(encoded.key_frame, "the first picture decodes on its own");
        assert!(decoder.decode(encoded.data, &mut shown).expect("it decodes"));
        assert_eq!((shown.width, shown.height), (320, 180));
        assert!(close(pixel(&shown, 20, 20), ORANGE), "{:?}", pixel(&shown, 20, 20));
        assert!(close(pixel(&shown, 300, 160), CREAM), "{:?}", pixel(&shown, 300, 160));

        let second = halves(320, 180, BLUE, CREAM);
        let encoded = encoder.encode(&second, false).expect("it encodes").expect("a picture");
        assert!(!encoded.key_frame);
        assert!(decoder.decode(encoded.data, &mut shown).expect("it decodes"));
        assert!(close(pixel(&shown, 20, 20), BLUE), "{:?}", pixel(&shown, 20, 20));
    }

    #[test]
    fn key_frames_come_first_when_asked_for_and_every_two_seconds() {
        let mut encoder = PictureEncoder::new(sending(160, 90)).expect("the encoder starts");
        let mut key_frames = Vec::new();
        for number in 0..130u32 {
            // Something moves, so that every picture is worth encoding.
            let shade = (number * 2 % 256) as u8;
            let picture = halves(160, 90, [shade, 107, 53], CREAM);
            let encoded =
                encoder.encode(&picture, number == 20).expect("it encodes").expect("a picture");
            if encoded.key_frame {
                key_frames.push(number);
                // A host that only just started listening can show it.
                let mut decoder = VideoDecoder::new().expect("the decoder starts");
                let mut shown = Picture::default();
                assert!(decoder.decode(encoded.data, &mut shown).expect("it decodes alone"));
            }
        }
        assert_eq!(key_frames, [0, 20, 80]);
    }

    #[test]
    fn a_picture_too_large_for_the_host_is_halved_until_it_fits() {
        let fitted = Sending::fit(320, 180, 30, (100, 100), 30).expect("it fits when halved");
        assert_eq!((fitted.width, fitted.height), (80, 44));
        let mut encoder = PictureEncoder::new(fitted).expect("the encoder starts");
        let mut decoder = VideoDecoder::new().expect("the decoder starts");
        let mut shown = Picture::default();

        let picture = halves(320, 180, ORANGE, BLUE);
        let encoded = encoder.encode(&picture, false).expect("it encodes").expect("a picture");
        assert!(decoder.decode(encoded.data, &mut shown).expect("it decodes"));
        assert_eq!((shown.width, shown.height), (80, 44));
        assert!(close(pixel(&shown, 10, 10), ORANGE), "{:?}", pixel(&shown, 10, 10));
        assert!(close(pixel(&shown, 70, 34), BLUE), "{:?}", pixel(&shown, 70, 34));
    }

    #[test]
    fn an_odd_size_loses_its_last_row_and_column() {
        let fitted = sending(161, 91);
        assert_eq!((fitted.width, fitted.height), (160, 90));
        let mut encoder = PictureEncoder::new(fitted).expect("the encoder starts");
        let mut decoder = VideoDecoder::new().expect("the decoder starts");
        let mut shown = Picture::default();

        let picture = halves(161, 91, CREAM, BLUE);
        let encoded = encoder.encode(&picture, false).expect("it encodes").expect("a picture");
        assert!(decoder.decode(encoded.data, &mut shown).expect("it decodes"));
        assert_eq!((shown.width, shown.height), (160, 90));
        assert!(close(pixel(&shown, 10, 80), CREAM), "{:?}", pixel(&shown, 10, 80));
        assert!(close(pixel(&shown, 150, 10), BLUE), "{:?}", pixel(&shown, 150, 10));
    }

    #[test]
    fn a_camera_that_changes_its_size_needs_a_new_encoder() {
        let mut encoder = PictureEncoder::new(sending(320, 180)).expect("the encoder starts");
        assert!(encoder.encode(&halves(320, 180, ORANGE, CREAM), false).is_ok());
        // Twice as large is the same picture, halved. Anything else is not.
        assert!(encoder.encode(&halves(640, 360, ORANGE, CREAM), false).is_ok());
        assert!(encoder.encode(&halves(320, 240, ORANGE, CREAM), false).is_err());
        assert!(encoder.encode(&halves(180, 320, ORANGE, CREAM), false).is_err());
        let cut_short = Picture { width: 320, height: 180, rgba: vec![0; 100] };
        assert!(encoder.encode(&cut_short, false).is_err());
    }

    #[test]
    fn a_camera_is_sent_as_large_and_as_fast_as_the_host_takes() {
        let fit = |width, height, fps, largest, fastest| {
            Sending::fit(width, height, fps, largest, fastest)
                .map(|sending| (sending.width, sending.height, sending.fps, sending.bitrate_kbps))
        };
        let full_hd = (1920, 1080);
        // What Leto captures at when the camera has it.
        assert_eq!(fit(1280, 720, 30, full_hd, 30), Some((1280, 720, 30, 2500)));
        assert_eq!(fit(640, 480, 30, full_hd, 30), Some((640, 480, 30, 833)));
        assert_eq!(fit(1920, 1080, 30, full_hd, 30), Some((1920, 1080, 30, 5625)));
        // A host that takes less gets less.
        assert_eq!(fit(1920, 1080, 30, (1280, 720), 30), Some((960, 540, 30, 1406)));
        assert_eq!(fit(3840, 2160, 60, (1280, 720), 30), Some((960, 540, 30, 1406)));
        assert_eq!(fit(1280, 720, 30, full_hd, 15), Some((1280, 720, 15, 1250)));
        // The bit rate stays within what a camera is worth.
        assert_eq!(fit(160, 120, 10, full_hd, 30), Some((160, 120, 10, 300)));
        assert_eq!(fit(3840, 2160, 60, (3840, 2160), 60), Some((3840, 2160, 60, 8000)));
        // Nothing to send.
        assert_eq!(fit(0, 0, 30, full_hd, 30), None);
        assert_eq!(fit(1280, 720, 0, full_hd, 30), None);
        assert_eq!(fit(1280, 720, 30, (16, 16), 30), None);
        assert_eq!(fit(100_000, 100_000, 30, (1280, 720), 30), None);
    }
}
