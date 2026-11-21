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
    thread,
    time::{Duration, Instant},
};

use leto_core::{
    engine::{BITRATES_KBPS, Choices, Codec, Codecs, ScreenMode, bitrate_for},
    input::{GamepadButtons, GamepadKind, Input, InputAllowed, MouseButton},
};
use moonlight_common::{
    error::Error as ProtocolError,
    http::server_info::ApolloPermissions,
    stream::{
        EncryptionFlags, MoonlightStreamSettings, StreamingConfig,
        audio::{AudioConfig, AudioDecoder, AudioFrame, OpusMultistreamConfig},
        connection::ConnectionListener,
        control::{
            ActiveGamepads, ControllerButtons, ControllerCapabilities, ControllerType, KeyAction,
            KeyCode, KeyFlags, KeyModifiers, MouseButton as ProtocolMouseButton, MouseButtonAction,
            PacketSendError,
        },
        proto::control::{
            input_batcher::ClientInputEvent,
            packet::{ControlPacket, TerminationReason},
        },
        std::MoonlightStream,
        video::{
            ColorRange, ColorSpace, DecodeResult, FrameType, VideoDecodeUnit, VideoDecoder,
            VideoFormat, VideoFormats, VideoSetup as ProtocolVideoSetup,
        },
    },
};

/// How long to wait for a keyframe before asking the host again.
const KEYFRAME_PATIENCE: Duration = Duration::from_secs(1);
/// How long what was sent last gets to leave before a stream is left:
/// stopping drops what still waits to go out, such as the keys let go of
/// and the pointer the PC is asked to draw again.
const LAST_WORDS: Duration = Duration::from_millis(100);

/// The largest picture H.264 carries at a pace worth watching (its level
/// 5.2), and the one asked for when a screen is larger: as much of the
/// screen as fits, in the same shape.
const LARGEST: (u32, u32) = (4096, 2304);
/// The pace of a stream stays within these, whatever a screen says.
const SLOWEST: u32 = 10;
const FASTEST: u32 = 240;

/// What to ask the host for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamSettings {
    pub width: u32,
    pub height: u32,
    /// Pictures per second.
    pub fps: u32,
    /// Video bit rate, in kilobits per second.
    pub bitrate_kbps: u32,
    /// Asks the host for a screen of its own, made in the stream's size and
    /// pace, instead of showing one of its own screens. Apollo makes one;
    /// the others ignore the question.
    pub virtual_display: bool,
    /// The host may change a game's settings to suit the stream.
    pub optimize_games: bool,
    /// The host's own speakers keep playing too.
    pub sound_on_pc: bool,
    /// The codecs the pictures may come in. H.264 always may: every host
    /// sends it.
    pub codecs: Codecs,
}

impl Default for StreamSettings {
    /// Full HD at 60 pictures per second and 20 Mbit/s: what Moonlight asks
    /// for by default, and what a home network carries without trouble.
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 60,
            bitrate_kbps: 20_000,
            virtual_display: false,
            optimize_games: true,
            sound_on_pc: false,
            codecs: Codecs::default(),
        }
    }
}

impl StreamSettings {
    /// A stream that fills `screen` as it is: its size and its pace, on a
    /// screen the host makes just like it.
    ///
    /// The bit rate is the one [`bitrate_for`] gives the stream's size and
    /// pace.
    pub fn for_screen(screen: ScreenMode) -> Self {
        let (width, height) = fit(screen.width, screen.height);
        let fps = screen.refresh_hz.clamp(SLOWEST, FASTEST);
        Self {
            width,
            height,
            fps,
            bitrate_kbps: bitrate_for(width, height, fps),
            virtual_display: true,
            ..Self::default()
        }
    }

    /// A stream for `screen`, when the shell knows it, as the person chose.
    /// Without a screen, it is the default one, and the host makes none.
    pub fn chosen(screen: Option<ScreenMode>, choices: Choices) -> Self {
        let mut settings = screen.map_or_else(Self::default, Self::for_screen);
        settings.virtual_display &= choices.virtual_display;
        if let Some(kbps) = choices.bitrate_kbps {
            settings.bitrate_kbps = kbps.clamp(BITRATES_KBPS.0, BITRATES_KBPS.1);
        }
        settings.optimize_games = choices.optimize_games;
        settings.sound_on_pc = choices.sound_on_pc;
        settings.codecs = choices.codecs;
        settings
    }

    /// What the host's launch is told besides what the protocol says,
    /// starting with `&`.
    pub(crate) fn launch_parameters(self) -> String {
        let mut parameters = MoonlightStream::launch_query_parameters().to_owned();
        if self.virtual_display {
            // Apollo reads it. A screen its owner set for this device
            // overrides it. Its `scaleFactor` is not how large it draws text
            // on that screen: it multiplies the screen's size, and the
            // stream's with it, which only makes more pixels to decode.
            parameters.push_str("&virtualDisplay=1");
        }
        parameters
    }

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
            sops: self.optimize_games,
            hdr: false,
            supported_video_formats: video_formats(self.codecs),
            // What a software decoder converts to the screen's colours.
            color_space: ColorSpace::Rec601,
            color_range: ColorRange::Limited,
            local_audio_play_mode: self.sound_on_pc,
            audio_config: AudioConfig::STEREO,
            gamepads_attached: ActiveGamepads::empty(),
            gamepads_persist_after_disconnect: false,
            enable_mic: false,
        }
    }
}

/// `width` by `height`, or as much of it as H.264 carries in the same
/// shape, in even numbers as a video encoder wants them. Never nothing.
fn fit(width: u32, height: u32) -> (u32, u32) {
    let (width, height) = (width.max(2), height.max(2));
    let scale = (f64::from(LARGEST.0) / f64::from(width))
        .min(f64::from(LARGEST.1) / f64::from(height))
        .min(1.0);
    let even = |side: u32| ((f64::from(side) * scale) as u32 & !1).max(2);
    (even(width), even(height))
}

/// The video the host is about to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoSetup {
    pub codec: Codec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

/// What the protocol is told this device decodes: 8-bit pictures of the
/// codecs it has, which every PC can make. H.264 always.
fn video_formats(codecs: Codecs) -> VideoFormats {
    let mut formats = VideoFormats::H264;
    if codecs.contains(Codec::H265) {
        formats |= VideoFormats::H265;
    }
    if codecs.contains(Codec::Av1) {
        formats |= VideoFormats::AV1_MAIN8;
    }
    formats
}

/// The codec of what the protocol agreed with the host.
fn codec(format: VideoFormat) -> Codec {
    match format {
        VideoFormat::H264 | VideoFormat::H264High8_444 => Codec::H264,
        VideoFormat::H265
        | VideoFormat::H265Main10
        | VideoFormat::H265Rext8_444
        | VideoFormat::H265Rext10_444 => Codec::H265,
        VideoFormat::Av1Main8
        | VideoFormat::Av1Main10
        | VideoFormat::Av1High8_444
        | VideoFormat::Av1High10_444 => Codec::Av1,
    }
}

/// One encoded picture.
#[derive(Debug, Clone, Copy)]
pub struct VideoFrame<'a> {
    /// Counts up by one per picture the host made. A gap means pictures were
    /// lost on the way.
    pub number: u32,
    /// A picture that decodes on its own. The others build on the ones before.
    pub keyframe: bool,
    /// A whole picture: for H.264 and H.265 an access unit with start
    /// codes, for AV1 its OBUs one after the other.
    pub data: &'a [u8],
    /// How long the host took to capture and encode it, when it says.
    pub host_latency: Option<Duration>,
    /// How many more pictures have arrived behind this one and wait their
    /// turn. A sink that falls behind can skip showing this one: a newer
    /// one would replace it before anyone saw it.
    pub waiting: usize,
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

/// Why the PC did not get something the person did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InputError {
    /// The owner of the PC did not allow this device that kind of input.
    /// Apollo decides it device by device.
    #[error("the PC does not accept this kind of input from this device")]
    NotAllowed,
    #[error("the stream is not connected")]
    NotConnected,
}

/// What Apollo's permissions for this device mean for input. No permissions
/// at all is every host but Apollo: nobody decides, everything is allowed.
fn input_allowed(permissions: Option<&ApolloPermissions>) -> InputAllowed {
    permissions.map_or(InputAllowed::ALL, |granted| InputAllowed {
        keyboard: granted.contains(ApolloPermissions::INPUT_KEYBOARD),
        mouse: granted.contains(ApolloPermissions::INPUT_MOUSE),
        gamepad: granted.contains(ApolloPermissions::INPUT_CONTROLLER),
    })
}

/// A running stream. Dropping it leaves the stream. The app keeps running on
/// the PC either way, and can be resumed.
pub struct Session {
    stream: Arc<MoonlightStream>,
    leaving: Arc<AtomicBool>,
    input_allowed: InputAllowed,
    /// The host knows a wheel that turns sideways: Sunshine and its
    /// descendants do, Nvidia's old service does not.
    sideways_wheel: bool,
    /// The host takes the microphone of this device.
    microphone: bool,
    /// The stream waits a few milliseconds at a time, in the engine, in the
    /// decoders and in the sound: the system counts them finely meanwhile.
    _clock: leto_system::clock::Fine,
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
        // Before the first wait of the stream.
        let clock = leto_system::clock::fine();
        let control = Arc::new(Control::default());
        let leaving = Arc::new(AtomicBool::new(false));
        let input_allowed = input_allowed(config.apollo_permissions.as_ref());
        let sideways_wheel = config.version.is_sunshine_like();
        // Asked for, and the handshake only succeeds when the host agrees.
        let microphone = config.foundation_enable_mic;

        let formats = settings.supported_video_formats;
        let stream = MoonlightStream::connect(
            config,
            settings,
            VideoAdapter {
                sink: video,
                control: Arc::clone(&control),
                waiting_for_keyframe: None,
                scratch: Vec::new(),
                formats,
                waiting: 0,
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
        Ok(Self { stream, leaving, input_allowed, sideways_wheel, microphone, _clock: clock })
    }

    /// Whether the stream has ended, by either side.
    pub fn is_over(&self) -> bool {
        self.stream.is_stopped()
    }

    /// Leaves the stream, once what was sent last has had a moment to
    /// reach the PC. The app keeps running on the PC.
    pub fn stop(self) {
        // Leaving is not an ending to report, even while the last words go.
        self.leaving.store(true, Ordering::SeqCst);
        thread::sleep(LAST_WORDS);
        // Dropping does the rest.
    }

    /// What the owner of the PC lets this device do with keyboard, mouse and
    /// gamepads, as it was when the stream started.
    pub fn input_allowed(&self) -> InputAllowed {
        self.input_allowed
    }

    /// How long a message takes to reach the host and come back, as the
    /// connection measures it. `None` until it has.
    pub fn round_trip(&self) -> Option<Duration> {
        self.stream.estimated_rtt().ok().map(|estimate| estimate.rtt)
    }

    /// Whether the PC takes the microphone of this device. Few do: it needs
    /// the Apollo microphone fork, Foundation Sunshine, or Leto on the PC.
    pub fn takes_microphone(&self) -> bool {
        self.microphone
    }

    /// Sends a moment of the microphone: one Opus frame, one channel at
    /// 48 kHz, that starts `at` this long after the first one. Does not
    /// wait. It travels encrypted, like the rest of the stream. Returns
    /// whether it left.
    pub fn send_microphone(&self, at: Duration, frame: &[u8]) -> bool {
        self.microphone && self.stream.send_microphone_opus_data(at, frame)
    }

    /// Passes on something the person did with the keyboard, the mouse or a
    /// gamepad. Does not wait, and the PC gets everything in the order it was
    /// sent: a click never overtakes the movement before it.
    ///
    /// Nothing is gathered on the way, so add up the mouse movements that
    /// come together before sending them. A gamepad must have arrived before
    /// its state is sent. The PC repeats a held key by itself, so a key that
    /// is already down is not sent again.
    pub fn send(&self, input: Input) -> Result<(), InputError> {
        match input {
            Input::MouseMove { dx, dy } => {
                self.send_mouse(ControlPacket::MouseMoveRelative { delta_x: dx, delta_y: dy })
            }
            Input::MousePosition { x, y, width, height } => {
                if width <= 0 || height <= 0 {
                    return Ok(());
                }
                self.send_mouse(ControlPacket::MouseMoveAbsolute {
                    x,
                    y,
                    unused: 0,
                    reference_width: width,
                    reference_height: height,
                })
            }
            Input::Scroll { horizontal, vertical } => {
                if vertical != 0 {
                    self.send_mouse(ControlPacket::MouseScroll {
                        scroll_amount_1: vertical,
                        scroll_amount_2: vertical,
                        zero: 0,
                    })?;
                }
                if horizontal != 0 && self.sideways_wheel {
                    self.send_mouse(ControlPacket::MouseHorizontalScroll {
                        scroll_amount: horizontal,
                    })?;
                }
                Ok(())
            }
            other => self.stream.send_input(protocol_event(other)).map_err(refusal),
        }
    }

    /// Mouse movements and the wheel go out directly. The protocol library
    /// would hold them back for a millisecond to gather them, and let a
    /// click sent meanwhile arrive first.
    fn send_mouse(&self, packet: ControlPacket) -> Result<(), InputError> {
        if !self.input_allowed.mouse {
            return Err(InputError::NotAllowed);
        }
        self.stream.send_input_raw(packet).map_err(refusal)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Leaving is not an ending to report.
        self.leaving.store(true, Ordering::SeqCst);
        self.stream.stop();
    }
}

fn refusal(error: ProtocolError) -> InputError {
    match error {
        ProtocolError::PacketSend(PacketSendError::ApolloPermissionDenied) => {
            InputError::NotAllowed
        }
        other => {
            log::debug!("an input did not leave: {other}");
            InputError::NotConnected
        }
    }
}

/// `input` in the protocol's terms, for everything but the movements of the
/// mouse and its wheel.
fn protocol_event(input: Input) -> ClientInputEvent {
    match input {
        Input::Key { key, down, modifiers } => {
            let mut held = KeyModifiers::empty();
            held.set(KeyModifiers::SHIFT, modifiers.shift);
            held.set(KeyModifiers::CTRL, modifiers.ctrl);
            held.set(KeyModifiers::ALT, modifiers.alt);
            held.set(KeyModifiers::META, modifiers.meta);
            ClientInputEvent::Keyboard {
                action: if down { KeyAction::Down } else { KeyAction::Up },
                // The code names a place on a US keyboard, and the host
                // applies its own layout: what Moonlight calls normalized.
                flags: KeyFlags::empty(),
                key_code: KeyCode(key.0 as i16),
                modifiers: held,
            }
        }
        // These three never come here: `Session::send` sends them itself.
        Input::MouseMove { dx, dy } => {
            ClientInputEvent::MouseMoveRelative { delta_x: dx, delta_y: dy }
        }
        Input::MousePosition { x, y, width, height } => ClientInputEvent::MouseMoveAbsolute {
            x,
            y,
            reference_width: width,
            reference_height: height,
        },
        Input::Scroll { vertical, .. } => {
            ClientInputEvent::MouseScrollVertical { scroll_y: vertical }
        }
        Input::MouseButton { button, down } => ClientInputEvent::MouseButton {
            action: if down { MouseButtonAction::Press } else { MouseButtonAction::Release },
            button: match button {
                MouseButton::Left => ProtocolMouseButton::Left,
                MouseButton::Middle => ProtocolMouseButton::Middle,
                MouseButton::Right => ProtocolMouseButton::Right,
                MouseButton::Back => ProtocolMouseButton::X1,
                MouseButton::Forward => ProtocolMouseButton::X2,
            },
        },
        Input::GamepadArrived { number, kind } => ClientInputEvent::ControllerConnect {
            controller_number: number,
            ty: match kind {
                GamepadKind::Unknown => ControllerType::Unknown,
                GamepadKind::Xbox => ControllerType::Xbox,
                GamepadKind::PlayStation => ControllerType::PlayStation,
                GamepadKind::Nintendo => ControllerType::Nintendo,
            },
            capabilities: ControllerCapabilities::ANALOG_TRIGGERS,
            supported_buttons: protocol_buttons(
                GamepadButtons::EACH.into_iter().fold(GamepadButtons::NONE, GamepadButtons::with),
            ),
        },
        Input::Gamepad { number, state } => ClientInputEvent::ControllerState {
            controller_number: number,
            pressed_buttons: protocol_buttons(state.buttons),
            left_trigger: state.left_trigger,
            right_trigger: state.right_trigger,
            left_stick_x: state.left_stick[0],
            left_stick_y: state.left_stick[1],
            right_stick_x: state.right_stick[0],
            right_stick_y: state.right_stick[1],
        },
        Input::GamepadLeft { number } => {
            ClientInputEvent::ControllerDisconnect { controller_number: number }
        }
    }
}

fn protocol_buttons(buttons: GamepadButtons) -> ControllerButtons {
    let mut held = ControllerButtons::empty();
    for (ours, theirs) in [
        (GamepadButtons::A, ControllerButtons::A),
        (GamepadButtons::B, ControllerButtons::B),
        (GamepadButtons::X, ControllerButtons::X),
        (GamepadButtons::Y, ControllerButtons::Y),
        (GamepadButtons::UP, ControllerButtons::UP),
        (GamepadButtons::DOWN, ControllerButtons::DOWN),
        (GamepadButtons::LEFT, ControllerButtons::LEFT),
        (GamepadButtons::RIGHT, ControllerButtons::RIGHT),
        (GamepadButtons::LEFT_SHOULDER, ControllerButtons::LB),
        (GamepadButtons::RIGHT_SHOULDER, ControllerButtons::RB),
        (GamepadButtons::START, ControllerButtons::PLAY),
        (GamepadButtons::BACK, ControllerButtons::BACK),
        (GamepadButtons::LEFT_STICK, ControllerButtons::LS_CLK),
        (GamepadButtons::RIGHT_STICK, ControllerButtons::RS_CLK),
        (GamepadButtons::GUIDE, ControllerButtons::SPECIAL),
    ] {
        held.set(theirs, buttons.contains(ours));
    }
    held
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
    /// What the protocol was told this device decodes.
    formats: VideoFormats,
    /// Pictures waiting behind the next one.
    waiting: usize,
}

impl<S: VideoSink> VideoDecoder for VideoAdapter<S> {
    fn setup(&mut self, setup: ProtocolVideoSetup) -> i32 {
        self.sink.setup(VideoSetup {
            codec: codec(setup.format),
            width: setup.width,
            height: setup.height,
            fps: setup.redraw_rate,
        });
        0
    }

    fn start(&mut self) {}

    fn frames_waiting(&mut self, count: usize) {
        self.waiting = count;
    }

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
        let frame = VideoFrame {
            number: unit.frame_number.0,
            keyframe,
            data: &self.scratch,
            host_latency: unit.frame_processing_latency,
            waiting: self.waiting,
        };
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
        self.formats
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
    /// Pictures the host made that never arrived, or arrived too late to
    /// be waited for.
    pub frames_missing: u64,
    /// Pictures that came after missing ones and were no keyframes: they
    /// build on pictures nobody has, and should never have been handed over.
    pub frames_orphaned: u64,
    /// The most pictures that were ever waiting behind one.
    pub most_waiting: usize,
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
            tally.most_waiting = tally.most_waiting.max(frame.waiting);
            if let Some(last) = tally.last_number {
                let missing = u64::from(frame.number.saturating_sub(last + 1));
                tally.frames_missing += missing;
                tally.frames_orphaned += u64::from(missing > 0 && !frame.keyframe);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(width: u32, height: u32, refresh_hz: u32) -> ScreenMode {
        ScreenMode { width, height, refresh_hz }
    }

    #[test]
    fn a_stream_for_a_screen_is_made_in_its_size_and_pace() {
        let settings = StreamSettings::for_screen(screen(2560, 1440, 144));
        assert_eq!((settings.width, settings.height, settings.fps), (2560, 1440, 144));
        assert!(settings.virtual_display);
        // Full HD at 60 is what Moonlight asks for by default, at 20 Mbit/s.
        let full_hd = StreamSettings::for_screen(screen(1920, 1080, 60));
        assert_eq!(full_hd.bitrate_kbps, 20_000);
        assert_eq!(StreamSettings { virtual_display: false, ..full_hd }, StreamSettings::default());
    }

    #[test]
    fn the_bit_rate_grows_with_the_pixels_and_less_than_the_pace() {
        let bitrate = |width, height, fps| {
            StreamSettings::for_screen(screen(width, height, fps)).bitrate_kbps
        };
        assert_eq!(bitrate(3840, 2160, 60), 80_000);
        assert_eq!(bitrate(1920, 1080, 30), 10_000);
        assert_eq!(bitrate(1280, 720, 60), 8_889);
        // Twice the pace is not twice the bits.
        assert_eq!(bitrate(1920, 1080, 120), 28_284);
        // Within what Moonlight asks for at most, and enough to be seen.
        assert_eq!(bitrate(3840, 2160, 240), 150_000);
        assert_eq!(bitrate(640, 360, 30), 2_000);
    }

    #[test]
    fn a_screen_larger_than_h264_carries_is_asked_for_in_its_shape() {
        let fitted = |width, height| {
            let settings = StreamSettings::for_screen(screen(width, height, 60));
            (settings.width, settings.height)
        };
        assert_eq!(fitted(3840, 2160), (3840, 2160));
        assert_eq!(fitted(5120, 2880), (4096, 2304));
        // An ultrawide screen keeps its shape.
        assert_eq!(fitted(5120, 1440), (4096, 1152));
        // Odd sides become even, and nothing becomes nothing.
        assert_eq!(fitted(1367, 769), (1366, 768));
        assert_eq!(fitted(0, 0), (2, 2));
    }

    #[test]
    fn a_screens_pace_stays_within_what_makes_sense() {
        let settings = StreamSettings::for_screen(screen(1920, 1080, 0));
        assert_eq!(settings.fps, SLOWEST);
        let settings = StreamSettings::for_screen(screen(1920, 1080, 500));
        assert_eq!(settings.fps, FASTEST);
    }

    #[test]
    fn a_stream_is_what_the_person_chose() {
        let native = screen(2560, 1440, 144);
        // As Leto would have it.
        assert_eq!(
            StreamSettings::chosen(Some(native), Choices::default()),
            StreamSettings::for_screen(native)
        );
        assert_eq!(StreamSettings::chosen(None, Choices::default()), StreamSettings::default());
        // As the person would.
        let chosen = Choices {
            bitrate_kbps: Some(35_000),
            virtual_display: false,
            optimize_games: false,
            sound_on_pc: true,
            codecs: Codecs::of(Codec::Av1).with(Codec::H264),
        };
        let settings = StreamSettings::chosen(Some(native), chosen);
        assert_eq!((settings.width, settings.height, settings.fps), (2560, 1440, 144));
        assert_eq!(settings.bitrate_kbps, 35_000);
        assert!(!settings.virtual_display);
        let protocol = settings.to_protocol();
        assert!(!protocol.sops);
        assert!(protocol.local_audio_play_mode);
        assert_eq!(protocol.supported_video_formats, VideoFormats::AV1_MAIN8 | VideoFormats::H264);
        // H.264 is offered whatever else is: every PC makes it.
        let none = Choices { codecs: Codecs::NONE, ..Choices::default() };
        assert_eq!(
            StreamSettings::chosen(None, none).to_protocol().supported_video_formats,
            VideoFormats::H264
        );
        // Within what a stream can be given.
        let greedy = Choices { bitrate_kbps: Some(900_000), ..Choices::default() };
        assert_eq!(StreamSettings::chosen(None, greedy).bitrate_kbps, BITRATES_KBPS.1);
    }

    #[test]
    fn the_host_is_asked_for_a_screen_of_its_own_only_for_a_screen() {
        let plain = StreamSettings::default().launch_parameters();
        assert_eq!(plain, MoonlightStream::launch_query_parameters());
        // Never with a scale: Apollo would make the screen that much larger
        // than this one.
        let native = StreamSettings::for_screen(screen(2560, 1440, 144)).launch_parameters();
        assert_eq!(native, format!("{plain}&virtualDisplay=1"));
    }
}
