//! The app's single window: a top bar and one screen at a time.

use egui::{Align, Layout, RichText, Sense, Vec2};
use leto_core::{brand, platform::Platform};

use crate::{
    backend::{Backend, Door, Pace, Unavailable},
    settings::{self, Settings},
    state::{Cam, Device, Devices, Event, Mic, Screen, State, Trial},
    theme,
    ui::{
        self,
        apps::Covers,
        devices::Preview,
        gate, icons,
        stream::{Player, Status},
        widgets,
    },
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
    /// What the camera shows while the person tries it out.
    preview: Preview,
    /// The code a browser types to be let in by its bridge.
    gate: gate::Form,
    /// The backend took requests in the last frame.
    was_open: bool,
    /// What the person chose, and what of it is kept already.
    settings: Settings,
    saved: Settings,
    /// The settings are on screen, over whatever was there: it carries on
    /// underneath, and is back when they close.
    settings_open: bool,
}

impl LetoApp {
    /// Called once before the first frame, with the settings kept from
    /// before.
    pub fn new(cc: &eframe::CreationContext<'_>, settings: Settings) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx);
        Self {
            backend: Backend::new(&cc.egui_ctx),
            state: State::new(),
            covers: Covers::default(),
            player: Player::default(),
            preview: Preview::default(),
            gate: gate::Form::default(),
            was_open: false,
            settings,
            saved: settings,
            settings_open: false,
        }
    }
}

impl LetoApp {
    /// Runs the camera and the microphone of this device as `tried` has
    /// them, and no further: what is not on captures nothing. Returns how
    /// loud the microphone is while it captures.
    fn try_devices(&mut self, ctx: &egui::Context, tried: &Devices) -> Option<f32> {
        let watching = tried.camera == Trial::On;
        match self.backend.camera(watching) {
            Ok(Some(seen)) => self.preview.show(ctx, seen),
            Ok(None) => {}
            Err(why) => self.state.device_failed(Device::Camera, why),
        }
        if !watching {
            self.preview.clear();
        }
        let listening = tried.microphone == Trial::On;
        let heard = self.backend.hear(listening).unwrap_or_else(|why| {
            self.state.device_failed(Device::Microphone, why);
            None
        });
        if watching || listening {
            // What they capture changes all the time, touched or not.
            ctx.request_repaint_after_secs(1.0 / 30.0);
        }
        heard
    }
}

impl eframe::App for LetoApp {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // While a stream is connected the keyboard is the PC's: the
        // interface must not act on Tab, Enter or Escape itself.
        if matches!(&self.state.screen, Screen::Stream(view) if view.connected) {
            self.player.take_keyboard(raw_input);
            // A browser reports how the mouse itself moved to the page, not
            // to egui.
            #[cfg(target_arch = "wasm32")]
            raw_input
                .events
                .extend(crate::window::mouse_moves().into_iter().map(egui::Event::MouseMoved));
        } else {
            #[cfg(not(target_arch = "wasm32"))]
            crate::clipboard::shortcuts(raw_input);
        }
    }

    /// What shows where the app paints nothing. In a browser that is the
    /// page: the pictures of a stream are there, on a canvas behind this
    /// one, and every screen paints its own ground.
    #[cfg(target_arch = "wasm32")]
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let now = ui.input(|input| input.time);
        let door = self.backend.door();
        let open = door == Door::Open;
        if open != self.was_open {
            // What was on screen belongs to a backend that is gone, or was
            // never there: start from the list of PCs.
            self.was_open = open;
            self.state = State::new();
            self.gate = gate::Form::default();
        }

        for event in self.backend.events() {
            match event {
                Event::Cover { host, app, image } => {
                    self.covers.insert(ui.ctx(), host, app, image);
                }
                other => self.state.apply(other, now),
            }
        }
        if open {
            self.state.tick(now);
        }

        // The camera and the microphone are tried out on one screen, and
        // are off everywhere else, the settings over it included.
        let tried = match (&door, &self.state.screen) {
            (Door::Open, Screen::Devices(devices)) if !self.settings_open => devices.clone(),
            _ => Devices::default(),
        };
        let heard = self.try_devices(ui.ctx(), &tried);

        match (&door, self.state.screen.clone()) {
            // A stream takes the whole window: no bar, no margins.
            (Door::Open, Screen::Stream(view)) => {
                // Only the pictures the person sees are worth the work, and
                // the battery: one a second while another window has the
                // person's attention, none while this one is out of sight.
                let pace = ui.input(|input| {
                    let window = input.viewport();
                    if window.minimized == Some(true) || window.occluded == Some(true) {
                        Pace::Hidden
                    } else if input.focused {
                        Pace::Full
                    } else {
                        Pace::Glance
                    }
                });
                self.backend.pace(pace);
                self.backend.mute(self.settings.mute_in_background && pace != Pace::Full);
                if let Some(picture) = self.backend.take_picture() {
                    self.player.picture(ui.ctx(), picture);
                }
                let sound_problem = self.backend.sound_problem();
                // The microphone captures while its switch is on, and not a
                // moment longer.
                let listening = view.connected && view.microphone == Mic::On;
                let voice = self.backend.microphone(listening).unwrap_or_else(|problem| {
                    self.state.microphone_failed(problem);
                    None
                });
                // The same goes for the camera.
                let filming = view.connected && view.camera == Cam::On;
                let film = self.backend.film(filming).unwrap_or_else(|problem| {
                    self.state.camera_failed(problem);
                    None
                });
                // The stream screen paints its own ground: on the web the
                // pictures show through the window.
                let inputs = egui::CentralPanel::default()
                    .frame(egui::Frame::new())
                    .show(ui, |ui| {
                        let status = Status {
                            sound_problem: sound_problem.as_deref(),
                            voice,
                            film,
                            counts: self.backend.counts(),
                            round_trip: self.backend.round_trip(),
                        };
                        let settings = &self.settings;
                        ui::stream::show(
                            ui,
                            &mut self.state,
                            &view,
                            &mut self.player,
                            settings,
                            status,
                        )
                    })
                    .inner;
                // Before anything else is asked of the backend: what was
                // held down is let go of before the stream is left.
                self.backend.input(inputs);
                self.backend.place_picture(self.player.hole());
            }
            (door, screen) => {
                self.player.clear(ui.ctx());
                self.backend.place_picture(None);
                let ground = egui::Frame::new().fill(theme::GROUND);
                egui::CentralPanel::default().frame(ground).show(ui, |ui| {
                    if top_bar(ui, self.settings_open) {
                        self.settings_open = !self.settings_open;
                    }
                    // Escape closes the settings, as their back button does.
                    let escape = ui.input(|input| input.key_pressed(egui::Key::Escape));
                    if self.settings_open && escape {
                        self.settings_open = false;
                    }
                    let scroll = egui::ScrollArea::vertical().auto_shrink(false);
                    // The settings and the screens scroll on their own.
                    let scroll =
                        scroll.id_salt(if self.settings_open { "settings" } else { "screen" });
                    scroll.show(ui, |ui| {
                        widgets::page(ui, |ui| match (door, screen) {
                            _ if self.settings_open => {
                                let native = crate::window::screen(ui.ctx());
                                let back_to = back_to(&self.state.screen);
                                if ui::settings::show(ui, &mut self.settings, native, back_to) {
                                    self.settings_open = false;
                                }
                            }
                            (Door::Closed(problem), _) => out_of_order(ui, problem),
                            (Door::Opening { failed }, _) => gate::reaching(ui, *failed),
                            (Door::Code { refused, checking }, _) => {
                                let code = gate::code(ui, &mut self.gate, *refused, *checking);
                                if let Some(code) = code {
                                    self.backend.knock(&code);
                                }
                            }
                            (Door::Open, Screen::Pcs) => {
                                ui::pcs::show(ui, &mut self.state, now);
                                ui.add_space(24.0);
                                ui.label(widgets::dim(build_line()).size(theme::SMALL - 1.0));
                            }
                            (Door::Open, Screen::Devices(devices)) => {
                                let preview = &self.preview;
                                ui::devices::show(
                                    ui,
                                    &mut self.state,
                                    &devices,
                                    preview,
                                    heard,
                                    now,
                                );
                            }
                            (Door::Open, Screen::Pairing(pairing)) => {
                                ui::pairing::show(ui, &mut self.state, &pairing, now);
                            }
                            (Door::Open, Screen::Apps(view)) => {
                                let (covers, settings) = (&self.covers, &self.settings);
                                ui::apps::show(ui, &mut self.state, &view, covers, settings, now);
                            }
                            // Handled above.
                            (Door::Open, Screen::Stream(_)) => {}
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
        // Gamepads are read away from the window, and go straight to the PC.
        self.backend.swap_gamepad_buttons(self.settings.swap_gamepad_buttons);
        if self.settings != self.saved {
            // A change is kept at once: a crash loses none.
            self.saved = self.settings;
            settings::save(self.settings);
        }
        if open && self.state.screen == Screen::Pcs {
            // Wake up for the next search even if nobody touches the app.
            ui.ctx().request_repaint_after_secs(1.0);
        }
    }
}

/// The bar at the top: the logo, and the settings on the right. Returns
/// whether the person asked to open the settings, or to close them.
fn top_bar(ui: &mut egui::Ui, settings_open: bool) -> bool {
    let size = egui::vec2(ui.available_width(), 60.0);
    let margin = if size.x < 560.0 { 16.0 } else { 32.0 };
    ui.allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), |ui| {
        ui.add_space(margin);
        ui.add(
            egui::Image::new(egui::include_image!("../../../assets/brand/png/leto-logo.png"))
                .fit_to_exact_size(LOGO),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(margin - 8.0);
            let name = if settings_open { "Close the settings" } else { "Settings" };
            widgets::icon_button(ui, icons::gear, name).clicked()
        })
        .inner
    })
    .inner
}

/// The name of the screen under the settings, for their back button.
fn back_to(screen: &Screen) -> &str {
    match screen {
        Screen::Pcs => "PCs",
        Screen::Devices(_) => "Camera and microphone",
        Screen::Pairing(_) => "Pairing",
        Screen::Apps(view) => &view.host.name,
        Screen::Stream(_) => "Stream",
    }
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
