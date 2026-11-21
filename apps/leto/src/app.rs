//! The app's single window: a top bar and one screen at a time.

use egui::{Align, Id, Layout, Rect, RichText, Sense, UiBuilder, Vec2, pos2};
use leto_core::{brand, platform::Platform};
use leto_system::pane::Pane;

use crate::{
    backend::{Backend, Door, Pace, Unavailable},
    settings::{self, Settings},
    state::{Cam, Device, Devices, Event, Mic, Screen, State, Trial},
    theme,
    ui::{
        self,
        apps::Covers,
        card::OnCard,
        devices::Preview,
        frame, gate, icons,
        stream::{Player, Status},
        widgets,
    },
};

/// Seconds between leaving a stream because the window was closed and
/// closing it: what was sent last, the PC's pointer asked back among it,
/// gets the time to reach the PC before the app is gone.
const CLOSES_AFTER_LEAVING: f64 = 0.3;

/// Displayed size of the logo in the top bar, in points. The image is larger,
/// so it stays sharp on high-density screens.
const LOGO: Vec2 = Vec2::new(98.0, 28.0);

/// State of the Leto window.
pub struct LetoApp {
    backend: Backend,
    state: State,
    covers: Covers,
    player: Player,
    /// The graphics card, where it draws the stream's pictures.
    card: Option<OnCard>,
    /// The pane over the place of the pictures, while the graphics card
    /// that decodes them shows them itself.
    pane: Option<Pane>,
    /// Windows opened no pane during this stream: it is not asked again
    /// before the next one.
    pane_refused: bool,
    /// What the camera shows while the person tries it out.
    preview: Preview,
    /// The code a browser types to be let in by its bridge.
    gate: gate::Form,
    /// The backend took requests in the last frame.
    was_open: bool,
    /// The page that chooses the devices was on screen in the last frame.
    was_choosing: bool,
    /// What the person chose, and what of it is kept already.
    settings: Settings,
    saved: Settings,
    /// The settings are on screen, over whatever was there: it carries on
    /// underneath, and is back when they close.
    settings_open: bool,
    /// App time at which the window closes, once the stream it was closed
    /// on has been left.
    closes_at: Option<f64>,
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
            card: card(cc, settings.vsync),
            pane: None,
            pane_refused: false,
            preview: Preview::default(),
            gate: gate::Form::default(),
            was_open: false,
            was_choosing: false,
            saved: settings.clone(),
            settings,
            settings_open: false,
            closes_at: None,
        }
    }

    /// During a stream: has the graphics card that decodes the pictures
    /// show them itself, when the person asked for that, on a pane it keeps
    /// over the place the window leaves them, less what the window shows
    /// there.
    fn place_pane(&mut self, ctx: &egui::Context, window: &eframe::Frame) {
        if !self.settings.straight_to_screen {
            self.close_pane();
            return;
        }
        if self.pane.is_none() && !self.pane_refused {
            self.pane = window_of(window).and_then(Pane::open);
            self.pane_refused = self.pane.is_none();
        }
        let Some(pane) = &mut self.pane else { return };
        self.backend.show_on(Some((pane.handle(), !self.settings.vsync)));
        // Until the card has shown a picture the window leaves no place,
        // and the pane stays hidden.
        let scale = ctx.pixels_per_point();
        let place = self.player.hole().map(|hole| pixels(hole, scale));
        let holes: Vec<_> = self
            .player
            .hole()
            .into_iter()
            .flat_map(|hole| self.player.over().iter().map(move |over| over.intersect(hole)))
            .filter(|over| over.is_positive())
            .map(|over| pixels(over, scale))
            .collect();
        pane.place(place, &holes);
    }

    /// Outside a stream, or when the person no longer asks for it: the
    /// pictures go to the window again, and the pane is closed.
    fn close_pane(&mut self) {
        self.pane_refused = false;
        if self.pane.is_some() {
            // The card hears of it before the pane is gone.
            self.backend.show_on(None);
            self.pane = None;
        }
    }
}

/// The handle Windows gave Leto's window. `None` anywhere else.
#[cfg(not(target_arch = "wasm32"))]
fn window_of(window: &eframe::Frame) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

/// A page in a browser is no window of Windows.
#[cfg(target_arch = "wasm32")]
const fn window_of(_window: &eframe::Frame) -> Option<isize> {
    None
}

/// A place of the window in its pixels, from points.
fn pixels(place: Rect, scale: f32) -> leto_system::pane::Place {
    // Rounded first: a screen has far fewer pixels than an `i32` counts.
    #[allow(clippy::cast_possible_truncation)]
    let pixel = |points: f32| (points * scale).round() as i32;
    let (left, top) = (pixel(place.min.x), pixel(place.min.y));
    [left, top, pixel(place.max.x) - left, pixel(place.max.y) - top]
}

/// The graphics card the window draws with, where it draws the stream's
/// pictures. With `vsync`, a picture never waits behind others for the
/// screen.
#[cfg(not(target_arch = "wasm32"))]
fn card(cc: &eframe::CreationContext<'_>, vsync: bool) -> Option<OnCard> {
    OnCard::new(cc.gl.as_deref(), vsync)
}

/// A browser's page shows the pictures itself.
#[cfg(target_arch = "wasm32")]
const fn card(_cc: &eframe::CreationContext<'_>, _vsync: bool) -> Option<OnCard> {
    None
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

    /// What is on the graphics card goes with the window.
    #[cfg(not(target_arch = "wasm32"))]
    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        if let (Some(card), Some(gl)) = (&self.card, gl) {
            card.destroy(gl);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, window: &mut eframe::Frame) {
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

        // The devices chosen are used from the next moment, and the page
        // that chooses them starts with the ones there are now.
        self.backend.choose_devices(&self.settings.devices);
        let choosing = matches!((&door, &self.state.screen), (Door::Open, Screen::Devices(_)));
        if choosing && !self.was_choosing {
            self.backend.look_for_devices();
        }
        self.was_choosing = choosing;

        // The camera and the microphone are tried out on one screen, and
        // are off everywhere else, the settings over it included.
        let tried = match (&door, &self.state.screen) {
            (Door::Open, Screen::Devices(devices)) if !self.settings_open => devices.clone(),
            _ => Devices::default(),
        };
        let heard = self.try_devices(ui.ctx(), &tried);

        // Closing the window leaves a connected stream first, as the person
        // would: the PC lets go of what was held, and draws its pointer
        // again. The window closes a moment later.
        let close = ui.input(|input| input.viewport().close_requested());
        if close
            && self.closes_at.is_none()
            && matches!(&self.state.screen, Screen::Stream(view) if view.connected)
        {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.player.leave_to_close();
            self.closes_at = Some(now + CLOSES_AFTER_LEAVING);
        }
        if let Some(at) = self.closes_at {
            if now >= at {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                ui.ctx().request_repaint_after_secs((at - now) as f32);
            }
        }

        let streaming = matches!((&door, &self.state.screen), (Door::Open, Screen::Stream(_)));
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
                self.backend.allow_graphics_card(self.settings.decoder.hardware());
                let keeps = self.card.as_ref().is_some_and(OnCard::keeps);
                self.backend.keep_on_card(self.settings.keep_on_card && keeps);
                if let Some(picture) = self.backend.take_picture() {
                    self.player.picture(ui.ctx(), picture, self.card.as_ref());
                }
                let sound_problem = self.backend.sound_problem();
                let picture_problem = self.backend.picture_problem();
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
                            picture_problem: picture_problem.as_deref(),
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
                self.place_pane(ui.ctx(), window);
            }
            (door, screen) => {
                self.player.clear(ui.ctx());
                self.backend.place_picture(None);
                self.close_pane();
                let ground = egui::Frame::new().fill(theme::GROUND);
                egui::CentralPanel::default().frame(ground).show(ui, |ui| {
                    top_bar(ui);
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
                                let back_to = back_to(&self.state.screen).to_owned();
                                let decodable = self.backend.decodable();
                                let settings = &mut self.settings;
                                // The tunnel is this device's: a browser
                                // has the one of its bridge, and no say.
                                let tunnel = self.backend.has_tunnel().then(|| {
                                    self.state.watch_tunnel(now);
                                    ui.ctx().request_repaint_after_secs(1.0);
                                    &mut self.state.tunnel
                                });
                                let asked = ui::settings::show(
                                    ui, settings, native, decodable, tunnel, &back_to,
                                );
                                match asked.tunnel {
                                    Some(ui::tunnel::Asked::Use) => self.state.use_tunnel(),
                                    Some(ui::tunnel::Asked::Remove) => self.state.remove_tunnel(),
                                    None => {}
                                }
                                if asked.back {
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
                                if ui::pcs::show(ui, &mut self.state, now) {
                                    self.settings_open = true;
                                }
                                ui.add_space(24.0);
                                ui.label(widgets::dim(build_line()).size(theme::SMALL - 1.0));
                            }
                            (Door::Open, Screen::Devices(devices)) => {
                                let (names, chime) =
                                    (self.backend.device_names(), self.backend.chime());
                                let choices = ui::devices::Choices {
                                    chosen: &mut self.settings.devices,
                                    names: names.as_ref(),
                                    chime: &chime,
                                };
                                let preview = &self.preview;
                                let asked = ui::devices::show(
                                    ui,
                                    &mut self.state,
                                    &devices,
                                    preview,
                                    heard,
                                    choices,
                                    now,
                                );
                                if asked.look_for_devices {
                                    self.backend.look_for_devices();
                                }
                                if asked.test_sound {
                                    self.backend.test_sound();
                                }
                            }
                            (Door::Open, Screen::Pairing(pairing)) => {
                                ui::pairing::show(ui, &mut self.state, &pairing, now);
                            }
                            (Door::Open, Screen::Apps(view)) => {
                                let (covers, settings) = (&self.covers, &self.settings);
                                let decodable = || self.backend.decodable();
                                ui::apps::show(
                                    ui,
                                    &mut self.state,
                                    &view,
                                    covers,
                                    settings,
                                    decodable,
                                    now,
                                );
                            }
                            // Handled above.
                            (Door::Open, Screen::Stream(_)) => {}
                        });
                    });
                });
            }
        }

        if !streaming {
            // Last, so that the pointer on an edge shows that it resizes the
            // window, whatever is under it. A stream's edges are the PC's.
            frame::edges(ui.ctx());
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
            self.saved = self.settings.clone();
            settings::save(self.settings.clone());
        }
        if open && self.state.screen == Screen::Pcs {
            // Wake up for the next search even if nobody touches the app.
            ui.ctx().request_repaint_after_secs(1.0);
        }
    }
}

/// The bar at the top: the logo and nothing else. Where Leto draws the
/// window's frame, the bar moves the window, and the buttons that minimize,
/// maximize and close it sit in its top right corner.
fn top_bar(ui: &mut egui::Ui) {
    let width = ui.available_width();
    // The logo is as far from the top of the window as from its left side,
    // where the pages under it start too.
    let margin = if width < 560.0 { 16.0 } else { 32.0 };
    let bar = Rect::from_min_size(ui.cursor().min, egui::vec2(width, margin + LOGO.y + 16.0));
    frame::drag_by(ui, bar, Id::unique("top-bar"));
    let buttons_width = 3.0 * frame::BUTTON_WIDTH;
    if frame::OWN {
        // In the corner, where the pointer stops, as Windows puts them.
        let corner = Rect::from_min_size(
            bar.right_top() - egui::vec2(buttons_width, 0.0),
            egui::vec2(buttons_width, frame::BUTTON_HEIGHT),
        );
        let layout = Layout::right_to_left(Align::Min);
        // Taking no room from the bar, which goes under them.
        let mut buttons = ui.new_child(UiBuilder::new().max_rect(corner).layout(layout));
        frame::buttons(&mut buttons, egui::vec2(frame::BUTTON_WIDTH, frame::BUTTON_HEIGHT), 0);
    }
    // The line the logo sits on.
    let line = Rect::from_center_size(
        pos2(bar.center().x, bar.top() + margin + LOGO.y / 2.0),
        egui::vec2(width, 36.0),
    );
    let mut line_ui =
        ui.new_child(UiBuilder::new().max_rect(line).layout(Layout::left_to_right(Align::Center)));
    line_ui.add_space(margin);
    line_ui.add(
        egui::Image::new(egui::include_image!("../../../assets/brand/png/leto-logo.png"))
            .fit_to_exact_size(LOGO),
    );
    ui.advance_cursor_after_rect(bar);
}

/// The name of the screen under the settings, for their back button.
fn back_to(screen: &Screen) -> &str {
    match screen {
        Screen::Pcs => "PCs",
        Screen::Devices(_) => ui::devices::NAME,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_place_in_points_is_the_same_place_in_pixels() {
        let place = Rect::from_min_max(pos2(10.0, 20.4), pos2(110.0, 70.4));
        assert_eq!(pixels(place, 1.0), [10, 20, 100, 50]);
        // Each edge goes to the nearest pixel: two places side by side
        // stay side by side.
        assert_eq!(pixels(place, 1.5), [15, 31, 150, 75]);
        let beside = Rect::from_min_max(pos2(110.0, 20.4), pos2(200.0, 70.4));
        assert_eq!(pixels(beside, 1.5)[0], 15 + 150);
    }
}
