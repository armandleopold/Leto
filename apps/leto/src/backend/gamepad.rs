//! The gamepads plugged into this device, passed on to the PC being watched.
//!
//! One thread follows them for as long as the app runs. Every stream hears
//! about each gamepad once, then about every change, and about a gamepad
//! being unplugged.

use std::{thread, time::Duration};

use gilrs::{Axis, Button, EventType, Gilrs};
use leto_core::input::{GamepadButtons, GamepadState, Input};

use super::pads::{Menu, Pads, Press, kind};

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
/// an input on to that stream. Outside a stream, what a gamepad presses
/// goes to Leto's own screens through `menu`.
pub fn watch(
    stream: impl Fn() -> Option<u64> + Send + 'static,
    send: impl Fn(Input) + Send + 'static,
    menu: impl Fn(Press) + Send + 'static,
) {
    let spawned = thread::Builder::new().name("leto: gamepads".to_owned()).spawn(move || {
        match Gilrs::new() {
            Ok(gilrs) => follow(gilrs, &stream, &send, &menu),
            // Everything else works without them.
            Err(error) => log::warn!("gamepads cannot be used: {error}"),
        }
    });
    if let Err(error) = spawned {
        log::error!("could not start the gamepad thread: {error}");
    }
}

fn follow(
    mut gilrs: Gilrs,
    stream: &dyn Fn() -> Option<u64>,
    send: &dyn Fn(Input),
    menu: &dyn Fn(Press),
) {
    let mut pads = Pads::default();
    let mut screens = Menu::default();
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
                    screens.unplugged(id);
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
        // Presses are followed during a stream too: a button held when it
        // ends is no press on the screen after it.
        for (id, pad) in gilrs.gamepads() {
            let presses = screens.presses(usize::from(id), &state(&pad));
            if watched.is_none() {
                presses.into_iter().for_each(menu);
            }
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
