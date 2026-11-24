//! The Leto app: host and client in one window.
//!
//! The same code runs as a native desktop app, as an Android app and,
//! compiled to WebAssembly, in the browser. Each is a thin entry over this
//! library: `src/main.rs` for the desktop and the browser, `apps/leto-android`
//! for Android.

mod app;
mod backend;
#[cfg(not(target_arch = "wasm32"))]
mod clipboard;
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
mod host;
mod input;
mod settings;
mod state;
mod stats;
mod theme;
mod touch;
mod ui;
mod window;

/// Id of the `<canvas>` element the web build draws into.
#[cfg(target_arch = "wasm32")]
const CANVAS_ID: &str = "leto_canvas";

/// Id of the element that shows "Loading" until the first frame, then errors.
#[cfg(target_arch = "wasm32")]
const STATUS_ID: &str = "leto_status";

/// Runs the desktop app until its window closes.
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
pub fn run_desktop() -> eframe::Result {
    use leto_system::instance;

    // The app has no terminal on Windows: what it says goes to a file.
    if let Ok(settings) = leto_client::default_directory() {
        leto_client::logs::start("leto", &settings);
    }

    // One Leto at a time: started again, it shows the one that runs, even
    // put away beside the clock, and opens no window of its own.
    let first = match leto_client::default_directory().map(|folder| instance::claim(&folder)) {
        Ok(Ok(instance::Claim::Other)) => {
            log::info!("Leto already runs: its window was shown instead");
            return Ok(());
        }
        Ok(Ok(instance::Claim::First(first))) => Some(first),
        Ok(Err(problem)) => {
            log::warn!("cannot tell whether Leto already runs: {problem}");
            None
        }
        Err(_) => None,
    };

    // V-Sync is chosen once, before the window opens.
    let settings = settings::load();

    let icon = eframe::icon_data::from_png_bytes(include_bytes!(
        "../../../assets/brand/png/leto-icon-256.png"
    ))
    .expect("the embedded app icon is a valid PNG");

    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(leto_core::brand::NAME)
            .with_inner_size([960.0, 600.0])
            .with_min_inner_size([480.0, 360.0])
            // Where Leto draws the frame itself, in its top bar.
            .with_decorations(!ui::frame::OWN)
            .with_icon(icon),
        // A stream needs Ctrl+C and Ctrl+V as keys, for the PC. Leto's own
        // text fields get them back through `clipboard`.
        clipboard_shortcuts: false,
        ..Default::default()
    };
    options.glow_options.vsync = settings.vsync;

    eframe::run_native(
        leto_core::brand::NAME,
        options,
        Box::new(move |cc| {
            if let Some(first) = first {
                first.listen(shower(cc));
            }
            Ok(Box::new(app::LetoApp::new(cc, settings)))
        }),
    )
}

/// What brings Leto's window back in front, when Leto is started again.
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
fn shower(cc: &eframe::CreationContext<'_>) -> impl Fn() + Send + 'static {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let ctx = cc.egui_ctx.clone();
    // Windows draws nothing in a window it hid: Windows itself shows it.
    let window = match cc.window_handle().map(|handle| handle.as_raw()) {
        Ok(RawWindowHandle::Win32(handle)) => Some(handle.hwnd.get()),
        _ => None,
    };
    move || {
        if let Some(window) = window {
            leto_system::shown::show(window);
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        ctx.request_repaint();
    }
}

/// The Android activity the app runs in, as `apps/leto-android` is given it.
#[cfg(target_os = "android")]
pub use winit::platform::android::activity::AndroidApp;

/// Runs the app in an Android activity until the system ends it. The
/// settings, pairings and logs go in the app's own folder.
#[cfg(target_os = "android")]
pub fn run_android(activity: AndroidApp) -> eframe::Result {
    if let Some(folder) = activity.internal_data_path() {
        leto_client::use_directory(folder);
    }
    if let Ok(settings) = leto_client::default_directory() {
        leto_client::logs::start("leto", &settings);
    }
    let settings = settings::load();
    // Gamepads, which winit would make touches and unknown keys of.
    winit::platform::android::set_input_hook(leto_system::gamepad::hear);

    let mut options = eframe::NativeOptions {
        android_app: Some(activity),
        // As on the desktop: a stream needs Ctrl+C and Ctrl+V as keys.
        clipboard_shortcuts: false,
        ..Default::default()
    };
    options.glow_options.vsync = settings.vsync;

    eframe::run_native(
        leto_core::brand::NAME,
        options,
        Box::new(move |cc| Ok(Box::new(app::LetoApp::new(cc, settings)))),
    )
}

/// Starts the app in the page's `<canvas>`.
#[cfg(target_arch = "wasm32")]
pub fn start_web() {
    use eframe::wasm_bindgen::JsCast as _;

    eframe::WebLogger::init(log::LevelFilter::Info).ok();

    wasm_bindgen_futures::spawn_local(async {
        let document = web_sys::window()
            .and_then(|window| window.document())
            .expect("Leto runs in a browser page");
        let status = document.get_element_by_id(STATUS_ID);
        let canvas = document
            .get_element_by_id(CANVAS_ID)
            .and_then(|element| element.dyn_into::<web_sys::HtmlCanvasElement>().ok())
            .expect("the page has a <canvas id=\"leto_canvas\">");

        let result = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|cc| Ok(Box::new(app::LetoApp::new(cc, settings::load())))),
            )
            .await;

        match (result, status) {
            (Ok(()), Some(status)) => status.remove(),
            (Ok(()), None) => {}
            (Err(error), status) => {
                log::error!("Leto failed to start: {error:?}");
                if let Some(status) = status {
                    status.set_text_content(Some(
                        "Leto could not start in this browser. The console has the details.",
                    ));
                }
            }
        }
    });
}
