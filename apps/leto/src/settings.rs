//! What the person chose for Leto, kept from one start to the next.
//!
//! Every choice has a default, which is how Leto behaves for someone who
//! never opens the settings. A choice missing from what was saved, as after
//! an update that adds one, is its default; a saved file that cannot be
//! read is all defaults.

use leto_core::engine::{Choices, ScreenMode, bitrate_for};
use serde::{Deserialize, Serialize};

/// The picture sizes offered besides the screen's own, as in Moonlight.
pub const RESOLUTIONS: [[u32; 2]; 4] = [[1280, 720], [1920, 1080], [2560, 1440], [3840, 2160]];
/// The paces offered besides the screen's own, in pictures per second.
pub const FRAME_RATES: [u32; 7] = [30, 60, 90, 120, 144, 165, 240];

/// A screen as a PC makes one when the window does not know its own.
const PLAIN: ScreenMode =
    ScreenMode { width: 1920, height: 1080, refresh_hz: 60, scale_percent: 100 };

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    // The picture.
    /// Width and height of the stream. `None`: those of this screen.
    pub resolution: Option<[u32; 2]>,
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
    pub swap_mouse_buttons: bool,
    /// The wheel scrolls the other way on the PC.
    pub reverse_scroll: bool,
    /// A and B change places, and X and Y, for gamepads labelled the other
    /// way round.
    pub swap_gamepad_buttons: bool,

    // Advanced.
    /// The statistics show from the start of every stream.
    pub statistics: bool,
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
            swap_mouse_buttons: false,
            reverse_scroll: false,
            swap_gamepad_buttons: false,
            statistics: false,
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
    /// The screen a stream is made for, from `native`, this screen when the
    /// window knows it, and what the PC is told besides.
    pub fn stream(&self, native: Option<ScreenMode>) -> (Option<ScreenMode>, Choices) {
        let choices = Choices {
            bitrate_kbps: self.bitrate_kbps,
            virtual_display: self.virtual_display,
            optimize_games: self.optimize_games,
            sound_on_pc: !self.mute_pc,
        };
        (self.screen(native), choices)
    }

    /// The screen a stream is made for. `None` when there is nothing to go
    /// by: the stream is then the engine's default.
    pub fn screen(&self, native: Option<ScreenMode>) -> Option<ScreenMode> {
        if self.resolution.is_none() && self.frame_rate.is_none() {
            return native;
        }
        // What is not chosen is the screen's, or what a PC makes.
        let base = native.unwrap_or(PLAIN);
        let [width, height] = self.resolution.unwrap_or([base.width, base.height]);
        Some(ScreenMode {
            width,
            height,
            refresh_hz: self.frame_rate.unwrap_or(base.refresh_hz),
            scale_percent: base.scale_percent,
        })
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

    pub fn to_saved(self) -> String {
        serde_json::to_string_pretty(&self).unwrap_or_default()
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

    const NATIVE: ScreenMode =
        ScreenMode { width: 2560, height: 1600, refresh_hz: 120, scale_percent: 150 };

    #[test]
    fn by_default_a_stream_is_just_like_this_screen() {
        let (screen, choices) = Settings::default().stream(Some(NATIVE));
        assert_eq!(screen, Some(NATIVE));
        assert_eq!(choices, Choices::default());
        assert_eq!(Settings::default().stream(None), (None, Choices::default()));
    }

    #[test]
    fn a_chosen_size_or_pace_keeps_the_rest_of_the_screen() {
        let sized = Settings { resolution: Some([1920, 1080]), ..Settings::default() };
        assert_eq!(
            sized.screen(Some(NATIVE)),
            Some(ScreenMode { width: 1920, height: 1080, refresh_hz: 120, scale_percent: 150 })
        );
        let paced = Settings { frame_rate: Some(60), ..Settings::default() };
        assert_eq!(
            paced.screen(Some(NATIVE)),
            Some(ScreenMode { width: 2560, height: 1600, refresh_hz: 60, scale_percent: 150 })
        );
        // A window that does not know its screen goes by Full HD at 60.
        assert_eq!(paced.screen(None), Some(ScreenMode { refresh_hz: 60, ..PLAIN }));
        let both = Settings { resolution: Some([3840, 2160]), ..paced };
        assert_eq!(
            both.screen(None),
            Some(ScreenMode { width: 3840, height: 2160, refresh_hz: 60, scale_percent: 100 })
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
        let (_, choices) = chosen.stream(None);
        assert_eq!(
            choices,
            Choices {
                bitrate_kbps: Some(40_000),
                virtual_display: false,
                optimize_games: false,
                sound_on_pc: true,
            }
        );
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
    fn saved_settings_read_back_and_what_is_missing_is_its_default() {
        let chosen = Settings {
            resolution: Some([2560, 1440]),
            bitrate_kbps: Some(50_000),
            capture_shortcuts: Capture::FullScreen,
            swap_gamepad_buttons: true,
            ..Settings::default()
        };
        assert_eq!(Settings::from_saved(&chosen.to_saved()), chosen);
        // Saved by a Leto that knew fewer choices.
        let older = Settings::from_saved(r#"{"frame_rate":60,"statistics":true}"#);
        assert_eq!(
            older,
            Settings { frame_rate: Some(60), statistics: true, ..Settings::default() }
        );
        // Not settings at all.
        assert_eq!(Settings::from_saved("{ not json"), Settings::default());
    }
}
