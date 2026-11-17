//! The Leto app: host and client in one window.
//!
//! The same code runs as a native desktop app and, compiled to WebAssembly,
//! in the browser.

// Hide the console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod theme;

/// Id of the `<canvas>` element the web build draws into.
#[cfg(target_arch = "wasm32")]
const CANVAS_ID: &str = "leto_canvas";

/// Id of the element that shows "Loading" until the first frame, then errors.
#[cfg(target_arch = "wasm32")]
const STATUS_ID: &str = "leto_status";

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    env_logger::init();

    let icon = eframe::icon_data::from_png_bytes(include_bytes!(
        "../../../assets/brand/png/leto-icon-256.png"
    ))
    .expect("the embedded app icon is a valid PNG");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(leto_core::brand::NAME)
            .with_inner_size([960.0, 600.0])
            .with_min_inner_size([480.0, 360.0])
            .with_icon(icon),
        ..Default::default()
    };

    eframe::run_native(
        leto_core::brand::NAME,
        options,
        Box::new(|cc| Ok(Box::new(app::LetoApp::new(cc)))),
    )
}

#[cfg(target_arch = "wasm32")]
fn main() {
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
                Box::new(|cc| Ok(Box::new(app::LetoApp::new(cc)))),
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
