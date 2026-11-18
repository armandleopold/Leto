//! The gamepads plugged into this device, passed on to the PC being watched.
//!
//! One thread follows them for as long as the app runs. Every stream hears
//! about each gamepad once, then about every change, and about a gamepad
//! being unplugged.

use std::{thread, time::Duration};

use gilrs::{Axis, Button, EventType, Gilrs};
use leto_core::input::{GamepadButtons, GamepadKind, GamepadState, Input, MAX_GAMEPADS};

/// How long to wait for a gamepad to do something before looking whether a
/// stream started or ended meanwhile.
const LOOK_EVERY: Duration = Duration::from_millis(100);

/// The buttons of a gamepad, by where they sit: A is the lowest of the four,
/// whatever letter is printed on it.
const BUTTONS: [(Button, GamepadButtons); 15] = [
    (Button::South, GamepadButtons::A),
    (Button::East, GamepadButtons::B),
    (Button::West, GamepadButtons::X),
    (Button::North, GamepadButtons::Y),
    (Button::DPadUp, GamepadButtons::UP),
    (Button::DPadDown, GamepadButtons::DOWN),
    (Button::DPadLeft, GamepadButtons::LEFT),
    (Button::DPadRight, GamepadButtons::RIGHT),
    (Button::LeftTrigger, GamepadButtons::LEFT_SHOULDER),
    (Button::RightTrigger, GamepadButtons::RIGHT_SHOULDER),
    (Button::Start, GamepadButtons::START),
    (Button::Select, GamepadButtons::BACK),
    (Button::LeftThumb, GamepadButtons::LEFT_STICK),
    (Button::RightThumb, GamepadButtons::RIGHT_STICK),
    (Button::Mode, GamepadButtons::GUIDE),
];

/// Starts following the gamepads. `stream` tells the stream being watched
/// from the one before it, and is `None` while there is none. `send` passes
/// an input on to that stream.
pub fn watch(
    stream: impl Fn() -> Option<u64> + Send + 'static,
    send: impl Fn(Input) + Send + 'static,
) {
    let spawned = thread::Builder::new().name("leto: gamepads".to_owned()).spawn(move || {
        match Gilrs::new() {
            Ok(gilrs) => follow(gilrs, &stream, &send),
            // Everything else works without them.
            Err(error) => log::warn!("gamepads cannot be used: {error}"),
        }
    });
    if let Err(error) = spawned {
        log::error!("could not start the gamepad thread: {error}");
    }
}

fn follow(mut gilrs: Gilrs, stream: &dyn Fn() -> Option<u64>, send: &dyn Fn(Input)) {
    let mut pads = Pads::default();
    for (id, pad) in gilrs.gamepads() {
        pads.plugged(id.into(), kind(pad.vendor_id(), pad.name()));
    }
    // The stream that has heard about these gamepads.
    let mut told = None;

    loop {
        // The gamepads that did something, each once: several of its
        // buttons and sticks moving together make one report.
        let mut moved = Vec::new();
        let mut event = gilrs.next_event_blocking(Some(LOOK_EVERY));
        let watched = stream();
        while let Some(gilrs::Event { id: pad, event: what, .. }) = event {
            let id = usize::from(pad);
            match what {
                EventType::Connected => {
                    let pad = gilrs.gamepad(pad);
                    let kind = kind(pad.vendor_id(), pad.name());
                    log::info!("gamepad plugged in: {} ({kind:?})", pad.name());
                    pads.plugged(id, kind);
                }
                EventType::Disconnected => {
                    log::info!("gamepad unplugged");
                    if let Some(input) = pads.unplugged(id)
                        && watched.is_some()
                        && watched == told
                    {
                        send(input);
                    }
                }
                _ => {}
            }
            if !moved.contains(&id) {
                moved.push(id);
            }
            event = gilrs.next_event();
        }

        if watched != told {
            // Another stream, or none: the PC knows nothing of the gamepads.
            told = watched;
            pads.forget_what_was_sent();
            moved = pads.plugged_in();
        }
        if watched.is_none() {
            continue;
        }
        for (id, pad) in gilrs.gamepads() {
            let id = usize::from(id);
            if moved.contains(&id) {
                pads.report(id, state(&pad)).into_iter().for_each(send);
            }
        }
    }
}

/// Which gamepad has which number on the PC, and what the PC knows of each.
#[derive(Default)]
struct Pads {
    /// By number on the PC.
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
    /// A gamepad was plugged in: it takes the lowest free number. One more
    /// than a PC accepts is left out.
    fn plugged(&mut self, id: usize, kind: GamepadKind) {
        if self.slots.iter().flatten().any(|pad| pad.id == id) {
            return;
        }
        match self.slots.iter_mut().find(|slot| slot.is_none()) {
            Some(free) => *free = Some(Pad { id, kind, sent: None }),
            None => log::warn!("more gamepads than a PC accepts: one is ignored"),
        }
    }

    /// A gamepad was unplugged. Returns what to tell the PC, if it had heard
    /// of that gamepad.
    fn unplugged(&mut self, id: usize) -> Option<Input> {
        let (number, slot) = self.slot(id)?;
        let pad = slot.take()?;
        pad.sent.map(|_| Input::GamepadLeft { number })
    }

    /// The PC knows nothing of the gamepads any more.
    fn forget_what_was_sent(&mut self) {
        for pad in self.slots.iter_mut().flatten() {
            pad.sent = None;
        }
    }

    fn plugged_in(&self) -> Vec<usize> {
        self.slots.iter().flatten().map(|pad| pad.id).collect()
    }

    /// The gamepad `id` is in `state`. Returns what to tell the PC: first
    /// that the gamepad exists, then its state, when either is news.
    fn report(&mut self, id: usize, state: GamepadState) -> Vec<Input> {
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

/// Everything `pad` reports right now.
fn state(pad: &gilrs::Gamepad<'_>) -> GamepadState {
    read(
        |button| pad.is_pressed(button),
        |button| pad.button_data(button).map_or(0.0, gilrs::ev::state::ButtonData::value),
        |axis| pad.value(axis),
    )
}

/// The state of a gamepad, from whether a button is `pressed`, how `deep`,
/// and where an `axis` is, from -1 to 1 with up and right positive.
fn read(
    pressed: impl Fn(Button) -> bool,
    deep: impl Fn(Button) -> f32,
    axis: impl Fn(Axis) -> f32,
) -> GamepadState {
    let mut buttons = GamepadButtons::NONE;
    for (button, ours) in BUTTONS {
        buttons.set(ours, pressed(button));
    }
    // A trigger is a button that says how far it is pressed. Some gamepads
    // report it as an axis instead.
    let trigger = |button, as_axis| deep(button).max(axis(as_axis)).clamp(0.0, 1.0);
    let stick = |x, y| [axis(x).clamp(-1.0, 1.0), axis(y).clamp(-1.0, 1.0)];
    GamepadState {
        buttons,
        left_trigger: trigger(Button::LeftTrigger2, Axis::LeftZ),
        right_trigger: trigger(Button::RightTrigger2, Axis::RightZ),
        left_stick: stick(Axis::LeftStickX, Axis::LeftStickY),
        right_stick: stick(Axis::RightStickX, Axis::RightStickY),
    }
}

/// The family of a gamepad, so the PC can show the right button names.
fn kind(vendor: Option<u16>, name: &str) -> GamepadKind {
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
    fn buttons_sticks_and_triggers_are_read_by_where_they_sit() {
        let state = read(
            |button| matches!(button, Button::South | Button::DPadLeft | Button::Mode),
            |button| if button == Button::LeftTrigger2 { 0.5 } else { 0.0 },
            |axis| match axis {
                Axis::LeftStickX => -1.0,
                Axis::LeftStickY => 0.25,
                Axis::RightStickY => 3.0,
                // The right trigger of this gamepad is an axis.
                Axis::RightZ => 0.75,
                _ => 0.0,
            },
        );
        assert_eq!(
            state,
            GamepadState {
                buttons: GamepadButtons::A.with(GamepadButtons::LEFT).with(GamepadButtons::GUIDE),
                left_trigger: 0.5,
                right_trigger: 0.75,
                left_stick: [-1.0, 0.25],
                right_stick: [0.0, 1.0],
            }
        );

        // Every button of the PC's gamepad has one here, and only one.
        let mut all = GamepadButtons::NONE;
        for (_, ours) in BUTTONS {
            assert!(!all.contains(ours));
            all = all.with(ours);
        }
        assert!(GamepadButtons::EACH.iter().all(|button| all.contains(*button)));
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
}
