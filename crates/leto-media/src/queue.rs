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
/// How much the cushion grows each time the speakers run dry: a device or
/// a network that cannot keep up with the shortest delay gets a longer one
/// rather than a hole in the sound again and again.
const GROWTH: f32 = 0.01;
/// The longest the cushion grows to.
const LONGEST_CUSHION: f32 = 0.1;
/// How long the sound must play without running dry for the cushion to
/// shrink by one step again: what made it grow may have passed, and every
/// step is delay.
const CALM: f32 = 10.0;
/// How long the sound takes to fade when it stops for want of more, and
/// to come up when it starts: a sound cut at full level, or started there,
/// is heard as a click.
const FADE: f32 = 0.004;

/// Stereo sound on its way to speakers with `channels` channels.
pub(crate) struct Queue {
    /// Samples laid out for the speakers.
    samples: VecDeque<f32>,
    channels: usize,
    /// Frames (one sample per channel) the queue holds before it plays.
    cushion: usize,
    growth: usize,
    longest_cushion: usize,
    /// The cushion it started with, the shortest it shrinks back to.
    shortest_cushion: usize,
    calm: usize,
    /// Frames played since the last time it ran dry, or the cushion shrank.
    played_calmly: usize,
    slack: usize,
    /// The most frames the speakers ever took at once.
    largest_take: usize,
    /// Frames a fade lasts.
    fade: usize,
    /// Frames of the fade in still to play, when playing has just started.
    rising: usize,
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
            growth: frames(GROWTH),
            longest_cushion: frames(LONGEST_CUSHION),
            shortest_cushion: frames(CUSHION),
            calm: frames(CALM),
            played_calmly: 0,
            slack: frames(SLACK),
            largest_take: 0,
            fade: frames(FADE).max(1),
            rising: 0,
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
            self.rising = self.fade;
        }
        if !self.playing {
            out.fill(0.0);
            return;
        }

        let given = out.len().min(self.samples.len());
        for (slot, sample) in out.iter_mut().zip(self.samples.drain(..given)) {
            *slot = sample;
        }
        // Coming up from silence.
        let channels = self.channels;
        for frame in out[..given].chunks_mut(channels) {
            if self.rising == 0 {
                break;
            }
            let level = 1.0 - self.rising as f32 / self.fade as f32;
            frame.iter_mut().for_each(|sample| *sample *= level);
            self.rising -= 1;
        }
        if given < out.len() {
            // Ran dry: fill up again before playing on, or every chunk
            // from here on would have a hole in it.
            out[given..].fill(0.0);
            self.playing = false;
            self.dry_runs += 1;
            self.cushion = (self.cushion + self.growth).min(self.longest_cushion);
            self.played_calmly = 0;
        } else {
            self.played_calmly += wanted;
            if self.played_calmly >= self.calm {
                self.played_calmly = 0;
                self.cushion = self.cushion.saturating_sub(self.growth).max(self.shortest_cushion);
            }
        }
        if self.samples.is_empty() && given > 0 {
            // The last of the sound for now, as when the PC plays nothing
            // more: it goes down to silence rather than stop where it was.
            let frames = (given / channels).min(self.fade);
            let start = given - frames * channels;
            for (place, frame) in out[start..given].chunks_mut(channels).enumerate() {
                let level = 1.0 - (place + 1) as f32 / frames as f32;
                frame.iter_mut().for_each(|sample| *sample *= level);
            }
            // What comes next comes up from that silence.
            self.rising = self.fade;
        }
    }

    /// How long the queue waits for before playing, in frames, beyond what
    /// the speakers take at once.
    pub fn cushion(&self) -> usize {
        self.cushion
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    /// What the speakers take at once in these tests: ten milliseconds.
    const TAKE: usize = 480;

    /// A queue that neither fades in nor out, for the tests that count
    /// samples.
    fn abrupt(channels: usize) -> Queue {
        Queue { fade: 0, ..Queue::new(RATE, channels) }
    }

    fn stereo(frames: usize, value: f32) -> Vec<f32> {
        vec![value; frames * 2]
    }

    #[test]
    fn nothing_plays_until_enough_is_waiting() {
        let mut queue = abrupt(2);
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
        let mut queue = abrupt(2);
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
        // Having run dry, it waits ten milliseconds more than at first.
        queue.push(&stereo(720 + 480, 0.25));
        queue.take(&mut out);
        assert!(out.iter().all(|sample| *sample == 0.25));
        assert_eq!(queue.dry_runs, 1);
    }

    #[test]
    fn running_dry_again_and_again_waits_longer_each_time_up_to_a_tenth() {
        let mut queue = abrupt(2);
        let mut out = vec![0.0; TAKE * 2];
        queue.take(&mut out);
        let start = queue.cushion();
        // Plays as much as asked for before starting, then runs dry.
        let mut run_dry = |queue: &mut Queue| {
            let dry_runs = queue.dry_runs;
            queue.push(&stereo(TAKE + queue.cushion(), 1.0));
            while queue.dry_runs == dry_runs {
                queue.take(&mut out);
            }
        };
        run_dry(&mut queue);
        assert_eq!(queue.cushion(), start + 480);
        for _ in 0..20 {
            run_dry(&mut queue);
        }
        assert_eq!(queue.cushion(), 4_800);
    }

    #[test]
    fn after_a_calm_while_the_cushion_shrinks_back_to_where_it_started() {
        let mut queue = Queue::new(RATE, 2);
        let mut out = vec![0.0; TAKE * 2];
        queue.take(&mut out);
        let start = queue.cushion();
        for _ in 0..3 {
            let dry_runs = queue.dry_runs;
            queue.push(&stereo(TAKE + queue.cushion(), 1.0));
            while queue.dry_runs == dry_runs {
                queue.take(&mut out);
            }
        }
        assert_eq!(queue.cushion(), start + 3 * 480);

        // Ten seconds of sound that arrives on time, a chunk at a time.
        let play_calmly = |queue: &mut Queue, out: &mut Vec<f32>, seconds: usize| {
            for _ in 0..seconds * 100 {
                queue.push(&stereo(TAKE, 1.0));
                queue.take(out);
            }
        };
        queue.push(&stereo(TAKE + queue.cushion(), 1.0));
        play_calmly(&mut queue, &mut out, 10);
        assert_eq!(queue.dry_runs, 3);
        assert_eq!(queue.cushion(), start + 2 * 480, "one step less");
        play_calmly(&mut queue, &mut out, 60);
        assert_eq!(queue.cushion(), start, "never shorter than at first");
    }

    #[test]
    fn a_pile_up_is_skipped_to_catch_up() {
        let mut queue = abrupt(2);
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
        let mut mono = abrupt(1);
        let mut out = vec![0.0; 4];
        mono.take(&mut out);
        mono.push(&[0.2, 0.6].repeat(1000));
        mono.take(&mut out);
        assert!(out.iter().all(|sample| (*sample - 0.4).abs() < 1e-6), "{out:?}");

        let mut surround = abrupt(6);
        let mut out = vec![9.0; 12];
        surround.take(&mut out);
        surround.push(&[0.2, 0.6].repeat(1000));
        surround.take(&mut out);
        assert_eq!(out, [0.2, 0.6, 0.0, 0.0, 0.0, 0.0, 0.2, 0.6, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn sound_comes_up_from_silence_and_goes_down_to_it_when_it_ends() {
        let mut queue = Queue::new(RATE, 2);
        let mut out = vec![0.0; TAKE * 2];
        queue.take(&mut out);
        // Enough to start, and exactly three chunks: then nothing more, as
        // when the PC stops playing.
        queue.push(&stereo(TAKE * 3, 0.5));
        queue.take(&mut out);
        assert_eq!(out[0], 0.0, "starts from silence");
        assert!(out[..384].windows(2).all(|pair| pair[0] <= pair[1]), "and comes up");
        assert!(out[384..].iter().all(|sample| *sample == 0.5));

        queue.take(&mut out);
        assert!(out.iter().all(|sample| *sample == 0.5));
        queue.take(&mut out);
        assert!(out[..TAKE * 2 - 384].iter().all(|sample| *sample == 0.5));
        assert!(out[TAKE * 2 - 384..].windows(2).all(|pair| pair[0] >= pair[1]), "goes down");
        assert_eq!(out[TAKE * 2 - 1], 0.0, "to silence");
        assert_eq!(queue.dry_runs, 0);
    }
}
