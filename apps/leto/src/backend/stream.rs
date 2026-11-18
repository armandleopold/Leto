//! Where a stream goes on the desktop: pictures to the window, sound to the
//! speakers.
//!
//! The engine hands over encoded pictures and sound on threads of its own.
//! They are decoded there, so the interface's thread only shows the result.

use std::sync::{Arc, Mutex, PoisonError};

use leto_client::{AudioSetup, AudioSink, Decoded, VideoFrame, VideoSetup, VideoSink};
use leto_media::{Picture, SoundDecoder, Speaker, VideoDecoder};

/// What a stream leaves for the interface to pick up.
#[derive(Default)]
pub struct Outlet {
    /// The latest picture nobody has shown yet.
    picture: Mutex<Option<egui::ColorImage>>,
    /// Why there is no sound, when there is none.
    sound_problem: Mutex<Option<String>>,
}

impl Outlet {
    /// The latest picture, once.
    pub fn take_picture(&self) -> Option<egui::ColorImage> {
        self.picture.lock().unwrap_or_else(PoisonError::into_inner).take()
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
pub struct Screen {
    decoder: VideoDecoder,
    picture: Picture,
    outlet: Arc<Outlet>,
    ctx: egui::Context,
}

impl Screen {
    pub fn new(outlet: &Arc<Outlet>, ctx: &egui::Context) -> Result<Self, leto_media::Error> {
        Ok(Self {
            decoder: VideoDecoder::new()?,
            picture: Picture::default(),
            outlet: Arc::clone(outlet),
            ctx: ctx.clone(),
        })
    }
}

impl VideoSink for Screen {
    fn setup(&mut self, setup: VideoSetup) {
        log::info!("the stream is {} by {} at {} fps", setup.width, setup.height, setup.fps);
    }

    fn frame(&mut self, frame: VideoFrame<'_>) -> Decoded {
        match self.decoder.decode(frame.data, &mut self.picture) {
            Ok(true) => {
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

/// Makes a sentence of an error message: a capital first, a full stop last.
pub fn sentence(text: &str) -> String {
    let mut letters = text.chars();
    let first = letters.next().map(|letter| letter.to_uppercase().to_string());
    format!("{}{}.", first.unwrap_or_default(), letters.as_str().trim_end_matches('.'))
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

    #[test]
    fn sentences_get_a_capital_and_one_full_stop() {
        assert_eq!(sentence("the PC did not answer"), "The PC did not answer.");
        assert_eq!(sentence("It may have been disconnected."), "It may have been disconnected.");
    }

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
        let settings = StreamSettings { width: 640, height: 360, fps: 30, bitrate_kbps: 4000 };
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
}
