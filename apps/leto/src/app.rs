//! The app's single window: a top bar and one screen at a time.

use egui::{Align, Layout, RichText, Sense, Vec2};
use leto_core::{brand, platform::Platform};

use crate::{
    backend::{Backend, Unavailable},
    state::{Event, Screen, State},
    theme,
    ui::{self, apps::Covers, icons, stream::Player, widgets},
};

/// Displayed size of the logo in the top bar, in points. The image is larger,
/// so it stays sharp on high-density screens.
const LOGO: Vec2 = Vec2::new(98.0, 28.0);

/// State of the Leto window.
pub struct LetoApp {
    backend: Backend,
    state: State,
    covers: Covers,
    player: Player,
}

impl LetoApp {
    /// Called once before the first frame.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx);
        Self {
            backend: Backend::new(&cc.egui_ctx),
            state: State::new(),
            covers: Covers::default(),
            player: Player::default(),
        }
    }
}

impl eframe::App for LetoApp {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // While a stream is connected the keyboard is the PC's: the
        // interface must not act on Tab, Enter or Escape itself.
        if matches!(&self.state.screen, Screen::Stream(view) if view.connected) {
            self.player.take_keyboard(raw_input);
        } else {
            #[cfg(not(target_arch = "wasm32"))]
            crate::clipboard::shortcuts(raw_input);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let now = ui.input(|input| input.time);
        let unavailable = self.backend.unavailable();

        for event in self.backend.events() {
            match event {
                Event::Cover { host, app, image } => {
                    self.covers.insert(ui.ctx(), host, app, image);
                }
                other => self.state.apply(other, now),
            }
        }
        if unavailable.is_none() {
            self.state.tick(now);
        }

        match (&unavailable, self.state.screen.clone()) {
            // A stream takes the whole window: no bar, no margins.
            (None, Screen::Stream(view)) => {
                if let Some(picture) = self.backend.take_picture() {
                    self.player.picture(ui.ctx(), picture);
                }
                let sound_problem = self.backend.sound_problem();
                let inputs = egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(egui::Color32::BLACK))
                    .show(ui, |ui| {
                        let problem = sound_problem.as_deref();
                        ui::stream::show(ui, &mut self.state, &view, &mut self.player, problem)
                    })
                    .inner;
                // Before anything else is asked of the backend: what was
                // held down is let go of before the stream is left.
                self.backend.input(inputs);
            }
            (unavailable, screen) => {
                self.player.clear(ui.ctx());
                let ground = egui::Frame::new().fill(theme::GROUND);
                egui::CentralPanel::default().frame(ground).show(ui, |ui| {
                    top_bar(ui);
                    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                        widgets::page(ui, |ui| match (unavailable, screen) {
                            (Some(problem), _) => out_of_order(ui, problem),
                            (None, Screen::Pcs) => {
                                ui::pcs::show(ui, &mut self.state, now);
                                ui.add_space(24.0);
                                ui.label(widgets::dim(build_line()).size(theme::SMALL - 1.0));
                            }
                            (None, Screen::Pairing(pairing)) => {
                                ui::pairing::show(ui, &mut self.state, &pairing, now);
                            }
                            (None, Screen::Apps(view)) => {
                                ui::apps::show(ui, &mut self.state, &view, &self.covers, now);
                            }
                            // Handled above.
                            (None, Screen::Stream(_)) => {}
                        });
                    });
                });
            }
        }

        // Covers are only kept for the PC whose apps are on screen, or were a
        // moment ago: leaving a stream returns to them.
        let shown = match &self.state.screen {
            Screen::Apps(view) => Some(&view.host.id),
            Screen::Stream(view) => Some(&view.host.id),
            _ => None,
        };
        self.covers.keep_only(shown);

        for request in self.state.take_requests() {
            self.backend.send(request);
        }
        if unavailable.is_none() && self.state.screen == Screen::Pcs {
            // Wake up for the next search even if nobody touches the app.
            ui.ctx().request_repaint_after_secs(1.0);
        }
    }
}

fn top_bar(ui: &mut egui::Ui) {
    let size = egui::vec2(ui.available_width(), 60.0);
    ui.allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), |ui| {
        ui.add_space(if size.x < 560.0 { 16.0 } else { 32.0 });
        ui.add(
            egui::Image::new(egui::include_image!("../../../assets/brand/png/leto-logo.png"))
                .fit_to_exact_size(LOGO),
        );
    });
}

/// Shown instead of every screen when the backend cannot work at all.
fn out_of_order(ui: &mut egui::Ui, problem: &Unavailable) {
    ui.add_space((ui.available_height() * 0.16).clamp(16.0, 120.0));
    ui.vertical_centered(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(56.0), Sense::hover());
        icons::monitor(ui.painter(), rect, theme::LINE);
        ui.add_space(6.0);
        ui.label(RichText::new(problem.title).size(20.0));
        ui.scope(|ui| {
            ui.set_max_width(460.0_f32.min(ui.available_width()));
            let text = widgets::dim(problem.explanation.as_str());
            ui.add(egui::Label::new(text).wrap().halign(Align::Center));
        });
        ui.add_space(18.0);
        ui.label(widgets::dim(build_line()).size(theme::SMALL - 1.0));
    });
}

/// The small line at the bottom: version, platform and role.
fn build_line() -> String {
    let platform = Platform::current();
    let name = platform.map_or("unsupported platform", Platform::name);
    let role = if platform.is_some_and(Platform::can_host) { "host and client" } else { "client" };
    format!("{} {} · {name} · {role}", brand::NAME, leto_core::VERSION)
}
