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
#[cfg_attr(not(any(windows, target_os = "android")), allow(dead_code))]
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

/// The mode of the screen Leto's window is on when that screen is plugged
/// into this device rather than its own: on Android, a monitor the phone
/// was plugged into, with the system's desktop on it. `None` on the
/// phone's own screen, and always elsewhere.
pub fn plugged() -> Option<Mode> {
    #[cfg(target_os = "android")]
    return android::of_window().and_then(|(mode, plugged)| plugged.then_some(mode));
    #[cfg(not(target_os = "android"))]
    None
}

#[cfg(target_os = "android")]
mod android {
    use std::{
        sync::{Mutex, PoisonError},
        time::{Duration, Instant},
    };

    use jni::{JNIEnv, errors::Result, objects::JObject};

    use super::{Mode, pace};
    use crate::java::{sdk, with_java};

    /// `Display.DEFAULT_DISPLAY`: the phone's own screen.
    const OWN_SCREEN: i32 = 0;

    /// A screen's mode, and whether it is not the phone's own.
    type Said = (Mode, bool);

    /// How long what Java said is kept: the app asks every frame, and a
    /// window moves from screen to screen rarely.
    const KEPT: Duration = Duration::from_millis(500);

    /// The mode of the screen the activity is on, and whether that screen
    /// is not the phone's own.
    pub fn of_window() -> Option<Said> {
        static LAST: Mutex<Option<(Instant, Option<Said>)>> = Mutex::new(None);
        let mut last = LAST.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((when, said)) = *last
            && when.elapsed() < KEPT
        {
            return said;
        }
        let said = ask();
        *last = Some((Instant::now(), said));
        said
    }

    fn ask() -> Option<Said> {
        let read = with_java(|env, activity| {
            let display = display(env, activity)?;
            if display.is_null() {
                return Ok(None);
            }
            let id = env.call_method(&display, "getDisplayId", "()I", &[])?.i()?;
            let mode =
                env.call_method(&display, "getMode", "()Landroid/view/Display$Mode;", &[])?.l()?;
            let width = env.call_method(&mode, "getPhysicalWidth", "()I", &[])?.i()?;
            let height = env.call_method(&mode, "getPhysicalHeight", "()I", &[])?.i()?;
            let mut hz = env.call_method(&mode, "getRefreshRate", "()F", &[])?.f()?;
            // A monitor is often left at 60 pictures a second when it shows
            // more: the fastest of its modes with these pixels is the one
            // a stream asks Android for (`Screen.pace`), so the one to ask
            // the PC for.
            if id != OWN_SCREEN {
                let modes = env
                    .call_method(
                        &display,
                        "getSupportedModes",
                        "()[Landroid/view/Display$Mode;",
                        &[],
                    )?
                    .l()?;
                let modes = jni::objects::JObjectArray::from(modes);
                for at in 0..env.get_array_length(&modes)? {
                    let other = env.get_object_array_element(&modes, at)?;
                    let same = env.call_method(&other, "getPhysicalWidth", "()I", &[])?.i()?
                        == width
                        && env.call_method(&other, "getPhysicalHeight", "()I", &[])?.i()? == height;
                    if same {
                        hz = hz.max(env.call_method(&other, "getRefreshRate", "()F", &[])?.f()?);
                    }
                    env.delete_local_ref(other)?;
                }
            }
            Ok(Some((width, height, hz, id)))
        });
        match read {
            Ok(Some((width, height, hz, id))) if width > 0 && height > 0 => {
                // 59.94 is told as 59.94: the pace sold is the one above.
                let hz = hz.max(1.0).floor() as u32;
                let mode = Mode {
                    width: width as u32,
                    height: height as u32,
                    refresh_hz: pace(hz).max(24),
                };
                static TOLD: Mutex<Option<Said>> = Mutex::new(None);
                let said = (mode, id != OWN_SCREEN);
                let mut told = TOLD.lock().unwrap_or_else(PoisonError::into_inner);
                if *told != Some(said) {
                    *told = Some(said);
                    log::info!(
                        "the window is on screen {id}, {width} by {height}, at most {} pictures a second",
                        mode.refresh_hz
                    );
                }
                Some(said)
            }
            Ok(_) => None,
            Err(error) => {
                log::warn!("the screen's mode is unknown: {error}");
                None
            }
        }
    }

    /// The screen the activity is on. `Activity.getDisplay` came with
    /// Android 11; before, an app was only ever on the phone's own screen.
    fn display<'local>(
        env: &mut JNIEnv<'local>,
        activity: &JObject<'_>,
    ) -> Result<JObject<'local>> {
        if sdk() >= 30 {
            return env.call_method(activity, "getDisplay", "()Landroid/view/Display;", &[])?.l();
        }
        let manager = env
            .call_method(activity, "getWindowManager", "()Landroid/view/WindowManager;", &[])?
            .l()?;
        env.call_method(&manager, "getDefaultDisplay", "()Landroid/view/Display;", &[])?.l()
    }
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
