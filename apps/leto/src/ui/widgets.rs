//! Buttons, text and layout helpers shared by the screens.

use egui::{
    Align, Button, Color32, CursorIcon, Event, FontId, Id, Key, Layout, Modifiers, Painter,
    PointerButton, Pos2, Rect, Response, RichText, Role, Sense, Stroke, Ui, UiBuilder, Vec2,
    WidgetInfo,
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
    RichText::new(text).color(theme::dim())
}

/// The title of a screen.
pub fn title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(theme::TITLE));
}

/// The one button a screen wants pressed.
pub fn primary_button(ui: &mut Ui, text: &str) -> Response {
    let button = Button::new(RichText::new(text).color(theme::on_sun()))
        .fill(theme::sun())
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

/// A square button that shows a state: its icon is drawn in `color`, by a
/// function that knows more than a plain icon does. One that is not
/// `usable` only says why, in `name`.
pub fn state_button(
    ui: &mut Ui,
    icon: impl Fn(&Painter, Rect, Color32),
    name: &str,
    color: Color32,
    usable: bool,
) -> Response {
    let sense = if usable { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(36.0), sense);
    response.widget_info(|| WidgetInfo::labeled(Role::Button, usable, name));
    if usable && (response.hovered() || response.has_focus()) {
        ui.painter().rect_filled(rect, theme::CONTROL_RADIUS, theme::surface_hover());
    }
    icon(ui.painter(), rect.shrink(rect.width() * 0.27), color);
    let response = response.on_hover_text(name);
    if usable { response.on_hover_cursor(CursorIcon::PointingHand) } else { response }
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
        painter.rect_filled(rect, theme::CONTROL_RADIUS, theme::surface_hover());
    }
    icon(painter, rect.shrink(rect.width() * 0.27), if lit { theme::text() } else { theme::dim() });
}

/// A switch that turns `on` on or off when clicked. `name` is what it
/// turns on, for screen readers.
pub fn switch(ui: &mut Ui, on: &mut bool, name: &str) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(44.0, 24.0), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let enabled = ui.is_enabled();
    response.widget_info(|| WidgetInfo::selected(Role::Switch, enabled, *on, name));
    let how_on = ui.ctx().animate_bool_responsive(response.id, *on);
    let lit = response.hovered() || response.has_focus();
    let track = if *on {
        theme::sun()
    } else if lit {
        theme::surface_hover()
    } else {
        theme::ground()
    };
    let line = if *on {
        theme::sun()
    } else if lit {
        theme::dim()
    } else {
        theme::line()
    };
    let radius = rect.height() / 2.0;
    ui.painter().rect(rect, radius, track, Stroke::new(1.0, line), egui::StrokeKind::Inside);
    let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), how_on);
    let knob = if *on { theme::on_sun() } else { theme::dim() };
    ui.painter().circle_filled(Pos2::new(x, rect.center().y), radius * 0.62, knob);
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// Whether the person asks to go back in this frame, as a back button
/// would: with Escape, the back button of a mouse or of a keyboard, Android's
/// back gesture or button, Alt+Left, or Backspace away from text fields.
/// Only the first to ask in a frame hears yes: one press goes back one
/// step, never two.
pub fn back_asked(ui: &Ui) -> bool {
    let typing = ui.ctx().egui_wants_keyboard_input();
    let asked = ui.input(|input| {
        // The keys held with each key as it was pressed.
        let pressed = |wanted: Key, held: Modifiers| {
            input.events.iter().any(|event| {
                matches!(event, Event::Key { key, pressed: true, modifiers, .. }
                    if *key == wanted && modifiers.matches_exact(held))
            })
        };
        input.key_pressed(Key::Escape)
            || input.key_pressed(Key::BrowserBack)
            || input.pointer.button_pressed(PointerButton::Extra1)
            || (!typing && pressed(Key::Backspace, Modifiers::NONE))
            || (!typing && pressed(Key::ArrowLeft, Modifiers::ALT))
    });
    if !asked {
        return false;
    }
    let pass = ui.ctx().cumulative_pass_nr();
    let id = Id::unique("leto-went-back");
    ui.ctx().data_mut(|data| {
        let first = data.get_temp::<u64>(id) != Some(pass);
        data.insert_temp(id, pass);
        first
    })
}

/// A button with the back arrow and the name of where it leads.
pub fn back_button(ui: &mut Ui, to: &str) -> Response {
    let font = FontId::proportional(theme::BODY);
    let galley = ui.painter().layout_no_wrap(to.to_owned(), font, theme::dim());
    let size = vec2(14.0 + 8.0 + galley.size().x + 12.0, 36.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, format!("Back to {to}")));
    let lit = response.hovered() || response.has_focus();
    let color = if lit { theme::text() } else { theme::dim() };
    if lit {
        ui.painter().rect_filled(
            rect.expand2(vec2(6.0, 0.0)),
            theme::CONTROL_RADIUS,
            theme::surface(),
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
        .stroke(Stroke::new(1.0, theme::line()))
        .corner_radius(theme::CONTROL_RADIUS)
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                icons::info(ui.painter(), rect, theme::dim());
                ui.add(egui::Label::new(dim(text)).wrap());
            });
        });
}

/// A spinner with a line of text, for something that takes a moment.
pub fn waiting(ui: &mut Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.add(egui::Spinner::new().size(16.0).color(theme::sun()));
        ui.label(dim(text));
    });
}

#[cfg(test)]
mod tests {
    use egui::{RawInput, pos2};

    use super::*;

    /// Runs one frame with `events`, a text field focused or not, and
    /// returns how many of two screens asking heard yes.
    fn asked(ctx: &egui::Context, events: Vec<Event>, typing: bool) -> usize {
        let input = RawInput { events, ..Default::default() };
        let mut yes = 0;
        let mut output = ctx.run_ui(input, |ui| {
            let mut text = String::new();
            let field = ui.text_edit_singleline(&mut text);
            if typing {
                field.request_focus();
            } else {
                field.surrender_focus();
            }
            yes += usize::from(back_asked(ui)) + usize::from(back_asked(ui));
        });
        output.textures_delta.clear();
        yes
    }

    fn key(key: Key, modifiers: Modifiers) -> Event {
        Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
    }

    #[test]
    fn back_is_asked_in_every_usual_way_and_heard_once() {
        let ctx = egui::Context::default();
        // A frame first, for the text field to be focused in the next.
        asked(&ctx, Vec::new(), false);
        let none = Modifiers::NONE;
        let mouse_back = |pressed| Event::PointerButton {
            pos: pos2(10.0, 10.0),
            button: PointerButton::Extra1,
            pressed,
            modifiers: none,
        };
        let ways = [
            vec![key(Key::Escape, none)],
            vec![key(Key::BrowserBack, none)],
            vec![key(Key::ArrowLeft, Modifiers::ALT)],
            vec![key(Key::Backspace, none)],
            vec![Event::PointerMoved(pos2(10.0, 10.0)), mouse_back(true), mouse_back(false)],
        ];
        for events in ways {
            assert_eq!(asked(&ctx, events.clone(), false), 1, "{events:?}");
        }
        assert_eq!(asked(&ctx, Vec::new(), false), 0);
        assert_eq!(asked(&ctx, vec![key(Key::ArrowLeft, none)], false), 0);
    }

    #[test]
    fn backspace_and_alt_left_are_the_text_fields_while_one_is_typed_in() {
        let ctx = egui::Context::default();
        asked(&ctx, Vec::new(), true);
        assert_eq!(asked(&ctx, vec![key(Key::Backspace, Modifiers::NONE)], true), 0);
        assert_eq!(asked(&ctx, vec![key(Key::ArrowLeft, Modifiers::ALT)], true), 0);
        // Escape still goes back, as it did.
        assert_eq!(asked(&ctx, vec![key(Key::Escape, Modifiers::NONE)], true), 1);
    }
}
