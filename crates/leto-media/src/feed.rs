//! A camera on its way out: its pictures captured, brought down to what
//! whoever takes them wants, encoded and handed over one by one.
//!
//! One thread does it for each feed. This crate knows nothing about the
//! network: a [`Carrier`] takes the encoded pictures from here.

use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use crate::{Camera, Encoded, Picture, PictureEncoder, Sending};

/// How many pictures a second a camera is taken to give. Most give thirty,
/// and fewer in the dark, which only leaves the encoder with bits to spare.
const CAMERA_FPS: u32 = 30;

/// The longest a feed goes without looking whether it is still wanted.
const NAP: Duration = Duration::from_millis(100);

/// Whatever carries the encoded pictures of a camera away.
pub trait Carrier: Send + 'static {
    /// Called once, when the camera gave its first picture: pictures like
    /// `sending` follow. An error ends the feed: it is why nobody takes
    /// them, in whatever words the feed should then give.
    fn open(&mut self, sending: Sending) -> Result<(), String>;

    /// One picture, taken `at` this long after the first one. Says whether
    /// all of it left: when not, the next one is made to decode on its own.
    /// `None` when nobody takes pictures any more, which ends the feed.
    fn carry(&mut self, at: Duration, picture: Encoded<'_>) -> Option<bool>;
}

/// The most that whoever takes a camera wants of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The largest picture, in pixels.
    pub width: u32,
    pub height: u32,
    /// The most pictures a second.
    pub fps: u32,
}

/// What a feed is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Feeding {
    /// The camera is opening, or its carrier is.
    Starting,
    /// Pictures are leaving, like this.
    Sending(Sending),
    /// Nobody takes pictures any more.
    Over,
    /// The camera could not be used, or its pictures not encoded: why, as
    /// the end of a sentence.
    Lost(String),
    /// The carrier did not open: why, in its own words.
    Refused(String),
}

/// What left so far.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Fed {
    /// Pictures that left whole.
    pub pictures: u64,
    /// How many of them decode on their own.
    pub key_frames: u64,
    /// Their size once encoded, in bytes.
    pub bytes: u64,
}

/// A camera whose pictures leave for as long as this is kept. Capturing
/// starts when it is started and stops when it is dropped.
pub struct CameraFeed {
    shared: Arc<Shared>,
    /// Wakes the thread of the feed.
    wake: mpsc::SyncSender<()>,
    thread: Option<thread::JoinHandle<()>>,
}

struct Shared {
    /// Set once nobody wants the feed any more.
    let_go: AtomicBool,
    state: Mutex<Feeding>,
    pictures: AtomicU64,
    key_frames: AtomicU64,
    bytes: AtomicU64,
}

impl Shared {
    fn set(&self, state: Feeding) {
        *self.state.lock().unwrap_or_else(PoisonError::into_inner) = state;
    }
}

impl CameraFeed {
    /// Starts sending the system's camera through `carrier`, within
    /// `limits`, and returns at once. `news` is called, on a thread of the
    /// feed, whenever [`CameraFeed::state`] changes.
    pub fn open(limits: Limits, carrier: impl Carrier, news: impl Fn() + Send + 'static) -> Self {
        Self::start(Camera::open, limits, carrier, news)
    }

    /// The same with a test card for a camera, for trying things out where
    /// there is none.
    pub fn test_card(
        limits: Limits,
        carrier: impl Carrier,
        news: impl Fn() + Send + 'static,
    ) -> Self {
        Self::start(Camera::test_card, limits, carrier, news)
    }

    fn start(
        camera: fn(Box<dyn Fn() + Send>) -> Camera,
        limits: Limits,
        carrier: impl Carrier,
        news: impl Fn() + Send + 'static,
    ) -> Self {
        let shared = Arc::new(Shared {
            let_go: AtomicBool::new(false),
            state: Mutex::new(Feeding::Starting),
            pictures: AtomicU64::new(0),
            key_frames: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
        });
        // One place is enough: a feed that is already told to look does not
        // need telling twice.
        let (wake, woken) = mpsc::sync_channel(1);
        let feed = {
            let (shared, wake) = (Arc::clone(&shared), wake.clone());
            move || {
                let fresh = move || {
                    let _ = wake.try_send(());
                };
                let camera = camera(Box::new(fresh));
                let end = run(&shared, &camera, &woken, limits, carrier, &news);
                // The camera is let go of before anyone hears that it is.
                drop(camera);
                if let Some(end) = end {
                    shared.set(end);
                    news();
                }
            }
        };
        let thread = thread::Builder::new().name("camera feed".to_owned()).spawn(feed);
        let thread = thread
            .inspect_err(|error| {
                shared.set(Feeding::Lost(format!("the system gave no thread to send it: {error}")));
            })
            .ok();
        Self { shared, wake, thread }
    }

    /// What the feed is doing.
    pub fn state(&self) -> Feeding {
        self.shared.state.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// What left so far.
    pub fn fed(&self) -> Fed {
        Fed {
            pictures: self.shared.pictures.load(Ordering::Relaxed),
            key_frames: self.shared.key_frames.load(Ordering::Relaxed),
            bytes: self.shared.bytes.load(Ordering::Relaxed),
        }
    }

    /// Stops the feed and waits until its camera and its carrier are let go
    /// of. Dropping it stops it too, without waiting.
    pub fn stop(mut self) {
        self.let_go();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    fn let_go(&self) {
        self.shared.let_go.store(true, Ordering::Relaxed);
        let _ = self.wake.try_send(());
    }
}

impl Drop for CameraFeed {
    fn drop(&mut self) {
        self.let_go();
    }
}

/// Feeds until the feed is let go of, which returns `None`, or ends by
/// itself, which returns how.
fn run(
    shared: &Shared,
    camera: &Camera,
    woken: &mpsc::Receiver<()>,
    limits: Limits,
    mut carrier: impl Carrier,
    news: &dyn Fn(),
) -> Option<Feeding> {
    let mut picture = Picture::default();
    let mut encoder: Option<(PictureEncoder, Instant)> = None;
    // When the next picture is due, counted from the first one.
    let mut due = Duration::ZERO;
    let mut key_frame = false;
    loop {
        let _ = woken.recv_timeout(NAP);
        if shared.let_go.load(Ordering::Relaxed) {
            return None;
        }
        match camera.take(&mut picture) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(error) => return Some(Feeding::Lost(error.to_string())),
        }

        if encoder.is_none() {
            // The first picture says how large the camera's are.
            let largest = (limits.width, limits.height);
            let Some(sending) =
                Sending::fit(picture.width, picture.height, CAMERA_FPS, largest, limits.fps)
            else {
                let (width, height) = (picture.width, picture.height);
                let why = format!("its pictures of {width} by {height} cannot be sent");
                return Some(Feeding::Lost(why));
            };
            let made = match PictureEncoder::new(sending) {
                Ok(made) => made,
                Err(error) => return Some(Feeding::Lost(error.to_string())),
            };
            if let Err(why) = carrier.open(sending) {
                return Some(Feeding::Refused(why));
            }
            // Asking took a moment, and the feed may not be wanted any more.
            if shared.let_go.load(Ordering::Relaxed) {
                return None;
            }
            shared.set(Feeding::Sending(sending));
            news();
            encoder = Some((made, Instant::now()));
        }
        let Some((encoder, started)) = &mut encoder else { continue };

        // No more pictures than were promised: a camera that is faster has
        // the others left out. One that comes a little early is on time.
        let at = started.elapsed();
        let step = Duration::from_secs(1) / encoder.sending().fps;
        if at + step / 4 < due {
            continue;
        }
        due = (due + step).max(at + step / 2);

        let encoded = match encoder.encode(&picture, key_frame) {
            Ok(Some(encoded)) => encoded,
            // Left out by the encoder, to keep to its bit rate.
            Ok(None) => continue,
            Err(error) => return Some(Feeding::Lost(error.to_string())),
        };
        match carrier.carry(at, encoded) {
            Some(true) => {
                key_frame = false;
                shared.pictures.fetch_add(1, Ordering::Relaxed);
                shared.key_frames.fetch_add(u64::from(encoded.key_frame), Ordering::Relaxed);
                shared.bytes.fetch_add(encoded.data.len() as u64, Ordering::Relaxed);
            }
            // What follows builds on a picture that did not arrive.
            Some(false) => key_frame = true,
            None => return Some(Feeding::Over),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::VideoDecoder;

    /// How long any feed of the tests may take: cameras wait for each other
    /// on one thread.
    const PATIENCE: Duration = Duration::from_secs(30);

    const ANY: Limits = Limits { width: 1920, height: 1080, fps: 30 };

    /// What a carrier was handed, for a test to look at.
    #[derive(Default)]
    struct Carried {
        opened: Option<Sending>,
        /// When each picture was taken, whether it decodes on its own, and
        /// what it is.
        pictures: Vec<(Duration, bool, Vec<u8>)>,
        /// The carrier was let go of.
        dropped: bool,
    }

    /// A carrier that keeps what it is handed.
    struct Keeper {
        carried: Arc<Mutex<Carried>>,
        /// What `open` answers when set.
        refuses: Option<&'static str>,
        /// The number of the picture that does not leave whole, and of the
        /// one after which nobody takes any.
        loses: Option<usize>,
        ends_after: Option<usize>,
    }

    impl Keeper {
        fn new() -> (Self, Arc<Mutex<Carried>>) {
            let carried = Arc::new(Mutex::new(Carried::default()));
            let keeper = Self {
                carried: Arc::clone(&carried),
                refuses: None,
                loses: None,
                ends_after: None,
            };
            (keeper, carried)
        }
    }

    impl Carrier for Keeper {
        fn open(&mut self, sending: Sending) -> Result<(), String> {
            self.carried.lock().unwrap_or_else(PoisonError::into_inner).opened = Some(sending);
            self.refuses.map_or(Ok(()), |why| Err(why.to_owned()))
        }

        fn carry(&mut self, at: Duration, picture: Encoded<'_>) -> Option<bool> {
            let mut carried = self.carried.lock().unwrap_or_else(PoisonError::into_inner);
            let number = carried.pictures.len();
            if self.ends_after.is_some_and(|last| number > last) {
                return None;
            }
            carried.pictures.push((at, picture.key_frame, picture.data.to_vec()));
            Some(self.loses != Some(number))
        }
    }

    impl Drop for Keeper {
        fn drop(&mut self) {
            self.carried.lock().unwrap_or_else(PoisonError::into_inner).dropped = true;
        }
    }

    fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
        let start = Instant::now();
        while !done() {
            assert!(start.elapsed() < PATIENCE, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn count(carried: &Mutex<Carried>) -> usize {
        carried.lock().unwrap_or_else(PoisonError::into_inner).pictures.len()
    }

    #[test]
    fn what_a_camera_sees_leaves_encoded_at_its_pace_until_the_feed_is_stopped() {
        let (keeper, carried) = Keeper::new();
        let (told, news) = mpsc::channel();
        let feed = CameraFeed::test_card(ANY, keeper, move || {
            let _ = told.send(());
        });
        news.recv_timeout(PATIENCE).expect("the feed said that it sends");
        // The card is 640 by 360, thirty times a second.
        let sending = Sending { width: 640, height: 360, fps: 30, bitrate_kbps: 625 };
        assert_eq!(feed.state(), Feeding::Sending(sending));
        wait_for("pictures", || count(&carried) >= 20);
        let fed = feed.fed();
        assert!(fed.pictures >= 20 && fed.key_frames >= 1 && fed.bytes > 0, "{fed:?}");

        feed.stop();
        let carried = carried.lock().unwrap_or_else(PoisonError::into_inner);
        assert!(carried.dropped, "a feed that stopped keeps nothing");
        assert_eq!(carried.opened, Some(sending));
        let (first, last) = (&carried.pictures[0], &carried.pictures[carried.pictures.len() - 1]);
        assert!(first.1, "the first picture decodes on its own");
        // About thirty a second, by the clock that the pictures carry.
        let pace = (carried.pictures.len() - 1) as f64 / (last.0 - first.0).as_secs_f64();
        assert!((20.0..=33.0).contains(&pace), "{pace} pictures a second");

        // And they are the card: white on the left, black on the right.
        let mut decoder = VideoDecoder::new().expect("a decoder");
        let mut picture = Picture::default();
        for (_, _, data) in &carried.pictures {
            decoder.decode(data, &mut picture).expect("what was encoded decodes");
        }
        assert_eq!((picture.width, picture.height), (640, 360));
        let pixel = |column: usize| &picture.rgba[(8 * 640 + column) * 4..][..3];
        assert!(pixel(8).iter().all(|channel| *channel > 235), "{:?}", pixel(8));
        assert!(pixel(632).iter().all(|channel| *channel < 20), "{:?}", pixel(632));
    }

    #[test]
    fn a_camera_is_brought_down_to_what_is_wanted_of_it() {
        let (keeper, carried) = Keeper::new();
        let small = Limits { width: 400, height: 300, fps: 10 };
        let feed = CameraFeed::test_card(small, keeper, || {});
        wait_for("pictures", || count(&carried) >= 5);
        feed.stop();

        let carried = carried.lock().unwrap_or_else(PoisonError::into_inner);
        let sending = carried.opened.expect("the carrier was opened");
        assert_eq!((sending.width, sending.height, sending.fps), (320, 180, 10));
        let (first, last) = (&carried.pictures[0], &carried.pictures[carried.pictures.len() - 1]);
        let pace = (carried.pictures.len() - 1) as f64 / (last.0 - first.0).as_secs_f64();
        assert!((6.0..=11.0).contains(&pace), "{pace} pictures a second");
    }

    #[test]
    fn a_picture_that_did_not_leave_whole_is_followed_by_one_that_stands_alone() {
        let (mut keeper, carried) = Keeper::new();
        keeper.loses = Some(3);
        let feed = CameraFeed::test_card(ANY, keeper, || {});
        wait_for("pictures", || count(&carried) >= 6);
        feed.stop();

        let carried = carried.lock().unwrap_or_else(PoisonError::into_inner);
        let key_frames: Vec<bool> = carried.pictures[..6].iter().map(|picture| picture.1).collect();
        assert_eq!(key_frames, [true, false, false, false, true, false]);
    }

    #[test]
    fn a_feed_ends_when_nobody_takes_its_pictures_and_says_why_it_never_started() {
        let (mut keeper, carried) = Keeper::new();
        keeper.ends_after = Some(2);
        let feed = CameraFeed::test_card(ANY, keeper, || {});
        wait_for("the feed to end", || feed.state() == Feeding::Over);
        assert_eq!(count(&carried), 3);
        wait_for("the carrier to be let go of", || {
            carried.lock().unwrap_or_else(PoisonError::into_inner).dropped
        });
        drop(feed);

        let (mut keeper, carried) = Keeper::new();
        keeper.refuses = Some("The PC said no.");
        let (told, news) = mpsc::channel();
        let feed = CameraFeed::test_card(ANY, keeper, move || {
            let _ = told.send(());
        });
        news.recv_timeout(PATIENCE).expect("the feed said that it ended");
        assert_eq!(feed.state(), Feeding::Refused("The PC said no.".to_owned()));
        assert_eq!(count(&carried), 0);
        drop(feed);

        // Nothing fits in a picture this small.
        let (keeper, carried) = Keeper::new();
        let tiny = Limits { width: 16, height: 16, fps: 30 };
        let feed = CameraFeed::test_card(tiny, keeper, || {});
        wait_for("the feed to give up", || matches!(feed.state(), Feeding::Lost(_)));
        assert_eq!(carried.lock().unwrap_or_else(PoisonError::into_inner).opened, None);
    }
}
