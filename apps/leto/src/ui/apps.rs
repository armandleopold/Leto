//! The apps of one PC, as a grid of covers.

use std::collections::HashMap;

use egui::{
    Align, Align2, Color32, FontId, Key, Layout, Rect, RichText, Sense, Stroke, StrokeKind,
    TextureHandle, TextureOptions, Ui, Vec2, pos2, vec2,
};
use leto_core::model::{AppSummary, HostId, Reachability};

use super::{icons, widgets};
use crate::{
    state::{AppsView, Loadable, State},
    theme,
};

const TILE_MIN_WIDTH: f32 = 132.0;
const GAP: f32 = 18.0;
/// Height of a cover for a width of 1: the usual box-art shape.
const COVER_SHAPE: f32 = 1.5;

/// The cover pictures received so far, for the apps on screen.
#[derive(Default)]
pub struct Covers {
    /// `None` for an app whose PC has no usable picture.
    pictures: HashMap<(HostId, u32), Option<TextureHandle>>,
}

impl Covers {
    pub fn insert(
        &mut self,
        ctx: &egui::Context,
        host: HostId,
        app: u32,
        image: Option<egui::ColorImage>,
    ) {
        let texture = image.map(|image| {
            ctx.load_texture(format!("cover-{host}-{app}"), image, TextureOptions::LINEAR)
        });
        self.pictures.insert((host, app), texture);
    }

    /// Frees the pictures of every PC but `keep`.
    pub fn keep_only(&mut self, keep: Option<&HostId>) {
        self.pictures.retain(|(host, _), _| Some(host) == keep);
    }

    fn of(&self, host: &HostId, app: u32) -> Cover<'_> {
        match self.pictures.get(&(host.clone(), app)) {
            None => Cover::Loading,
            Some(None) => Cover::Missing,
            Some(Some(texture)) => Cover::Ready(texture),
        }
    }
}

enum Cover<'a> {
    Loading,
    Missing,
    Ready(&'a TextureHandle),
}

pub fn show(ui: &mut Ui, state: &mut State, view: &AppsView, covers: &Covers, now: f64) {
    let host = &view.host;
    let mut leave = ui.input(|input| input.key_pressed(Key::Escape));
    let mut forget = false;

    ui.horizontal(|ui| {
        leave |= widgets::back_button(ui, "PCs").clicked();
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let menu = widgets::icon_button(ui, icons::more, "More");
            egui::Popup::menu(&menu).show(|ui| {
                ui.set_min_width(170.0);
                forget = ui.button("Forget this PC").clicked();
            });
        });
    });
    ui.add_space(4.0);
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        widgets::title(ui, &host.name);
        let online = host.reachability != Reachability::Offline;
        let state_text = match (online, host.busy) {
            (false, _) => "Offline",
            (true, true) => "An app is running",
            (true, false) => "Ready",
        };
        ui.label(widgets::dim(format!("{state_text} · {}", host.kind.name())));
    });
    ui.add_space(14.0);

    match &view.apps {
        Loadable::Loading => widgets::waiting(ui, "Asking the PC for its apps…"),
        Loadable::Failed(problem) => {
            ui.add(egui::Label::new(RichText::new(&problem.message).color(theme::PROBLEM)).wrap());
            ui.add_space(6.0);
            if problem.needs_pairing {
                if widgets::primary_button(ui, "Pair again").clicked() {
                    state.start_pairing(host.clone());
                }
            } else if widgets::primary_button(ui, "Try again").clicked() {
                state.reload_apps();
            }
        }
        Loadable::Ready(apps) if apps.is_empty() => {
            ui.label(widgets::dim(format!(
                "This PC offers no app yet. Add some in {} on the PC.",
                host.kind.name()
            )));
        }
        Loadable::Ready(apps) => {
            widgets::note(
                ui,
                "Leto cannot start a stream yet. Playing these apps is the next part of this step.",
            );
            ui.add_space(12.0);
            grid(ui, &host.id, apps, covers);
        }
    }

    if forget {
        state.forget(&host.id);
    } else if leave {
        state.back(now);
    }
}

fn grid(ui: &mut Ui, host: &HostId, apps: &[AppSummary], covers: &Covers) {
    let available = ui.available_width();
    let columns = (((available + GAP) / (TILE_MIN_WIDTH + GAP)).floor() as usize).clamp(2, 7);
    let width = ((available - GAP * (columns as f32 - 1.0)) / columns as f32).min(210.0);
    for row in apps.chunks(columns) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for app in row {
                tile(ui, app, covers.of(host, app.id), width);
            }
        });
        ui.add_space(GAP - ui.spacing().item_spacing.y);
    }
}

fn tile(ui: &mut Ui, app: &AppSummary, cover: Cover<'_>, width: f32) {
    let cover_size = vec2(width, (width * COVER_SHAPE).round());
    let (rect, response) = ui.allocate_exact_size(cover_size + vec2(0.0, 30.0), Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, app.title.as_str()));
    if !ui.is_rect_visible(rect) {
        return;
    }
    let picture = Rect::from_min_size(rect.min, cover_size);
    let painter = ui.painter();

    match cover {
        Cover::Ready(texture) => {
            egui::Image::new((texture.id(), cover_size))
                .uv(centre_crop(texture.size_vec2(), cover_size))
                .corner_radius(theme::CONTROL_RADIUS)
                .paint_at(ui, picture);
        }
        Cover::Loading | Cover::Missing => {
            painter.rect_filled(picture, theme::CONTROL_RADIUS, theme::SURFACE);
            // The first letter stands in for the picture.
            let letter: String = app.title.chars().take(1).flat_map(char::to_uppercase).collect();
            painter.text(
                picture.center(),
                Align2::CENTER_CENTER,
                letter,
                FontId::proportional(width * 0.42),
                theme::LINE,
            );
        }
    }
    let border = Stroke::new(1.0, theme::LINE);
    painter.rect_stroke(picture, theme::CONTROL_RADIUS, border, StrokeKind::Inside);

    if app.hdr {
        let badge = Rect::from_min_size(
            pos2(picture.right() - 46.0, picture.top() + 8.0),
            vec2(38.0, 20.0),
        );
        painter.rect_filled(badge, 6, Color32::from_black_alpha(190));
        let font = FontId::proportional(11.5);
        painter.text(badge.center(), Align2::CENTER_CENTER, "HDR", font, theme::MOON);
    }

    let title = pos2(rect.left() + 2.0, picture.bottom() + 8.0);
    let drawn =
        widgets::line(painter, title, &app.title, theme::BODY - 0.5, theme::MOON, width - 4.0);
    // The whole title for the ones that were cut.
    if drawn.width() >= width - 6.0 {
        response.on_hover_text(&app.title);
    }
}

/// The part of a picture of size `picture` that fills a frame of size
/// `frame` without distortion, as texture coordinates.
fn centre_crop(picture: Vec2, frame: Vec2) -> Rect {
    if picture.x <= 0.0 || picture.y <= 0.0 || frame.x <= 0.0 || frame.y <= 0.0 {
        return Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    }
    let picture_shape = picture.x / picture.y;
    let frame_shape = frame.x / frame.y;
    let (keep_x, keep_y) = if picture_shape > frame_shape {
        (frame_shape / picture_shape, 1.0)
    } else {
        (1.0, picture_shape / frame_shape)
    };
    Rect::from_center_size(pos2(0.5, 0.5), vec2(keep_x, keep_y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_of_the_right_shape_is_shown_whole() {
        let uv = centre_crop(vec2(300.0, 450.0), vec2(150.0, 225.0));
        assert_eq!(uv, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)));
    }

    #[test]
    fn a_wide_picture_loses_its_sides_and_a_tall_one_its_ends() {
        let wide = centre_crop(vec2(900.0, 450.0), vec2(150.0, 225.0));
        assert!((wide.width() - 1.0 / 3.0).abs() < 1e-5 && wide.height() == 1.0);
        assert!((wide.center().x - 0.5).abs() < 1e-5);

        let tall = centre_crop(vec2(300.0, 900.0), vec2(150.0, 225.0));
        assert!(tall.width() == 1.0 && (tall.height() - 0.5).abs() < 1e-5);
    }
}
