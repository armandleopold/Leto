//! The camera: what it sees, as pixels.
//!
//! Opening a camera takes a moment and reading it waits for every picture,
//! so neither is done where a window is drawn. One thread does both for
//! every camera this crate opens, one camera after the other: Windows wants
//! its cameras used from the thread that first asked about them.

mod card;
#[cfg(any(windows, target_os = "linux"))]
mod device;
// Chosen among what a system's camera offers, where Leto reads one.
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
mod format;
mod pixels;

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex, MutexGuard, OnceLock, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

use self::pixels::Raw;
use crate::{Error, Picture};

/// Pictures in a row that may be unreadable before the camera is given up
/// on. Cameras that compress send one now and then, mostly at first.
const UNREADABLE: u32 = 30;

/// Captures a camera as pictures.
///
/// Capturing starts when this is opened and stops when it is dropped:
/// nothing is looked at in between two of them.
pub struct Camera {
    shared: Arc<Shared>,
}

/// What a camera and the thread that reads it share.
#[derive(Default)]
struct Shared {
    /// Set once nobody wants the pictures any more.
    let_go: AtomicBool,
    slot: Mutex<Slot>,
}

#[derive(Default)]
struct Slot {
    /// The latest picture, until someone takes it.
    fresh: Option<Picture>,
    /// A picture nobody needs any more: the next one reuses its memory.
    spare: Option<Picture>,
    /// Why the camera gave up.
    problem: Option<String>,
}

impl Shared {
    fn slot(&self) -> MutexGuard<'_, Slot> {
        // Pictures are whole or absent: what a thread that panicked left is
        // as good as before.
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Something that delivers pictures one after the other.
trait Source {
    /// Waits for the next picture and hands it over as it was made.
    fn next(&mut self) -> Result<Raw<'_>, Error>;
}

/// Which pictures a [`Camera`] captures.
enum Which {
    /// Those of the system's camera by this name, or of the first one with
    /// colour.
    System(Option<String>),
    /// A test card.
    Card,
    /// What goes wrong with cameras, for the tests.
    #[cfg(test)]
    Missing,
    #[cfg(test)]
    Panicking,
}

/// One camera's turn on the thread of the cameras.
struct Turn {
    shared: Arc<Shared>,
    which: Which,
    news: Box<dyn Fn() + Send>,
}

impl Camera {
    /// Starts capturing the system's camera, and returns at once: whether
    /// it could be opened shows in [`Camera::take`] a moment later.
    ///
    /// `news` is called on the camera's thread every time there is a picture
    /// to take, and once if the camera gives up.
    pub fn open(news: impl Fn() + Send + 'static) -> Self {
        Self::start(Which::System(None), Box::new(news))
    }

    /// The same with the camera called `name`, as [`camera_names`] lists
    /// it. One that is not there is replaced by the first with colour.
    pub fn open_named(name: Option<String>, news: impl Fn() + Send + 'static) -> Self {
        Self::start(Which::System(name), Box::new(news))
    }

    /// Starts handing out a test card as a camera would hand out what it
    /// sees, for trying things out where there is no camera.
    pub fn test_card(news: impl Fn() + Send + 'static) -> Self {
        Self::start(Which::Card, Box::new(news))
    }

    fn start(which: Which, news: Box<dyn Fn() + Send>) -> Self {
        let shared = Arc::new(Shared::default());
        if !queue(Job::Capture(Turn { shared: Arc::clone(&shared), which, news })) {
            shared.slot().problem = Some("the system gave no thread to read it on".to_owned());
        }
        Self { shared }
    }

    /// Puts the latest picture in `picture` if nobody took it yet, and says
    /// whether it did. The memory of the picture it replaces is reused.
    ///
    /// An error is why the camera could not be opened or gave up, and stays
    /// from then on: there will be no more pictures.
    pub fn take(&self, picture: &mut Picture) -> Result<bool, Error> {
        let mut slot = self.shared.slot();
        if let Some(problem) = &slot.problem {
            return Err(Error::Camera(problem.clone()));
        }
        let Some(fresh) = slot.fresh.take() else { return Ok(false) };
        slot.spare = Some(std::mem::replace(picture, fresh));
        Ok(true)
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        self.shared.let_go.store(true, Ordering::Relaxed);
    }
}

/// Work for the thread of the cameras.
enum Job {
    Capture(Turn),
    /// Look at which cameras there are, then call this.
    Look(Box<dyn Fn() + Send>),
}

/// The names of the system's cameras, as last looked at.
static NAMES: Mutex<Option<Vec<String>>> = Mutex::new(None);

/// The names of the system's cameras, as they were last looked at: when a
/// camera was opened, or [`look_for_cameras`] looked. `None` before that.
pub fn camera_names() -> Option<Vec<String>> {
    NAMES.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Looks at which cameras the system has, on the thread of the cameras
/// once a camera capturing there is let go of, and calls `news` once
/// [`camera_names`] knows.
pub fn look_for_cameras(news: impl Fn() + Send + 'static) {
    if !queue(Job::Look(Box::new(news))) {
        remember(Vec::new());
    }
}

fn remember(names: Vec<String>) {
    *NAMES.lock().unwrap_or_else(PoisonError::into_inner) = Some(names);
}

/// Queues `job` for the thread of the cameras, which starts the first
/// time. `false` when there is no such thread.
fn queue(job: Job) -> bool {
    static JOBS: OnceLock<Option<mpsc::Sender<Job>>> = OnceLock::new();
    let jobs = JOBS.get_or_init(|| {
        let (jobs, waiting) = mpsc::channel::<Job>();
        let reader = move || {
            for job in waiting {
                match job {
                    Job::Capture(turn) => turn.run(),
                    Job::Look(news) => {
                        // Not Leto's code either.
                        let looked = catch_unwind(look);
                        remember(looked.unwrap_or_default());
                        news();
                    }
                }
            }
        };
        thread::Builder::new().name("camera".to_owned()).spawn(reader).ok().map(|_| jobs)
    });
    jobs.as_ref().is_some_and(|jobs| jobs.send(job).is_ok())
}

/// The names of the system's cameras.
fn look() -> Vec<String> {
    #[cfg(any(windows, target_os = "linux"))]
    match device::names() {
        Ok(names) => names,
        Err(error) => {
            log::debug!("the cameras could not be listed: {error}");
            Vec::new()
        }
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    Vec::new()
}

impl Turn {
    fn run(self) {
        let Self { shared, which, news } = self;
        // What a system has for cameras is not Leto's code. If it panics,
        // the thread is still there for the next camera.
        let outcome = catch_unwind(AssertUnwindSafe(|| capture(&shared, which, &*news)));
        let problem = match outcome {
            Ok(Ok(())) => return,
            Ok(Err(Error::Camera(problem))) => problem,
            Ok(Err(other)) => other.to_string(),
            Err(_) => "it stopped working".to_owned(),
        };
        log::warn!("the camera gave up: {problem}");
        shared.slot().problem = Some(problem);
        news();
    }
}

/// Reads a camera until it is let go of.
fn capture(shared: &Shared, which: Which, news: &dyn Fn()) -> Result<(), Error> {
    // Let go of while it waited for its turn: the camera is not even opened.
    if shared.let_go.load(Ordering::Relaxed) {
        return Ok(());
    }
    let mut source = which.open()?;
    let mut unreadable = 0;
    while !shared.let_go.load(Ordering::Relaxed) {
        let raw = source.next()?;
        let mut picture = shared.slot().spare.take().unwrap_or_default();
        match pixels::read(&raw, &mut picture) {
            Ok(()) => {
                unreadable = 0;
                let mut slot = shared.slot();
                slot.spare = slot.fresh.replace(picture);
                drop(slot);
                news();
            }
            Err(error) if unreadable < UNREADABLE => {
                log::debug!("a picture of the camera was skipped: {error}");
                unreadable += 1;
                shared.slot().spare = Some(picture);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

impl Which {
    fn open(self) -> Result<Box<dyn Source>, Error> {
        match self {
            #[cfg(any(windows, target_os = "linux"))]
            Self::System(name) => Ok(Box::new(device::Device::open(name.as_deref())?)),
            #[cfg(not(any(windows, target_os = "linux")))]
            Self::System(_) => {
                Err(Error::Camera("Leto does not read cameras on this system yet".to_owned()))
            }
            Self::Card => Ok(Box::new(card::Card::new())),
            #[cfg(test)]
            Self::Missing => Err(Error::Camera("this device has no camera".to_owned())),
            #[cfg(test)]
            Self::Panicking => panic!("a camera that panics, for a test"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// How long any camera of the tests may take: they wait for each other
    /// on the one thread.
    const PATIENCE: Duration = Duration::from_secs(30);

    /// A camera of the tests, with the news it gives.
    fn start(which: Which) -> (Camera, mpsc::Receiver<()>) {
        let (told, news) = mpsc::channel();
        let camera = Camera::start(
            which,
            Box::new(move || {
                let _ = told.send(());
            }),
        );
        (camera, news)
    }

    fn pixel(picture: &Picture, column: usize, row: usize) -> [u8; 3] {
        let at = (row * picture.width + column) * 4;
        [0, 1, 2].map(|byte| picture.rgba[at + byte])
    }

    #[test]
    fn a_camera_hands_out_pictures_until_it_is_let_go_of() {
        let (camera, news) = start(Which::Card);
        let mut picture = Picture::default();
        news.recv_timeout(PATIENCE).expect("a picture came");
        assert_eq!(camera.take(&mut picture).ok(), Some(true));
        assert_eq!((picture.width, picture.height), (card::WIDTH, card::HEIGHT));
        assert_eq!(picture.rgba.len(), card::WIDTH * card::HEIGHT * 4);
        // The card's first bar is white and its last one black, and its
        // sixth is red: colours come out as they went in.
        assert!(pixel(&picture, 8, 8).iter().all(|channel| *channel > 250));
        assert!(pixel(&picture, card::WIDTH - 8, 8).iter().all(|channel| *channel < 5));
        let [red, green, blue] = pixel(&picture, card::WIDTH * 11 / 16, 8);
        assert!(red > 250 && green < 5 && blue < 5, "{red} {green} {blue}");

        // The next picture is another one: the square has moved on.
        let first = picture.clone();
        let later = loop {
            news.recv_timeout(PATIENCE).expect("another picture came");
            if camera.take(&mut picture).ok() == Some(true) && picture != first {
                break picture.clone();
            }
        };
        assert_eq!((later.width, later.height), (first.width, first.height));

        // Once it is let go of the thread is free for the next camera.
        drop(camera);
        let (camera, news) = start(Which::Card);
        news.recv_timeout(PATIENCE).expect("the next camera got its turn");
        assert_eq!(camera.take(&mut picture).ok(), Some(true));
    }

    #[test]
    fn a_camera_that_cannot_be_opened_says_why_and_leaves_the_thread_to_the_next() {
        let (camera, news) = start(Which::Missing);
        news.recv_timeout(PATIENCE).expect("the camera gave up");
        let mut picture = Picture::default();
        for _ in 0..2 {
            let error = camera.take(&mut picture).expect_err("there is no camera");
            assert_eq!(
                error.to_string(),
                "the camera could not be used: this device has no camera"
            );
        }

        let (panicking, news) = start(Which::Panicking);
        news.recv_timeout(PATIENCE).expect("the camera gave up");
        let error = panicking.take(&mut picture).expect_err("the camera panicked");
        assert_eq!(error.to_string(), "the camera could not be used: it stopped working");

        let (card, news) = start(Which::Card);
        news.recv_timeout(PATIENCE).expect("a picture came");
        assert_eq!(card.take(&mut picture).ok(), Some(true));
    }

    #[test]
    fn a_camera_let_go_of_before_its_turn_is_never_opened() {
        // The thread is busy with a card while a camera that would panic
        // if it were opened comes and goes.
        let (card, news) = start(Which::Card);
        news.recv_timeout(PATIENCE).expect("a picture came");
        let (panicking, gave_up) = start(Which::Panicking);
        drop(panicking);
        drop(card);
        assert!(gave_up.recv_timeout(Duration::from_millis(300)).is_err());
    }
}
