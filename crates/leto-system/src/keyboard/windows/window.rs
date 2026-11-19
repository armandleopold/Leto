//! The keys that reach Leto's window all the same: everywhere Windows never
//! calls the hook, as security software may see to, and wherever it removed
//! it without a word.
//!
//! Two things then keep the Windows key from acting on this device while
//! Leto's window is in front. Windows is asked to leave aside the shortcuts
//! programs registered, which it does for a program that listens to the
//! keyboard itself: the Start menu is Explorer's, as are Windows+E,
//! Windows+D and the rest. And each key is taken from the window's messages
//! for the stream before the window acts on it, so that Alt+F4 does not
//! close Leto either. Only what Windows itself keeps, Alt+Tab among them,
//! still acts here: the hook alone can keep that.
//!
//! All of it happens on the window's thread, the one that asks for the
//! keyboard.

use std::{cell::RefCell, ptr};

use windows_sys::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    System::Threading::GetCurrentThreadId,
    UI::{
        Input::{
            GetRegisteredRawInputDevices, KeyboardAndMouse::GetAsyncKeyState, RAWINPUTDEVICE,
            RIDEV_NOHOTKEYS, RIDEV_REMOVE, RegisterRawInputDevices,
        },
        WindowsAndMessaging::{
            CallNextHookEx, HC_ACTION, HHOOK, MSG, PM_NOREMOVE, PM_REMOVE, PeekMessageW,
            SetWindowsHookExW, UnhookWindowsHookEx, WH_GETMESSAGE, WM_KEYDOWN, WM_KEYFIRST,
            WM_KEYLAST, WM_KEYUP, WM_NULL, WM_SYSKEYDOWN, WM_SYSKEYUP,
        },
    },
};

use super::super::{Gate, Raw, Verdict, at_window, made_up_at_window};

/// The keyboard among the devices a program reads itself: its usage page
/// and usage.
const GENERIC: u16 = 0x01;
const KEYBOARD: u16 = 0x06;

/// What the window's thread keeps while the stream has the keyboard.
struct Kept {
    /// Watches the window's messages, or null where Windows did not let it.
    hook: HHOOK,
    gate: Gate,
    /// How the program read the keyboard itself before, if it did.
    read_before: Option<RAWINPUTDEVICE>,
    /// Keys taken from the window for the stream.
    taken: u32,
}

thread_local! {
    static KEPT: RefCell<Option<Kept>> = const { RefCell::new(None) };
}

/// Takes the keys that reach the window from now on, and asks Windows to
/// leave the programs' shortcuts aside. Called on the window's thread.
pub(super) fn arm() {
    if KEPT.with_borrow(Option::is_some) {
        return;
    }
    // SAFETY: a procedure that matches HOOKPROC, of this program, for this
    // very thread: no module is needed.
    let hook = unsafe {
        SetWindowsHookExW(WH_GETMESSAGE, Some(heard), ptr::null_mut(), GetCurrentThreadId())
    };
    if hook.is_null() {
        log::warn!("Windows does not let Leto take the keys that reach its window");
    }
    let read_before = read_keyboard();
    let mut asked = read_before.unwrap_or(RAWINPUTDEVICE {
        usUsagePage: GENERIC,
        usUsage: KEYBOARD,
        dwFlags: 0,
        hwndTarget: ptr::null_mut(),
    });
    asked.dwFlags |= RIDEV_NOHOTKEYS;
    if !register(&asked) {
        log::warn!("Windows does not leave the programs' shortcuts aside for Leto");
    }
    let mut gate = Gate::new();
    // What is down now went down for the window, and comes up there.
    // SAFETY: `GetAsyncKeyState` takes any number.
    gate.arm(|code| unsafe { GetAsyncKeyState(i32::from(code)) } < 0);
    KEPT.set(Some(Kept { hook, gate, read_before, taken: 0 }));
}

/// Gives the window its keys back, and Windows its shortcuts.
pub(super) fn disarm() {
    let Some(kept) = KEPT.take() else { return };
    if !kept.hook.is_null() {
        // SAFETY: a hook set by `arm`, removed once.
        unsafe { UnhookWindowsHookEx(kept.hook) };
    }
    let before = kept.read_before.unwrap_or(RAWINPUTDEVICE {
        usUsagePage: GENERIC,
        usUsage: KEYBOARD,
        dwFlags: RIDEV_REMOVE,
        hwndTarget: ptr::null_mut(),
    });
    if !register(&before) {
        log::warn!("Windows did not take back how Leto read the keyboard");
    }
    if kept.taken > 0 {
        log::debug!("{} keys reached Leto's window, and went to the stream from there", kept.taken);
    }
}

/// How this program reads the keyboard itself, if it does: winit does, for
/// its device events.
fn read_keyboard() -> Option<RAWINPUTDEVICE> {
    let size = size_of::<RAWINPUTDEVICE>() as u32;
    let mut count = 0;
    // SAFETY: no array, to be told how long it must be.
    unsafe { GetRegisteredRawInputDevices(ptr::null_mut(), &mut count, size) };
    let mut devices = vec![RAWINPUTDEVICE::default(); count as usize];
    // SAFETY: an array of `count` devices, and the size of one.
    let got = unsafe { GetRegisteredRawInputDevices(devices.as_mut_ptr(), &mut count, size) };
    devices.truncate(usize::try_from(got).ok()?);
    devices.into_iter().find(|device| device.usUsagePage == GENERIC && device.usUsage == KEYBOARD)
}

fn register(device: &RAWINPUTDEVICE) -> bool {
    // SAFETY: one device, and its size.
    unsafe { RegisterRawInputDevices(device, 1, size_of::<RAWINPUTDEVICE>() as u32) != 0 }
}

/// Called by Windows for each message the window's thread takes from its
/// queue, before anything is done with it: a key for the stream becomes a
/// message that says nothing.
unsafe extern "system" fn heard(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // A message only looked at is heard again when it is taken.
    if code == HC_ACTION as i32 && wparam == PM_REMOVE as usize {
        // SAFETY: for WH_GETMESSAGE, `lparam` points to the message taken,
        // which the hook may change.
        let message = unsafe { &mut *(lparam as *mut MSG) };
        if matches!(message.message, WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP) {
            let raw = at_window((message.wParam & 0xFFFF) as u16, message.lParam as u32);
            let made_up = made_up(message, raw);
            if keep(raw, made_up) {
                *message = MSG { message: WM_NULL, ..*message };
            }
        }
    }
    // SAFETY: passing on what Windows gave, as it asks of every hook.
    unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) }
}

/// Whether `raw`, from `message`, is the Ctrl AltGr makes up: the next key
/// waiting is AltGr, pressed at the same moment.
fn made_up(message: &MSG, raw: Raw) -> bool {
    if !raw.down || raw.virtual_key != 0xA2 {
        return false;
    }
    let mut next = MSG::default();
    // SAFETY: `next` is a valid MSG to write to; it is only looked at.
    let waiting =
        unsafe { PeekMessageW(&mut next, message.hwnd, WM_KEYFIRST, WM_KEYLAST, PM_NOREMOVE) } != 0;
    let next_raw = (waiting && matches!(next.message, WM_KEYDOWN | WM_SYSKEYDOWN))
        .then(|| at_window((next.wParam & 0xFFFF) as u16, next.lParam as u32));
    made_up_at_window(raw, next_raw, next.time == message.time)
}

/// Decides what becomes of a key that reached the window, and whether the
/// window is to hear of it.
fn keep(raw: Raw, made_up: bool) -> bool {
    let verdict = KEPT.with(|kept| {
        let mut kept = kept.try_borrow_mut().ok()?;
        let kept = kept.as_mut()?;
        if made_up {
            return Some(Verdict::Drop);
        }
        let verdict = kept.gate.decide(raw, true);
        if matches!(verdict, Verdict::Take(_)) {
            kept.taken = kept.taken.saturating_add(1);
        }
        Some(verdict)
    });
    match verdict {
        Some(Verdict::Take(stroke)) => {
            super::taken(stroke);
            // The hook should have had it: it is set again.
            if raw.down {
                super::slipped();
            }
            true
        }
        Some(Verdict::Drop) => true,
        Some(Verdict::PassOn) | None => false,
    }
}
