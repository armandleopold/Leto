//! Which of the formats a camera offers Leto captures in.

use super::pixels::Layout;

/// What Leto would like from a camera: enough for a face to be seen well,
/// and no more than a home connection carries.
const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
const RATE: u32 = 30;

/// Under this many pictures a second, movement stutters.
const SMOOTH: u32 = 24;

/// One way a camera can deliver its pictures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Offer {
    pub layout: Layout,
    pub width: u32,
    pub height: u32,
    /// Pictures a second.
    pub rate: u32,
}

/// The format to capture in among those a camera offers: one that moves
/// smoothly before one that does not, then the size closest to the one Leto
/// would like, then the rate closest to it, then the layout that takes the
/// least work to read.
pub(super) fn choose(offered: &[Offer]) -> Option<Offer> {
    let wanted = u64::from(WIDTH) * u64::from(HEIGHT);
    offered
        .iter()
        .copied()
        .filter(|offer| offer.width > 0 && offer.height > 0 && offer.rate > 0)
        .min_by_key(|offer| {
            let pixels = u64::from(offer.width) * u64::from(offer.height);
            (
                offer.rate < SMOOTH,
                pixels.abs_diff(wanted),
                offer.rate.abs_diff(RATE),
                work(offer.layout),
            )
        })
}

/// How much work a layout is to read, the least first. Grey comes last: it
/// has no colour.
fn work(layout: Layout) -> u8 {
    match layout {
        Layout::Yuyv => 0,
        Layout::Nv12 => 1,
        Layout::Jpeg => 2,
        Layout::Gray => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(layout: Layout, width: u32, height: u32, rate: u32) -> Offer {
        Offer { layout, width, height, rate }
    }

    #[test]
    fn the_size_and_the_rate_leto_would_like_are_chosen_when_a_camera_has_them() {
        // What a common webcam offers: everything compressed, and without
        // compression only small pictures move smoothly.
        let offered = [
            offer(Layout::Yuyv, 640, 480, 30),
            offer(Layout::Yuyv, 1280, 720, 10),
            offer(Layout::Yuyv, 1920, 1080, 5),
            offer(Layout::Jpeg, 640, 480, 30),
            offer(Layout::Jpeg, 1280, 720, 60),
            offer(Layout::Jpeg, 1280, 720, 30),
            offer(Layout::Jpeg, 1920, 1080, 30),
        ];
        assert_eq!(choose(&offered), Some(offer(Layout::Jpeg, 1280, 720, 30)));
    }

    #[test]
    fn a_smooth_picture_comes_before_a_large_one() {
        let offered = [offer(Layout::Yuyv, 1280, 720, 10), offer(Layout::Yuyv, 640, 480, 30)];
        assert_eq!(choose(&offered), Some(offer(Layout::Yuyv, 640, 480, 30)));
        // A camera that never moves smoothly is used all the same.
        assert_eq!(choose(&offered[..1]), Some(offer(Layout::Yuyv, 1280, 720, 10)));
    }

    #[test]
    fn the_closest_size_is_chosen_when_the_one_leto_would_like_is_missing() {
        let offered = [
            offer(Layout::Nv12, 640, 360, 30),
            offer(Layout::Nv12, 960, 540, 30),
            offer(Layout::Nv12, 1920, 1080, 30),
            offer(Layout::Nv12, 3840, 2160, 30),
        ];
        assert_eq!(choose(&offered), Some(offer(Layout::Nv12, 960, 540, 30)));
    }

    #[test]
    fn the_layout_that_takes_the_least_work_is_chosen_among_equals() {
        let offered = [
            offer(Layout::Gray, 1280, 720, 30),
            offer(Layout::Jpeg, 1280, 720, 30),
            offer(Layout::Nv12, 1280, 720, 30),
            offer(Layout::Yuyv, 1280, 720, 30),
        ];
        assert_eq!(choose(&offered), Some(offer(Layout::Yuyv, 1280, 720, 30)));
        assert_eq!(choose(&offered[..2]), Some(offer(Layout::Jpeg, 1280, 720, 30)));
    }

    #[test]
    fn a_camera_that_offers_nothing_usable_gets_no_choice() {
        assert_eq!(choose(&[]), None);
        assert_eq!(
            choose(&[offer(Layout::Yuyv, 0, 0, 30), offer(Layout::Yuyv, 640, 480, 0)]),
            None
        );
    }
}
