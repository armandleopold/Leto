//! What a person does with a keyboard, a mouse or a gamepad, in the terms a
//! PC understands.
//!
//! Every shell collects these its own way (a window, a browser page, a touch
//! screen) and every way to a PC carries them, so they are described once,
//! here, without any platform code.

use serde::{Deserialize, Serialize};

/// A key, named by its Windows virtual-key code: what the Moonlight protocol
/// carries.
///
/// The code names the place of the key on a US keyboard, and the PC applies
/// its own layout to it. The key right of Tab is [`Key::letter`]`('Q')` on
/// every keyboard, and types "a" on a PC set up for French.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Key(pub u16);

impl Key {
    pub const BACKSPACE: Self = Self(0x08);
    pub const TAB: Self = Self(0x09);
    pub const CLEAR: Self = Self(0x0C);
    pub const ENTER: Self = Self(0x0D);
    pub const PAUSE: Self = Self(0x13);
    pub const CAPS_LOCK: Self = Self(0x14);
    pub const ESCAPE: Self = Self(0x1B);
    pub const SPACE: Self = Self(0x20);
    pub const PAGE_UP: Self = Self(0x21);
    pub const PAGE_DOWN: Self = Self(0x22);
    pub const END: Self = Self(0x23);
    pub const HOME: Self = Self(0x24);
    pub const LEFT: Self = Self(0x25);
    pub const UP: Self = Self(0x26);
    pub const RIGHT: Self = Self(0x27);
    pub const DOWN: Self = Self(0x28);
    pub const PRINT_SCREEN: Self = Self(0x2C);
    pub const INSERT: Self = Self(0x2D);
    pub const DELETE: Self = Self(0x2E);
    pub const HELP: Self = Self(0x2F);
    pub const LEFT_SUPER: Self = Self(0x5B);
    pub const RIGHT_SUPER: Self = Self(0x5C);
    /// The key that opens the context menu.
    pub const MENU: Self = Self(0x5D);
    pub const SLEEP: Self = Self(0x5F);
    pub const NUM_LOCK: Self = Self(0x90);
    pub const SCROLL_LOCK: Self = Self(0x91);
    pub const LEFT_SHIFT: Self = Self(0xA0);
    pub const RIGHT_SHIFT: Self = Self(0xA1);
    pub const LEFT_CTRL: Self = Self(0xA2);
    pub const RIGHT_CTRL: Self = Self(0xA3);
    pub const LEFT_ALT: Self = Self(0xA4);
    pub const RIGHT_ALT: Self = Self(0xA5);
    pub const BROWSER_BACK: Self = Self(0xA6);
    pub const BROWSER_FORWARD: Self = Self(0xA7);
    pub const BROWSER_REFRESH: Self = Self(0xA8);
    pub const BROWSER_STOP: Self = Self(0xA9);
    pub const BROWSER_SEARCH: Self = Self(0xAA);
    pub const BROWSER_FAVORITES: Self = Self(0xAB);
    pub const BROWSER_HOME: Self = Self(0xAC);
    pub const VOLUME_MUTE: Self = Self(0xAD);
    pub const VOLUME_DOWN: Self = Self(0xAE);
    pub const VOLUME_UP: Self = Self(0xAF);
    pub const MEDIA_NEXT: Self = Self(0xB0);
    pub const MEDIA_PREVIOUS: Self = Self(0xB1);
    pub const MEDIA_STOP: Self = Self(0xB2);
    pub const MEDIA_PLAY_PAUSE: Self = Self(0xB3);
    pub const LAUNCH_MAIL: Self = Self(0xB4);
    pub const LAUNCH_APP_1: Self = Self(0xB6);
    pub const LAUNCH_APP_2: Self = Self(0xB7);

    // The keys named after what they type on a US keyboard.
    pub const SEMICOLON: Self = Self(0xBA);
    pub const EQUALS: Self = Self(0xBB);
    pub const COMMA: Self = Self(0xBC);
    pub const MINUS: Self = Self(0xBD);
    pub const PERIOD: Self = Self(0xBE);
    pub const SLASH: Self = Self(0xBF);
    pub const BACKTICK: Self = Self(0xC0);
    pub const OPEN_BRACKET: Self = Self(0xDB);
    pub const BACKSLASH: Self = Self(0xDC);
    pub const CLOSE_BRACKET: Self = Self(0xDD);
    pub const QUOTE: Self = Self(0xDE);
    /// The key between left Shift and the first letter, on keyboards that
    /// have one: `<` on a French keyboard, `\` on a British one.
    pub const EXTRA: Self = Self(0xE2);

    /// The key of a letter of the US keyboard, in either case.
    pub const fn letter(letter: char) -> Option<Self> {
        match letter {
            'A'..='Z' => Some(Self(letter as u16)),
            'a'..='z' => Some(Self(letter as u16 - 0x20)),
            _ => None,
        }
    }

    /// The key of a digit in the row above the letters.
    pub const fn digit(digit: u8) -> Option<Self> {
        if digit <= 9 { Some(Self(0x30 + digit as u16)) } else { None }
    }

    /// F1 to F24.
    pub const fn function(number: u8) -> Option<Self> {
        if number >= 1 && number <= 24 { Some(Self(0x6F + number as u16)) } else { None }
    }
}

/// Which of the keys that change the others are held.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// The Windows key, or Command on a Mac.
    pub meta: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    /// The thumb buttons of a mouse that has them.
    Back,
    Forward,
}

/// What a gamepad looks like, so the PC can pretend to have the same.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GamepadKind {
    #[default]
    Unknown,
    Xbox,
    PlayStation,
    Nintendo,
}

/// The buttons of a gamepad that are held, named as on an Xbox gamepad.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GamepadButtons(u32);

impl GamepadButtons {
    pub const NONE: Self = Self(0);
    pub const A: Self = Self(1);
    pub const B: Self = Self(1 << 1);
    pub const X: Self = Self(1 << 2);
    pub const Y: Self = Self(1 << 3);
    pub const UP: Self = Self(1 << 4);
    pub const DOWN: Self = Self(1 << 5);
    pub const LEFT: Self = Self(1 << 6);
    pub const RIGHT: Self = Self(1 << 7);
    pub const LEFT_SHOULDER: Self = Self(1 << 8);
    pub const RIGHT_SHOULDER: Self = Self(1 << 9);
    pub const START: Self = Self(1 << 10);
    /// "Select", "View" or "Share", left of the middle.
    pub const BACK: Self = Self(1 << 11);
    /// Pressing the left stick.
    pub const LEFT_STICK: Self = Self(1 << 12);
    pub const RIGHT_STICK: Self = Self(1 << 13);
    /// The button with the maker's logo.
    pub const GUIDE: Self = Self(1 << 14);

    /// Every button there is, one by one.
    pub const EACH: [Self; 15] = [
        Self::A,
        Self::B,
        Self::X,
        Self::Y,
        Self::UP,
        Self::DOWN,
        Self::LEFT,
        Self::RIGHT,
        Self::LEFT_SHOULDER,
        Self::RIGHT_SHOULDER,
        Self::START,
        Self::BACK,
        Self::LEFT_STICK,
        Self::RIGHT_STICK,
        Self::GUIDE,
    ];

    /// These buttons and `other`.
    #[must_use]
    pub const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Presses or releases `button`.
    pub const fn set(&mut self, button: Self, held: bool) {
        if held {
            self.0 |= button.0;
        } else {
            self.0 &= !button.0;
        }
    }

    /// Whether all of `buttons` are held.
    pub const fn contains(self, buttons: Self) -> bool {
        self.0 & buttons.0 == buttons.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// Everything a gamepad reports at one moment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct GamepadState {
    pub buttons: GamepadButtons,
    /// From 0, released, to 1, fully pressed.
    pub left_trigger: f32,
    pub right_trigger: f32,
    /// From -1 to 1: first to the right, then up.
    pub left_stick: [f32; 2],
    pub right_stick: [f32; 2],
}

/// One thing a person did, to pass on to the PC.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Input {
    /// A key went down or came up. `modifiers` are those held at that moment.
    Key {
        key: Key,
        down: bool,
        modifiers: Modifiers,
    },
    /// The mouse moved by this much, to the right and down. For games that
    /// turn the view with the mouse.
    MouseMove {
        dx: i16,
        dy: i16,
    },
    /// The pointer is at (`x`, `y`) of a picture `width` by `height`.
    MousePosition {
        x: i16,
        y: i16,
        width: i16,
        height: i16,
    },
    MouseButton {
        button: MouseButton,
        down: bool,
    },
    /// The wheel turned: 120 for one notch, positive up and to the right.
    Scroll {
        horizontal: i16,
        vertical: i16,
    },
    /// A gamepad was plugged in. Gamepads are numbered from 0, and a PC
    /// accepts up to sixteen.
    GamepadArrived {
        number: u8,
        kind: GamepadKind,
    },
    Gamepad {
        number: u8,
        state: GamepadState,
    },
    GamepadLeft {
        number: u8,
    },
}

/// One notch of a mouse wheel, in the unit of [`Input::Scroll`].
pub const WHEEL_NOTCH: i16 = 120;

/// The most gamepads a PC accepts from one device.
pub const MAX_GAMEPADS: u8 = 16;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_carry_the_codes_windows_gives_them() {
        assert_eq!(Key::letter('A'), Some(Key(0x41)));
        assert_eq!(Key::letter('z'), Some(Key(0x5A)));
        assert_eq!(Key::letter('é'), None);
        assert_eq!(Key::digit(0), Some(Key(0x30)));
        assert_eq!(Key::digit(9), Some(Key(0x39)));
        assert_eq!(Key::digit(10), None);
        assert_eq!(Key::function(1), Some(Key(0x70)));
        assert_eq!(Key::function(12), Some(Key(0x7B)));
        assert_eq!(Key::function(24), Some(Key(0x87)));
        assert_eq!(Key::function(0), None);
        assert_eq!(Key::function(25), None);
    }

    #[test]
    fn gamepad_buttons_are_pressed_and_released_one_by_one() {
        let mut buttons = GamepadButtons::NONE;
        assert!(buttons.is_empty());

        buttons.set(GamepadButtons::A, true);
        buttons.set(GamepadButtons::START, true);
        assert!(buttons.contains(GamepadButtons::A.with(GamepadButtons::START)));
        assert!(!buttons.contains(GamepadButtons::B));

        buttons.set(GamepadButtons::A, false);
        assert_eq!(buttons, GamepadButtons::START);
    }

    #[test]
    fn every_gamepad_button_is_a_different_one() {
        let mut all = GamepadButtons::NONE;
        for button in GamepadButtons::EACH {
            assert!(!all.contains(button), "{button:?} is listed twice");
            all = all.with(button);
        }
    }
}
