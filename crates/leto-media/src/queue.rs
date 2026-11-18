//! The samples waiting between the network and the speakers.
//!
//! Sound arrives in small packets at the pace of the network, and the
//! speakers take it in chunks at their own pace. The queue in between is kept
//! short, because every sample waiting in it is delay between what happens
//! in the game and what is heard.

use std::collections::VecDeque;

/// How much the queue keeps beyond what the speakers take at once, before
/// it starts playing: room for the network's unevenness.
const CUSHION: f32 = 0.015;
/// How much may wait beyond the starting level before the oldest sound is
/// dropped to catch up.
const SLACK: f32 = 0.08;

/// Stereo sound on its way to speakers with `channels` channels.
pub(crate) struct Queue {
    /// Samples laid out for the speakers.
    samples: VecDeque<f32>,
    channels: usize,
    /// Frames (one sample per channel) the queue holds before it plays.
    cushion: usize,
    slack: usize,
    /// The most frames the speakers ever took at once.
    largest_take: usize,
    /// Playing. False while filling up, at the start and after running dry.
    playing: bool,
    /// Times the speakers wanted sound and there was none.
    pub dry_runs: u64,
    /// Frames dropped to catch up.
    pub dropped: u64,
}

impl Queue {
    pub fn new(sample_rate: u32, channels: usize) -> Self {
        let frames = |seconds: f32| (sample_rate as f32 * seconds) as usize;
        Self {
            samples: VecDeque::new(),
            channels: channels.max(1),
            cushion: frames(CUSHION),
            slack: frames(SLACK),
            largest_take: 0,
            playing: false,
            dry_runs: 0,
            dropped: 0,
        }
    }

    /// The level at which playing starts, in frames.
    fn start_level(&self) -> usize {
        self.largest_take + self.cushion
    }

    /// Adds stereo samples (left, right, left, right…).
    pub fn push(&mut self, stereo: &[f32]) {
        for pair in stereo.chunks_exact(2) {
            match self.channels {
                // One speaker gets both sides.
                1 => self.samples.push_back((pair[0] + pair[1]) / 2.0),
                // More than two: the front pair plays, the others stay quiet.
                channels => {
                    self.samples.extend(pair);
                    self.samples.extend(std::iter::repeat_n(0.0, channels - 2));
                }
            }
        }

        // Too much waiting means the speakers fell behind. Skip ahead rather
        // than stay late for the rest of the stream.
        let waiting = self.samples.len() / self.channels;
        if waiting > self.start_level() + self.slack {
            let excess = waiting - self.start_level();
            self.samples.drain(..excess * self.channels);
            self.dropped += excess as u64;
        }
    }

    /// Fills `out`, laid out for the speakers, with silence where there is
    /// no sound to give.
    pub fn take(&mut self, out: &mut [f32]) {
        let wanted = out.len() / self.channels;
        self.largest_take = self.largest_take.max(wanted);
        let waiting = self.samples.len() / self.channels;

        if !self.playing && waiting >= self.start_level() {
            self.playing = true;
        }
        if !self.playing {
            out.fill(0.0);
            return;
        }

        let given = out.len().min(self.samples.len());
        for (slot, sample) in out.iter_mut().zip(self.samples.drain(..given)) {
            *slot = sample;
        }
        if given < out.len() {
            // Ran dry: fill up again before playing on, or every chunk
            // from here on would have a hole in it.
            out[given..].fill(0.0);
            self.playing = false;
            self.dry_runs += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    /// What the speakers take at once in these tests: ten milliseconds.
    const TAKE: usize = 480;

    fn stereo(frames: usize, value: f32) -> Vec<f32> {
        vec![value; frames * 2]
    }

    #[test]
    fn nothing_plays_until_enough_is_waiting() {
        let mut queue = Queue::new(RATE, 2);
        let mut out = vec![9.0; TAKE * 2];

        queue.push(&stereo(TAKE, 0.5));
        queue.take(&mut out);
        assert!(out.iter().all(|sample| *sample == 0.0), "one chunk waiting is not enough");

        // One chunk plus the cushion of fifteen milliseconds.
        queue.push(&stereo(720, 0.5));
        queue.take(&mut out);
        assert!(out.iter().all(|sample| *sample == 0.5));
        assert_eq!(queue.dry_runs, 0);
    }

    #[test]
    fn running_dry_plays_what_is_left_then_fills_up_again() {
        let mut queue = Queue::new(RATE, 2);
        let mut out = vec![0.0; TAKE * 2];
        queue.take(&mut out);
        queue.push(&stereo(TAKE * 2 + 340, 0.5));
        queue.take(&mut out);
        queue.take(&mut out);
        assert_eq!(queue.dry_runs, 0);

        // 340 frames are left: they play, followed by silence.
        queue.take(&mut out);
        assert_eq!(queue.dry_runs, 1);
        assert!(out[..680].iter().all(|sample| *sample == 0.5));
        assert!(out[680..].iter().all(|sample| *sample == 0.0));

        // A little sound is not played at once.
        queue.push(&stereo(TAKE, 0.25));
        queue.take(&mut out);
        assert!(out.iter().all(|sample| *sample == 0.0));
        queue.push(&stereo(720, 0.25));
        queue.take(&mut out);
        assert!(out.iter().all(|sample| *sample == 0.25));
        assert_eq!(queue.dry_runs, 1);
    }

    #[test]
    fn a_pile_up_is_skipped_to_catch_up() {
        let mut queue = Queue::new(RATE, 2);
        let mut out = vec![0.0; TAKE * 2];
        queue.take(&mut out);

        // A second of sound at once: the speakers stalled.
        queue.push(&stereo(24_000, 0.1));
        queue.push(&stereo(24_000, 0.7));
        assert!(queue.dropped > 40_000, "{} frames dropped", queue.dropped);

        // What plays next is recent.
        queue.take(&mut out);
        assert!(out.iter().all(|sample| *sample == 0.7));
        assert_eq!(queue.dry_runs, 0);
    }

    #[test]
    fn one_speaker_gets_both_sides_and_many_get_the_front_pair() {
        let mut mono = Queue::new(RATE, 1);
        let mut out = vec![0.0; 4];
        mono.take(&mut out);
        mono.push(&[0.2, 0.6].repeat(1000));
        mono.take(&mut out);
        assert!(out.iter().all(|sample| (*sample - 0.4).abs() < 1e-6), "{out:?}");

        let mut surround = Queue::new(RATE, 6);
        let mut out = vec![9.0; 12];
        surround.take(&mut out);
        surround.push(&[0.2, 0.6].repeat(1000));
        surround.take(&mut out);
        assert_eq!(out, [0.2, 0.6, 0.0, 0.0, 0.0, 0.0, 0.2, 0.6, 0.0, 0.0, 0.0, 0.0]);
    }
}
