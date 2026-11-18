//! Pictures as decoders give them, turned into pixels a window shows.

use crate::Picture;

/// A picture in NV12, as decoders on the graphics card give it: a plane of
/// brightness, one byte a pixel, then a plane of colour at half the width
/// and half the height, the two colour bytes of each pair of pixels side by
/// side. Rows are `stride` bytes apart in both planes.
pub struct Nv12<'a> {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub luma: &'a [u8],
    pub chroma: &'a [u8],
}

impl Nv12<'_> {
    /// Whether the planes hold as many rows of as many bytes as the size
    /// says.
    pub fn fits(&self) -> bool {
        // A pair of colour bytes covers two pixels, the last one too.
        let colour_row = self.width.div_ceil(2) * 2;
        let rows =
            |plane: &[u8], count: usize, row: usize| plane.len() >= self.stride * (count - 1) + row;
        self.width > 0
            && self.height > 0
            && self.stride >= colour_row
            && rows(self.luma, self.height, self.width)
            && rows(self.chroma, self.height.div_ceil(2), colour_row)
    }
}

/// Turns `source` into `picture`, reusing its memory. Colours are read as
/// BT.601 in limited range, which is what Leto asks hosts to send. Returns
/// `false`, and leaves `picture` alone, when the planes are too small for
/// the size they claim.
pub fn nv12_to_rgba(source: &Nv12<'_>, picture: &mut Picture) -> bool {
    if !source.fits() {
        return false;
    }
    let (width, height, stride) = (source.width, source.height, source.stride);
    picture.width = width;
    picture.height = height;
    picture.rgba.resize(width * height * 4, 0);
    for (row, out) in picture.rgba.chunks_exact_mut(width * 4).enumerate() {
        let luma = &source.luma[row * stride..row * stride + width];
        let colours = (row / 2) * stride;
        let chroma = &source.chroma[colours..colours + width.div_ceil(2) * 2];
        // Two pixels share a colour, worked out once for both.
        let pairs = out.chunks_mut(8).zip(luma.chunks(2)).zip(chroma.chunks_exact(2));
        for ((pixels, brights), colour) in pairs {
            let tint = Tint::new(colour[0], colour[1]);
            for (pixel, &bright) in pixels.chunks_exact_mut(4).zip(brights) {
                pixel.copy_from_slice(&tint.rgba(bright));
            }
        }
    }
    true
}

/// What a colour adds to the red, the green and the blue of a pixel, from
/// BT.601 in limited range, in whole numbers: 409 is 1.596 in 256ths, and so
/// on.
struct Tint {
    red: i32,
    green: i32,
    blue: i32,
}

impl Tint {
    fn new(u: u8, v: u8) -> Self {
        let (d, e) = (i32::from(u) - 128, i32::from(v) - 128);
        Self { red: 409 * e, green: -100 * d - 208 * e, blue: 516 * d }
    }

    /// A pixel of this colour and of brightness `y`.
    fn rgba(&self, y: u8) -> [u8; 4] {
        // 298 is 1.164 in 256ths: limited range spreads 16 to 235 over all.
        let c = 298 * (i32::from(y) - 16) + 128;
        let clamp = |value: i32| (value >> 8).clamp(0, 255) as u8;
        [clamp(c + self.red), clamp(c + self.green), clamp(c + self.blue), 255]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same colour all over, in BT.601 limited range.
    fn plain(height: usize, stride: usize, [y, u, v]: [u8; 3]) -> (Vec<u8>, Vec<u8>) {
        let luma = vec![y; stride * height];
        let mut chroma = vec![0; stride * height.div_ceil(2)];
        for pair in chroma.chunks_exact_mut(2) {
            pair.copy_from_slice(&[u, v]);
        }
        (luma, chroma)
    }

    #[test]
    fn colours_come_out_as_bt_601_says() {
        for (yuv, rgb) in [
            ([16, 128, 128], [0, 0, 0]),
            ([235, 128, 128], [255, 255, 255]),
            // Leto's orange, and a blue, as an encoder writes them.
            ([141, 82, 197], [255, 107, 53]),
            ([61, 201, 108], [20, 40, 200]),
        ] {
            let (luma, chroma) = plain(4, 8, yuv);
            let source = Nv12 { width: 6, height: 4, stride: 8, luma: &luma, chroma: &chroma };
            let mut picture = Picture::default();
            assert!(nv12_to_rgba(&source, &mut picture));
            assert_eq!((picture.width, picture.height, picture.rgba.len()), (6, 4, 96));
            for pixel in picture.rgba.chunks_exact(4) {
                let close =
                    pixel[..3].iter().zip(rgb).all(|(found, wanted)| found.abs_diff(wanted) <= 2);
                assert!(close && pixel[3] == 255, "{yuv:?} gave {pixel:?}, not {rgb:?}");
            }
        }
    }

    #[test]
    fn each_pair_of_pixels_takes_its_own_colour_and_padding_is_left_out() {
        // Three pixels wide and two rows, in rows of 4 bytes: a grey pair,
        // then a red one cut in half by the edge.
        let luma = [126, 126, 85, 0, 126, 126, 85, 0];
        let chroma = [128, 128, 103, 203];
        let source = Nv12 { width: 3, height: 2, stride: 4, luma: &luma, chroma: &chroma };
        let mut picture = Picture::default();
        assert!(nv12_to_rgba(&source, &mut picture));
        assert_eq!(picture.rgba.len(), 3 * 2 * 4);
        assert_eq!(&picture.rgba[..8], &[128, 128, 128, 255, 128, 128, 128, 255]);
        assert_eq!(&picture.rgba[8..12], &[200, 29, 30, 255]);
        assert_eq!(&picture.rgba[12..16], &[128, 128, 128, 255]);
    }

    #[test]
    fn planes_too_small_for_their_size_are_refused() {
        let (luma, chroma) = plain(8, 8, [16, 128, 128]);
        let mut picture = Picture::default();
        let short = Nv12 { width: 8, height: 9, stride: 8, luma: &luma, chroma: &chroma };
        assert!(!nv12_to_rgba(&short, &mut picture));
        let narrow = Nv12 { width: 9, height: 8, stride: 8, luma: &luma, chroma: &chroma };
        assert!(!nv12_to_rgba(&narrow, &mut picture));
        assert_eq!(picture, Picture::default());
    }
}
