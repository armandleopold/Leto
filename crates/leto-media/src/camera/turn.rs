//! Pictures in planes of YUV 4:2:0, as phones give them, made into NV12
//! and turned upright on the way.

/// The planes of a picture: luminance at full size, U and V at half size
/// each way, each row `chroma_stride` apart and each sample `chroma_step`.
pub(super) struct Planes<'a> {
    pub width: usize,
    pub height: usize,
    pub luma: &'a [u8],
    pub luma_stride: usize,
    pub u: &'a [u8],
    pub v: &'a [u8],
    pub chroma_stride: usize,
    pub chroma_step: usize,
}

/// Writes `planes` into `out` as NV12, turned clockwise by `turn` degrees
/// (0, 90, 180 or 270), and returns its width and height then. Sizes are
/// taken down to even numbers, as NV12 needs.
pub(super) fn nv12(planes: &Planes<'_>, turn: u32, out: &mut Vec<u8>) -> (usize, usize) {
    let (width, height) = (planes.width & !1, planes.height & !1);
    let sideways = turn == 90 || turn == 270;
    let (out_width, out_height) = if sideways { (height, width) } else { (width, height) };
    out.clear();
    out.resize(out_width * out_height * 3 / 2, 128);
    // Where in the picture as made the pixel at (x, y) of the turned one
    // comes from, in a picture `w` by `h` as made.
    let source = |x: usize, y: usize, w: usize, h: usize| match turn {
        90 => (y, h - 1 - x),
        180 => (w - 1 - x, h - 1 - y),
        270 => (w - 1 - y, x),
        _ => (x, y),
    };
    let byte = |plane: &[u8], at: usize| plane.get(at).copied().unwrap_or(128);
    for y in 0..out_height {
        let row = &mut out[y * out_width..(y + 1) * out_width];
        for (x, pixel) in row.iter_mut().enumerate() {
            let (sx, sy) = source(x, y, width, height);
            *pixel = byte(planes.luma, sy * planes.luma_stride + sx);
        }
    }
    let (half_width, half_height) = (width / 2, height / 2);
    let (out_half_width, out_half_height) = (out_width / 2, out_height / 2);
    let chroma = &mut out[out_width * out_height..];
    for y in 0..out_half_height {
        for x in 0..out_half_width {
            let (sx, sy) = source(x, y, half_width, half_height);
            let at = sy * planes.chroma_stride + sx * planes.chroma_step;
            let pair = (y * out_half_width + x) * 2;
            chroma[pair] = byte(planes.u, at);
            chroma[pair + 1] = byte(planes.v, at);
        }
    }
    (out_width, out_height)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 4 by 2 picture whose luminance counts its pixels, and whose two
    /// colour samples differ, interleaved as phones give them.
    fn planes() -> (Vec<u8>, Vec<u8>) {
        let luma = vec![0, 1, 2, 3, 4, 5, 6, 7];
        // U0 V0 U1 V1: the V plane starts one byte into the same memory.
        let chroma = vec![10, 20, 11, 21];
        (luma, chroma)
    }

    fn turned(turn: u32) -> (usize, usize, Vec<u8>) {
        let (luma, chroma) = planes();
        let planes = Planes {
            width: 4,
            height: 2,
            luma: &luma,
            luma_stride: 4,
            u: &chroma,
            v: &chroma[1..],
            chroma_stride: 4,
            chroma_step: 2,
        };
        let mut out = Vec::new();
        let (width, height) = nv12(&planes, turn, &mut out);
        (width, height, out)
    }

    #[test]
    fn an_upright_picture_is_copied() {
        assert_eq!(turned(0), (4, 2, vec![0, 1, 2, 3, 4, 5, 6, 7, 10, 20, 11, 21]));
    }

    #[test]
    fn a_quarter_turn_puts_the_bottom_left_at_the_top_left() {
        // 4 0 / 5 1 / 6 2 / 7 3, colour: the first sample above the second.
        assert_eq!(turned(90), (2, 4, vec![4, 0, 5, 1, 6, 2, 7, 3, 10, 20, 11, 21]));
    }

    #[test]
    fn a_half_turn_reverses_the_picture() {
        assert_eq!(turned(180), (4, 2, vec![7, 6, 5, 4, 3, 2, 1, 0, 11, 21, 10, 20]));
    }

    #[test]
    fn three_quarters_put_the_top_right_at_the_top_left() {
        assert_eq!(turned(270), (2, 4, vec![3, 7, 2, 6, 1, 5, 0, 4, 11, 21, 10, 20]));
    }

    #[test]
    fn odd_sizes_lose_their_last_row_and_column() {
        let luma = vec![0; 15];
        let chroma = vec![128; 4];
        let planes = Planes {
            width: 5,
            height: 3,
            luma: &luma,
            luma_stride: 5,
            u: &chroma,
            v: &chroma,
            chroma_stride: 2,
            chroma_step: 1,
        };
        let mut out = Vec::new();
        assert_eq!(nv12(&planes, 0, &mut out), (4, 2));
        assert_eq!(out.len(), 12);
    }
}
