//! What the screen Leto's window is on shows: how many pixels, how often.

/// The mode of a screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mode {
    /// Pixels across and down.
    pub width: u32,
    pub height: u32,
    /// Pictures per second.
    pub refresh_hz: u32,
}

/// The paces screens are sold with. Windows says one less for those that
/// run a thousandth slower, as television does: 59 for 59.94.
const PACES: [u32; 13] = [30, 50, 60, 75, 90, 100, 120, 144, 165, 170, 180, 200, 240];

/// The pace a screen that reports `hz` is sold with.
#[cfg_attr(not(windows), allow(dead_code))]
fn pace(hz: u32) -> u32 {
    if PACES.contains(&(hz + 1)) { hz + 1 } else { hz }
}

/// The mode of the screen the active window of the calling thread is on, or
/// of the main screen when it has none. Call it from the thread of Leto's
/// window. `None` where the system is not asked.
pub fn of_active_window() -> Option<Mode> {
    #[cfg(windows)]
    return windows::of_active_window();
    #[cfg(not(windows))]
    None
}

#[cfg(windows)]
mod windows {
    use std::ptr;

    use windows_sys::Win32::{
        Graphics::Gdi::{
            DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW, GetMonitorInfoW,
            MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MONITORINFOEXW,
            MonitorFromWindow,
        },
        UI::Input::KeyboardAndMouse::GetActiveWindow,
    };

    use super::{Mode, pace};

    pub fn of_active_window() -> Option<Mode> {
        // SAFETY: no arguments.
        let window = unsafe { GetActiveWindow() };
        let fallback =
            if window.is_null() { MONITOR_DEFAULTTOPRIMARY } else { MONITOR_DEFAULTTONEAREST };
        // SAFETY: any window handle, or none, with a fallback.
        let monitor = unsafe { MonitorFromWindow(window, fallback) };
        if monitor.is_null() {
            return None;
        }
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        // SAFETY: `info` is the larger form of MONITORINFO, and says so.
        let found =
            unsafe { GetMonitorInfoW(monitor, ptr::from_mut(&mut info).cast::<MONITORINFO>()) };
        if found == 0 {
            return None;
        }
        let mut mode = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..DEVMODEW::default() };
        // SAFETY: the screen's name as Windows wrote it, ending with a zero,
        // and a DEVMODEW that says its size.
        let read = unsafe {
            EnumDisplaySettingsW(info.szDevice.as_ptr(), ENUM_CURRENT_SETTINGS, &mut mode)
        };
        if read == 0 || mode.dmPelsWidth == 0 || mode.dmPelsHeight == 0 {
            return None;
        }
        // 0 and 1 stand for the screen's own pace, unsaid.
        let refresh_hz =
            if mode.dmDisplayFrequency > 1 { pace(mode.dmDisplayFrequency) } else { 60 };
        Some(Mode { width: mode.dmPelsWidth, height: mode.dmPelsHeight, refresh_hz })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn windows_says_what_its_screen_shows() {
            // A machine with no screen at all has a stand-in one.
            let mode = of_active_window().expect("Windows has a screen");
            assert!(mode.width >= 640 && mode.height >= 480, "{mode:?}");
            assert!(mode.refresh_hz >= 24, "{mode:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_screen_a_thousandth_slower_is_asked_for_at_its_round_pace() {
        assert_eq!(pace(59), 60);
        assert_eq!(pace(60), 60);
        assert_eq!(pace(119), 120);
        assert_eq!(pace(143), 144);
        assert_eq!(pace(144), 144);
        assert_eq!(pace(164), 165);
        // A pace nobody sells is what it says.
        assert_eq!(pace(85), 85);
    }
}
