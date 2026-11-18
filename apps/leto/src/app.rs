//! The app's single window.

use egui::{Align, Layout, RichText};
use leto_core::{brand, platform::Platform};

use crate::theme;

/// Displayed width of the logo, in points. The image is four times larger so
/// it stays sharp on high-density screens.
const LOGO_WIDTH: f32 = 301.0;
const LOGO_HEIGHT: f32 = 86.0;

/// Space kept free on each side of the logo, in points.
const SIDE_MARGIN: f32 = 24.0;

/// State of the Leto window.
pub struct LetoApp;

impl LetoApp {
    /// Called once before the first frame.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx);
        Self
    }
}

impl eframe::App for LetoApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                ui.add_space(16.0);
                ui.label(RichText::new(build_line()).color(theme::color(brand::MOON_DIM)).small());

                ui.with_layout(Layout::top_down(Align::Center), |ui| {
                    // Shrink the logo on narrow screens instead of clipping it.
                    let scale = ((ui.available_width() - 2.0 * SIDE_MARGIN) / LOGO_WIDTH).min(1.0);
                    let logo = egui::vec2(LOGO_WIDTH, LOGO_HEIGHT) * scale.max(0.25);

                    let free = ui.available_height() - logo.y - 96.0;
                    ui.add_space((free / 2.0).max(16.0));
                    ui.add(
                        egui::Image::new(egui::include_image!(
                            "../../../assets/brand/png/leto-logo.png"
                        ))
                        .fit_to_exact_size(logo),
                    );
                    ui.add_space(24.0);
                    ui.label(RichText::new(brand::TAGLINE).size(18.0));
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("Foundation build. Streaming arrives in the next steps.")
                            .color(theme::color(brand::MOON_DIM)),
                    );
                });
            });
        });
    }
}

/// The small line at the bottom: version, platform and role.
fn build_line() -> String {
    let platform = Platform::current();
    let name = platform.map_or("unsupported platform", Platform::name);
    let role = if platform.is_some_and(Platform::can_host) { "host and client" } else { "client" };
    format!("{} {} · {name} · {role}", brand::NAME, leto_core::VERSION)
}
