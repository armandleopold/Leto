//! Whether the app runs on a television, as a Fire TV or an Android TV,
//! seen from a sofa and gone through with a remote or a gamepad rather
//! than touched. Elsewhere it never does.

/// Whether this device is a television. Asked of the system once.
pub fn is_television() -> bool {
    #[cfg(target_os = "android")]
    {
        static ASKED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ASKED.get_or_init(ask)
    }
    #[cfg(not(target_os = "android"))]
    false
}

/// `PackageManager.hasSystemFeature` for Android TV's home screen
/// (leanback) and for Amazon's Fire TV, whose older models say only the
/// second.
#[cfg(target_os = "android")]
fn ask() -> bool {
    const FEATURES: [&str; 2] = ["android.software.leanback", "amazon.hardware.fire_tv"];
    let asked = crate::java::with_java(|env, app| {
        let manager = env
            .call_method(app, "getPackageManager", "()Landroid/content/pm/PackageManager;", &[])?
            .l()?;
        for feature in FEATURES {
            let name = env.new_string(feature)?;
            let has = env
                .call_method(
                    &manager,
                    "hasSystemFeature",
                    "(Ljava/lang/String;)Z",
                    &[jni::objects::JValue::Object(&name)],
                )?
                .z()?;
            if has {
                return Ok(true);
            }
        }
        Ok(false)
    });
    match asked {
        Ok(television) => {
            log::info!("this device is {}a television", if television { "" } else { "not " });
            television
        }
        Err(error) => {
            log::warn!("whether this device is a television is unknown: {error}");
            false
        }
    }
}
