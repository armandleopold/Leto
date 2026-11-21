//! The "Braise" look, by night and by day: a warm black ground with ivory
//! text, or an ivory ground with warm black text, and an orange accent on
//! both. Leto wears the one the system asks for, and changes with it.

use std::cell::Cell;

use egui::{Color32, CornerRadius, FontId, Stroke, TextStyle, Theme, ThemePreference, Visuals};
use leto_core::brand::{self, Rgb};

/// The colours of one look.
pub struct Palette {
    /// What the pages are painted on.
    pub ground: Color32,
    /// A card or a panel, raised a little from the ground.
    pub surface: Color32,
    /// A card or a button under the pointer.
    pub surface_hover: Color32,
    /// Hairlines: card borders and separators.
    pub line: Color32,
    /// The accent: what is lit, chosen or going on.
    pub sun: Color32,
    /// The accent where it is text, which has to be read: a link.
    pub sun_text: Color32,
    /// What is written or drawn on the accent.
    pub on_sun: Color32,
    /// Text and icons.
    pub text: Color32,
    /// Secondary text.
    pub dim: Color32,
    /// The dot next to a PC that is ready, and the words that say so.
    pub ready: Color32,
    /// Text that reports a problem.
    pub problem: Color32,
}

/// The look Leto was drawn in first. A stream is always shown in it: its
/// pictures have black around them, and what is drawn over them has to be
/// read on them.
pub const NIGHT: Palette = Palette {
    ground: color(brand::GROUND),
    surface: color(brand::SURFACE),
    surface_hover: Color32::from_rgb(0x2B, 0x27, 0x24),
    line: Color32::from_rgb(0x3A, 0x35, 0x31),
    sun: color(brand::SUN),
    sun_text: color(brand::SUN),
    on_sun: color(brand::GROUND),
    text: color(brand::MOON),
    dim: color(brand::MOON_DIM),
    ready: Color32::from_rgb(0x8F, 0xD1, 0x7F),
    problem: Color32::from_rgb(0xF4, 0x92, 0x85),
};

/// The same look turned over, for a system set to light: the moon's ivory is
/// the ground and the warm black is the ink. Only the two brand colours are
/// the brand's; the others were chosen to be read on them.
pub const DAY: Palette = Palette {
    ground: color(brand::MOON),
    surface: Color32::from_rgb(0xFB, 0xF8, 0xF3),
    surface_hover: Color32::from_rgb(0xE7, 0xE0, 0xD5),
    line: Color32::from_rgb(0xD6, 0xCE, 0xC1),
    // The brand's orange is too light to be told from ivory: a deeper one.
    sun: Color32::from_rgb(0xE8, 0x54, 0x1C),
    sun_text: Color32::from_rgb(0xC8, 0x40, 0x0B),
    on_sun: color(brand::GROUND),
    text: color(brand::GROUND),
    dim: Color32::from_rgb(0x62, 0x5C, 0x54),
    ready: Color32::from_rgb(0x23, 0x80, 0x3A),
    problem: Color32::from_rgb(0xB3, 0x26, 0x1E),
};

// The app has one window and paints it from one thread, which sets these as
// each frame starts. Nothing else asks for a colour.
thread_local! {
    /// Whether the look worn now is the one of the day.
    static BY_DAY: Cell<bool> = const { Cell::new(false) };

    /// Whether a stream is on screen, which is shown by night whatever the
    /// system asks for.
    static NIGHT_ONLY: Cell<bool> = const { Cell::new(false) };
}

/// The colours of the look worn now.
pub fn now() -> &'static Palette {
    if by_day() { &DAY } else { &NIGHT }
}

/// Whether the look worn now is the one of the day.
pub fn by_day() -> bool {
    BY_DAY.get()
}

pub fn ground() -> Color32 {
    now().ground
}

pub fn surface() -> Color32 {
    now().surface
}

pub fn surface_hover() -> Color32 {
    now().surface_hover
}

pub fn line() -> Color32 {
    now().line
}

pub fn sun() -> Color32 {
    now().sun
}

pub fn on_sun() -> Color32 {
    now().on_sun
}

pub fn text() -> Color32 {
    now().text
}

pub fn dim() -> Color32 {
    now().dim
}

pub fn ready() -> Color32 {
    now().ready
}

pub fn problem() -> Color32 {
    now().problem
}

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

/// Gives an egui context both looks, and has it follow the system's light
/// or dark setting. `LETO_THEME=light` or `dark` in the environment asks for
/// one of them instead, for a system that does not tell which it wants.
pub fn apply(ctx: &egui::Context) {
    ctx.set_visuals_of(Theme::Dark, visuals(&NIGHT));
    ctx.set_visuals_of(Theme::Light, visuals(&DAY));
    ctx.set_theme(asked(std::env::var("LETO_THEME").ok().as_deref()));
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
    BY_DAY.set(ctx.theme() == Theme::Light);
    NIGHT_ONLY.set(false);
}

/// Wears the look the system asks for, from this frame on: called as each
/// frame starts, before anything is painted. With `night_only`, for a
/// stream, the look is the one of the night whatever the system asks for.
///
/// egui keeps following the system all along. While a stream is on screen
/// its light theme is given the colours of the night, so that nothing has
/// to be asked of the window, and the system is still listened to.
pub fn follow(ui: &mut egui::Ui, night_only: bool) {
    let ctx = ui.ctx().clone();
    if NIGHT_ONLY.replace(night_only) != night_only {
        ctx.set_visuals_of(Theme::Light, visuals(if night_only { &NIGHT } else { &DAY }));
    }
    let by_day = !night_only && ctx.theme() == Theme::Light;
    BY_DAY.set(by_day);
    // The frame was started in the look of the one before.
    if ui.visuals().panel_fill != now().ground {
        ui.set_style(ctx.style_of(ctx.theme()));
    }
}

/// What `LETO_THEME` asks for.
fn asked(wish: Option<&str>) -> ThemePreference {
    match wish {
        Some("light") => ThemePreference::Light,
        Some("dark") => ThemePreference::Dark,
        _ => ThemePreference::System,
    }
}

/// egui's colours for one look.
fn visuals(palette: &Palette) -> Visuals {
    let by_day = palette.ground == DAY.ground;
    let mut visuals = if by_day { Visuals::light() } else { Visuals::dark() };
    visuals.panel_fill = palette.ground;
    visuals.window_fill = palette.surface;
    visuals.window_stroke = Stroke::new(1.0, palette.line);
    visuals.window_corner_radius = CornerRadius::same(CARD_RADIUS);
    visuals.menu_corner_radius = CornerRadius::same(CONTROL_RADIUS);
    visuals.extreme_bg_color = palette.ground;
    visuals.faint_bg_color = palette.surface;
    visuals.override_text_color = Some(palette.text);
    visuals.hyperlink_color = palette.sun_text;
    visuals.selection.bg_fill = palette.sun.gamma_multiply(0.45);
    visuals.selection.stroke = Stroke::new(1.0, palette.sun);

    let radius = CornerRadius::same(CONTROL_RADIUS);
    let widgets = &mut visuals.widgets;
    for (widget, fill, line) in [
        (&mut widgets.noninteractive, palette.surface, palette.line),
        (&mut widgets.inactive, palette.surface, palette.line),
        (&mut widgets.hovered, palette.surface_hover, palette.dim),
        (&mut widgets.active, palette.surface_hover, palette.text),
        (&mut widgets.open, palette.surface_hover, palette.dim),
    ] {
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        widget.bg_stroke = Stroke::new(1.0, line);
        widget.corner_radius = radius;
        widget.expansion = 0.0;
    }
    visuals
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How far apart two colours are for the eye, from 1 to 21, as the web's
    /// accessibility guidelines count it.
    fn contrast(one: Color32, other: Color32) -> f32 {
        fn light(color: Color32) -> f32 {
            let part = |value: u8| {
                let value = f32::from(value) / 255.0;
                if value <= 0.04045 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * part(color.r()) + 0.7152 * part(color.g()) + 0.0722 * part(color.b())
        }
        let (one, other) = (light(one), light(other));
        (one.max(other) + 0.05) / (one.min(other) + 0.05)
    }

    #[test]
    fn what_is_written_can_be_read_by_night_and_by_day() {
        for (name, look) in [("night", &NIGHT), ("day", &DAY)] {
            for (ground, on) in [(look.ground, "the ground"), (look.surface, "a card")] {
                for (ink, what) in [(look.text, "text"), (look.dim, "secondary text")] {
                    let read = contrast(ink, ground);
                    assert!(read >= 4.5, "{what} on {on} by {name}: {read}");
                }
                // Short words in colour, and marks: a dot, a border, an icon.
                for (ink, what) in [
                    (look.sun_text, "the accent as text"),
                    (look.ready, "ready"),
                    (look.problem, "a problem"),
                ] {
                    let read = contrast(ink, ground);
                    assert!(read >= 4.0, "{what} on {on} by {name}: {read}");
                }
                let seen = contrast(look.sun, ground);
                assert!(seen >= 3.0, "the accent on {on} by {name}: {seen}");
            }
            let read = contrast(look.on_sun, look.sun);
            assert!(read >= 4.5, "what is written on the accent by {name}: {read}");
        }
    }

    #[test]
    fn the_system_is_followed_unless_one_look_is_asked_for() {
        assert_eq!(asked(None), ThemePreference::System);
        assert_eq!(asked(Some("light")), ThemePreference::Light);
        assert_eq!(asked(Some("dark")), ThemePreference::Dark);
        assert_eq!(asked(Some("pink")), ThemePreference::System);
    }

    /// One frame on a system that asks for `system`, around a stream or
    /// not: the ground a page would be painted on, and the ground egui
    /// gives to what floats over it.
    fn frame(ctx: &egui::Context, system: Theme, stream: bool) -> (Color32, Color32) {
        let input = egui::RawInput { system_theme: Some(system), ..Default::default() };
        let mut grounds = None;
        let mut output = ctx.run_ui(input, |ui| {
            follow(ui, stream);
            let floating = ui.ctx().style_of(ui.ctx().theme()).visuals.panel_fill;
            assert_eq!(ui.visuals().panel_fill, ground());
            grounds = Some((ground(), floating));
        });
        // Nothing draws these frames: their textures go nowhere.
        output.textures_delta.clear();
        grounds.expect("the frame was run")
    }

    #[test]
    fn the_look_follows_the_system_as_it_changes_and_a_stream_stays_by_night() {
        let ctx = egui::Context::default();
        apply(&ctx);
        let (night, day) = ((NIGHT.ground, NIGHT.ground), (DAY.ground, DAY.ground));
        assert_eq!(frame(&ctx, Theme::Dark, false), night);
        // The system turns light while Leto runs, then a stream starts.
        assert_eq!(frame(&ctx, Theme::Light, false), day);
        assert!(by_day());
        assert_eq!(text(), DAY.text);
        assert_eq!(frame(&ctx, Theme::Light, true), night);
        assert!(!by_day());
        assert_eq!(text(), NIGHT.text);
        // The stream is left: back to what the system asks for.
        assert_eq!(frame(&ctx, Theme::Light, false), day);
        assert_eq!(frame(&ctx, Theme::Dark, false), night);
        // A system that turns light during a stream is followed after it.
        assert_eq!(frame(&ctx, Theme::Dark, true), night);
        assert_eq!(frame(&ctx, Theme::Light, true), night);
        assert_eq!(frame(&ctx, Theme::Light, false), day);
    }

    #[test]
    fn each_look_has_its_own_colours_in_egui() {
        let (night, day) = (visuals(&NIGHT), visuals(&DAY));
        assert!(night.dark_mode && !day.dark_mode);
        assert_eq!(night.panel_fill, NIGHT.ground);
        assert_eq!(day.panel_fill, DAY.ground);
        assert_eq!(day.override_text_color, Some(DAY.text));
    }
}
