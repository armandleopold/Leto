//! An app of a PC, streamed: the picture fills the window.

use egui::{
    Align, Color32, CursorIcon, Id, Key, Layout, Mesh, Rect, RichText, Sense, TextureHandle,
    TextureOptions, Ui, UiBuilder, Vec2, ViewportCommand, vec2,
};

use super::{icons, widgets};
use crate::{
    state::{State, StreamView},
    theme,
};

/// Height of the bar over the top of the picture.
const BAR: f32 = 76.0;
/// Seconds the bar stays after the pointer last moved.
const BAR_STAYS: f32 = 2.5;

/// The picture of the stream on screen.
#[derive(Default)]
pub struct Canvas {
    texture: Option<TextureHandle>,
    /// The stream screen made the window fill the screen, so leaving the
    /// stream gives the window back.
    filled_screen: bool,
}

impl Canvas {
    /// Shows `picture` from now on.
    pub fn show(&mut self, ctx: &egui::Context, picture: egui::ColorImage) {
        match &mut self.texture {
            Some(texture) => texture.set(picture, TextureOptions::LINEAR),
            None => {
                self.texture = Some(ctx.load_texture("stream", picture, TextureOptions::LINEAR));
            }
        }
    }

    /// The stream is over: forgets the picture and gives the window back.
    pub fn clear(&mut self, ctx: &egui::Context) {
        self.texture = None;
        if std::mem::take(&mut self.filled_screen) {
            ctx.send_viewport_cmd(ViewportCommand::Fullscreen(false));
        }
    }
}

pub fn show(
    ui: &mut Ui,
    state: &mut State,
    view: &StreamView,
    canvas: &mut Canvas,
    sound_problem: Option<&str>,
) {
    let window = ui.max_rect();
    let fills_screen = ui.input(|input| input.viewport().fullscreen.unwrap_or(false));
    let mut leave = false;
    let mut toggle_fill = ui.input(|input| input.key_pressed(Key::F11));
    if ui.input(|input| input.key_pressed(Key::Escape)) {
        // The way out of the full screen first, then out of the stream.
        if fills_screen { toggle_fill = true } else { leave = true }
    }

    ui.painter().rect_filled(window, 0, Color32::BLACK);
    let background = ui.interact(window, Id::unique("stream-picture"), Sense::click());
    toggle_fill |= background.double_clicked();

    match &canvas.texture {
        Some(texture) => {
            let place = fit(texture.size_vec2(), window);
            egui::Image::new((texture.id(), place.size())).paint_at(ui, place);
        }
        None => leave |= waiting(ui, window, view),
    }

    // The bar shows while the pointer moves, and whenever there is no
    // picture to look at.
    let (idle, pointer) =
        ui.input(|input| (input.pointer.time_since_last_movement(), input.pointer.hover_pos()));
    let over_bar = pointer.is_some_and(|pointer| pointer.y < window.top() + BAR);
    let wanted = canvas.texture.is_none() || over_bar || (pointer.is_some() && idle < BAR_STAYS);
    let shown = ui.ctx().animate_bool_with_time(Id::unique("stream-bar"), wanted, 0.2);
    if shown > 0.0 {
        let size = canvas.texture.as_ref().map(TextureHandle::size);
        let (left, filled) = bar(ui, window, view, size, sound_problem, shown);
        leave |= left;
        toggle_fill |= filled;
        // Come back to hide the bar even if no picture arrives meanwhile.
        ui.ctx().request_repaint_after_secs(0.25);
    } else if pointer.is_some() {
        ui.ctx().set_cursor_icon(CursorIcon::None);
    }

    if toggle_fill {
        canvas.filled_screen = !fills_screen;
        ui.ctx().send_viewport_cmd(ViewportCommand::Fullscreen(!fills_screen));
    }
    if leave {
        state.leave_stream();
    }
}

/// What is shown until the first picture. Returns true when the person
/// gives up waiting.
fn waiting(ui: &mut Ui, window: Rect, view: &StreamView) -> bool {
    let text = if view.connected {
        "Waiting for the first picture…".to_owned()
    } else {
        format!("Starting {} on {}…", view.app.title, view.host.name)
    };
    let area = Rect::from_center_size(window.center(), vec2(window.width().min(420.0), 120.0));
    let layout = Layout::top_down(Align::Center);
    ui.scope_builder(UiBuilder::new().max_rect(area).layout(layout), |ui| {
        ui.add(egui::Spinner::new().size(26.0).color(theme::SUN));
        ui.add_space(6.0);
        ui.add(egui::Label::new(widgets::dim(text)).wrap().halign(Align::Center));
        ui.add_space(6.0);
        widgets::quiet_button(ui, "Cancel").clicked()
    })
    .inner
}

/// The bar over the top of the picture. Returns whether the person asked to
/// leave, and whether they asked to fill the screen or stop filling it.
fn bar(
    ui: &mut Ui,
    window: Rect,
    view: &StreamView,
    picture: Option<[usize; 2]>,
    sound_problem: Option<&str>,
    opacity: f32,
) -> (bool, bool) {
    let rect = Rect::from_min_size(window.min, vec2(window.width(), BAR));
    // Dark at the top and fading out, so the text reads on any picture.
    let mut shade = Mesh::default();
    let top = Color32::from_black_alpha((215.0 * opacity) as u8);
    for (corner, color) in [
        (rect.left_top(), top),
        (rect.right_top(), top),
        (rect.right_bottom(), Color32::TRANSPARENT),
        (rect.left_bottom(), Color32::TRANSPARENT),
    ] {
        shade.colored_vertex(corner, color);
    }
    shade.add_triangle(0, 1, 2);
    shade.add_triangle(0, 2, 3);
    ui.painter().add(shade);

    let side = if window.width() < 560.0 { 12.0 } else { 20.0 };
    let content = Rect::from_min_max(
        rect.min + vec2(side + 6.0, 8.0),
        egui::pos2(rect.right() - side, rect.top() + 48.0),
    );
    let layout = Layout::left_to_right(Align::Center);
    ui.scope_builder(UiBuilder::new().max_rect(content).layout(layout), |ui| {
        ui.set_opacity(opacity);
        let leave = widgets::back_button(ui, "Apps").clicked();
        ui.add_space(8.0);
        ui.label(RichText::new(&view.app.title).color(theme::MOON));
        ui.label(widgets::dim(format!("· {}", view.host.name)));

        let mut fill = false;
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            fill = widgets::icon_button(ui, icons::expand, "Full screen (F11)").clicked();
            if let Some(problem) = sound_problem {
                let text = RichText::new("No sound").color(theme::PROBLEM).size(theme::SMALL);
                ui.label(text).on_hover_text(problem);
            }
            if let Some([width, height]) = picture {
                ui.label(widgets::dim(format!("{width} × {height}")).size(theme::SMALL));
            }
        });
        (leave, fill)
    })
    .inner
}

/// Where a picture of size `picture` goes in `window`: as large as fits,
/// undistorted and centred, with black around it.
fn fit(picture: Vec2, window: Rect) -> Rect {
    if picture.x <= 0.0 || picture.y <= 0.0 {
        return window;
    }
    let scale = (window.width() / picture.x).min(window.height() / picture.y);
    Rect::from_center_size(window.center(), picture * scale)
}

#[cfg(test)]
mod tests {
    use egui::pos2;

    use super::*;

    #[test]
    fn a_picture_of_the_windows_shape_fills_it() {
        let window = Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 540.0));
        assert_eq!(fit(vec2(1920.0, 1080.0), window), window);
    }

    #[test]
    fn a_picture_of_another_shape_is_centred_with_black_around_it() {
        // A tall window: black above and below.
        let tall = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 900.0));
        let place = fit(vec2(1920.0, 1080.0), tall);
        assert_eq!(place.size(), vec2(800.0, 450.0));
        assert_eq!(place.center(), tall.center());

        // A wide window: black left and right.
        let wide = Rect::from_min_size(pos2(0.0, 0.0), vec2(2000.0, 540.0));
        let place = fit(vec2(1920.0, 1080.0), wide);
        assert_eq!(place.size(), vec2(960.0, 540.0));
        assert_eq!(place.center(), wide.center());
    }

    #[test]
    fn a_picture_with_no_size_is_no_reason_to_divide_by_zero() {
        let window = Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 540.0));
        assert_eq!(fit(vec2(0.0, 0.0), window), window);
    }
}
