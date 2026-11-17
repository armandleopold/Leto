//! The "Braise" look: warm black ground, ivory text, orange accent.

use egui::{Color32, Theme, Visuals};
use leto_core::brand::{self, Rgb};

/// Converts a brand colour to an egui colour.
pub fn color(rgb: Rgb) -> Color32 {
    Color32::from_rgb(rgb.r, rgb.g, rgb.b)
}

/// Applies the Leto theme to an egui context.
///
/// Leto has one look. It does not follow the system's light or dark setting,
/// so the theme is pinned to dark before the colours are set.
pub fn apply(ctx: &egui::Context) {
    let ground = color(brand::GROUND);
    let surface = color(brand::SURFACE);
    let sun = color(brand::SUN);
    let moon = color(brand::MOON);

    let mut visuals = Visuals::dark();
    visuals.panel_fill = ground;
    visuals.window_fill = surface;
    visuals.extreme_bg_color = ground;
    visuals.faint_bg_color = surface;
    visuals.override_text_color = Some(moon);
    visuals.hyperlink_color = sun;
    visuals.selection.bg_fill = sun;
    visuals.selection.stroke.color = ground;
    ctx.set_theme(Theme::Dark);
    ctx.set_visuals_of(Theme::Dark, visuals);
}
