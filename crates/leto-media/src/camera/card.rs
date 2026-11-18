//! A made-up camera: a test card, delivered as a camera delivers pictures.
//!
//! For trying the app where there is no camera, and for the tests.

use std::{
    borrow::Cow,
    thread,
    time::{Duration, Instant},
};

use super::{
    Source,
    pixels::{Layout, Raw},
};
use crate::Error;

pub(super) const WIDTH: usize = 640;
pub(super) const HEIGHT: usize = 360;

/// Between two pictures: thirty a second.
const STEP: Duration = Duration::from_nanos(1_000_000_000 / 30);

/// The bars of the card from left to right, as luminance and colour: white,
/// yellow, cyan, green, magenta, red, blue and black.
const BARS: [(u8, u8, u8); 8] = [
    (235, 128, 128),
    (210, 16, 146),
    (170, 166, 16),
    (145, 54, 34),
    (106, 202, 222),
    (81, 90, 240),
    (41, 240, 110),
    (16, 128, 128),
];

/// The side of the square that crosses the card, to show that it is live
/// and which way round it is shown.
const SQUARE: usize = 48;

pub(super) struct Card {
    /// The bars alone, two pixels in four bytes.
    bars: Vec<u8>,
    /// The picture being handed over.
    picture: Vec<u8>,
    shown: usize,
    /// When the next picture is due.
    due: Instant,
}

impl Card {
    pub(super) fn new() -> Self {
        let mut bars = Vec::with_capacity(WIDTH * HEIGHT * 2);
        for _ in 0..HEIGHT {
            for pair in 0..WIDTH / 2 {
                let (y, u, v) = BARS[pair * 2 * BARS.len() / WIDTH];
                bars.extend([y, u, y, v]);
            }
        }
        Self { picture: bars.clone(), bars, shown: 0, due: Instant::now() }
    }
}

impl Source for Card {
    fn next(&mut self) -> Result<Raw<'_>, Error> {
        let now = Instant::now();
        thread::sleep(self.due.saturating_duration_since(now));
        // A reader that is late gets its pictures as fast as it reads them,
        // not a pile of them at once.
        self.due = (self.due + STEP).max(now);

        self.picture.copy_from_slice(&self.bars);
        // From left to right along the bottom, four pixels a picture.
        let left = self.shown * 4 % (WIDTH - SQUARE) / 2 * 2;
        let top = HEIGHT - SQUARE - 24;
        for row in top..top + SQUARE {
            let start = (row * WIDTH + left) * 2;
            for pair in self.picture[start..start + SQUARE * 2].chunks_exact_mut(4) {
                pair.copy_from_slice(&[235, 128, 235, 128]);
            }
        }
        self.shown += 1;

        Ok(Raw {
            layout: Layout::Yuyv,
            width: WIDTH,
            height: HEIGHT,
            data: Cow::Borrowed(&self.picture),
        })
    }
}
