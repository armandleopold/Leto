//! This PC, shared: Leto as the host, in the same window as the client.
//!
//! For now it is the companion of `docs/return-channel.md`: beside an
//! Apollo or a Sunshine that streams the screen as before, Leto takes the
//! camera of the devices they are paired with. See the `leto-host` crate.
//!
//! While this PC is shared, closing the window puts it away and Leto goes
//! on beside the clock, on Windows, where an icon brings it back.

use std::time::{Duration, Instant};

use egui::{Align, Color32, Layout, RichText, Stroke, TextureHandle, TextureOptions, Ui, vec2};
use leto_host::{
    Companion, Options,
    apollo::{Apollo, Device, Error},
};

use crate::{
    theme,
    ui::{pcs::THIS_PC, widgets},
};

/// How often the screen looks at what changed while nobody touches it.
const GLANCE: Duration = Duration::from_millis(250);

/// How often Apollo's list of devices is read again.
const DEVICES_EVERY: Duration = Duration::from_secs(2);

/// What Leto is doing for the devices that reach this PC.
enum State {
    /// Nothing: this PC is not shared.
    Off,
    /// Beside an Apollo, taking cameras.
    Beside(Companion),
    /// Asked to, and could not: why, in words for the person, and whether
    /// it is the system that keeps Apollo's settings from Leto.
    Stopped(String, bool),
}

/// This PC as a host, for as long as the app runs.
pub struct Sharing {
    state: State,
    devices: Vec<Device>,
    devices_read: Instant,
    /// What the camera that is being sent sees, and how many pictures had
    /// arrived when it was last drawn.
    camera: Option<(TextureHandle, u64)>,
    /// The icon beside the clock, while this PC is shared.
    #[cfg(windows)]
    tray: Option<tray_icon::TrayIcon>,
}

impl Default for Sharing {
    fn default() -> Self {
        Self {
            state: State::Off,
            devices: Vec::new(),
            devices_read: Instant::now(),
            camera: None,
            #[cfg(windows)]
            tray: None,
        }
    }
}

impl Sharing {
    /// Shares this PC or stops, as the setting says: called every frame,
    /// and does something when the setting changed. `window` is the handle
    /// of Leto's window, for the icon beside the clock to bring it back.
    pub fn follow(&mut self, wanted: bool, window: Option<isize>, ctx: &egui::Context) {
        match (&self.state, wanted) {
            (State::Off, true) => self.start(),
            (State::Beside(_) | State::Stopped(..), false) => {
                self.state = State::Off;
                self.camera = None;
            }
            _ => {}
        }
        #[cfg(windows)]
        {
            let serving = matches!(self.state, State::Beside(_));
            if serving && self.tray.is_none() {
                self.tray = window.and_then(|window| tray::start(window, ctx));
            } else if !serving {
                self.tray = None;
            }
        }
        #[cfg(not(windows))]
        let _ = (window, ctx);
    }

    /// Whether Leto goes on once its window is closed: while this PC is
    /// shared and an icon beside the clock can bring the window back.
    pub fn stays(&self) -> bool {
        #[cfg(windows)]
        return self.tray.is_some();
        #[cfg(not(windows))]
        false
    }

    /// Reads what Apollo keeps and starts beside it.
    fn start(&mut self) {
        self.state = match Apollo::find() {
            Ok(apollo) => match Companion::start(apollo, &Options::default()) {
                Ok(companion) => State::Beside(companion),
                Err(error) => State::Stopped(
                    format!(
                        "The port for the camera could not be opened: {error}. Leto may already \
                         be running on this PC: look beside the clock."
                    ),
                    false,
                ),
            },
            Err(error) => {
                let kept = matches!(&error, Error::Unreadable(_, error)
                    if error.kind() == std::io::ErrorKind::PermissionDenied);
                State::Stopped(format!("{error}."), kept)
            }
        };
        if let State::Stopped(why, _) = &self.state {
            log::warn!("this PC is not shared: {why}");
        }
        self.read_devices();
    }

    fn read_devices(&mut self) {
        self.devices_read = Instant::now();
        if let State::Beside(companion) = &self.state {
            // A file Apollo is writing at this moment keeps the last list.
            if let Ok(devices) = companion.apollo().devices() {
                self.devices = devices;
            }
        }
    }

    /// The screen. `share` is the setting. Returns whether the person asked
    /// to go back to `back_to`.
    pub fn show(&mut self, ui: &mut Ui, share: &mut bool, back_to: &str) -> bool {
        if self.devices_read.elapsed() >= DEVICES_EVERY {
            self.read_devices();
        }
        let back = widgets::back_button(ui, back_to).clicked();
        widgets::title(ui, THIS_PC);
        ui.add_space(8.0);

        card(ui, "Share this PC", |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width((ui.available_width() - 64.0).max(120.0));
                    ui.add(
                        egui::Label::new(widgets::dim(
                            "Your devices reach this PC through Leto. For now Leto works \
                             beside Apollo or Sunshine, which stream the screen, and takes \
                             what they do not: the camera of your devices.",
                        ))
                        .wrap(),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    widgets::switch(ui, share, "Share this PC");
                });
            });
        });

        let mut again = false;
        match &self.state {
            State::Off => {}
            State::Stopped(why, kept) => card(ui, "Not running", |ui| {
                ui.add(egui::Label::new(RichText::new(why).color(theme::problem())).wrap());
                // Windows keeps what is under `Program Files` from ordinary
                // programs: only an administrator reads Apollo's key.
                let elevates = *kept && cfg!(windows);
                let advice = if elevates {
                    "Windows only lets an administrator read the settings of Apollo. Leto can \
                     start again as one: Windows asks you first."
                } else if *kept {
                    "The system does not let Leto read these settings."
                } else {
                    "Apollo or Sunshine has to be installed on this PC."
                };
                ui.add(egui::Label::new(widgets::dim(advice)).wrap());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if elevates {
                        if widgets::primary_button(ui, "Restart as administrator").clicked()
                            && leto_system::elevate::restart_as_administrator()
                        {
                            // The other Leto shares from here on: the choice
                            // is kept already.
                            std::process::exit(0);
                        }
                        again = widgets::quiet_button(ui, "Try again").clicked();
                    } else {
                        again = widgets::primary_button(ui, "Try again").clicked();
                    }
                });
            }),
            State::Beside(companion) => {
                card(ui, "Running", |ui| {
                    ui.horizontal(|ui| {
                        dot(ui, theme::ready());
                        ui.label(format!("Beside Apollo, on port {}", companion.port()));
                    });
                    ui.add(
                        egui::Label::new(widgets::dim(format!(
                            "Devices are the ones Apollo is paired with. Its settings are read \
                             from {} and never changed.",
                            companion.apollo().folder().display()
                        )))
                        .wrap(),
                    );
                });
                let sender = companion.sender();
                devices(ui, &self.devices, sender.as_deref());
                camera(ui, companion, sender.as_deref(), &mut self.camera);
                if self.stays() {
                    ui.add(
                        egui::Label::new(
                            widgets::dim(
                                "Closing the window keeps Leto running while this PC is \
                                 shared. Its icon beside the clock brings the window back, \
                                 and quits Leto.",
                            )
                            .size(theme::SMALL),
                        )
                        .wrap(),
                    );
                }
                ui.ctx().request_repaint_after(GLANCE);
            }
        }
        if again {
            self.start();
        }
        back
    }
}

/// The devices Apollo is paired with, and which may send a camera.
fn devices(ui: &mut Ui, devices: &[Device], sender: Option<&str>) {
    card(ui, "Devices", |ui| {
        if devices.is_empty() {
            ui.label(widgets::dim("Apollo is paired with no device yet. Pair one from Leto."));
        }
        for device in devices {
            let name = if device.name.is_empty() { "A device" } else { &device.name };
            ui.horizontal(|ui| {
                ui.label(name);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if sender == Some(device.name.as_str()) {
                        ui.label(RichText::new("Sending its camera").color(theme::sun()));
                    } else if device.trusted() {
                        ui.label(widgets::dim("May send its camera"));
                    } else {
                        ui.label(widgets::dim("No camera")).on_hover_text(
                            "Apollo does not let this device use the keyboard and the mouse.",
                        );
                    }
                });
            });
        }
        if devices.iter().any(|device| !device.trusted()) {
            ui.add_space(2.0);
            ui.add(
                egui::Label::new(
                    widgets::dim(
                        "A device may send its camera once Apollo lets it use the keyboard \
                         and the mouse: tick both for it on Apollo's page, under PIN.",
                    )
                    .size(theme::SMALL),
                )
                .wrap(),
            );
        }
    });
}

/// What the camera that is being sent sees.
fn camera(
    ui: &mut Ui,
    companion: &Companion,
    sender: Option<&str>,
    shown: &mut Option<(TextureHandle, u64)>,
) {
    card(ui, "Camera", |ui| {
        let report = companion.cameras().report();
        if !report.open {
            *shown = None;
            ui.add(
                egui::Label::new(widgets::dim(
                    "No device is sending its camera. In Leto, on the device, turn the camera \
                     on during a stream.",
                ))
                .wrap(),
            );
            return;
        }
        let from = sender.filter(|name| !name.is_empty()).unwrap_or("a device");
        match report.size {
            Some((width, height)) => ui.label(format!("From {from}, {width} by {height}")),
            None => ui.label(format!("From {from}: waiting for its first picture")),
        };
        // Drawn again only when a picture arrived since.
        if shown.as_ref().is_none_or(|(_, pictures)| *pictures != report.pictures)
            && let Some(seen) = companion.cameras().seen()
        {
            let image =
                egui::ColorImage::from_rgba_unmultiplied([seen.width, seen.height], &seen.rgba);
            match shown {
                Some((texture, pictures)) => {
                    texture.set(image, TextureOptions::LINEAR);
                    *pictures = report.pictures;
                }
                None => {
                    let texture = ui.ctx().load_texture("camera", image, TextureOptions::LINEAR);
                    *shown = Some((texture, report.pictures));
                }
            }
        }
        if let Some((texture, _)) = shown {
            let size = texture.size_vec2();
            // Small enough to leave the rest of the screen in sight.
            let width = ui.available_width().min(size.x).min(420.0);
            let image = egui::Image::new(&*texture)
                .fit_to_exact_size(vec2(width, width * size.y / size.x))
                .corner_radius(theme::CONTROL_RADIUS);
            ui.add(image);
        }
        if report.lost > 0 {
            ui.label(
                widgets::dim(format!(
                    "{} pictures arrived, {} were lost",
                    report.pictures, report.lost
                ))
                .size(theme::SMALL),
            );
        }
        ui.add(
            egui::Label::new(
                widgets::dim("The apps of this PC do not see it as a camera yet: that is next.")
                    .size(theme::SMALL),
            )
            .wrap(),
        );
    });
}

/// A group of things, under its name.
fn card(ui: &mut Ui, name: &str, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(theme::surface())
        .stroke(Stroke::new(1.0, theme::line()))
        .corner_radius(theme::CARD_RADIUS)
        .inner_margin(egui::Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(name).size(18.0));
            ui.add_space(2.0);
            add(ui);
        });
    ui.add_space(12.0);
}

/// The dot that says something is going well.
fn dot(ui: &mut Ui, colour: Color32) {
    let (place, _) = ui.allocate_exact_size(vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(place.center(), 5.0, colour);
}

/// The icon beside the clock, on Windows.
#[cfg(windows)]
mod tray {
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
        menu::{Menu, MenuEvent, MenuItem},
    };

    /// Puts Leto's icon beside the clock. A click on it, or "Open Leto" in
    /// its menu, brings back the window with this handle; "Quit" ends Leto.
    ///
    /// The icon lives as long as what is returned. It has to be made by the
    /// thread that runs the window, which is where Windows sends its clicks.
    pub fn start(window: isize, ctx: &egui::Context) -> Option<TrayIcon> {
        let icon =
            image::load_from_memory(include_bytes!("../../../assets/brand/png/leto-icon-32.png"))
                .ok()?
                .into_rgba8();
        let (width, height) = icon.dimensions();
        let icon = Icon::from_rgba(icon.into_raw(), width, height).ok()?;

        let open = MenuItem::new("Open Leto", true, None);
        let quit = MenuItem::new("Quit", true, None);
        let menu = Menu::new();
        menu.append_items(&[&open, &quit]).ok()?;
        let tray = TrayIconBuilder::new()
            .with_icon(icon)
            .with_tooltip("Leto: this PC is shared")
            .with_menu(Box::new(menu))
            // A click opens the window, and the other button the menu.
            .with_menu_on_left_click(false)
            .build()
            .ok()?;

        let bring_back = {
            let ctx = ctx.clone();
            move || {
                leto_system::shown::show(window);
                ctx.request_repaint();
            }
        };
        let (open, quit) = (open.id().clone(), quit.id().clone());
        MenuEvent::set_event_handler(Some({
            let bring_back = bring_back.clone();
            move |event: MenuEvent| {
                if event.id == open {
                    bring_back();
                } else if event.id == quit {
                    // The settings are kept as they change: nothing is left
                    // to put away.
                    std::process::exit(0);
                }
            }
        }));
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                bring_back();
            }
        }));
        Some(tray)
    }
}
