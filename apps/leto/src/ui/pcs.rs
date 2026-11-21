//! The first screen: the PCs Leto knows, and how to add one.

use egui::{
    Align, CursorIcon, Key, Layout, Rect, RichText, Role, Sense, Stroke, StrokeKind, Ui, Vec2,
    WidgetInfo, pos2, vec2,
};
use leto_core::model::{HostId, HostSummary, Reachability, join_address};

use super::{icons, widgets};
use crate::{state::State, theme};

const CARD_HEIGHT: f32 = 150.0;
const CARD_MIN_WIDTH: f32 = 236.0;
const GAP: f32 = 16.0;

/// What the person did to a card.
enum Chosen {
    Open(HostId),
    Forget(HostId),
    Add,
}

/// Returns whether the person asked for the settings.
pub fn show(ui: &mut Ui, state: &mut State, now: f64) -> bool {
    let settings = header(ui, state, now);
    ui.add_space(14.0);

    if let Some(notice) = &state.notice {
        widgets::note(ui, notice);
        ui.add_space(6.0);
    }

    if state.hosts.is_empty() && !state.add.open {
        nothing_found(ui, state, now);
        return settings;
    }

    let chosen = cards(ui, &state.hosts, &state.waking, state.add.open);
    if state.add.open {
        add_form(ui, state);
    }

    match chosen {
        Some(Chosen::Open(id)) => state.open(&id, now),
        Some(Chosen::Forget(id)) => state.forget(&id),
        Some(Chosen::Add) => state.open_add_form(),
        None => {}
    }
    settings
}

/// The title, and on its right what is about this device and not one PC:
/// the settings, the camera and the rest, and searching again. Returns
/// whether the person asked for the settings.
fn header(ui: &mut Ui, state: &mut State, now: f64) -> bool {
    if ui.available_width() < NARROW {
        // A phone held upright: the buttons go on a line of their own.
        let settings = ui
            .allocate_ui_with_layout(
                vec2(ui.available_width(), 36.0),
                Layout::right_to_left(Align::Center),
                |ui| header_buttons(ui, state, now),
            )
            .inner;
        titles(ui, state);
        settings
    } else {
        ui.horizontal(|ui| {
            titles(ui, state);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                header_buttons(ui, state, now)
            })
            .inner
        })
        .inner
    }
}

/// The title of the list of PCs and the line under it.
fn titles(ui: &mut Ui, state: &State) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        widgets::title(ui, "Your PCs");
        if state.searching {
            widgets::waiting(ui, "Looking on your network…");
        } else {
            let line = match state.hosts.len() {
                0 => "PCs running Apollo or Sunshine show up here.".to_owned(),
                1 => "1 PC".to_owned(),
                count => format!("{count} PCs"),
            };
            ui.add(egui::Label::new(widgets::dim(line)).wrap());
        }
    });
}

/// The buttons of the header, from the right. Returns whether the settings
/// were asked for.
fn header_buttons(ui: &mut Ui, state: &mut State, now: f64) -> bool {
    let again = ui.add_enabled_ui(!state.searching, |ui| {
        widgets::icon_button(ui, icons::refresh, "Search again")
    });
    if again.inner.clicked() {
        state.search(now);
    }
    if widgets::icon_button(ui, icons::camera, super::devices::NAME).clicked() {
        state.check_devices();
    }
    widgets::icon_button(ui, icons::gear, "Settings").clicked()
}

/// Below this width, in points, the header puts its buttons above the
/// titles rather than beside them.
const NARROW: f32 = 520.0;

/// The grid of PCs, ending with the card that adds one.
fn cards(ui: &mut Ui, hosts: &[HostSummary], waking: &[HostId], adding: bool) -> Option<Chosen> {
    let available = ui.available_width();
    let columns = (((available + GAP) / (CARD_MIN_WIDTH + GAP)).floor() as usize).clamp(1, 4);
    let width = (available - GAP * (columns as f32 - 1.0)) / columns as f32;
    let size = vec2(width, CARD_HEIGHT);

    let mut chosen = None;
    let mut slots: Vec<Option<&HostSummary>> = hosts.iter().map(Some).collect();
    if !adding {
        slots.push(None);
    }
    for row in slots.chunks(columns) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for slot in row {
                let clicked = match slot {
                    Some(host) => host_card(ui, host, waking.contains(&host.id), size),
                    None => add_card(ui, size).then_some(Chosen::Add),
                };
                chosen = chosen.take().or(clicked);
            }
        });
        ui.add_space(GAP - ui.spacing().item_spacing.y);
    }
    chosen
}

fn host_card(ui: &mut Ui, host: &HostSummary, waking: bool, size: Vec2) -> Option<Chosen> {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let (dot, state_text) = status(host, waking);
    response.widget_info(|| {
        WidgetInfo::labeled(Role::Button, true, format!("{}, {state_text}", host.name))
    });
    let offline = host.reachability == Reachability::Offline;
    let lit = response.hovered() || response.has_focus();

    let painter = ui.painter();
    let fill = if lit { theme::surface_hover() } else { theme::surface() };
    let border = if lit { theme::sun().gamma_multiply(0.7) } else { theme::line() };
    painter.rect(rect, theme::CARD_RADIUS, fill, Stroke::new(1.0, border), StrokeKind::Inside);

    let ink = if offline { theme::dim() } else { theme::text() };
    let tile = Rect::from_min_size(rect.min + vec2(16.0, 16.0), Vec2::splat(42.0));
    painter.rect_filled(tile, theme::CONTROL_RADIUS, theme::ground());
    icons::monitor(painter, tile.shrink(10.0), if offline { theme::dim() } else { theme::sun() });

    let text_width = size.x - 32.0;
    widgets::line(painter, rect.min + vec2(16.0, 70.0), &host.name, 19.0, ink, text_width);

    let dot_center = rect.min + vec2(21.0, 109.0);
    match dot {
        Dot::Full(color) => {
            painter.circle_filled(dot_center, 4.0, color);
        }
        Dot::Hollow => {
            painter.circle_stroke(dot_center, 3.5, Stroke::new(1.5, theme::dim()));
        }
    }
    let state_pos = rect.min + vec2(33.0, 100.0);
    widgets::line(painter, state_pos, state_text, theme::SMALL + 0.5, ink, text_width - 17.0);

    // Through Leto's own tunnel, the address is the PC's at home.
    let way = if host.tunnel { " · WireGuard" } else { "" };
    let detail = format!("{} · {}{way}", host.kind.name(), shown_address(host));
    let detail_pos = rect.min + vec2(16.0, 122.0);
    widgets::line(painter, detail_pos, &detail, theme::SMALL - 0.5, theme::dim(), text_width);

    // On top of the card, so that it takes its own clicks.
    let menu_rect =
        Rect::from_min_size(pos2(rect.right() - 46.0, rect.top() + 10.0), Vec2::splat(36.0));
    let menu_id = response.id.with("more");
    let menu = widgets::icon_button_at(ui, menu_rect, menu_id, icons::more, "More");
    let mut forget = false;
    egui::Popup::menu(&menu).show(|ui| {
        ui.set_min_width(170.0);
        forget = ui.button("Forget this PC").clicked();
    });

    if forget {
        Some(Chosen::Forget(host.id.clone()))
    } else if response.on_hover_cursor(CursorIcon::PointingHand).clicked() {
        Some(Chosen::Open(host.id.clone()))
    } else {
        None
    }
}

fn add_card(ui: &mut Ui, size: Vec2) -> bool {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, "Add a PC"));
    let lit = response.hovered() || response.has_focus();
    let painter = ui.painter();
    let border = if lit { theme::sun().gamma_multiply(0.7) } else { theme::line() };
    let fill = if lit { theme::surface() } else { theme::ground() };
    painter.rect(rect, theme::CARD_RADIUS, fill, Stroke::new(1.0, border), StrokeKind::Inside);

    let center = rect.center();
    let icon = Rect::from_center_size(center - vec2(0.0, 22.0), Vec2::splat(22.0));
    icons::plus(painter, icon, if lit { theme::sun() } else { theme::dim() });
    let font = egui::FontId::proportional(theme::BODY);
    painter.text(
        center + vec2(0.0, 12.0),
        egui::Align2::CENTER_CENTER,
        "Add a PC",
        font,
        theme::text(),
    );
    let font = egui::FontId::proportional(theme::SMALL - 0.5);
    let hint = "by its address";
    painter.text(center + vec2(0.0, 33.0), egui::Align2::CENTER_CENTER, hint, font, theme::dim());

    response.on_hover_cursor(CursorIcon::PointingHand).clicked()
}

/// Shown instead of the grid when there is no PC at all.
fn nothing_found(ui: &mut Ui, state: &mut State, now: f64) {
    ui.add_space((ui.available_height() * 0.16).clamp(16.0, 120.0));
    ui.vertical_centered(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(56.0), Sense::hover());
        icons::monitor(ui.painter(), rect, theme::line());
        ui.add_space(6.0);

        if !state.searched {
            ui.label(RichText::new("Looking for your PC…").size(20.0));
            ui.label(widgets::dim("This takes a couple of seconds."));
            return;
        }

        ui.label(RichText::new("No PC found yet").size(20.0));
        ui.scope(|ui| {
            ui.set_max_width(ui.available_width().min(440.0));
            let advice = if state.network_searched {
                "Leto finds PCs that run Apollo or Sunshine on the same network as this device. \
                 Check that yours is on, or add it by its address."
            } else {
                "Leto could not search the network from this device. You can still add your PC \
                 by its address."
            };
            ui.add(egui::Label::new(widgets::dim(advice)).wrap().halign(Align::Center));
        });
        ui.add_space(10.0);

        // Two buttons side by side, centred as a pair.
        let width = 150.0 * 2.0 + 10.0;
        ui.allocate_ui_with_layout(vec2(width, 40.0), Layout::left_to_right(Align::Center), |ui| {
            let add = ui.add_sized([150.0, 40.0], primary("Add a PC"));
            if add.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                state.open_add_form();
            }
            ui.add_enabled_ui(!state.searching, |ui| {
                let again = ui.add_sized([150.0, 40.0], quiet("Search again"));
                if again.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                    state.search(now);
                }
            });
        });
    });
}

fn primary(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).color(theme::on_sun()))
        .fill(theme::sun())
        .stroke(Stroke::NONE)
        .corner_radius(theme::CONTROL_RADIUS)
}

fn quiet(text: &str) -> egui::Button<'_> {
    egui::Button::new(text).fill(egui::Color32::TRANSPARENT).corner_radius(theme::CONTROL_RADIUS)
}

fn add_form(ui: &mut Ui, state: &mut State) {
    egui::Frame::new()
        .fill(theme::surface())
        .stroke(Stroke::new(1.0, theme::line()))
        .corner_radius(theme::CARD_RADIUS)
        .inner_margin(egui::Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.label(RichText::new("Add a PC by its address").size(18.0));
            ui.add(
                egui::Label::new(widgets::dim(
                    "Use the address of the PC on your network. It looks like 192.168.1.20.",
                ))
                .wrap(),
            );
            ui.add_space(8.0);

            let mut submit = false;
            let mut cancel = ui.input(|input| input.key_pressed(Key::Escape));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
                let field = ui.add_enabled(
                    !state.add.busy,
                    egui::TextEdit::singleline(&mut state.add.address)
                        .hint_text("192.168.1.20")
                        .desired_width(260.0_f32.min(ui.available_width() - 12.0))
                        .margin(egui::Margin::symmetric(12, 10)),
                );
                if std::mem::take(&mut state.add.wants_keyboard) {
                    field.request_focus();
                }
                submit = field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
                if state.add.busy {
                    widgets::waiting(ui, "Asking the PC…");
                } else {
                    submit |= widgets::primary_button(ui, "Add").clicked();
                    cancel |= widgets::quiet_button(ui, "Cancel").clicked();
                }
            });
            if let Some(error) = &state.add.error {
                ui.add_space(4.0);
                ui.add(egui::Label::new(RichText::new(error).color(theme::problem())).wrap());
            }

            if submit {
                state.submit_address();
            } else if cancel && !state.add.busy {
                state.add = crate::state::AddForm::default();
            }
        });
}

enum Dot {
    Full(egui::Color32),
    Hollow,
}

/// The dot and the words that say what can be done with a PC, or what is
/// being done: waking it up.
fn status(host: &HostSummary, waking: bool) -> (Dot, &'static str) {
    if waking && host.reachability != Reachability::Online {
        return (Dot::Hollow, "Waking up…");
    }
    match (host.reachability, host.paired, host.busy) {
        (Reachability::Offline, ..) if host.wakeable => (Dot::Hollow, "Offline · click to wake"),
        (Reachability::Offline, ..) => (Dot::Hollow, "Offline"),
        (Reachability::Unknown, ..) => (Dot::Hollow, "Checking…"),
        (Reachability::Online, false, _) => (Dot::Full(theme::sun()), "Not paired yet"),
        (Reachability::Online, true, true) => (Dot::Full(theme::ready()), "An app is running"),
        (Reachability::Online, true, false) => (Dot::Full(theme::ready()), "Ready"),
    }
}

/// The address as a person would type it: the port only when it is unusual.
fn shown_address(host: &HostSummary) -> String {
    if host.port == leto_core::model::DEFAULT_HOST_PORT {
        host.address.clone()
    } else {
        join_address(&host.address, host.port)
    }
}
