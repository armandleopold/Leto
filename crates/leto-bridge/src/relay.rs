//! What is on its way to one browser, and the stream's part of it.
//!
//! The engine hands pictures and sound over still encoded, and that is how
//! they travel on: the browser decodes them. The bridge only has to notice
//! when a browser cannot keep up, so that pictures do not pile up and reach
//! it late.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use leto_client::{AudioSetup, AudioSink, Decoded, VideoFrame, VideoSetup, VideoSink};
use leto_core::bridge::{AudioFormat, Down, Frame, VideoFormat};
use tokio::sync::mpsc;

/// Pictures that may wait for the browser to take them. With more than
/// that, the browser is behind by a good part of a second: better to skip
/// to a fresh picture than to show the past.
const PICTURES_WAITING: usize = 30;

/// Sound packets that may wait: a quarter of a second or so.
const SOUND_WAITING: usize = 60;

/// One message on its way to the browser.
pub enum Parcel {
    Text(String),
    Picture(Vec<u8>),
    Sound(Vec<u8>),
}

/// Where the engine's threads leave what the browser should get.
pub struct Outbox {
    parcels: mpsc::UnboundedSender<Parcel>,
    pictures_waiting: AtomicUsize,
    sound_waiting: AtomicUsize,
    /// The browser could not decode a picture and waits for a keyframe.
    keyframe_wanted: AtomicBool,
}

impl Outbox {
    /// An outbox, and the end its parcels come out of.
    pub fn new() -> (Arc<Self>, mpsc::UnboundedReceiver<Parcel>) {
        let (parcels, queue) = mpsc::unbounded_channel();
        let outbox = Self {
            parcels,
            pictures_waiting: AtomicUsize::new(0),
            sound_waiting: AtomicUsize::new(0),
            keyframe_wanted: AtomicBool::new(false),
        };
        (Arc::new(outbox), queue)
    }

    pub fn say(&self, message: &Down) {
        match serde_json::to_string(message) {
            // The browser is gone when nobody takes parcels any more.
            Ok(text) => drop(self.parcels.send(Parcel::Text(text))),
            Err(error) => log::error!("a message could not be written down: {error}"),
        }
    }

    /// The browser asks for a picture that decodes on its own.
    pub fn want_keyframe(&self) {
        self.keyframe_wanted.store(true, Ordering::SeqCst);
    }

    /// Whoever sends the parcels on says so, one by one.
    pub fn sent(&self, parcel: &Parcel) {
        match parcel {
            Parcel::Picture(_) => {
                self.pictures_waiting.fetch_sub(1, Ordering::SeqCst);
            }
            Parcel::Sound(_) => {
                self.sound_waiting.fetch_sub(1, Ordering::SeqCst);
            }
            Parcel::Text(_) => {}
        }
    }

    /// The sinks of a stream that goes to this browser.
    pub fn sinks(self: &Arc<Self>) -> (Pictures, Sound) {
        // What an earlier stream left unanswered is not about this one.
        self.keyframe_wanted.store(false, Ordering::SeqCst);
        let pictures = Pictures { outbox: Arc::clone(self), setup: None, codec: None };
        (pictures, Sound { outbox: Arc::clone(self), muted: false })
    }
}

/// Sends the pictures of a stream on to the browser.
pub struct Pictures {
    outbox: Arc<Outbox>,
    setup: Option<VideoSetup>,
    /// The codec the browser was told to expect.
    codec: Option<String>,
}

impl VideoSink for Pictures {
    fn setup(&mut self, setup: VideoSetup) {
        self.setup = Some(setup);
    }

    fn frame(&mut self, frame: VideoFrame<'_>) -> Decoded {
        let outbox = &self.outbox;
        if frame.keyframe {
            outbox.keyframe_wanted.store(false, Ordering::SeqCst);
            // A keyframe says what the pictures are made of. The browser
            // has to hear it before the first one, and again if it changes.
            let Some(codec) = avc_codec(frame.data) else {
                log::debug!("picture {} has no parameters to read", frame.number);
                return Decoded::NeedsKeyframe;
            };
            if self.codec.as_deref() != Some(&codec) {
                let setup = self.setup.unwrap_or(VideoSetup { width: 0, height: 0, fps: 0 });
                outbox.say(&Down::Pictures(VideoFormat {
                    codec: codec.clone(),
                    width: setup.width,
                    height: setup.height,
                    fps: setup.fps,
                }));
                self.codec = Some(codec);
            }
        } else if self.codec.is_none() || outbox.keyframe_wanted.load(Ordering::SeqCst) {
            // The browser can do nothing with this one.
            return Decoded::NeedsKeyframe;
        }

        if outbox.pictures_waiting.load(Ordering::SeqCst) >= PICTURES_WAITING {
            // The browser is behind. This picture is dropped, and so are the
            // next ones until a keyframe finds room.
            return Decoded::NeedsKeyframe;
        }
        let message =
            Frame::Picture { number: frame.number, keyframe: frame.keyframe, data: frame.data }
                .encode();
        outbox.pictures_waiting.fetch_add(1, Ordering::SeqCst);
        if outbox.parcels.send(Parcel::Picture(message)).is_err() {
            outbox.pictures_waiting.fetch_sub(1, Ordering::SeqCst);
        }
        Decoded::Ok
    }
}

/// Sends the sound of a stream on to the browser.
pub struct Sound {
    outbox: Arc<Outbox>,
    /// The browser was told the stream has no sound.
    muted: bool,
}

impl AudioSink for Sound {
    fn setup(&mut self, setup: AudioSetup) {
        let format = if setup.channels == 2 {
            Ok(AudioFormat {
                sample_rate: setup.sample_rate,
                channels: setup.channels,
                samples_per_packet: setup.samples_per_packet,
            })
        } else {
            // What the desktop app says, for the same reason.
            let channels = setup.channels;
            Err(format!("Leto plays stereo only, and the PC sends {channels} channels."))
        };
        self.muted = format.is_err();
        self.outbox.say(&Down::Sound(format));
    }

    fn packet(&mut self, opus: &[u8]) {
        let outbox = &self.outbox;
        if self.muted || outbox.sound_waiting.load(Ordering::SeqCst) >= SOUND_WAITING {
            // Late sound is worse than a gap.
            return;
        }
        outbox.sound_waiting.fetch_add(1, Ordering::SeqCst);
        if outbox.parcels.send(Parcel::Sound(Frame::Sound(opus).encode())).is_err() {
            outbox.sound_waiting.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// The name browsers give the H.264 of a keyframe, such as `avc1.640028`:
/// the profile, what it leaves out and the level, as its sequence
/// parameters state them.
pub fn avc_codec(access_unit: &[u8]) -> Option<String> {
    /// The kind of unit that carries the sequence parameters.
    const SEQUENCE_PARAMETERS: u8 = 7;

    let mut rest = access_unit;
    loop {
        // Units start after 00 00 01, however many zeros come before.
        let start = rest.windows(3).position(|window| window == [0, 0, 1])?;
        rest = &rest[start + 3..];
        let (kind, parameters) = rest.split_first()?;
        if kind & 0x1F == SEQUENCE_PARAMETERS {
            let [profile, constraints, level] = *parameters.first_chunk::<3>()?;
            return Some(format!("avc1.{profile:02x}{constraints:02x}{level:02x}"));
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A keyframe as a host sends it: parameters, then the picture.
    const KEYFRAME: &[u8] =
        &[0, 0, 0, 1, 0x67, 0x64, 0x00, 0x28, 0xAC, 0, 0, 0, 1, 0x68, 0xEE, 0, 0, 1, 0x65, 0x88];
    /// A picture that builds on the ones before.
    const DELTA: &[u8] = &[0, 0, 0, 1, 0x41, 0x9A];

    const SETUP: VideoSetup = VideoSetup { width: 1920, height: 1080, fps: 60 };

    fn frame(number: u32, data: &[u8]) -> VideoFrame<'_> {
        VideoFrame { number, keyframe: avc_codec(data).is_some(), data }
    }

    fn texts(queue: &mut mpsc::UnboundedReceiver<Parcel>) -> Vec<Down> {
        let mut texts = Vec::new();
        while let Ok(parcel) = queue.try_recv() {
            if let Parcel::Text(text) = parcel {
                texts.push(serde_json::from_str(&text).unwrap());
            }
        }
        texts
    }

    #[test]
    fn the_codec_is_read_from_the_sequence_parameters() {
        assert_eq!(avc_codec(KEYFRAME).as_deref(), Some("avc1.640028"));
        // Baseline at level 3.1, with short start codes.
        assert_eq!(
            avc_codec(&[0, 0, 1, 0x27, 0x42, 0xE0, 0x1F, 0x95]).as_deref(),
            Some("avc1.42e01f")
        );
        // Another unit first.
        assert_eq!(
            avc_codec(&[0, 0, 1, 0x09, 0x10, 0, 0, 1, 0x67, 0x4D, 0x40, 0x29]).as_deref(),
            Some("avc1.4d4029")
        );
        assert_eq!(avc_codec(DELTA), None);
        assert_eq!(avc_codec(&[0, 0, 1, 0x67, 0x64]), None);
        assert_eq!(avc_codec(&[]), None);
    }

    #[test]
    fn the_browser_hears_what_the_pictures_are_before_the_first_one() {
        let (outbox, mut queue) = Outbox::new();
        let (mut pictures, _) = outbox.sinks();
        pictures.setup(SETUP);

        assert_eq!(pictures.frame(frame(1, KEYFRAME)), Decoded::Ok);
        assert_eq!(pictures.frame(frame(2, DELTA)), Decoded::Ok);

        let Some(Parcel::Text(text)) = queue.try_recv().ok() else { panic!("no format first") };
        assert_eq!(
            serde_json::from_str::<Down>(&text).unwrap(),
            Down::Pictures(VideoFormat {
                codec: "avc1.640028".into(),
                width: 1920,
                height: 1080,
                fps: 60
            })
        );
        let Some(Parcel::Picture(first)) = queue.try_recv().ok() else { panic!("no picture") };
        assert_eq!(
            Frame::parse(&first),
            Some(Frame::Picture { number: 1, keyframe: true, data: KEYFRAME })
        );
        let Some(Parcel::Picture(second)) = queue.try_recv().ok() else { panic!("no picture") };
        assert_eq!(
            Frame::parse(&second),
            Some(Frame::Picture { number: 2, keyframe: false, data: DELTA })
        );
        // The same format is not said twice.
        assert_eq!(pictures.frame(frame(3, KEYFRAME)), Decoded::Ok);
        assert!(texts(&mut queue).is_empty());
    }

    #[test]
    fn a_stream_that_starts_in_the_middle_waits_for_a_keyframe() {
        let (outbox, mut queue) = Outbox::new();
        let (mut pictures, _) = outbox.sinks();
        pictures.setup(SETUP);
        assert_eq!(pictures.frame(frame(1, DELTA)), Decoded::NeedsKeyframe);
        assert!(queue.try_recv().is_err());
    }

    #[test]
    fn a_browser_that_falls_behind_skips_to_a_fresh_keyframe() {
        let (outbox, mut queue) = Outbox::new();
        let (mut pictures, _) = outbox.sinks();
        pictures.setup(SETUP);
        assert_eq!(pictures.frame(frame(0, KEYFRAME)), Decoded::Ok);
        for number in 1..PICTURES_WAITING as u32 {
            assert_eq!(pictures.frame(frame(number, DELTA)), Decoded::Ok);
        }
        // Nobody took anything: the next ones are dropped, keyframes too.
        assert_eq!(pictures.frame(frame(100, DELTA)), Decoded::NeedsKeyframe);
        assert_eq!(pictures.frame(frame(101, KEYFRAME)), Decoded::NeedsKeyframe);

        // The browser catches up.
        while let Ok(parcel) = queue.try_recv() {
            outbox.sent(&parcel);
        }
        assert_eq!(pictures.frame(frame(102, KEYFRAME)), Decoded::Ok);
        assert_eq!(pictures.frame(frame(103, DELTA)), Decoded::Ok);
    }

    #[test]
    fn a_browser_that_lost_a_picture_gets_nothing_until_a_keyframe() {
        let (outbox, mut queue) = Outbox::new();
        let (mut pictures, _) = outbox.sinks();
        pictures.setup(SETUP);
        assert_eq!(pictures.frame(frame(1, KEYFRAME)), Decoded::Ok);

        outbox.want_keyframe();
        assert_eq!(pictures.frame(frame(2, DELTA)), Decoded::NeedsKeyframe);
        assert_eq!(pictures.frame(frame(3, DELTA)), Decoded::NeedsKeyframe);
        assert_eq!(pictures.frame(frame(4, KEYFRAME)), Decoded::Ok);
        assert_eq!(pictures.frame(frame(5, DELTA)), Decoded::Ok);

        let mut numbers = Vec::new();
        while let Ok(parcel) = queue.try_recv() {
            if let Parcel::Picture(message) = parcel
                && let Some(Frame::Picture { number, .. }) = Frame::parse(&message)
            {
                numbers.push(number);
            }
        }
        assert_eq!(numbers, [1, 4, 5]);
    }

    #[test]
    fn stereo_sound_goes_through_and_anything_else_is_explained() {
        let stereo = AudioSetup {
            sample_rate: 48_000,
            channels: 2,
            samples_per_packet: 240,
            streams: 1,
            coupled_streams: 1,
            mapping: [0, 1, 0, 0, 0, 0, 0, 0],
        };
        let (outbox, mut queue) = Outbox::new();
        let (_, mut sound) = outbox.sinks();
        sound.setup(stereo);
        sound.packet(&[0xFC, 1, 2]);
        let Some(Parcel::Text(text)) = queue.try_recv().ok() else { panic!("no format first") };
        assert_eq!(
            serde_json::from_str::<Down>(&text).unwrap(),
            Down::Sound(Ok(AudioFormat {
                sample_rate: 48_000,
                channels: 2,
                samples_per_packet: 240
            }))
        );
        let Some(Parcel::Sound(packet)) = queue.try_recv().ok() else { panic!("no sound") };
        assert_eq!(Frame::parse(&packet), Some(Frame::Sound(&[0xFC, 1, 2])));

        let (outbox, mut queue) = Outbox::new();
        let (_, mut sound) = outbox.sinks();
        sound.setup(AudioSetup { channels: 6, streams: 4, coupled_streams: 2, ..stereo });
        sound.packet(&[0xFC, 1, 2]);
        let said = texts(&mut queue);
        let [Down::Sound(Err(why))] = said.as_slice() else { panic!("{said:?}") };
        assert_eq!(why, "Leto plays stereo only, and the PC sends 6 channels.");
    }

    #[test]
    fn sound_that_would_arrive_late_is_dropped() {
        let (outbox, mut queue) = Outbox::new();
        let (_, mut sound) = outbox.sinks();
        sound.setup(AudioSetup {
            sample_rate: 48_000,
            channels: 2,
            samples_per_packet: 240,
            streams: 1,
            coupled_streams: 1,
            mapping: [0, 1, 0, 0, 0, 0, 0, 0],
        });
        for _ in 0..SOUND_WAITING * 2 {
            sound.packet(&[0xFC]);
        }
        let mut packets = 0;
        while let Ok(parcel) = queue.try_recv() {
            packets += usize::from(matches!(parcel, Parcel::Sound(_)));
        }
        assert_eq!(packets, SOUND_WAITING);
    }
}
