//! The rules that keep a stream flowing in a browser: when each piece of
//! sound plays, which pictures a decoder may be given, and which pictures
//! of the camera leave for the PC.
//!
//! No browser is needed to state them, so they are tested with the rest of
//! the app, on the desktop.

/// Seconds of sound kept ahead of the speaker when it starts, or starts
/// again after running dry: what an uneven network may eat.
const CUSHION: f64 = 0.04;

/// With more than this many seconds ahead of the speaker, the sound is
/// behind the picture for good: some of it is skipped.
const TOO_FAR_AHEAD: f64 = 0.12;

/// Pictures a decoder that keeps up may have waiting: a twentieth of a
/// second of them.
const FEW_PICTURES: u32 = 3;

/// Seconds a decoder may have more than that waiting. It happens to every
/// decoder: while it starts, and when pictures arrive in a burst. One that
/// keeps up is through with them well before this.
const CATCHING_UP: f64 = 0.5;

/// With this many pictures waiting, a decoder shows what happened more than
/// a second ago, and nobody waits to see whether it catches up.
const TOO_MANY_PICTURES: u32 = 90;

/// Milliseconds between two pictures of the camera that decode on their
/// own, as in the desktop app: how long a PC that lost one shows nothing
/// new, at most.
const KEYFRAME_EVERY: f64 = 2_000.0;

/// Pictures of the camera an encoder that keeps up may have waiting.
const FEW_TO_ENCODE: u32 = 2;

/// Decides when each piece of sound plays.
///
/// Pieces arrive unevenly and must play back to back. A little sound is
/// kept ahead of the speaker to bridge the gaps, and no more: sound that
/// waits is sound that is late.
#[derive(Debug)]
pub struct Pace {
    /// The time on the speaker's clock where the next piece starts.
    next: f64,
    /// Too much sound was waiting: pieces are skipped until it is not.
    catching_up: bool,
}

impl Pace {
    pub fn new() -> Self {
        Self { next: f64::NEG_INFINITY, catching_up: false }
    }

    /// When a piece that lasts `length` seconds starts, on a speaker whose
    /// clock says `now`. `None` when the piece is to be skipped.
    pub fn place(&mut self, now: f64, length: f64) -> Option<f64> {
        let ahead = self.next - now;
        if ahead < 0.0 {
            // Nothing is playing: this is the first piece, or the sound
            // ran dry.
            self.next = now + CUSHION;
            self.catching_up = false;
        } else if ahead > TOO_FAR_AHEAD {
            self.catching_up = true;
        } else if ahead <= CUSHION {
            self.catching_up = false;
        }
        if self.catching_up {
            return None;
        }
        let at = self.next;
        self.next += length;
        Some(at)
    }
}

/// What to do with a picture that arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Decode,
    /// The decoder could do nothing with it. `ask` when the bridge has not
    /// been asked for a keyframe yet.
    Skip {
        ask: bool,
    },
}

/// Decides which pictures a decoder may be given.
///
/// Every picture but a keyframe builds on the ones before it. A decoder
/// that missed one, for whatever reason, shows garbage until the next
/// keyframe: better to show the last good picture, and to ask for one.
#[derive(Debug)]
pub struct Pictures {
    /// The decoder has nothing to build on.
    lost: bool,
    /// The bridge was asked for a keyframe since.
    asked: bool,
    /// Since when the decoder has had more than a few pictures waiting.
    crowded_since: Option<f64>,
}

impl Pictures {
    /// For a decoder that starts: it needs a keyframe first.
    pub fn new() -> Self {
        Self { lost: true, asked: false, crowded_since: None }
    }

    /// Whether a decoder with `waiting` pictures in its hands at `now`, in
    /// seconds, has fallen behind: it is not catching up, and what it would
    /// show is the past. One that has is emptied and starts over: see
    /// [`Self::lose`].
    pub fn behind(&mut self, waiting: u32, now: f64) -> bool {
        if waiting <= FEW_PICTURES {
            self.crowded_since = None;
            return false;
        }
        let since = *self.crowded_since.get_or_insert(now);
        let behind = waiting > TOO_MANY_PICTURES || now - since > CATCHING_UP;
        if behind {
            // The decoder that replaces it starts with none.
            self.crowded_since = None;
        }
        behind
    }

    /// The decoder failed, or was emptied: it starts over.
    pub fn lose(&mut self) {
        if !self.lost {
            (self.lost, self.asked) = (true, false);
        }
    }

    /// What to do with the next picture.
    pub fn judge(&mut self, keyframe: bool) -> Verdict {
        if keyframe {
            (self.lost, self.asked) = (false, false);
        }
        if !self.lost {
            return Verdict::Decode;
        }
        let ask = !self.asked;
        self.asked = true;
        Verdict::Skip { ask }
    }
}

/// A picture of the camera to take.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Taken {
    /// When, in milliseconds since the first one.
    pub at: f64,
    /// It has to decode on its own.
    pub keyframe: bool,
}

/// Decides which pictures of the camera are encoded for the PC, and which
/// of them decode on their own.
///
/// The camera is looked at some thirty times a second. A picture is left
/// out before it is encoded when it would only wait: what the PC shows is
/// then a moment old, not late for good. One left out afterwards breaks the
/// chain the following ones build on, and the next has to start a new one.
#[derive(Debug, Default)]
pub struct Shutter {
    /// When the first picture was taken, on the page's clock.
    first: Option<f64>,
    /// When the last picture that decodes on its own was.
    keyframe: Option<f64>,
    /// A picture went missing after the encoder made it.
    broken: bool,
}

impl Shutter {
    /// Whether a picture is taken at `now`, in milliseconds, with `waiting`
    /// pictures the encoder has not started on, and `room` or none on the
    /// way to the bridge.
    pub fn take(&mut self, now: f64, waiting: u32, room: bool) -> Option<Taken> {
        if waiting > FEW_TO_ENCODE || !room {
            return None;
        }
        let first = *self.first.get_or_insert(now);
        let keyframe = self.broken || self.keyframe.is_none_or(|last| now - last >= KEYFRAME_EVERY);
        if keyframe {
            (self.keyframe, self.broken) = (Some(now), false);
        }
        Some(Taken { at: (now - first).max(0.0), keyframe })
    }

    /// An encoded picture did not leave.
    pub fn lose(&mut self) {
        self.broken = true;
    }
}

/// The name browsers know H.264 by, for pictures of this size and this many
/// a second: the constrained baseline profile, which every decoder reads
/// and which is what the desktop app sends, at the lowest of three levels
/// that allows them.
pub fn camera_codec(width: u32, height: u32, fps: u32) -> &'static str {
    // Levels count in blocks of sixteen pixels a side.
    let blocks = u64::from(width.div_ceil(16)) * u64::from(height.div_ceil(16));
    let a_second = blocks * u64::from(fps);
    if blocks <= 3_600 && a_second <= 108_000 {
        // Level 3.1: up to 1280 by 720, thirty times a second.
        "avc1.42E01F"
    } else if blocks <= 8_192 && a_second <= 245_760 {
        // Level 4: up to 1920 by 1080, thirty times a second.
        "avc1.42E028"
    } else {
        // Level 5.1: up to 3840 by 2160.
        "avc1.42E033"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How long a piece of sound lasts: 5 ms, as PCs send them.
    const PIECE: f64 = 0.005;

    /// The time a piece was placed at, to the nanosecond: sums of
    /// fractions of a second are not exact.
    fn placed(pace: &mut Pace, now: f64) -> Option<i64> {
        pace.place(now, PIECE).map(nanoseconds)
    }

    fn nanoseconds(seconds: f64) -> i64 {
        (seconds * 1e9).round() as i64
    }

    #[test]
    fn sound_plays_back_to_back_a_little_ahead_of_the_speaker() {
        let mut pace = Pace::new();
        assert_eq!(placed(&mut pace, 10.0), Some(nanoseconds(10.0 + CUSHION)));
        // The next ones follow, whenever they arrive.
        assert_eq!(placed(&mut pace, 10.001), Some(nanoseconds(10.0 + CUSHION + PIECE)));
        assert_eq!(placed(&mut pace, 10.02), Some(nanoseconds(10.0 + CUSHION + 2.0 * PIECE)));
    }

    #[test]
    fn sound_that_ran_dry_starts_again_with_its_cushion() {
        let mut pace = Pace::new();
        assert_eq!(placed(&mut pace, 10.0), Some(nanoseconds(10.0 + CUSHION)));
        // Nothing for half a second: what was queued has long played.
        assert_eq!(placed(&mut pace, 10.5), Some(nanoseconds(10.5 + CUSHION)));
    }

    #[test]
    fn sound_that_piles_up_is_skipped_until_it_is_close_again() {
        let mut pace = Pace::new();
        let now = 10.0;
        // A burst: far more than the speaker plays in no time at all.
        let played = (0..100).filter(|_| placed(&mut pace, now).is_some()).count();
        // The burst is cut where it is too far ahead.
        let queued = CUSHION + played as f64 * PIECE;
        assert!((TOO_FAR_AHEAD..TOO_FAR_AHEAD + 2.0 * PIECE).contains(&queued), "{queued}");

        // Still too far ahead a moment later: skipped.
        assert_eq!(placed(&mut pace, now + 0.02), None);
        // Once the speaker is close again, sound plays again, back to back
        // with what played last.
        let later = now + queued - CUSHION / 2.0;
        assert_eq!(placed(&mut pace, later), Some(nanoseconds(now + queued)));
    }

    #[test]
    fn a_decoder_starts_with_a_keyframe() {
        let mut pictures = Pictures::new();
        assert_eq!(pictures.judge(false), Verdict::Skip { ask: true });
        assert_eq!(pictures.judge(false), Verdict::Skip { ask: false });
        assert_eq!(pictures.judge(true), Verdict::Decode);
        assert_eq!(pictures.judge(false), Verdict::Decode);
    }

    #[test]
    fn a_decoder_that_failed_waits_for_a_keyframe_and_asks_once() {
        let mut pictures = Pictures::new();
        assert_eq!(pictures.judge(true), Verdict::Decode);
        pictures.lose();
        assert_eq!(pictures.judge(false), Verdict::Skip { ask: true });
        // Failing again on the way changes nothing.
        pictures.lose();
        assert_eq!(pictures.judge(false), Verdict::Skip { ask: false });
        assert_eq!(pictures.judge(true), Verdict::Decode);

        // The next failure is asked about again.
        pictures.lose();
        assert_eq!(pictures.judge(false), Verdict::Skip { ask: true });
    }

    #[test]
    fn a_decoder_may_be_crowded_for_a_moment() {
        let mut pictures = Pictures::new();
        // It starts slowly, and a third of a second of pictures piles up.
        assert!(!pictures.behind(0, 10.0));
        assert!(!pictures.behind(10, 10.1));
        assert!(!pictures.behind(20, 10.3));
        // Then it works through them.
        assert!(!pictures.behind(8, 10.4));
        assert!(!pictures.behind(1, 10.5));
        // A burst later on is no different, however long after.
        assert!(!pictures.behind(12, 20.0));
        assert!(!pictures.behind(0, 20.1));
    }

    #[test]
    fn a_decoder_that_stays_crowded_is_behind() {
        let mut pictures = Pictures::new();
        assert!(!pictures.behind(5, 10.0));
        assert!(!pictures.behind(9, 10.3));
        assert!(pictures.behind(14, 10.0 + CATCHING_UP + 0.1));
        // Emptied, it gets the same patience again.
        assert!(!pictures.behind(5, 10.7));

        // With more than a second waiting, there is nothing to wait for.
        let mut pictures = Pictures::new();
        assert!(pictures.behind(TOO_MANY_PICTURES + 1, 10.0));
    }

    #[test]
    fn the_camera_starts_with_a_picture_that_decodes_on_its_own_and_repeats_one() {
        let mut shutter = Shutter::default();
        // The page's clock does not start at nought: the pictures do.
        assert_eq!(shutter.take(5_000.0, 0, true), Some(Taken { at: 0.0, keyframe: true }));
        assert_eq!(shutter.take(5_033.0, 0, true), Some(Taken { at: 33.0, keyframe: false }));
        assert_eq!(shutter.take(6_999.0, 1, true), Some(Taken { at: 1_999.0, keyframe: false }));
        assert_eq!(shutter.take(7_000.0, 2, true), Some(Taken { at: 2_000.0, keyframe: true }));
        assert_eq!(shutter.take(7_033.0, 0, true), Some(Taken { at: 2_033.0, keyframe: false }));
        assert_eq!(shutter.take(9_100.0, 0, true), Some(Taken { at: 4_100.0, keyframe: true }));
    }

    #[test]
    fn a_picture_that_would_only_wait_is_not_taken() {
        let mut shutter = Shutter::default();
        // Not even the first: the pictures start with the first one taken.
        assert_eq!(shutter.take(100.0, 0, false), None);
        assert_eq!(shutter.take(133.0, 0, true), Some(Taken { at: 0.0, keyframe: true }));

        // An encoder that is behind, or a line to the bridge that is full.
        assert_eq!(shutter.take(166.0, 3, true), None);
        assert_eq!(shutter.take(200.0, 0, false), None);
        // Nothing was encoded meanwhile: the next builds on the last.
        assert_eq!(shutter.take(233.0, 0, true), Some(Taken { at: 100.0, keyframe: false }));
    }

    #[test]
    fn a_picture_lost_after_encoding_is_followed_by_one_that_decodes_on_its_own() {
        let mut shutter = Shutter::default();
        assert!(shutter.take(0.0, 0, true).is_some_and(|taken| taken.keyframe));
        assert_eq!(shutter.take(33.0, 0, true), Some(Taken { at: 33.0, keyframe: false }));
        shutter.lose();
        assert_eq!(shutter.take(66.0, 0, true), Some(Taken { at: 66.0, keyframe: true }));
        assert_eq!(shutter.take(100.0, 0, true), Some(Taken { at: 100.0, keyframe: false }));
        // Two seconds are counted from that one.
        assert_eq!(shutter.take(2_033.0, 0, true), Some(Taken { at: 2_033.0, keyframe: false }));
        assert_eq!(shutter.take(2_066.0, 0, true), Some(Taken { at: 2_066.0, keyframe: true }));
    }

    #[test]
    fn the_codec_is_named_for_the_size_of_the_camera() {
        assert_eq!(camera_codec(640, 360, 30), "avc1.42E01F");
        assert_eq!(camera_codec(1280, 720, 30), "avc1.42E01F");
        assert_eq!(camera_codec(1280, 720, 60), "avc1.42E028");
        assert_eq!(camera_codec(1920, 1080, 30), "avc1.42E028");
        assert_eq!(camera_codec(3840, 2160, 30), "avc1.42E033");
    }
}
