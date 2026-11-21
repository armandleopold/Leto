//! The gamepads plugged into an Android phone. Android tells of them in
//! the window's input events, which winit would make touches and unknown
//! keys of: [`hear`] takes them first (winit is given it as its input
//! hook), and keeps what each gamepad reports for [`pads`] to read.

use std::{
    collections::BTreeMap,
    sync::{Condvar, Mutex, MutexGuard},
    time::Duration,
};

use android_activity::input::{Axis, InputEvent, KeyAction, Keycode};

/// The gamepad bit of a source: a gamepad's buttons.
const GAMEPAD: u32 = 0x0000_0400;
/// The joystick bit of a source: a gamepad's sticks and triggers.
const JOYSTICK: u32 = 0x0100_0000;

/// A button of a gamepad, by where it sits: A is the lowest of the four.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    A,
    B,
    X,
    Y,
    Up,
    Down,
    Left,
    Right,
    LeftShoulder,
    RightShoulder,
    LeftTrigger,
    RightTrigger,
    LeftStick,
    RightStick,
    Start,
    Select,
    Mode,
}

impl Button {
    fn of(key: Keycode) -> Option<Self> {
        Some(match key {
            Keycode::ButtonA => Self::A,
            Keycode::ButtonB => Self::B,
            Keycode::ButtonX => Self::X,
            Keycode::ButtonY => Self::Y,
            Keycode::DpadUp => Self::Up,
            Keycode::DpadDown => Self::Down,
            Keycode::DpadLeft => Self::Left,
            Keycode::DpadRight => Self::Right,
            Keycode::ButtonL1 => Self::LeftShoulder,
            Keycode::ButtonR1 => Self::RightShoulder,
            Keycode::ButtonL2 => Self::LeftTrigger,
            Keycode::ButtonR2 => Self::RightTrigger,
            Keycode::ButtonThumbl => Self::LeftStick,
            Keycode::ButtonThumbr => Self::RightStick,
            Keycode::ButtonStart => Self::Start,
            Keycode::ButtonSelect => Self::Select,
            Keycode::ButtonMode => Self::Mode,
            _ => return None,
        })
    }

    const fn bit(self) -> u32 {
        1 << self as u32
    }
}

/// What a gamepad reports, as Android tells it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pad {
    /// Android's number for the gamepad, the same while it stays plugged.
    pub id: i32,
    buttons: u32,
    /// Where the left stick is, each from -1 to 1, right and down positive.
    pub left_stick: [f32; 2],
    /// Where the right stick is, as the left one.
    pub right_stick: [f32; 2],
    /// How far each trigger is pressed, from 0 to 1, as an axis.
    pub left_trigger: f32,
    pub right_trigger: f32,
    /// The pad's arrows when the gamepad reports them as an axis, as a
    /// stick is.
    pub hat: [f32; 2],
}

impl Pad {
    /// Whether `button` is held.
    pub fn pressed(&self, button: Button) -> bool {
        self.buttons & button.bit() != 0
    }
}

struct Pads {
    pads: BTreeMap<i32, Pad>,
    /// Counts the changes, for [`wait`].
    changes: u64,
}

static PADS: Mutex<Pads> = Mutex::new(Pads { pads: BTreeMap::new(), changes: 0 });
static CHANGED: Condvar = Condvar::new();

fn pads_lock() -> MutexGuard<'static, Pads> {
    PADS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Takes `event` if a gamepad sent it, and keeps what it says. Returns
/// whether it took it.
pub fn hear(event: &InputEvent<'_>) -> bool {
    match event {
        InputEvent::KeyEvent(key) => {
            let source = u32::from(key.source());
            let Some(button) = Button::of(key.key_code()) else { return false };
            // The arrows of a keyboard are not a gamepad's.
            let arrow = matches!(button, Button::Up | Button::Down | Button::Left | Button::Right);
            if arrow && source & GAMEPAD == 0 {
                return false;
            }
            let down = match key.action() {
                KeyAction::Down => true,
                KeyAction::Up => false,
                _ => return true,
            };
            change(key.device_id(), |pad| {
                if down { pad.buttons |= button.bit() } else { pad.buttons &= !button.bit() }
            });
            true
        }
        InputEvent::MotionEvent(motion) => {
            if u32::from(motion.source()) & JOYSTICK == 0 || motion.pointer_count() == 0 {
                return false;
            }
            let pointer = motion.pointer_at_index(0);
            let axis = |axis| pointer.axis_value(axis);
            change(motion.device_id(), |pad| {
                pad.left_stick = [axis(Axis::X), axis(Axis::Y)];
                pad.right_stick = [axis(Axis::Z), axis(Axis::Rz)];
                // Gamepads tell their triggers on one axis or the other.
                pad.left_trigger = axis(Axis::Ltrigger).max(axis(Axis::Brake));
                pad.right_trigger = axis(Axis::Rtrigger).max(axis(Axis::Gas));
                pad.hat = [axis(Axis::HatX), axis(Axis::HatY)];
            });
            true
        }
        _ => false,
    }
}

fn change(id: i32, work: impl FnOnce(&mut Pad)) {
    let mut pads = pads_lock();
    let new = !pads.pads.contains_key(&id);
    let pad = pads.pads.entry(id).or_insert(Pad { id, ..Pad::default() });
    let before = *pad;
    work(pad);
    if new || *pad != before {
        pads.changes += 1;
        CHANGED.notify_all();
    }
}

/// Waits up to `patience` for a change after the one counted `seen`, and
/// returns the count of the last one.
pub fn wait(seen: u64, patience: Duration) -> u64 {
    let pads = pads_lock();
    let (pads, _) = CHANGED
        .wait_timeout_while(pads, patience, |pads| pads.changes == seen)
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    pads.changes
}

/// Every gamepad heard of, as it last reported.
pub fn pads() -> Vec<Pad> {
    pads_lock().pads.values().copied().collect()
}

/// Forgets the gamepad `id`, which was unplugged.
pub fn forget(id: i32) {
    let mut pads = pads_lock();
    if pads.pads.remove(&id).is_some() {
        pads.changes += 1;
        CHANGED.notify_all();
    }
}

/// The name and the vendor's USB number of the gamepad `id`, or `None`
/// once it is unplugged. Asks Java: not for the window's thread.
pub fn describe(id: i32) -> Option<(String, u16)> {
    let described = crate::java::with_java(|env, _app| {
        let device = env
            .call_static_method(
                "android/view/InputDevice",
                "getDevice",
                "(I)Landroid/view/InputDevice;",
                &[jni::objects::JValue::Int(id)],
            )?
            .l()?;
        if device.is_null() {
            return Ok(None);
        }
        let name = env.call_method(&device, "getName", "()Ljava/lang/String;", &[])?.l()?;
        let name: String = env.get_string(&name.into())?.into();
        let vendor = env.call_method(&device, "getVendorId", "()I", &[])?.i()?;
        Ok(Some((name, u16::try_from(vendor).unwrap_or(0))))
    });
    match described {
        Ok(described) => described,
        Err(error) => {
            log::warn!("what gamepad {id} is is unknown: {error}");
            Some((String::new(), 0))
        }
    }
}
