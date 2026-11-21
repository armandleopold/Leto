//! The Leto app on Android.
//!
//! Android starts `android_main` in a NativeActivity: the whole app is the
//! desktop one (`apps/leto`), drawn by egui. What only Android needs is
//! here: the Wi-Fi lock without which the PCs on the network cannot be
//! found. android-activity already sends what the app writes to its error
//! output to Android's log (`adb logcat -s RustStdoutStderr`).
//! `android/` holds the manifest, and `cargo xtask android` builds the APK.

#![cfg(target_os = "android")]
// Android's Java side is reached through JNI.
#![allow(unsafe_code)]

mod wifi;

/// Where Android starts the app.
#[unsafe(no_mangle)]
fn android_main(activity: leto::AndroidApp) {
    // `leto_client::logs` copies its messages to the error output, which
    // goes to Android's log, when this is set.
    // SAFETY: no other thread of the app runs yet.
    unsafe { std::env::set_var("RUST_LOG", "info") };
    // A panic's log says where it happened.
    // SAFETY: as above.
    unsafe { std::env::set_var("RUST_BACKTRACE", "1") };
    // Without it Android drops the multicast answers PCs announce
    // themselves with, and only PCs typed in by hand would show.
    let lock = wifi::MulticastLock::take(&activity);
    if let Err(error) = &lock {
        // The app's log has not started yet.
        eprintln!("PCs on this network may not be found: {error}");
    }
    if let Err(error) = leto::run_android(activity) {
        eprintln!("Leto stopped: {error}");
    }
    drop(lock);
}
