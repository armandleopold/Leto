//! Where the camera and the microphone of this device are tried out, with no
//! PC involved: what the camera shows, and whether the microphone hears.

use egui::{
    Align, Color32, Layout, Painter, Rect, RichText, Sense, Stroke, TextureHandle, TextureOptions,
    Ui, UiBuilder, Vec2, pos2, vec2,
};

use super::{icons, stream::meter, widgets};
use crate::{
    backend::Seen,
    state::{Device, Devices, State, Trial},
    theme,
};

/// From this width on the two cards stand side by side.
const SIDE_BY_SIDE: f32 = 760.0;
const GAP: f32 = 16.0;

/// What the camera shows, kept from one frame to the next.
#[derive(Default)]
pub struct Preview {
    texture: Option<TextureHandle>,
    /// The size the camera captures at.
    size: [usize; 2],
}

impl Preview {
    /// Shows `seen` from now on.
    pub fn show(&mut self, ctx: &egui::Context, seen: Seen) {
        self.size = seen.size;
        match &mut self.texture {
            Some(texture) => texture.set(seen.picture, TextureOptions::LINEAR),
            None => {
                let texture = ctx.load_texture("camera", seen.picture, TextureOptions::LINEAR);
                self.texture = Some(texture);
            }
        }
    }

    /// Forgets the picture: the camera is off.
    pub fn clear(&mut self) {
        self.texture = None;
    }
}

/// `heard` is how loud the microphone is, from 0 to 1, while it captures.
pub fn show(
    ui: &mut Ui,
    state: &mut State,
    devices: &Devices,
    preview: &Preview,
    heard: Option<f32>,
    now: f64,
) {
    if widgets::back_button(ui, "PCs").clicked() {
        state.back(now);
        return;
    }
    ui.add_space(4.0);
    widgets::title(ui, "Camera and microphone");
    ui.add(
        egui::Label::new(widgets::dim(
            "See what your camera shows and check that your microphone hears you. Nothing \
             leaves this device from here.",
        ))
        .wrap(),
    );
    ui.add_space(8.0);

    let mut switched = None;
    let width = ui.available_width();
    if width >= SIDE_BY_SIDE {
        let left = ((width - GAP) * 0.6).floor();
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            column(ui, left, |ui| {
                if camera_card(ui, &devices.camera, preview) {
                    switched = Some(Device::Camera);
                }
            });
            column(ui, width - GAP - left, |ui| {
                if microphone_card(ui, &devices.microphone, heard) {
                    switched = Some(Device::Microphone);
                }
            });
        });
    } else {
        if camera_card(ui, &devices.camera, preview) {
            switched = Some(Device::Camera);
        }
        ui.add_space(GAP - ui.spacing().item_spacing.y);
        if microphone_card(ui, &devices.microphone, heard) {
            switched = Some(Device::Microphone);
        }
    }
    if let Some(device) = switched {
        state.switch_device(device);
    }
}

fn column(ui: &mut Ui, width: f32, add: impl FnOnce(&mut Ui)) {
    ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::top_down(Align::Min), |ui| {
        ui.set_width(width);
        add(ui);
    });
}

fn card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(theme::SURFACE)
        .stroke(Stroke::new(1.0, theme::LINE))
        .corner_radius(theme::CARD_RADIUS)
        .inner_margin(egui::Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

/// The top of a card: what it is about, and its switch. Returns true when
/// the person pressed the switch.
fn heading(
    ui: &mut Ui,
    icon: impl FnOnce(&Painter, Rect, Color32),
    name: &str,
    trial: &Trial,
) -> bool {
    // As tall as its switch from the start, so that the name is level with
    // it.
    let size = vec2(ui.available_width(), 40.0);
    ui.allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), |ui| {
        let on = *trial == Trial::On;
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
        icon(ui.painter(), rect, if on { theme::SUN } else { theme::DIM });
        ui.label(RichText::new(name).size(18.0));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| match trial {
            Trial::On => widgets::quiet_button(ui, "Turn off").clicked(),
            Trial::Off => widgets::primary_button(ui, "Turn on").clicked(),
            Trial::Failed(_) => widgets::primary_button(ui, "Try again").clicked(),
        })
        .inner
    })
    .inner
}

fn camera_card(ui: &mut Ui, trial: &Trial, preview: &Preview) -> bool {
    let mut switched = false;
    card(ui, |ui| {
        switched = heading(ui, icons::camera, "Camera", trial);
        ui.add_space(4.0);

        let width = ui.available_width();
        let (view, _) =
            ui.allocate_exact_size(vec2(width, (width * 9.0 / 16.0).round()), Sense::hover());
        ui.painter().rect_filled(view, theme::CONTROL_RADIUS, theme::GROUND);
        match (trial, &preview.texture) {
            (Trial::On, Some(texture)) => {
                // The whole picture, whatever its shape.
                let size = texture.size_vec2();
                let scale = (view.width() / size.x).min(view.height() / size.y);
                let shown = Rect::from_center_size(view.center(), size * scale);
                // As in a mirror: what is on the person's left shows on the
                // left.
                let mirrored = Rect::from_min_max(pos2(1.0, 0.0), pos2(0.0, 1.0));
                egui::Image::new((texture.id(), shown.size()))
                    .uv(mirrored)
                    .corner_radius(theme::CONTROL_RADIUS)
                    .paint_at(ui, shown);
            }
            (Trial::On, None) => inside(ui, view, 50.0, |ui| {
                ui.add(egui::Spinner::new().size(22.0).color(theme::SUN));
                ui.label(widgets::dim("Waiting for the camera…"));
            }),
            (Trial::Off, _) => inside(ui, view, 68.0, |ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
                icons::camera(ui.painter(), rect, theme::LINE);
                ui.label(widgets::dim("The camera is off."));
            }),
            (Trial::Failed(why), _) => inside(ui, view, 60.0, |ui| {
                let why = RichText::new(why).color(theme::PROBLEM);
                ui.add(egui::Label::new(why).wrap().halign(Align::Center));
            }),
        }

        ui.add_space(2.0);
        match (trial, &preview.texture) {
            (Trial::On, Some(_)) => {
                let [width, height] = preview.size;
                live(ui, &format!("The camera is on · {width} × {height}, shown as in a mirror"));
            }
            (Trial::Off, _) => {
                ui.label(widgets::dim("It stays off until you turn it on.").size(theme::SMALL));
            }
            // The line stays, so that the card keeps its height.
            _ => {
                ui.label(RichText::new(" ").size(theme::SMALL));
            }
        }
    });
    switched
}

fn microphone_card(ui: &mut Ui, trial: &Trial, heard: Option<f32>) -> bool {
    let mut switched = false;
    card(ui, |ui| {
        let level = heard.map_or(0.0, meter);
        let icon = |painter: &Painter, rect: Rect, color: Color32| {
            icons::microphone(painter, rect, color, 0.0);
        };
        switched = heading(ui, icon, "Microphone", trial);
        ui.add_space(4.0);

        // How loud it is: a bar that fills from the left.
        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 10.0), Sense::hover());
        ui.painter().rect_filled(bar, 5.0, theme::GROUND);
        if level > 0.0 {
            let full = Rect::from_min_size(bar.min, vec2(bar.width() * level, bar.height()));
            ui.painter().with_clip_rect(full).rect_filled(bar, 5.0, theme::SUN);
        }

        ui.add_space(2.0);
        match (trial, heard) {
            (Trial::On, Some(_)) => {
                live(ui, "The microphone is on. Say something: the bar follows your voice.");
            }
            (Trial::On, None) => widgets::waiting(ui, "Waiting for the microphone…"),
            (Trial::Off, _) => {
                ui.label(widgets::dim("It stays off until you turn it on.").size(theme::SMALL));
            }
            (Trial::Failed(why), _) => {
                ui.add(egui::Label::new(RichText::new(why).color(theme::PROBLEM)).wrap());
            }
        }
    });
    switched
}

/// The sign that a device captures: a lit dot and what it says.
fn live(ui: &mut Ui, text: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (dot, _) = ui.allocate_exact_size(vec2(8.0, 18.0), Sense::hover());
        ui.painter().circle_filled(dot.center(), 4.0, theme::SUN);
        ui.label(RichText::new(text).size(theme::SMALL));
    });
}

/// Lays `add`, which is about `height` tall, out in the middle of `view`
/// and on top of it: the layout around it does not move.
fn inside(ui: &mut Ui, view: Rect, height: f32, add: impl FnOnce(&mut Ui)) {
    let size = vec2((view.width() - 36.0).clamp(40.0, 420.0), height.min(view.height()));
    let area = Rect::from_center_size(view.center(), size);
    let layout = Layout::top_down(Align::Center);
    let mut ui = ui.new_child(UiBuilder::new().max_rect(area).layout(layout));
    ui.spacing_mut().item_spacing.y = 8.0;
    add(&mut ui);
}
