//! The system's bars over the window: on Android the status bar at the top
//! and the navigation bar at the bottom, which the app draws under.

use std::sync::OnceLock;

/// How tall the bars over the top and the bottom of the window are, in
/// pixels. Nothing where the system keeps them off the window.
pub fn heights() -> [f32; 2] {
    static HEIGHTS: OnceLock<[f32; 2]> = OnceLock::new();
    *HEIGHTS.get_or_init(|| {
        #[cfg(target_os = "android")]
        return android::heights().unwrap_or_else(|error| {
            log::warn!("the heights of the system's bars are unknown: {error}");
            [0.0; 2]
        });
        #[cfg(not(target_os = "android"))]
        [0.0; 2]
    })
}

#[cfg(target_os = "android")]
mod android {
    use jni::{
        JNIEnv,
        errors::Result,
        objects::{JObject, JValue},
    };

    use crate::java::with_java;

    /// The heights Android's own resources give its bars.
    pub fn heights() -> Result<[f32; 2]> {
        with_java(|env, app| {
            let resources = env
                .call_method(app, "getResources", "()Landroid/content/res/Resources;", &[])?
                .l()?;
            Ok([
                dimension(env, &resources, "status_bar_height")?,
                dimension(env, &resources, "navigation_bar_height")?,
            ])
        })
    }

    fn dimension(env: &mut JNIEnv<'_>, resources: &JObject<'_>, name: &str) -> Result<f32> {
        let name = env.new_string(name)?;
        let kind = env.new_string("dimen")?;
        let package = env.new_string("android")?;
        let id = env
            .call_method(
                resources,
                "getIdentifier",
                "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)I",
                &[JValue::Object(&name), JValue::Object(&kind), JValue::Object(&package)],
            )?
            .i()?;
        if id == 0 {
            return Ok(0.0);
        }
        let pixels =
            env.call_method(resources, "getDimensionPixelSize", "(I)I", &[JValue::Int(id)])?.i()?;
        Ok(pixels as f32)
    }
}
