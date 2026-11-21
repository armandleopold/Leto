//! The Leto app on Android.
//!
//! Android starts `android_main` in a NativeActivity: the whole app is the
//! desktop one (`apps/leto`), drawn by egui. What only Android needs is
//! here: its log, and the Wi-Fi lock without which the PCs on the network
//! cannot be found. `android/` holds the manifest, and
//! `cargo xtask android` builds the APK.

#![cfg(target_os = "android")]
// Android's log and its Java side are reached through C.
#![allow(unsafe_code)]

mod logcat;
mod wifi;

/// Where Android starts the app.
#[unsafe(no_mangle)]
fn android_main(activity: leto::AndroidApp) {
    logcat::forward("leto");
    // Without it Android drops the multicast answers PCs announce
    // themselves with, and only PCs typed in by hand would show.
    let lock = wifi::MulticastLock::take(&activity);
    if let Err(error) = &lock {
        log::warn!("PCs on this network may not be found: {error}");
    }
    if let Err(error) = leto::run_android(activity) {
        log::error!("Leto stopped: {error}");
    }
    drop(lock);
}
