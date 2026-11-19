//! Listening to the keyboard ahead of Windows: a low-level keyboard hook.
//!
//! Windows calls the hook for every key of every program, on the thread that
//! set it, which must be waiting for messages meanwhile. Windows waits for
//! its answer only so long: past that, it hands the key on as if the hook
//! had let it go, so that the PC and this device would both act on it, and
//! it removes the hook without a word, so that every key after it reaches
//! this device too. Leto therefore gives the hook a thread of its own, at a
//! high priority, that does nothing else, and the hook only decides: it
//! waits for no other thread, and telling the window that a key came is the
//! work of another one.
//!
//! The hook is set while a stream has the keyboard and Leto's window is in
//! front, and set again each time the window comes back in front: Windows
//! asks the hook set last first, and another program may have set one
//! since. It is set again too when a key gets past it to the window, when a
//! key reached it late, and every second besides, since Windows says
//! nothing of a hook it removed. The new one is set before the old one goes,
//! and the old one goes half a second later, so that no key gets through in
//! between, not even one Windows was handing to the old one.
//!
//! Where Windows never calls the hook all the same, the keys that reach
//! Leto's window are taken there (`windows/window.rs`).

use std::{
    cell::{Cell, RefCell},
    ptr,
    sync::{
        Arc, Mutex, MutexGuard, OnceLock, PoisonError,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
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
            HHOOK, KBDLLHOOKSTRUCT, KillTimer, LLKHF_EXTENDED, LLKHF_UP, MSG, PM_NOREMOVE,
            PeekMessageW, PostThreadMessageW, SetTimer, SetWindowsHookExW, UnhookWindowsHookEx,
            WH_KEYBOARD_LL, WM_APP, WM_TIMER, WM_USER,
        },
    },
};

use super::{Gate, Raw, Stroke, Verdict};

mod window;

/// What the hook's thread is asked to do.
const ARM: u32 = WM_APP + 1;
const DISARM: u32 = WM_APP + 2;
/// Set the hook again, ahead of any another program set since.
const REARM: u32 = WM_APP + 3;

/// How often a key getting past the hook has it set again, at most.
const REARM_EVERY: Duration = Duration::from_secs(1);

/// How often the hook is set again anyway while it is set.
const RENEW_EVERY_MS: u32 = 1000;

/// How long a key may wait for the hook before Windows may have given up on
/// it: the hook is then set again at once.
const LATE: Duration = Duration::from_millis(100);

/// How long a hook stays set once another replaced it: Windows may be
/// handing it a key at that very moment, which would otherwise go past both.
const RETIRED_FOR: Duration = Duration::from_millis(500);

type Wake = Arc<dyn Fn() + Send + Sync>;

/// Whether the window asked for the keyboard last.
static WANTED: AtomicBool = AtomicBool::new(false);

/// Whether the hook is set and keeps keys from Windows. Only the hook's
/// thread changes it.
static CAPTURING: AtomicBool = AtomicBool::new(false);

/// Keys that got past the hook to the window since it was set.
static SLIPS: AtomicU32 = AtomicU32::new(0);

/// How many times the hook was set since the stream took the keyboard,
/// again included.
static SETS: AtomicU32 = AtomicU32::new(0);

/// Keys that waited so long for the hook since it was set that Windows may
/// have handed them on.
static LATE_KEYS: AtomicU32 = AtomicU32::new(0);

/// When a key getting past the hook last had it set again. Only the
/// window's thread uses it.
static REARMED: Mutex<Option<Instant>> = Mutex::new(None);

/// The keys taken, for the window. Only the window's thread locks it: the
/// hook only ever sends, which waits for nobody.
static TAKEN: OnceLock<Mutex<mpsc::Receiver<Stroke>>> = OnceLock::new();
static TO_TAKE: OnceLock<mpsc::Sender<Stroke>> = OnceLock::new();

/// Takes keys whatever window is in front. For the tests, which have none.
#[cfg(test)]
static ANYWHERE: AtomicBool = AtomicBool::new(false);

/// Who to tell that a key was taken. Only the thread that tells takes it.
static WAKE: Mutex<Option<Wake>> = Mutex::new(None);

/// Asks the thread that tells to do so. Never waits: one call waiting is
/// enough for any number of keys.
static NUDGE: OnceLock<mpsc::SyncSender<()>> = OnceLock::new();

/// The hook's thread, once started, or `None` if it could not be.
static THREAD: OnceLock<Option<u32>> = OnceLock::new();

/// What the hook knows, which only its thread ever touches.
struct Hook {
    gate: Gate,
    /// The longest a key waited for the hook's answer since it was set,
    /// from the moment it was pressed.
    slowest: Duration,
    /// The hook asked to be set again for a late key, and is not yet.
    renewing: bool,
}

thread_local! {
    static HOOK: RefCell<Hook> = const {
        RefCell::new(Hook {
            gate: Gate::new(),
            slowest: Duration::ZERO,
            renewing: false,
        })
    };
    /// The hook passed a key on, and hears of it again: Leto's hook is in
    /// the chain twice, for as long as it takes to set one and remove the
    /// other. A key is decided once.
    static PASSING_ON: Cell<bool> = const { Cell::new(false) };
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // What is shared stays usable even if a thread panicked holding it.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn capture(on: bool) -> bool {
    let Some(thread) = hook_thread() else { return false };
    if WANTED.swap(on, Ordering::SeqCst) != on {
        if on {
            *lock(&REARMED) = None;
            window::arm();
        } else {
            window::disarm();
        }
        ask(thread, if on { ARM } else { DISARM });
    }
    true
}

/// Hands a key taken to the window, and has it told.
fn taken(stroke: Stroke) {
    if let Some(to_take) = TO_TAKE.get() {
        to_take.send(stroke).ok();
    }
    if let Some(nudge) = NUDGE.get() {
        // Full: the window is already to be told.
        nudge.try_send(()).ok();
    }
}

pub fn slipped() {
    let Some(thread) = hook_thread() else { return };
    // Before the hook is set, a key may still reach the window.
    if !WANTED.load(Ordering::SeqCst) || !CAPTURING.load(Ordering::SeqCst) {
        return;
    }
    let slips = SLIPS.fetch_add(1, Ordering::SeqCst).saturating_add(1);
    let now = Instant::now();
    {
        let mut rearmed = lock(&REARMED);
        if rearmed.is_some_and(|at| now.duration_since(at) < REARM_EVERY) {
            return;
        }
        *rearmed = Some(now);
    }
    if slips == 1 {
        log::warn!("a key got past the hook to Leto's window, so the hook is set again");
    } else {
        log::debug!("{slips} keys got past the hook, which is set again");
    }
    ask(thread, REARM);
}

pub fn take() -> Vec<Stroke> {
    TAKEN.get().map(|taken| lock(taken).try_iter().collect()).unwrap_or_default()
}

pub fn set_wake(wake: impl Fn() + Send + Sync + 'static) {
    *lock(&WAKE) = Some(Arc::new(wake));
}

/// What became of the keys since the hook was set, for the logs: how many,
/// never which.
fn report(hook: &Hook) -> String {
    format!(
        "{} keys for the stream, {} for another window in front, {} past the hook set {} \
         times; a key waited {} ms at most for the hook, {} past {} ms",
        hook.gate.tally.taken,
        hook.gate.tally.elsewhere,
        SLIPS.load(Ordering::SeqCst),
        SETS.load(Ordering::SeqCst),
        hook.slowest.as_millis(),
        LATE_KEYS.load(Ordering::SeqCst),
        LATE.as_millis()
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
                let wake = lock(&WAKE).clone();
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

        let (to_take, taken) = mpsc::channel();
        TAKEN.set(Mutex::new(taken)).ok();
        TO_TAKE.set(to_take).ok();
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
    // Hooks replaced by a newer one, and since when.
    let mut retired: Vec<(HHOOK, Instant)> = Vec::new();
    let mut timer = 0;
    // SAFETY: `message` is a valid MSG to write to. Zero is the end, and a
    // negative answer a failure.
    while unsafe { GetMessageW(&mut message, ptr::null_mut(), 0, 0) } > 0 {
        match message.message {
            ARM if hook.is_null() => {
                hook = set_hook();
                if hook.is_null() {
                    log::warn!("Windows does not let Leto have the keyboard");
                    continue;
                }
                HOOK.with_borrow_mut(|state| {
                    // What is down now came down before the hook heard of it.
                    // SAFETY: `GetAsyncKeyState` takes any number.
                    state.gate.arm(|code| unsafe { GetAsyncKeyState(i32::from(code)) } < 0);
                    state.slowest = Duration::ZERO;
                    state.renewing = false;
                });
                SLIPS.store(0, Ordering::SeqCst);
                LATE_KEYS.store(0, Ordering::SeqCst);
                SETS.store(1, Ordering::SeqCst);
                CAPTURING.store(true, Ordering::SeqCst);
                // SAFETY: a timer of this thread, with no window and no
                // procedure: Windows posts WM_TIMER to the thread.
                timer = unsafe { SetTimer(ptr::null_mut(), 0, RENEW_EVERY_MS, None) };
                log::debug!("the stream has the keyboard");
            }
            REARM | WM_TIMER if !hook.is_null() => {
                // The new one first: the old one still answers meanwhile.
                let renewed = set_hook();
                HOOK.with_borrow_mut(|state| state.renewing = false);
                if renewed.is_null() {
                    log::debug!("Windows did not let the hook be set again");
                    continue;
                }
                retired.push((hook, Instant::now()));
                hook = renewed;
                SETS.fetch_add(1, Ordering::SeqCst);
                retired.retain(|&(old, since)| {
                    let kept = since.elapsed() < RETIRED_FOR;
                    if !kept {
                        unset_hook(old);
                    }
                    kept
                });
            }
            DISARM if !hook.is_null() => {
                unset_hook(hook);
                hook = ptr::null_mut();
                for (old, _) in retired.drain(..) {
                    unset_hook(old);
                }
                if timer != 0 {
                    // SAFETY: the timer set when the hook was.
                    unsafe { KillTimer(ptr::null_mut(), timer) };
                    timer = 0;
                }
                CAPTURING.store(false, Ordering::SeqCst);
                let (report, slow) = HOOK.with_borrow_mut(|state| {
                    let said = (report(state), LATE_KEYS.load(Ordering::SeqCst) > 0);
                    state.gate.disarm();
                    said
                });
                if slow {
                    log::warn!(
                        "the keyboard is the system's again; keys waited so long for the hook \
                         that Windows may have handed them on: {report}"
                    );
                } else {
                    log::debug!("the keyboard is the system's again: {report}");
                }
            }
            _ => {}
        }
    }
}

/// Sets the hook, ahead of every other, or says `null`.
fn set_hook() -> HHOOK {
    // SAFETY: a hook procedure that matches HOOKPROC, of this program, for
    // every thread of the desktop.
    unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hear), GetModuleHandleW(ptr::null()), 0) }
}

fn unset_hook(hook: HHOOK) {
    // SAFETY: a hook set by `set_hook`, removed once.
    unsafe { UnhookWindowsHookEx(hook) };
}

/// The hook: called by Windows for each key, before anyone else hears of it.
/// Returning a number other than zero keeps the key from going further.
///
/// It only decides, as fast as it can, and waits for no other thread.
unsafe extern "system" fn hear(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && !PASSING_ON.get() {
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
        #[cfg(test)]
        let in_front = ANYWHERE.load(Ordering::Relaxed) || in_front();
        #[cfg(not(test))]
        let in_front = in_front();
        // Only this thread ever borrows it; a key that comes while it does
        // is not Windows' to keep waiting.
        let verdict = HOOK.with(|state| {
            let mut state = state.try_borrow_mut().ok()?;
            state.heard(waited);
            Some(state.gate.decide(raw, in_front))
        });
        match verdict {
            Some(Verdict::Take(stroke)) => {
                taken(stroke);
                return 1;
            }
            Some(Verdict::Drop) => return 1,
            Some(Verdict::PassOn) | None => {}
        }
        PASSING_ON.set(true);
        // SAFETY: passing on what Windows gave, as it asks of every hook.
        let answer = unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) };
        PASSING_ON.set(false);
        return answer;
    }
    // SAFETY: passing on what Windows gave, as it asks of every hook.
    unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) }
}

impl Hook {
    /// A key reached the hook `waited` milliseconds after it was pressed.
    fn heard(&mut self, waited: u32) {
        // A key the tick count wrapped around on, or one Windows stamped
        // late, says nothing.
        if waited >= 60_000 {
            return;
        }
        let waited = Duration::from_millis(u64::from(waited));
        self.slowest = self.slowest.max(waited);
        if waited < LATE || !CAPTURING.load(Ordering::Relaxed) {
            return;
        }
        LATE_KEYS.fetch_add(1, Ordering::SeqCst);
        if !self.renewing {
            // Windows may have removed the hook meanwhile: it is set again
            // at once, and not only within a second.
            self.renewing = true;
            // SAFETY: no arguments, and a message with no pointers in it
            // posted to this very thread.
            unsafe { PostThreadMessageW(GetCurrentThreadId(), REARM, 0, 0) };
        }
    }
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
    use std::sync::atomic::AtomicUsize;

    use leto_core::input::Key;
    use windows_sys::Win32::UI::{
        Input::KeyboardAndMouse::{
            INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
            KEYEVENTF_SCANCODE, SendInput,
        },
        WindowsAndMessaging::{PM_REMOVE, WM_KEYDOWN, WM_KEYUP, WM_NULL},
    };

    use super::*;

    /// A key pressed or let go of as a keyboard would, by its scan code, at
    /// `time` on Windows' tick count, or now for zero.
    fn key_at_time(scan: u16, extended: bool, down: bool, time: u32) -> INPUT {
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
                ki: KEYBDINPUT { wVk: 0, wScan: scan, dwFlags: flags, time, dwExtraInfo: 0 },
            },
        }
    }

    fn key(scan: u16, extended: bool, down: bool) -> INPUT {
        key_at_time(scan, extended, down, 0)
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

    fn sets() -> u32 {
        SETS.load(Ordering::SeqCst)
    }

    /// A key reaching a window of this thread, as Windows would post it,
    /// and the message the thread then takes.
    fn at_window(virtual_key: u16, scan: u16, extended: bool, down: bool) -> u32 {
        let mut flags = 1 | u32::from(scan) << 16;
        if extended {
            flags |= 1 << 24;
        }
        if !down {
            flags |= 1 << 30 | 1 << 31;
        }
        let message = if down { WM_KEYDOWN } else { WM_KEYUP };
        // SAFETY: a message with no pointers in it, to this very thread.
        let posted = unsafe {
            PostThreadMessageW(GetCurrentThreadId(), message, virtual_key.into(), flags as isize)
        };
        assert_ne!(posted, 0, "the key was not posted");
        let mut taken = MSG::default();
        // SAFETY: `taken` is a valid MSG to write to.
        let got = unsafe { PeekMessageW(&mut taken, ptr::null_mut(), 0, 0, PM_REMOVE) };
        assert_ne!(got, 0, "the key never came");
        taken.message
    }

    #[test]
    fn windows_says_what_its_layout_prints_on_a_key() {
        // The runners type in English: the key next to Tab prints Q.
        assert_eq!(letter_at(0x10), Some('Q'));
        assert_eq!(letter_at(0x1E), Some('A'));
        // A key that prints no letter.
        assert_eq!(letter_at(0x02), None);
    }

    /// One test, since there is one keyboard and one hook.
    #[test]
    fn the_windows_key_goes_to_the_stream_and_not_to_windows() {
        static WOKEN: AtomicUsize = AtomicUsize::new(0);
        set_wake(|| {
            WOKEN.fetch_add(1, Ordering::SeqCst);
        });
        ANYWHERE.store(true, Ordering::SeqCst);
        assert!(capture(true));
        // The hook is set by its thread, in a moment.
        until("the hook being set", || CAPTURING.load(Ordering::SeqCst));

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
        // as before, once each although the hook was in the chain twice for
        // a moment.
        let before = sets();
        slipped();
        until("the hook being set again", || sets() > before);
        assert_eq!(SLIPS.load(Ordering::SeqCst), 1);
        send(&[key(0x1E, false, true), key(0x1E, false, false)]);
        assert_eq!(heard(2), [(a, true), (a, false)]);

        // It is set again every second besides, since Windows says nothing
        // of a hook it removed.
        let before = sets();
        until("the hook being set again in time", || sets() > before);
        send(&[key(0x5B, true, true), key(0x5B, true, false)]);
        assert_eq!(heard(2), [(Key::LEFT_SUPER, true), (Key::LEFT_SUPER, false)]);

        // A key that waited long for the hook has it set again at once, and
        // the log says so.
        // SAFETY: no arguments.
        let long_ago = unsafe { GetTickCount() }.wrapping_sub(500);
        send(&[key_at_time(0x1E, false, true, long_ago), key_at_time(0x1E, false, false, 0)]);
        assert_eq!(heard(2), [(a, true), (a, false)]);
        assert_eq!(LATE_KEYS.load(Ordering::SeqCst), 1);

        // Where Windows never calls the hook, a key reaches the window: it
        // is taken there, before the window acts on it, and the hook is set
        // again.
        let before = sets();
        assert_eq!(at_window(0x5B, 0x5B, true, true), WM_NULL);
        assert_eq!(at_window(0x5B, 0x5B, true, false), WM_NULL);
        assert_eq!(heard(2), [(Key::LEFT_SUPER, true), (Key::LEFT_SUPER, false)]);
        until("the hook being set again for a key that got past", || sets() > before);
        assert_eq!(SLIPS.load(Ordering::SeqCst), 2);

        capture(false);
        until("the hook being removed", || !CAPTURING.load(Ordering::SeqCst));
        ANYWHERE.store(false, Ordering::SeqCst);
        // The window hears of its keys again.
        assert_eq!(at_window(0x5B, 0x5B, true, true), WM_KEYDOWN);
        assert_eq!(at_window(0x5B, 0x5B, true, false), WM_KEYUP);
        assert!(take().is_empty());
    }
}
