//! Which way the app's window stands. On Android it can be held in
//! landscape, whichever way the phone is turned; elsewhere the window
//! stands as the screen does, and this does nothing.

#[cfg(target_os = "android")]
use std::sync::atomic::{AtomicI8, Ordering};

/// What was last asked of Android: -1 nothing yet, 0 free, 1 landscape.
#[cfg(target_os = "android")]
static ASKED: AtomicI8 = AtomicI8::new(-1);

/// Holds the window in landscape while `on`, either way up as the phone
/// is turned, and lets it turn freely again when not. Cheap to call every
/// frame: Android is only told of a change.
pub fn landscape(on: bool) {
    #[cfg(target_os = "android")]
    {
        let wanted = i8::from(on);
        if ASKED.swap(wanted, Ordering::Relaxed) == wanted {
            return;
        }
        // `ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE`, and
        // `SCREEN_ORIENTATION_UNSPECIFIED`.
        let orientation = if on { 6 } else { -1 };
        let told = crate::java::with_java(|env, activity| {
            env.call_method(
                activity,
                "setRequestedOrientation",
                "(I)V",
                &[jni::objects::JValue::Int(orientation)],
            )
            .map(|_| ())
        });
        if let Err(error) = told {
            log::warn!("the window could not be turned: {error}");
        }
    }
    #[cfg(not(target_os = "android"))]
    let _ = on;
}
