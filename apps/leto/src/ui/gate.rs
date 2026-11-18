//! What a browser sees before it may use its bridge: the wait while the
//! bridge is reached, and the code the bridge asks a new browser for.

use egui::{Align, Align2, FontId, Key, RichText, Sense, Ui, Vec2};
use leto_core::bridge::CODE_DIGITS;

use super::{icons, widgets};
use crate::theme;

/// The size of the code's digits, in points: large, to compare at a glance
/// with the ones the bridge shows.
const CODE_SIZE: f32 = 30.0;

/// The code being typed.
#[derive(Default)]
pub struct Form {
    code: String,
    /// The field has had the keyboard once: it is not taken again from
    /// whatever the person moved on to.
    greeted: bool,
}

/// The page is reaching its bridge. `failed` once the bridge has not
/// answered.
pub fn reaching(ui: &mut Ui, failed: bool) {
    ui.add_space((ui.available_height() * 0.16).clamp(16.0, 120.0));
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        if !failed {
            ui.add_space(24.0);
            ui.add(egui::Spinner::new().size(22.0).color(theme::SUN));
            ui.label(widgets::dim("Reaching the bridge…"));
            return;
        }
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(56.0), Sense::hover());
        icons::monitor(ui.painter(), rect, theme::LINE);
        ui.label(RichText::new("The bridge does not answer").size(20.0));
        centred(
            ui,
            widgets::dim(
                "Leto reaches your PCs through the bridge that sent this page. Check that it \
                 is still running, and that this device is on its network.",
            ),
        );
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            // Centred as a pair: the spinner and what it is about.
            let text = "Trying again…";
            let font = FontId::proportional(theme::BODY);
            let width = ui.painter().layout_no_wrap(text.to_owned(), font, theme::DIM).size().x;
            ui.add_space(((ui.available_width() - width - 24.0) / 2.0).max(0.0));
            widgets::waiting(ui, text);
        });
    });
}

/// Asks for the code of the bridge. Returns it when the person gives one.
pub fn code(ui: &mut Ui, form: &mut Form, refused: bool, checking: bool) -> Option<String> {
    let mut given = None;
    ui.add_space((ui.available_height() * 0.08).clamp(8.0, 64.0));
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        widgets::title(ui, "This browser is new here");
        centred(
            ui,
            widgets::dim(
                "Leto reaches your PCs through the bridge that sent this page. To let this \
                 browser in, type the code the bridge shows where it runs.",
            ),
        );
        ui.add_space(18.0);

        // Digits, and the space people put in the middle. Nothing else.
        form.code.retain(|typed| typed.is_ascii_digit() || typed == ' ');
        let complete = digits(&form.code).len() == CODE_DIGITS;
        let field = ui.add_enabled(
            !checking,
            egui::TextEdit::singleline(&mut form.code)
                .hint_text(widgets::dim("1234 5678").size(CODE_SIZE).color(theme::LINE))
                .font(FontId::proportional(CODE_SIZE))
                .align(Align2::CENTER_CENTER)
                .desired_width(230.0_f32.min(ui.available_width() - 24.0))
                .margin(egui::Margin::symmetric(12, 12)),
        );
        if !std::mem::replace(&mut form.greeted, true) {
            field.request_focus();
        }
        let entered = field.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));

        ui.add_space(12.0);
        if checking {
            ui.add(egui::Spinner::new().size(22.0).color(theme::SUN));
        } else {
            let pressed = ui
                .add_enabled_ui(complete, |ui| widgets::primary_button(ui, "Continue").clicked())
                .inner;
            if complete && (pressed || entered) {
                given = Some(digits(&form.code));
            } else if entered {
                // Enter on half a code: keep typing.
                field.request_focus();
            }
        }

        if refused && !checking {
            ui.add_space(6.0);
            let problem = "That is not the code. The bridge draws a new one after a few wrong \
                           tries: look again where it runs.";
            centred(ui, RichText::new(problem).color(theme::PROBLEM));
        }
        ui.add_space(14.0);
        centred(
            ui,
            widgets::dim("The bridge remembers this browser: it asks once.").size(theme::SMALL),
        );
    });
    if given.is_some() {
        // Right or wrong, this code is spent.
        form.code.clear();
        form.greeted = false;
    }
    given
}

/// The digits of what was typed.
fn digits(typed: &str) -> String {
    typed.chars().filter(char::is_ascii_digit).collect()
}

/// A wrapped, centred paragraph.
fn centred(ui: &mut Ui, text: RichText) {
    ui.scope(|ui| {
        ui.set_max_width(460.0_f32.min(ui.available_width()));
        ui.add(egui::Label::new(text).wrap().halign(Align::Center));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_is_its_digits_however_it_was_typed() {
        assert_eq!(digits("1234 5678"), "12345678");
        assert_eq!(digits(" 12 34 56 78 "), "12345678");
        assert_eq!(digits(""), "");
    }
}
