//! The pairing screen: the PIN to type on the PC.

use egui::{Align2, FontId, Key, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2, vec2};
use leto_core::model::{HostKind, HostSummary, Pin, join_address};
use leto_protocol::ports::{PortMap, Service};

use super::widgets;
use crate::{
    state::{Pairing, PairingStep, State},
    theme,
};

/// How long the PC is given to receive the PIN, in seconds. The engine stops
/// waiting after that.
const PATIENCE: f64 = 90.0;

pub fn show(ui: &mut Ui, state: &mut State, pairing: &Pairing, now: f64) {
    let host = &pairing.host;
    let mut leave = widgets::back_button(ui, "PCs").clicked()
        || ui.input(|input| input.key_pressed(Key::Escape));

    ui.add_space((ui.available_height() * 0.08).clamp(8.0, 64.0));
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        widgets::title(ui, &format!("Pair with {}", host.name));

        match &pairing.step {
            PairingStep::Preparing => {
                ui.add_space(24.0);
                ui.add(egui::Spinner::new().size(22.0).color(theme::SUN));
            }
            PairingStep::ShowingPin { pin, since } => {
                let lead = format!("Type this code on {} to let this device connect.", host.name);
                centred(ui, 460.0, widgets::dim(lead));
                ui.add_space(22.0);
                digits(ui, *pin);
                ui.add_space(14.0);
                patience(ui, ((now - since) / PATIENCE) as f32);
                ui.add_space(18.0);
                centred(ui, 460.0, RichText::new(where_to_type(host)));
                if let Some(url) = pin_page(host) {
                    let link = ui.link("Open the PIN page of the PC in the browser");
                    if link.on_hover_text(&url).clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                    }
                    centred(
                        ui,
                        460.0,
                        widgets::dim(
                            "The browser warns about the PC's own certificate first. That is \
                             expected.",
                        )
                        .size(theme::SMALL),
                    );
                }
                ui.add_space(20.0);
                leave |= widgets::quiet_button(ui, "Cancel").clicked();
                // The bar moves: keep drawing.
                ui.ctx().request_repaint_after_secs(0.25);
            }
            PairingStep::Failed(message) => {
                ui.add_space(8.0);
                centred(ui, 460.0, RichText::new(message).color(theme::PROBLEM));
                ui.add_space(18.0);
                if widgets::primary_button(ui, "Try again").clicked() {
                    state.start_pairing(host.clone());
                }
            }
        }
    });

    if leave {
        state.back(now);
    }
}

/// A wrapped, centred paragraph no wider than `width`.
fn centred(ui: &mut Ui, width: f32, text: RichText) {
    ui.scope(|ui| {
        ui.set_max_width(width.min(ui.available_width()));
        ui.add(egui::Label::new(text).wrap().halign(egui::Align::Center));
    });
}

/// The four digits, each in its own tile.
fn digits(ui: &mut Ui, pin: Pin) {
    const TILE: Vec2 = vec2(62.0, 78.0);
    const GAP: f32 = 12.0;
    let size = vec2(TILE.x * 4.0 + GAP * 3.0, TILE.y);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, format!("Code: {pin}")));
    let painter = ui.painter();
    for (index, digit) in pin.digits().into_iter().enumerate() {
        let min = rect.min + vec2((TILE.x + GAP) * index as f32, 0.0);
        let tile = Rect::from_min_size(min, TILE);
        let border = Stroke::new(1.5, theme::SUN.gamma_multiply(0.8));
        painter.rect(tile, theme::CARD_RADIUS, theme::SURFACE, border, StrokeKind::Inside);
        painter.text(
            tile.center(),
            Align2::CENTER_CENTER,
            digit,
            FontId::proportional(42.0),
            theme::MOON,
        );
    }
}

/// A thin bar that empties while the PC is given time to receive the PIN.
fn patience(ui: &mut Ui, spent: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(284.0, 3.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2, theme::LINE);
    let mut left = rect;
    left.set_width(rect.width() * (1.0 - spent.clamp(0.0, 1.0)));
    painter.rect_filled(left, 2, theme::SUN);
    ui.add_space(4.0);
    ui.label(widgets::dim("Waiting for the code…").size(theme::SMALL));
}

/// Where the person finds the place to type the PIN.
fn where_to_type(host: &HostSummary) -> String {
    let name = &host.name;
    match host.kind {
        HostKind::Apollo => format!("On {name}, open Apollo and choose PIN."),
        HostKind::Sunshine | HostKind::FoundationSunshine => {
            format!("On {name}, open Sunshine and choose PIN.")
        }
        HostKind::GameStream => {
            format!("On {name}, a GeForce Experience window asks for the code.")
        }
    }
}

/// The address of the PIN page in the host's own configuration pages.
fn pin_page(host: &HostSummary) -> Option<String> {
    if host.kind == HostKind::GameStream {
        return None;
    }
    let port = PortMap::new(host.port).ok()?.port(Service::WebUi);
    Some(format!("https://{}/pin", join_address(&host.address, port)))
}

#[cfg(test)]
mod tests {
    use leto_core::model::{HostId, Reachability};

    use super::*;

    fn host(kind: HostKind, address: &str, port: u16) -> HostSummary {
        HostSummary {
            id: HostId("id".into()),
            name: "Salon".into(),
            address: address.into(),
            port,
            kind,
            paired: false,
            reachability: Reachability::Online,
            busy: false,
            running_app: None,
            wakeable: false,
            tunnel: false,
        }
    }

    #[test]
    fn the_pin_page_is_on_the_port_after_the_host_port() {
        let salon = host(HostKind::Apollo, "192.168.1.20", 47989);
        assert_eq!(pin_page(&salon).as_deref(), Some("https://192.168.1.20:47990/pin"));

        let moved = host(HostKind::Sunshine, "fe80::1", 48989);
        assert_eq!(pin_page(&moved).as_deref(), Some("https://[fe80::1]:48990/pin"));
    }

    #[test]
    fn gamestream_has_no_pin_page() {
        assert_eq!(pin_page(&host(HostKind::GameStream, "192.168.1.20", 47989)), None);
        assert!(where_to_type(&host(HostKind::GameStream, "pc", 47989)).contains("GeForce"));
    }
}
