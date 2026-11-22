//! What this device's keyboard and mouse mean to the PC being watched.
//!
//! No drawing and no networking: the stream screen feeds [`Controls`] what
//! the window reports and takes out the [`Input`]s to send, in order.

use egui::{MouseWheelUnit, PointerButton, Pos2, Rect, Vec2, vec2};
use leto_core::{
    input::{Input, Key, Modifiers, MouseButton, WHEEL_NOTCH},
    model::HostKind,
};

/// Points of smooth scrolling, on a touchpad, that count as one wheel notch.
const POINTS_PER_NOTCH: f32 = 50.0;
/// Wheel notches that scrolling by a page counts as.
const NOTCHES_PER_PAGE: f32 = 5.0;

/// Whether the PC's streaming software leaves its pointer out of the
/// pictures when asked, and puts it back when asked again: Sunshine and the
/// software made from it switch on Ctrl+Alt+Shift+N, the shortcut Moonlight
/// leaves for people to press. GameStream always draws it.
pub const fn hides_its_pointer(kind: HostKind) -> bool {
    match kind {
        HostKind::Apollo | HostKind::Sunshine | HostKind::FoundationSunshine => true,
        HostKind::GameStream => false,
    }
}

/// Whether this device has a pointer of its own to show on the pictures. A
/// phone has none: a finger puts the PC's pointer where it touches, so the
/// PC's own is the only sign of where the mouse is, and stays in the
/// pictures. A phone that asked it out also left the PC without one for
/// the next device whenever the app was closed while it streamed, since
/// nothing then asks the pointer back.
pub const HAS_POINTER: bool = !cfg!(target_os = "android");

/// How wide the band along each edge of the picture is, in points, where a
/// pointer counts as nearer the edge than it is: the half of it by the edge
/// is the edge itself. A finger cannot touch the last row of a phone's
/// screen, and the system keeps the edges of the screen for its own
/// gestures and bars, on a phone as on its desktop on a monitor: the PC's
/// taskbar that hides itself, and the buttons in the corner of a window
/// that fills its screen, were out of reach. Nothing where a mouse reaches
/// the edges of its window.
const EDGE_BAND: f32 = if cfg!(target_os = "android") { 24.0 } else { 0.0 };

/// Where a pointer at `at` along a picture `length` long counts as being,
/// with a `band` along each end: the outer half of a band is the end
/// itself, and the inner half stretches over the whole band, so that
/// nothing jumps. A picture too short for two bands is left as it is.
fn near_edge(at: f32, length: f32, band: f32) -> f32 {
    if band <= 0.0 || length < band * 4.0 {
        return at;
    }
    if at < band {
        (at * 2.0 - band).max(0.0)
    } else if at > length - band {
        length - ((length - at) * 2.0 - band).max(0.0)
    } else {
        at
    }
}

/// The letter of [`hides_its_pointer`]'s shortcut.
const POINTER_SWITCH: char = 'N';

/// That shortcut, in words. Leto passes it on when the person presses it:
/// the PC only switches, for every device at once, so what Leto believes of
/// the PC's pointer can be wrong, and this puts it right.
pub const PC_POINTER_KEYS: &str = "Ctrl+Alt+Shift+N";

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
    /// T, for talk: turn the microphone on, or off again.
    Microphone,
    /// C: turn the camera on, or off again.
    Camera,
    /// S: show how the stream is doing, or stop showing it.
    Statistics,
    /// Z: give the keyboard and the mouse back to this device, or take them
    /// again for the PC.
    Release,
    /// P: show this device's pointer on the pictures, or the PC's own.
    Pointer,
}

impl Shortcut {
    /// The shortcut `letter` stands for, where `letter` is what the layout
    /// prints: people look for the Q they see.
    fn of(letter: char) -> Option<Self> {
        match letter.to_ascii_uppercase() {
            'Q' => Some(Self::Leave),
            'X' => Some(Self::FullScreen),
            'M' => Some(Self::GameMouse),
            'T' => Some(Self::Microphone),
            'C' => Some(Self::Camera),
            'S' => Some(Self::Statistics),
            'Z' => Some(Self::Release),
            'P' => Some(Self::Pointer),
            _ => None,
        }
    }

    /// The keys to press, in words.
    pub const fn keys(self) -> &'static str {
        match self {
            Self::Leave => "Ctrl+Alt+Shift+Q",
            Self::FullScreen => "Ctrl+Alt+Shift+X",
            Self::GameMouse => "Ctrl+Alt+Shift+M",
            Self::Microphone => "Ctrl+Alt+Shift+T",
            Self::Camera => "Ctrl+Alt+Shift+C",
            Self::Statistics => "Ctrl+Alt+Shift+S",
            Self::Release => "Ctrl+Alt+Shift+Z",
            Self::Pointer => "Ctrl+Alt+Shift+P",
        }
    }
}

/// The letter the layout prints on `key`, when it is one.
fn letter(key: egui::Key) -> Option<char> {
    let mut name = key.name().chars();
    match (name.next(), name.next()) {
        (Some(letter), None) if letter.is_ascii_alphabetic() => Some(letter),
        _ => None,
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
    /// Keys and buttons held down, oldest first. The PC believes so too,
    /// unless the controls are muted.
    keys: Vec<Key>,
    buttons: Vec<MouseButton>,
    /// Scrolling and movement too small to send yet.
    wheel: Vec2,
    motion: Vec2,
    queue: Vec<Input>,
    /// The PC hears nothing: the keyboard and the mouse are this device's.
    /// What is held is still followed, for the shortcuts.
    muted: bool,
    /// The person chose to have the left and right buttons change places,
    /// and the wheel scroll the other way.
    swap_buttons: bool,
    reverse_scroll: bool,
}

impl Controls {
    /// A key of this keyboard went down or came up, as the window says.
    /// `printed` is the key by what the layout prints on it, `place` the key
    /// by where it sits. See [`Controls::stroke`].
    pub fn key(
        &mut self,
        printed: egui::Key,
        place: Option<egui::Key>,
        down: bool,
    ) -> Option<Shortcut> {
        // egui says what a key types, which Ctrl and Alt held together
        // change on a layout with AltGr: it then names the key by its place,
        // and the system tells what the key prints.
        let letter = match place {
            Some(place) if place == printed => {
                key(place).and_then(crate::window::printed_letter).or_else(|| letter(printed))
            }
            _ => letter(printed),
        };
        self.stroke(key(place.unwrap_or(printed)), letter, down)
    }

    /// A key went down or came up: `key` by where it sits, when the PC has
    /// a name for it, and the letter the layout prints on it, when it is
    /// one.
    ///
    /// Returns the shortcut the press completes, which the PC does not hear
    /// about. A key that is held repeats nothing: the PC repeats on its own.
    pub fn stroke(
        &mut self,
        key: Option<Key>,
        letter: Option<char>,
        down: bool,
    ) -> Option<Shortcut> {
        let held = self.modifiers();
        if down
            && held.ctrl
            && held.alt
            && held.shift
            && let Some(shortcut) = letter.and_then(Shortcut::of)
        {
            return Some(shortcut);
        }
        let key = key?;
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
        self.send(Input::Key { key, down, modifiers: self.modifiers() });
        None
    }

    /// Makes the left and right buttons change places on the PC, and the
    /// wheel scroll the other way, or not. For the next press and turn.
    pub fn choose(&mut self, swap_buttons: bool, reverse_scroll: bool) {
        self.swap_buttons = swap_buttons;
        self.reverse_scroll = reverse_scroll;
    }

    /// A mouse button went down or came up.
    pub fn button(&mut self, pressed: PointerButton, down: bool) {
        let button = match (button(pressed), self.swap_buttons) {
            (MouseButton::Left, true) => MouseButton::Right,
            (MouseButton::Right, true) => MouseButton::Left,
            (button, _) => button,
        };
        if down == self.buttons.contains(&button) {
            return;
        }
        if down {
            self.buttons.push(button);
        } else {
            self.buttons.retain(|held| *held != button);
        }
        self.send(Input::MouseButton { button, down });
    }

    /// Stops telling the PC anything, after letting go of what it holds, or
    /// starts again. A key let go of later is told even if the PC never
    /// heard of it going down: it lets go of what it does not hold without
    /// a word.
    pub fn mute(&mut self, mute: bool) {
        if mute && !self.muted {
            self.release_all();
        }
        self.muted = mute;
    }

    /// The pointer is at `pointer`, over a picture drawn at `place`, in a
    /// window that makes `scale` pixels of a point. Outside the picture
    /// counts as its nearest edge.
    ///
    /// The PC is told which pixel of the window the pointer is on, counted
    /// from the picture's corner, and the last pixel the picture covers
    /// here is the far edge of the PC's screen: the size told is one less
    /// than the pixels counted. A pointer cannot be further than on that
    /// pixel, and a PC told "1079 of 1080" stops one row short of the edge,
    /// where a taskbar that hides itself never comes out. Moonlight tells
    /// the same: its window's pixels (moonlight-qt), out of one less
    /// (moonlight-common-c, `inputSendThreadProc`).
    pub fn pointer_at(&mut self, pointer: Pos2, place: Rect, scale: f32) {
        // Two pixels at least: one would be both edges.
        let pixels = |points: f32| {
            i16::try_from((points * scale).round() as i32).ok().filter(|pixels| *pixels >= 2)
        };
        let (Some(width), Some(height)) = (pixels(place.width()), pixels(place.height())) else {
            return;
        };
        let (last_x, last_y) = (width - 1, height - 1);
        let on = |points: f32, last: i16| (points * scale).round().clamp(0.0, f32::from(last));
        let x = near_edge(pointer.x - place.left(), place.width(), EDGE_BAND);
        let y = near_edge(pointer.y - place.top(), place.height(), EDGE_BAND);
        self.push(Input::MousePosition {
            x: on(x, last_x) as i16,
            y: on(y, last_y) as i16,
            width: last_x,
            height: last_y,
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
        // on screen moving right is the wheel tilting left. Unless the person
        // would rather have it the other way.
        let way = if self.reverse_scroll { -1.0 } else { 1.0 };
        self.wheel += vec2(-notches.x, notches.y) * f32::from(WHEEL_NOTCH) * way;
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
            self.send(Input::MouseButton { button, down: false });
        }
        while let Some(key) = self.keys.pop() {
            self.send(Input::Key { key, down: false, modifiers: self.modifiers() });
        }
        self.wheel = Vec2::ZERO;
        self.motion = Vec2::ZERO;
    }

    /// Asks the PC to stop drawing its pointer in the pictures, or to start
    /// again: it switches. See [`hides_its_pointer`].
    pub fn switch_pc_pointer(&mut self) {
        if let Some(key) = Key::letter(POINTER_SWITCH) {
            self.pc_shortcut(key);
        }
    }

    /// Presses Ctrl+Alt+Shift and `key` on the PC and lets go again, for a
    /// shortcut of the PC's streaming software. What the person holds of
    /// Ctrl, Alt and Shift stays held. Told even while the controls are
    /// muted: Leto asks this, not the person.
    fn pc_shortcut(&mut self, key: Key) {
        // A muted PC holds nothing.
        // A muted PC holds nothing.
        let mut modifiers = if self.muted { Modifiers::default() } else { self.modifiers() };
        let all: [(Key, Flag); 3] = [
            (Key::LEFT_CTRL, |held| &mut held.ctrl),
            (Key::LEFT_ALT, |held| &mut held.alt),
            (Key::LEFT_SHIFT, |held| &mut held.shift),
        ];
        let mut pressed = Vec::new();
        for (modifier, flag) in all {
            if !*flag(&mut modifiers) {
                *flag(&mut modifiers) = true;
                self.queue.push(Input::Key { key: modifier, down: true, modifiers });
                pressed.push((modifier, flag));
            }
        }
        self.queue.push(Input::Key { key, down: true, modifiers });
        self.queue.push(Input::Key { key, down: false, modifiers });
        while let Some((modifier, flag)) = pressed.pop() {
            *flag(&mut modifiers) = false;
            self.queue.push(Input::Key { key: modifier, down: false, modifiers });
        }
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

    /// Queues `input` for the PC, unless it hears nothing.
    fn send(&mut self, input: Input) {
        if !self.muted {
            self.queue.push(input);
        }
    }

    /// Queues `input`. Movement that follows movement is merged into it: the
    /// window reports far more of it than a PC needs. Anything else keeps
    /// its place, so a click still lands where the pointer was.
    fn push(&mut self, input: Input) {
        if self.muted {
            return;
        }
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

/// Where [`Modifiers`] says that one of them is held.
type Flag = fn(&mut Modifiers) -> &mut bool;

/// Takes the whole part out of `amount` and leaves the rest for next time.
fn whole(amount: &mut Vec2) -> [i16; 2] {
    let limit = f32::from(i16::MAX);
    let taken = vec2(amount.x.trunc().clamp(-limit, limit), amount.y.trunc().clamp(-limit, limit));
    *amount -= taken;
    [taken.x as i16, taken.y as i16]
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_pointer_near_an_edge_of_the_picture_counts_as_on_it() {
        use super::near_edge;
        // No band: where it is.
        assert_eq!(near_edge(3.0, 800.0, 0.0), 3.0);
        // The outer half of the band is the edge, at both ends.
        assert_eq!(near_edge(0.0, 800.0, 24.0), 0.0);
        assert_eq!(near_edge(12.0, 800.0, 24.0), 0.0);
        assert_eq!(near_edge(790.0, 800.0, 24.0), 800.0);
        // The inner half stretches, and meets the rest without a jump.
        assert_eq!(near_edge(18.0, 800.0, 24.0), 12.0);
        assert_eq!(near_edge(24.0, 800.0, 24.0), 24.0);
        assert_eq!(near_edge(776.0, 800.0, 24.0), 776.0);
        assert_eq!(near_edge(400.0, 800.0, 24.0), 400.0);
        // Outside the picture is still its edge, once clamped.
        assert!(near_edge(-5.0, 800.0, 24.0) <= 0.0);
        // A picture too short is left alone.
        assert_eq!(near_edge(5.0, 60.0, 24.0), 5.0);
    }

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
        assert_eq!(controls.key(egui::Key::T, None, true), Some(Shortcut::Microphone));
        assert_eq!(controls.key(egui::Key::C, None, true), Some(Shortcut::Camera));
        assert_eq!(controls.key(egui::Key::S, None, true), Some(Shortcut::Statistics));
        assert_eq!(controls.key(egui::Key::Z, Some(egui::Key::W), true), Some(Shortcut::Release));
        assert_eq!(controls.take(), [], "the PC hears nothing of a shortcut's letter");

        // Any other letter goes to the PC, with the three modifiers.
        assert_eq!(controls.key(egui::Key::D, None, true), None);
        let all = Modifiers { shift: true, ctrl: true, alt: true, meta: false };
        assert_eq!(controls.take(), [Input::Key { key: letter('D'), down: true, modifiers: all }]);

        // Without all three, Q is a letter like any other.
        let mut controls = Controls::default();
        controls.key(egui::Key::ControlLeft, None, true);
        assert_eq!(controls.key(egui::Key::Q, None, true), None);
        assert_eq!(controls.take().len(), 2);
    }

    #[test]
    fn keys_taken_from_the_system_find_the_shortcuts_by_their_letter() {
        // Windows names the key by where it sits, and its layout says the
        // letter: on a French keyboard Q sits where A does elsewhere.
        let mut controls = Controls::default();
        for modifier in [Key::LEFT_CTRL, Key::LEFT_ALT, Key::LEFT_SHIFT] {
            assert_eq!(controls.stroke(Some(modifier), None, true), None);
        }
        assert_eq!(controls.stroke(Some(letter('A')), Some('Q'), true), Some(Shortcut::Leave));
        assert_eq!(controls.stroke(Some(Key::LEFT_SUPER), None, true), None);
        let all = Modifiers { shift: true, ctrl: true, alt: true, meta: false };
        let held = controls.take();
        assert_eq!(held.len(), 4, "{held:?}");
        assert_eq!(
            held[3],
            Input::Key {
                key: Key::LEFT_SUPER,
                down: true,
                modifiers: Modifiers { meta: true, ..all }
            }
        );
        // A key the PC has no name for is nobody's.
        assert_eq!(controls.stroke(None, None, true), None);
        assert_eq!(controls.take(), []);
    }

    #[test]
    fn muted_controls_tell_the_pc_nothing_but_still_find_the_shortcuts() {
        let mut controls = Controls::default();
        controls.stroke(Some(Key::LEFT_CTRL), None, true);
        controls.button(PointerButton::Primary, true);
        controls.take();

        // The PC lets go of what it held, and hears nothing more.
        controls.mute(true);
        assert_eq!(
            controls.take(),
            [
                Input::MouseButton { button: MouseButton::Left, down: false },
                Input::Key { key: Key::LEFT_CTRL, down: false, modifiers: NO_MODIFIERS },
            ]
        );
        controls.stroke(Some(Key::LEFT_CTRL), None, true);
        controls.stroke(Some(Key::LEFT_ALT), None, true);
        controls.stroke(Some(Key::LEFT_SHIFT), None, true);
        controls.pointer_at(
            pos2(10.0, 10.0),
            Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 100.0)),
            1.0,
        );
        controls.wheel(MouseWheelUnit::Line, vec2(0.0, 1.0));
        assert_eq!(controls.stroke(Some(letter('W')), Some('Z'), true), Some(Shortcut::Release));
        assert_eq!(controls.take(), []);

        // Back to the PC: the keys of the shortcut come up there.
        controls.mute(false);
        controls.stroke(Some(Key::LEFT_SHIFT), None, false);
        assert_eq!(controls.take().len(), 1);
    }

    #[test]
    fn a_shortcut_of_the_pc_presses_what_is_missing_and_lets_go_of_it_alone() {
        let n = letter('N');
        let ctrl = Modifiers { ctrl: true, ..NO_MODIFIERS };
        let ctrl_alt = Modifiers { alt: true, ..ctrl };
        let all = Modifiers { shift: true, ..ctrl_alt };
        let key = |key, down, modifiers| Input::Key { key, down, modifiers };

        // Nothing held: all three go down, the letter is typed, and they
        // come up again, the last pressed first.
        let mut controls = Controls::default();
        controls.pc_shortcut(n);
        assert_eq!(
            controls.take(),
            [
                key(Key::LEFT_CTRL, true, ctrl),
                key(Key::LEFT_ALT, true, ctrl_alt),
                key(Key::LEFT_SHIFT, true, all),
                key(n, true, all),
                key(n, false, all),
                key(Key::LEFT_SHIFT, false, ctrl_alt),
                key(Key::LEFT_ALT, false, ctrl),
                key(Key::LEFT_CTRL, false, NO_MODIFIERS),
            ]
        );
        assert!(!controls.holds_anything());

        // What the person holds stays held, on either side.
        controls.stroke(Some(Key::RIGHT_CTRL), None, true);
        controls.stroke(Some(Key::LEFT_SHIFT), None, true);
        controls.take();
        controls.pc_shortcut(n);
        let shift_ctrl = Modifiers { shift: true, ..ctrl };
        assert_eq!(
            controls.take(),
            [
                key(Key::LEFT_ALT, true, all),
                key(n, true, all),
                key(n, false, all),
                key(Key::LEFT_ALT, false, shift_ctrl),
            ]
        );

        // Muted, the PC holds nothing of it, and hears the shortcut all the
        // same.
        controls.mute(true);
        controls.take();
        controls.switch_pc_pointer();
        let heard = controls.take();
        assert_eq!(heard.len(), 8);
        assert_eq!(heard[3], key(n, true, all));
    }

    #[test]
    fn the_person_can_switch_the_pcs_pointer_too() {
        // When Leto is wrong about the PC's pointer, as after a stream that
        // ended badly, the person presses the PC's shortcut: Leto keeps
        // none for itself on that letter.
        assert_eq!(Shortcut::of(POINTER_SWITCH), None);
        let mut controls = Controls::default();
        for modifier in [Key::LEFT_CTRL, Key::LEFT_ALT, Key::LEFT_SHIFT] {
            controls.stroke(Some(modifier), None, true);
        }
        assert_eq!(controls.stroke(Key::letter(POINTER_SWITCH), Some(POINTER_SWITCH), true), None);
        assert_eq!(controls.take().len(), 4);
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
        // A picture drawn 960 by 540 points with black above it, in a window
        // that makes two pixels of a point: 1920 by 1080 pixels.
        let place = Rect::from_min_size(pos2(0.0, 100.0), vec2(960.0, 540.0));
        let at = |pointer| {
            let mut controls = Controls::default();
            controls.pointer_at(pointer, place, 2.0);
            match controls.take()[..] {
                [Input::MousePosition { x, y, width: 1919, height: 1079 }] => (x, y),
                ref other => panic!("{other:?}"),
            }
        };
        assert_eq!(at(pos2(480.0, 370.0)), (960, 540));
        assert_eq!(at(pos2(0.0, 100.0)), (0, 0));
        // In the black around the picture: its nearest edge.
        assert_eq!(at(pos2(240.0, 20.0)), (480, 0));
        assert_eq!(at(pos2(-50.0, 900.0)), (0, 1079));
        assert_eq!(at(pos2(960.0, 640.0)), (1919, 1079));

        // No picture, no position.
        let mut controls = Controls::default();
        controls.pointer_at(pos2(10.0, 10.0), place, 0.0);
        controls.pointer_at(pos2(10.0, 10.0), Rect::ZERO, 2.0);
        assert_eq!(controls.take(), []);
    }

    #[test]
    fn the_last_pixel_of_the_picture_is_the_far_edge_of_the_pcs_screen() {
        // A picture that fills a screen of 3840 by 1080 pixels: the pointer
        // goes no further than the pixel 3839, 1079, and a taskbar that
        // hides at the bottom of the PC's screen waits for the last row.
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let place = Rect::from_min_size(pos2(0.0, 0.0), vec2(3840.0, 1080.0) / scale);
            let mut controls = Controls::default();
            controls.pointer_at(pos2(3839.0, 1079.0) / scale, place, scale);
            let [Input::MousePosition { x, y, width, height }] = controls.take()[..] else {
                panic!("one position");
            };
            assert_eq!((x, y), (width, height), "at {scale} pixels a point");
            assert_eq!((width, height), (3839, 1079));

            // The first pixel is the near edge, as it was.
            controls.pointer_at(pos2(0.0, 0.0), place, scale);
            let [Input::MousePosition { x: 0, y: 0, .. }] = controls.take()[..] else {
                panic!("the corner");
            };
        }
    }

    #[test]
    fn movement_is_merged_but_a_click_keeps_its_place() {
        let place = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 100.0));
        let mut controls = Controls::default();
        for x in [10.0, 20.0, 30.0] {
            controls.pointer_at(pos2(x, 50.0), place, 1.0);
        }
        controls.button(PointerButton::Primary, true);
        controls.pointer_at(pos2(40.0, 50.0), place, 1.0);
        controls.pointer_at(pos2(60.0, 50.0), place, 1.0);
        controls.button(PointerButton::Primary, false);

        let position = |x| Input::MousePosition { x, y: 50, width: 99, height: 99 };
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

    #[test]
    fn the_buttons_and_the_wheel_go_the_way_the_person_chose() {
        let mut controls = Controls::default();
        controls.choose(true, true);
        controls.button(PointerButton::Primary, true);
        controls.button(PointerButton::Middle, true);
        controls.wheel(MouseWheelUnit::Line, vec2(1.0, 1.0));
        assert_eq!(
            controls.take(),
            [
                Input::MouseButton { button: MouseButton::Right, down: true },
                Input::MouseButton { button: MouseButton::Middle, down: true },
                Input::Scroll { horizontal: 120, vertical: -120 },
            ]
        );
        // What is held is let go of as the PC knows it.
        controls.release_all();
        assert_eq!(
            controls.take(),
            [
                Input::MouseButton { button: MouseButton::Middle, down: false },
                Input::MouseButton { button: MouseButton::Right, down: false },
            ]
        );
    }
}
