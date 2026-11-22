//! Leto started again with the rights of an administrator, for what
//! Windows keeps from an ordinary program: the settings of an Apollo under
//! `Program Files`. Windows asks the person first. Elsewhere this does
//! nothing.

/// Asks Windows to start this program again as administrator. Returns
/// whether it did: the person may say no. The one that asked then has to
/// end, so that only one Leto runs.
pub fn restart_as_administrator() -> bool {
    #[cfg(windows)]
    return windows::restart();
    #[cfg(not(windows))]
    false
}

#[cfg(windows)]
mod windows {
    use std::{iter, os::windows::ffi::OsStrExt as _, ptr};

    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};

    pub fn restart() -> bool {
        let Ok(program) = std::env::current_exe() else { return false };
        let wide = |text: &std::ffi::OsStr| -> Vec<u16> {
            text.encode_wide().chain(iter::once(0)).collect()
        };
        let (verb, program) = (wide("runas".as_ref()), wide(program.as_os_str()));
        // SAFETY: both strings end with a zero and outlive the call, and
        // the other pointers may be null.
        let started = unsafe {
            ShellExecuteW(
                ptr::null_mut(),
                verb.as_ptr(),
                program.as_ptr(),
                ptr::null(),
                ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        // Windows answers with more than 32 when it started the program.
        started as usize > 32
    }
}
