//! Android's Java side, which some of what the app asks of the system is
//! only reached through.

use jni::{JNIEnv, JavaVM, errors::Result, objects::JObject};

/// Runs `work` with the app's Java side, and clears what Java threw so
/// that the next call into Java does not fail for it.
pub fn with_java<T>(work: impl FnOnce(&mut JNIEnv<'_>, &JObject<'_>) -> Result<T>) -> Result<T> {
    let context = ndk_context::android_context();
    // SAFETY: android-activity hands ndk-context the app's Java VM and a
    // global reference to its context, both alive as long as the app.
    let vm = unsafe { JavaVM::from_raw(context.vm().cast())? };
    let mut env = vm.attach_current_thread()?;
    // SAFETY: as above; the reference is never deleted here.
    let app = unsafe { JObject::from_raw(context.context().cast()) };
    let result = env.with_local_frame(16, |env| work(env, &app));
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    result
}

/// The version of Android's API the phone runs, `Build.VERSION.SDK_INT`;
/// 0 when Java does not say.
pub fn sdk() -> i32 {
    static SDK: std::sync::OnceLock<i32> = std::sync::OnceLock::new();
    *SDK.get_or_init(|| {
        with_java(|env, _| env.get_static_field("android/os/Build$VERSION", "SDK_INT", "I")?.i())
            .unwrap_or_else(|error| {
                log::warn!("the version of Android is unknown: {error}");
                0
            })
    })
}
