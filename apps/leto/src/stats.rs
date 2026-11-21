//! How a stream is doing, in numbers: what Ctrl+Alt+Shift+S shows over the
//! picture, as in Moonlight.
//!
//! The backend counts what happens to the pictures, and the stream screen
//! turns two counts a moment apart into rates. No drawing and no clock
//! here.

use std::time::Duration;

use leto_core::engine::Codec;

/// What happened to the pictures of a stream since it started. Every number
/// only grows.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    /// What the PC said it sends: width, height, pictures per second.
    pub sent: Option<[u32; 3]>,
    /// The codec of the pictures, and whether the graphics card decodes
    /// them, where the backend knows.
    pub codec: Option<Codec>,
    pub on_graphics_card: Option<bool>,
    /// Pictures that arrived, and their size in bytes.
    pub received: u64,
    pub bytes: u64,
    /// Pictures the PC made that never arrived, where that is known.
    pub lost: Option<u64>,
    /// Pictures decoded, and how long that took them all, where measured.
    pub decoded: u64,
    pub decoding: Option<Duration>,
    /// Of the decoded pictures, those copied out of the decoder to be shown,
    /// and how long the copying took them all. Part of the decoding.
    pub copied: u64,
    pub copying: Duration,
    /// Pictures decoded and not shown, because newer ones were waiting
    /// behind them.
    pub skipped: u64,
    /// Pictures the backend showed itself, where it does. Otherwise the
    /// window counts the pictures it shows.
    pub shown: Option<u64>,
    /// Pictures the PC said how long it took to make, and that time.
    pub timed_by_host: u64,
    pub host_time: Duration,
}

impl Counts {
    /// A picture arrived, `bytes` long, that the PC took `host` to make.
    /// `gap` is how many pictures the PC made between the last one and this
    /// one, where that is known.
    pub fn received(&mut self, bytes: usize, gap: Option<u64>, host: Option<Duration>) {
        self.received += 1;
        self.bytes += bytes as u64;
        if let Some(gap) = gap {
            *self.lost.get_or_insert(0) += gap;
        }
        if let Some(host) = host {
            self.timed_by_host += 1;
            self.host_time += host;
        }
    }

    /// A picture was decoded, in `took` where that is measured.
    pub fn decoded(&mut self, took: Option<Duration>) {
        self.decoded += 1;
        if let Some(took) = took {
            *self.decoding.get_or_insert(Duration::ZERO) += took;
        }
    }

    /// Of a picture decoded to be shown, `took` went to copying it out of
    /// the decoder. The browser does that itself, unseen.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub fn copied(&mut self, took: Duration) {
        self.copied += 1;
        self.copying += took;
    }
}

/// How a stream did over a stretch of time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rates {
    /// Pictures per second that arrived, were decoded, were shown.
    pub received: f64,
    pub decoded: f64,
    pub shown: f64,
    /// Megabits per second of pictures.
    pub megabits: f64,
    /// Of the pictures the PC made, the share that never arrived, in
    /// percent, and how many in all since the stream started.
    pub lost: Option<(f64, u64)>,
    /// Milliseconds a picture took to decode, on average.
    pub decoding_ms: Option<f64>,
    /// Milliseconds a picture to be shown took to copy out of the decoder,
    /// on average, where the backend copies them.
    pub copying_ms: Option<f64>,
    /// Pictures per second decoded and not shown, to keep up.
    pub skipped: f64,
    /// Milliseconds the PC took to make a picture, on average.
    pub host_ms: Option<f64>,
}

impl Rates {
    /// The rates from `before` to `after`, `seconds` apart, while the window
    /// showed `shown` pictures. `None` for no time at all.
    pub fn between(before: &Counts, after: &Counts, shown: u64, seconds: f64) -> Option<Self> {
        if seconds <= 0.0 {
            return None;
        }
        let per_second = |count: u64| count as f64 / seconds;
        let received = after.received.saturating_sub(before.received);
        let decoded = after.decoded.saturating_sub(before.decoded);
        let shown = match (before.shown, after.shown) {
            (Some(before), Some(after)) => after.saturating_sub(before),
            _ => shown,
        };
        let lost = after.lost.map(|lost| {
            let gone = lost.saturating_sub(before.lost.unwrap_or(0));
            let made = gone + received;
            let share = if made == 0 { 0.0 } else { 100.0 * gone as f64 / made as f64 };
            (share, lost)
        });
        let average = |time: Duration, count: u64| {
            (count > 0).then(|| time.as_secs_f64() * 1000.0 / count as f64)
        };
        let decoding_ms = match (before.decoding, after.decoding) {
            (Some(before), Some(after)) => average(after.saturating_sub(before), decoded),
            (None, Some(after)) => average(after, decoded),
            _ => None,
        };
        let timed = after.timed_by_host.saturating_sub(before.timed_by_host);
        let copied = after.copied.saturating_sub(before.copied);
        Some(Self {
            received: per_second(received),
            decoded: per_second(decoded),
            shown: per_second(shown),
            megabits: after.bytes.saturating_sub(before.bytes) as f64 * 8.0 / 1e6 / seconds,
            lost,
            decoding_ms,
            copying_ms: average(after.copying.saturating_sub(before.copying), copied),
            skipped: per_second(after.skipped.saturating_sub(before.skipped)),
            host_ms: average(after.host_time.saturating_sub(before.host_time), timed),
        })
    }
}

/// How often the log is told how a stream does, in seconds.
pub const TOLD_EVERY: f64 = 10.0;

impl Rates {
    /// How the stream did, in a line for the log: what the statistics over
    /// the picture show, for whoever reads the log afterwards. `round_trip`
    /// is how long a message takes to reach the PC and come back.
    pub fn told(&self, round_trip: Option<Duration>) -> String {
        let mut line = format!(
            "a second of the stream: {:.0} pictures arrived, {:.0} decoded, {:.0} skipped, {:.1} \
             Mbit/s",
            self.received, self.decoded, self.skipped, self.megabits
        );
        if let Some((share, _)) = self.lost {
            line += &format!(", {share:.1} % lost");
        }
        if let Some(decoding) = self.decoding_ms {
            line += &format!("; a picture took {decoding:.1} ms to decode");
            if let Some(copying) = self.copying_ms {
                line += &format!(", {copying:.1} ms of them to leave the decoder when shown");
            }
        }
        if let Some(host) = self.host_ms {
            line += &format!("; the PC took {host:.1} ms to make one");
        }
        if let Some(round_trip) = round_trip {
            line += &format!("; {} ms to the PC and back", round_trip.as_millis());
        }
        line
    }
}

/// What the log is told of how the pictures are drawn: on how many pixels
/// of the screen, when that is not one for each of theirs. Pictures drawn
/// larger or smaller are less sharp than the PC made them.
pub fn drawn(drawn: Option<([usize; 2], [u32; 2])>) -> String {
    match drawn {
        Some(([width, height], [across, down]))
            if width == across as usize && height == down as usize =>
        {
            format!("; drawn pixel for pixel, {width} by {height}")
        }
        Some(([width, height], [across, down])) => {
            format!("; pictures of {width} by {height} drawn on {across} by {down} pixels")
        }
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_is_told_when_pictures_are_not_drawn_pixel_for_pixel() {
        assert_eq!(
            drawn(Some(([3840, 1080], [3840, 1080]))),
            "; drawn pixel for pixel, 3840 by 1080"
        );
        assert_eq!(
            drawn(Some(([3840, 1080], [3826, 1076]))),
            "; pictures of 3840 by 1080 drawn on 3826 by 1076 pixels"
        );
        assert_eq!(drawn(None), "");
    }

    #[test]
    fn the_log_is_told_what_was_measured_and_nothing_else() {
        let mut counts = Counts::default();
        let before = counts;
        for _ in 0..1440 {
            counts.received(54_000, Some(0), Some(Duration::from_micros(3800)));
            counts.decoded(Some(Duration::from_millis(17)));
            counts.copied(Duration::from_millis(15));
        }
        let rates = Rates::between(&before, &counts, 0, 10.0).expect("ten seconds went by");
        assert_eq!(
            rates.told(Some(Duration::from_millis(11))),
            "a second of the stream: 144 pictures arrived, 144 decoded, 0 skipped, 62.2 Mbit/s, \
             0.0 % lost; a picture took 17.0 ms to decode, 15.0 ms of them to leave the decoder \
             when shown; the PC took 3.8 ms to make one; 11 ms to the PC and back"
        );

        // A backend that measures little says little.
        let mut counts = Counts::default();
        counts.received(125_000, None, None);
        counts.decoded(None);
        let rates = Rates::between(&before, &counts, 0, 1.0).expect("a second went by");
        assert_eq!(
            rates.told(None),
            "a second of the stream: 1 pictures arrived, 1 decoded, 0 skipped, 1.0 Mbit/s"
        );
    }

    #[test]
    fn two_counts_a_second_apart_make_rates() {
        let mut counts = Counts { sent: Some([1920, 1080, 60]), ..Counts::default() };
        let before = counts;
        for _ in 0..58 {
            counts.received(41_667, Some(0), Some(Duration::from_millis(4)));
            counts.decoded(Some(Duration::from_millis(6)));
        }
        // Two pictures never arrived.
        counts.received(41_667, Some(2), None);
        counts.decoded(Some(Duration::from_millis(6)));

        let rates = Rates::between(&before, &counts, 57, 1.0).expect("a second went by");
        assert_eq!(rates.received, 59.0);
        assert_eq!(rates.decoded, 59.0);
        assert_eq!(rates.shown, 57.0);
        assert!((rates.megabits - 19.67).abs() < 0.01, "{}", rates.megabits);
        let (share, lost) = rates.lost.expect("this backend knows what was lost");
        assert_eq!(lost, 2);
        assert!((share - 100.0 * 2.0 / 61.0).abs() < 1e-9, "{share}");
        assert!((rates.decoding_ms.expect("decoding is timed") - 6.0).abs() < 1e-9);
        assert!((rates.host_ms.expect("the PC said") - 4.0).abs() < 1e-9);

        assert_eq!((rates.copying_ms, rates.skipped), (None, 0.0));

        // Over half a second, twice the rate.
        let rates = Rates::between(&before, &counts, 57, 0.5).expect("time went by");
        assert_eq!(rates.received, 118.0);
    }

    #[test]
    fn pictures_copied_out_and_skipped_are_counted_apart() {
        let mut counts = Counts::default();
        let before = counts;
        for shown in [true, false, true, false] {
            counts.received(1000, Some(0), None);
            if shown {
                counts.decoded(Some(Duration::from_millis(9)));
                counts.copied(Duration::from_millis(7));
            } else {
                counts.decoded(Some(Duration::from_millis(2)));
                counts.skipped += 1;
            }
        }
        let rates = Rates::between(&before, &counts, 2, 1.0).expect("a second went by");
        assert!((rates.decoding_ms.expect("decoding is timed") - 5.5).abs() < 1e-9);
        assert!((rates.copying_ms.expect("copying is timed") - 7.0).abs() < 1e-9);
        assert_eq!(rates.skipped, 2.0);
    }

    #[test]
    fn what_a_backend_does_not_measure_is_not_made_up() {
        let mut counts = Counts::default();
        let before = counts;
        counts.received(1000, None, None);
        counts.decoded(None);
        let rates = Rates::between(&before, &counts, 1, 1.0).expect("a second went by");
        assert_eq!((rates.lost, rates.decoding_ms, rates.host_ms), (None, None, None));
        assert_eq!((rates.copying_ms, rates.skipped), (None, 0.0));
        // A backend that shows the pictures itself counts them.
        let shown = Counts { shown: Some(10), ..counts };
        let later = Counts { shown: Some(40), ..counts };
        assert_eq!(Rates::between(&shown, &later, 0, 1.0).map(|rates| rates.shown), Some(30.0));
        // No time, no rate.
        assert_eq!(Rates::between(&before, &counts, 1, 0.0), None);
    }
}
