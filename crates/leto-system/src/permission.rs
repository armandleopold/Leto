//! What the person allows the app to use. On Android the camera and the
//! microphone are each allowed once, in a question the system asks for the
//! app; elsewhere the system asks by itself when they are opened, or not
//! at all, and this says they are allowed.

/// What needs the person's leave.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    Camera,
    Microphone,
}

impl Need {
    #[cfg(target_os = "android")]
    fn name(self) -> &'static str {
        match self {
            Self::Camera => "android.permission.CAMERA",
            Self::Microphone => "android.permission.RECORD_AUDIO",
        }
    }
}

/// Whether the person allowed it. When this cannot be known it is taken
/// as allowed: opening it then says what is wrong.
pub fn allowed(need: Need) -> bool {
    #[cfg(target_os = "android")]
    return android::allowed(need.name()).unwrap_or_else(|error| {
        log::warn!("whether Leto may use the {need:?} is unknown: {error}");
        true
    });
    #[cfg(not(target_os = "android"))]
    {
        let _ = need;
        true
    }
}

/// Asks the person to allow it. The question shows over the app, and the
/// answer is only seen by [`allowed`] once it is given.
pub fn ask(need: Need) {
    #[cfg(target_os = "android")]
    if let Err(error) = android::ask(need.name()) {
        log::warn!("Leto could not ask for the {need:?}: {error}");
    }
    #[cfg(not(target_os = "android"))]
    let _ = need;
}

#[cfg(target_os = "android")]
mod android {
    use jni::{
        errors::Result,
        objects::{JObject, JValue},
    };

    use crate::java::with_java;

    /// `PackageManager.PERMISSION_GRANTED`.
    const GRANTED: i32 = 0;

    pub fn allowed(name: &str) -> Result<bool> {
        with_java(|env, app| {
            let name = env.new_string(name)?;
            let answer = env
                .call_method(
                    app,
                    "checkSelfPermission",
                    "(Ljava/lang/String;)I",
                    &[JValue::Object(&name)],
                )?
                .i()?;
            Ok(answer == GRANTED)
        })
    }

    pub fn ask(name: &str) -> Result<()> {
        with_java(|env, app| {
            let name = env.new_string(name)?;
            let names = env.new_object_array(1, "java/lang/String", JObject::null())?;
            env.set_object_array_element(&names, 0, name)?;
            // The answer goes to the activity, which a NativeActivity does
            // not pass on: `allowed` reads it instead.
            env.call_method(
                app,
                "requestPermissions",
                "([Ljava/lang/String;I)V",
                &[JValue::Object(&names), JValue::Int(1)],
            )?;
            Ok(())
        })
    }
}
