//! How finely the system counts time for Leto.
//!
//! Windows wakes a thread that waits a given time on the ticks of its
//! clock, which are 15.6 ms apart until a program asks for finer ones. A
//! stream waits a few milliseconds at a time in many places, some of them
//! Windows' own: the decoders of Media Foundation wait for the graphics
//! card that way. On coarse ticks each such wait can last a whole picture
//! or two. Moonlight asks for ticks a millisecond apart, through SDL, and so
//! does Leto while a stream runs.
//!
//! Since Windows 10 2004 the asking counts for the program that asks alone.
//! Other systems count finely without being asked.

/// Fine ticks, for as long as it lives.
#[derive(Debug)]
#[must_use = "the clock is coarse again as soon as this is dropped"]
pub struct Fine {
    /// Windows agreed, and is to be told when the ticks are no longer needed.
    #[cfg_attr(not(windows), allow(dead_code))]
    asked: bool,
}

/// Asks the system to count time finely from now on. Asking twice is fine:
/// the clock stays fine until the last one is dropped.
pub fn fine() -> Fine {
    #[cfg(windows)]
    return Fine { asked: windows::begin() };
    #[cfg(not(windows))]
    Fine { asked: false }
}

impl Fine {
    /// Whether the system was asked and agreed. `false` where it counts
    /// finely by itself.
    pub const fn asked(&self) -> bool {
        self.asked
    }
}

impl Drop for Fine {
    fn drop(&mut self) {
        #[cfg(windows)]
        if self.asked {
            windows::end();
        }
    }
}

#[cfg(windows)]
mod windows {
    use windows_sys::Win32::Media::{TIMERR_NOERROR, timeBeginPeriod, timeEndPeriod};

    /// The ticks asked for, in milliseconds: the finest this call gives.
    const TICK: u32 = 1;

    pub fn begin() -> bool {
        // SAFETY: a number in, a number out.
        let agreed = unsafe { timeBeginPeriod(TICK) } == TIMERR_NOERROR;
        if !agreed {
            log::warn!("Windows keeps its coarse clock, so short waits last up to 16 ms");
        }
        agreed
    }

    pub fn end() {
        // SAFETY: as above, with the number given to `begin`.
        unsafe { timeEndPeriod(TICK) };
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn a_short_wait_is_short_while_the_clock_is_fine() {
        let clock = fine();
        // Asked for twice, and let go of once: still fine.
        drop(fine());
        assert_eq!(clock.asked(), cfg!(windows));
        // A wait on a condition nobody signals, as the engine's are. The
        // shortest of several: a busy machine can make any one of them long.
        let (lock, signal) = (std::sync::Mutex::new(()), std::sync::Condvar::new());
        let shortest = (0..20)
            .map(|_| {
                let start = Instant::now();
                let guard = lock.lock().expect("nobody else holds it");
                drop(signal.wait_timeout(guard, Duration::from_millis(2)).expect("it waits"));
                start.elapsed()
            })
            .min()
            .expect("it waited");
        assert!(shortest < Duration::from_millis(10), "the shortest wait took {shortest:?}");
    }
}
