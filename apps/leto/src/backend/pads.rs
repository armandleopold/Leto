//! Which gamepad has which number on the PC, and what the PC knows of each.
//!
//! The desktop app hears of gamepads from the system and the web build from
//! the browser. What they tell the PC is the same and is decided here: a
//! stream hears of every gamepad once, then of each change, and of a gamepad
//! being unplugged.

use leto_core::input::{GamepadButtons, GamepadKind, GamepadState, Input, MAX_GAMEPADS};

/// The gamepads of this device, by number on the PC.
#[derive(Default)]
pub struct Pads {
    slots: [Option<Pad>; MAX_GAMEPADS as usize],
}

struct Pad {
    /// The gamepad's id on this device.
    id: usize,
    kind: GamepadKind,
    /// What the PC was last told, once it has heard of this gamepad.
    sent: Option<GamepadState>,
}

impl Pads {
    /// Whether the gamepad `id` has a number.
    pub fn knows(&self, id: usize) -> bool {
        self.slots.iter().flatten().any(|pad| pad.id == id)
    }

    /// A gamepad was plugged in: it takes the lowest free number. One more
    /// than a PC accepts is left out.
    pub fn plugged(&mut self, id: usize, kind: GamepadKind) {
        if self.knows(id) {
            return;
        }
        match self.slots.iter_mut().find(|slot| slot.is_none()) {
            Some(free) => *free = Some(Pad { id, kind, sent: None }),
            None => log::warn!("more gamepads than a PC accepts: one is ignored"),
        }
    }

    /// A gamepad was unplugged. Returns what to tell the PC, if it had heard
    /// of that gamepad.
    pub fn unplugged(&mut self, id: usize) -> Option<Input> {
        let (number, slot) = self.slot(id)?;
        let pad = slot.take()?;
        pad.sent.map(|_| Input::GamepadLeft { number })
    }

    /// The PC knows nothing of the gamepads any more.
    // A page starts from nothing with each stream instead.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub fn forget_what_was_sent(&mut self) {
        for pad in self.slots.iter_mut().flatten() {
            pad.sent = None;
        }
    }

    // Android's gamepads are reported whole each time.
    #[cfg_attr(target_os = "android", allow(dead_code))]
    pub fn plugged_in(&self) -> Vec<usize> {
        self.slots.iter().flatten().map(|pad| pad.id).collect()
    }

    /// The gamepad `id` is in `state`. Returns what to tell the PC: first
    /// that the gamepad exists, then its state, when either is news.
    pub fn report(&mut self, id: usize, state: GamepadState) -> Vec<Input> {
        let Some((number, Some(pad))) = self.slot(id) else { return Vec::new() };
        let mut inputs = Vec::new();
        if pad.sent.is_none() {
            inputs.push(Input::GamepadArrived { number, kind: pad.kind });
        }
        if pad.sent != Some(state) {
            pad.sent = Some(state);
            inputs.push(Input::Gamepad { number, state });
        }
        inputs
    }

    fn slot(&mut self, id: usize) -> Option<(u8, &mut Option<Pad>)> {
        let index =
            self.slots.iter().position(|slot| slot.as_ref().is_some_and(|pad| pad.id == id))?;
        Some((u8::try_from(index).ok()?, &mut self.slots[index]))
    }
}

/// `input` as the person chose to have it: with `swap` the buttons A and B
/// change places, and X and Y, for gamepads labelled the other way round.
pub fn as_chosen(input: Input, swap: bool) -> Input {
    let Input::Gamepad { number, mut state } = input else { return input };
    if swap {
        let held = state.buttons;
        for (one, other) in
            [(GamepadButtons::A, GamepadButtons::B), (GamepadButtons::X, GamepadButtons::Y)]
        {
            state.buttons.set(one, held.contains(other));
            state.buttons.set(other, held.contains(one));
        }
    }
    Input::Gamepad { number, state }
}

/// The family of a gamepad, so the PC can show the right button names.
pub fn kind(vendor: Option<u16>, name: &str) -> GamepadKind {
    const MICROSOFT: u16 = 0x045E;
    const SONY: u16 = 0x054C;
    const NINTENDO: u16 = 0x057E;
    match vendor {
        Some(MICROSOFT) => return GamepadKind::Xbox,
        Some(SONY) => return GamepadKind::PlayStation,
        Some(NINTENDO) => return GamepadKind::Nintendo,
        _ => {}
    }
    let name = name.to_lowercase();
    let named = |words: &[&str]| words.iter().any(|word| name.contains(word));
    if named(&["xbox", "x-box", "xinput"]) {
        GamepadKind::Xbox
    } else if named(&["playstation", "dualsense", "dualshock", "ps3", "ps4", "ps5"]) {
        GamepadKind::PlayStation
    } else if named(&["nintendo", "switch", "joy-con"]) {
        GamepadKind::Nintendo
    } else {
        GamepadKind::Unknown
    }
}

/// The maker of a gamepad, when the name a browser gives it says: Chrome and
/// Edge end it with "(… Vendor: 054c Product: 09cc)", Firefox starts it with
/// "054c-09cc-".
#[cfg(any(target_arch = "wasm32", test))]
pub fn vendor(name: &str) -> Option<u16> {
    let number = |digits: &str| {
        let hexadecimal = !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_hexdigit());
        hexadecimal.then(|| u16::from_str_radix(digits, 16).ok()).flatten()
    };
    if let Some((_, after)) = name.split_once("Vendor: ") {
        return number(after.split(' ').next()?);
    }
    let mut parts = name.splitn(3, '-');
    let (vendor, product) = (parts.next()?, parts.next()?);
    // Two numbers, or it is a name with dashes in it.
    number(product).and(number(vendor))
}

/// The buttons of the layout browsers call "standard", in its order. The
/// two triggers come seventh and eighth, and say how far they are pressed.
#[cfg(any(target_arch = "wasm32", test))]
const STANDARD: [Option<GamepadButtons>; 17] = [
    Some(GamepadButtons::A),
    Some(GamepadButtons::B),
    Some(GamepadButtons::X),
    Some(GamepadButtons::Y),
    Some(GamepadButtons::LEFT_SHOULDER),
    Some(GamepadButtons::RIGHT_SHOULDER),
    None,
    None,
    Some(GamepadButtons::BACK),
    Some(GamepadButtons::START),
    Some(GamepadButtons::LEFT_STICK),
    Some(GamepadButtons::RIGHT_STICK),
    Some(GamepadButtons::UP),
    Some(GamepadButtons::DOWN),
    Some(GamepadButtons::LEFT),
    Some(GamepadButtons::RIGHT),
    Some(GamepadButtons::GUIDE),
];
#[cfg(any(target_arch = "wasm32", test))]
const LEFT_TRIGGER: usize = 6;
#[cfg(any(target_arch = "wasm32", test))]
const RIGHT_TRIGGER: usize = 7;

/// The state of a gamepad that a browser describes in its "standard" layout:
/// whether each button is pressed and how far, then where each axis is, from
/// -1 to 1 with right and down positive.
#[cfg(any(target_arch = "wasm32", test))]
pub fn standard(buttons: &[(bool, f32)], axes: &[f32]) -> GamepadState {
    let mut held = GamepadButtons::NONE;
    for (&(pressed, _), button) in buttons.iter().zip(STANDARD) {
        if let Some(button) = button {
            held.set(button, pressed);
        }
    }
    // A number that is none says nothing: the PC is not sent one.
    let within =
        |value: f32, least: f32| if value.is_nan() { 0.0 } else { value.clamp(least, 1.0) };
    let trigger = |index: usize| match buttons.get(index) {
        // A trigger that only says it is pressed is pressed all the way.
        Some(&(true, deep)) if within(deep, 0.0) == 0.0 => 1.0,
        Some(&(_, deep)) => within(deep, 0.0),
        None => 0.0,
    };
    let axis = |index: usize| within(axes.get(index).copied().unwrap_or(0.0), -1.0);
    GamepadState {
        buttons: held,
        left_trigger: trigger(LEFT_TRIGGER),
        right_trigger: trigger(RIGHT_TRIGGER),
        // A browser counts downwards, a PC upwards.
        left_stick: [axis(0), 0.0 - axis(1)],
        right_stick: [axis(2), 0.0 - axis(3)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REST: GamepadState = GamepadState {
        buttons: GamepadButtons::NONE,
        left_trigger: 0.0,
        right_trigger: 0.0,
        left_stick: [0.0, 0.0],
        right_stick: [0.0, 0.0],
    };

    #[test]
    fn a_and_b_and_x_and_y_change_places_when_the_person_chose_so() {
        let held = GamepadButtons::A.with(GamepadButtons::Y).with(GamepadButtons::START);
        let input = Input::Gamepad { number: 0, state: GamepadState { buttons: held, ..REST } };
        assert_eq!(as_chosen(input, false), input);
        let swapped = GamepadButtons::B.with(GamepadButtons::X).with(GamepadButtons::START);
        assert_eq!(
            as_chosen(input, true),
            Input::Gamepad { number: 0, state: GamepadState { buttons: swapped, ..REST } }
        );
        // Nothing else changes.
        assert_eq!(
            as_chosen(Input::GamepadLeft { number: 1 }, true),
            Input::GamepadLeft { number: 1 }
        );
    }

    #[test]
    fn a_stream_hears_of_a_gamepad_once_then_of_every_change() {
        let mut pads = Pads::default();
        pads.plugged(7, GamepadKind::Xbox);

        assert_eq!(
            pads.report(7, REST),
            [
                Input::GamepadArrived { number: 0, kind: GamepadKind::Xbox },
                Input::Gamepad { number: 0, state: REST },
            ]
        );
        // Nothing changed: nothing to say.
        assert_eq!(pads.report(7, REST), []);

        let jumping = GamepadState { buttons: GamepadButtons::A, ..REST };
        assert_eq!(pads.report(7, jumping), [Input::Gamepad { number: 0, state: jumping }]);

        // The next stream knows nothing: it hears of the gamepad again.
        pads.forget_what_was_sent();
        assert_eq!(
            pads.report(7, jumping),
            [
                Input::GamepadArrived { number: 0, kind: GamepadKind::Xbox },
                Input::Gamepad { number: 0, state: jumping },
            ]
        );
        // A gamepad nobody plugged in says nothing.
        assert_eq!(pads.report(3, REST), []);
    }

    #[test]
    fn gamepads_take_the_lowest_free_number() {
        let mut pads = Pads::default();
        pads.plugged(10, GamepadKind::Xbox);
        pads.plugged(11, GamepadKind::PlayStation);
        pads.plugged(11, GamepadKind::PlayStation);
        assert_eq!(pads.plugged_in(), [10, 11]);
        assert!(pads.knows(11));
        assert!(!pads.knows(12));
        pads.report(10, REST);
        pads.report(11, REST);

        // The PC had heard of the first one: it is told that it left.
        assert_eq!(pads.unplugged(10), Some(Input::GamepadLeft { number: 0 }));
        assert_eq!(pads.unplugged(10), None);

        // The next gamepad takes its place, and the second keeps its number.
        pads.plugged(12, GamepadKind::Nintendo);
        assert_eq!(
            pads.report(12, REST)[0],
            Input::GamepadArrived { number: 0, kind: GamepadKind::Nintendo }
        );
        assert_eq!(pads.report(11, REST), []);

        // A gamepad the PC never heard of leaves without a word.
        pads.plugged(13, GamepadKind::Unknown);
        assert_eq!(pads.unplugged(13), None);
    }

    #[test]
    fn one_gamepad_too_many_is_left_out() {
        let mut pads = Pads::default();
        for id in 0..20 {
            pads.plugged(id, GamepadKind::Unknown);
        }
        assert_eq!(pads.plugged_in().len(), usize::from(MAX_GAMEPADS));
        assert_eq!(pads.report(19, REST), []);
    }

    #[test]
    fn a_gamepad_is_told_apart_by_its_maker_then_by_its_name() {
        assert_eq!(kind(Some(0x045E), "Controller"), GamepadKind::Xbox);
        assert_eq!(kind(Some(0x054C), "Wireless Controller"), GamepadKind::PlayStation);
        assert_eq!(kind(Some(0x057E), "Pro Controller"), GamepadKind::Nintendo);
        assert_eq!(kind(None, "Xbox Series X Controller"), GamepadKind::Xbox);
        assert_eq!(kind(Some(0x0F0D), "HORI PS4 Fighting Stick"), GamepadKind::PlayStation);
        assert_eq!(kind(None, "8BitDo Pro 2"), GamepadKind::Unknown);
    }

    #[test]
    fn the_maker_is_read_from_the_name_a_browser_gives() {
        // Chrome and Edge.
        let chrome = "DualSense Wireless Controller (STANDARD GAMEPAD Vendor: 054c Product: 0ce6)";
        assert_eq!(vendor(chrome), Some(0x054C));
        assert_eq!(vendor("Xbox 360 Controller (XInput STANDARD GAMEPAD)"), None);
        // Firefox, which does not always write four digits.
        assert_eq!(vendor("057e-2009-Pro Controller"), Some(0x057E));
        assert_eq!(vendor("45e-28e-Xbox 360 Wired Controller"), Some(0x045E));
        assert_eq!(vendor("xinput"), None);
        // Safari names the gamepad and no maker, and a name may have dashes.
        assert_eq!(vendor("Xbox Wireless Controller Extended Gamepad"), None);
        assert_eq!(vendor("Joy-Con (L)"), None);
        assert_eq!(vendor("8BitDo-Pro-2"), None);
        assert_eq!(vendor("Vendor: zzzz"), None);
        assert_eq!(vendor(""), None);

        // Either way the family follows, by maker or by name.
        for (name, family) in [
            (chrome, GamepadKind::PlayStation),
            ("Xbox 360 Controller (XInput STANDARD GAMEPAD)", GamepadKind::Xbox),
            ("xinput", GamepadKind::Xbox),
            ("057e-2009-Pro Controller", GamepadKind::Nintendo),
            ("Unknown Gamepad (STANDARD GAMEPAD Vendor: 2dc8 Product: 6101)", GamepadKind::Unknown),
        ] {
            assert_eq!(kind(vendor(name), name), family, "{name}");
        }
    }

    #[test]
    fn the_standard_layout_of_browsers_is_read_by_where_buttons_sit() {
        let mut buttons = [(false, 0.0); 17];
        // A, the left trigger half way, Start, left on the cross, the logo.
        for (index, value) in [(0, 1.0), (6, 0.5), (9, 1.0), (14, 1.0), (16, 1.0)] {
            buttons[index] = (true, value);
        }
        // The right trigger of this gamepad only says that it is pressed.
        buttons[7] = (true, 0.0);
        // Left and a little down, then up further than a stick goes.
        let state = standard(&buttons, &[-1.0, 0.25, 0.0, -3.0]);
        assert_eq!(
            state,
            GamepadState {
                buttons: GamepadButtons::A
                    .with(GamepadButtons::START)
                    .with(GamepadButtons::LEFT)
                    .with(GamepadButtons::GUIDE),
                left_trigger: 0.5,
                right_trigger: 1.0,
                left_stick: [-1.0, -0.25],
                right_stick: [0.0, 1.0],
            }
        );

        // A gamepad with fewer buttons and sticks has the others at rest,
        // and a number that is none is not passed on.
        let small = standard(&[(false, 0.0), (true, 1.0)], &[f32::NAN]);
        assert_eq!(small, GamepadState { buttons: GamepadButtons::B, ..REST });

        // Every button of the PC's gamepad has a place, and only one.
        let mut all = GamepadButtons::NONE;
        for button in STANDARD.into_iter().flatten() {
            assert!(!all.contains(button));
            all = all.with(button);
        }
        assert!(GamepadButtons::EACH.iter().all(|button| all.contains(*button)));
        assert_eq!(STANDARD[LEFT_TRIGGER], None);
        assert_eq!(STANDARD[RIGHT_TRIGGER], None);
    }
}
