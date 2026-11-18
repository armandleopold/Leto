//! Every key for the PC, the ones the system keeps for itself too: the
//! Windows key, Alt+Tab, Alt+F4 and the rest.
//!
//! A window hears of a key only once the system has acted on it, and Windows
//! acts on its own shortcuts before any window hears of them. While a stream
//! has the keyboard, Leto therefore listens to the keyboard itself, ahead of
//! the system, and keeps every key from going further while its window is in
//! front: the stream gets them, in order, from [`take`]. Only Ctrl+Alt+Del
//! and Windows+L stay with the system, which no program can take.
//!
//! The decisions are made here, on every system, and tested everywhere. The
//! listening itself is Windows' (`keyboard/windows.rs`).

use leto_core::input::Key;

#[cfg(windows)]
mod windows;

/// A key that went down or came up while the stream had the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stroke {
    /// The key by where it sits, as the PC understands it.
    pub key: Key,
    /// The letter the layout of this device prints on it, when it is one:
    /// Leto's own shortcuts are found by the letter people see.
    pub letter: Option<char>,
    pub down: bool,
}

/// Lets the stream have every key while its window is in front, or gives
/// them back to the system. Returns whether this system lets Leto have them:
/// where it does not, the stream has what its window gets.
///
/// Ask for them only while the window is in front: each time it comes back,
/// Leto takes them again ahead of any program that started listening
/// meanwhile. Cheap to call every frame: only a change does anything.
pub fn capture(on: bool) -> bool {
    #[cfg(windows)]
    return windows::capture(on);
    #[cfg(not(windows))]
    {
        let _ = on;
        false
    }
}

/// Tells that a key reached the window while the stream had the keyboard,
/// which it should not have: something got in the way. Where the system lets
/// Leto have the keys, Leto takes them again, ahead of whatever did.
pub fn slipped() {
    #[cfg(windows)]
    windows::slipped();
}

/// The letter this device's layout prints on the key that sits where
/// `place` sits on a US keyboard, where the system tells. A window hears of
/// a letter through what it types, which Ctrl and Alt held together change
/// on a layout with AltGr: Leto's own shortcuts need the letter on the key.
pub fn letter_at(place: Key) -> Option<char> {
    #[cfg(windows)]
    return windows::letter_at(scan_of(place)?);
    #[cfg(not(windows))]
    {
        let _ = place;
        None
    }
}

/// The scan code of the key that sits where the letter `place` sits on a US
/// keyboard: what [`key_at`] does for letters, the other way round.
#[cfg_attr(not(windows), allow(dead_code))]
fn scan_of(place: Key) -> Option<u16> {
    let wanted = u8::try_from(place.0).ok().filter(u8::is_ascii_uppercase)?;
    [(TOP_ROW, 0x10), (MIDDLE_ROW, 0x1E), (BOTTOM_ROW, 0x2C)].into_iter().find_map(
        |(row, first)| {
            let at = row.iter().position(|letter| *letter == wanted)?;
            Some(first + u16::try_from(at).ok()?)
        },
    )
}

/// The letters of a US keyboard, row by row, and the scan code of the first
/// of each row.
const TOP_ROW: &[u8] = b"QWERTYUIOP";
const MIDDLE_ROW: &[u8] = b"ASDFGHJKL";
const BOTTOM_ROW: &[u8] = b"ZXCVBNM";

/// What the stream got since the last call, in order.
pub fn take() -> Vec<Stroke> {
    #[cfg(windows)]
    return windows::take();
    #[cfg(not(windows))]
    Vec::new()
}

/// Has `wake` called whenever a key is taken, from another thread, so that
/// whoever calls [`take`] does so soon.
pub fn set_wake(wake: impl Fn() + Send + Sync + 'static) {
    #[cfg(windows)]
    windows::set_wake(wake);
    #[cfg(not(windows))]
    let _ = wake;
}

/// A key as the system reports it, before anyone acted on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
struct Raw {
    /// What the layout of this device makes of the key: a Windows
    /// virtual-key code.
    virtual_key: u16,
    /// Where the key sits: its scan code, without the prefix that marks the
    /// keys added to the first PC keyboards.
    scan: u16,
    /// The key has that prefix.
    extended: bool,
    down: bool,
}

/// What becomes of a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
enum Verdict {
    /// The system and the window in front get it as usual.
    PassOn,
    /// The stream gets it, and nobody else.
    Take(Stroke),
    /// Nobody gets it: a key the keyboard made up, which the PC makes up
    /// too.
    Drop,
}

/// Decides, key after key, which are the stream's.
///
/// A key goes to whoever got it go down: a key held when the stream took the
/// keyboard, or pressed while another window was in front, comes up there
/// too. Otherwise that window would believe it is still held.
#[derive(Debug)]
#[cfg_attr(not(windows), allow(dead_code))]
struct Gate {
    capturing: bool,
    /// Keys the system got go down and has not yet seen come up, by
    /// virtual-key code.
    passed_down: [bool; 256],
    /// Keys the stream got go down and has not yet seen come up.
    taken_down: [bool; 256],
    /// What became of the keys pressed since the stream took the keyboard.
    tally: Tally,
}

/// How many keys went down for the stream since it took the keyboard, and
/// how many for another window in front. Which keys, the logs never say.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
struct Tally {
    taken: u32,
    elsewhere: u32,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Gate {
    const fn new() -> Self {
        Self {
            capturing: false,
            passed_down: [false; 256],
            taken_down: [false; 256],
            tally: Tally { taken: 0, elsewhere: 0 },
        }
    }

    /// Starts taking keys. `held` says which keys are down at this moment.
    fn arm(&mut self, held: impl Fn(u16) -> bool) {
        self.capturing = true;
        for (code, passed) in (0..).zip(&mut self.passed_down) {
            *passed = held(code);
        }
        self.taken_down = [false; 256];
    }

    fn disarm(&mut self) {
        *self = Self::new();
    }

    /// What becomes of `raw`. `in_front` says whether Leto's window is in
    /// front, which only matters for a key going down.
    fn decide(&mut self, raw: Raw, in_front: bool) -> Verdict {
        if !self.capturing {
            return Verdict::PassOn;
        }
        let code = usize::from(raw.virtual_key & 0xFF);
        if raw.down {
            if self.passed_down[code] {
                // Held down elsewhere, repeating.
                return Verdict::PassOn;
            }
            if in_front {
                return match self.take(raw) {
                    Some(verdict) => {
                        let taken = matches!(verdict, Verdict::Take(_));
                        self.taken_down[code] |= taken;
                        self.tally.taken = self.tally.taken.saturating_add(u32::from(taken));
                        verdict
                    }
                    None => {
                        self.passed_down[code] = true;
                        Verdict::PassOn
                    }
                };
            }
            self.passed_down[code] = true;
            self.tally.elsewhere = self.tally.elsewhere.saturating_add(1);
            return Verdict::PassOn;
        }
        if std::mem::take(&mut self.passed_down[code]) {
            return Verdict::PassOn;
        }
        if std::mem::take(&mut self.taken_down[code]) || in_front {
            // A key the stream got go down, or one nobody did: the PC lets
            // go of a key it does not hold without a word.
            return self.take(raw).unwrap_or(Verdict::PassOn);
        }
        Verdict::PassOn
    }

    /// What the stream makes of `raw`, or `None` for a key the PC has no
    /// name for.
    fn take(&self, raw: Raw) -> Option<Verdict> {
        if made_up(raw) {
            return Some(Verdict::Drop);
        }
        let key = key_at(raw.scan, raw.extended, raw.virtual_key)?;
        let letter =
            (0x41..=0x5A).contains(&raw.virtual_key).then(|| char::from(raw.virtual_key as u8));
        Some(Verdict::Take(Stroke { key, letter, down: raw.down }))
    }
}

/// Whether the keyboard, or Windows for it, made `raw` up.
///
/// A layout with AltGr reports a Ctrl with each press of it, with a scan
/// code no key has. Older keyboards report a Shift with the prefix of the
/// added keys around the arrows and the keys above them, depending on Num
/// Lock: no real Shift has the prefix. The PC makes up its own.
const fn made_up(raw: Raw) -> bool {
    raw.scan == 0x21D || (raw.extended && matches!(raw.scan, 0x2A | 0x36))
}

/// The key of the PC's keyboard that sits where a key reported with `scan`,
/// `extended` and `virtual_key` sits on this one.
///
/// The keys that type something are named by where they sit, as on a US
/// keyboard: the PC applies its own layout. So are the keys of the numeric
/// keypad, which this device's Num Lock would otherwise turn into arrows:
/// the PC's Num Lock decides. Every other key is named by what it does,
/// which no layout changes.
pub fn key_at(scan: u16, extended: bool, virtual_key: u16) -> Option<Key> {
    let letter = |row: &[u8], first: u16| Key::letter(char::from(row[usize::from(scan - first)]));
    let placed = match (scan, extended) {
        (0x02..=0x0A, false) => Key::digit((scan - 0x01) as u8),
        (0x0B, false) => Key::digit(0),
        (0x0C, false) => Some(Key::MINUS),
        (0x0D, false) => Some(Key::EQUALS),
        (0x10..=0x19, false) => letter(TOP_ROW, 0x10),
        (0x1A, false) => Some(Key::OPEN_BRACKET),
        (0x1B, false) => Some(Key::CLOSE_BRACKET),
        (0x1E..=0x26, false) => letter(MIDDLE_ROW, 0x1E),
        (0x27, false) => Some(Key::SEMICOLON),
        (0x28, false) => Some(Key::QUOTE),
        (0x29, false) => Some(Key::BACKTICK),
        (0x2B, false) => Some(Key::BACKSLASH),
        (0x2C..=0x32, false) => letter(BOTTOM_ROW, 0x2C),
        (0x33, false) => Some(Key::COMMA),
        (0x34, false) => Some(Key::PERIOD),
        (0x35, false) => Some(Key::SLASH),
        (0x56, false) => Some(Key::EXTRA),
        // The numeric keypad.
        (0x35, true) => Some(Key::KEYPAD_SLASH),
        (0x37, false) => Some(Key::KEYPAD_STAR),
        (0x47, false) => Key::keypad(7),
        (0x48, false) => Key::keypad(8),
        (0x49, false) => Key::keypad(9),
        (0x4A, false) => Some(Key::KEYPAD_MINUS),
        (0x4B, false) => Key::keypad(4),
        (0x4C, false) => Key::keypad(5),
        (0x4D, false) => Key::keypad(6),
        (0x4E, false) => Some(Key::KEYPAD_PLUS),
        (0x4F, false) => Key::keypad(1),
        (0x50, false) => Key::keypad(2),
        (0x51, false) => Key::keypad(3),
        (0x52, false) => Key::keypad(0),
        (0x53, false) => Some(Key::KEYPAD_PERIOD),
        _ => None,
    };
    if placed.is_some() {
        return placed;
    }
    match virtual_key {
        // No key at all, the mouse's buttons, a character typed by a
        // program rather than a key, and the code for none.
        0x00..=0x06 | 0xE7 | 0xFF => None,
        // Shift, Ctrl and Alt are always reported as left or right.
        code => Some(Key(code)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(virtual_key: u16, scan: u16, extended: bool, down: bool) -> Raw {
        Raw { virtual_key, scan, extended, down }
    }

    fn letter(letter: char) -> Key {
        Key::letter(letter).expect("a letter is a key")
    }

    #[test]
    fn a_key_that_types_is_named_by_where_it_sits() {
        // The key right of Tab: Q on a US keyboard, A on a French one, which
        // the French layout reports as A. The PC hears Q and applies its own
        // layout.
        assert_eq!(key_at(0x10, false, 0x41), Some(letter('Q')));
        assert_eq!(key_at(0x10, false, 0x51), Some(letter('Q')));
        assert_eq!(key_at(0x1E, false, 0x51), Some(letter('A')));
        assert_eq!(key_at(0x26, false, 0x4C), Some(letter('L')));
        assert_eq!(key_at(0x2C, false, 0x57), Some(letter('Z')));
        assert_eq!(key_at(0x32, false, 0xBC), Some(letter('M')));
        // The row of digits, which a French layout reports as signs.
        assert_eq!(key_at(0x02, false, 0x31), Key::digit(1));
        assert_eq!(key_at(0x0A, false, 0x39), Key::digit(9));
        assert_eq!(key_at(0x0B, false, 0x30), Key::digit(0));
        assert_eq!(key_at(0x0C, false, 0xDB), Some(Key::MINUS));
        // The key left of Z on a European keyboard.
        assert_eq!(key_at(0x56, false, 0xE2), Some(Key::EXTRA));
    }

    #[test]
    fn a_letter_is_found_by_where_it_sits() {
        for place in 'A'..='Z' {
            let scan = scan_of(letter(place)).expect("every letter sits somewhere");
            assert_eq!(key_at(scan, false, 0), Some(letter(place)), "{place}");
        }
        assert_eq!(scan_of(Key::digit(1).expect("1 is a key")), None);
        assert_eq!(scan_of(Key::TAB), None);
    }

    #[test]
    fn the_keypad_is_the_keypad_whatever_num_lock_says() {
        // With Num Lock off the 7 of the keypad reports itself as Home.
        assert_eq!(key_at(0x47, false, 0x24), Key::keypad(7));
        assert_eq!(key_at(0x47, false, 0x67), Key::keypad(7));
        assert_eq!(key_at(0x52, false, 0x2D), Key::keypad(0));
        assert_eq!(key_at(0x53, false, 0x2E), Some(Key::KEYPAD_PERIOD));
        assert_eq!(key_at(0x37, false, 0x6A), Some(Key::KEYPAD_STAR));
        assert_eq!(key_at(0x35, true, 0x6F), Some(Key::KEYPAD_SLASH));
        // The keys that share the keypad's scan codes, with the prefix of
        // the added keys, are themselves.
        assert_eq!(key_at(0x47, true, 0x24), Some(Key::HOME));
        assert_eq!(key_at(0x48, true, 0x26), Some(Key::UP));
        assert_eq!(key_at(0x53, true, 0x2E), Some(Key::DELETE));
        assert_eq!(key_at(0x37, true, 0x2C), Some(Key::PRINT_SCREEN));
        // Enter is Enter, on the keypad too.
        assert_eq!(key_at(0x1C, true, 0x0D), Some(Key::ENTER));
    }

    #[test]
    fn the_other_keys_are_named_by_what_they_do() {
        assert_eq!(key_at(0x5B, true, 0x5B), Some(Key::LEFT_SUPER));
        assert_eq!(key_at(0x38, false, 0xA4), Some(Key::LEFT_ALT));
        assert_eq!(key_at(0x38, true, 0xA5), Some(Key::RIGHT_ALT));
        assert_eq!(key_at(0x1D, true, 0xA3), Some(Key::RIGHT_CTRL));
        assert_eq!(key_at(0x0F, false, 0x09), Some(Key::TAB));
        assert_eq!(key_at(0x3E, false, 0x73), Key::function(4));
        assert_eq!(key_at(0x01, false, 0x1B), Some(Key::ESCAPE));
        // What no PC can do anything with.
        assert_eq!(key_at(0x00, false, 0xE7), None);
        assert_eq!(key_at(0x00, false, 0xFF), None);
    }

    #[test]
    fn nothing_is_taken_until_the_stream_has_the_keyboard() {
        let mut gate = Gate::new();
        let windows_key = raw(0x5B, 0x5B, true, true);
        assert_eq!(gate.decide(windows_key, true), Verdict::PassOn);
        gate.arm(|_| false);
        let taken = Stroke { key: Key::LEFT_SUPER, letter: None, down: true };
        assert_eq!(gate.decide(windows_key, true), Verdict::Take(taken));
        gate.disarm();
        assert_eq!(gate.decide(windows_key, true), Verdict::PassOn);
    }

    #[test]
    fn alt_tab_and_alt_f4_go_to_the_pc() {
        let mut gate = Gate::new();
        gate.arm(|_| false);
        let presses = [
            raw(0xA4, 0x38, false, true),
            raw(0x09, 0x0F, false, true),
            raw(0x09, 0x0F, false, false),
            raw(0x73, 0x3E, false, true),
            raw(0x73, 0x3E, false, false),
            raw(0xA4, 0x38, false, false),
        ];
        let heard: Vec<_> = presses
            .into_iter()
            .map(|press| match gate.decide(press, true) {
                Verdict::Take(stroke) => (stroke.key, stroke.down),
                other => panic!("{press:?} was not taken: {other:?}"),
            })
            .collect();
        let f4 = Key::function(4).expect("F4 is a key");
        assert_eq!(
            heard,
            [
                (Key::LEFT_ALT, true),
                (Key::TAB, true),
                (Key::TAB, false),
                (f4, true),
                (f4, false),
                (Key::LEFT_ALT, false)
            ]
        );
    }

    #[test]
    fn a_letter_says_what_the_layout_prints_on_it() {
        let mut gate = Gate::new();
        gate.arm(|_| false);
        // The key at the place of Q on a French keyboard prints A.
        let Verdict::Take(stroke) = gate.decide(raw(0x41, 0x10, false, true), true) else {
            panic!("a letter is taken");
        };
        assert_eq!(stroke, Stroke { key: letter('Q'), letter: Some('A'), down: true });
    }

    #[test]
    fn a_key_held_when_the_stream_took_the_keyboard_comes_up_where_it_went_down() {
        // Ctrl, Alt and Shift are still held from the shortcut that gave
        // the keyboard back to the stream.
        let mut gate = Gate::new();
        gate.arm(|code| matches!(code, 0xA2 | 0xA4 | 0xA0));
        assert_eq!(gate.decide(raw(0xA2, 0x1D, false, true), true), Verdict::PassOn);
        assert_eq!(gate.decide(raw(0xA2, 0x1D, false, false), true), Verdict::PassOn);
        // Pressed again, it is the stream's.
        assert!(matches!(gate.decide(raw(0xA2, 0x1D, false, true), true), Verdict::Take(_)));
        assert!(matches!(gate.decide(raw(0xA2, 0x1D, false, false), true), Verdict::Take(_)));
    }

    #[test]
    fn keys_pressed_for_another_window_stay_there() {
        let mut gate = Gate::new();
        gate.arm(|_| false);
        let a = raw(0x41, 0x1E, false, true);
        assert_eq!(gate.decide(a, false), Verdict::PassOn);
        // Leto comes in front meanwhile: the key still comes up there, and
        // repeats there until then.
        assert_eq!(gate.decide(a, true), Verdict::PassOn);
        assert_eq!(gate.decide(Raw { down: false, ..a }, true), Verdict::PassOn);

        // And a key the stream got go down comes up on the PC, even when
        // another window came in front in the meantime.
        assert!(matches!(gate.decide(a, true), Verdict::Take(_)));
        let up = Stroke { key: letter('A'), letter: Some('A'), down: false };
        assert_eq!(gate.decide(Raw { down: false, ..a }, false), Verdict::Take(up));
        // The logs say how many went where, and nothing of which.
        assert_eq!(gate.tally, Tally { taken: 1, elsewhere: 1 });
        gate.disarm();
        assert_eq!(gate.tally, Tally::default());
    }

    #[test]
    fn the_keys_a_keyboard_makes_up_reach_nobody() {
        let mut gate = Gate::new();
        gate.arm(|_| false);
        // The Ctrl that comes with AltGr, then AltGr itself.
        let made_up_ctrl = raw(0xA2, 0x21D, false, true);
        assert_eq!(gate.decide(made_up_ctrl, true), Verdict::Drop);
        assert!(matches!(gate.decide(raw(0xA5, 0x38, true, true), true), Verdict::Take(_)));
        assert_eq!(gate.decide(Raw { down: false, ..made_up_ctrl }, true), Verdict::Drop);
        // The Shift an old keyboard sends around an arrow key.
        assert_eq!(gate.decide(raw(0xA0, 0x2A, true, false), true), Verdict::Drop);
        assert_eq!(gate.decide(raw(0xA0, 0x2A, true, true), true), Verdict::Drop);
        // A real Shift is the PC's.
        assert!(matches!(gate.decide(raw(0xA0, 0x2A, false, true), true), Verdict::Take(_)));
    }
}
