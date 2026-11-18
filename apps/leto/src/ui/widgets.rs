//! Buttons, text and layout helpers shared by the screens.

use egui::{
    Align, Button, Color32, CursorIcon, FontId, Layout, Painter, Pos2, Rect, Response, RichText,
    Role, Sense, Stroke, Ui, UiBuilder, Vec2, WidgetInfo,
    text::{LayoutJob, TextWrapping},
    vec2,
};

use super::icons::{self, Icon};
use crate::theme;

/// Widest the content of a screen gets, in points.
const PAGE_WIDTH: f32 = 1040.0;

/// Lays `add` out in a centred column with margins that fit the window.
pub fn page(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let full = ui.available_width();
    let margin = if full < 560.0 { 16.0 } else { 32.0 };
    let width = (full - 2.0 * margin).clamp(120.0, PAGE_WIDTH);
    let mut rect = ui.available_rect_before_wrap();
    rect.min.x += ((full - width) / 2.0).max(0.0);
    rect.max.x = rect.min.x + width;
    ui.scope_builder(UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)), |ui| {
        ui.set_width(width);
        add(ui);
        ui.add_space(32.0);
    });
}

/// Secondary text.
pub fn dim(text: impl Into<String>) -> RichText {
    RichText::new(text).color(theme::DIM)
}

/// The title of a screen.
pub fn title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(theme::TITLE));
}

/// The one button a screen wants pressed.
pub fn primary_button(ui: &mut Ui, text: &str) -> Response {
    let button = Button::new(RichText::new(text).color(theme::GROUND))
        .fill(theme::SUN)
        .stroke(Stroke::NONE)
        .corner_radius(theme::CONTROL_RADIUS)
        .min_size(vec2(88.0, 40.0));
    ui.add(button).on_hover_cursor(CursorIcon::PointingHand)
}

/// A button for everything else.
pub fn quiet_button(ui: &mut Ui, text: &str) -> Response {
    let button = Button::new(text)
        .fill(Color32::TRANSPARENT)
        .corner_radius(theme::CONTROL_RADIUS)
        .min_size(vec2(88.0, 40.0));
    ui.add(button).on_hover_cursor(CursorIcon::PointingHand)
}

/// A square button that shows only an icon. `name` is what it does, for the
/// tooltip and for screen readers.
pub fn icon_button(ui: &mut Ui, icon: Icon, name: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(36.0), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, name));
    paint_icon_button(ui.painter(), rect, &response, icon);
    response.on_hover_text(name).on_hover_cursor(CursorIcon::PointingHand)
}

/// Like [`icon_button`], on top of something else: at a place chosen by the
/// caller, and taking no room in the layout. `id` tells it from its twins.
pub fn icon_button_at(ui: &Ui, rect: Rect, id: egui::Id, icon: Icon, name: &str) -> Response {
    let response = ui.interact(rect, id, Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, name));
    paint_icon_button(ui.painter(), rect, &response, icon);
    response.on_hover_text(name).on_hover_cursor(CursorIcon::PointingHand)
}

fn paint_icon_button(painter: &Painter, rect: Rect, response: &Response, icon: Icon) {
    let lit = response.hovered() || response.has_focus();
    if lit {
        painter.rect_filled(rect, theme::CONTROL_RADIUS, theme::SURFACE_HOVER);
    }
    icon(painter, rect.shrink(rect.width() * 0.27), if lit { theme::MOON } else { theme::DIM });
}

/// A button with the back arrow and the name of where it leads.
pub fn back_button(ui: &mut Ui, to: &str) -> Response {
    let font = FontId::proportional(theme::BODY);
    let galley = ui.painter().layout_no_wrap(to.to_owned(), font, theme::DIM);
    let size = vec2(14.0 + 8.0 + galley.size().x + 12.0, 36.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, format!("Back to {to}")));
    let lit = response.hovered() || response.has_focus();
    let color = if lit { theme::MOON } else { theme::DIM };
    if lit {
        ui.painter().rect_filled(
            rect.expand2(vec2(6.0, 0.0)),
            theme::CONTROL_RADIUS,
            theme::SURFACE,
        );
    }
    let icon = Rect::from_center_size(rect.left_center() + vec2(7.0, 0.0), Vec2::splat(14.0));
    icons::back(ui.painter(), icon, color);
    let text = Pos2::new(rect.left() + 22.0, rect.center().y - galley.size().y / 2.0);
    ui.painter().galley_with_override_text_color(text, galley, color);
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// Paints one line of text, cut with an ellipsis if it is wider than
/// `max_width`. Returns the space it took.
pub fn line(
    painter: &Painter,
    pos: Pos2,
    text: &str,
    size: f32,
    color: Color32,
    max_width: f32,
) -> Rect {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), FontId::proportional(size), color);
    job.wrap = TextWrapping {
        max_width,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = painter.layout_job(job);
    let rect = Rect::from_min_size(pos, galley.size());
    painter.galley(pos, galley, color);
    rect
}

/// A note in a bordered box, with the "i" icon.
pub fn note(ui: &mut Ui, text: &str) {
    egui::Frame::new()
        .stroke(Stroke::new(1.0, theme::LINE))
        .corner_radius(theme::CONTROL_RADIUS)
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                icons::info(ui.painter(), rect, theme::DIM);
                ui.add(egui::Label::new(dim(text)).wrap());
            });
        });
}

/// A spinner with a line of text, for something that takes a moment.
pub fn waiting(ui: &mut Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.add(egui::Spinner::new().size(16.0).color(theme::SUN));
        ui.label(dim(text));
    });
}
