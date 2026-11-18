//! What the person does with keyboard, mouse and gamepad reaches a stand-in
//! host, in order and unchanged.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU32, Ordering},
    thread,
    time::{Duration, Instant},
};

use leto_client::{
    Client, Counter, InputError, Session, StreamSettings,
    input::{
        GamepadButtons, GamepadKind, GamepadState, Input, InputAllowed, Key, Modifiers, MouseButton,
    },
};
use leto_testhost::{FakeHost, Options, permission};

/// A small picture, so the tests spend their time on the protocol.
const SMALL: StreamSettings = StreamSettings {
    width: 320,
    height: 180,
    fps: 30,
    bitrate_kbps: 2000,
    virtual_display: None,
    optimize_games: true,
    sound_on_pc: false,
};

/// A fresh settings folder per test.
struct Settings(PathBuf);

impl Settings {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name = format!(
            "leto-input-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        );
        let path = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&path);
        Self(path)
    }
}

impl Drop for Settings {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A paired client streaming the first app of a stand-in host.
struct Playing {
    session: Session,
    host: FakeHost,
    _client: Client,
    _settings: Settings,
}

fn play() -> Playing {
    play_on(FakeHost::start())
}

fn play_on(host: FakeHost) -> Playing {
    let settings = Settings::new();
    let client = Client::open(&settings.0).unwrap();
    let added = client.add(&host.address()).unwrap();
    let pin = client.new_pin().unwrap();
    host.type_pin(pin);
    let paired = client.pair(&added.id, pin).unwrap();
    let counter = Counter::default();
    let session =
        client.play(&paired.id, 1, SMALL, counter.video(), counter.audio(), |_| {}).unwrap();
    Playing { session, host, _client: client, _settings: settings }
}

/// Waits until the host has received `count` inputs and returns them.
fn received(host: &FakeHost, count: usize) -> Vec<Input> {
    let start = Instant::now();
    loop {
        let inputs = host.inputs();
        if inputs.len() >= count {
            return inputs;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the host received {inputs:?} and {count} inputs were expected"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

const NO_MODIFIERS: Modifiers = Modifiers { shift: false, ctrl: false, alt: false, meta: false };

#[test]
fn keys_arrive_with_their_modifiers() {
    let playing = play();
    let shift = Modifiers { shift: true, ..NO_MODIFIERS };
    let a = Key::letter('A').unwrap();
    let typed = [
        Input::Key { key: Key::LEFT_SHIFT, down: true, modifiers: shift },
        Input::Key { key: a, down: true, modifiers: shift },
        Input::Key { key: a, down: false, modifiers: shift },
        Input::Key { key: Key::LEFT_SHIFT, down: false, modifiers: NO_MODIFIERS },
    ];

    for input in typed {
        playing.session.send(input).unwrap();
    }

    assert_eq!(received(&playing.host, 4), typed);
}

#[test]
fn a_held_key_is_sent_once() {
    let playing = play();
    let w = Key::letter('W').unwrap();
    let down = Input::Key { key: w, down: true, modifiers: NO_MODIFIERS };
    let up = Input::Key { key: w, down: false, modifiers: NO_MODIFIERS };

    // What a keyboard does while a key is held: it says "down" again and again.
    for _ in 0..5 {
        playing.session.send(down).unwrap();
    }
    playing.session.send(up).unwrap();

    assert_eq!(received(&playing.host, 2), [down, up]);
}

#[test]
fn a_click_never_overtakes_the_movement_before_it() {
    let playing = play();
    // Sent in one go, as a window hands them over: where the pointer went,
    // then what its buttons did there.
    let clicks = [
        Input::MousePosition { x: 160, y: 90, width: 320, height: 180 },
        Input::MouseButton { button: MouseButton::Left, down: true },
        Input::MouseButton { button: MouseButton::Left, down: false },
        Input::MousePosition { x: 20, y: 170, width: 320, height: 180 },
        Input::MouseButton { button: MouseButton::Right, down: true },
        Input::MouseButton { button: MouseButton::Right, down: false },
        Input::MouseMove { dx: -5, dy: 12 },
        Input::MouseButton { button: MouseButton::Back, down: true },
        Input::MouseButton { button: MouseButton::Back, down: false },
        Input::MouseButton { button: MouseButton::Forward, down: true },
        Input::MouseButton { button: MouseButton::Forward, down: false },
        Input::MouseButton { button: MouseButton::Middle, down: true },
        Input::MouseButton { button: MouseButton::Middle, down: false },
    ];

    for input in clicks {
        playing.session.send(input).unwrap();
    }

    assert_eq!(received(&playing.host, clicks.len()), clicks);
}

#[test]
fn every_mouse_movement_arrives() {
    let playing = play();

    for step in 0..200 {
        playing.session.send(Input::MouseMove { dx: 3, dy: -1 - step % 2 }).unwrap();
    }

    let moved = received(&playing.host, 200).iter().fold((0i32, 0i32), |sum, input| {
        let Input::MouseMove { dx, dy } = input else { panic!("{input:?} is not a movement") };
        (sum.0 + i32::from(*dx), sum.1 + i32::from(*dy))
    });
    assert_eq!(moved, (600, -300));
}

#[test]
fn the_wheel_turns_both_ways() {
    let playing = play();
    let turns = [
        Input::Scroll { horizontal: 0, vertical: 120 },
        Input::Scroll { horizontal: 0, vertical: -360 },
        Input::Scroll { horizontal: -240, vertical: 0 },
    ];

    for input in turns {
        playing.session.send(input).unwrap();
    }
    // Both at once are two messages for the PC.
    playing.session.send(Input::Scroll { horizontal: 120, vertical: 120 }).unwrap();

    let mut expected = turns.to_vec();
    expected.push(Input::Scroll { horizontal: 0, vertical: 120 });
    expected.push(Input::Scroll { horizontal: 120, vertical: 0 });
    assert_eq!(received(&playing.host, 5), expected);
}

#[test]
fn a_gamepad_arrives_plays_and_leaves() {
    let playing = play();
    let state = GamepadState {
        buttons: GamepadButtons::A.with(GamepadButtons::LEFT_SHOULDER).with(GamepadButtons::UP),
        left_trigger: 1.0,
        right_trigger: 0.0,
        left_stick: [1.0, -1.0],
        right_stick: [0.0, 1.0],
    };
    let played = [
        Input::GamepadArrived { number: 0, kind: GamepadKind::Xbox },
        Input::Gamepad { number: 0, state },
        Input::Gamepad { number: 0, state: GamepadState::default() },
        Input::GamepadLeft { number: 0 },
    ];

    for input in played {
        playing.session.send(input).unwrap();
        thread::sleep(Duration::from_millis(20));
    }

    assert_eq!(received(&playing.host, 4), played);
}

#[test]
fn every_gamepad_button_is_told_apart() {
    let playing = play();
    playing.session.send(Input::GamepadArrived { number: 1, kind: GamepadKind::Unknown }).unwrap();

    for button in GamepadButtons::EACH {
        let state = GamepadState { buttons: button, ..GamepadState::default() };
        playing.session.send(Input::Gamepad { number: 1, state }).unwrap();
        thread::sleep(Duration::from_millis(10));
    }

    let inputs = received(&playing.host, 1 + GamepadButtons::EACH.len());
    for (input, button) in inputs[1..].iter().zip(GamepadButtons::EACH) {
        let Input::Gamepad { number: 1, state } = input else {
            panic!("{input:?} is not the state of the second gamepad");
        };
        assert_eq!(state.buttons, button);
    }
}

#[test]
fn nothing_can_be_sent_once_the_stream_is_over() {
    let playing = play();
    playing.host.drop_stream();
    let start = Instant::now();
    while !playing.session.is_over() {
        assert!(start.elapsed() < Duration::from_secs(10), "the stream did not end");
        thread::sleep(Duration::from_millis(10));
    }

    let sent = playing.session.send(Input::MouseButton { button: MouseButton::Left, down: true });

    assert_eq!(sent, Err(InputError::NotConnected));
}

#[test]
fn every_input_is_allowed_where_nobody_decides_it() {
    let playing = play();
    assert_eq!(playing.session.input_allowed(), InputAllowed::ALL);
}

#[test]
fn a_device_apollo_only_lets_play_with_a_gamepad_is_told_so() {
    let allowed = permission::LAUNCH_APPS | permission::VIEW_STREAMS | permission::GAMEPAD;
    let playing =
        play_on(FakeHost::start_with(Options { permissions: Some(allowed), ..Options::default() }));

    assert_eq!(
        playing.session.input_allowed(),
        InputAllowed { keyboard: false, mouse: false, gamepad: true }
    );
    let key = Input::Key { key: Key::SPACE, down: true, modifiers: NO_MODIFIERS };
    assert_eq!(playing.session.send(key), Err(InputError::NotAllowed));
    let moved = Input::MouseMove { dx: 1, dy: 1 };
    assert_eq!(playing.session.send(moved), Err(InputError::NotAllowed));
    let click = Input::MouseButton { button: MouseButton::Left, down: true };
    assert_eq!(playing.session.send(click), Err(InputError::NotAllowed));

    let arrived = Input::GamepadArrived { number: 0, kind: GamepadKind::PlayStation };
    playing.session.send(arrived).unwrap();
    assert_eq!(received(&playing.host, 1), [arrived]);
}
