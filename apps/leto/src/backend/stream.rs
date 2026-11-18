//! Where a stream goes on the desktop: pictures to the window, sound to the
//! speakers.
//!
//! The engine hands over encoded pictures and sound on threads of its own.
//! They are decoded there, so the interface's thread only shows the result.

use std::{
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use leto_client::{
    AudioSetup, AudioSink, Decoded, VideoFrame, VideoSetup, VideoSink, service::sentence,
};
use leto_media::{Picture, SoundDecoder, Speaker, VideoDecoder};

use super::Pace;
use crate::stats::Counts;

/// How long a window that is only glanced at shows each picture.
const GLANCE: Duration = Duration::from_secs(1);

/// What a stream leaves for the interface to pick up.
#[derive(Default)]
pub struct Outlet {
    /// The latest picture nobody has shown yet.
    picture: Mutex<Option<egui::ColorImage>>,
    /// Why there is no sound, when there is none.
    sound_problem: Mutex<Option<String>>,
    /// What happened to the pictures so far.
    counts: Mutex<Counts>,
    /// How often the person sees the pictures.
    pace: Mutex<Pace>,
}

impl Outlet {
    /// The latest picture, once.
    pub fn take_picture(&self) -> Option<egui::ColorImage> {
        self.picture.lock().unwrap_or_else(PoisonError::into_inner).take()
    }

    /// What happened to the pictures of the stream so far.
    pub fn counts(&self) -> Counts {
        *self.counts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn count(&self, change: impl FnOnce(&mut Counts)) {
        change(&mut self.counts.lock().unwrap_or_else(PoisonError::into_inner));
    }

    /// Makes as many pictures into pixels as the person sees from now on.
    pub fn set_pace(&self, pace: Pace) {
        let mut was = self.pace.lock().unwrap_or_else(PoisonError::into_inner);
        if *was != pace {
            log::debug!("the stream is shown at {pace:?} pace");
            *was = pace;
        }
    }

    fn pace(&self) -> Pace {
        *self.pace.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn sound_problem(&self) -> Option<String> {
        self.sound_problem.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn set_sound_problem(&self, problem: String) {
        log::warn!("the stream plays without sound: {problem}");
        *self.sound_problem.lock().unwrap_or_else(PoisonError::into_inner) = Some(problem);
    }
}

/// Decodes the pictures of a stream and leaves the latest in the outlet.
///
/// Turning a picture into pixels, putting it on the graphics card and
/// drawing the window cost more than decoding it. While the window is only
/// glanced at, every picture is still decoded, since the next ones build on
/// it, but only one a second goes further. While the window is out of sight
/// nothing is decoded at all, and the PC is asked for a keyframe once it is
/// back. What the PC sends stays the same: nothing in the protocol slows a
/// PC down during a stream.
pub struct Screen {
    decoder: VideoDecoder,
    picture: Picture,
    outlet: Arc<Outlet>,
    ctx: egui::Context,
    /// The number of the last picture that arrived.
    last: Option<u32>,
    /// When the last picture was left in the outlet.
    left: Option<Instant>,
    /// Pictures went undecoded: the next ones are no use until a keyframe.
    stale: bool,
}

impl Screen {
    pub fn new(outlet: &Arc<Outlet>, ctx: &egui::Context) -> Result<Self, leto_media::Error> {
        Ok(Self {
            decoder: VideoDecoder::new()?,
            picture: Picture::default(),
            outlet: Arc::clone(outlet),
            ctx: ctx.clone(),
            last: None,
            left: None,
            stale: false,
        })
    }
}

impl VideoSink for Screen {
    fn setup(&mut self, setup: VideoSetup) {
        log::info!("the stream is {} by {} at {} fps", setup.width, setup.height, setup.fps);
        self.outlet.count(|counts| counts.sent = Some([setup.width, setup.height, setup.fps]));
    }

    fn frame(&mut self, frame: VideoFrame<'_>) -> Decoded {
        // Pictures are numbered as the PC made them.
        let gap = self.last.map_or(0, |last| frame.number.saturating_sub(last.saturating_add(1)));
        self.last = Some(frame.number);
        self.outlet.count(|counts| {
            counts.received(frame.data.len(), Some(u64::from(gap)), frame.host_latency);
        });
        let pace = self.outlet.pace();
        if pace == Pace::Hidden {
            self.stale = true;
            return Decoded::Ok;
        }
        if self.stale {
            if !frame.keyframe {
                // The engine asks the PC for one, and waits.
                return Decoded::NeedsKeyframe;
            }
            self.stale = false;
        }
        let seen = pace == Pace::Full || self.left.is_none_or(|left| left.elapsed() >= GLANCE);
        let start = Instant::now();
        let decoded = if seen {
            self.decoder.decode(frame.data, &mut self.picture)
        } else {
            self.decoder.decode_unseen(frame.data)
        };
        let took = start.elapsed();
        match decoded {
            Ok(true) if !seen => {
                self.outlet.count(|counts| counts.decoded(Some(took)));
                Decoded::Ok
            }
            Ok(true) => {
                self.outlet.count(|counts| counts.decoded(Some(took)));
                self.left = Some(Instant::now());
                let size = [self.picture.width, self.picture.height];
                // Pictures are opaque, so plain and premultiplied are the same.
                let image = egui::ColorImage::from_rgba_premultiplied(size, &self.picture.rgba);
                // A picture nobody got to show is replaced: the newest wins.
                *self.outlet.picture.lock().unwrap_or_else(PoisonError::into_inner) = Some(image);
                self.ctx.request_repaint();
                Decoded::Ok
            }
            Ok(false) => Decoded::Ok,
            Err(error) => {
                log::debug!("picture {} is lost: {error}", frame.number);
                Decoded::NeedsKeyframe
            }
        }
    }
}

/// Decodes the sound of a stream and plays it.
pub struct Sound {
    outlet: Arc<Outlet>,
    playing: Option<(SoundDecoder, Speaker)>,
}

impl Sound {
    pub fn new(outlet: &Arc<Outlet>) -> Self {
        Self { outlet: Arc::clone(outlet), playing: None }
    }
}

impl AudioSink for Sound {
    fn setup(&mut self, setup: AudioSetup) {
        if setup.channels != 2 {
            let problem = format!("Leto plays stereo only, and the PC sends {}", setup.channels);
            return self.outlet.set_sound_problem(format!("{problem} channels."));
        }
        let decoder = SoundDecoder::new(
            setup.sample_rate,
            setup.channels as usize,
            setup.streams as usize,
            setup.coupled_streams as usize,
            &setup.mapping,
        );
        // A stream without sound is still worth watching.
        match decoder.and_then(|decoder| Ok((decoder, Speaker::open(setup.sample_rate)?))) {
            Ok(playing) => self.playing = Some(playing),
            Err(error) => self.outlet.set_sound_problem(sentence(&error.to_string())),
        }
    }

    fn packet(&mut self, opus: &[u8]) {
        let Some((decoder, speaker)) = &mut self.playing else { return };
        match decoder.decode(opus) {
            Ok(samples) => speaker.play(samples),
            Err(error) => log::debug!("a packet of sound is lost: {error}"),
        }
    }

    fn stop(&mut self) {
        self.playing = None;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::{
        thread,
        time::{Duration, Instant},
    };

    use leto_client::{Client, Counter, StreamSettings};
    use leto_testhost::FakeHost;

    use super::*;

    /// The whole way of a picture: drawn and encoded by the stand-in PC,
    /// encrypted, sent with recovery data, put together again, decoded and
    /// converted. Colours that arrive right were right at every step.
    #[test]
    fn the_test_card_of_a_stand_in_pc_arrives_with_its_colours() {
        let folder = std::env::temp_dir().join(format!("leto-app-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let host = FakeHost::start();
        let client = Client::open(&folder).unwrap();
        let added = client.add(&host.address()).unwrap();
        let pin = client.new_pin().unwrap();
        host.type_pin(pin);
        let paired = client.pair(&added.id, pin).unwrap();

        let outlet = Arc::new(Outlet::default());
        let screen = Screen::new(&outlet, &egui::Context::default()).unwrap();
        // The sound is counted, not played: tests should be quiet.
        let sound = Counter::default();
        let settings = StreamSettings {
            width: 640,
            height: 360,
            fps: 30,
            bitrate_kbps: 4000,
            virtual_display: None,
        };
        let session = client.play(&paired.id, 1, settings, screen, sound.audio(), |_| {}).unwrap();

        let start = Instant::now();
        let picture = loop {
            if let Some(picture) = outlet.take_picture() {
                break picture;
            }
            assert!(start.elapsed() < Duration::from_secs(10), "no picture arrived");
            thread::sleep(Duration::from_millis(10));
        };
        session.stop();
        let _ = std::fs::remove_dir_all(&folder);

        assert_eq!(picture.size, [640, 360]);
        // The colour bars along the bottom: white, yellow, cyan, green,
        // magenta, red, blue and black, at three quarters of full brightness.
        let bars = [
            [191, 191, 191],
            [191, 191, 0],
            [0, 191, 191],
            [0, 191, 0],
            [191, 0, 191],
            [191, 0, 0],
            [0, 0, 191],
            [0, 0, 0],
        ];
        for (index, wanted) in bars.into_iter().enumerate() {
            let found = picture[(index * 80 + 40, 342)].to_array();
            let near = found.iter().zip(wanted).all(|(found, wanted)| found.abs_diff(wanted) <= 6);
            assert!(near && found[3] == 255, "bar {index} is {found:?}, not {wanted:?}");
        }
        // The ground between the discs and the bars.
        let ground = picture[(60, 290)].to_array();
        assert!(ground[..3].iter().zip([0x15, 0x13, 0x12]).all(|(f, w)| f.abs_diff(w) <= 6));
    }

    /// Waits until `outlet` has a picture, and says how long that took.
    fn next_picture(outlet: &Outlet, patience: Duration) -> Option<Duration> {
        let start = Instant::now();
        while start.elapsed() < patience {
            if outlet.take_picture().is_some() {
                return Some(start.elapsed());
            }
            thread::sleep(Duration::from_millis(5));
        }
        None
    }

    #[test]
    fn a_window_glanced_at_gets_a_picture_a_second_and_one_out_of_sight_none() {
        let folder = std::env::temp_dir().join(format!("leto-app-pace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let host = FakeHost::start();
        let client = Client::open(&folder).unwrap();
        let added = client.add(&host.address()).unwrap();
        let pin = client.new_pin().unwrap();
        host.type_pin(pin);
        let paired = client.pair(&added.id, pin).unwrap();

        let outlet = Arc::new(Outlet::default());
        let screen = Screen::new(&outlet, &egui::Context::default()).unwrap();
        let sound = Counter::default();
        let settings = StreamSettings {
            width: 320,
            height: 180,
            fps: 30,
            bitrate_kbps: 2000,
            virtual_display: None,
        };
        let session = client.play(&paired.id, 1, settings, screen, sound.audio(), |_| {}).unwrap();
        assert!(next_picture(&outlet, Duration::from_secs(10)).is_some(), "no picture arrived");

        // Glanced at: every picture is decoded, one a second is shown.
        outlet.set_pace(Pace::Glance);
        let _ = outlet.take_picture();
        next_picture(&outlet, Duration::from_secs(3)).expect("a glanced-at window has pictures");
        let decoded = outlet.counts().decoded;
        let waited = next_picture(&outlet, Duration::from_secs(3)).expect("one a second");
        assert!(waited > Duration::from_millis(800), "a picture came after {waited:?}");
        assert!(outlet.counts().decoded > decoded + 5, "pictures were not decoded meanwhile");

        // Out of sight: nothing is decoded at all.
        outlet.set_pace(Pace::Hidden);
        thread::sleep(Duration::from_millis(100));
        let decoded = outlet.counts().decoded;
        let _ = outlet.take_picture();
        assert_eq!(next_picture(&outlet, Duration::from_millis(1500)), None);
        assert_eq!(outlet.counts().decoded, decoded);

        // Back in sight: the PC is asked for a keyframe, and pictures flow.
        let asked = host.stream_report().unwrap().keyframes_requested;
        outlet.set_pace(Pace::Full);
        next_picture(&outlet, Duration::from_secs(5)).expect("pictures again");
        assert!(host.stream_report().unwrap().keyframes_requested > asked);
        let waited = next_picture(&outlet, Duration::from_secs(3)).expect("every picture");
        assert!(waited < Duration::from_millis(700), "a picture came after {waited:?}");

        session.stop();
        let _ = std::fs::remove_dir_all(&folder);
    }
}
