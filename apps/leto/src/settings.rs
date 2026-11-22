//! What the person chose for Leto, kept from one start to the next.
//!
//! Every choice has a default, which is how Leto behaves for someone who
//! never opens the settings. A choice missing from what was saved, as after
//! an update that adds one, is its default; a saved file that cannot be
//! read is all defaults.

use leto_core::{
    engine::{Choices, Codec, Codecs, Decodable, ScreenMode, bitrate_for},
    model::HostId,
};
use serde::{Deserialize, Serialize};

/// The picture sizes offered besides the screen's own, as in Moonlight.
pub const RESOLUTIONS: [[u32; 2]; 4] = [[1280, 720], [1920, 1080], [2560, 1440], [3840, 2160]];
/// The paces offered besides the screen's own, in pictures per second.
pub const FRAME_RATES: [u32; 7] = [30, 60, 90, 120, 144, 165, 240];

/// How much larger the PC draws everything than at its own size, in
/// percent: the stream then has that many times fewer pixels each way.
pub const SCALES: [u32; 7] = [100, 125, 150, 175, 200, 250, 300];

/// The scale by default. A phone's screen is small and dense: at its own
/// pixels, the PC's text is too small to read, and at 200% it is about
/// the size it is on a laptop held further away.
pub const DEFAULT_SCALE: u32 = if cfg!(target_os = "android") { 200 } else { 100 };

/// A screen as a PC makes one when the window does not know its own.
const PLAIN: ScreenMode = ScreenMode { width: 1920, height: 1080, refresh_hz: 60 };

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    // The picture.
    /// Width and height of the stream. `None`: those of this screen.
    pub resolution: Option<[u32; 2]>,
    /// With the screen's own resolution, how much larger the PC draws, in
    /// percent: a stream of 2856 by 1280 at 200% is one of 1424 by 640.
    pub scale: u32,
    /// The same on a monitor a phone is plugged into, which is as large as
    /// a PC's own and needs no larger text.
    pub plugged_scale: u32,
    /// Pictures per second. `None`: as many as this screen shows.
    pub frame_rate: Option<u32>,
    /// Video bit rate, in kilobits per second. `None`: what the size and
    /// the pace call for.
    pub bitrate_kbps: Option<u32>,
    /// A stream fills the screen as soon as it connects.
    pub full_screen: bool,
    /// The window waits for the screen before it shows a picture, so none
    /// is ever cut in two. Only read when Leto starts.
    pub vsync: bool,
    /// A PC that can make a screen of its own for the stream, as Apollo
    /// does, is asked to.
    pub virtual_display: bool,

    // The sound.
    /// The PC's own speakers are silent during a stream.
    pub mute_pc: bool,
    /// Leto is silent while another window is in front.
    pub mute_in_background: bool,

    // The PC.
    /// The PC may change a game's settings to suit the stream.
    pub optimize_games: bool,
    /// Leaving a stream closes the app on the PC too.
    pub quit_on_leave: bool,

    // Keyboard, mouse and gamepads.
    /// When the system's own shortcuts go to the PC.
    pub capture_shortcuts: Capture,
    /// The pointer on the picture is this device's own, which follows the
    /// mouse at once: the PC hears where it goes and is asked not to draw
    /// its own, where it can be.
    pub local_pointer: bool,
    /// Not a choice: the PCs Leto asked to leave their own pointer out of
    /// the pictures, while this device's shows instead. A PC only switches,
    /// and stays as it is for every device until it is asked again, even
    /// after a stream that ended without Leto leaving it. Kept here so that
    /// Leto still knows what it asked once it was closed and started again.
    pub pointers_left_out: Vec<HostId>,
    pub swap_mouse_buttons: bool,
    /// The wheel scrolls the other way on the PC.
    pub reverse_scroll: bool,
    /// A and B change places, and X and Y, for gamepads labelled the other
    /// way round.
    pub swap_gamepad_buttons: bool,

    // Advanced.
    /// Who decodes the pictures.
    pub decoder: Decoder,
    /// The codec the PC is asked for. `None`: the best this device's
    /// graphics card decodes, AV1 then H.265, and otherwise H.264.
    pub codec: Option<Codec>,
    /// The statistics show from the start of every stream.
    pub statistics: bool,

    /// The camera, microphone and speakers of this device that Leto uses,
    /// chosen on their own page.
    pub devices: DeviceChoices,
}

/// The camera, microphone and speakers Leto uses, by the names the system
/// gives them. `None` is the system's default: for a camera, its first one
/// with colour. One that is gone is replaced by the default, and comes
/// back when it does.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceChoices {
    pub camera: Option<String>,
    pub microphone: Option<String>,
    pub speakers: Option<String>,
}

/// Who decodes the pictures of a stream.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decoder {
    /// The graphics card when it can, the processor otherwise.
    #[default]
    Automatic,
    /// The processor, always.
    Software,
}

impl Decoder {
    pub const ALL: [Self; 2] = [Self::Automatic, Self::Software];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Automatic => "Automatic",
            Self::Software => "Software",
        }
    }

    /// The graphics card may decode.
    pub const fn hardware(self) -> bool {
        matches!(self, Self::Automatic)
    }
}

/// The codecs offered to choose from, `None` for the automatic one.
pub const CODECS: [Option<Codec>; 4] =
    [None, Some(Codec::Av1), Some(Codec::H265), Some(Codec::H264)];

pub const fn codec_name(codec: Option<Codec>) -> &'static str {
    match codec {
        None => "Automatic",
        Some(codec) => codec.name(),
    }
}

/// When the system's own shortcuts, such as the Windows key or Alt+Tab, go
/// to the PC.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Capture {
    /// While the stream has the keyboard.
    #[default]
    Always,
    /// While the stream also fills the screen.
    FullScreen,
    /// Never: they act on this device.
    Never,
}

impl Default for Settings {
    /// Leto as it is for someone who never opens the settings: a stream that
    /// is just like this screen and fills it, and every key for the PC.
    fn default() -> Self {
        Self {
            resolution: None,
            scale: DEFAULT_SCALE,
            plugged_scale: 100,
            frame_rate: None,
            bitrate_kbps: None,
            full_screen: true,
            vsync: true,
            virtual_display: true,
            mute_pc: true,
            mute_in_background: false,
            optimize_games: true,
            quit_on_leave: false,
            capture_shortcuts: Capture::Always,
            local_pointer: true,
            pointers_left_out: Vec::new(),
            swap_mouse_buttons: false,
            reverse_scroll: false,
            swap_gamepad_buttons: false,
            decoder: Decoder::Automatic,
            codec: None,
            statistics: false,
            devices: DeviceChoices::default(),
        }
    }
}

impl Capture {
    pub const ALL: [Self; 3] = [Self::Always, Self::FullScreen, Self::Never];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Always => "Always",
            Self::FullScreen => "In full screen",
            Self::Never => "Never",
        }
    }

    /// Whether the system's shortcuts go to the PC, for a window that fills
    /// the screen or not.
    pub const fn takes(self, fills_screen: bool) -> bool {
        match self {
            Self::Always => true,
            Self::FullScreen => fills_screen,
            Self::Never => false,
        }
    }
}

impl Settings {
    /// What "Restore the defaults" leaves: every default, and the camera,
    /// microphone and sound output chosen, which have a page of their own
    /// and are this device's, not a matter of taste.
    pub fn defaults(&self) -> Self {
        Self { devices: self.devices.clone(), ..Self::default() }
    }

    /// The screen a stream is made for, from `native`, this screen when the
    /// window knows it, and what the PC is told besides, for a device that
    /// decodes `decodable`.
    pub fn stream(
        &self,
        native: Option<ScreenMode>,
        decodable: Decodable,
    ) -> (Option<ScreenMode>, Choices) {
        let choices = Choices {
            bitrate_kbps: self.bitrate_kbps,
            virtual_display: self.virtual_display,
            optimize_games: self.optimize_games,
            sound_on_pc: !self.mute_pc,
            codecs: self.codecs(decodable),
        };
        (self.screen(native), choices)
    }

    /// The codecs the PC may send, of those this device decodes: the one
    /// chosen, or by default those the graphics card decodes. H.264 always,
    /// as every PC makes it: the PC picks the best of them it can.
    pub fn codecs(&self, decodable: Decodable) -> Codecs {
        let wanted = match self.codec {
            Some(codec) if decodable.any.contains(codec) => Codecs::of(codec),
            None if self.decoder.hardware() => decodable.hardware,
            _ => Codecs::NONE,
        };
        wanted.with(Codec::H264)
    }

    /// The screen a stream is made for. `None` when there is nothing to go
    /// by: the stream is then the engine's default.
    pub fn screen(&self, native: Option<ScreenMode>) -> Option<ScreenMode> {
        let native = native.map(|native| self.scaled(native));
        if self.resolution.is_none() && self.frame_rate.is_none() {
            return native;
        }
        // What is not chosen is the screen's, or what a PC makes.
        let base = native.unwrap_or(PLAIN);
        let [width, height] = self.resolution.unwrap_or([base.width, base.height]);
        Some(ScreenMode { width, height, refresh_hz: self.frame_rate.unwrap_or(base.refresh_hz) })
    }

    /// The scale for the screen the window is on: [`Self::plugged_scale`]
    /// on a monitor a phone is `plugged` into, [`Self::scale`] elsewhere.
    pub fn scale_here(&self, plugged: bool) -> u32 {
        if plugged { self.plugged_scale } else { self.scale }
    }

    /// The scale [`Self::scale_here`] gives, to change it.
    pub fn scale_here_mut(&mut self, plugged: bool) -> &mut u32 {
        if plugged { &mut self.plugged_scale } else { &mut self.scale }
    }

    /// `native` with [`Self::scale_here`]'s fewer pixels, in the same shape, with
    /// a width the encoders take (a multiple of 8) and an even height.
    fn scaled(&self, native: ScreenMode) -> ScreenMode {
        let scale = u64::from(self.scale_here(crate::window::on_plugged_screen()).clamp(100, 400));
        if scale == 100 {
            return native;
        }
        let shrink = |size: u32, step: u64| {
            let size = u64::from(size) * 100 / scale;
            (size / step * step).max(step) as u32
        };
        ScreenMode {
            width: shrink(native.width, 8),
            height: shrink(native.height, 2),
            refresh_hz: native.refresh_hz,
        }
    }

    /// The bit rate a stream gets when the person leaves it to Leto, in
    /// kilobits per second.
    pub fn automatic_bitrate(&self, native: Option<ScreenMode>) -> u32 {
        let screen = self.screen(native).unwrap_or(PLAIN);
        bitrate_for(screen.width, screen.height, screen.refresh_hz)
    }

    /// What was saved, read back. Anything else is the defaults.
    pub fn from_saved(text: &str) -> Self {
        serde_json::from_str(text).unwrap_or_else(|error| {
            log::warn!("the saved settings cannot be read, so they are the defaults: {error}");
            Self::default()
        })
    }

    pub fn to_saved(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

/// Where the settings are kept: a file next to the paired PCs on the
/// desktop, the page's storage in a browser.
#[cfg(not(target_arch = "wasm32"))]
mod kept {
    use std::{fs, io, path::PathBuf};

    use super::Settings;

    fn file() -> io::Result<PathBuf> {
        let folder = leto_client::default_directory()
            .map_err(|error| io::Error::new(io::ErrorKind::NotFound, error.to_string()))?;
        Ok(folder.join("settings.json"))
    }

    pub fn load() -> Settings {
        match file().and_then(fs::read_to_string) {
            Ok(text) => Settings::from_saved(&text),
            // Nobody chose anything yet.
            Err(error) if error.kind() == io::ErrorKind::NotFound => Settings::default(),
            Err(error) => {
                log::warn!("the settings cannot be read, so they are the defaults: {error}");
                Settings::default()
            }
        }
    }

    pub fn save(settings: Settings) {
        let written = file().and_then(|file| {
            if let Some(folder) = file.parent() {
                fs::create_dir_all(folder)?;
            }
            // Written whole, then put in place: a crash leaves the old file.
            let fresh = file.with_extension("json.new");
            fs::write(&fresh, settings.to_saved())?;
            fs::rename(&fresh, &file)
        });
        if let Err(error) = written {
            log::warn!("the settings could not be saved: {error}");
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod kept {
    use super::Settings;

    /// The key in the page's storage.
    const KEY: &str = "leto.settings";

    fn storage() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok().flatten()
    }

    pub fn load() -> Settings {
        match storage().and_then(|storage| storage.get_item(KEY).ok().flatten()) {
            Some(text) => Settings::from_saved(&text),
            None => Settings::default(),
        }
    }

    pub fn save(settings: Settings) {
        let saved = storage().map(|storage| storage.set_item(KEY, &settings.to_saved()));
        if !matches!(saved, Some(Ok(()))) {
            log::warn!("the browser did not keep the settings");
        }
    }
}

pub use kept::{load, save};

#[cfg(test)]
mod tests {
    use super::*;

    const NATIVE: ScreenMode = ScreenMode { width: 2560, height: 1600, refresh_hz: 120 };

    /// A device whose graphics card decodes H.265 and H.264, and that
    /// decodes AV1 on the processor.
    const DEVICE: Decodable = Decodable {
        hardware: Codecs::of(Codec::H265).with(Codec::H264),
        any: Codecs::of(Codec::Av1).with(Codec::H265).with(Codec::H264),
    };

    #[test]
    fn by_default_a_stream_is_just_like_this_screen() {
        let (screen, choices) = Settings::default().stream(Some(NATIVE), Decodable::SOFTWARE);
        assert_eq!(screen, Some(NATIVE));
        assert_eq!(choices, Choices::default());
        let plain = Settings::default().stream(None, Decodable::SOFTWARE);
        assert_eq!(plain, (None, Choices::default()));
    }

    #[test]
    fn a_monitor_a_phone_is_plugged_into_keeps_a_scale_of_its_own() {
        let settings = Settings { scale: 200, ..Settings::default() };
        assert_eq!(settings.scale_here(false), 200);
        assert_eq!(settings.scale_here(true), 100);
        let mut settings = settings;
        *settings.scale_here_mut(true) = 125;
        assert_eq!((settings.scale, settings.plugged_scale), (200, 125));
    }

    #[test]
    fn a_scale_shrinks_the_screens_own_size_and_keeps_its_shape() {
        let phone = ScreenMode { width: 2856, height: 1280, refresh_hz: 120 };
        let at = |scale| Settings { scale, ..Settings::default() }.screen(Some(phone));
        assert_eq!(at(100), Some(phone));
        assert_eq!(at(200), Some(ScreenMode { width: 1424, height: 640, refresh_hz: 120 }));
        assert_eq!(at(150), Some(ScreenMode { width: 1904, height: 852, refresh_hz: 120 }));
        assert_eq!(at(300), Some(ScreenMode { width: 952, height: 426, refresh_hz: 120 }));
        // A resolution chosen is the one the PC is asked for.
        let sized = Settings { scale: 200, resolution: Some([1920, 1080]), ..Settings::default() };
        assert_eq!(sized.screen(Some(phone)).map(|mode| mode.width), Some(1920));
        // Without a screen to go by, the stream is the engine's default.
        assert_eq!(Settings { scale: 200, ..Settings::default() }.screen(None), None);
    }

    #[test]
    fn a_chosen_size_or_pace_keeps_the_rest_of_the_screen() {
        let sized = Settings { resolution: Some([1920, 1080]), ..Settings::default() };
        assert_eq!(
            sized.screen(Some(NATIVE)),
            Some(ScreenMode { width: 1920, height: 1080, refresh_hz: 120 })
        );
        let paced = Settings { frame_rate: Some(60), ..Settings::default() };
        assert_eq!(
            paced.screen(Some(NATIVE)),
            Some(ScreenMode { width: 2560, height: 1600, refresh_hz: 60 })
        );
        // A window that does not know its screen goes by Full HD at 60.
        assert_eq!(paced.screen(None), Some(ScreenMode { refresh_hz: 60, ..PLAIN }));
        let both = Settings { resolution: Some([3840, 2160]), ..paced };
        assert_eq!(
            both.screen(None),
            Some(ScreenMode { width: 3840, height: 2160, refresh_hz: 60 })
        );
    }

    #[test]
    fn what_the_pc_is_told_follows_the_choices() {
        let chosen = Settings {
            bitrate_kbps: Some(40_000),
            virtual_display: false,
            mute_pc: false,
            optimize_games: false,
            ..Settings::default()
        };
        let (_, choices) = chosen.stream(None, DEVICE);
        assert_eq!(
            choices,
            Choices {
                bitrate_kbps: Some(40_000),
                virtual_display: false,
                optimize_games: false,
                sound_on_pc: true,
                codecs: Codecs::of(Codec::H265).with(Codec::H264),
            }
        );
    }

    #[test]
    fn the_pc_is_offered_what_the_graphics_card_decodes_or_what_was_chosen() {
        let automatic = Settings::default();
        // The best the card decodes, and H.264 to fall back on.
        assert_eq!(automatic.codecs(DEVICE), DEVICE.hardware);
        assert_eq!(automatic.codecs(Decodable::SOFTWARE), Codecs::of(Codec::H264));
        // AV1 only decodes on the processor here, so it takes being chosen.
        let av1 = Settings { codec: Some(Codec::Av1), ..automatic.clone() };
        assert_eq!(av1.codecs(DEVICE), Codecs::of(Codec::Av1).with(Codec::H264));
        // A codec chosen on another device, that this one cannot decode.
        assert_eq!(av1.codecs(Decodable::SOFTWARE), Codecs::of(Codec::H264));
        let h264 = Settings { codec: Some(Codec::H264), ..automatic.clone() };
        assert_eq!(h264.codecs(DEVICE), Codecs::of(Codec::H264));
        // The processor alone gets H.264, unless something else is chosen.
        let software = Settings { decoder: Decoder::Software, ..automatic.clone() };
        assert_eq!(software.codecs(DEVICE), Codecs::of(Codec::H264));
        let h265 = Settings { codec: Some(Codec::H265), ..software };
        assert_eq!(h265.codecs(DEVICE), Codecs::of(Codec::H265).with(Codec::H264));
    }

    #[test]
    fn the_automatic_bit_rate_follows_the_size_and_the_pace() {
        let settings = Settings::default();
        assert_eq!(settings.automatic_bitrate(None), 20_000);
        let four_k = Settings { resolution: Some([3840, 2160]), frame_rate: Some(60), ..settings };
        assert_eq!(four_k.automatic_bitrate(Some(NATIVE)), 80_000);
    }

    #[test]
    fn the_system_shortcuts_go_to_the_pc_as_chosen() {
        assert!(Capture::Always.takes(false));
        assert!(Capture::FullScreen.takes(true));
        assert!(!Capture::FullScreen.takes(false));
        assert!(!Capture::Never.takes(true));
    }

    #[test]
    fn restoring_the_defaults_keeps_the_devices_chosen() {
        let devices = DeviceChoices { camera: Some("USB Camera".to_owned()), ..Default::default() };
        let chosen = Settings { statistics: true, devices: devices.clone(), ..Settings::default() };
        assert_eq!(chosen.defaults(), Settings { devices, ..Settings::default() });
        assert_eq!(Settings::default().defaults(), Settings::default());
    }

    #[test]
    fn saved_settings_read_back_and_what_is_missing_is_its_default() {
        let chosen = Settings {
            resolution: Some([2560, 1440]),
            bitrate_kbps: Some(50_000),
            capture_shortcuts: Capture::FullScreen,
            swap_gamepad_buttons: true,
            decoder: Decoder::Software,
            codec: Some(Codec::Av1),
            devices: DeviceChoices {
                camera: Some("USB Camera".to_owned()),
                microphone: None,
                speakers: Some("Headphones (USB Audio)".to_owned()),
            },
            ..Settings::default()
        };
        assert_eq!(Settings::from_saved(&chosen.to_saved()), chosen);
        // Saved by a Leto that knew fewer choices.
        let older = Settings::from_saved(r#"{"frame_rate":60,"statistics":true}"#);
        assert_eq!(
            older,
            Settings { frame_rate: Some(60), statistics: true, ..Settings::default() }
        );
        // Choices an older Leto offered and this one no longer does.
        let gone = r#"{"statistics":true,"keep_on_card":false,"straight_to_screen":false}"#;
        assert_eq!(
            Settings::from_saved(gone),
            Settings { statistics: true, ..Settings::default() }
        );
        // Not settings at all.
        assert_eq!(Settings::from_saved("{ not json"), Settings::default());
    }
}
