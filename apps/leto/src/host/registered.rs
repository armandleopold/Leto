//! The devices Apollo is paired with, and what each may do: the page of
//! one device, behind its card on the first screen, and the page that
//! pairs one more.
//!
//! What they show is read from Apollo's folder, as "This PC" is. What the
//! owner of the PC changes here, Apollo changes itself: Leto
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

use std::collections::BTreeMap;

use egui::{
    Button, Color32, CursorIcon, RichText, Role, Sense, Stroke, Ui, Vec2, WidgetInfo, vec2,
};
use leto_host::{
    apollo::{Apollo, Device, RIGHTS, Right},
    pages::{Client, Error, Pages},
};

use super::card;
use crate::{
    settings::ClientIcon,
    theme,
    ui::{icons, widgets},
};

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
    /// Apollo listed its devices at least once.
    heard: bool,
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
                            told.heard = true;
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

/// A device Apollo is paired with, as its pages or its file tell of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Known {
    /// What Apollo knows it by. Empty for a device of an old Sunshine.
    pub id: String,
    pub name: String,
    /// What it may do: see [`RIGHTS`].
    pub permissions: u32,
    /// It is streaming at this moment, which only Apollo's pages say.
    pub streaming: bool,
}

impl Known {
    /// What tells this device from the others: what Apollo knows it by,
    /// or its name where a Sunshine keeps nothing else.
    pub fn key(&self) -> &str {
        if self.id.is_empty() { &self.name } else { &self.id }
    }

    /// What it may do, in a few words, for its card.
    pub fn may(&self) -> String {
        let every = RIGHTS.iter().flat_map(|(_, rights)| rights.iter());
        if every.clone().all(|right| right.held_by(self.permissions)) {
            return "May do everything".to_owned();
        }
        let [(_, apps), _, (_, input)] = RIGHTS;
        // The last of the apps' rights it holds says the most.
        let apps = match apps.iter().rposition(|right| right.held_by(self.permissions)) {
            Some(2) => "Starts apps",
            Some(1) => "Watches",
            Some(_) => "Lists the apps",
            None => "No stream",
        };
        let held: Vec<String> = input
            .iter()
            .filter(|right| right.held_by(self.permissions))
            .map(|right| right.name.to_lowercase())
            .collect();
        let input = match held.len() {
            0 => "no input".to_owned(),
            count if count == input.len() => "every input".to_owned(),
            _ => held.join(", "),
        };
        format!("{apps} \u{b7} {input}")
    }
}

/// The devices of Apollo, for as long as this PC is shared beside it.
#[derive(Default)]
pub struct Registered {
    /// The name and the password of Apollo's pages, as they are typed.
    name: String,
    password: String,
    /// Why signing in did not work.
    refused: Option<String>,
    session: Option<Session>,
    /// The device whose page is open, and its name as it is typed there.
    naming: Option<(String, String)>,
    /// The device the owner is asked again about before it is forgotten.
    unpairing: Option<String>,
    /// The PIN a new device shows, and a name for it, as they are typed.
    pin: String,
    pin_name: String,
}

impl Registered {
    /// A sign-in Apollo no longer takes is over.
    fn tend(&mut self) {
        if self.session.as_ref().is_some_and(|session| lock(&session.told).signed_out) {
            self.session = None;
            self.refused = Some("Apollo asks to sign in again.".to_owned());
        }
    }

    /// The devices Apollo is paired with: as its pages list them once the
    /// owner signed in, and until then as `devices`, its file, names them.
    pub fn listed(&mut self, devices: &[Device]) -> Vec<Known> {
        self.tend();
        if let Some(session) = &self.session {
            let told = lock(&session.told);
            if told.heard {
                return told
                    .clients
                    .iter()
                    .map(|client| Known {
                        id: client.id.clone(),
                        name: client.name.clone(),
                        permissions: client.permissions,
                        streaming: client.connected,
                    })
                    .collect();
            }
        }
        devices
            .iter()
            .map(|device| Known {
                id: device.id.clone(),
                name: device.name.clone(),
                permissions: device.permissions,
                streaming: false,
            })
            .collect()
    }

    /// The page of the device known as `key`. `doing` is what it is doing
    /// at this moment. Returns whether there is nothing left to show: the
    /// device was unpaired, here or elsewhere.
    pub fn page(
        &mut self,
        ui: &mut Ui,
        apollo: &Apollo,
        devices: &[Device],
        key: &str,
        doing: Option<&str>,
        icons: &mut BTreeMap<String, ClientIcon>,
    ) -> bool {
        let Some(known) = self.listed(devices).into_iter().find(|known| known.key() == key) else {
            return true;
        };
        let name = if known.name.is_empty() { "A device" } else { &known.name };
        let icon = icons.get(key).copied().unwrap_or_else(|| ClientIcon::guessed(&known.name));

        ui.horizontal(|ui| {
            let (tile, _) = ui.allocate_exact_size(Vec2::splat(46.0), Sense::hover());
            ui.painter().rect_filled(tile, theme::CONTROL_RADIUS, theme::surface());
            icons::client(icon)(ui.painter(), tile.shrink(11.0), theme::sun());
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                widgets::title(ui, name);
                match doing {
                    Some(doing) => ui.label(RichText::new(doing).color(theme::ready())),
                    None => ui.label(widgets::dim("Paired with this PC")),
                };
            });
        });
        ui.add_space(8.0);

        card(ui, "Icon", |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
                for choice in ClientIcon::ALL {
                    if icon_choice(ui, choice, choice == icon) {
                        icons.insert(key.to_owned(), choice);
                    }
                }
            });
        });

        // Its page of Apollo, once signed in: what can be changed.
        let client = self.session.as_ref().and_then(|session| {
            lock(&session.told).clients.iter().find(|client| client.id == known.id).cloned()
        });
        let mut ask = None;
        let mut gone = false;

        if let Some(client) = &client {
            card(ui, "Name", |ui| {
                if self.naming.as_ref().is_none_or(|(id, _)| *id != client.id) {
                    self.naming = Some((client.id.clone(), client.name.clone()));
                }
                let Some((_, typed)) = &mut self.naming else { return };
                ui.horizontal(|ui| {
                    let field = ui.add(field(typed, "Name of the device", 280.0));
                    let entered =
                        field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    let new = typed.trim();
                    let changed = !new.is_empty() && new != client.name;
                    let save = ui.add_enabled(changed, small_button("Save", true)).clicked();
                    if changed && (save || entered) {
                        let mut named = client.clone();
                        named.name = new.to_owned();
                        ask = Some(Ask::Update(named));
                    }
                });
            });
        }

        card(ui, "What it may do", |ui| {
            if let Some(permissions) = rights(ui, known.permissions, client.is_some())
                && let Some(client) = &client
            {
                let mut allowed = client.clone();
                allowed.permissions = permissions;
                ask = Some(Ask::Update(allowed));
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

        match &client {
            Some(client) => card(ui, "This device", |ui| {
                ui.horizontal(|ui| {
                    if self.unpairing.as_deref() == Some(client.id.as_str()) {
                        if ui.add(small_button("Unpair", true)).clicked() {
                            ask = Some(Ask::Unpair(client.id.clone()));
                            self.unpairing = None;
                            gone = true;
                        }
                        if ui.add(small_button("Keep", false)).clicked() {
                            self.unpairing = None;
                        }
                        ui.label(widgets::dim("It will have to be paired again."));
                        return;
                    }
                    if client.connected && ui.add(small_button("End its stream", false)).clicked() {
                        ask = Some(Ask::Disconnect(client.id.clone()));
                    }
                    if ui.add(small_button("Unpair", false)).clicked() {
                        self.unpairing = Some(client.id.clone());
                    }
                });
            }),
            // A Sunshine keeps nothing to change for each device.
            None if known.id.is_empty() => {}
            None => card(ui, "Change it", |ui| {
                self.sign_in(
                    ui,
                    apollo,
                    "To change what this device may do, to rename it, to end its stream or to \
                     unpair it, sign in with the name and the password of Apollo's own page.",
                );
            }),
        }

        if let (Some(ask), Some(session)) = (ask, &self.session) {
            // Shown at once, and put right by what Apollo says next.
            let mut told = lock(&session.told);
            match &ask {
                Ask::Update(client) => {
                    if let Some(known) = told.clients.iter_mut().find(|known| known.id == client.id)
                    {
                        *known = client.clone();
                    }
                }
                Ask::Unpair(id) => told.clients.retain(|client| client.id != *id),
                Ask::Disconnect(_) | Ask::Pair { .. } => {}
            }
            drop(told);
            let _ = session.ask.send(ask);
        }
        self.problem(ui);
        gone
    }

    /// The page that pairs one more device with Apollo, by the PIN the
    /// device shows.
    pub fn pairing(&mut self, ui: &mut Ui, apollo: &Apollo) {
        self.tend();
        widgets::title(ui, "Pair a device");
        ui.add_space(8.0);
        card(ui, "Its PIN", |ui| {
            ui.add(
                egui::Label::new(widgets::dim(
                    "On the device, in Leto, add this PC and ask to pair: it shows a PIN of four \
                     digits. Type it here, with a name to know the device by.",
                ))
                .wrap(),
            );
            ui.add_space(4.0);
            let Some(session) = &self.session else {
                self.sign_in(
                    ui,
                    apollo,
                    "Apollo takes the PIN on its own page: sign in with the name and the \
                     password of that page.",
                );
                return;
            };
            let paired = lock(&session.told).paired;
            ui.horizontal(|ui| {
                self.pin.retain(|digit| digit.is_ascii_digit());
                self.pin.truncate(4);
                ui.add(field(&mut self.pin, "PIN", 80.0));
                ui.add(field(&mut self.pin_name, "Name of the device", 240.0));
                let ready = self.pin.len() == 4 && !self.pin_name.trim().is_empty();
                if ui.add_enabled(ready, small_button("Pair", true)).clicked() {
                    lock(&session.told).paired = None;
                    let _ = session.ask.send(Ask::Pair {
                        pin: std::mem::take(&mut self.pin),
                        name: std::mem::take(&mut self.pin_name).trim().to_owned(),
                    });
                }
            });
            match paired {
                Some(true) => {
                    ui.label(
                        RichText::new("Paired. Its card is on the first screen.")
                            .color(theme::ready()),
                    );
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
        self.problem(ui);
    }

    /// Why the last thing asked of Apollo was not done, when it was not.
    fn problem(&self, ui: &mut Ui) {
        let problem = self.session.as_ref().and_then(|session| lock(&session.told).problem.clone());
        if let Some(problem) = problem {
            ui.add(egui::Label::new(RichText::new(problem).color(theme::problem())).wrap());
        }
    }

    /// Where the owner signs in to Apollo's pages, to change anything.
    fn sign_in(&mut self, ui: &mut Ui, apollo: &Apollo, why: &str) {
        ui.add(
            egui::Label::new(widgets::dim(format!(
                "{why} Apollo makes the change itself, and a browser that was signed in to its \
                 page is signed out."
            )))
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

/// One icon a device can be given: lit when it is the one it has. Returns
/// whether it was chosen.
fn icon_choice(ui: &mut Ui, icon: ClientIcon, chosen: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(92.0, 78.0), Sense::click());
    response.widget_info(|| WidgetInfo::selected(Role::RadioButton, true, chosen, icon.name()));
    let lit = response.hovered() || response.has_focus();
    let (fill, ink, border) = if chosen {
        (theme::sun(), theme::on_sun(), Stroke::NONE)
    } else if lit {
        (theme::surface_hover(), theme::text(), Stroke::new(1.0, theme::sun().gamma_multiply(0.7)))
    } else {
        (theme::ground(), theme::dim(), Stroke::new(1.0, theme::line()))
    };
    let painter = ui.painter();
    painter.rect(rect, theme::CONTROL_RADIUS, fill, border, egui::StrokeKind::Inside);
    let drawn = egui::Rect::from_center_size(rect.center() - vec2(0.0, 10.0), Vec2::splat(26.0));
    icons::client(icon)(painter, drawn, ink);
    painter.text(
        rect.center() + vec2(0.0, 24.0),
        egui::Align2::CENTER_CENTER,
        icon.name(),
        egui::FontId::proportional(theme::SMALL - 0.5),
        ink,
    );
    response.on_hover_cursor(CursorIcon::PointingHand).clicked()
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
