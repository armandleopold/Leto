//! The apps of one PC, as a grid of covers.

use std::collections::HashMap;

use egui::{
    Align, Align2, Color32, CursorIcon, FontId, Key, Layout, Rect, RichText, Sense, Stroke,
    StrokeKind, TextureHandle, TextureOptions, Ui, Vec2, pos2, vec2,
};
use leto_core::model::{AppSummary, HostId, Reachability};

use super::{icons, widgets};
use crate::{
    state::{AppsView, Loadable, State},
    theme, window,
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
        let kind = host.kind.name();
        let line = match (online, host.busy) {
            (false, _) => format!("Offline · {kind}"),
            // The card below says which app is running.
            (true, true) if shows_running_card(view) => kind.to_owned(),
            (true, true) => format!("An app is running · {kind}"),
            (true, false) => format!("Ready · {kind}"),
        };
        ui.label(widgets::dim(line));
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
            let mut chosen = None;
            let mut quit = false;
            if let Some(notice) = &view.notice {
                let color = if notice.problem { theme::PROBLEM } else { theme::DIM };
                ui.add(egui::Label::new(RichText::new(&notice.text).color(color)).wrap());
                ui.add_space(4.0);
            }
            if shows_running_card(view) {
                let (resume, asked_to_quit) = running_card(ui, view);
                chosen = resume.then(|| view.running()).flatten();
                quit = asked_to_quit;
                ui.add_space(8.0);
            }
            chosen = grid(ui, view, apps, covers).or(chosen);

            if quit {
                state.quit_app();
            } else if let Some(app) = chosen {
                // The stream is made for the screen the window is on.
                state.play(app, window::screen(ui.ctx()));
            }
        }
    }

    if forget {
        state.forget(&host.id);
    } else if leave {
        state.back(now);
    }
}

fn shows_running_card(view: &AppsView) -> bool {
    view.host.running_app.is_some() && matches!(view.apps, Loadable::Ready(_))
}

/// Says which app is running on the PC and offers to go back to it or to
/// close it. Returns which of the two the person asked for.
fn running_card(ui: &mut Ui, view: &AppsView) -> (bool, bool) {
    let mut resume = false;
    let mut quit = false;
    egui::Frame::new()
        .fill(theme::SURFACE)
        .corner_radius(theme::CARD_RADIUS)
        .inner_margin(egui::Margin::symmetric(16, 10))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(40.0);
                let (dot, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                ui.painter().circle_filled(dot.center(), 4.0, theme::READY);
                let text = match view.running() {
                    Some(app) => format!("{} is running", app.title),
                    None => "An app is running".to_owned(),
                };
                ui.add(egui::Label::new(text).truncate());

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if view.quitting {
                        widgets::waiting(ui, "Closing…");
                        return;
                    }
                    if view.running().is_some() {
                        resume = widgets::primary_button(ui, "Resume").clicked();
                    }
                    quit = widgets::quiet_button(ui, "Close it").clicked();
                });
            });
        });
    (resume, quit)
}

/// The covers. Returns the app the person chose, if any.
fn grid<'a>(
    ui: &mut Ui,
    view: &AppsView,
    apps: &'a [AppSummary],
    covers: &Covers,
) -> Option<&'a AppSummary> {
    let available = ui.available_width();
    let columns = (((available + GAP) / (TILE_MIN_WIDTH + GAP)).floor() as usize).clamp(2, 7);
    let width = ((available - GAP * (columns as f32 - 1.0)) / columns as f32).min(210.0);
    let mut chosen = None;
    for row in apps.chunks(columns) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for app in row {
                let running = view.host.running_app == Some(app.id);
                if tile(ui, app, covers.of(&view.host.id, app.id), width, running) {
                    chosen = Some(app);
                }
            }
        });
        ui.add_space(GAP - ui.spacing().item_spacing.y);
    }
    chosen
}

/// One app: its cover and its title. Returns true when the person chose it.
fn tile(ui: &mut Ui, app: &AppSummary, cover: Cover<'_>, width: f32, running: bool) -> bool {
    let cover_size = vec2(width, (width * COVER_SHAPE).round());
    let (rect, response) = ui.allocate_exact_size(cover_size + vec2(0.0, 30.0), Sense::click());
    response.widget_info(|| {
        let name = if running { format!("Resume {}", app.title) } else { app.title.clone() };
        egui::WidgetInfo::labeled(egui::Role::Button, true, name)
    });
    let chosen = response.clicked();
    if !ui.is_rect_visible(rect) {
        return chosen;
    }
    let lit = response.hovered() || response.has_focus();
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
    let border = if lit { Stroke::new(2.0, theme::SUN) } else { Stroke::new(1.0, theme::LINE) };
    painter.rect_stroke(picture, theme::CONTROL_RADIUS, border, StrokeKind::Inside);

    if running {
        let font = FontId::proportional(11.5);
        let label = painter.layout_no_wrap("Running".to_owned(), font, theme::MOON);
        let badge = Rect::from_min_size(
            picture.left_top() + vec2(8.0, 8.0),
            vec2(label.size().x + 26.0, 20.0),
        );
        painter.rect_filled(badge, 6, Color32::from_black_alpha(190));
        painter.circle_filled(badge.left_center() + vec2(10.0, 0.0), 3.5, theme::READY);
        let text = pos2(badge.left() + 19.0, badge.center().y - label.size().y / 2.0);
        painter.galley(text, label, theme::MOON);
    }

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
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    // The whole title for the ones that were cut.
    if drawn.width() >= width - 6.0 {
        response.on_hover_text(&app.title);
    }
    chosen
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
