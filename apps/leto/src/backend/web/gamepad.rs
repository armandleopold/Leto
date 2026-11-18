//! The gamepads the browser shows this page, passed on to the PC being
//! watched.
//!
//! A browser says nothing of what the buttons of a gamepad do: a page has to
//! look. A timer looks for as long as a stream is connected. A browser also
//! keeps a gamepad from a page until one of its buttons is pressed there, so
//! the PC hears of a gamepad at its first press.

use leto_core::input::{GamepadState, Input};
use wasm_bindgen::{JsCast as _, JsValue, closure::Closure};
use web_sys::{Gamepad, GamepadButton, GamepadMappingType};

use crate::backend::pads::{self, Pads};

/// Milliseconds between two looks: 125 looks a second, which is how often
/// most gamepads report.
const LOOK_EVERY: i32 = 8;

/// The gamepads of one stream. The next stream starts from nothing, as the
/// PC does.
pub struct Gamepads {
    pads: Pads,
    /// The gamepads that are left out, by the browser's number for them:
    /// each is explained once.
    left_out: Vec<usize>,
    timer: Option<i32>,
    _tick: Closure<dyn FnMut()>,
}

impl Gamepads {
    /// Starts a timer that calls `tick` whenever it is time to
    /// [`look`](Self::look), until this is dropped.
    pub fn watch(tick: impl FnMut() + 'static) -> Self {
        let tick = Closure::<dyn FnMut()>::new(tick);
        let timer = web_sys::window().and_then(|window| {
            window
                .set_interval_with_callback_and_timeout_and_arguments_0(
                    tick.as_ref().unchecked_ref(),
                    LOOK_EVERY,
                )
                .ok()
        });
        if timer.is_none() {
            log::warn!("the browser gave the page no timer: gamepads are not followed");
        }
        Self { pads: Pads::default(), left_out: Vec::new(), timer, _tick: tick }
    }

    /// What the gamepads did since the last look, for the PC to hear.
    pub fn look(&mut self) -> Vec<Input> {
        let shown = shown();
        let mut inputs = Vec::new();
        for id in self.pads.plugged_in() {
            if !shown.iter().any(|pad| number(pad) == id) {
                log::info!("gamepad unplugged");
                inputs.extend(self.pads.unplugged(id));
            }
        }
        self.left_out.retain(|id| shown.iter().any(|pad| number(pad) == *id));

        // A page that is not on screen hears nothing more of its gamepads:
        // what was held when it went away would stay held on the PC.
        let hidden = web_sys::window()
            .and_then(|window| window.document())
            .is_some_and(|document| document.hidden());
        for pad in &shown {
            let id = number(pad);
            if !self.pads.knows(id) && !self.meet(id, pad) {
                continue;
            }
            let state = if hidden { GamepadState::default() } else { state(pad) };
            inputs.extend(self.pads.report(id, state));
        }
        inputs
    }

    /// A gamepad the PC has no number for is there. Returns whether it has
    /// one now.
    fn meet(&mut self, id: usize, pad: &Gamepad) -> bool {
        if self.left_out.contains(&id) {
            return false;
        }
        let name = pad.id();
        if pad.mapping() != GamepadMappingType::Standard {
            // Which button is which is then anyone's guess, and a wrong
            // guess presses the wrong ones.
            log::warn!("the browser does not know the layout of this gamepad: {name} is left out");
            self.left_out.push(id);
            return false;
        }
        let kind = pads::kind(pads::vendor(&name), &name);
        log::info!("gamepad plugged in: {name} ({kind:?})");
        self.pads.plugged(id, kind);
        self.pads.knows(id)
    }
}

impl Drop for Gamepads {
    fn drop(&mut self) {
        if let (Some(timer), Some(window)) = (self.timer, web_sys::window()) {
            window.clear_interval_with_handle(timer);
        }
    }
}

/// The gamepads the browser shows this page right now.
fn shown() -> Vec<Gamepad> {
    let Some(window) = web_sys::window() else { return Vec::new() };
    // A page inside another one can be refused its gamepads.
    let Ok(list) = window.navigator().get_gamepads() else { return Vec::new() };
    list.iter()
        // The list keeps a hole where a gamepad was unplugged.
        .filter(JsValue::is_object)
        .map(JsValue::unchecked_into::<Gamepad>)
        .filter(Gamepad::connected)
        .collect()
}

/// The browser's number for a gamepad, which it keeps while plugged in.
fn number(pad: &Gamepad) -> usize {
    pad.index() as usize
}

/// Everything `pad` reports right now.
fn state(pad: &Gamepad) -> GamepadState {
    let buttons: Vec<(bool, f32)> = pad
        .buttons()
        .iter()
        .map(|button| {
            let button: GamepadButton = button.unchecked_into();
            (button.pressed(), button.value() as f32)
        })
        .collect();
    let axes: Vec<f32> =
        pad.axes().iter().map(|axis| axis.as_f64().unwrap_or(0.0) as f32).collect();
    pads::standard(&buttons, &axes)
}
