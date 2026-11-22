//! Leto's own WireGuard tunnel, in the settings: the file it comes from,
//! how it is doing, and taking it away again.

use egui::{Align, Color32, Layout, RichText, Sense, TextStyle, Ui, vec2};
use leto_core::model::TunnelSummary;

use super::widgets;
use crate::{state::TunnelForm, theme};

/// What the person asked for about the tunnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asked {
    /// Use the file in the form.
    Use,
    Remove,
}

/// A handshake older than this is no proof that the other end still
/// answers: WireGuard makes a new one every two minutes while it is used.
const FRESH: u64 = 180;
/// The largest file read when one is dropped on the window. WireGuard
/// files are a few hundred bytes.
#[cfg(not(target_arch = "wasm32"))]
const LARGEST_FILE: u64 = 64 * 1024;

const ABOUT: &str = "Reach a PC at home from anywhere, through your own WireGuard server. Leto \
                     opens the tunnel itself, for its own connections only: no WireGuard app \
                     and no administrator rights.";
const HOW: &str = "Paste the text of a WireGuard file, or drop the file on this window. Give \
                   Leto a peer of its own: keys that the WireGuard app uses at the same time \
                   would cut each other off.";
const HINT: &str = "[Interface]\nPrivateKey = …\nAddress = 10.8.0.2/24\n\n[Peer]\nPublicKey = \
                    …\nEndpoint = home.example.org:51820\nAllowedIPs = 10.8.0.0/24";

/// Draws what goes in the tunnel's card, under its name.
pub fn show(ui: &mut Ui, form: &mut TunnelForm) -> Option<Asked> {
    let mut asked = None;
    ui.add(egui::Label::new(widgets::dim(ABOUT).size(theme::SMALL)).wrap());
    ui.add_space(8.0);

    if !form.known {
        widgets::waiting(ui, "Looking at the tunnel…");
        return None;
    }
    if let Some(summary) = &form.summary {
        state(ui, summary);
        ui.add_space(8.0);
    }
    if form.open {
        asked = file_form(ui, form);
    } else {
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!form.busy, |ui| {
                let name = if form.summary.is_some() {
                    "Replace the file"
                } else {
                    "Add a WireGuard file"
                };
                if widgets::quiet_button(ui, name).clicked() {
                    form.open = true;
                    form.refused = None;
                }
                if form.summary.is_some() && widgets::quiet_button(ui, "Remove").clicked() {
                    asked = Some(Asked::Remove);
                }
            });
        });
        if let Some(why) = &form.refused {
            ui.add(egui::Label::new(RichText::new(why).color(theme::problem())).wrap());
        }
    }
    asked
}

/// How the tunnel is doing, and what it is.
fn state(ui: &mut Ui, summary: &TunnelSummary) {
    let (color, text) = status(summary);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (dot, _) = ui.allocate_exact_size(vec2(8.0, 20.0), Sense::hover());
        ui.painter().circle_filled(dot.center(), 4.0, color);
        ui.label(text);
    });
    if summary.address.is_empty() {
        // A file that could not be read says nothing more.
        return;
    }
    let about = format!(
        "This device is {} in the tunnel, which leads to {}.",
        summary.address,
        summary.covers.join(", ")
    );
    ui.add(egui::Label::new(widgets::dim(about).size(theme::SMALL)).wrap());
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(widgets::dim("Its public key, for the server:").size(theme::SMALL));
        let key = RichText::new(&summary.public_key).monospace().size(theme::SMALL);
        ui.add(egui::Label::new(key).truncate());
        if ui.small_button("Copy").clicked() {
            copy(ui.ctx(), summary.public_key.clone());
        }
    });
}

/// The light of the tunnel and the words beside it.
fn status(summary: &TunnelSummary) -> (Color32, String) {
    let end = summary.endpoint.as_deref().unwrap_or("the other end");
    match (&summary.problem, summary.running, summary.handshake) {
        (Some(problem), ..) => (theme::problem(), format!("The tunnel does not work: {problem}")),
        (None, true, Some(ago)) if ago <= FRESH => (theme::ready(), format!("Connected to {end}.")),
        (None, true, Some(ago)) => {
            (theme::dim(), format!("Open. {end} last answered {} minutes ago.", ago / 60))
        }
        (None, true, None) => match &summary.silence {
            Some(why) => (theme::problem(), format!("The tunnel does not open: {why}")),
            None => (theme::sun(), format!("Waiting for {end} to answer…")),
        },
        (None, false, _) => {
            (theme::dim(), format!("To {end}. It opens when a PC it leads to is looked for."))
        }
    }
}

/// A button that pastes the file: a phone has no menu to paste into the
/// field with, and no keyboard shortcut either.
#[cfg(not(target_arch = "wasm32"))]
fn paste_button(ui: &mut Ui, form: &mut TunnelForm) {
    let paste = ui.add_enabled_ui(!form.busy, |ui| widgets::quiet_button(ui, "Paste"));
    if paste.inner.clicked() {
        match crate::clipboard::pasted() {
            Some(text) => {
                form.draft.0 = text;
                form.refused = None;
            }
            None => form.refused = Some("There is no text to paste.".to_owned()),
        }
    }
}

/// A browser page cannot read the clipboard when it likes: Ctrl+V it is.
#[cfg(target_arch = "wasm32")]
fn paste_button(_ui: &mut Ui, _form: &mut TunnelForm) {}

/// Puts the key on the clipboard. A browser page goes through egui.
#[cfg(not(target_arch = "wasm32"))]
fn copy(ctx: &egui::Context, text: String) {
    crate::clipboard::copy(ctx, text);
}

#[cfg(target_arch = "wasm32")]
fn copy(ctx: &egui::Context, text: String) {
    ctx.copy_text(text);
}

/// Where the file is pasted or dropped.
fn file_form(ui: &mut Ui, form: &mut TunnelForm) -> Option<Asked> {
    let mut asked = None;
    take_dropped(ui, form);
    ui.add(egui::Label::new(widgets::dim(HOW).size(theme::SMALL)).wrap());
    ui.add_space(4.0);
    ui.add_enabled(
        !form.busy,
        egui::TextEdit::multiline(&mut form.draft.0)
            .font(TextStyle::Monospace)
            .desired_rows(8)
            .desired_width(f32::INFINITY)
            .hint_text(HINT),
    );
    if let Some(why) = &form.refused {
        ui.add(egui::Label::new(RichText::new(why).color(theme::problem())).wrap());
    }
    ui.add_space(4.0);
    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
        let ready = !form.busy && !form.draft.0.trim().is_empty();
        let used = ui.add_enabled_ui(ready, |ui| widgets::primary_button(ui, "Use this tunnel"));
        if used.inner.clicked() {
            asked = Some(Asked::Use);
        }
        paste_button(ui, form);
        let cancelled = ui.add_enabled_ui(!form.busy, |ui| widgets::quiet_button(ui, "Cancel"));
        if cancelled.inner.clicked() {
            // The key leaves the window with the form.
            form.open = false;
            form.refused = None;
            form.draft.0.clear();
        }
        if form.busy {
            widgets::waiting(ui, "Opening the tunnel…");
        }
    });
    asked
}

/// A file dropped on the window fills the form.
#[cfg(not(target_arch = "wasm32"))]
fn take_dropped(ui: &Ui, form: &mut TunnelForm) {
    let Some(path) =
        ui.input(|input| input.raw.dropped_files.first().map(|file| file.path().to_path_buf()))
    else {
        return;
    };
    let small = std::fs::metadata(&path).is_ok_and(|file| file.len() <= LARGEST_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) if small => {
            form.draft.0 = text;
            form.refused = None;
        }
        _ => form.refused = Some("Leto could not read this file as a WireGuard file.".to_owned()),
    }
}

#[cfg(target_arch = "wasm32")]
fn take_dropped(_ui: &Ui, _form: &mut TunnelForm) {}

#[cfg(test)]
mod tests {
    use egui::{Pos2, Rect};
    use leto_core::engine::WireGuardFile;

    use super::*;

    fn summary() -> TunnelSummary {
        TunnelSummary {
            address: "10.8.0.2".to_owned(),
            endpoint: Some("home.example.org:51820".to_owned()),
            covers: vec!["10.8.0.0/24".to_owned()],
            public_key: "uJ8T0yLmZqv3n8k0mO0m0b9eH0x2Yw6b5c4d3e2f1g0=".to_owned(),
            running: true,
            handshake: Some(12),
            problem: None,
            silence: None,
        }
    }

    #[test]
    fn the_light_says_whether_the_other_end_answers() {
        let connected = summary();
        assert_eq!(
            status(&connected),
            (theme::ready(), "Connected to home.example.org:51820.".into())
        );
        let waiting = TunnelSummary { handshake: None, ..summary() };
        assert_eq!(status(&waiting).0, theme::sun());
        let silent = TunnelSummary {
            silence: Some("no answer from home.example.org:51820 after 4 handshakes".into()),
            ..waiting
        };
        assert_eq!(status(&silent).0, theme::problem());
        assert!(status(&silent).1.contains("after 4 handshakes"));
        let quiet = TunnelSummary { handshake: Some(600), ..summary() };
        assert!(status(&quiet).1.contains("10 minutes ago"));
        let closed = TunnelSummary { running: false, handshake: None, ..summary() };
        assert!(status(&closed).1.contains("opens when a PC"));
        let broken = TunnelSummary { problem: Some("no route".into()), ..summary() };
        assert_eq!(status(&broken).0, theme::problem());
    }

    /// Every state of the card draws in a narrow window and a wide one, and
    /// asks for nothing by itself.
    #[test]
    fn the_card_draws_in_every_state_and_asks_for_nothing_by_itself() {
        let unreadable = TunnelSummary {
            address: String::new(),
            endpoint: None,
            covers: Vec::new(),
            public_key: String::new(),
            running: false,
            handshake: None,
            problem: Some("the file has no PrivateKey".to_owned()),
            silence: None,
        };
        let forms = [
            TunnelForm::default(),
            TunnelForm { known: true, ..TunnelForm::default() },
            TunnelForm { known: true, summary: Some(summary()), ..TunnelForm::default() },
            TunnelForm { known: true, summary: Some(unreadable), ..TunnelForm::default() },
            TunnelForm {
                known: true,
                open: true,
                busy: true,
                draft: WireGuardFile("[Interface]".to_owned()),
                refused: Some("Leto cannot use this WireGuard file.".to_owned()),
                ..TunnelForm::default()
            },
        ];
        for width in [480.0, 1400.0] {
            for form in &forms {
                let ctx = egui::Context::default();
                let input = egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 900.0))),
                    ..Default::default()
                };
                let mut drawn = form.clone();
                let mut asked = None;
                let mut output = ctx.run_ui(input, |ui| asked = show(ui, &mut drawn));
                // Nothing draws the frame: its textures go nowhere.
                output.textures_delta.clear();
                assert_eq!(asked, None);
                assert_eq!(&drawn, form);
            }
        }
    }
}
