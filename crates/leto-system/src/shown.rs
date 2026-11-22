//! A window put away and brought back, for a program that lives on beside
//! the clock once its window is closed.
//!
//! The window is hidden and shown by Windows itself, behind the back of
//! what draws in it: a hidden window is not drawn, so whatever would bring
//! it back from inside is not running. Elsewhere these do nothing.

/// Takes the window off the screen and off the taskbar. `window` is its
/// handle, as Windows gave it.
pub fn hide(window: isize) {
    #[cfg(windows)]
    windows::hide(window);
    #[cfg(not(windows))]
    let _ = window;
}

/// Puts the window back on the screen, in front of the others.
pub fn show(window: isize) {
    #[cfg(windows)]
    windows::show(window);
    #[cfg(not(windows))]
    let _ = window;
}

#[cfg(windows)]
mod windows {
    use windows_sys::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{
            IsIconic, SW_HIDE, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindow,
        },
    };

    pub fn hide(window: isize) {
        // SAFETY: a handle that is no window's makes these calls fail, and
        // nothing else: they take no pointer of Leto's.
        unsafe {
            ShowWindow(window as HWND, SW_HIDE);
        }
    }

    pub fn show(window: isize) {
        // SAFETY: as in `hide`.
        unsafe {
            let window = window as HWND;
            ShowWindow(window, if IsIconic(window) != 0 { SW_RESTORE } else { SW_SHOW });
            SetForegroundWindow(window);
        }
    }
}
