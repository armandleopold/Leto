//! What fingers on a touch screen mean for the PC's mouse.
//!
//! One finger is the mouse: the pointer goes where it touches, presses the
//! left button while it stays down and lets go when it lifts. egui already
//! turns the first finger into those pointer events. Two fingers are the
//! rest, as on a laptop's touchpad: moved together they turn the wheel, and
//! tapped together they press the right button. While two are down, what
//! egui says of the first one is not the mouse's.

use std::collections::BTreeMap;

use egui::{Pos2, TouchPhase, Vec2};

/// How far, in points, two fingers may wander and still be a tap.
const TAP_WANDER: f32 = 12.0;
/// How long, in seconds, two fingers may stay down and still be a tap.
const TAP_LASTS: f64 = 0.4;

/// What the fingers did that the PC's mouse must do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gesture {
    /// A second finger came down: the left button, pressed by the first,
    /// must be let go of.
    LetGo,
    /// Two fingers moved together: what is on screen follows them by this
    /// much, in points.
    Scroll(Vec2),
    /// Two fingers tapped: a right click, where the first one was.
    RightClick(Pos2),
}

/// The fingers on the screen.
#[derive(Debug, Default)]
pub struct Touches {
    /// Where each finger down is, by the id the system gave it.
    down: BTreeMap<u64, Pos2>,
    /// Set from the moment a second finger comes down until the last one
    /// lifts.
    pair: Option<Pair>,
}

#[derive(Debug)]
struct Pair {
    /// When the second finger came down.
    since: f64,
    /// Where the first finger was then.
    first: Pos2,
    /// How far the fingers moved since, all together.
    wandered: f32,
}

impl Touches {
    /// Whether egui's pointer events, made of the first finger, must be
    /// left out: two fingers are down, or were and not all lifted yet.
    pub const fn hold_the_pointer(&self) -> bool {
        self.pair.is_some()
    }

    /// One finger did something, at `now` in seconds.
    pub fn touch(&mut self, id: u64, phase: TouchPhase, at: Pos2, now: f64) -> Option<Gesture> {
        match phase {
            TouchPhase::Start => {
                let first = self.down.values().next().copied();
                self.down.insert(id, at);
                match (first, &self.pair) {
                    (Some(first), None) => {
                        self.pair = Some(Pair { since: now, first, wandered: 0.0 });
                        Some(Gesture::LetGo)
                    }
                    _ => None,
                }
            }
            TouchPhase::Move => {
                let before = self.down.insert(id, at)?;
                let pair = self.pair.as_mut()?;
                let moved = at - before;
                pair.wandered += moved.length();
                // Each finger moves the screen by its share: two moving
                // the same way move it as one would.
                let share = moved / self.down.len().max(1) as f32;
                (share != Vec2::ZERO).then_some(Gesture::Scroll(share))
            }
            TouchPhase::End | TouchPhase::Cancel => {
                self.down.remove(&id);
                if !self.down.is_empty() {
                    return None;
                }
                let pair = self.pair.take()?;
                let tapped = phase == TouchPhase::End
                    && pair.wandered <= TAP_WANDER
                    && now - pair.since <= TAP_LASTS;
                tapped.then_some(Gesture::RightClick(pair.first))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use egui::{pos2, vec2};

    use super::*;

    #[test]
    fn one_finger_is_left_to_the_pointer() {
        let mut touches = Touches::default();
        assert_eq!(touches.touch(1, TouchPhase::Start, pos2(10.0, 10.0), 0.0), None);
        assert_eq!(touches.touch(1, TouchPhase::Move, pos2(30.0, 10.0), 0.1), None);
        assert!(!touches.hold_the_pointer());
        assert_eq!(touches.touch(1, TouchPhase::End, pos2(30.0, 10.0), 0.2), None);
    }

    #[test]
    fn two_fingers_tapped_are_a_right_click_where_the_first_was() {
        let mut touches = Touches::default();
        touches.touch(1, TouchPhase::Start, pos2(100.0, 50.0), 0.0);
        let second = touches.touch(2, TouchPhase::Start, pos2(140.0, 50.0), 0.05);
        assert_eq!(second, Some(Gesture::LetGo));
        assert!(touches.hold_the_pointer());
        assert_eq!(touches.touch(2, TouchPhase::End, pos2(140.0, 50.0), 0.15), None);
        // The first finger lifting is not the mouse's either.
        assert!(touches.hold_the_pointer());
        let last = touches.touch(1, TouchPhase::End, pos2(100.0, 50.0), 0.2);
        assert_eq!(last, Some(Gesture::RightClick(pos2(100.0, 50.0))));
        assert!(!touches.hold_the_pointer());
    }

    #[test]
    fn two_fingers_moved_together_scroll_and_click_nothing() {
        let mut touches = Touches::default();
        touches.touch(1, TouchPhase::Start, pos2(100.0, 100.0), 0.0);
        touches.touch(2, TouchPhase::Start, pos2(140.0, 100.0), 0.0);
        let mut scrolled = Vec2::ZERO;
        for step in 1..=5 {
            let y = 100.0 + 10.0 * step as f32;
            for (id, x) in [(1, 100.0), (2, 140.0)] {
                if let Some(Gesture::Scroll(by)) =
                    touches.touch(id, TouchPhase::Move, pos2(x, y), 0.05 * step as f64)
                {
                    scrolled += by;
                }
            }
        }
        assert_eq!(scrolled, vec2(0.0, 50.0));
        touches.touch(1, TouchPhase::End, pos2(100.0, 150.0), 0.3);
        assert_eq!(touches.touch(2, TouchPhase::End, pos2(140.0, 150.0), 0.3), None);
    }

    #[test]
    fn two_fingers_held_long_are_no_tap() {
        let mut touches = Touches::default();
        touches.touch(1, TouchPhase::Start, pos2(0.0, 0.0), 0.0);
        touches.touch(2, TouchPhase::Start, pos2(40.0, 0.0), 0.0);
        touches.touch(2, TouchPhase::End, pos2(40.0, 0.0), 1.0);
        assert_eq!(touches.touch(1, TouchPhase::End, pos2(0.0, 0.0), 1.0), None);
    }

    #[test]
    fn fingers_the_system_took_away_are_no_tap() {
        let mut touches = Touches::default();
        touches.touch(1, TouchPhase::Start, pos2(0.0, 0.0), 0.0);
        touches.touch(2, TouchPhase::Start, pos2(40.0, 0.0), 0.0);
        touches.touch(2, TouchPhase::Cancel, pos2(40.0, 0.0), 0.1);
        assert_eq!(touches.touch(1, TouchPhase::Cancel, pos2(0.0, 0.0), 0.1), None);
        assert!(!touches.hold_the_pointer());
    }
}
