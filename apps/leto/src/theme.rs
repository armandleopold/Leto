//! The "Braise" look: warm black ground, ivory text, orange accent.

use egui::{Color32, CornerRadius, FontId, Stroke, TextStyle, Theme, Visuals};
use leto_core::brand::{self, Rgb};

pub const GROUND: Color32 = color(brand::GROUND);
pub const SURFACE: Color32 = color(brand::SURFACE);
pub const SUN: Color32 = color(brand::SUN);
pub const MOON: Color32 = color(brand::MOON);
/// Secondary text.
pub const DIM: Color32 = color(brand::MOON_DIM);

// Colours of the interface only. They are not part of the brand.

/// A card under the pointer.
pub const SURFACE_HOVER: Color32 = Color32::from_rgb(0x2B, 0x27, 0x24);
/// Hairlines: card borders and separators.
pub const LINE: Color32 = Color32::from_rgb(0x3A, 0x35, 0x31);
/// The dot next to a PC that is ready.
pub const READY: Color32 = Color32::from_rgb(0x8F, 0xD1, 0x7F);
/// Text that reports a problem.
pub const PROBLEM: Color32 = Color32::from_rgb(0xF4, 0x92, 0x85);

/// Text sizes, in points.
pub const TITLE: f32 = 28.0;
pub const BODY: f32 = 15.0;
pub const SMALL: f32 = 13.0;

/// Corner radius of cards and panels.
pub const CARD_RADIUS: u8 = 14;
/// Corner radius of buttons and fields.
pub const CONTROL_RADIUS: u8 = 10;

/// Converts a brand colour to an egui colour.
pub const fn color(rgb: Rgb) -> Color32 {
    Color32::from_rgb(rgb.r, rgb.g, rgb.b)
}

/// Applies the Leto theme to an egui context.
///
/// Leto has one look. It does not follow the system's light or dark setting,
/// so the theme is pinned to dark before the colours are set.
pub fn apply(ctx: &egui::Context) {
    let mut visuals = Visuals::dark();
    visuals.panel_fill = GROUND;
    visuals.window_fill = SURFACE;
    visuals.window_stroke = Stroke::new(1.0, LINE);
    visuals.window_corner_radius = CornerRadius::same(CARD_RADIUS);
    visuals.menu_corner_radius = CornerRadius::same(CONTROL_RADIUS);
    visuals.extreme_bg_color = GROUND;
    visuals.faint_bg_color = SURFACE;
    visuals.override_text_color = Some(MOON);
    visuals.hyperlink_color = SUN;
    visuals.selection.bg_fill = SUN.gamma_multiply(0.45);
    visuals.selection.stroke = Stroke::new(1.0, SUN);

    let radius = CornerRadius::same(CONTROL_RADIUS);
    let widgets = &mut visuals.widgets;
    for (widget, fill, line) in [
        (&mut widgets.noninteractive, SURFACE, LINE),
        (&mut widgets.inactive, SURFACE, LINE),
        (&mut widgets.hovered, SURFACE_HOVER, DIM),
        (&mut widgets.active, SURFACE_HOVER, MOON),
        (&mut widgets.open, SURFACE_HOVER, DIM),
    ] {
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        widget.bg_stroke = Stroke::new(1.0, line);
        widget.corner_radius = radius;
        widget.expansion = 0.0;
    }

    ctx.set_theme(Theme::Dark);
    ctx.set_visuals_of(Theme::Dark, visuals);
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(10.0, 10.0);
        style.spacing.button_padding = egui::vec2(16.0, 9.0);
        style.spacing.menu_margin = egui::Margin::same(8);
        style.interaction.selectable_labels = false;
        style.text_styles.insert(TextStyle::Body, FontId::proportional(BODY));
        style.text_styles.insert(TextStyle::Button, FontId::proportional(BODY));
        style.text_styles.insert(TextStyle::Small, FontId::proportional(SMALL));
        style.text_styles.insert(TextStyle::Heading, FontId::proportional(TITLE));
    });
}
