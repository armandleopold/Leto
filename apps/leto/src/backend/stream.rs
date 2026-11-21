//! Where a stream goes on the desktop: pictures to the window, sound to the
//! speakers.
//!
//! The engine hands over encoded pictures and sound on threads of its own.
//! They are decoded there, so the interface's thread only shows the result.

use std::{
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use leto_client::{
    AudioSetup, AudioSink, Decoded, VideoFrame, VideoSetup, VideoSink, service::sentence,
};
use leto_media::{Planes, SoundDecoder, Speaker, VideoDecoder};

use super::Pace;
use crate::stats::Counts;

/// How long a window that is only glanced at shows each picture.
const GLANCE: Duration = Duration::from_secs(1);

/// How long a stream may go without any sound before Leto says none
/// arrives. A PC sends sound all the time, silence included, from the first
/// moments of a stream.
const SILENCE: Duration = Duration::from_secs(5);

/// How many packets of sound that all fail to decode mean that none will:
/// about a second of sound.
const UNPLAYABLE: u64 = 200;

/// How long pictures may go unshown while newer ones keep waiting behind
/// them, should the decoder never catch up.
const CATCH_UP: Duration = Duration::from_millis(100);

/// What a stream leaves for the interface to pick up.
#[derive(Default)]
pub struct Outlet {
    /// The latest picture nobody has shown yet.
    picture: Mutex<Option<Planes>>,
    /// Why there is no sound, when there is none.
    sound_problem: Mutex<Option<String>>,
    /// What happened to the pictures so far.
    counts: Mutex<Counts>,
    /// How often the person sees the pictures.
    pace: Mutex<Pace>,
    /// The sound is decoded and not played.
    muted: AtomicBool,
    /// Why the stream has no pictures, when it has none.
    picture_problem: Mutex<Option<String>>,
    /// What happened to the sound so far.
    heard: Mutex<Heard>,
}

/// What happened to the sound of a stream, to tell when it never plays.
#[derive(Debug, Default)]
struct Heard {
    /// When the stream said what sound it sends.
    since: Option<Instant>,
    /// Packets that arrived.
    arrived: u64,
    /// Packets that did not decode.
    lost: u64,
    /// The problem last written in the log.
    told: Option<&'static str>,
}

impl Heard {
    /// Why nothing is heard, judging by what arrived by `now`.
    fn problem(&self, now: Instant) -> Option<&'static str> {
        let since = self.since?;
        if self.arrived == 0 && now.saturating_duration_since(since) >= SILENCE {
            Some("No sound arrives from the PC.")
        } else if self.arrived >= UNPLAYABLE && self.lost == self.arrived {
            Some("The sound from the PC does not decode.")
        } else {
            None
        }
    }
}

impl Outlet {
    /// The latest picture, once.
    pub fn take_picture(&self) -> Option<Planes> {
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

    /// Silences the sound from now on, or lets it be heard again.
    pub fn set_muted(&self, muted: bool) {
        if self.muted.swap(muted, Ordering::Relaxed) != muted {
            log::debug!("the stream is {}", if muted { "muted" } else { "heard" });
        }
    }

    fn pace(&self) -> Pace {
        *self.pace.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Why the stream has no sound, when it has none: the speakers did not
    /// open, or nothing arrives that can be played.
    pub fn sound_problem(&self) -> Option<String> {
        self.sound_problem_at(Instant::now())
    }

    fn sound_problem_at(&self, now: Instant) -> Option<String> {
        let set = self.sound_problem.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if set.is_some() {
            return set;
        }
        let mut heard = self.heard();
        let problem = heard.problem(now);
        if problem != heard.told {
            match problem {
                Some(problem) => log::warn!(
                    "the stream plays without sound: {problem} {} packets of sound arrived, {} \
                     did not decode",
                    heard.arrived,
                    heard.lost,
                ),
                None => log::info!("the stream has sound after all"),
            }
            heard.told = problem;
        }
        problem.map(str::to_owned)
    }

    fn heard(&self) -> MutexGuard<'_, Heard> {
        self.heard.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The stream said what sound it sends at `now`: it has had none yet.
    fn sound_begins(&self, now: Instant) {
        *self.heard() = Heard { since: Some(now), ..Heard::default() };
    }

    /// A packet of sound arrived, and decoded or not.
    fn sound_arrived(&self, decoded: bool) {
        let mut heard = self.heard();
        heard.arrived += 1;
        heard.lost += u64::from(!decoded);
        if heard.arrived == 1 {
            log::info!("the first sound arrived from the PC");
        }
    }

    fn set_sound_problem(&self, problem: String) {
        log::warn!("the stream plays without sound: {problem}");
        *self.sound_problem.lock().unwrap_or_else(PoisonError::into_inner) = Some(problem);
    }

    pub fn picture_problem(&self) -> Option<String> {
        self.picture_problem.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn set_picture_problem(&self, problem: String) {
        log::warn!("the stream has no pictures: {problem}");
        *self.picture_problem.lock().unwrap_or_else(PoisonError::into_inner) = Some(problem);
    }
}

/// Decodes the pictures of a stream and leaves the latest in the outlet.
///
/// Copying a picture out of the decoder, putting it on the graphics card
/// and drawing the window cost about as much as decoding it, so pictures
/// stay as the decoder gave them, in planes, and the card makes them into
/// pixels as the window draws them. A picture that newer ones already wait
/// behind would be replaced before anyone saw it: it is decoded, since the
/// next ones build on it, and goes no further, so a decoder slower than the
/// stream skips pictures instead of falling further and further behind.
/// While the window is only glanced at, every picture is still decoded, but
/// only one a second goes further. While the window is out of sight
/// nothing is decoded at all, and the PC is asked for a keyframe once it is
/// back. What the PC sends stays the same: nothing in the protocol slows a
/// PC down during a stream.
///
/// The decoder is made once the PC said what it sends, in which codec: on
/// the graphics card when the person lets it and the card can.
pub struct Screen {
    decoder: Option<VideoDecoder>,
    /// The pictures may be decoded on the graphics card.
    hardware: bool,
    /// The picture being decoded, in memory a picture nobody showed left.
    picture: Planes,
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
    pub fn new(outlet: &Arc<Outlet>, ctx: &egui::Context, hardware: bool) -> Self {
        Self {
            decoder: None,
            hardware,
            picture: Planes::default(),
            outlet: Arc::clone(outlet),
            ctx: ctx.clone(),
            last: None,
            left: None,
            stale: false,
        }
    }

    /// Says in the counts who decodes the pictures, which can change during
    /// a stream: a graphics card that fails H.264 hands it to the processor.
    fn count_decoder(&self) {
        let decoder = self.decoder.as_ref();
        let (codec, card) =
            (decoder.map(VideoDecoder::codec), decoder.map(VideoDecoder::on_graphics_card));
        self.outlet.count(|counts| {
            counts.codec = codec.or(counts.codec);
            counts.on_graphics_card = card;
        });
    }
}

impl VideoSink for Screen {
    fn setup(&mut self, setup: VideoSetup) {
        let VideoSetup { codec, width, height, fps } = setup;
        log::info!("the stream is {width} by {height} at {fps} fps, in {}", codec.name());
        self.outlet.count(|counts| {
            counts.sent = Some([width, height, fps]);
            counts.codec = Some(codec);
        });
        self.decoder = match VideoDecoder::for_stream(codec, width, height, fps, self.hardware) {
            Ok(decoder) => Some(decoder),
            Err(error) => {
                let problem = format!(
                    "{} H.264 can be chosen in the settings, under Advanced.",
                    sentence(&error.to_string())
                );
                self.outlet.set_picture_problem(problem);
                None
            }
        };
        self.count_decoder();
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
        // Without a decoder there is nothing to do, and nothing to ask for.
        let Some(decoder) = &mut self.decoder else { return Decoded::Ok };
        let since_shown = self.left.map(|left| left.elapsed());
        let behind = frame.waiting > 0 && since_shown.is_some_and(|since| since < CATCH_UP);
        let seen =
            !behind && (pace == Pace::Full || since_shown.is_none_or(|since| since >= GLANCE));
        let start = Instant::now();
        let decoded = if seen {
            decoder.decode_planes(frame.data, &mut self.picture)
        } else {
            decoder.decode_unseen(frame.data)
        };
        let took = start.elapsed();
        match decoded {
            Ok(true) if !seen => {
                self.outlet.count(|counts| {
                    counts.decoded(Some(took));
                    counts.skipped += u64::from(behind);
                });
                Decoded::Ok
            }
            Ok(true) => {
                let copying = decoder.copying();
                self.outlet.count(|counts| {
                    counts.decoded(Some(took));
                    counts.copied(copying);
                });
                self.left = Some(Instant::now());
                let planes = std::mem::take(&mut self.picture);
                // A picture nobody got to show is replaced: the newest wins,
                // and the next one is decoded into its memory.
                let unseen = self
                    .outlet
                    .picture
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .replace(planes);
                self.picture = unseen.unwrap_or_default();
                self.ctx.request_repaint();
                Decoded::Ok
            }
            Ok(false) => Decoded::Ok,
            Err(error) => {
                log::debug!("picture {} is lost: {error}", frame.number);
                self.count_decoder();
                Decoded::NeedsKeyframe
            }
        }
    }
}

/// Decodes the sound of a stream and plays it.
pub struct Sound {
    outlet: Arc<Outlet>,
    /// The sound output the person chose, by the name the system gives it;
    /// `None` plays on the system's default.
    speakers: Option<String>,
    playing: Option<(SoundDecoder, Speaker)>,
    /// Packets that did not decode.
    lost: u64,
}

impl Sound {
    pub fn new(outlet: &Arc<Outlet>, speakers: Option<String>) -> Self {
        Self { outlet: Arc::clone(outlet), speakers, playing: None, lost: 0 }
    }
}

impl AudioSink for Sound {
    fn setup(&mut self, setup: AudioSetup) {
        self.outlet.sound_begins(Instant::now());
        self.lost = 0;
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
        match decoder.and_then(|decoder| {
            Ok((decoder, Speaker::open(setup.sample_rate, self.speakers.as_deref())?))
        }) {
            Ok(playing) => self.playing = Some(playing),
            Err(error) => self.outlet.set_sound_problem(sentence(&error.to_string())),
        }
    }

    fn packet(&mut self, opus: &[u8]) {
        let Some((decoder, speaker)) = &mut self.playing else {
            return self.outlet.sound_arrived(false);
        };
        // Decoded even when muted: the next packets build on this one.
        let decoded = decoder.decode(opus);
        self.outlet.sound_arrived(decoded.is_ok());
        match decoded {
            Ok(_) if self.outlet.muted.load(Ordering::Relaxed) => {}
            Ok(samples) => speaker.play(samples),
            // The first says why, should none ever decode.
            Err(error) if self.lost == 0 => {
                self.lost += 1;
                log::warn!("a packet of sound did not decode: {error}");
            }
            Err(error) => {
                self.lost += 1;
                log::debug!("a packet of sound did not decode: {error}");
            }
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

    use leto_client::{
        Client, Counter, StreamSettings,
        engine::{Codec, Codecs},
    };
    use leto_testhost::FakeHost;

    use super::*;

    #[test]
    fn a_stream_without_sound_says_so_after_a_while() {
        let outlet = Outlet::default();
        let start = Instant::now();
        assert_eq!(outlet.sound_problem_at(start + SILENCE), None, "no sound was promised yet");

        outlet.sound_begins(start);
        assert_eq!(outlet.sound_problem_at(start + Duration::from_secs(1)), None, "it may come");
        let silent = outlet.sound_problem_at(start + SILENCE).expect("too long without sound");
        assert!(silent.contains("No sound arrives"), "{silent}");

        // Sound that arrives late is sound all the same.
        outlet.sound_arrived(true);
        assert_eq!(outlet.sound_problem_at(start + SILENCE * 2), None);

        // A new stream starts with none.
        outlet.sound_begins(start + SILENCE * 2);
        assert!(outlet.sound_problem_at(start + SILENCE * 3).is_some());
    }

    #[test]
    fn sound_that_never_decodes_is_said_and_some_losses_are_not() {
        let outlet = Outlet::default();
        let start = Instant::now();
        outlet.sound_begins(start);
        for _ in 1..UNPLAYABLE {
            outlet.sound_arrived(false);
        }
        assert_eq!(outlet.sound_problem_at(start), None, "not yet a second of it");
        outlet.sound_arrived(false);
        let problem = outlet.sound_problem_at(start).expect("none decoded");
        assert!(problem.contains("does not decode"), "{problem}");

        outlet.sound_arrived(true);
        assert_eq!(outlet.sound_problem_at(start), None, "one decoded");
    }

    #[test]
    fn speakers_that_did_not_open_are_the_problem_whatever_arrives() {
        let outlet = Outlet::default();
        let start = Instant::now();
        outlet.sound_begins(start);
        outlet.set_sound_problem("This device has no sound output.".to_owned());
        outlet.sound_arrived(false);
        assert_eq!(
            outlet.sound_problem_at(start + SILENCE).as_deref(),
            Some("This device has no sound output.")
        );
    }

    /// The whole way of a picture: drawn and encoded by the stand-in PC,
    /// encrypted, sent with recovery data, put together again, decoded and
    /// converted as the graphics card does. Colours that arrive right were
    /// right at every step.
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
        let screen = Screen::new(&outlet, &egui::Context::default(), true);
        // The sound is counted, not played: tests should be quiet.
        let sound = Counter::default();
        let settings = StreamSettings {
            width: 640,
            height: 360,
            fps: 30,
            bitrate_kbps: 4000,
            virtual_display: false,
            optimize_games: true,
            sound_on_pc: false,
            codecs: Codecs::of(Codec::H264),
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

        assert_eq!((picture.width, picture.height), (640, 360));
        let mut pixels = leto_media::Picture::default();
        assert!(picture.to_rgba(&mut pixels));
        let pixel = |column: usize, row: usize| {
            let at = (row * pixels.width + column) * 4;
            [pixels.rgba[at], pixels.rgba[at + 1], pixels.rgba[at + 2], pixels.rgba[at + 3]]
        };
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
            let found = pixel(index * 80 + 40, 342);
            let near = found.iter().zip(wanted).all(|(found, wanted)| found.abs_diff(wanted) <= 6);
            assert!(near && found[3] == 255, "bar {index} is {found:?}, not {wanted:?}");
        }
        // The ground between the discs and the bars.
        let ground = pixel(60, 290);
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
        let screen = Screen::new(&outlet, &egui::Context::default(), true);
        let sound = Counter::default();
        let settings = StreamSettings {
            width: 320,
            height: 180,
            fps: 30,
            bitrate_kbps: 2000,
            virtual_display: false,
            optimize_games: true,
            sound_on_pc: false,
            codecs: Codecs::of(Codec::H264),
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

    /// A window too slow for its stream: each picture reaches the screen
    /// later than the PC makes the next.
    struct Slow(Screen);

    impl VideoSink for Slow {
        fn setup(&mut self, setup: VideoSetup) {
            self.0.setup(setup);
        }

        fn frame(&mut self, frame: VideoFrame<'_>) -> Decoded {
            thread::sleep(Duration::from_millis(60));
            self.0.frame(frame)
        }
    }

    #[test]
    fn a_screen_that_falls_behind_skips_pictures_and_still_shows_some() {
        let folder = std::env::temp_dir().join(format!("leto-app-behind-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let host = FakeHost::start();
        let client = Client::open(&folder).unwrap();
        let added = client.add(&host.address()).unwrap();
        let pin = client.new_pin().unwrap();
        host.type_pin(pin);
        let paired = client.pair(&added.id, pin).unwrap();

        let outlet = Arc::new(Outlet::default());
        let screen = Slow(Screen::new(&outlet, &egui::Context::default(), true));
        let sound = Counter::default();
        let settings = StreamSettings {
            width: 320,
            height: 180,
            fps: 30,
            bitrate_kbps: 2000,
            virtual_display: false,
            optimize_games: true,
            sound_on_pc: false,
            codecs: Codecs::of(Codec::H264),
        };
        let session = client.play(&paired.id, 1, settings, screen, sound.audio(), |_| {}).unwrap();
        assert!(next_picture(&outlet, Duration::from_secs(10)).is_some(), "no picture arrived");
        thread::sleep(Duration::from_secs(2));
        let counts = outlet.counts();
        // Still shown now and then, while the pictures pile up.
        let waited = next_picture(&outlet, Duration::from_secs(2)).expect("pictures still come");
        session.stop();
        let _ = std::fs::remove_dir_all(&folder);

        assert!(counts.skipped > 5, "pictures newer ones waited behind were shown: {counts:?}");
        assert!(counts.copied > 5, "too few pictures were shown: {counts:?}");
        assert_eq!(counts.decoded, counts.copied + counts.skipped, "all decoded: {counts:?}");
        assert!(waited < Duration::from_millis(500), "a picture came after {waited:?}");
    }
}
