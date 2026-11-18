//! What this device's keyboard and mouse mean to the PC being watched.
//!
//! No drawing and no networking: the stream screen feeds [`Controls`] what
//! the window reports and takes out the [`Input`]s to send, in order.

use egui::{MouseWheelUnit, PointerButton, Pos2, Rect, Vec2, vec2};
use leto_core::input::{Input, Key, Modifiers, MouseButton, WHEEL_NOTCH};

/// Points of smooth scrolling, on a touchpad, that count as one wheel notch.
const POINTS_PER_NOTCH: f32 = 50.0;
/// Wheel notches that scrolling by a page counts as.
const NOTCHES_PER_PAGE: f32 = 5.0;

/// What Leto keeps for itself while a stream has the keyboard. Each is its
/// letter pressed with Ctrl, Alt and Shift held, as in Moonlight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    /// Q: stop watching.
    Leave,
    /// X: fill the screen, or stop filling it.
    FullScreen,
    /// M: switch between the pointer and the game mouse.
    GameMouse,
}

impl Shortcut {
    /// The shortcut `key` stands for, where `key` is the letter the layout
    /// prints: people look for the Q they see.
    fn of(key: egui::Key) -> Option<Self> {
        match key {
            egui::Key::Q => Some(Self::Leave),
            egui::Key::X => Some(Self::FullScreen),
            egui::Key::M => Some(Self::GameMouse),
            _ => None,
        }
    }

    /// The keys to press, in words.
    pub const fn keys(self) -> &'static str {
        match self {
            Self::Leave => "Ctrl+Alt+Shift+Q",
            Self::FullScreen => "Ctrl+Alt+Shift+X",
            Self::GameMouse => "Ctrl+Alt+Shift+M",
        }
    }
}

/// The key of the PC's keyboard that sits where `key` sits on this one.
///
/// Keys are named by place, not by what the layout prints: the PC applies
/// its own layout, so a French keyboard types French on a French PC.
pub fn key(key: egui::Key) -> Option<Key> {
    use egui::Key as K;
    Some(match key {
        K::ArrowDown => Key::DOWN,
        K::ArrowLeft => Key::LEFT,
        K::ArrowRight => Key::RIGHT,
        K::ArrowUp => Key::UP,
        K::Escape => Key::ESCAPE,
        K::Tab => Key::TAB,
        K::Backspace => Key::BACKSPACE,
        K::Enter => Key::ENTER,
        K::Space => Key::SPACE,
        K::Insert => Key::INSERT,
        K::Delete => Key::DELETE,
        K::Home => Key::HOME,
        K::End => Key::END,
        K::PageUp => Key::PAGE_UP,
        K::PageDown => Key::PAGE_DOWN,

        // Punctuation. A sign reached with Shift is the key it is printed on.
        K::Comma => Key::COMMA,
        K::Period => Key::PERIOD,
        K::Semicolon | K::Colon => Key::SEMICOLON,
        K::Backslash | K::Pipe => Key::BACKSLASH,
        K::Slash | K::Questionmark => Key::SLASH,
        K::OpenBracket | K::OpenCurlyBracket => Key::OPEN_BRACKET,
        K::CloseBracket | K::CloseCurlyBracket => Key::CLOSE_BRACKET,
        K::Backtick => Key::BACKTICK,
        K::Quote => Key::QUOTE,
        K::Minus => Key::MINUS,
        K::Equals => Key::EQUALS,
        // By place, this is only ever the plus of the numeric keypad.
        K::Plus => Key::KEYPAD_PLUS,
        K::Exclamationmark => return Key::digit(1),
        K::IntlBackslash => Key::EXTRA,

        K::Num0 => return Key::digit(0),
        K::Num1 => return Key::digit(1),
        K::Num2 => return Key::digit(2),
        K::Num3 => return Key::digit(3),
        K::Num4 => return Key::digit(4),
        K::Num5 => return Key::digit(5),
        K::Num6 => return Key::digit(6),
        K::Num7 => return Key::digit(7),
        K::Num8 => return Key::digit(8),
        K::Num9 => return Key::digit(9),

        K::ShiftLeft => Key::LEFT_SHIFT,
        K::ShiftRight => Key::RIGHT_SHIFT,
        K::ControlLeft => Key::LEFT_CTRL,
        K::ControlRight => Key::RIGHT_CTRL,
        K::AltLeft => Key::LEFT_ALT,
        K::AltRight => Key::RIGHT_ALT,
        K::SuperLeft => Key::LEFT_SUPER,
        K::SuperRight => Key::RIGHT_SUPER,

        K::CapsLock => Key::CAPS_LOCK,
        K::NumLock => Key::NUM_LOCK,
        K::ScrollLock => Key::SCROLL_LOCK,
        K::PrintScreen => Key::PRINT_SCREEN,
        K::Pause => Key::PAUSE,
        K::Menu => Key::MENU,
        K::Help => Key::HELP,
        K::Sleep => Key::SLEEP,
        K::Clear => Key::CLEAR,

        K::BrowserBack => Key::BROWSER_BACK,
        K::BrowserForward => Key::BROWSER_FORWARD,
        K::BrowserRefresh => Key::BROWSER_REFRESH,
        K::BrowserStop => Key::BROWSER_STOP,
        K::BrowserSearch => Key::BROWSER_SEARCH,
        K::BrowserFavorites => Key::BROWSER_FAVORITES,
        K::BrowserHome => Key::BROWSER_HOME,
        K::AudioVolumeMute => Key::VOLUME_MUTE,
        K::AudioVolumeDown => Key::VOLUME_DOWN,
        K::AudioVolumeUp => Key::VOLUME_UP,
        K::MediaTrackNext => Key::MEDIA_NEXT,
        K::MediaTrackPrevious => Key::MEDIA_PREVIOUS,
        K::MediaStop => Key::MEDIA_STOP,
        K::MediaPlayPause => Key::MEDIA_PLAY_PAUSE,
        K::LaunchMail => Key::LAUNCH_MAIL,
        K::LaunchApp1 => Key::LAUNCH_APP_1,
        K::LaunchApp2 => Key::LAUNCH_APP_2,

        other => {
            let name = other.name();
            let mut letters = name.chars();
            return match (letters.next(), letters.as_str()) {
                (Some(letter), "") => Key::letter(letter),
                // F1 to F24. Windows has no key beyond those.
                (Some('F'), number) => number.parse().ok().and_then(Key::function),
                _ => None,
            };
        }
    })
}

const fn button(button: PointerButton) -> MouseButton {
    match button {
        PointerButton::Primary => MouseButton::Left,
        PointerButton::Secondary => MouseButton::Right,
        PointerButton::Middle => MouseButton::Middle,
        PointerButton::Extra1 => MouseButton::Back,
        PointerButton::Extra2 => MouseButton::Forward,
    }
}

/// Follows what is held down, and queues what the PC should hear.
#[derive(Debug, Default)]
pub struct Controls {
    /// Keys and buttons the PC believes are down, oldest first.
    keys: Vec<Key>,
    buttons: Vec<MouseButton>,
    /// Scrolling and movement too small to send yet.
    wheel: Vec2,
    motion: Vec2,
    queue: Vec<Input>,
}

impl Controls {
    /// A key of this keyboard went down or came up. `printed` is the key by
    /// what the layout prints on it, `place` the key by where it sits.
    ///
    /// Returns the shortcut the press completes, which the PC does not hear
    /// about. A key that is held repeats nothing: the PC repeats on its own.
    pub fn key(
        &mut self,
        printed: egui::Key,
        place: Option<egui::Key>,
        down: bool,
    ) -> Option<Shortcut> {
        let held = self.modifiers();
        if down
            && held.ctrl
            && held.alt
            && held.shift
            && let Some(shortcut) = Shortcut::of(printed)
        {
            return Some(shortcut);
        }
        let key = key(place.unwrap_or(printed))?;
        // Nothing new: a repeat, or the release of a key the PC never saw
        // go down, such as the letter of a shortcut.
        if down == self.keys.contains(&key) {
            return None;
        }
        if down {
            self.keys.push(key);
        } else {
            self.keys.retain(|held| *held != key);
        }
        self.queue.push(Input::Key { key, down, modifiers: self.modifiers() });
        None
    }

    /// A mouse button went down or came up.
    pub fn button(&mut self, pressed: PointerButton, down: bool) {
        let button = button(pressed);
        if down == self.buttons.contains(&button) {
            return;
        }
        if down {
            self.buttons.push(button);
        } else {
            self.buttons.retain(|held| *held != button);
        }
        self.queue.push(Input::MouseButton { button, down });
    }

    /// The pointer is at `pointer`, over a picture of `size` pixels drawn at
    /// `place`. Outside the picture counts as its nearest edge.
    pub fn pointer_at(&mut self, pointer: Pos2, place: Rect, size: [usize; 2]) {
        let (Ok(width), Ok(height)) = (i16::try_from(size[0]), i16::try_from(size[1])) else {
            return;
        };
        if width <= 0 || height <= 0 || place.width() <= 0.0 || place.height() <= 0.0 {
            return;
        }
        let across = ((pointer.x - place.left()) / place.width()).clamp(0.0, 1.0);
        let down = ((pointer.y - place.top()) / place.height()).clamp(0.0, 1.0);
        self.push(Input::MousePosition {
            x: (across * f32::from(width)).round() as i16,
            y: (down * f32::from(height)).round() as i16,
            width,
            height,
        });
    }

    /// The mouse itself moved by `delta`, wherever the pointer is. For the
    /// game mouse.
    pub fn moved(&mut self, delta: Vec2) {
        self.motion += delta;
        let [dx, dy] = whole(&mut self.motion);
        if dx != 0 || dy != 0 {
            self.push(Input::MouseMove { dx, dy });
        }
    }

    /// The wheel turned, as egui reports it: positive moves what is on
    /// screen to the right and down.
    pub fn wheel(&mut self, unit: MouseWheelUnit, delta: Vec2) {
        let notches = match unit {
            MouseWheelUnit::Line => delta,
            MouseWheelUnit::Point => delta / POINTS_PER_NOTCH,
            MouseWheelUnit::Page => delta * NOTCHES_PER_PAGE,
        };
        // What is on screen moving down is the wheel turning up, and what is
        // on screen moving right is the wheel tilting left.
        self.wheel += vec2(-notches.x, notches.y) * f32::from(WHEEL_NOTCH);
        let [horizontal, vertical] = whole(&mut self.wheel);
        if horizontal != 0 || vertical != 0 {
            self.push(Input::Scroll { horizontal, vertical });
        }
    }

    /// Lets go of everything held, last pressed first. For when the window
    /// loses the keyboard, and before leaving: nothing may stay stuck down
    /// on the PC.
    pub fn release_all(&mut self) {
        while let Some(button) = self.buttons.pop() {
            self.queue.push(Input::MouseButton { button, down: false });
        }
        while let Some(key) = self.keys.pop() {
            self.queue.push(Input::Key { key, down: false, modifiers: self.modifiers() });
        }
        self.wheel = Vec2::ZERO;
        self.motion = Vec2::ZERO;
    }

    /// Whether a key or a button is held down.
    pub fn holds_anything(&self) -> bool {
        !self.keys.is_empty() || !self.buttons.is_empty()
    }

    /// What to send to the PC since the last call, in order.
    pub fn take(&mut self) -> Vec<Input> {
        std::mem::take(&mut self.queue)
    }

    fn modifiers(&self) -> Modifiers {
        let held = |left, right| self.keys.contains(&left) || self.keys.contains(&right);
        Modifiers {
            shift: held(Key::LEFT_SHIFT, Key::RIGHT_SHIFT),
            ctrl: held(Key::LEFT_CTRL, Key::RIGHT_CTRL),
            alt: held(Key::LEFT_ALT, Key::RIGHT_ALT),
            meta: held(Key::LEFT_SUPER, Key::RIGHT_SUPER),
        }
    }

    /// Queues `input`. Movement that follows movement is merged into it: the
    /// window reports far more of it than a PC needs. Anything else keeps
    /// its place, so a click still lands where the pointer was.
    fn push(&mut self, input: Input) {
        match (self.queue.last_mut(), input) {
            (Some(last @ Input::MousePosition { .. }), Input::MousePosition { .. }) => {
                *last = input;
            }
            (Some(Input::MouseMove { dx, dy }), Input::MouseMove { dx: more_x, dy: more_y }) => {
                match (dx.checked_add(more_x), dy.checked_add(more_y)) {
                    (Some(x), Some(y)) => (*dx, *dy) = (x, y),
                    _ => self.queue.push(input),
                }
            }
            (
                Some(Input::Scroll { horizontal, vertical }),
                Input::Scroll { horizontal: more_x, vertical: more_y },
            ) => match (horizontal.checked_add(more_x), vertical.checked_add(more_y)) {
                (Some(x), Some(y)) => (*horizontal, *vertical) = (x, y),
                _ => self.queue.push(input),
            },
            _ => self.queue.push(input),
        }
    }
}

/// Takes the whole part out of `amount` and leaves the rest for next time.
fn whole(amount: &mut Vec2) -> [i16; 2] {
    let limit = f32::from(i16::MAX);
    let taken = vec2(amount.x.trunc().clamp(-limit, limit), amount.y.trunc().clamp(-limit, limit));
    *amount -= taken;
    [taken.x as i16, taken.y as i16]
}

#[cfg(test)]
mod tests {
    use egui::pos2;

    use super::*;

    const NO_MODIFIERS: Modifiers =
        Modifiers { shift: false, ctrl: false, alt: false, meta: false };

    fn letter(letter: char) -> Key {
        Key::letter(letter).expect("a letter is a key")
    }

    #[test]
    fn a_key_is_named_by_where_it_sits_not_by_what_it_prints() {
        // The key next to Tab prints A on a French keyboard and Q on an
        // American one. The PC hears about the place and applies its layout.
        let mut controls = Controls::default();
        controls.key(egui::Key::A, Some(egui::Key::Q), true);
        assert_eq!(
            controls.take(),
            [Input::Key { key: letter('Q'), down: true, modifiers: NO_MODIFIERS }]
        );

        // Where the place is unknown, what the key prints is all there is.
        controls.key(egui::Key::Period, None, true);
        assert_eq!(
            controls.take(),
            [Input::Key { key: Key::PERIOD, down: true, modifiers: NO_MODIFIERS }]
        );
    }

    #[test]
    fn every_ordinary_key_has_a_place_on_the_pcs_keyboard() {
        let unknown: Vec<_> =
            egui::Key::ALL.iter().copied().filter(|known| key(*known).is_none()).collect();
        // Clipboard keys of some keyboards, keys Windows has no code for,
        // and function keys beyond F24.
        let expected = [
            egui::Key::Copy,
            egui::Key::Cut,
            egui::Key::Paste,
            egui::Key::F25,
            egui::Key::F26,
            egui::Key::F27,
            egui::Key::F28,
            egui::Key::F29,
            egui::Key::F30,
            egui::Key::F31,
            egui::Key::F32,
            egui::Key::F33,
            egui::Key::F34,
            egui::Key::F35,
            egui::Key::Fn,
            egui::Key::Eject,
            egui::Key::Power,
        ];
        assert_eq!(unknown, expected);

        assert_eq!(key(egui::Key::Z), Some(letter('Z')));
        assert_eq!(key(egui::Key::Num7), Key::digit(7));
        assert_eq!(key(egui::Key::F1), Key::function(1));
        assert_eq!(key(egui::Key::F24), Key::function(24));
        assert_eq!(key(egui::Key::IntlBackslash), Some(Key::EXTRA));
    }

    #[test]
    fn a_held_key_is_sent_once_and_modifiers_follow_what_is_held() {
        let mut controls = Controls::default();
        controls.key(egui::Key::ShiftLeft, Some(egui::Key::ShiftLeft), true);
        for _ in 0..3 {
            controls.key(egui::Key::A, Some(egui::Key::A), true);
        }
        controls.key(egui::Key::A, Some(egui::Key::A), false);
        controls.key(egui::Key::ShiftLeft, Some(egui::Key::ShiftLeft), false);
        // A release nobody pressed for says nothing.
        controls.key(egui::Key::B, Some(egui::Key::B), false);

        let shift = Modifiers { shift: true, ..NO_MODIFIERS };
        assert_eq!(
            controls.take(),
            [
                Input::Key { key: Key::LEFT_SHIFT, down: true, modifiers: shift },
                Input::Key { key: letter('A'), down: true, modifiers: shift },
                Input::Key { key: letter('A'), down: false, modifiers: shift },
                Input::Key { key: Key::LEFT_SHIFT, down: false, modifiers: NO_MODIFIERS },
            ]
        );
        assert!(!controls.holds_anything());
    }

    #[test]
    fn the_shortcuts_stay_on_this_device() {
        let mut controls = Controls::default();
        for modifier in [egui::Key::ControlLeft, egui::Key::AltLeft, egui::Key::ShiftRight] {
            assert_eq!(controls.key(modifier, Some(modifier), true), None);
        }
        controls.take();

        // On a French keyboard the Q people see sits where A does elsewhere.
        assert_eq!(controls.key(egui::Key::Q, Some(egui::Key::A), true), Some(Shortcut::Leave));
        assert_eq!(controls.key(egui::Key::Q, Some(egui::Key::A), false), None);
        assert_eq!(controls.key(egui::Key::X, None, true), Some(Shortcut::FullScreen));
        assert_eq!(controls.key(egui::Key::M, None, true), Some(Shortcut::GameMouse));
        assert_eq!(controls.take(), [], "the PC hears nothing of a shortcut's letter");

        // Any other letter goes to the PC, with the three modifiers.
        assert_eq!(controls.key(egui::Key::S, None, true), None);
        let all = Modifiers { shift: true, ctrl: true, alt: true, meta: false };
        assert_eq!(controls.take(), [Input::Key { key: letter('S'), down: true, modifiers: all }]);

        // Without all three, Q is a letter like any other.
        let mut controls = Controls::default();
        controls.key(egui::Key::ControlLeft, None, true);
        assert_eq!(controls.key(egui::Key::Q, None, true), None);
        assert_eq!(controls.take().len(), 2);
    }

    #[test]
    fn letting_go_of_everything_releases_the_last_pressed_first() {
        let mut controls = Controls::default();
        controls.key(egui::Key::ControlLeft, None, true);
        controls.key(egui::Key::C, None, true);
        controls.button(PointerButton::Primary, true);
        controls.take();

        controls.release_all();
        assert_eq!(
            controls.take(),
            [
                Input::MouseButton { button: MouseButton::Left, down: false },
                Input::Key {
                    key: letter('C'),
                    down: false,
                    modifiers: Modifiers { ctrl: true, ..NO_MODIFIERS },
                },
                Input::Key { key: Key::LEFT_CTRL, down: false, modifiers: NO_MODIFIERS },
            ]
        );
        assert!(!controls.holds_anything());
        controls.release_all();
        assert_eq!(controls.take(), []);
    }

    #[test]
    fn the_pointer_is_placed_on_the_picture_not_on_the_window() {
        // A 1920 × 1080 picture drawn at half size with black above it.
        let place = Rect::from_min_size(pos2(0.0, 100.0), vec2(960.0, 540.0));
        let size = [1920, 1080];
        let at = |pointer| {
            let mut controls = Controls::default();
            controls.pointer_at(pointer, place, size);
            match controls.take()[..] {
                [Input::MousePosition { x, y, width: 1920, height: 1080 }] => (x, y),
                ref other => panic!("{other:?}"),
            }
        };
        assert_eq!(at(pos2(480.0, 370.0)), (960, 540));
        assert_eq!(at(pos2(0.0, 100.0)), (0, 0));
        assert_eq!(at(pos2(960.0, 640.0)), (1920, 1080));
        // In the black around the picture: its nearest edge.
        assert_eq!(at(pos2(240.0, 20.0)), (480, 0));
        assert_eq!(at(pos2(-50.0, 900.0)), (0, 1080));

        // No picture, no position.
        let mut controls = Controls::default();
        controls.pointer_at(pos2(10.0, 10.0), place, [0, 0]);
        controls.pointer_at(pos2(10.0, 10.0), Rect::ZERO, size);
        assert_eq!(controls.take(), []);
    }

    #[test]
    fn movement_is_merged_but_a_click_keeps_its_place() {
        let place = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 100.0));
        let mut controls = Controls::default();
        for x in [10.0, 20.0, 30.0] {
            controls.pointer_at(pos2(x, 50.0), place, [100, 100]);
        }
        controls.button(PointerButton::Primary, true);
        controls.pointer_at(pos2(40.0, 50.0), place, [100, 100]);
        controls.pointer_at(pos2(60.0, 50.0), place, [100, 100]);
        controls.button(PointerButton::Primary, false);

        let position = |x| Input::MousePosition { x, y: 50, width: 100, height: 100 };
        assert_eq!(
            controls.take(),
            [
                position(30),
                Input::MouseButton { button: MouseButton::Left, down: true },
                position(60),
                Input::MouseButton { button: MouseButton::Left, down: false },
            ]
        );
    }

    #[test]
    fn the_game_mouse_loses_no_movement() {
        let mut controls = Controls::default();
        // Four moves of a third of a unit: one unit, and a third kept.
        for _ in 0..4 {
            controls.moved(vec2(0.34, -0.34));
        }
        assert_eq!(controls.take(), [Input::MouseMove { dx: 1, dy: -1 }]);

        let mut controls = Controls::default();
        controls.moved(vec2(5.0, 0.0));
        controls.moved(vec2(7.0, 2.0));
        controls.button(PointerButton::Secondary, true);
        controls.moved(vec2(-3.0, 0.0));
        assert_eq!(
            controls.take(),
            [
                Input::MouseMove { dx: 12, dy: 2 },
                Input::MouseButton { button: MouseButton::Right, down: true },
                Input::MouseMove { dx: -3, dy: 0 },
            ]
        );
    }

    #[test]
    fn the_wheel_turns_the_way_the_pc_expects() {
        let mut controls = Controls::default();
        // One notch away from the person: egui moves the page down.
        controls.wheel(MouseWheelUnit::Line, vec2(0.0, 1.0));
        assert_eq!(controls.take(), [Input::Scroll { horizontal: 0, vertical: 120 }]);
        controls.wheel(MouseWheelUnit::Line, vec2(0.0, -2.0));
        assert_eq!(controls.take(), [Input::Scroll { horizontal: 0, vertical: -240 }]);
        // A tilt to the right moves the page to the left.
        controls.wheel(MouseWheelUnit::Line, vec2(-1.0, 0.0));
        assert_eq!(controls.take(), [Input::Scroll { horizontal: 120, vertical: 0 }]);

        // A touchpad scrolls by points: fifty of them make a notch, and
        // every little move counts.
        for _ in 0..10 {
            controls.wheel(MouseWheelUnit::Point, vec2(0.0, 5.0));
        }
        assert_eq!(controls.take(), [Input::Scroll { horizontal: 0, vertical: 120 }]);
    }
}
