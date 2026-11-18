//! Listening to the keyboard ahead of Windows: a low-level keyboard hook.
//!
//! Windows calls the hook for every key of every program, on the thread that
//! set it, which must be waiting for messages meanwhile. Windows waits for
//! its answer only so long: past that, it hands the key on as if the hook
//! had let it go, so that the PC and this device would both act on it. Leto
//! therefore gives the hook a thread of its own, at a high priority, that
//! does nothing else, and the hook only decides: telling the window that a
//! key came is the work of another thread.
//!
//! The hook is set while a stream has the keyboard and Leto's window is in
//! front, and set again each time the window comes back in front: Windows
//! asks the hook set last first, and another program may have set one
//! since. It is set again too when a key gets past it to the window.

use std::{
    ptr,
    sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, mpsc},
    thread,
    time::{Duration, Instant},
};

use windows_sys::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    System::{
        LibraryLoader::GetModuleHandleW,
        SystemInformation::GetTickCount,
        Threading::{
            GetCurrentProcessId, GetCurrentThread, GetCurrentThreadId, SetThreadPriority,
            THREAD_PRIORITY_HIGHEST,
        },
    },
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, GetKeyboardLayout, MAPVK_VSC_TO_VK, MapVirtualKeyExW,
        },
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
/// Set the hook again, ahead of any another program set since.
const REARM: u32 = WM_APP + 3;

/// How often a key getting past the hook has it set again, at most.
const REARM_EVERY: Duration = Duration::from_secs(1);

type Wake = Arc<dyn Fn() + Send + Sync>;

/// What the hook and the window share.
struct Shared {
    gate: Gate,
    /// Taken, and not yet handed to the window.
    strokes: Vec<Stroke>,
    /// Whether the window asked for the keyboard last.
    wanted: bool,
    /// The longest a key waited for the hook's answer since it was set,
    /// from the moment it was pressed.
    slowest: Duration,
    /// Keys that got past the hook to the window since it was set, and when
    /// that last had the hook set again.
    slips: u32,
    rearmed: Option<Instant>,
    /// How many times the hook was set, again included.
    sets: u32,
    /// Takes keys whatever window is in front. For the tests, which have
    /// none.
    #[cfg(test)]
    anywhere: bool,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared {
    gate: Gate::new(),
    strokes: Vec::new(),
    wanted: false,
    slowest: Duration::ZERO,
    slips: 0,
    rearmed: None,
    sets: 0,
    #[cfg(test)]
    anywhere: false,
});

/// Who to tell that a key was taken. Only the thread that tells takes it.
static WAKE: Mutex<Option<Wake>> = Mutex::new(None);

/// Asks the thread that tells to do so. Never waits: one call waiting is
/// enough for any number of keys.
static NUDGE: OnceLock<mpsc::SyncSender<()>> = OnceLock::new();

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
        ask(thread, if on { ARM } else { DISARM });
    }
    true
}

pub fn slipped() {
    let Some(thread) = hook_thread() else { return };
    let mut shared = shared();
    // Before the hook is set, a key may still reach the window.
    if !shared.wanted || !shared.gate.capturing {
        return;
    }
    shared.slips = shared.slips.saturating_add(1);
    let now = Instant::now();
    if shared.rearmed.is_some_and(|at| now.duration_since(at) < REARM_EVERY) {
        return;
    }
    shared.rearmed = Some(now);
    if shared.slips == 1 {
        log::warn!(
            "a key got past the hook to Leto's window, so the hook is set again ({})",
            report(&shared)
        );
    } else {
        log::debug!("the hook is set again ({})", report(&shared));
    }
    ask(thread, REARM);
}

pub fn take() -> Vec<Stroke> {
    std::mem::take(&mut shared().strokes)
}

pub fn set_wake(wake: impl Fn() + Send + Sync + 'static) {
    *WAKE.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(wake));
}

/// What became of the keys since the hook was set, for the logs: how many,
/// never which.
fn report(shared: &Shared) -> String {
    format!(
        "{} keys for the stream, {} for another window in front, {} past the hook set {} \
         times; a key waited {} ms at most for the hook",
        shared.gate.tally.taken,
        shared.gate.tally.elsewhere,
        shared.slips,
        shared.sets,
        shared.slowest.as_millis()
    )
}

fn ask(thread: u32, what: u32) {
    // SAFETY: posting a message with no pointers in it to a thread.
    if unsafe { PostThreadMessageW(thread, what, 0, 0) } == 0 {
        log::warn!("the keyboard's thread did not take a message");
    }
}

/// Starts the hook's thread and the thread that tells the window the first
/// time, and waits until the hook's can be asked things.
fn hook_thread() -> Option<u32> {
    *THREAD.get_or_init(|| {
        let (nudge, nudged) = mpsc::sync_channel(1);
        let telling = thread::Builder::new().name("leto: keys taken".to_owned()).spawn(move || {
            for () in nudged {
                let wake = WAKE.lock().unwrap_or_else(PoisonError::into_inner).clone();
                if let Some(wake) = wake {
                    wake();
                }
            }
        });
        if let Err(error) = telling {
            log::warn!("no thread to tell of keys: {error}");
            return None;
        }
        NUDGE.set(nudge).ok();

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
    // SAFETY: the pseudo handle of this very thread.
    if unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST) } == 0 {
        log::debug!("the keyboard's thread runs at the usual priority");
    }
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
                hook = set_hook();
                if hook.is_null() {
                    continue;
                }
                let mut shared = shared();
                // What is down now came down before the hook heard of it.
                // SAFETY: `GetAsyncKeyState` takes any number.
                shared.gate.arm(|code| unsafe { GetAsyncKeyState(i32::from(code)) } < 0);
                shared.slowest = Duration::ZERO;
                shared.slips = 0;
                shared.rearmed = None;
                shared.sets = 1;
                drop(shared);
                log::debug!("the stream has the keyboard");
            }
            REARM if !hook.is_null() => {
                unset_hook(hook);
                hook = set_hook();
                let mut shared = shared();
                if hook.is_null() {
                    shared.gate.disarm();
                } else {
                    shared.sets = shared.sets.saturating_add(1);
                }
            }
            DISARM if !hook.is_null() => {
                unset_hook(hook);
                hook = ptr::null_mut();
                let mut shared = shared();
                let report = report(&shared);
                let slow = shared.slowest > Duration::from_millis(100);
                shared.gate.disarm();
                drop(shared);
                if slow {
                    log::warn!("the keyboard is the system's again, Windows waited long: {report}");
                } else {
                    log::debug!("the keyboard is the system's again: {report}");
                }
            }
            _ => {}
        }
    }
}

/// Sets the hook, ahead of every other, or says why not.
fn set_hook() -> HHOOK {
    // SAFETY: a hook procedure that matches HOOKPROC, of this program, for
    // every thread of the desktop.
    let hook =
        unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hear), GetModuleHandleW(ptr::null()), 0) };
    if hook.is_null() {
        log::warn!("Windows does not let Leto have the keyboard");
    }
    hook
}

fn unset_hook(hook: HHOOK) {
    // SAFETY: a hook set by `set_hook`, removed once.
    unsafe { UnhookWindowsHookEx(hook) };
}

/// The hook: called by Windows for each key, before anyone else hears of it.
/// Returning a number other than zero keeps the key from going further.
///
/// It only decides, and as fast as it can.
unsafe extern "system" fn hear(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, `lparam` points to the key's description.
        let event = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        // SAFETY: no arguments.
        let waited = unsafe { GetTickCount() }.wrapping_sub(event.time);
        let raw = Raw {
            virtual_key: (event.vkCode & 0xFFFF) as u16,
            scan: (event.scanCode & 0xFFFF) as u16,
            extended: event.flags & LLKHF_EXTENDED != 0,
            down: event.flags & LLKHF_UP == 0,
        };
        let mut shared = shared();
        // A key the tick count wrapped around on, or one Windows stamped
        // late, says nothing.
        if waited < 60_000 {
            shared.slowest = shared.slowest.max(Duration::from_millis(u64::from(waited)));
        }
        #[cfg(test)]
        let in_front = shared.anywhere || in_front();
        #[cfg(not(test))]
        let in_front = in_front();
        match shared.gate.decide(raw, in_front) {
            Verdict::Take(stroke) => {
                shared.strokes.push(stroke);
                drop(shared);
                if let Some(nudge) = NUDGE.get() {
                    // Full: the window is already to be told.
                    nudge.try_send(()).ok();
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

pub fn letter_at(scan: u16) -> Option<char> {
    // SAFETY: no arguments: the layout of the calling thread.
    let layout = unsafe { GetKeyboardLayout(0) };
    // SAFETY: any number, with a layout Windows gave.
    let virtual_key = unsafe { MapVirtualKeyExW(u32::from(scan), MAPVK_VSC_TO_VK, layout) };
    u8::try_from(virtual_key).ok().filter(u8::is_ascii_uppercase).map(char::from)
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
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    fn send(presses: &[INPUT]) {
        // SAFETY: an array of keyboard inputs, and the size of one.
        let sent =
            unsafe { SendInput(presses.len() as u32, presses.as_ptr(), size_of::<INPUT>() as i32) };
        assert_eq!(sent, presses.len() as u32, "Windows took no keys to send");
    }

    fn heard(count: usize) -> Vec<(Key, bool)> {
        let mut heard = Vec::new();
        let start = Instant::now();
        while heard.len() < count {
            assert!(start.elapsed() < Duration::from_secs(5), "only {heard:?} arrived");
            heard.extend(take().into_iter().map(|stroke| (stroke.key, stroke.down)));
            thread::sleep(Duration::from_millis(10));
        }
        heard
    }

    fn until(what: &str, done: impl Fn() -> bool) {
        let start = Instant::now();
        while !done() {
            assert!(start.elapsed() < Duration::from_secs(5), "{what} never happened");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn windows_says_what_its_layout_prints_on_a_key() {
        // The runners type in English: the key next to Tab prints Q.
        assert_eq!(letter_at(0x10), Some('Q'));
        assert_eq!(letter_at(0x1E), Some('A'));
        // A key that prints no letter.
        assert_eq!(letter_at(0x02), None);
    }

    #[test]
    fn the_windows_key_goes_to_the_stream_and_not_to_windows() {
        static WOKEN: AtomicUsize = AtomicUsize::new(0);
        set_wake(|| {
            WOKEN.fetch_add(1, Ordering::SeqCst);
        });
        shared().anywhere = true;
        assert!(capture(true));
        // The hook is set by its thread, in a moment.
        until("the hook being set", || shared().gate.capturing);

        // Windows+A, which would open the quick settings.
        send(&[
            key(0x5B, true, true),
            key(0x1E, false, true),
            key(0x1E, false, false),
            key(0x5B, true, false),
        ]);
        let a = Key::letter('A').unwrap();
        assert_eq!(
            heard(4),
            [(Key::LEFT_SUPER, true), (a, true), (a, false), (Key::LEFT_SUPER, false)]
        );
        // The window was told, by another thread than the hook's.
        until("the window being told", || WOKEN.load(Ordering::SeqCst) > 0);

        // A key that got past the hook has it set again, and keys are taken
        // as before.
        slipped();
        until("the hook being set again", || shared().sets == 2);
        assert_eq!(shared().slips, 1);
        send(&[key(0x1E, false, true), key(0x1E, false, false)]);
        assert_eq!(heard(2), [(a, true), (a, false)]);

        capture(false);
        until("the hook being removed", || !shared().gate.capturing);
        shared().anywhere = false;
    }
}
