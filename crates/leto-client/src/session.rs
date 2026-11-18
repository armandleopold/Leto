//! A stream from a PC: what it carries and where it goes.
//!
//! The engine receives the stream and hands it over still encoded: H.264
//! pictures and Opus sound packets. Decoding and playing them is the job of
//! whoever starts the stream, through [`VideoSink`] and [`AudioSink`]. The
//! command line counts them, the app shows and plays them.

use std::{
    sync::{
        Arc, Mutex, OnceLock, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use moonlight_common::stream::{
    EncryptionFlags, MoonlightStreamSettings, StreamingConfig,
    audio::{AudioConfig, AudioDecoder, AudioFrame, OpusMultistreamConfig},
    connection::ConnectionListener,
    control::ActiveGamepads,
    proto::control::packet::{ControlPacket, TerminationReason},
    std::MoonlightStream,
    video::{
        ColorRange, ColorSpace, DecodeResult, FrameType, VideoDecodeUnit, VideoDecoder,
        VideoFormats, VideoSetup as ProtocolVideoSetup,
    },
};

/// How long to wait for a keyframe before asking the host again.
const KEYFRAME_PATIENCE: Duration = Duration::from_secs(1);

/// What to ask the host for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamSettings {
    pub width: u32,
    pub height: u32,
    /// Pictures per second.
    pub fps: u32,
    /// Video bit rate, in kilobits per second.
    pub bitrate_kbps: u32,
}

impl Default for StreamSettings {
    /// Full HD at 60 pictures per second and 20 Mbit/s: what Moonlight asks
    /// for by default, and what a home network carries without trouble.
    fn default() -> Self {
        Self { width: 1920, height: 1080, fps: 60, bitrate_kbps: 20_000 }
    }
}

impl StreamSettings {
    pub(crate) fn to_protocol(self) -> MoonlightStreamSettings {
        MoonlightStreamSettings {
            width: self.width,
            height: self.height,
            fps: self.fps,
            fps_x100: self.fps * 100,
            bitrate: self.bitrate_kbps,
            // The largest packet that crosses an ordinary network in one
            // piece. The protocol lowers it by itself for remote hosts.
            packet_size: 1392,
            // Nothing travels in the clear, on the home network either.
            encryption_flags: EncryptionFlags::AUDIO | EncryptionFlags::VIDEO,
            streaming_remotely: StreamingConfig::Auto,
            // Let the host set the game's resolution to the stream's.
            sops: true,
            hdr: false,
            supported_video_formats: VideoFormats::H264,
            // What a software decoder converts to the screen's colours.
            color_space: ColorSpace::Rec601,
            color_range: ColorRange::Limited,
            local_audio_play_mode: false,
            audio_config: AudioConfig::STEREO,
            gamepads_attached: ActiveGamepads::empty(),
            gamepads_persist_after_disconnect: false,
            enable_mic: false,
        }
    }
}

/// The video the host is about to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoSetup {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

/// One encoded picture.
#[derive(Debug, Clone, Copy)]
pub struct VideoFrame<'a> {
    /// Counts up by one per picture the host made. A gap means pictures were
    /// lost on the way.
    pub number: u32,
    /// A picture that decodes on its own. The others build on the ones before.
    pub keyframe: bool,
    /// An H.264 access unit with start codes.
    pub data: &'a [u8],
}

/// What a [`VideoSink`] made of a picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decoded {
    Ok,
    /// The picture could not be decoded, and neither can the following ones
    /// until the host sends a keyframe. The engine asks for one.
    NeedsKeyframe,
}

/// Receives the pictures of a stream, in order, on a thread of the stream.
pub trait VideoSink: Send + 'static {
    fn setup(&mut self, setup: VideoSetup);
    fn frame(&mut self, frame: VideoFrame<'_>) -> Decoded;
    /// The stream is over. Nothing follows.
    fn stop(&mut self) {}
}

/// The sound the host is about to send, in terms of an Opus decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioSetup {
    pub sample_rate: u32,
    pub channels: u32,
    /// Samples per channel in one packet.
    pub samples_per_packet: u32,
    /// Opus streams in a packet, and how many of them are stereo pairs. One
    /// and one for plain stereo.
    pub streams: u32,
    pub coupled_streams: u32,
    /// Which decoded channel goes to which speaker.
    pub mapping: [u8; 8],
}

/// Receives the sound packets of a stream, in order, on a thread of the
/// stream.
pub trait AudioSink: Send + 'static {
    fn setup(&mut self, setup: AudioSetup);
    /// One Opus packet.
    fn packet(&mut self, opus: &[u8]);
    /// The stream is over. Nothing follows.
    fn stop(&mut self) {}
}

/// Why a stream ended without the person asking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended {
    /// The app closed on the PC.
    ByHost,
    /// The stream broke. The app may still be running on the PC.
    ConnectionLost,
    /// The PC ended the stream and gave this error code.
    HostError(u32),
}

/// A running stream. Dropping it leaves the stream. The app keeps running on
/// the PC either way, and can be resumed.
pub struct Session {
    stream: Arc<MoonlightStream>,
    leaving: Arc<AtomicBool>,
}

impl Session {
    /// Connects the stream the host prepared. Blocks until pictures can flow.
    pub(crate) fn connect(
        config: moonlight_common::stream::MoonlightStreamConfig,
        settings: MoonlightStreamSettings,
        video: impl VideoSink,
        audio: impl AudioSink,
        ended: impl FnOnce(Ended) + Send + 'static,
    ) -> Result<Self, moonlight_common::error::Error> {
        let control = Arc::new(Control::default());
        let leaving = Arc::new(AtomicBool::new(false));

        let stream = MoonlightStream::connect(
            config,
            settings,
            VideoAdapter {
                sink: video,
                control: Arc::clone(&control),
                waiting_for_keyframe: None,
                scratch: Vec::new(),
            },
            AudioAdapter { sink: audio },
            Listener { ended: Some(Box::new(ended)), leaving: Arc::clone(&leaving) },
            Arc::new(moonlight_common::crypto::rustcrypto::RustCryptoBackend),
        )?;
        let stream = Arc::new(stream);
        // From here on the video side can reach the host.
        let _ = control.stream.set(Arc::clone(&stream));
        if control.keyframe_wanted_early.swap(false, Ordering::SeqCst) {
            control.request_keyframe();
        }
        Ok(Self { stream, leaving })
    }

    /// Whether the stream has ended, by either side.
    pub fn is_over(&self) -> bool {
        self.stream.is_stopped()
    }

    /// Leaves the stream. The app keeps running on the PC.
    pub fn stop(self) {
        // Dropping does it.
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Leaving is not an ending to report.
        self.leaving.store(true, Ordering::SeqCst);
        self.stream.stop();
    }
}

/// The way from the video side back to the host, once the stream is up.
#[derive(Default)]
struct Control {
    stream: OnceLock<Arc<MoonlightStream>>,
    /// A keyframe was needed before the stream could be asked for one.
    keyframe_wanted_early: AtomicBool,
}

impl Control {
    fn request_keyframe(&self) {
        match self.stream.get() {
            Some(stream) => {
                if let Err(error) = stream.send_input_raw(ControlPacket::RequestIdr) {
                    log::debug!("could not ask the host for a keyframe: {error}");
                }
            }
            None => self.keyframe_wanted_early.store(true, Ordering::SeqCst),
        }
    }
}

struct VideoAdapter<S> {
    sink: S,
    control: Arc<Control>,
    /// Since when the sink waits for a keyframe to continue.
    waiting_for_keyframe: Option<Instant>,
    /// The picture being handed over, put back together in one piece.
    scratch: Vec<u8>,
}

impl<S: VideoSink> VideoDecoder for VideoAdapter<S> {
    fn setup(&mut self, setup: ProtocolVideoSetup) -> i32 {
        self.sink.setup(VideoSetup {
            width: setup.width,
            height: setup.height,
            fps: setup.redraw_rate,
        });
        0
    }

    fn start(&mut self) {}

    fn submit_decode_unit(&mut self, unit: VideoDecodeUnit<&[u8]>) -> DecodeResult {
        let keyframe = unit.frame_type == FrameType::Idr;
        if let Some(since) = self.waiting_for_keyframe {
            if !keyframe {
                // Useless without the keyframe. Ask again if it is slow to come.
                if since.elapsed() > KEYFRAME_PATIENCE {
                    self.waiting_for_keyframe = Some(Instant::now());
                    self.control.request_keyframe();
                }
                return DecodeResult::NeedIdr;
            }
            self.waiting_for_keyframe = None;
        }

        // The protocol splits a keyframe into its parameter sets and its
        // picture. A decoder wants them back in one piece.
        self.scratch.clear();
        for buffer in &unit.buffers {
            self.scratch.extend_from_slice(buffer.data);
        }
        let frame = VideoFrame { number: unit.frame_number.0, keyframe, data: &self.scratch };
        match self.sink.frame(frame) {
            Decoded::Ok => DecodeResult::Ok,
            Decoded::NeedsKeyframe => {
                self.waiting_for_keyframe = Some(Instant::now());
                self.control.request_keyframe();
                DecodeResult::NeedIdr
            }
        }
    }

    fn stop(&mut self) {
        self.sink.stop();
    }

    fn supported_formats(&self) -> VideoFormats {
        VideoFormats::H264
    }
}

struct AudioAdapter<S> {
    sink: S,
}

impl<S: AudioSink> AudioDecoder for AudioAdapter<S> {
    fn setup(&mut self, _: AudioConfig, config: OpusMultistreamConfig) -> i32 {
        self.sink.setup(AudioSetup {
            sample_rate: config.sample_rate,
            channels: config.channel_count,
            samples_per_packet: config.samples_per_frame,
            streams: config.streams,
            coupled_streams: config.coupled_streams,
            mapping: config.mapping,
        });
        0
    }

    fn start(&mut self) {}

    fn stop(&mut self) {
        self.sink.stop();
    }

    fn decode_and_play_sample(&mut self, sample: AudioFrame<&[u8]>) {
        self.sink.packet(sample.buffer);
    }

    fn config(&self) -> AudioConfig {
        AudioConfig::STEREO
    }
}

type EndedCallback = Box<dyn FnOnce(Ended) + Send>;

struct Listener {
    ended: Option<EndedCallback>,
    leaving: Arc<AtomicBool>,
}

impl ConnectionListener for Listener {
    fn connection_terminated(&mut self, error_code: i32) {
        // Called for every ending, also the one this side asked for.
        if self.leaving.load(Ordering::SeqCst) {
            return;
        }
        let Some(ended) = self.ended.take() else { return };
        let code = error_code as u32;
        ended(match TerminationReason::GRACEFUL {
            TerminationReason::Long(graceful) if code == graceful => Ended::ByHost,
            _ if code == 0 => Ended::ConnectionLost,
            _ => Ended::HostError(code),
        });
    }
}

/// A sink that only counts, for diagnostics and tests.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    pub setup: Option<VideoSetup>,
    pub frames: u64,
    pub keyframes: u64,
    pub video_bytes: u64,
    /// Pictures the host made that never arrived.
    pub frames_missing: u64,
    last_number: Option<u32>,
    pub audio_setup: Option<AudioSetup>,
    pub audio_packets: u64,
    pub audio_bytes: u64,
    pub stopped: bool,
}

/// Counts what a stream delivers. Give [`Counter::video`] and
/// [`Counter::audio`] to the stream and read [`Counter::tally`] meanwhile.
#[derive(Debug, Default, Clone)]
pub struct Counter(Arc<Mutex<Tally>>);

impl Counter {
    pub fn tally(&self) -> Tally {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn video(&self) -> impl VideoSink + use<> {
        CountingSink(self.clone())
    }

    pub fn audio(&self) -> impl AudioSink + use<> {
        CountingSink(self.clone())
    }

    fn update(&self, change: impl FnOnce(&mut Tally)) {
        change(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner));
    }
}

struct CountingSink(Counter);

impl VideoSink for CountingSink {
    fn setup(&mut self, setup: VideoSetup) {
        self.0.update(|tally| tally.setup = Some(setup));
    }

    fn frame(&mut self, frame: VideoFrame<'_>) -> Decoded {
        self.0.update(|tally| {
            tally.frames += 1;
            tally.keyframes += u64::from(frame.keyframe);
            tally.video_bytes += frame.data.len() as u64;
            if let Some(last) = tally.last_number {
                tally.frames_missing += u64::from(frame.number.saturating_sub(last + 1));
            }
            tally.last_number = Some(frame.number);
        });
        Decoded::Ok
    }

    fn stop(&mut self) {
        self.0.update(|tally| tally.stopped = true);
    }
}

impl AudioSink for CountingSink {
    fn setup(&mut self, setup: AudioSetup) {
        self.0.update(|tally| tally.audio_setup = Some(setup));
    }

    fn packet(&mut self, opus: &[u8]) {
        self.0.update(|tally| {
            tally.audio_packets += 1;
            tally.audio_bytes += opus.len() as u64;
        });
    }
}
