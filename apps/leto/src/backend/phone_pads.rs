//! The gamepads plugged into an Android phone, passed on to the PC being
//! watched, as `gamepad.rs` does elsewhere. Android tells of them in the
//! window's input events, which `leto_system::gamepad` keeps.

use std::{thread, time::Duration};

use leto_core::input::{GamepadButtons, GamepadState, Input};
use leto_system::gamepad::{self, Button, Pad};

use super::pads::{Menu, Pads, Press, kind};

/// How long to wait for a gamepad to do something before looking whether a
/// stream started or ended meanwhile, or a gamepad was unplugged.
const LOOK_EVERY: Duration = Duration::from_millis(100);

/// How often the gamepads heard of are looked for, to find those unplugged.
const UNPLUGGED_EVERY: u32 = 10;

/// The buttons of a gamepad, as Android and as the PC name them.
const BUTTONS: [(Button, GamepadButtons); 15] = [
    (Button::A, GamepadButtons::A),
    (Button::B, GamepadButtons::B),
    (Button::X, GamepadButtons::X),
    (Button::Y, GamepadButtons::Y),
    (Button::Up, GamepadButtons::UP),
    (Button::Down, GamepadButtons::DOWN),
    (Button::Left, GamepadButtons::LEFT),
    (Button::Right, GamepadButtons::RIGHT),
    (Button::LeftShoulder, GamepadButtons::LEFT_SHOULDER),
    (Button::RightShoulder, GamepadButtons::RIGHT_SHOULDER),
    (Button::Start, GamepadButtons::START),
    (Button::Select, GamepadButtons::BACK),
    (Button::LeftStick, GamepadButtons::LEFT_STICK),
    (Button::RightStick, GamepadButtons::RIGHT_STICK),
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
    let spawned = thread::Builder::new()
        .name("leto: gamepads".to_owned())
        .spawn(move || follow(&stream, &send, &menu));
    if let Err(error) = spawned {
        log::error!("could not start the gamepad thread: {error}");
    }
}

fn follow(stream: &dyn Fn() -> Option<u64>, send: &dyn Fn(Input), menu: &dyn Fn(Press)) {
    let mut pads = Pads::default();
    let mut screens = Menu::default();
    let mut told = None;
    let mut seen = 0;
    let mut looks = 0;
    loop {
        seen = gamepad::wait(seen, LOOK_EVERY);
        let watched = stream();
        let heard = gamepad::pads();

        // Android tells of a gamepad when it is first used.
        for pad in &heard {
            let id = number(pad.id);
            if !pads.knows(id) {
                let (name, vendor) = gamepad::describe(pad.id).unwrap_or_default();
                let kind = kind((vendor != 0).then_some(vendor), &name);
                log::info!("gamepad found: {name} ({kind:?})");
                pads.plugged(id, kind);
            }
        }
        looks += 1;
        if looks % UNPLUGGED_EVERY == 0 {
            for pad in &heard {
                if gamepad::describe(pad.id).is_none() {
                    log::info!("gamepad unplugged");
                    gamepad::forget(pad.id);
                    screens.unplugged(number(pad.id));
                    if let Some(input) = pads.unplugged(number(pad.id))
                        && watched.is_some()
                        && watched == told
                    {
                        send(input);
                    }
                }
            }
        }

        if watched != told {
            // Another stream, or none: the PC knows nothing of the gamepads.
            told = watched;
            pads.forget_what_was_sent();
        }
        // Presses are followed during a stream too: a button held when it
        // ends is no press on the screen after it.
        for pad in &heard {
            let presses = screens.presses(number(pad.id), &state(pad));
            if watched.is_none() {
                presses.into_iter().for_each(menu);
            }
        }
        if watched.is_none() {
            continue;
        }
        // Only news is sent: `report` leaves out a state already told.
        for pad in &heard {
            pads.report(number(pad.id), state(pad)).into_iter().for_each(send);
        }
    }
}

/// The gamepad's number in [`Pads`].
fn number(id: i32) -> usize {
    usize::try_from(id).unwrap_or(usize::MAX)
}

/// What `pad` reports, as the PC wants it: up positive, and the arrows and
/// triggers whichever way the gamepad tells them.
fn state(pad: &Pad) -> GamepadState {
    let mut buttons = GamepadButtons::NONE;
    for (button, ours) in BUTTONS {
        buttons.set(ours, pad.pressed(button));
    }
    // Most gamepads tell their arrows as an axis, from -1 to 1.
    let [hat_x, hat_y] = pad.hat;
    for (held, ours) in [
        (hat_x < -0.5, GamepadButtons::LEFT),
        (hat_x > 0.5, GamepadButtons::RIGHT),
        (hat_y < -0.5, GamepadButtons::UP),
        (hat_y > 0.5, GamepadButtons::DOWN),
    ] {
        if held {
            buttons.set(ours, true);
        }
    }
    let trigger = |axis: f32, button| {
        let pressed = if pad.pressed(button) { 1.0 } else { 0.0 };
        axis.max(pressed).clamp(0.0, 1.0)
    };
    let stick = |[x, y]: [f32; 2]| [x.clamp(-1.0, 1.0), (-y).clamp(-1.0, 1.0)];
    GamepadState {
        buttons,
        left_trigger: trigger(pad.left_trigger, Button::LeftTrigger),
        right_trigger: trigger(pad.right_trigger, Button::RightTrigger),
        left_stick: stick(pad.left_stick),
        right_stick: stick(pad.right_stick),
    }
}
