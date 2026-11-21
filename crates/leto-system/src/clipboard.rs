//! The system's clipboard, where egui cannot reach it: on Android it is
//! Java's `ClipboardManager`, and a NativeActivity has no menu to paste
//! with. Elsewhere this finds nothing, and the app uses its own way.

/// The text on the clipboard, when there is some and it can be read.
pub fn text() -> Option<String> {
    #[cfg(target_os = "android")]
    return android::text().unwrap_or_else(|error| {
        log::warn!("the clipboard cannot be read: {error}");
        None
    });
    #[cfg(not(target_os = "android"))]
    None
}

/// Puts `text` on the clipboard. Returns whether this did it: where it
/// does not, the app uses its own way.
pub fn copy(text: &str) -> bool {
    #[cfg(target_os = "android")]
    return android::copy(text)
        .inspect_err(|error| log::warn!("the clipboard cannot be written: {error}"))
        .is_ok();
    #[cfg(not(target_os = "android"))]
    {
        let _ = text;
        false
    }
}

#[cfg(target_os = "android")]
mod android {
    use jni::{
        JNIEnv, JavaVM,
        errors::Result,
        objects::{JObject, JString, JValue},
    };

    /// Runs `work` with the app's Java side, and clears what Java threw so
    /// that the next call into Java does not fail for it.
    fn with_java<T>(work: impl FnOnce(&mut JNIEnv<'_>, &JObject<'_>) -> Result<T>) -> Result<T> {
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

    fn manager<'local>(env: &mut JNIEnv<'local>, app: &JObject<'_>) -> Result<JObject<'local>> {
        let name = env.new_string("clipboard")?;
        env.call_method(
            app,
            "getSystemService",
            "(Ljava/lang/String;)Ljava/lang/Object;",
            &[JValue::Object(&name)],
        )?
        .l()
    }

    pub fn text() -> Result<Option<String>> {
        with_java(|env, app| {
            let manager = manager(env, app)?;
            let clip = env
                .call_method(&manager, "getPrimaryClip", "()Landroid/content/ClipData;", &[])?
                .l()?;
            if clip.is_null() {
                return Ok(None);
            }
            let item = env
                .call_method(
                    &clip,
                    "getItemAt",
                    "(I)Landroid/content/ClipData$Item;",
                    &[JValue::Int(0)],
                )?
                .l()?;
            let text = env
                .call_method(
                    &item,
                    "coerceToText",
                    "(Landroid/content/Context;)Ljava/lang/CharSequence;",
                    &[JValue::Object(app)],
                )?
                .l()?;
            if text.is_null() {
                return Ok(None);
            }
            let text = JString::from(
                env.call_method(&text, "toString", "()Ljava/lang/String;", &[])?.l()?,
            );
            let text: String = env.get_string(&text)?.into();
            Ok((!text.is_empty()).then_some(text))
        })
    }

    pub fn copy(text: &str) -> Result<()> {
        with_java(|env, app| {
            let manager = manager(env, app)?;
            let label = env.new_string("Leto")?;
            let text = env.new_string(text)?;
            let clip = env
                .call_static_method(
                    "android/content/ClipData",
                    "newPlainText",
                    "(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;",
                    &[JValue::Object(&label), JValue::Object(&text)],
                )?
                .l()?;
            env.call_method(
                &manager,
                "setPrimaryClip",
                "(Landroid/content/ClipData;)V",
                &[JValue::Object(&clip)],
            )?;
            Ok(())
        })
    }
}
