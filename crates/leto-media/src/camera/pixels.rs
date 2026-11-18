//! What a camera hands over, turned into the pixels the app shows.
//!
//! Cameras deliver their pictures compressed as JPEG, or as luminance with
//! the colour at half the detail (YUYV, NV12), rarely as plain grey.

use std::borrow::Cow;

use crate::{Error, Picture};

/// How the bytes of a picture from a camera are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// Where Leto reads no camera of the system yet, only the test card makes
// pictures, in one layout.
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
pub(super) enum Layout {
    /// A JPEG file. Its size is its own.
    Jpeg,
    /// Two pixels in four bytes: their two luminances, and one colour
    /// between them (Y, U, Y, V).
    Yuyv,
    /// A plane of luminance, then one of colour at half the size each way
    /// (U and V in turn).
    Nv12,
    /// One byte of luminance per pixel.
    Gray,
}

/// One picture as a camera hands it over.
#[derive(Debug, Clone)]
pub(super) struct Raw<'a> {
    pub layout: Layout,
    pub width: usize,
    pub height: usize,
    pub data: Cow<'a, [u8]>,
}

/// Turns `raw` into `picture`, reusing its memory.
///
/// Colours are read as BT.601 in limited range, which is what cameras send
/// when they do not say.
pub(super) fn read(raw: &Raw<'_>, picture: &mut Picture) -> Result<(), Error> {
    let data: &[u8] = &raw.data;
    if raw.layout == Layout::Jpeg {
        return jpeg(data, picture);
    }
    let (width, height) = (raw.width, raw.height);
    if width == 0 || height == 0 {
        return Err(cut_short());
    }
    picture.width = width;
    picture.height = height;
    picture.rgba.resize(width * height * 4, 255);
    match raw.layout {
        Layout::Yuyv => {
            // Some systems pad each row: the rows share what there is.
            let stride = row_length(data.len(), height, width.div_ceil(2) * 4)?;
            for (row, pixels) in picture.rgba.chunks_exact_mut(width * 4).enumerate() {
                let line = &data[row * stride..][..stride];
                for (pair, pixels) in pixels.chunks_mut(8).enumerate() {
                    let [y0, u, y1, v] = [0, 1, 2, 3].map(|byte| line[pair * 4 + byte]);
                    for (pixel, y) in pixels.chunks_exact_mut(4).zip([y0, y1]) {
                        pixel.copy_from_slice(&colour(y, u, v));
                    }
                }
            }
        }
        Layout::Nv12 => {
            let rows = height + height.div_ceil(2);
            let stride = row_length(data.len(), rows, width.next_multiple_of(2))?;
            let (luminance, colours) = data.split_at(stride * height);
            for (row, pixels) in picture.rgba.chunks_exact_mut(width * 4).enumerate() {
                let line = &luminance[row * stride..][..width];
                let shared = &colours[(row / 2) * stride..][..width.next_multiple_of(2)];
                for (column, pixel) in pixels.chunks_exact_mut(4).enumerate() {
                    let pair = column / 2 * 2;
                    pixel.copy_from_slice(&colour(line[column], shared[pair], shared[pair + 1]));
                }
            }
        }
        Layout::Gray => {
            let stride = row_length(data.len(), height, width)?;
            for (row, pixels) in picture.rgba.chunks_exact_mut(width * 4).enumerate() {
                let line = &data[row * stride..][..width];
                for (pixel, y) in pixels.chunks_exact_mut(4).zip(line) {
                    pixel.copy_from_slice(&[*y, *y, *y, 255]);
                }
            }
        }
        Layout::Jpeg => {}
    }
    Ok(())
}

/// How many bytes each of `rows` rows takes in a picture of `length`
/// bytes, when that is at least the `needed` ones.
fn row_length(length: usize, rows: usize, needed: usize) -> Result<usize, Error> {
    let stride = length / rows;
    if stride >= needed && needed > 0 { Ok(stride) } else { Err(cut_short()) }
}

fn cut_short() -> Error {
    Error::Camera("it sent a picture that is cut short".to_owned())
}

/// One pixel from its luminance and its colour, BT.601 in limited range.
fn colour(y: u8, u: u8, v: u8) -> [u8; 4] {
    let luminance = 298 * (i32::from(y) - 16) + 128;
    let (blue, red) = (i32::from(u) - 128, i32::from(v) - 128);
    let channel = |value: i32| (value >> 8).clamp(0, 255) as u8;
    [
        channel(luminance + 409 * red),
        channel(luminance - 100 * blue - 208 * red),
        channel(luminance + 516 * blue),
        255,
    ]
}

fn jpeg(data: &[u8], picture: &mut Picture) -> Result<(), Error> {
    let decoded = image::load_from_memory_with_format(data, image::ImageFormat::Jpeg)
        .map_err(|error| Error::Camera(format!("it sent a picture that cannot be read: {error}")))?
        .into_rgba8();
    picture.width = decoded.width() as usize;
    picture.height = decoded.height() as usize;
    picture.rgba = decoded.into_raw();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_new(layout: Layout, width: usize, height: usize, data: &[u8]) -> Picture {
        let mut picture = Picture::default();
        read(&Raw { layout, width, height, data: data.into() }, &mut picture)
            .expect("a whole picture");
        picture
    }

    fn pixel(picture: &Picture, column: usize, row: usize) -> [u8; 4] {
        let at = (row * picture.width + column) * 4;
        [0, 1, 2, 3].map(|byte| picture.rgba[at + byte])
    }

    /// Black, white, and the red, green and blue of BT.601 in limited range.
    const BLACK: (u8, u8, u8) = (16, 128, 128);
    const WHITE: (u8, u8, u8) = (235, 128, 128);
    const RED: (u8, u8, u8) = (81, 90, 240);
    const GREEN: (u8, u8, u8) = (145, 54, 34);
    const BLUE: (u8, u8, u8) = (41, 240, 110);

    fn close(pixel: [u8; 4], wanted: [u8; 3]) -> bool {
        pixel[3] == 255 && pixel.iter().zip(wanted).all(|(got, want)| got.abs_diff(want) <= 2)
    }

    #[test]
    fn the_colours_of_television_come_out_as_the_colours_of_screens() {
        for ((y, u, v), wanted) in [
            (BLACK, [0, 0, 0]),
            (WHITE, [255, 255, 255]),
            (RED, [255, 0, 0]),
            (GREEN, [0, 255, 0]),
            (BLUE, [0, 0, 255]),
        ] {
            assert!(close(colour(y, u, v), wanted), "{:?} for {wanted:?}", colour(y, u, v));
        }
        // What is outside the range is brought back into it.
        assert_eq!(colour(0, 128, 128), [0, 0, 0, 255]);
        assert_eq!(colour(255, 128, 128), [255, 255, 255, 255]);
    }

    #[test]
    fn pairs_of_pixels_share_a_colour_and_keep_their_own_light() {
        // Two rows of four pixels: red then blue, white then black.
        let data = [
            [RED.0, RED.1, RED.0, RED.2, BLUE.0, BLUE.1, BLUE.0, BLUE.2],
            [WHITE.0, 128, BLACK.0, 128, BLACK.0, 128, WHITE.0, 128],
        ]
        .concat();
        let picture = read_new(Layout::Yuyv, 4, 2, &data);
        assert_eq!((picture.width, picture.height, picture.rgba.len()), (4, 2, 32));
        assert!(close(pixel(&picture, 0, 0), [255, 0, 0]));
        assert!(close(pixel(&picture, 1, 0), [255, 0, 0]));
        assert!(close(pixel(&picture, 2, 0), [0, 0, 255]));
        assert!(close(pixel(&picture, 3, 0), [0, 0, 255]));
        assert!(close(pixel(&picture, 0, 1), [255, 255, 255]));
        assert!(close(pixel(&picture, 1, 1), [0, 0, 0]));
        assert!(close(pixel(&picture, 3, 1), [255, 255, 255]));
    }

    #[test]
    fn a_plane_of_light_and_a_smaller_one_of_colour_make_a_picture() {
        // Four by four: the left half green, the right half blue, and the
        // top row of the right half lit to white where the colour says so.
        let mut data = Vec::new();
        for _ in 0..4 {
            data.extend([GREEN.0, GREEN.0, BLUE.0, BLUE.0]);
        }
        for _ in 0..2 {
            data.extend([GREEN.1, GREEN.2, BLUE.1, BLUE.2]);
        }
        let picture = read_new(Layout::Nv12, 4, 4, &data);
        for row in 0..4 {
            assert!(close(pixel(&picture, 0, row), [0, 255, 0]), "row {row}");
            assert!(close(pixel(&picture, 1, row), [0, 255, 0]), "row {row}");
            assert!(close(pixel(&picture, 2, row), [0, 0, 255]), "row {row}");
            assert!(close(pixel(&picture, 3, row), [0, 0, 255]), "row {row}");
        }
    }

    #[test]
    fn rows_that_a_system_padded_are_read_without_their_padding() {
        // Three pixels a row in rows of eight bytes, as a system that
        // rounds rows up would hand them over.
        let grey = [[10, 20, 30, 0, 0, 0, 0, 0], [40, 50, 60, 0, 0, 0, 0, 0]].concat();
        let picture = read_new(Layout::Gray, 3, 2, &grey);
        assert_eq!(pixel(&picture, 2, 0), [30, 30, 30, 255]);
        assert_eq!(pixel(&picture, 0, 1), [40, 40, 40, 255]);

        // The same for light and colour: two by two in rows of four.
        let planes =
            [[WHITE.0, BLACK.0, 0, 0], [BLACK.0, WHITE.0, 0, 0], [128, 128, 0, 0]].concat();
        let picture = read_new(Layout::Nv12, 2, 2, &planes);
        assert!(close(pixel(&picture, 0, 0), [255, 255, 255]));
        assert!(close(pixel(&picture, 1, 0), [0, 0, 0]));
        assert!(close(pixel(&picture, 1, 1), [255, 255, 255]));
    }

    #[test]
    fn an_odd_width_keeps_its_last_column() {
        // Three pixels: the fourth of the last pair is not part of it.
        let data = [WHITE.0, 128, WHITE.0, 128, BLACK.0, 128, WHITE.0, 128];
        let picture = read_new(Layout::Yuyv, 3, 1, &data);
        assert_eq!(picture.rgba.len(), 12);
        assert!(close(pixel(&picture, 2, 0), [0, 0, 0]));

        let planes = [WHITE.0, WHITE.0, BLACK.0, 0, 128, 128, 128, 128];
        let picture = read_new(Layout::Nv12, 3, 1, &planes);
        assert!(close(pixel(&picture, 2, 0), [0, 0, 0]));
    }

    #[test]
    fn a_picture_cut_short_is_refused() {
        let mut picture = Picture::default();
        for (layout, data) in
            [(Layout::Yuyv, vec![0; 15]), (Layout::Nv12, vec![0; 11]), (Layout::Gray, vec![0; 7])]
        {
            let raw = Raw { layout, width: 4, height: 2, data: data.into() };
            assert!(read(&raw, &mut picture).is_err(), "{layout:?}");
        }
        let nothing = Raw { layout: Layout::Gray, width: 0, height: 0, data: Vec::new().into() };
        assert!(read(&nothing, &mut picture).is_err());
    }

    #[test]
    fn a_jpeg_brings_its_own_size_and_a_broken_one_is_refused() {
        let orange = image::RgbImage::from_pixel(16, 8, image::Rgb([255, 107, 53]));
        let mut file = std::io::Cursor::new(Vec::new());
        orange.write_to(&mut file, image::ImageFormat::Jpeg).expect("a JPEG fits in memory");

        // The size a camera claims is not what counts.
        let picture = read_new(Layout::Jpeg, 640, 480, file.get_ref());
        assert_eq!((picture.width, picture.height), (16, 8));
        let [red, green, blue, opacity] = pixel(&picture, 8, 4);
        assert!(
            red > 240 && green.abs_diff(107) < 12 && blue.abs_diff(53) < 12,
            "{red} {green} {blue}"
        );
        assert_eq!(opacity, 255);

        let mut picture = Picture::default();
        let broken =
            Raw { layout: Layout::Jpeg, width: 16, height: 8, data: b"not a picture"[..].into() };
        assert!(read(&broken, &mut picture).is_err());
    }
}
