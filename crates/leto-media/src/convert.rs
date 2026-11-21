//! Pictures as decoders give them, kept as they are for a graphics card to
//! turn into pixels, or turned into pixels here.

use crate::{Kept, Picture};

/// A decoded picture as the decoders give it, before it is turned into
/// pixels: a plane of brightness, one byte a pixel, then a plane of colour
/// at half the width and half the height, the two colour bytes of each
/// square of four pixels side by side. Rows are `stride` bytes apart in
/// both planes, the width made even. Colours are BT.601 in limited range,
/// which is what Leto asks hosts to send.
///
/// That is less than half the bytes of the pixels, and a graphics card
/// turns it into pixels as it draws it for next to nothing.
///
/// A picture that stayed on the graphics card it was decoded on has its
/// size and no planes: it is [`Self::kept`], and only a [`crate::Card`]
/// shows it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Planes {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub luma: Vec<u8>,
    pub chroma: Vec<u8>,
    /// Where the picture is, when it never left the graphics card.
    pub kept: Option<Kept>,
    /// The decoder showed the picture itself, straight on the screen:
    /// there is nothing of it to show, only its size.
    pub shown: bool,
}

impl Planes {
    /// Width and height of the plane of colour.
    pub const fn chroma_size(&self) -> (usize, usize) {
        (self.width.div_ceil(2), self.height.div_ceil(2))
    }

    /// Turns the picture into `picture`, reusing its memory.
    pub fn to_rgba(&self, picture: &mut Picture) -> bool {
        nv12_to_rgba(&self.as_nv12(), picture)
    }

    fn as_nv12(&self) -> Nv12<'_> {
        Nv12 {
            width: self.width,
            height: self.height,
            stride: self.stride,
            luma: &self.luma,
            chroma: &self.chroma,
        }
    }

    /// Makes room for a picture `width` by `height`, reusing the memory.
    fn start(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.stride = width.div_ceil(2) * 2;
        self.luma.clear();
        self.chroma.clear();
        self.kept = None;
        self.shown = false;
        self.luma.reserve(self.stride * height);
        self.chroma.reserve(self.stride * height.div_ceil(2));
    }

    /// Says the picture, `width` by `height`, stayed on the graphics card:
    /// there are no planes of it.
    // Only Windows keeps pictures there.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn keep(&mut self, kept: Kept, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.stride = 0;
        self.luma.clear();
        self.chroma.clear();
        self.kept = Some(kept);
        self.shown = false;
    }

    /// Says the picture, `width` by `height`, was shown by its decoder:
    /// there are no planes of it.
    // Only Windows shows pictures that way.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn show(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.stride = 0;
        self.luma.clear();
        self.chroma.clear();
        self.kept = None;
        self.shown = true;
    }

    /// Copies `source`, leaving out the padding of its rows. Returns `false`,
    /// and leaves the planes alone, when `source` is too small for the size
    /// it claims.
    pub fn copy_nv12(&mut self, source: &Nv12<'_>) -> bool {
        if !source.fits() {
            return false;
        }
        self.start(source.width, source.height);
        let (stride, width) = (source.stride, self.stride);
        for row in 0..source.height {
            let start = row * stride;
            self.luma.extend_from_slice(&source.luma[start..start + source.width]);
            // An odd width gets one more byte, so rows line up with colour.
            self.luma.resize((row + 1) * width, 16);
        }
        for row in 0..source.height.div_ceil(2) {
            let start = row * stride;
            self.chroma.extend_from_slice(&source.chroma[start..start + width]);
        }
        true
    }

    /// Copies a picture in I420, as OpenH264 gives it: the two colours in
    /// planes of their own, rows `strides` bytes apart, brightness first.
    /// Returns `false`, and leaves the planes alone, when a plane is too
    /// small for the size.
    pub fn copy_i420(
        &mut self,
        (width, height): (usize, usize),
        [luma, blue, red]: [&[u8]; 3],
        [luma_stride, blue_stride, red_stride]: [usize; 3],
    ) -> bool {
        let (colour_width, colour_height) = (width.div_ceil(2), height.div_ceil(2));
        let fits = |plane: &[u8], stride: usize, rows: usize, row: usize| {
            rows > 0 && stride >= row && plane.len() >= stride * (rows - 1) + row
        };
        if width == 0
            || !fits(luma, luma_stride, height, width)
            || !fits(blue, blue_stride, colour_height, colour_width)
            || !fits(red, red_stride, colour_height, colour_width)
        {
            return false;
        }
        self.start(width, height);
        let stride = self.stride;
        for row in 0..height {
            let start = row * luma_stride;
            self.luma.extend_from_slice(&luma[start..start + width]);
            self.luma.resize((row + 1) * stride, 16);
        }
        for row in 0..colour_height {
            let blue = &blue[row * blue_stride..row * blue_stride + colour_width];
            let red = &red[row * red_stride..row * red_stride + colour_width];
            self.chroma.extend(blue.iter().zip(red).flat_map(|(&blue, &red)| [blue, red]));
        }
        true
    }
}

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
        let mut planes = Planes::default();
        assert!(!planes.copy_nv12(&short));
        assert!(!planes.copy_i420((8, 9), [&luma, &chroma, &chroma], [8, 4, 4]));
        assert_eq!(planes, Planes::default());
    }

    #[test]
    fn planes_keep_a_picture_without_its_padding_and_give_the_same_pixels() {
        // Three pixels wide, rows of 4 bytes, as in the test above.
        let luma = [126, 126, 85, 0, 126, 126, 85, 0];
        let chroma = [128, 128, 103, 203];
        let source = Nv12 { width: 3, height: 2, stride: 4, luma: &luma, chroma: &chroma };
        let mut planes = Planes::default();
        assert!(planes.copy_nv12(&source));
        // The width made even: one byte more on each row of brightness.
        assert_eq!((planes.width, planes.height, planes.stride), (3, 2, 4));
        assert_eq!(planes.luma, [126, 126, 85, 16, 126, 126, 85, 16]);
        assert_eq!(planes.chroma, chroma);
        assert_eq!(planes.chroma_size(), (2, 1));
        let (mut direct, mut through) = (Picture::default(), Picture::default());
        assert!(nv12_to_rgba(&source, &mut direct));
        assert!(planes.to_rgba(&mut through));
        assert_eq!(direct, through);
    }

    #[test]
    fn colours_in_planes_of_their_own_end_up_side_by_side() {
        // Four by two, rows padded to 6 and 3 bytes.
        let luma = [1, 2, 3, 4, 0, 0, 5, 6, 7, 8, 0, 0];
        let (blue, red) = ([10, 11, 0], [20, 21, 0]);
        let mut planes = Planes::default();
        assert!(planes.copy_i420((4, 2), [&luma, &blue, &red], [6, 3, 3]));
        assert_eq!((planes.width, planes.height, planes.stride), (4, 2, 4));
        assert_eq!(planes.luma, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(planes.chroma, [10, 20, 11, 21]);
        // The memory is reused for the next picture, of another size.
        assert!(planes.copy_i420((2, 2), [&luma, &blue, &red], [6, 3, 3]));
        assert_eq!(planes.luma, [1, 2, 5, 6]);
        assert_eq!(planes.chroma, [10, 20]);
    }
}
