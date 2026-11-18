//! H.264 to pixels, in software.

use openh264::{
    OpenH264API,
    decoder::{Decoder, DecoderConfig, Flush},
    formats::YUVSource,
};

use crate::Error;

/// A decoded picture: rows of pixels, four bytes each (red, green, blue and
/// an opacity that is always full).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// Decodes the pictures of one stream, in order.
pub struct VideoDecoder {
    decoder: Decoder,
}

impl VideoDecoder {
    pub fn new() -> Result<Self, Error> {
        // A host sends pictures in the order they are shown, so each one can
        // come out as soon as it went in. Damage is not hidden: an error is
        // the cue to ask the host for a fresh keyframe.
        let config = DecoderConfig::new().flush_after_decode(Flush::NoFlush);
        let decoder = Decoder::with_api_config(OpenH264API::from_source(), config)
            .map_err(|error| Error::Video(error.to_string()))?;
        Ok(Self { decoder })
    }

    /// Decodes one picture (an H.264 access unit with start codes) into
    /// `picture`, reusing its memory.
    ///
    /// Returns `false` when the data held no picture to show. After an error
    /// the following pictures are unreliable until a keyframe arrives.
    ///
    /// Colours are converted as BT.601 in limited range, which is what Leto
    /// asks hosts to send.
    pub fn decode(&mut self, data: &[u8], picture: &mut Picture) -> Result<bool, Error> {
        let decoded = self.decoder.decode(data).map_err(|error| Error::Video(error.to_string()))?;
        let Some(decoded) = decoded else { return Ok(false) };
        let (width, height) = decoded.dimensions();
        picture.width = width;
        picture.height = height;
        picture.rgba.resize(width * height * 4, 0);
        decoded.write_rgba8(&mut picture.rgba);
        Ok(true)
    }

    /// Decodes one picture without turning it into pixels: for a picture
    /// nobody will see, which the next ones build on. Turning a picture into
    /// pixels costs about as much as decoding it.
    ///
    /// Returns `false` when the data held no picture.
    pub fn decode_unseen(&mut self, data: &[u8]) -> Result<bool, Error> {
        let decoded = self.decoder.decode(data).map_err(|error| Error::Video(error.to_string()))?;
        Ok(decoded.is_some())
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
