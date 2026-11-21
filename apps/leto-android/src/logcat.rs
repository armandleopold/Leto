//! What the app writes to its standard output and error goes to Android's
//! log (`adb logcat -s leto`), which is where Android looks.

use std::{
    ffi::CString,
    fs::File,
    io::{BufRead as _, BufReader},
    os::fd::FromRawFd as _,
    thread,
};

/// `ANDROID_LOG_INFO` in `<android/log.h>`.
const INFO: libc::c_int = 4;

unsafe extern "C" {
    fn __android_log_write(
        priority: libc::c_int,
        tag: *const libc::c_char,
        text: *const libc::c_char,
    ) -> libc::c_int;
}

/// Sends both outputs to the log under `tag`, line by line. The app's own
/// log is asked to write there too.
pub fn forward(tag: &str) {
    // `leto_client::logs` copies its messages to the error output when
    // this is set.
    // SAFETY: no other thread runs yet.
    unsafe { std::env::set_var("RUST_LOG", "info") };

    let mut ends = [0; 2];
    // SAFETY: `ends` holds the two descriptors `pipe` writes.
    if unsafe { libc::pipe(ends.as_mut_ptr()) } != 0 {
        return;
    }
    // SAFETY: the descriptors are valid; the pipe's end replaces both outputs.
    unsafe {
        libc::dup2(ends[1], libc::STDOUT_FILENO);
        libc::dup2(ends[1], libc::STDERR_FILENO);
        libc::close(ends[1]);
    }
    // SAFETY: the reading end is open and owned by this file alone.
    let reading = unsafe { File::from_raw_fd(ends[0]) };
    let Ok(tag) = CString::new(tag) else { return };
    let _ = thread::Builder::new().name("logcat".into()).spawn(move || {
        for line in BufReader::new(reading).lines().map_while(Result::ok) {
            let Ok(text) = CString::new(line) else { continue };
            // SAFETY: both strings end with a nul and outlive the call.
            unsafe { __android_log_write(INFO, tag.as_ptr(), text.as_ptr()) };
        }
    });
}
