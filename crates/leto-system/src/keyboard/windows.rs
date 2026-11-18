//! Listening to the keyboard ahead of Windows: a low-level keyboard hook.
//!
//! Windows calls the hook for every key of every program, on the thread that
//! set it, which must be waiting for messages meanwhile. Leto gives it a
//! thread of its own that does nothing else, so that a busy window never
//! keeps the keyboard of the whole system waiting: Windows gives up on a hook
//! that takes too long, and removes one that keeps doing so.
//!
//! The hook is only set while a stream has the keyboard.

use std::{
    ptr,
    sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, mpsc},
    thread,
};

use windows_sys::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    System::{
        LibraryLoader::GetModuleHandleW,
        Threading::{GetCurrentProcessId, GetCurrentThreadId},
    },
    UI::{
        Input::KeyboardAndMouse::GetAsyncKeyState,
        WindowsAndMessaging::{
            CallNextHookEx, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId, HC_ACTION,
            HHOOK, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_UP, MSG, PM_NOREMOVE, PeekMessageW,
            PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_APP,
            WM_USER,
        },
    },
};

use super::{Gate, Raw, Stroke, Verdict};

/// What the hook's thread is asked to do.
const ARM: u32 = WM_APP + 1;
const DISARM: u32 = WM_APP + 2;

type Wake = Arc<dyn Fn() + Send + Sync>;

/// What the hook and the window share.
struct Shared {
    gate: Gate,
    /// Taken, and not yet handed to the window.
    strokes: Vec<Stroke>,
    wake: Option<Wake>,
    /// Whether the window asked for the keyboard last.
    wanted: bool,
    /// Takes keys whatever window is in front. For the tests, which have
    /// none.
    #[cfg(test)]
    anywhere: bool,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared {
    gate: Gate::new(),
    strokes: Vec::new(),
    wake: None,
    wanted: false,
    #[cfg(test)]
    anywhere: false,
});

/// The hook's thread, once started, or `None` if it could not be.
static THREAD: OnceLock<Option<u32>> = OnceLock::new();

fn shared() -> MutexGuard<'static, Shared> {
    // What is shared stays usable even if a thread panicked holding it.
    SHARED.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn capture(on: bool) -> bool {
    let Some(thread) = hook_thread() else { return false };
    let mut shared = shared();
    if shared.wanted != on {
        shared.wanted = on;
        let asked = if on { ARM } else { DISARM };
        // SAFETY: posting a message with no pointers in it to a thread.
        if unsafe { PostThreadMessageW(thread, asked, 0, 0) } == 0 {
            log::warn!("the keyboard's thread did not take a message");
        }
    }
    true
}

pub fn take() -> Vec<Stroke> {
    std::mem::take(&mut shared().strokes)
}

pub fn set_wake(wake: impl Fn() + Send + Sync + 'static) {
    shared().wake = Some(Arc::new(wake));
}

/// Starts the hook's thread the first time, and waits until it can be
/// asked things.
fn hook_thread() -> Option<u32> {
    *THREAD.get_or_init(|| {
        let (ready, started) = mpsc::channel();
        let spawned =
            thread::Builder::new().name("leto: keyboard".to_owned()).spawn(move || listen(&ready));
        if let Err(error) = spawned {
            log::warn!("no thread for the keyboard: {error}");
            return None;
        }
        started.recv().ok()
    })
}

/// The hook's thread: sets and removes the hook when asked, and waits for
/// messages meanwhile, which is when Windows calls the hook.
fn listen(ready: &mpsc::Sender<u32>) {
    let mut message = MSG::default();
    // SAFETY: `message` is a valid MSG to write to. Peeking makes the
    // thread's message queue, without which nothing can be posted to it.
    unsafe { PeekMessageW(&mut message, ptr::null_mut(), WM_USER, WM_USER, PM_NOREMOVE) };
    // SAFETY: no arguments.
    if ready.send(unsafe { GetCurrentThreadId() }).is_err() {
        return;
    }
    let mut hook: HHOOK = ptr::null_mut();
    // SAFETY: `message` is a valid MSG to write to. Zero is the end, and a
    // negative answer a failure.
    while unsafe { GetMessageW(&mut message, ptr::null_mut(), 0, 0) } > 0 {
        match message.message {
            ARM if hook.is_null() => {
                // SAFETY: a hook procedure that matches HOOKPROC, of this
                // program, for every thread of the desktop.
                hook = unsafe {
                    SetWindowsHookExW(WH_KEYBOARD_LL, Some(hear), GetModuleHandleW(ptr::null()), 0)
                };
                if hook.is_null() {
                    log::warn!("Windows does not let Leto have the keyboard");
                    continue;
                }
                // What is down now came down before the hook heard of it.
                // SAFETY: `GetAsyncKeyState` takes any number.
                shared().gate.arm(|code| unsafe { GetAsyncKeyState(i32::from(code)) } < 0);
                log::debug!("the stream has the keyboard");
            }
            DISARM if !hook.is_null() => {
                // SAFETY: the hook set above, removed once.
                unsafe { UnhookWindowsHookEx(hook) };
                hook = ptr::null_mut();
                shared().gate.disarm();
                log::debug!("the keyboard is the system's again");
            }
            _ => {}
        }
    }
}

/// The hook: called by Windows for each key, before anyone else hears of it.
/// Returning a number other than zero keeps the key from going further.
unsafe extern "system" fn hear(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, `lparam` points to the key's description.
        let event = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let raw = Raw {
            virtual_key: (event.vkCode & 0xFFFF) as u16,
            scan: (event.scanCode & 0xFFFF) as u16,
            extended: event.flags & LLKHF_EXTENDED != 0,
            down: event.flags & LLKHF_UP == 0,
        };
        let mut shared = shared();
        #[cfg(test)]
        let in_front = shared.anywhere || in_front();
        #[cfg(not(test))]
        let in_front = in_front();
        match shared.gate.decide(raw, in_front) {
            Verdict::Take(stroke) => {
                shared.strokes.push(stroke);
                let wake = shared.wake.clone();
                drop(shared);
                if let Some(wake) = wake {
                    wake();
                }
                return 1;
            }
            Verdict::Drop => return 1,
            Verdict::PassOn => {}
        }
    }
    // SAFETY: passing on what Windows gave, as it asks of every hook.
    unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) }
}

/// Whether a window of this program is the one in front, the one that gets
/// the keys.
fn in_front() -> bool {
    // SAFETY: no arguments.
    let window = unsafe { GetForegroundWindow() };
    if window.is_null() {
        return false;
    }
    let mut process = 0;
    // SAFETY: `window` is a window handle, and `process` a number to write.
    unsafe { GetWindowThreadProcessId(window, &mut process) };
    // SAFETY: no arguments.
    process == unsafe { GetCurrentProcessId() }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::time::{Duration, Instant};

    use leto_core::input::Key;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
        KEYEVENTF_SCANCODE, SendInput,
    };

    use super::*;

    /// A key pressed or let go of as a keyboard would, by its scan code.
    fn key(scan: u16, extended: bool, down: bool) -> INPUT {
        let mut flags = KEYEVENTF_SCANCODE;
        if extended {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        if !down {
            flags |= KEYEVENTF_KEYUP;
        }
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT { wVk: 0, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 },
            },
        }
    }

    #[test]
    fn the_windows_key_goes_to_the_stream_and_not_to_windows() {
        shared().anywhere = true;
        assert!(capture(true));
        // The hook is set by its thread, in a moment.
        let start = Instant::now();
        while !shared().gate.capturing {
            assert!(start.elapsed() < Duration::from_secs(5), "the hook was never set");
            thread::sleep(Duration::from_millis(10));
        }

        // Windows+A, which would open the quick settings.
        let presses = [
            key(0x5B, true, true),
            key(0x1E, false, true),
            key(0x1E, false, false),
            key(0x5B, true, false),
        ];
        // SAFETY: an array of keyboard inputs, and the size of one.
        let sent =
            unsafe { SendInput(presses.len() as u32, presses.as_ptr(), size_of::<INPUT>() as i32) };
        assert_eq!(sent, presses.len() as u32, "Windows took no keys to send");

        let mut heard = Vec::new();
        let start = Instant::now();
        while heard.len() < presses.len() {
            assert!(start.elapsed() < Duration::from_secs(5), "only {heard:?} arrived");
            heard.extend(take().into_iter().map(|stroke| (stroke.key, stroke.down)));
            thread::sleep(Duration::from_millis(10));
        }
        let a = Key::letter('A').unwrap();
        assert_eq!(
            heard,
            [(Key::LEFT_SUPER, true), (a, true), (a, false), (Key::LEFT_SUPER, false)]
        );

        capture(false);
        shared().anywhere = false;
    }
}
