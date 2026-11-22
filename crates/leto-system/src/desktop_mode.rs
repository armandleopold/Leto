//! A phone's desktop on a monitor it is plugged into, as Android 15 and
//! later show one: apps there are in windows, which an app may ask to turn
//! into its whole screen. Elsewhere this does nothing.

/// Asks the system to turn the app's window into the whole screen it is
/// on, when `fill`, or back into the window it was, when not. Does nothing
/// where the window is not one of a desktop's: on the phone's own screen,
/// before Android 15, or off Android.
pub fn fill_screen(fill: bool) {
    #[cfg(target_os = "android")]
    if let Err(error) = android::fill_screen(fill) {
        log::warn!("the desktop did not turn Leto's window around: {error}");
    }
    #[cfg(not(target_os = "android"))]
    let _ = fill;
}

#[cfg(target_os = "android")]
mod android {
    use jni::{
        errors::Result,
        objects::{JObject, JValue},
    };

    use crate::java::{sdk, with_java};

    /// `Activity.FULLSCREEN_MODE_REQUEST_EXIT` and `_ENTER`.
    const EXIT: i32 = 0;
    const ENTER: i32 = 1;

    pub fn fill_screen(fill: bool) -> Result<()> {
        // `Activity.requestFullscreenMode` came with Android 15.
        if sdk() < 35 {
            return Ok(());
        }
        with_java(|env, activity| {
            // A window of a desktop is one of several on its screen; a
            // window turned into the whole screen is not.
            let windowed = env.call_method(activity, "isInMultiWindowMode", "()Z", &[])?.z()?;
            if fill != windowed {
                return Ok(());
            }
            let request = if fill { ENTER } else { EXIT };
            // No answer is waited for: the window changes size if the
            // system agrees, and the app follows its size.
            env.call_method(
                activity,
                "requestFullscreenMode",
                "(ILandroid/os/OutcomeReceiver;)V",
                &[JValue::Int(request), JValue::Object(&JObject::null())],
            )?;
            log::info!(
                "asked the desktop to {} Leto's window",
                if fill { "fill the screen with" } else { "give back" }
            );
            Ok(())
        })
    }
}
