//! Android's own keyboard shortcuts, for the PC: the Windows key, which
//! opens the list of apps on a phone's desktop, Alt+Tab and the rest.
//!
//! Android 16's second quarterly release lets a window ask for them
//! (`WindowManager.LayoutParams.setKeyboardCaptureEnabled`, with the
//! `CAPTURE_KEYBOARD` permission, which needs no question): while the
//! window is in front, the keys reach it as any key does. Before that
//! release Android keeps them, and nothing here changes anything.

use std::sync::atomic::{AtomicI8, Ordering};

use jni::{errors::Result, objects::JValue};

use crate::java::{sdk, with_java};

/// What was last asked of Android: -1 nothing yet, 0 its own, 1 Leto's.
static ASKED: AtomicI8 = AtomicI8::new(-1);

/// Asks the window for Android's own shortcuts while `on`, and gives them
/// back when not. Only a change is told.
pub fn capture(on: bool) {
    let wanted = i8::from(on);
    if ASKED.swap(wanted, Ordering::Relaxed) == wanted {
        return;
    }
    // The method came within Android 16 (API 36); before, there is none.
    if sdk() < 36 {
        return;
    }
    match ask(on) {
        Ok(()) => log::info!(
            "Android's own keyboard shortcuts are {}",
            if on { "asked for the PC" } else { "given back" }
        ),
        // An Android 16 from before the release that has the method.
        Err(error) => log::info!("Android keeps its own keyboard shortcuts: {error}"),
    }
}

fn ask(on: bool) -> Result<()> {
    with_java(|env, activity| {
        let window = env.call_method(activity, "getWindow", "()Landroid/view/Window;", &[])?.l()?;
        let attributes = env
            .call_method(
                &window,
                "getAttributes",
                "()Landroid/view/WindowManager$LayoutParams;",
                &[],
            )?
            .l()?;
        env.call_method(
            &attributes,
            "setKeyboardCaptureEnabled",
            "(Z)V",
            &[JValue::Bool(u8::from(on))],
        )?;
        env.call_method(
            &window,
            "setAttributes",
            "(Landroid/view/WindowManager$LayoutParams;)V",
            &[JValue::Object(&attributes)],
        )?;
        Ok(())
    })
}
