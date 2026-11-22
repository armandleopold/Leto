//! The devices Apollo is paired with, and what each may do: the card
//! "Registered clients" of "This PC".
//!
//! What it shows is read from Apollo's folder, as the rest of this screen
//! is. What the owner of the PC changes here, Apollo changes itself: Leto
//! asks its own pages (`leto_host::pages`), once the owner has signed in
//! with their name and their password. The password is given to Apollo and
//! kept nowhere.

use std::{
    sync::{
        Arc, Mutex, PoisonError,
        mpsc::{self, RecvTimeoutError, Sender},
    },
    thread,
    time::Duration,
};

use egui::{Align, Button, Color32, CursorIcon, Layout, RichText, Sense, Stroke, Ui, vec2};
use leto_host::{
    apollo::{Apollo, Device, RIGHTS, Right},
    pages::{Client, Error, Pages},
};

use super::{card, dot};
use crate::{theme, ui::widgets};

/// How often Apollo is asked for its devices while nothing is changed.
const LOOKED_EVERY: Duration = Duration::from_secs(2);

/// What is asked of Apollo's pages.
enum Ask {
    Update(Client),
    Unpair(String),
    Disconnect(String),
    Pair { pin: String, name: String },
}

/// What Apollo's pages said last.
#[derive(Default)]
struct Told {
    clients: Vec<Client>,
    /// Why the last thing asked was not done.
    problem: Option<String>,
    /// Apollo wants the owner to sign in again.
    signed_out: bool,
    /// Whether Apollo was waiting for the last PIN typed.
    paired: Option<bool>,
}

/// Apollo's pages, signed in to, and asked on a thread of their own: the
/// window never waits for them.
struct Session {
    ask: Sender<Ask>,
    told: Arc<Mutex<Told>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Session {
    fn start(pages: Pages, ctx: egui::Context) -> Self {
        let (ask, asked) = mpsc::channel();
        let told = Arc::new(Mutex::new(Told::default()));
        thread::spawn({
            let told = Arc::clone(&told);
            move || {
                let mut first = true;
                loop {
                    let ask = if std::mem::take(&mut first) {
                        None
                    } else {
                        match asked.recv_timeout(LOOKED_EVERY) {
                            Ok(ask) => Some(ask),
                            Err(RecvTimeoutError::Timeout) => None,
                            // The card is gone.
                            Err(RecvTimeoutError::Disconnected) => return,
                        }
                    };
                    let mut paired = None;
                    let done = match ask {
                        Some(Ask::Update(client)) => pages.update(&client),
                        Some(Ask::Unpair(id)) => pages.unpair(&id),
                        Some(Ask::Disconnect(id)) => pages.disconnect(&id),
                        Some(Ask::Pair { pin, name }) => {
                            pages.pair(&pin, &name).map(|was_awaited| paired = Some(was_awaited))
                        }
                        None => Ok(()),
                    };
                    let asked_something = paired.is_some() || done.is_err();
                    // What Apollo knows now, whatever came of what was asked.
                    let listed = pages.clients();
                    let mut told = lock(&told);
                    if paired.is_some() {
                        told.paired = paired;
                    }
                    let signed_out = matches!(done, Err(Error::SignedOut))
                        || matches!(listed, Err(Error::SignedOut));
                    match (done, listed) {
                        (Err(error), _) => told.problem = Some(format!("{error}.")),
                        (Ok(()), Err(error)) => told.problem = Some(format!("{error}.")),
                        (Ok(()), Ok(clients)) => {
                            if told.clients != clients || asked_something {
                                ctx.request_repaint();
                            }
                            told.clients = clients;
                            told.problem = None;
                        }
                    }
                    if signed_out {
                        told.signed_out = true;
                        ctx.request_repaint();
                        return;
                    }
                    if told.problem.is_some() {
                        ctx.request_repaint();
                    }
                }
            }
        });
        Self { ask, told }
    }
}

/// The card, for as long as this PC is shared beside an Apollo.
#[derive(Default)]
pub struct Registered {
    /// The name and the password of Apollo's pages, as they are typed.
    name: String,
    password: String,
    /// Why signing in did not work.
    refused: Option<String>,
    session: Option<Session>,
    /// The device being given another name, and the name as it is typed.
    renaming: Option<(String, String)>,
    /// The field for that name has just appeared: it is typed in at once.
    renaming_new: bool,
    /// The device the owner is asked again about before it is forgotten.
    unpairing: Option<String>,
    /// The PIN a new device shows, and a name for it, as they are typed.
    pin: String,
    pin_name: String,
}

/// What the owner asked for on the row of one device.
enum Asked {
    Nothing,
    Update(Client),
    Unpair(String),
    Disconnect(String),
}

impl Registered {
    /// The card. `devices` is what Apollo's file says, `sender` and
    /// `speaker` the names of the devices that send a camera and a
    /// microphone at this moment.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        apollo: &Apollo,
        devices: &[Device],
        sender: Option<&str>,
        speaker: Option<&str>,
    ) {
        // A sign-in Apollo no longer takes is over.
        if self.session.as_ref().is_some_and(|session| lock(&session.told).signed_out) {
            self.session = None;
            self.refused = Some("Apollo asks to sign in again.".to_owned());
        }
        card(ui, "Registered clients", |ui| {
            match &self.session {
                Some(_) => self.signed_in(ui, sender, speaker),
                None => {
                    listed_from_file(ui, devices, sender, speaker);
                    self.sign_in(ui, apollo);
                }
            }
            ui.add_space(2.0);
            ui.add(
                egui::Label::new(
                    widgets::dim(
                        "A device may send its camera and its microphone once it may use the \
                         mouse and the keyboard.",
                    )
                    .size(theme::SMALL),
                )
                .wrap(),
            );
        });
    }

    /// The devices as Apollo's pages list them, each with what can be
    /// changed about it, and the place to pair one more.
    fn signed_in(&mut self, ui: &mut Ui, sender: Option<&str>, speaker: Option<&str>) {
        // Taken for the time of the frame: the rows change the card too.
        let Some(session) = self.session.take() else { return };
        let (clients, problem, paired) = {
            let told = lock(&session.told);
            (told.clients.clone(), told.problem.clone(), told.paired)
        };
        if clients.is_empty() {
            ui.label(widgets::dim("Apollo is paired with no device yet."));
        }
        let mut asked = Asked::Nothing;
        for (number, client) in clients.iter().enumerate() {
            if number > 0 {
                ui.separator();
            }
            let row = self.row(ui, client, sender, speaker);
            if !matches!(row, Asked::Nothing) {
                asked = row;
            }
        }
        match asked {
            Asked::Nothing => {}
            Asked::Update(client) => {
                // Shown at once, and put right by what Apollo says next.
                let mut told = lock(&session.told);
                if let Some(known) = told.clients.iter_mut().find(|known| known.id == client.id) {
                    *known = client.clone();
                }
                let _ = session.ask.send(Ask::Update(client));
            }
            Asked::Unpair(id) => {
                let _ = session.ask.send(Ask::Unpair(id));
            }
            Asked::Disconnect(id) => {
                let _ = session.ask.send(Ask::Disconnect(id));
            }
        }
        if let Some(problem) = problem {
            ui.add(egui::Label::new(RichText::new(problem).color(theme::problem())).wrap());
        }

        ui.separator();
        ui.label("Pair a device");
        ui.add(
            egui::Label::new(
                widgets::dim("Type the PIN the device shows, and a name to know it by.")
                    .size(theme::SMALL),
            )
            .wrap(),
        );
        ui.horizontal(|ui| {
            self.pin.retain(|digit| digit.is_ascii_digit());
            self.pin.truncate(4);
            ui.add(field(&mut self.pin, "PIN", 80.0));
            ui.add(field(&mut self.pin_name, "Name of the device", 220.0));
            let ready = self.pin.len() == 4 && !self.pin_name.trim().is_empty();
            if ui.add_enabled(ready, small_button("Pair", true)).clicked() {
                lock(&session.told).paired = None;
                let _ = session.ask.send(Ask::Pair {
                    pin: std::mem::take(&mut self.pin),
                    name: std::mem::take(&mut self.pin_name).trim().to_owned(),
                });
            }
            match paired {
                Some(true) => {
                    ui.label(RichText::new("Paired.").color(theme::ready()));
                }
                Some(false) => {
                    ui.label(
                        RichText::new("Apollo was not waiting for this PIN.")
                            .color(theme::problem()),
                    );
                }
                None => {}
            }
        });
        self.session = Some(session);
    }

    /// One device: its name, what it is doing, what it may do, and the
    /// buttons that change it.
    fn row(
        &mut self,
        ui: &mut Ui,
        client: &Client,
        sender: Option<&str>,
        speaker: Option<&str>,
    ) -> Asked {
        let mut asked = Asked::Nothing;
        let renamed = self.renaming.as_ref().is_some_and(|(id, _)| *id == client.id);
        let doubted = self.unpairing.as_deref() == Some(client.id.as_str());
        ui.horizontal(|ui| {
            if let (true, Some((_, name))) = (renamed, &mut self.renaming) {
                let typed = ui.add(field(name, "Name of the device", 240.0));
                if std::mem::take(&mut self.renaming_new) {
                    typed.request_focus();
                }
                let entered =
                    typed.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if ui.add(small_button("Save", true)).clicked() || entered {
                    let name = name.trim().to_owned();
                    if !name.is_empty() && name != client.name {
                        let mut named = client.clone();
                        named.name = name;
                        asked = Asked::Update(named);
                    }
                    self.renaming = None;
                } else if ui.add(small_button("Cancel", false)).clicked() {
                    self.renaming = None;
                }
                return;
            }
            let name = if client.name.is_empty() { "A device" } else { &client.name };
            ui.label(RichText::new(name).strong());
            doing(ui, client.connected, &client.name, sender, speaker);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if doubted {
                    if ui.add(small_button("Keep", false)).clicked() {
                        self.unpairing = None;
                    }
                    if ui.add(small_button("Unpair", true)).clicked() {
                        asked = Asked::Unpair(client.id.clone());
                        self.unpairing = None;
                    }
                    ui.label(widgets::dim("It will have to be paired again."));
                    return;
                }
                if ui.add(small_button("Unpair", false)).clicked() {
                    self.unpairing = Some(client.id.clone());
                }
                if client.connected && ui.add(small_button("End its stream", false)).clicked() {
                    asked = Asked::Disconnect(client.id.clone());
                }
                if ui.add(small_button("Rename", false)).clicked() {
                    self.renaming = Some((client.id.clone(), client.name.clone()));
                    self.renaming_new = true;
                }
            });
        });
        if let Some(permissions) = rights(ui, client.permissions, true) {
            let mut allowed = client.clone();
            allowed.permissions = permissions;
            asked = Asked::Update(allowed);
        }
        asked
    }

    /// Where the owner signs in to Apollo's pages, to change anything.
    fn sign_in(&mut self, ui: &mut Ui, apollo: &Apollo) {
        ui.separator();
        ui.add(
            egui::Label::new(widgets::dim(
                "To change what a device may do, to rename it or to unpair it, sign in with the \
                 name and the password of Apollo's own page. Apollo makes the change itself, \
                 and a browser that was signed in to its page is signed out.",
            ))
            .wrap(),
        );
        ui.add_space(4.0);
        let mut go = false;
        ui.horizontal(|ui| {
            ui.add(field(&mut self.name, "Name", 180.0));
            let password = ui.add(field(&mut self.password, "Password", 180.0).password(true));
            go = password.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            let ready = !self.name.is_empty() && !self.password.is_empty();
            go |= ui.add_enabled(ready, small_button("Sign in", true)).clicked();
            go &= ready;
        });
        if go {
            // Apollo is on this very computer: it answers at once, or not
            // at all.
            match Pages::sign_in(apollo, self.name.trim(), &self.password) {
                Ok(pages) => {
                    self.session = Some(Session::start(pages, ui.ctx().clone()));
                    self.refused = None;
                }
                Err(error) => {
                    log::info!("signing in to Apollo's pages did not work: {error}");
                    self.refused = Some(format!("{error}."));
                }
            }
            // Typed again next time: it is kept nowhere.
            self.password.clear();
        }
        if let Some(refused) = &self.refused {
            ui.add(egui::Label::new(RichText::new(refused).color(theme::problem())).wrap());
        }
    }
}

/// The devices as Apollo's file names them: what each may do, to look at.
fn listed_from_file(ui: &mut Ui, devices: &[Device], sender: Option<&str>, speaker: Option<&str>) {
    if devices.is_empty() {
        ui.label(widgets::dim("Apollo is paired with no device yet. Pair one from Leto."));
    }
    for (number, device) in devices.iter().enumerate() {
        if number > 0 {
            ui.separator();
        }
        ui.horizontal(|ui| {
            let name = if device.name.is_empty() { "A device" } else { &device.name };
            ui.label(RichText::new(name).strong());
            doing(ui, false, &device.name, sender, speaker);
        });
        rights(ui, device.permissions, false);
    }
}

/// What a device is doing at this moment, after its name.
fn doing(ui: &mut Ui, streaming: bool, name: &str, sender: Option<&str>, speaker: Option<&str>) {
    let sends = match (sender == Some(name), speaker == Some(name)) {
        (true, true) => Some("sending its camera and its microphone"),
        (true, false) => Some("sending its camera"),
        (false, true) => Some("sending its microphone"),
        (false, false) => None,
    };
    let said = match (streaming, sends) {
        (true, Some(sends)) => format!("Streaming, {sends}"),
        (true, None) => "Streaming".to_owned(),
        (false, Some(sends)) => {
            let mut said = sends.to_owned();
            if let Some(first) = said.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            said
        }
        (false, None) => return,
    };
    dot(ui, theme::ready());
    ui.label(RichText::new(said).color(theme::ready()).size(theme::SMALL));
}

/// What a device may do, a group to a line. With `changes`, a press on one
/// gives or takes it: the permissions that result are returned.
fn rights(ui: &mut Ui, permissions: u32, changes: bool) -> Option<u32> {
    let mut changed = None;
    for (group, of_group) in RIGHTS {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            let (place, _) = ui.allocate_exact_size(vec2(72.0, 26.0), Sense::hover());
            ui.painter().text(
                place.left_center(),
                egui::Align2::LEFT_CENTER,
                group,
                egui::FontId::proportional(theme::SMALL),
                theme::dim(),
            );
            for right in of_group {
                if chip(ui, right, permissions, changes) {
                    changed = Some(permissions ^ right.bit);
                }
            }
        });
    }
    changed
}

/// One thing a device may do: lit when it may. Returns whether it was
/// pressed to change it.
fn chip(ui: &mut Ui, right: &Right, permissions: u32, changes: bool) -> bool {
    let held = right.held_by(permissions);
    // One that comes with another cannot be taken away alone.
    let brought = right.brought_to(permissions);
    let (fill, text, stroke) = match (held, brought) {
        (true, false) => (theme::sun(), theme::on_sun(), Stroke::NONE),
        (true, true) => (theme::sun().gamma_multiply(0.55), theme::on_sun(), Stroke::NONE),
        (false, _) => (Color32::TRANSPARENT, theme::dim(), Stroke::new(1.0, theme::line())),
    };
    let pressed = changes && !brought;
    let button = Button::new(RichText::new(right.name).size(theme::SMALL).color(text))
        .fill(fill)
        .stroke(stroke)
        .corner_radius(theme::CONTROL_RADIUS)
        .min_size(vec2(0.0, 26.0))
        .sense(if pressed { Sense::click() } else { Sense::hover() });
    let mut response = ui.add(button);
    if pressed {
        response = response.on_hover_cursor(CursorIcon::PointingHand);
    } else if brought && changes {
        let with = RIGHTS
            .iter()
            .flat_map(|(_, rights)| rights.iter())
            .find(|other| other.bit & right.comes_with & permissions != 0)
            .map_or("another", |other| other.name);
        response = response.on_hover_text(format!("Comes with \u{201c}{with}\u{201d}."));
    }
    response.clicked()
}

/// A field to type in, as wide as said.
fn field<'a>(text: &'a mut String, hint: &str, width: f32) -> egui::TextEdit<'a> {
    egui::TextEdit::singleline(text)
        .hint_text(hint)
        .desired_width(width)
        .margin(egui::Margin::symmetric(10, 7))
}

/// A button of the size of a line of the card. `main` for the one a row
/// wants pressed.
fn small_button(text: &str, main: bool) -> Button<'_> {
    let button = if main {
        Button::new(RichText::new(text).color(theme::on_sun()))
            .fill(theme::sun())
            .stroke(Stroke::NONE)
    } else {
        Button::new(text).fill(Color32::TRANSPARENT)
    };
    button.corner_radius(theme::CONTROL_RADIUS).min_size(vec2(64.0, 30.0))
}
