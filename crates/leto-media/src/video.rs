//! Pictures from what a PC sends.
//!
//! H.264 is decoded everywhere on the processor, by OpenH264. On Windows,
//! Windows' own decoders also take H.265 and AV1, and decode on the
//! graphics card when it can: that is what makes those codecs worth having,
//! as the processor alone is too slow for them at the sizes of a stream.

use std::time::{Duration, Instant};

use leto_core::engine::{Codec, Decodable};
use openh264::{
    OpenH264API,
    decoder::{Decoder, DecoderConfig, Flush},
    formats::YUVSource,
};

use crate::{Error, Planes};

// Media Foundation and Direct3D are reached through their C interfaces.
#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;

/// A decoded picture: rows of pixels, four bytes each (red, green, blue and
/// an opacity that is always full).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// The codecs this device decodes. Found out once, which takes a moment on
/// Windows: best asked before a stream starts, away from the window.
pub fn decodable() -> Decodable {
    #[cfg(windows)]
    let found = windows::decodable();
    #[cfg(not(windows))]
    let found = Decodable {
        hardware: leto_core::engine::Codecs::NONE,
        any: leto_core::engine::Codecs::NONE,
    };
    Decodable { hardware: found.hardware, any: found.any.and(Decodable::SOFTWARE.any) }
}

/// How many pictures in a row Windows may fail to decode before H.264 goes
/// to OpenH264 instead, for the rest of the stream.
#[cfg(windows)]
const FAILURES_BEFORE_SOFTWARE: u32 = 3;

/// A pane to show pictures on: a window of the system's inside the app's,
/// by the handle the system gave it. With `torn`, a picture may be shown in
/// the middle of a refresh of the screen, where the system lets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pane {
    pub window: isize,
    pub torn: bool,
}

/// Decodes the pictures of one stream, in order.
pub struct VideoDecoder {
    codec: Codec,
    by: By,
    /// Pictures in a row that did not decode.
    failures: u32,
    /// A picture on its way to pixels.
    planes: Planes,
}

/// Who decodes.
enum By {
    OpenH264(Software),
    #[cfg(windows)]
    Windows(windows::Decoder),
}

impl VideoDecoder {
    /// An H.264 decoder on the processor, which works everywhere.
    pub fn new() -> Result<Self, Error> {
        Ok(Self::by(Codec::H264, By::OpenH264(Software::new()?)))
    }

    /// A decoder for a stream in `codec`, `width` by `height` at `fps`. On
    /// the graphics card when `hardware` and it can, otherwise on the
    /// processor.
    ///
    /// H.264 always has a decoder, OpenH264 when nothing better works. The
    /// other codecs only have one where [`decodable`] says so.
    pub fn for_stream(
        codec: Codec,
        width: u32,
        height: u32,
        fps: u32,
        hardware: bool,
    ) -> Result<Self, Error> {
        let decoder = Self::system(codec, width, height, fps, hardware).or_else(|error| {
            if codec == Codec::H264 {
                if let Some(error) = error {
                    log::info!("H.264 is decoded by OpenH264: {error}");
                }
                Self::new()
            } else {
                Err(error
                    .unwrap_or_else(|| Error::Video(format!("no {} decoder here", codec.name()))))
            }
        })?;
        log::info!(
            "{} is decoded on the {}",
            codec.name(),
            if decoder.on_graphics_card() { "graphics card" } else { "processor" }
        );
        Ok(decoder)
    }

    /// Windows' own decoder, when it is worth having: for H.264 only on the
    /// graphics card, as OpenH264 does as well on the processor. `None` for
    /// an error when there is no reason to try.
    #[cfg(windows)]
    fn system(
        codec: Codec,
        width: u32,
        height: u32,
        fps: u32,
        hardware: bool,
    ) -> Result<Self, Option<Error>> {
        let found = windows::decodable();
        let worth = if codec == Codec::H264 {
            hardware && found.hardware.contains(codec)
        } else {
            found.any.contains(codec)
        };
        if !worth {
            return Err(None);
        }
        let decoder = windows::Decoder::new(codec, width, height, fps, hardware).map_err(Some)?;
        Ok(Self::by(codec, By::Windows(decoder)))
    }

    #[cfg(not(windows))]
    fn system(
        _codec: Codec,
        _width: u32,
        _height: u32,
        _fps: u32,
        _hardware: bool,
    ) -> Result<Self, Option<Error>> {
        Err(None)
    }

    fn by(codec: Codec, by: By) -> Self {
        Self { codec, by, failures: 0, planes: Planes::default() }
    }

    pub const fn codec(&self) -> Codec {
        self.codec
    }

    /// How long the last picture decoded into planes spent getting out of
    /// the decoder: copied from the graphics card, for one decoded there,
    /// waiting for the card to finish it included. Nothing for a picture
    /// decoded unseen.
    pub fn copying(&self) -> Duration {
        match &self.by {
            By::OpenH264(decoder) => decoder.copying,
            #[cfg(windows)]
            By::Windows(decoder) => decoder.copying(),
        }
    }

    /// Leaves pictures on the graphics card they are decoded on from now
    /// on, for a [`crate::Card`] to draw from, or has them read back as
    /// before. Only Windows' decoders on the card do: every other picture
    /// comes in planes whatever is asked. Asked for by who shows the planes
    /// with a card that [`crate::Card::keeps`]: nothing else shows them.
    pub fn keep_on_card(&mut self, keep: bool) {
        match &mut self.by {
            By::OpenH264(_) => {
                let _ = keep;
            }
            #[cfg(windows)]
            By::Windows(decoder) => decoder.keep_on_card(keep),
        }
    }

    /// Has the graphics card that decodes the pictures show them itself,
    /// straight on `pane`, from now on, or no longer for `None`. Only
    /// Windows' decoders on the card do: every other picture comes as it
    /// did whatever is asked. A picture shown that way says so
    /// ([`Planes::shown`]) and has nothing left to show.
    pub fn show_on(&mut self, pane: Option<Pane>) {
        match &mut self.by {
            By::OpenH264(_) => {
                let _ = pane;
            }
            #[cfg(windows)]
            By::Windows(decoder) => decoder.show_on(pane),
        }
    }

    /// Whether the next picture decoded into planes is shown by the
    /// graphics card itself instead.
    pub fn shows_itself(&self) -> bool {
        match &self.by {
            By::OpenH264(_) => false,
            #[cfg(windows)]
            By::Windows(decoder) => decoder.shows(),
        }
    }

    /// Whether the next picture decoded into planes stays on the graphics
    /// card instead.
    pub fn keeps_on_card(&self) -> bool {
        match &self.by {
            By::OpenH264(_) => false,
            #[cfg(windows)]
            By::Windows(decoder) => decoder.keeps(),
        }
    }

    /// The pictures are decoded on the graphics card.
    pub fn on_graphics_card(&self) -> bool {
        match &self.by {
            By::OpenH264(_) => false,
            #[cfg(windows)]
            By::Windows(decoder) => decoder.on_graphics_card(),
        }
    }

    /// Decodes one picture (an access unit, with start codes for H.264 and
    /// H.265) into `planes`, reusing their memory, as the decoder gives it:
    /// for a graphics card to turn into pixels as it draws it.
    ///
    /// Returns `false` when the data held no picture to show. After an error
    /// the following pictures are unreliable until a keyframe arrives.
    pub fn decode_planes(&mut self, data: &[u8], planes: &mut Planes) -> Result<bool, Error> {
        let decoded = match &mut self.by {
            By::OpenH264(decoder) => decoder.decode(data, Some(planes)),
            #[cfg(windows)]
            By::Windows(decoder) => decoder.decode(data, Some(planes)),
        };
        self.count(decoded)
    }

    /// Decodes one picture into `picture`, reusing its memory, as
    /// [`Self::decode_planes`] does, then turns it into pixels here.
    ///
    /// Colours are converted as BT.601 in limited range, which is what Leto
    /// asks hosts to send.
    pub fn decode(&mut self, data: &[u8], picture: &mut Picture) -> Result<bool, Error> {
        let mut planes = std::mem::take(&mut self.planes);
        let decoded = self.decode_planes(data, &mut planes);
        let shown = match decoded {
            Ok(true) if planes.to_rgba(picture) => Ok(true),
            Ok(true) => Err(Error::Video("a decoded picture is smaller than it says".into())),
            other => other,
        };
        self.planes = planes;
        shown
    }

    /// Decodes one picture without turning it into pixels: for a picture
    /// nobody will see, which the next ones build on. Turning a picture into
    /// pixels costs about as much as decoding it.
    ///
    /// Returns `false` when the data held no picture.
    pub fn decode_unseen(&mut self, data: &[u8]) -> Result<bool, Error> {
        let decoded = match &mut self.by {
            By::OpenH264(decoder) => decoder.decode(data, None),
            #[cfg(windows)]
            By::Windows(decoder) => decoder.decode(data, None),
        };
        self.count(decoded)
    }

    /// Keeps count of the failures, and gives H.264 to OpenH264 when
    /// Windows fails it too often.
    fn count(&mut self, decoded: Result<bool, Error>) -> Result<bool, Error> {
        if decoded.is_ok() {
            self.failures = 0;
            return decoded;
        }
        self.failures += 1;
        #[cfg(windows)]
        if self.codec == Codec::H264
            && matches!(self.by, By::Windows(_))
            && self.failures >= FAILURES_BEFORE_SOFTWARE
        {
            log::warn!("Windows fails to decode H.264, so OpenH264 does from the next keyframe");
            self.by = By::OpenH264(Software::new()?);
            self.failures = 0;
        }
        decoded
    }
}

/// H.264 by OpenH264, on the processor.
struct Software {
    decoder: Decoder,
    /// How long the last picture took to copy out.
    copying: Duration,
}

impl Software {
    fn new() -> Result<Self, Error> {
        // A host sends pictures in the order they are shown, so each one can
        // come out as soon as it went in. Damage is not hidden: an error is
        // the cue to ask the host for a fresh keyframe.
        let config = DecoderConfig::new().flush_after_decode(Flush::NoFlush);
        let decoder = Decoder::with_api_config(OpenH264API::from_source(), config)
            .map_err(|error| Error::Video(error.to_string()))?;
        Ok(Self { decoder, copying: Duration::ZERO })
    }

    fn decode(&mut self, data: &[u8], planes: Option<&mut Planes>) -> Result<bool, Error> {
        self.copying = Duration::ZERO;
        let decoded = self.decoder.decode(data).map_err(|error| Error::Video(error.to_string()))?;
        let Some(decoded) = decoded else { return Ok(false) };
        if let Some(planes) = planes {
            let start = Instant::now();
            let (luma, blue, red) = decoded.strides();
            let copied = planes.copy_i420(
                decoded.dimensions(),
                [decoded.y(), decoded.u(), decoded.v()],
                [luma, blue, red],
            );
            if !copied {
                return Err(Error::Video("a decoded picture is smaller than it says".into()));
            }
            self.copying = start.elapsed();
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use openh264::{
        encoder::{Encoder, EncoderConfig, QpRange, RateControlMode},
        formats::{RgbSliceU8, YUVBuffer},
    };

    use super::*;

    const WIDTH: usize = 128;
    const HEIGHT: usize = 72;

    /// The left half in one colour, the right half in another.
    fn halves(left: [u8; 3], right: [u8; 3]) -> YUVBuffer {
        let mut rgb = Vec::with_capacity(WIDTH * HEIGHT * 3);
        for _ in 0..HEIGHT {
            for column in 0..WIDTH {
                rgb.extend(if column < WIDTH / 2 { left } else { right });
            }
        }
        YUVBuffer::from_rgb_source(RgbSliceU8::new(&rgb, (WIDTH, HEIGHT)))
    }

    fn encoder() -> Encoder {
        let config = EncoderConfig::new()
            .rate_control_mode(RateControlMode::Off)
            .qp(QpRange::new(18, 18))
            .skip_frames(false);
        Encoder::with_api_config(OpenH264API::from_source(), config).expect("the encoder starts")
    }

    fn pixel(picture: &Picture, column: usize, row: usize) -> [u8; 3] {
        let at = (row * picture.width + column) * 4;
        assert_eq!(picture.rgba[at + 3], 255, "pictures are opaque");
        [picture.rgba[at], picture.rgba[at + 1], picture.rgba[at + 2]]
    }

    fn close(found: [u8; 3], wanted: [u8; 3]) -> bool {
        found.iter().zip(wanted).all(|(found, wanted)| found.abs_diff(wanted) <= 6)
    }

    #[test]
    fn pictures_come_out_with_their_colours() {
        let (orange, cream, blue) = ([255, 107, 53], [242, 237, 228], [20, 40, 200]);
        let mut encoder = encoder();
        let mut decoder = VideoDecoder::new().expect("the decoder starts");
        let mut picture = Picture::default();

        let first = encoder.encode(&halves(orange, cream)).expect("a picture encodes").to_vec();
        assert!(decoder.decode(&first, &mut picture).expect("a keyframe decodes"));
        assert_eq!((picture.width, picture.height), (WIDTH, HEIGHT));
        assert_eq!(picture.rgba.len(), WIDTH * HEIGHT * 4);
        assert!(close(pixel(&picture, 10, 10), orange), "{:?}", pixel(&picture, 10, 10));
        assert!(close(pixel(&picture, 110, 60), cream), "{:?}", pixel(&picture, 110, 60));

        // The next picture only says what changed, and decodes all the same.
        let second = encoder.encode(&halves(blue, cream)).expect("a picture encodes").to_vec();
        assert!(decoder.decode(&second, &mut picture).expect("a later picture decodes"));
        assert!(close(pixel(&picture, 10, 10), blue), "{:?}", pixel(&picture, 10, 10));
        assert!(close(pixel(&picture, 110, 60), cream), "{:?}", pixel(&picture, 110, 60));
    }

    #[test]
    fn a_picture_decoded_unseen_is_built_on_all_the_same() {
        let (orange, cream, blue) = ([255, 107, 53], [242, 237, 228], [20, 40, 200]);
        let mut encoder = encoder();
        let mut decoder = VideoDecoder::new().expect("the decoder starts");
        let first = encoder.encode(&halves(orange, cream)).expect("a picture encodes").to_vec();
        assert!(decoder.decode_unseen(&first).expect("a keyframe decodes"));

        let mut picture = Picture::default();
        let second = encoder.encode(&halves(blue, cream)).expect("a picture encodes").to_vec();
        assert!(decoder.decode(&second, &mut picture).expect("a later picture decodes"));
        assert!(close(pixel(&picture, 10, 10), blue), "{:?}", pixel(&picture, 10, 10));
    }

    #[test]
    fn a_picture_whose_keyframe_never_came_is_an_error() {
        let mut encoder = encoder();
        let grey = [128, 128, 128];
        let _lost = encoder.encode(&halves(grey, grey)).expect("a picture encodes").to_vec();
        let later = encoder.encode(&halves([0, 0, 0], grey)).expect("a picture encodes").to_vec();

        let mut decoder = VideoDecoder::new().expect("the decoder starts");
        let mut picture = Picture::default();
        assert!(decoder.decode(&later, &mut picture).is_err());
    }

    #[test]
    fn garbage_is_an_error_or_no_picture_but_never_a_picture() {
        let mut decoder = VideoDecoder::new().expect("the decoder starts");
        let mut picture = Picture::default();
        let garbage = [0, 0, 0, 1, 0x65, 0xFF, 0x00, 0x12, 0x34, 0x56, 0x78];
        assert!(!matches!(decoder.decode(&garbage, &mut picture), Ok(true)));
    }
}
