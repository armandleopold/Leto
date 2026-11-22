//! The app among the others the system runs. On Android the app can step
//! behind them, as Android's back does on the first screen of any app;
//! elsewhere a window stays where the person put it, and this does nothing.

/// Puts the app behind the others, still running, as the home screen does:
/// the person comes back to it where they left it.
pub fn go_behind() {
    #[cfg(target_os = "android")]
    {
        // `Activity.moveTaskToBack(true)`: the whole task, whichever of its
        // activities is in front.
        let told = crate::java::with_java(|env, activity| {
            env.call_method(activity, "moveTaskToBack", "(Z)Z", &[jni::objects::JValue::Bool(1)])
                .map(|_| ())
        });
        if let Err(error) = told {
            log::warn!("the app could not step behind the others: {error}");
        }
    }
}
