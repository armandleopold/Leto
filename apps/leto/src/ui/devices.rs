//! Where the camera, the microphone and the sound output of this device are
//! chosen and tried out, with no PC involved: what the camera shows, whether
//! the microphone hears, and where sound plays.

use egui::{
    Align, Color32, Layout, Painter, Rect, RichText, Sense, Stroke, TextureHandle, TextureOptions,
    Ui, UiBuilder, Vec2, pos2, vec2,
};

use super::{icons, stream::meter, widgets};
use crate::{
    backend::{Chime, DeviceNames, Seen},
    settings::DeviceChoices,
    state::{Device, Devices, State, Trial},
    theme,
};

/// The page's name. In a browser, which chooses the devices itself, it is
/// about the camera and the microphone only.
#[cfg(not(target_arch = "wasm32"))]
pub const NAME: &str = "Camera, microphone and sound";
#[cfg(target_arch = "wasm32")]
pub const NAME: &str = "Camera and microphone";

/// The choice that leaves the camera to Leto: the first one that films in
/// colour.
const ANY_CAMERA: &str = "Automatic";
/// The choice that leaves a microphone or a sound output to the system.
const SYSTEM_DEFAULT: &str = "System default";

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

/// The devices the person chooses from on this page, and the test sound.
pub struct Choices<'a> {
    pub chosen: &'a mut DeviceChoices,
    /// `None` where Leto does not choose: a browser asks the person itself.
    pub names: Option<&'a DeviceNames>,
    pub chime: &'a Chime,
}

/// What the person asked for on this page, beyond what it changed itself.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Asked {
    /// They opened a list: a device plugged in since it was made belongs in
    /// it.
    pub look_for_devices: bool,
    pub test_sound: bool,
}

/// `heard` is how loud the microphone is, from 0 to 1, while it captures.
pub fn show(
    ui: &mut Ui,
    state: &mut State,
    devices: &Devices,
    preview: &Preview,
    heard: Option<f32>,
    choices: Choices<'_>,
    now: f64,
) -> Asked {
    let mut asked = Asked::default();
    if widgets::back_button(ui, "PCs").clicked() || widgets::back_asked(ui) {
        state.back(now);
        return asked;
    }
    ui.add_space(4.0);
    widgets::title(ui, NAME);
    let about = if choices.names.is_some() {
        "Choose the camera, microphone and sound output Leto uses, and check them: see what the \
         camera shows, that the microphone hears you, and where sound plays. Nothing leaves \
         this device from here."
    } else {
        "See what your camera shows and check that your microphone hears you. Nothing leaves \
         this device from here."
    };
    ui.add(egui::Label::new(widgets::dim(about)).wrap());
    ui.add_space(8.0);

    let Choices { chosen, names, chime } = choices;
    let mut switched = None;
    let mut camera = |ui: &mut Ui, asked: &mut Asked| {
        let cameras = names.map(|names| names.cameras.as_deref());
        match camera_card(ui, &devices.camera, preview, &mut chosen.camera, cameras, asked) {
            Pressed::Switch => switched = Some(Device::Camera),
            // Another camera is tried at once after one that failed.
            Pressed::Choice if matches!(devices.camera, Trial::Failed(_)) => {
                switched = Some(Device::Camera);
            }
            Pressed::Choice | Pressed::Nothing => {}
        }
    };
    let mut microphone_switched = None;
    let mut sound = |ui: &mut Ui, asked: &mut Asked| {
        let microphones = names.map(|names| names.microphones.as_slice());
        let pressed = microphone_card(
            ui,
            &devices.microphone,
            heard,
            &mut chosen.microphone,
            microphones,
            asked,
        );
        match pressed {
            Pressed::Switch => microphone_switched = Some(Device::Microphone),
            Pressed::Choice if matches!(devices.microphone, Trial::Failed(_)) => {
                microphone_switched = Some(Device::Microphone);
            }
            Pressed::Choice | Pressed::Nothing => {}
        }
        if let Some(names) = names {
            ui.add_space(GAP - ui.spacing().item_spacing.y);
            sound_card(ui, &mut chosen.speakers, &names.speakers, chime, asked);
        }
    };
    let width = ui.available_width();
    if width >= SIDE_BY_SIDE {
        let left = ((width - GAP) * 0.6).floor();
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            column(ui, left, |ui| camera(ui, &mut asked));
            column(ui, width - GAP - left, |ui| sound(ui, &mut asked));
        });
    } else {
        camera(ui, &mut asked);
        ui.add_space(GAP - ui.spacing().item_spacing.y);
        sound(ui, &mut asked);
    }
    for device in [switched, microphone_switched].into_iter().flatten() {
        state.switch_device(device);
    }
    asked
}

/// What the person pressed on a card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pressed {
    Nothing,
    /// Its switch.
    Switch,
    /// Another device in its list.
    Choice,
}

fn column(ui: &mut Ui, width: f32, add: impl FnOnce(&mut Ui)) {
    ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::top_down(Align::Min), |ui| {
        ui.set_width(width);
        add(ui);
    });
}

fn card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(theme::surface())
        .stroke(Stroke::new(1.0, theme::line()))
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
    top(ui, icon, name, *trial == Trial::On, |ui| match trial {
        Trial::On => widgets::quiet_button(ui, "Turn off").clicked(),
        Trial::Off => widgets::primary_button(ui, "Turn on").clicked(),
        Trial::Failed(_) => widgets::primary_button(ui, "Try again").clicked(),
    })
}

/// The top of a card: its icon, lit while the device is at work, its name,
/// and `button` on the right.
fn top<R>(
    ui: &mut Ui,
    icon: impl FnOnce(&Painter, Rect, Color32),
    name: &str,
    lit: bool,
    button: impl FnOnce(&mut Ui) -> R,
) -> R {
    // As tall as its button from the start, so that the name is level with
    // it.
    let size = vec2(ui.available_width(), 40.0);
    ui.allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), |ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
        icon(ui.painter(), rect, if lit { theme::sun() } else { theme::dim() });
        ui.label(RichText::new(name).size(18.0));
        ui.with_layout(Layout::right_to_left(Align::Center), button).inner
    })
    .inner
}

/// The list to choose a device from, as wide as its card: `automatic`
/// first, then the devices by the names the system gives them, or a word
/// that they are being looked for while `names` is `None`. A device chosen
/// before that is not there now stays in the list, marked so. Returns
/// whether the person chose another.
fn picker(
    ui: &mut Ui,
    id: &str,
    chosen: &mut Option<String>,
    names: Option<&[String]>,
    automatic: &str,
    asked: &mut Asked,
) -> bool {
    let before = chosen.clone();
    let gone = before.clone().filter(|name| names.is_some_and(|names| !names.contains(name)));
    let away = |name: &str| format!("{name} (not connected)");
    let selected = match (&before, &gone) {
        (_, Some(name)) => away(name),
        (Some(name), None) => name.clone(),
        (None, None) => automatic.to_owned(),
    };
    let width = ui.available_width();
    let list = egui::ComboBox::from_id_salt(id)
        .width(width)
        .truncate()
        .selected_text(selected)
        .show_ui(ui, |ui| {
            // No wider than the list's button: a long name is cut short
            // there too, and is whole when the pointer rests on it.
            ui.set_max_width(width);
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            ui.selectable_value(chosen, None, automatic);
            match names {
                Some(names) => {
                    for name in names {
                        ui.selectable_value(chosen, Some(name.clone()), name).on_hover_text(name);
                    }
                }
                None => {
                    ui.label(widgets::dim("Looking for them…"));
                }
            }
            if let Some(name) = gone {
                let shown = away(&name);
                ui.selectable_value(chosen, Some(name), shown);
            }
        });
    if list.response.clicked() {
        asked.look_for_devices = true;
    }
    *chosen != before
}

/// `names` is `None` where Leto does not choose the camera, and holds
/// `None` while the cameras are being looked for.
fn camera_card(
    ui: &mut Ui,
    trial: &Trial,
    preview: &Preview,
    chosen: &mut Option<String>,
    names: Option<Option<&[String]>>,
    asked: &mut Asked,
) -> Pressed {
    let mut pressed = Pressed::Nothing;
    card(ui, |ui| {
        if heading(ui, icons::camera, "Camera", trial) {
            pressed = Pressed::Switch;
        }
        ui.add_space(4.0);
        if let Some(names) = names {
            if picker(ui, "camera", chosen, names, ANY_CAMERA, asked) {
                pressed = Pressed::Choice;
            }
            ui.add_space(4.0);
        }

        let width = ui.available_width();
        let (view, _) =
            ui.allocate_exact_size(vec2(width, (width * 9.0 / 16.0).round()), Sense::hover());
        ui.painter().rect_filled(view, theme::CONTROL_RADIUS, theme::ground());
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
                ui.add(egui::Spinner::new().size(22.0).color(theme::sun()));
                ui.label(widgets::dim("Waiting for the camera…"));
            }),
            (Trial::Off, _) => inside(ui, view, 68.0, |ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
                icons::camera(ui.painter(), rect, theme::line());
                ui.label(widgets::dim("The camera is off."));
            }),
            (Trial::Failed(why), _) => inside(ui, view, 60.0, |ui| {
                let why = RichText::new(why).color(theme::problem());
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
    pressed
}

/// `names` is `None` where Leto does not choose the microphone.
fn microphone_card(
    ui: &mut Ui,
    trial: &Trial,
    heard: Option<f32>,
    chosen: &mut Option<String>,
    names: Option<&[String]>,
    asked: &mut Asked,
) -> Pressed {
    let mut pressed = Pressed::Nothing;
    card(ui, |ui| {
        let level = heard.map_or(0.0, meter);
        let icon = |painter: &Painter, rect: Rect, color: Color32| {
            icons::microphone(painter, rect, color, 0.0);
        };
        if heading(ui, icon, "Microphone", trial) {
            pressed = Pressed::Switch;
        }
        ui.add_space(4.0);
        if let Some(names) = names {
            if picker(ui, "microphone", chosen, Some(names), SYSTEM_DEFAULT, asked) {
                pressed = Pressed::Choice;
            }
            ui.add_space(6.0);
        }

        // How loud it is: a bar that fills from the left.
        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 10.0), Sense::hover());
        ui.painter().rect_filled(bar, 5.0, theme::ground());
        if level > 0.0 {
            let full = Rect::from_min_size(bar.min, vec2(bar.width() * level, bar.height()));
            ui.painter().with_clip_rect(full).rect_filled(bar, 5.0, theme::sun());
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
                ui.add(egui::Label::new(RichText::new(why).color(theme::problem())).wrap());
            }
        }
    });
    pressed
}

/// Where the sound of the PCs plays, and a sound to hear that it does.
fn sound_card(
    ui: &mut Ui,
    chosen: &mut Option<String>,
    names: &[String],
    chime: &Chime,
    asked: &mut Asked,
) {
    card(ui, |ui| {
        let playing = *chime == Chime::Playing;
        let pressed = top(ui, icons::speaker, "Sound output", playing, |ui| {
            ui.add_enabled_ui(!playing, |ui| widgets::quiet_button(ui, "Play a test sound"))
                .inner
                .clicked()
        });
        asked.test_sound |= pressed;
        ui.add_space(4.0);
        picker(ui, "speakers", chosen, Some(names), SYSTEM_DEFAULT, asked);
        ui.add_space(2.0);
        match chime {
            Chime::Playing => live(ui, "Playing the test sound…"),
            Chime::Failed(why) => {
                ui.add(egui::Label::new(RichText::new(why).color(theme::problem())).wrap());
            }
            Chime::Silent => {
                let about = "The sound of your PCs plays here, from the next stream on.";
                ui.add(egui::Label::new(widgets::dim(about).size(theme::SMALL)).wrap());
            }
        }
    });
}

/// The sign that a device captures: a lit dot and what it says.
fn live(ui: &mut Ui, text: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (dot, _) = ui.allocate_exact_size(vec2(8.0, 18.0), Sense::hover());
        ui.painter().circle_filled(dot.center(), 4.0, theme::sun());
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

#[cfg(test)]
mod tests {
    use egui::Pos2;

    use super::*;

    /// The three cards fit a window as narrow as Leto's smallest and a wide
    /// one, with every list there is: full, still being looked for, with a
    /// device chosen before that is unplugged now, and none at all as in a
    /// browser. Drawing them asks for nothing and chooses nothing.
    #[test]
    fn the_page_draws_narrow_and_wide_and_changes_nothing_by_itself() {
        let names = DeviceNames {
            cameras: Some(vec!["Integrated Camera".to_owned(), "USB Webcam".to_owned()]),
            microphones: vec!["Microphone Array (Realtek Audio)".to_owned()],
            speakers: vec!["Speakers (Realtek Audio)".to_owned(), "Headphones".to_owned()],
        };
        let looking = DeviceNames { cameras: None, ..names.clone() };
        let chosen = DeviceChoices {
            camera: Some("USB Webcam".to_owned()),
            microphone: Some("A microphone that was unplugged".to_owned()),
            speakers: None,
        };
        let chimes = [Chime::Silent, Chime::Playing, Chime::Failed("No sound output.".to_owned())];
        for width in [480.0, 1400.0] {
            for (names, chime) in [Some(&names), Some(&looking), None].into_iter().zip(&chimes) {
                let ctx = egui::Context::default();
                let input = egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 900.0))),
                    ..Default::default()
                };
                let mut state = State::new();
                let devices = Devices {
                    camera: Trial::Failed("No camera.".to_owned()),
                    ..Devices::default()
                };
                let mut kept = chosen.clone();
                let mut asked = None;
                let mut output = ctx.run_ui(input, |ui| {
                    let choices = Choices { chosen: &mut kept, names, chime };
                    let preview = Preview::default();
                    asked = Some(show(ui, &mut state, &devices, &preview, Some(0.4), choices, 0.0));
                });
                // Nothing draws the frame: its textures go nowhere.
                output.textures_delta.clear();
                assert_eq!(asked, Some(Asked::default()));
                assert_eq!(kept, chosen);
            }
        }
    }
}
