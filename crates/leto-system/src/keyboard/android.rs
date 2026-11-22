//! Android's own keyboard shortcuts, for the PC: the Windows key, which
//! opens the list of apps on a phone's desktop, Alt+Tab and the rest.
//!
//! Android 16's second quarterly release lets a window ask for them
//! (`WindowManager.LayoutParams.setKeyboardCaptureEnabled`, with the
//! `CAPTURE_KEYBOARD` permission, which needs no question): while the
//! window is in front, the keys reach it as any key does. Before that
//! release Android keeps them, and nothing here changes anything.
//!
//! A window's attributes are only changed on the thread that draws it, so
//! the app's Java class does it (`Screen.keys`).

use std::sync::atomic::{AtomicI8, Ordering};

use crate::java::{screen, sdk};

/// What was last asked of Android: -1 nothing yet, 0 its own, 1 Leto's.
static ASKED: AtomicI8 = AtomicI8::new(-1);

/// Asks the window for Android's own shortcuts while `on`, and gives them
/// back when not. Only a change is told.
pub fn capture(on: bool) {
    let wanted = i8::from(on);
    if ASKED.swap(wanted, Ordering::Relaxed) == wanted {
        return;
    }
    // The method came within Android 16 (API 36); before, there is none.
    if sdk() < 36 {
        return;
    }
    if let Err(error) = screen("keys", on) {
        log::warn!("Android was not asked for its own keyboard shortcuts: {error}");
    }
}
