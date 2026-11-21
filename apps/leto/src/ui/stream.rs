//! An app of a PC, streamed: the picture fills the window, and the keyboard
//! and the mouse of this device go to the PC.

use egui::{
    Align, Align2, Color32, CursorIcon, Event, FontId, Id, Key, Layout, Margin, Order, Pos2, Rect,
    RichText, Sense, Stroke, TextFormat, TextureHandle, Ui, UiBuilder, Vec2,
    text::{LayoutJob, TextWrapping},
    vec2,
};
use std::time::Duration;

use leto_core::{engine::Codec, input::Input, model::HostId};

use super::{card::OnCard, frame, icons, widgets};
use crate::{
    backend::{Filming, Picture},
    input::{Controls, Shortcut, hides_its_pointer},
    settings::Settings,
    state::{Cam, Mic, State, StreamView},
    stats::{Counts, Rates},
    theme, window,
};

/// Seconds the bar stays when a stream starts, to say how to get out of it.
const BAR_STAYS: f64 = 6.0;
/// Seconds the bar comes back for when it has something new to say.
const BAR_RETURNS: f64 = 4.0;
/// Seconds the grip that opens the bar stays after the pointer last moved.
const GRIP_STAYS: f32 = 2.5;
/// Resting the pointer here, at the top of the window, opens the bar.
const GRIP_ZONE: Vec2 = Vec2::new(150.0, 16.0);
/// The pointer still counts as over the bar this close to it.
const BAR_MARGIN: f32 = 10.0;

/// A browser fills the screen with F11 on its own, without telling the
/// page: there, F11 is not Leto's to act on.
const F11_FILLS: bool = cfg!(not(target_arch = "wasm32"));
/// A stream fills the screen as soon as it connects, as in Moonlight, unless
/// the person chose otherwise. A browser only lets a page fill the screen
/// right after a click.
const FILLS_ON_CONNECT: bool = cfg!(not(target_arch = "wasm32"));
/// Seconds between two updates of the statistics.
const STATISTICS_EVERY: f64 = 1.0;

/// The quietest sound the microphone's meter shows, in decibels below the
/// loudest a microphone captures.
const METER_FLOOR: f32 = -50.0;

/// A key that went down or came up while a stream had the keyboard.
enum KeyStroke {
    /// As the window heard of it.
    Window {
        /// The key by what the layout prints on it.
        printed: Key,
        /// The key by where it sits, when the system says.
        place: Option<Key>,
        down: bool,
    },
    /// Taken from the system before it acted on it: the Windows key,
    /// Alt+Tab and every other key, which the window never hears of.
    System(window::Stroke),
}

/// What the pictures of the stream are shown on.
// A browser shows them behind the window, and never draws them.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
enum Surface {
    /// The graphics card turns them into pixels as the window draws them,
    /// this large.
    Card(OnCard, [usize; 2]),
    /// The window draws them from pixels.
    Texture(TextureHandle),
    /// The backend shows them behind the window, this large: the window
    /// leaves a hole.
    Behind([usize; 2]),
}

impl Surface {
    /// Width and height of the pictures.
    fn size(&self) -> [usize; 2] {
        match self {
            Self::Texture(texture) => texture.size(),
            Self::Card(_, size) | Self::Behind(size) => *size,
        }
    }
}

/// The stream on screen: its picture, and what the person does to it.
#[derive(Default)]
pub struct Player {
    surface: Option<Surface>,
    /// Where the window lets pictures shown behind it through.
    hole: Option<Rect>,
    /// The stream screen made the window fill the screen, so leaving the
    /// stream gives the window back.
    filled_screen: bool,
    controls: Controls,
    /// Keys taken from the interface before it acted on them.
    strokes: Vec<KeyStroke>,
    /// The mouse turns the view of a game instead of moving a pointer.
    game_mouse: bool,
    /// The pointer is being kept inside the window, for the game mouse.
    pointer_kept: bool,
    /// App time until which the bar stays although nobody asks for it.
    /// `None` until the stream connects.
    bar_until: Option<f64>,
    /// Where the bar was in the last frame, when it was there.
    bar: Option<Rect>,
    /// The microphone as it was in the last frame of this stream, and
    /// whether it was capturing.
    microphone_was: Option<Mic>,
    captured: bool,
    /// The same for the camera, and how it was doing.
    camera_was: Option<Cam>,
    filmed: Option<Filming>,
    /// What the last thing worth a line in the bar was about, when it was
    /// about something this device sends.
    talks_of: Option<Told>,
    /// The keyboard and the mouse are this device's again: the PC hears
    /// nothing of them until the person gives them back.
    released: bool,
    /// Every key was the stream's in the last frame, the system's
    /// shortcuts included where the system lets go of them.
    keyboard_kept: bool,
    /// A key pressed reached the window meanwhile all the same.
    slipped: bool,
    /// Pictures the window was given to show so far.
    shown: u64,
    /// The statistics over the picture, while they show.
    statistics: Option<Statistics>,
    /// The PCs that leave their own pointer out of the pictures, because
    /// Leto asked them to while this device's pointer shows instead. A PC
    /// keeps doing so until asked again, even after a stream that ended
    /// without Leto leaving it, so this outlives the stream.
    hiding_pointer: Vec<HostId>,
    /// The window is closing: the stream is left without closing the app
    /// on the PC, whatever the settings say.
    closing: bool,
}

/// What the statistics show, and what they were worked out from.
struct Statistics {
    /// App time they were last worked out at, what the backend had counted
    /// then and what the window had shown.
    since: f64,
    counts: Counts,
    shown: u64,
    rates: Option<Rates>,
}

/// Where the keyboard and the mouse of this device go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hands {
    /// To the PC, the mouse moving its pointer.
    Pointer,
    /// To the PC, the mouse turning the view of a game.
    GameMouse,
    /// Nowhere: they are this device's.
    Released,
}

/// What of this device the bar has something new to say about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Told {
    Microphone,
    Camera,
}

impl Player {
    /// Shows `picture` from now on, on `card` when the window has one that
    /// works.
    pub fn picture(&mut self, ctx: &egui::Context, picture: Picture, card: Option<&OnCard>) {
        match picture {
            #[cfg(not(target_arch = "wasm32"))]
            Picture::Planes(planes) => {
                let size = [planes.width, planes.height];
                if let Some(card) = card.filter(|card| card.works()) {
                    card.show(planes);
                    self.surface = Some(Surface::Card(card.clone(), size));
                } else {
                    self.pixels(ctx, pixels(&planes));
                }
                self.shown += 1;
            }
            Picture::Behind(size) => self.surface = Some(Surface::Behind(size)),
        }
        // A browser has no card, and draws nothing: the page shows the
        // pictures.
        let _ = (ctx, card);
    }

    /// Shows `pixels` from now on, drawn by the window.
    #[cfg(not(target_arch = "wasm32"))]
    fn pixels(&mut self, ctx: &egui::Context, pixels: egui::ColorImage) {
        if let Some(Surface::Texture(texture)) = &mut self.surface {
            texture.set(pixels, egui::TextureOptions::LINEAR);
        } else {
            let texture = ctx.load_texture("stream", pixels, egui::TextureOptions::LINEAR);
            self.surface = Some(Surface::Texture(texture));
        }
    }

    /// Where the keyboard and the mouse go.
    fn hands(&self) -> Hands {
        match (self.released, self.game_mouse) {
            (true, _) => Hands::Released,
            (false, true) => Hands::GameMouse,
            (false, false) => Hands::Pointer,
        }
    }

    /// Where the backend should show its pictures, when it is the one
    /// showing them: the hole the window leaves, in points.
    pub fn hole(&self) -> Option<Rect> {
        self.hole
    }

    /// Takes the keyboard away from the interface, for the PC: while a
    /// stream is connected, Tab, Enter, Escape and the rest mean something
    /// there, not here.
    pub fn take_keyboard(&mut self, raw: &mut egui::RawInput) {
        raw.events.retain(|event| match event {
            Event::Key { key, physical_key, pressed, repeat, .. } => {
                self.slipped |= self.keyboard_kept && *pressed && !*repeat;
                self.strokes.push(KeyStroke::Window {
                    printed: *key,
                    place: *physical_key,
                    down: *pressed,
                });
                false
            }
            Event::Text(_)
            | Event::Ime(_)
            | Event::Copy
            | Event::Cut
            | Event::Paste(_)
            | Event::PasteImage(_) => false,
            _ => true,
        });
    }

    /// The stream is over: forgets it and gives the window and the pointer
    /// back.
    pub fn clear(&mut self, ctx: &egui::Context) {
        if self.filled_screen {
            window::fill_screen(ctx, false);
        }
        if self.pointer_kept {
            window::keep_pointer(ctx, false);
        }
        window::keep_keyboard(false);
        let hiding_pointer = std::mem::take(&mut self.hiding_pointer);
        *self = Self { hiding_pointer, ..Self::default() };
    }

    /// Leaves the stream at the next frame, because the window is closing.
    pub fn leave_to_close(&mut self) {
        self.closing = true;
    }

    /// Whether the PC of `view` leaves its pointer out of the pictures, as
    /// far as Leto knows.
    fn pc_hides_pointer(&self, view: &StreamView) -> bool {
        self.hiding_pointer.contains(&view.host.id)
    }
}

/// What the backend says of a stream besides its pictures: its sound, and
/// what of this device goes to the PC.
#[derive(Clone, Copy)]
pub struct Status<'a> {
    /// Why the stream has no sound, when it has none.
    pub sound_problem: Option<&'a str>,
    /// Why it has no pictures, when it has none.
    pub picture_problem: Option<&'a str>,
    /// How loud the microphone is, from 0 to 1, while it captures. `None`
    /// with the switch on means it is still opening: a browser asks the
    /// person first.
    pub voice: Option<f32>,
    /// How the camera is doing, while it captures.
    pub film: Option<Filming>,
    /// What happened to the pictures so far, where the backend counts it.
    pub counts: Option<Counts>,
    /// How long a message takes to reach the PC and come back, once
    /// measured.
    pub round_trip: Option<Duration>,
}

/// Draws the stream and returns what the person did since the last frame,
/// to send to the PC in this order.
pub fn show(
    ui: &mut Ui,
    state: &mut State,
    view: &StreamView,
    player: &mut Player,
    settings: &Settings,
    status: Status<'_>,
) -> Vec<Input> {
    let window = ui.max_rect();
    let (now, focused) = ui.input(|input| (input.time, input.focused));
    let fills_screen = window::fills_screen(ui.ctx());
    let closing = std::mem::take(&mut player.closing);
    let mut asked = Asked { leave: closing, ..Asked::default() };
    player.controls.choose(settings.swap_mouse_buttons, settings.reverse_scroll);
    // Once connected, every key is the PC's, the system's shortcuts too
    // where the system lets go of them and the person did not choose
    // otherwise, while the window is in front and until the person takes
    // them back.
    let keep = view.connected
        && !player.released
        && focused
        && settings.capture_shortcuts.takes(fills_screen);
    if std::mem::take(&mut player.slipped) && keep {
        window::slipped();
    }
    window::keep_keyboard(keep);
    player.keyboard_kept = keep;
    player.strokes.extend(window::keys().into_iter().map(KeyStroke::System));

    player.hole = None;
    let picture = player.surface.as_ref().map(|surface| {
        let size = surface.size();
        let place = fit(vec2(size[0] as f32, size[1] as f32), window);
        match surface {
            Surface::Card(card, _) => {
                ui.painter().rect_filled(window, 0, Color32::BLACK);
                ui.painter().add(card.paint(place));
            }
            Surface::Texture(texture) => {
                ui.painter().rect_filled(window, 0, Color32::BLACK);
                egui::Image::new((texture.id(), place.size())).paint_at(ui, place);
            }
            Surface::Behind(_) => {
                // Black around the pictures, and nothing over them.
                for side in around(place, window) {
                    ui.painter().rect_filled(side, 0, Color32::BLACK);
                }
                player.hole = Some(place);
            }
        }
        (place, size)
    });
    if picture.is_none() {
        ui.painter().rect_filled(window, 0, Color32::BLACK);
        asked.leave |= waiting(ui, window, view, status.picture_problem);
    }

    if view.connected {
        if player.bar_until.is_none() {
            // Just connected.
            asked.fill = FILLS_ON_CONNECT && settings.full_screen && !fills_screen;
            if settings.statistics && player.statistics.is_none() {
                player.statistics = Some(Statistics::new(status.counts, player.shown, now));
            }
        }
        let bar_until = *player.bar_until.get_or_insert(now + BAR_STAYS);
        let mut refused = false;

        for stroke in player.strokes.drain(..) {
            let (shortcut, down) = match stroke {
                KeyStroke::Window { printed, place, down } => {
                    (player.controls.key(printed, place, down), down)
                }
                KeyStroke::System(stroke) => (
                    player.controls.stroke(Some(stroke.key), stroke.letter, stroke.down),
                    stroke.down,
                ),
            };
            refused |= down && !view.input.keyboard && !player.released;
            match shortcut {
                Some(Shortcut::Leave) => asked.leave = true,
                Some(Shortcut::FullScreen) => asked.fill = true,
                Some(Shortcut::GameMouse) => asked.game_mouse = true,
                Some(Shortcut::Microphone) => asked.microphone = true,
                Some(Shortcut::Camera) => asked.camera = true,
                Some(Shortcut::Statistics) => asked.statistics = true,
                Some(Shortcut::Release) => asked.release = true,
                None => {}
            }
        }
        if let Some((place, size)) = picture {
            let clicked = mouse(ui, player, place, size);
            if player.released {
                // A click on the picture gives the keyboard and the mouse
                // back to the PC, as in Moonlight. The PC hears nothing of
                // the click itself.
                asked.release |= clicked;
            } else {
                refused |= clicked && !view.input.mouse;
            }
        }
        if !focused && player.controls.holds_anything() {
            // Another window has the keyboard: nothing may stay held down
            // on the PC.
            player.controls.release_all();
        }
        if refused {
            player.bar_until = Some(bar_until.max(now + BAR_RETURNS));
        }
        let capturing = status.voice.is_some();
        if player.microphone_was.as_ref() != Some(&view.microphone) || player.captured != capturing
        {
            // The bar says what the microphone just did. That a PC takes
            // one is not news: its switch is there to see.
            let was_on = player.microphone_was == Some(Mic::On);
            if was_on || matches!(view.microphone, Mic::On | Mic::Failed(_)) {
                player.talks_of = Some(Told::Microphone);
                player.bar_until = Some(bar_until.max(now + BAR_RETURNS));
            }
            player.microphone_was = Some(view.microphone.clone());
            player.captured = capturing;
        }
        if player.camera_was.as_ref() != Some(&view.camera) || player.filmed != status.film {
            // And what the camera just did.
            let was_on = player.camera_was == Some(Cam::On);
            if was_on || matches!(view.camera, Cam::On | Cam::Failed(_)) {
                player.talks_of = Some(Told::Camera);
                player.bar_until =
                    Some(player.bar_until.unwrap_or(bar_until).max(now + BAR_RETURNS));
            }
            player.camera_was = Some(view.camera.clone());
            player.filmed = status.film;
        }
        if view.microphone == Mic::On {
            // The meter moves, and a microphone that stops is noticed, even
            // while the picture stands still. Once a second is enough for a
            // window that is only glanced at.
            ui.ctx().request_repaint_after_secs(if focused { 0.05 } else { 1.0 });
        }
    } else {
        // Nothing to play with yet: the keys are still Leto's.
        player.strokes.clear();
        let (escape, f11) =
            ui.input(|input| (input.key_pressed(Key::Escape), input.key_pressed(Key::F11)));
        asked.fill |= f11 && F11_FILLS;
        // The way out of the full screen first, then out of the stream.
        if escape {
            if fills_screen { asked.fill = true } else { asked.leave = true }
        }
    }

    overlay(ui, window, view, player, status, now, &mut asked);
    if view.connected
        && let Some(statistics) = &mut player.statistics
    {
        statistics.update(status.counts, player.shown, now);
        statistics_panel(ui.ctx(), statistics, status);
        // The numbers change even while the picture stands still.
        ui.ctx().request_repaint_after_secs(STATISTICS_EVERY as f32 / 2.0);
    }

    if asked.microphone {
        state.switch_microphone();
    }
    if asked.camera {
        state.switch_camera();
    }
    if asked.game_mouse {
        player.game_mouse = !player.game_mouse;
        // The bar says what just changed, and how to change it back.
        player.bar_until = Some(now + BAR_RETURNS);
        player.talks_of = None;
    }
    if asked.release {
        player.released = !player.released;
        // Whatever the PC held, it lets go of.
        player.controls.mute(player.released);
        player.bar_until = Some(now + BAR_RETURNS);
        player.talks_of = None;
    }
    if asked.statistics {
        player.statistics = match player.statistics {
            Some(_) => None,
            None => Some(Statistics::new(status.counts, player.shown, now)),
        };
    }
    if player.pointer_kept && window::pointer_lost() {
        // A browser gives the pointer back when Escape is pressed, whoever
        // asked for it: the game mouse is over.
        player.pointer_kept = false;
        player.game_mouse = false;
        player.bar_until = Some(now + BAR_RETURNS);
        player.talks_of = None;
    }
    let keep = player.game_mouse && view.connected && focused && !asked.leave && !player.released;
    if keep != player.pointer_kept {
        player.pointer_kept = keep;
        window::keep_pointer(ui.ctx(), keep);
    }
    if asked.fill {
        player.filled_screen = !fills_screen;
        window::fill_screen(ui.ctx(), !fills_screen);
    }
    // This device's pointer shows on the picture instead of the PC's, where
    // the PC can leave its own out and hears both keyboard and mouse. The
    // PC draws its own again for the game mouse, which has no pointer here,
    // and before the stream is left.
    let hide = settings.local_pointer
        && hides_its_pointer(view.host.kind)
        && view.input.keyboard
        && view.input.mouse
        && !player.game_mouse
        && !asked.leave;
    if view.connected && hide != player.pc_hides_pointer(view) {
        player.controls.switch_pc_pointer();
        if hide {
            player.hiding_pointer.push(view.host.id.clone());
        } else {
            player.hiding_pointer.retain(|host| *host != view.host.id);
        }
    }
    if asked.leave {
        player.controls.release_all();
        state.leave_stream(settings.quit_on_leave && !closing);
    }
    player.controls.take()
}

/// What the person asked of Leto itself in this frame.
#[derive(Default)]
struct Asked {
    leave: bool,
    /// Fill the screen, or stop filling it.
    fill: bool,
    /// Switch between the pointer and the game mouse.
    game_mouse: bool,
    /// Turn the microphone on, or off again.
    microphone: bool,
    /// Turn the camera on, or off again.
    camera: bool,
    /// Show the statistics, or stop showing them.
    statistics: bool,
    /// Give the keyboard and the mouse back to this device, or to the PC.
    release: bool,
}

/// Passes what the mouse did on to the controls. Returns whether a button
/// was pressed on the picture.
fn mouse(ui: &Ui, player: &mut Player, place: Rect, size: [usize; 2]) -> bool {
    let game_mouse = player.game_mouse;
    // What happens on the bar is for the bar.
    let bar = player.bar.filter(|_| !game_mouse).map(|bar| bar.expand(BAR_MARGIN));
    let on_bar = |pointer: Pos2| bar.is_some_and(|bar| bar.contains(pointer));
    let controls = &mut player.controls;
    let mut clicked = false;

    if ui.ctx().current_pass_index() > 0 {
        // egui is drawing this frame a second time, with the same events:
        // the PC has heard of them already.
        return false;
    }

    ui.input(|input| {
        let mut pointer = input.pointer.latest_pos();
        for event in &input.events {
            match event {
                Event::PointerMoved(at) => {
                    pointer = Some(*at);
                    if !game_mouse && !on_bar(*at) {
                        controls.pointer_at(*at, place, size);
                    }
                }
                Event::MouseMoved(delta) if game_mouse => controls.moved(*delta),
                Event::PointerButton { pos, button, pressed: true, .. } => {
                    if on_bar(*pos) {
                        continue;
                    }
                    if !game_mouse {
                        // The click lands where the pointer is, even if the
                        // PC has not heard of it moving there.
                        controls.pointer_at(*pos, place, size);
                    }
                    controls.button(*button, true);
                    clicked = true;
                }
                Event::PointerButton { button, pressed: false, .. } => {
                    controls.button(*button, false);
                }
                Event::MouseWheel { unit, delta, .. } if !pointer.is_some_and(on_bar) => {
                    controls.wheel(*unit, *delta);
                }
                _ => {}
            }
        }
    });
    clicked
}

/// Everything drawn over the picture: the bar, the grip that opens it, and
/// the pointer of this device, or its absence.
fn overlay(
    ui: &Ui,
    window: Rect,
    view: &StreamView,
    player: &mut Player,
    status: Status<'_>,
    now: f64,
    asked: &mut Asked,
) {
    let has_picture = player.surface.is_some();
    let (idle, pointer) =
        ui.input(|input| (input.pointer.time_since_last_movement(), input.pointer.hover_pos()));
    // With the game mouse the pointer is hidden and going nowhere: it opens
    // nothing.
    let game_mouse = player.hands() == Hands::GameMouse;
    let pointing = pointer.filter(|_| !game_mouse);
    let grip_zone =
        Rect::from_center_size(window.center_top() + vec2(0.0, GRIP_ZONE.y / 2.0), GRIP_ZONE);
    let on_bar = pointing.is_some_and(|pointer| {
        grip_zone.contains(pointer)
            || player.bar.is_some_and(|bar| bar.expand(BAR_MARGIN).contains(pointer))
    });
    let timed = player.bar_until.is_some_and(|until| now < until);
    if !timed {
        // What the bar said about the microphone or the camera has been
        // said.
        player.talks_of = None;
    }
    let wanted = !has_picture || timed || on_bar;
    let opacity = ui.ctx().animate_bool_with_time(Id::unique("stream-bar"), wanted, 0.2);

    if opacity > 0.0 {
        let content = Bar {
            view,
            picture: player.surface.as_ref().map(Surface::size),
            sound_problem: status.sound_problem,
            game_mouse: player.game_mouse && !player.released,
            voice: status.voice.map(meter),
            film: status.film,
            windowed: frame::OWN && !window::fills_screen(ui.ctx()),
            last: player.bar,
            note: bar_note(
                view,
                player.hands(),
                timed,
                player.talks_of.map(|told| match told {
                    Told::Microphone => News::Microphone(status.voice.is_some()),
                    Told::Camera => News::Camera(status.film),
                }),
            ),
            opacity,
        };
        player.bar = Some(bar(ui.ctx(), window, &content, asked));
        if timed {
            // Come back to put the bar away even if no picture arrives.
            ui.ctx().request_repaint_after_secs(0.25);
        }
    } else {
        player.bar = None;
    }

    // The PC hears this device, or sees it: that shows for as long as it
    // does, whether the bar is there to say so or not.
    let mut signs = 0;
    if let (Mic::On, Some(level)) = (&view.microphone, status.voice)
        && opacity < 1.0
    {
        let level = meter(level);
        live(ui, window, signs, 1.0 - opacity, |painter, rect, color| {
            icons::microphone(painter, rect, color, level);
        });
        signs += 1;
    }
    if let (Cam::On, Some(Filming::Sending(_))) = (&view.camera, status.film)
        && opacity < 1.0
    {
        live(ui, window, signs, 1.0 - opacity, icons::camera);
    }
    if !view.connected || !has_picture {
        return;
    }
    let pc_draws_pointer = !player.pc_hides_pointer(view);
    if game_mouse || (pointer.is_some() && !on_bar && !player.released && pc_draws_pointer) {
        // The PC draws its own pointer in the picture, or the game mouse
        // has none.
        ui.ctx().set_cursor_icon(CursorIcon::None);
    }
    if pointing.is_some() && opacity == 0.0 && idle < GRIP_STAYS {
        grip(ui, window);
        ui.ctx().request_repaint_after_secs(0.25);
    }
}

impl Statistics {
    fn new(counts: Option<Counts>, shown: u64, now: f64) -> Self {
        Self { since: now, counts: counts.unwrap_or_default(), shown, rates: None }
    }

    /// Works the rates out again, once a second.
    fn update(&mut self, counts: Option<Counts>, shown: u64, now: f64) {
        let Some(counts) = counts else { return };
        if now - self.since < STATISTICS_EVERY {
            return;
        }
        let shown_since = shown.saturating_sub(self.shown);
        self.rates = Rates::between(&self.counts, &counts, shown_since, now - self.since);
        (self.since, self.counts, self.shown) = (now, counts, shown);
    }
}

/// What decodes the pictures, for the statistics.
/// Who decodes the pictures, when the backend does not say.
const DECODER: &str = if cfg!(target_arch = "wasm32") { "The browser's" } else { "Not yet known" };

/// The statistics over the picture, in a corner, as in Moonlight.
fn statistics_panel(ctx: &egui::Context, statistics: &Statistics, status: Status<'_>) {
    let lines = statistics_lines(statistics, status.counts, status.round_trip);
    egui::Area::new(Id::unique("stream-statistics"))
        .order(Order::Foreground)
        .anchor(Align2::LEFT_TOP, vec2(10.0, 10.0))
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(Color32::from_black_alpha(200))
                .stroke(Stroke::new(1.0, theme::LINE))
                .corner_radius(theme::CARD_RADIUS)
                .inner_margin(Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    egui::Grid::new(Id::unique("stream-statistics-lines"))
                        .num_columns(2)
                        .spacing(vec2(14.0, 3.0))
                        .show(ui, |ui| {
                            for (name, value) in lines {
                                ui.label(widgets::dim(name).size(theme::SMALL));
                                ui.label(
                                    RichText::new(value).color(theme::MOON).size(theme::SMALL),
                                );
                                ui.end_row();
                            }
                        });
                    ui.add_space(4.0);
                    let hide = format!("{} hides these.", Shortcut::Statistics.keys());
                    ui.label(widgets::dim(hide).size(theme::SMALL - 1.0));
                });
        });
}

/// The lines of the statistics: a name, and what it is at.
fn statistics_lines(
    statistics: &Statistics,
    counts: Option<Counts>,
    round_trip: Option<Duration>,
) -> Vec<(&'static str, String)> {
    let Some(counts) = counts else {
        return vec![("Stream", "Leto does not count its pictures here.".to_owned())];
    };
    let mut lines = Vec::new();
    if let Some([width, height, fps]) = counts.sent {
        let codec = counts.codec.unwrap_or(Codec::H264).name();
        lines.push(("Video", format!("{width} × {height}, {fps} per second, {codec}")));
    }
    let decoder = match counts.on_graphics_card {
        Some(true) => "On the graphics card",
        Some(false) => "In software, on the processor",
        None => DECODER,
    };
    lines.push(("Decoder", decoder.to_owned()));
    let Some(rates) = statistics.rates else {
        lines.push(("Arrived", "Counting…".to_owned()));
        return lines;
    };
    let per_second = |rate: f64| format!("{rate:.1} per second");
    lines
        .push(("Arrived", format!("{}, {:.1} Mbit/s", per_second(rates.received), rates.megabits)));
    if let Some((share, lost)) = rates.lost {
        lines.push(("Lost on the way", format!("{lost} in all, {share:.1} % lately")));
    }
    lines.push((
        "Decoded",
        match rates.decoding_ms {
            Some(took) => format!("{}, {took:.1} ms each", per_second(rates.decoded)),
            None => per_second(rates.decoded),
        },
    ));
    lines.push(("Shown", per_second(rates.shown)));
    if let Some(took) = rates.host_ms {
        lines.push(("On the PC", format!("{took:.1} ms to capture and encode")));
    }
    if let Some(round_trip) = round_trip {
        lines.push(("Network", format!("{} ms there and back", round_trip.as_millis())));
    }
    lines
}

/// A small handle at the top of the picture: where to rest the pointer to
/// get the bar.
fn grip(ui: &Ui, window: Rect) {
    let rect = Rect::from_center_size(window.center_top() + vec2(0.0, 7.0), vec2(44.0, 5.0));
    let painter = ui.painter();
    painter.rect_filled(rect.expand(1.0), 4, Color32::from_black_alpha(120));
    painter.rect_filled(rect, 3, theme::MOON.gamma_multiply(0.55));
}

/// A small sign in a corner of the picture that the PC is listening, or
/// looking: the microphone, filled with what it hears, or the camera. Signs
/// stand side by side, `place` counting them from the corner.
fn live(
    ui: &Ui,
    window: Rect,
    place: usize,
    opacity: f32,
    icon: impl Fn(&egui::Painter, Rect, Color32),
) {
    let center = window.right_top() + vec2(-28.0 - 40.0 * place as f32, 28.0);
    let rect = Rect::from_center_size(center, Vec2::splat(32.0));
    let painter =
        ui.ctx().layer_painter(egui::LayerId::new(Order::Foreground, Id::unique("stream-live")));
    let ground = Color32::from_black_alpha(170).gamma_multiply(opacity);
    painter.circle_filled(rect.center(), rect.width() / 2.0, ground);
    icon(&painter, rect.shrink(8.0), theme::SUN.gamma_multiply(opacity));
}

/// How full the microphone's meter is for a level from 0 to 1. Ears hear in
/// ratios, so the meter does too: each halving of the level takes the same
/// slice off it.
pub(super) fn meter(level: f32) -> f32 {
    if level <= 0.0 {
        return 0.0;
    }
    let decibels = 20.0 * level.log10();
    ((decibels - METER_FLOOR) / -METER_FLOOR).clamp(0.0, 1.0)
}

/// What is shown until the first picture, or why none comes. Returns true
/// when the person gives up waiting.
fn waiting(ui: &mut Ui, window: Rect, view: &StreamView, problem: Option<&str>) -> bool {
    let text = if view.connected {
        "Waiting for the first picture…".to_owned()
    } else {
        format!("Starting {} on {}…", view.app.title, view.host.name)
    };
    let area = Rect::from_center_size(window.center(), vec2(window.width().min(420.0), 120.0));
    let layout = Layout::top_down(Align::Center);
    ui.scope_builder(UiBuilder::new().max_rect(area).layout(layout), |ui| {
        if let Some(problem) = problem {
            let text = RichText::new(problem).color(theme::PROBLEM);
            ui.add(egui::Label::new(text).wrap().halign(Align::Center));
            ui.add_space(6.0);
            return widgets::quiet_button(ui, "Leave").clicked();
        }
        ui.add(egui::Spinner::new().size(26.0).color(theme::SUN));
        ui.add_space(6.0);
        ui.add(egui::Label::new(widgets::dim(text)).wrap().halign(Align::Center));
        ui.add_space(6.0);
        widgets::quiet_button(ui, "Cancel").clicked()
    })
    .inner
}

/// What the bar shows.
struct Bar<'a> {
    view: &'a StreamView,
    picture: Option<[usize; 2]>,
    sound_problem: Option<&'a str>,
    game_mouse: bool,
    /// How full the microphone's meter is, while it captures.
    voice: Option<f32>,
    /// How the camera is doing, while it captures.
    film: Option<Filming>,
    /// The stream is in a window whose frame Leto draws: the bar moves it,
    /// and has its buttons.
    windowed: bool,
    /// Where the bar was in the frame before, if it showed.
    last: Option<Rect>,
    note: Option<Note>,
    opacity: f32,
}

/// A line under the buttons of the bar.
#[derive(Debug, PartialEq, Eq)]
struct Note {
    text: String,
    /// Something does not work, as opposed to something worth knowing.
    problem: bool,
}

/// The bar floating over the top of the picture. Returns where it is.
fn bar(ctx: &egui::Context, window: Rect, content: &Bar<'_>, asked: &mut Asked) -> Rect {
    let narrow = window.width() < 560.0;
    // The hidden pointer of the game mouse must not press anything.
    let usable = !content.game_mouse;
    egui::Area::new(Id::unique("stream-bar-area"))
        .order(Order::Foreground)
        .anchor(Align2::CENTER_TOP, vec2(0.0, 10.0))
        .interactable(usable)
        .show(ctx, |ui| {
            ui.set_opacity(content.opacity);
            let windowed = usable && content.windowed;
            if let Some(last) = content.last.filter(|_| windowed) {
                // Under what the bar holds, which stays pressable.
                frame::drag_by(ui, last, Id::unique("stream-bar-drag"));
            }
            egui::Frame::new()
                .fill(Color32::from_black_alpha(225))
                .stroke(Stroke::new(1.0, theme::LINE))
                .corner_radius(theme::CARD_RADIUS)
                .inner_margin(Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = vec2(8.0, 4.0);
                    let row = ui
                        .horizontal(|ui| {
                            if usable {
                                asked.leave |= widgets::back_button(ui, "Apps").clicked();
                            }
                            // Room for the names once everything else has its own.
                            let has_camera = content.view.camera != Cam::Unknown;
                            let others = if narrow { 210.0 } else { 330.0 }
                                + if has_camera { 44.0 } else { 0.0 }
                                + if windowed { 3.0 * 36.0 + 17.0 } else { 0.0 };
                            names(ui, content.view, (window.width() - others).clamp(80.0, 360.0));
                            if let Some([width, height]) = content.picture.filter(|_| !narrow) {
                                let size = format!("{width} × {height}");
                                ui.label(widgets::dim(size).size(theme::SMALL));
                            }
                            if let Some(problem) = content.sound_problem {
                                let text = RichText::new("No sound").color(theme::PROBLEM);
                                ui.label(text.size(theme::SMALL)).on_hover_text(problem);
                            }
                            if usable && content.view.connected {
                                asked.microphone |=
                                    microphone_switch(ui, content.view, content.voice);
                                asked.camera |= camera_switch(ui, content.view, content.film);
                            }
                            if usable {
                                let mouse = format!(
                                    "Game mouse: the mouse turns the view instead of moving a \
                                     pointer ({})",
                                    Shortcut::GameMouse.keys()
                                );
                                let switch = widgets::icon_button(ui, icons::mouse, &mouse);
                                asked.game_mouse |= switch.clicked() && content.view.connected;
                                let fill = if content.view.connected {
                                    format!("Full screen ({})", Shortcut::FullScreen.keys())
                                } else if F11_FILLS {
                                    "Full screen (F11)".to_owned()
                                } else {
                                    "Full screen".to_owned()
                                };
                                asked.fill |=
                                    widgets::icon_button(ui, icons::expand, &fill).clicked();
                            }
                            if windowed {
                                ui.separator();
                                frame::buttons(ui, Vec2::splat(36.0), theme::CONTROL_RADIUS);
                            }
                        })
                        .response
                        .rect;
                    if let Some(note) = &content.note {
                        // As wide as the buttons, on as many lines as it takes.
                        ui.set_max_width(row.width().max(260.0_f32.min(window.width() - 44.0)));
                        let color = if note.problem { theme::PROBLEM } else { theme::DIM };
                        let text = RichText::new(&note.text).color(color).size(theme::SMALL);
                        ui.add(egui::Label::new(text).wrap());
                        ui.add_space(2.0);
                    }
                });
        })
        .response
        .rect
}

/// The switch of the microphone, which also shows what the microphone
/// hears. Returns whether the person pressed it.
fn microphone_switch(ui: &mut Ui, view: &StreamView, voice: Option<f32>) -> bool {
    let host = &view.host.name;
    let keys = Shortcut::Microphone.keys();
    let (name, color, level) = match &view.microphone {
        Mic::NotTaken => (
            format!("{host} takes no microphone. Apollo and Sunshine need a build that does."),
            theme::LINE,
            0.0,
        ),
        Mic::Off => (format!("Microphone: let {host} hear this device ({keys})"), theme::DIM, 0.0),
        Mic::On => match voice {
            Some(level) => (
                format!("Microphone on: {host} hears this device ({keys} turns it off)"),
                theme::SUN,
                level,
            ),
            None => (
                format!("Opening the microphone: {host} hears nothing yet ({keys} turns it off)"),
                theme::DIM,
                0.0,
            ),
        },
        Mic::Failed(why) => (format!("{why} Press to try again."), theme::PROBLEM, 0.0),
    };
    let usable = view.microphone != Mic::NotTaken;
    let icon = |painter: &egui::Painter, rect: Rect, color: Color32| {
        icons::microphone(painter, rect, color, level);
    };
    widgets::state_button(ui, icon, &name, color, usable).clicked()
}

/// The switch of the camera, which also shows whether the PC sees this
/// device. Returns whether the person pressed it. There is none until the
/// PC said whether it takes a camera.
fn camera_switch(ui: &mut Ui, view: &StreamView, film: Option<Filming>) -> bool {
    let host = &view.host.name;
    let keys = Shortcut::Camera.keys();
    let (name, color) = match (&view.camera, film) {
        (Cam::Unknown, _) => return false,
        (Cam::NotTaken, _) => (
            format!(
                "{host} takes no camera. No Apollo or Sunshine does: it takes Leto on the PC, \
                 which comes later."
            ),
            theme::LINE,
        ),
        (Cam::Off, _) => (format!("Camera: let {host} see this device ({keys})"), theme::DIM),
        (Cam::On, Some(Filming::Sending([width, height]))) => (
            format!("Camera on: {host} sees this device, {width} × {height} ({keys} turns it off)"),
            theme::SUN,
        ),
        (Cam::On, _) => (
            format!("Opening the camera: {host} sees nothing yet ({keys} turns it off)"),
            theme::DIM,
        ),
        (Cam::Failed(why), _) => (format!("{why} Press to try again."), theme::PROBLEM),
    };
    let usable = view.camera != Cam::NotTaken;
    widgets::state_button(ui, icons::camera, &name, color, usable).clicked()
}

/// What the bar has news about, and how that is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum News {
    /// The microphone, and whether it captures.
    Microphone(bool),
    /// The camera, and how it is doing while it captures.
    Camera(Option<Filming>),
}

/// The app and the PC on one line, cut short if the window is narrow.
fn names(ui: &mut Ui, view: &StreamView, max_width: f32) {
    let font = FontId::proportional(theme::BODY);
    let mut job = LayoutJob::default();
    job.append(&view.app.title, 0.0, TextFormat::simple(font.clone(), theme::MOON));
    job.append(&format!("  ·  {}", view.host.name), 0.0, TextFormat::simple(font, theme::DIM));
    job.wrap = TextWrapping {
        max_width,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = ui.painter().layout_job(job);
    let (rect, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
    ui.painter().galley(rect.min, galley, theme::MOON);
}

/// The line under the buttons of the bar, when there is something to say:
/// not every time the pointer passes. `timed` says the bar is there on its
/// own, because something changed. `news` is set when the change was the
/// microphone's or the camera's.
fn bar_note(view: &StreamView, hands: Hands, timed: bool, news: Option<News>) -> Option<Note> {
    let host = &view.host.name;
    if view.connected && !(view.input.keyboard && view.input.mouse) && hands != Hands::Released {
        // What does not work stays said for as long as the bar shows.
        return Some(note(view, hands));
    }
    // So does what failed: the one that just did first.
    let failed = match (&view.microphone, &view.camera, news) {
        (_, Cam::Failed(why), Some(News::Camera(_))) => Some(why),
        (Mic::Failed(why), _, _) | (_, Cam::Failed(why), _) => Some(why),
        _ => None,
    };
    if let Some(why) = failed {
        return Some(Note { text: why.clone(), problem: true });
    }
    let said = |text: String| Some(Note { text, problem: false });
    match news.filter(|_| timed) {
        Some(News::Microphone(captures)) => match (&view.microphone, captures) {
            (Mic::On, true) => {
                return said(format!(
                    "The microphone is on: {host} hears this device. {} turns it off.",
                    Shortcut::Microphone.keys()
                ));
            }
            (Mic::On, false) => {
                return said(format!("Opening the microphone: {host} hears nothing yet."));
            }
            (Mic::Off, _) => {
                return said(format!("The microphone is off: {host} no longer hears this device."));
            }
            _ => {}
        },
        Some(News::Camera(film)) => match (&view.camera, film) {
            (Cam::On, Some(Filming::Sending(_))) => {
                return said(format!(
                    "The camera is on: {host} sees this device. {} turns it off.",
                    Shortcut::Camera.keys()
                ));
            }
            (Cam::On, _) => {
                return said(format!("Opening the camera: {host} sees nothing yet."));
            }
            (Cam::Off, _) => {
                return said(format!("The camera is off: {host} no longer sees this device."));
            }
            _ => {}
        },
        None => {}
    }
    timed.then(|| note(view, hands))
}

/// What the bar has to say about playing on this PC.
fn note(view: &StreamView, hands: Hands) -> Note {
    let host = &view.host.name;
    let allowed = view.input;
    let apollo = "on Apollo's PIN page, under Device Management";
    if hands == Hands::Released {
        let text = format!(
            "Keyboard and mouse are this device's: {host} hears nothing of them. {} or a click \
             on the picture gives them back.",
            Shortcut::Release.keys()
        );
        return Note { text, problem: false };
    }
    let text = match (allowed.keyboard, allowed.mouse) {
        (false, false) => {
            let names = if allowed.gamepad {
                "\"Keyboard Input\" and \"Mouse Input\""
            } else {
                "\"Keyboard Input\", \"Mouse Input\" and \"Controller Input\""
            };
            format!("{host} lets this device watch only. To play, allow {names} for it {apollo}.")
        }
        (false, true) => format!(
            "{host} ignores the keyboard of this device. To type, allow \"Keyboard Input\" for \
             it {apollo}."
        ),
        (true, false) => format!(
            "{host} ignores the mouse of this device. To use it, allow \"Mouse Input\" for it \
             {apollo}."
        ),
        (true, true) => {
            let mut text = if hands == Hands::GameMouse {
                format!(
                    "Game mouse on: the mouse turns the view. {} gives the pointer back.",
                    Shortcut::GameMouse.keys()
                )
            } else {
                format!(
                    "Keyboard and mouse now go to {host}. {} gives them back, {} leaves.",
                    Shortcut::Release.keys(),
                    Shortcut::Leave.keys()
                )
            };
            if !allowed.gamepad {
                text.push_str(&format!(
                    " Gamepads are ignored until you allow \"Controller Input\" for this device \
                     {apollo}."
                ));
            }
            text
        }
    };
    Note { text, problem: !(allowed.keyboard && allowed.mouse) }
}

/// `planes` made into pixels on the processor, for a window without a card
/// that draws them.
#[cfg(not(target_arch = "wasm32"))]
fn pixels(planes: &leto_media::Planes) -> egui::ColorImage {
    let mut picture = leto_media::Picture::default();
    if !planes.to_rgba(&mut picture) {
        // Planes the decoder made are never too small for their size.
        log::debug!("a picture too small for its size was left out");
    }
    // Pictures are opaque, so plain and premultiplied are the same.
    egui::ColorImage::from_rgba_premultiplied([picture.width, picture.height], &picture.rgba)
}

/// Where a picture of size `picture` goes in `window`: as large as fits,
/// undistorted and centred, with black around it.
fn fit(picture: Vec2, window: Rect) -> Rect {
    if picture.x <= 0.0 || picture.y <= 0.0 {
        return window;
    }
    let scale = (window.width() / picture.x).min(window.height() / picture.y);
    Rect::from_center_size(window.center(), picture * scale)
}

/// What is left of `window` around `place`: above, below, left and right.
fn around(place: Rect, window: Rect) -> [Rect; 4] {
    [
        Rect::from_min_max(window.min, Pos2::new(window.max.x, place.min.y)),
        Rect::from_min_max(Pos2::new(window.min.x, place.max.y), window.max),
        Rect::from_min_max(Pos2::new(window.min.x, place.min.y), place.left_bottom()),
        Rect::from_min_max(place.right_top(), Pos2::new(window.max.x, place.max.y)),
    ]
}

#[cfg(test)]
mod tests {
    use egui::pos2;
    use leto_core::{
        input::{InputAllowed, Key as PcKey},
        model::{AppSummary, HostId, HostKind, HostSummary, Reachability},
    };

    use super::*;
    use crate::settings::Capture;

    #[test]
    fn a_key_pressed_that_reaches_the_window_while_the_stream_keeps_them_slipped_past() {
        let press = |pressed, repeat| Event::Key {
            key: egui::Key::A,
            physical_key: Some(egui::Key::Q),
            pressed,
            repeat,
            modifiers: egui::Modifiers::NONE,
        };
        let mut player = Player::default();
        let mut raw = egui::RawInput { events: vec![press(true, false)], ..Default::default() };
        player.take_keyboard(&mut raw);
        assert!(!player.slipped, "the stream did not keep the keys");

        player.keyboard_kept = true;
        // What was held before, and comes up or repeats here, is no news.
        let mut raw = egui::RawInput {
            events: vec![press(true, true), press(false, false)],
            ..Default::default()
        };
        player.take_keyboard(&mut raw);
        assert!(!player.slipped);
        let mut raw = egui::RawInput { events: vec![press(true, false)], ..Default::default() };
        player.take_keyboard(&mut raw);
        assert!(player.slipped);
        // Every key still goes on to the PC.
        assert_eq!(player.strokes.len(), 4);
        assert!(raw.events.is_empty());
    }

    #[test]
    fn a_picture_of_the_windows_shape_fills_it() {
        let window = Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 540.0));
        assert_eq!(fit(vec2(1920.0, 1080.0), window), window);
    }

    #[test]
    fn a_picture_of_another_shape_is_centred_with_black_around_it() {
        // A tall window: black above and below.
        let tall = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 900.0));
        let place = fit(vec2(1920.0, 1080.0), tall);
        assert_eq!(place.size(), vec2(800.0, 450.0));
        assert_eq!(place.center(), tall.center());

        // A wide window: black left and right.
        let wide = Rect::from_min_size(pos2(0.0, 0.0), vec2(2000.0, 540.0));
        let place = fit(vec2(1920.0, 1080.0), wide);
        assert_eq!(place.size(), vec2(960.0, 540.0));
        assert_eq!(place.center(), wide.center());
    }

    #[test]
    fn a_picture_with_no_size_is_no_reason_to_divide_by_zero() {
        let window = Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 540.0));
        assert_eq!(fit(vec2(0.0, 0.0), window), window);
    }

    fn view(input: InputAllowed) -> StreamView {
        StreamView {
            host: HostSummary {
                id: HostId("salon".into()),
                name: "Salon".into(),
                address: "192.168.1.20".into(),
                port: 47989,
                kind: HostKind::Apollo,
                paired: true,
                reachability: Reachability::Online,
                busy: true,
                running_app: Some(1),
            },
            app: AppSummary { id: 1, title: "Desktop".into(), hdr: false },
            attempt: 1,
            connected: true,
            input,
            microphone: Mic::Off,
            camera: Cam::Off,
        }
    }

    /// Settings that leave the window and the system's keys alone.
    fn quiet(local_pointer: bool) -> Settings {
        Settings {
            local_pointer,
            full_screen: false,
            capture_shortcuts: Capture::Never,
            ..Settings::default()
        }
    }

    /// Draws one frame of the stream screen with the pointer at `pointer`,
    /// and returns what the PC hears and the pointer this device shows.
    fn frame(
        ctx: &egui::Context,
        view: &StreamView,
        player: &mut Player,
        settings: &Settings,
        pointer: Option<Pos2>,
    ) -> (Vec<Input>, CursorIcon) {
        let mut state = State::new();
        let status = Status {
            sound_problem: None,
            picture_problem: None,
            voice: None,
            film: None,
            counts: None,
            round_trip: None,
        };
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(960.0, 540.0))),
            events: pointer.map(Event::PointerMoved).into_iter().collect(),
            ..Default::default()
        };
        let mut heard = Vec::new();
        let mut output = ctx.run_ui(input, |ui| {
            heard = show(ui, &mut state, view, player, settings, status);
        });
        output.textures_delta.clear();
        (heard, output.platform_output.cursor_icon)
    }

    /// How many times the PC was asked to switch its pointer.
    fn switches(heard: &[Input]) -> usize {
        let n = PcKey::letter('N');
        heard
            .iter()
            .filter(|input| matches!(input, Input::Key { key, down: true, .. } if Some(*key) == n))
            .count()
    }

    #[test]
    fn the_pc_hides_its_pointer_while_this_devices_pointer_shows() {
        let ctx = egui::Context::default();
        let settings = quiet(true);
        let view = view(InputAllowed::ALL);
        let mut player = Player::default();
        player.picture(&ctx, Picture::Behind([1920, 1080]), None);
        let middle = Some(pos2(480.0, 270.0));

        // Connected: the PC is asked once, and this device's pointer shows
        // from the next frame.
        assert_eq!(switches(&frame(&ctx, &view, &mut player, &settings, middle).0), 1);
        assert!(player.pc_hides_pointer(&view));
        let (heard, shown) = frame(&ctx, &view, &mut player, &settings, middle);
        assert_eq!(switches(&heard), 0);
        assert_eq!(shown, CursorIcon::Default);
        assert!(
            heard.iter().any(|input| matches!(input, Input::MousePosition { .. })),
            "the PC still hears where the pointer goes: {heard:?}"
        );

        // The game mouse has no pointer here: the PC draws its own again,
        // and hides it once more when the pointer is back.
        player.game_mouse = true;
        let (heard, shown) = frame(&ctx, &view, &mut player, &settings, middle);
        assert_eq!(switches(&heard), 1);
        assert_eq!(shown, CursorIcon::None);
        player.game_mouse = false;
        assert_eq!(switches(&frame(&ctx, &view, &mut player, &settings, middle).0), 1);

        // A stream that ended without leaving it left the PC hiding its
        // pointer: the next stream to that PC does not switch it back on.
        player.clear(&ctx);
        assert_eq!(switches(&frame(&ctx, &view, &mut player, &settings, None).0), 0);

        // Leaving, here because the window closes, has the PC draw it again.
        player.leave_to_close();
        assert_eq!(switches(&frame(&ctx, &view, &mut player, &settings, None).0), 1);
        assert!(!player.pc_hides_pointer(&view));
    }

    #[test]
    fn the_pc_draws_its_pointer_where_it_cannot_leave_it_out_or_nobody_wants_it_to() {
        let ctx = egui::Context::default();
        let no_keyboard = InputAllowed { keyboard: false, ..InputAllowed::ALL };
        let no_mouse = InputAllowed { mouse: false, ..InputAllowed::ALL };
        for (kind, input, local_pointer) in [
            (HostKind::GameStream, InputAllowed::ALL, true),
            (HostKind::Apollo, no_keyboard, true),
            (HostKind::Apollo, no_mouse, true),
            (HostKind::Sunshine, InputAllowed::ALL, false),
        ] {
            let mut view = view(input);
            view.host.kind = kind;
            let mut player = Player::default();
            player.picture(&ctx, Picture::Behind([1920, 1080]), None);
            let settings = quiet(local_pointer);
            let middle = Some(pos2(480.0, 270.0));
            for _ in 0..2 {
                let (heard, shown) = frame(&ctx, &view, &mut player, &settings, middle);
                assert_eq!(switches(&heard), 0, "{kind:?}, {input:?}");
                assert_eq!(shown, CursorIcon::None, "{kind:?}, {input:?}");
            }
        }
    }

    #[test]
    fn the_bar_says_how_to_get_out_of_the_stream() {
        let said = note(&view(InputAllowed::ALL), Hands::Pointer);
        assert_eq!(
            said.text,
            "Keyboard and mouse now go to Salon. Ctrl+Alt+Shift+Z gives them back, \
             Ctrl+Alt+Shift+Q leaves."
        );
        assert!(!said.problem);

        let said = note(&view(InputAllowed::ALL), Hands::GameMouse);
        assert_eq!(
            said.text,
            "Game mouse on: the mouse turns the view. Ctrl+Alt+Shift+M gives the pointer back."
        );
    }

    #[test]
    fn the_bar_says_how_to_give_the_keyboard_and_mouse_back_to_the_pc() {
        let said = note(&view(InputAllowed::ALL), Hands::Released);
        assert_eq!(
            said.text,
            "Keyboard and mouse are this device's: Salon hears nothing of them. \
             Ctrl+Alt+Shift+Z or a click on the picture gives them back."
        );
        assert!(!said.problem);
        // What the PC would ignore matters only once they go to it again.
        let watching = view(InputAllowed { keyboard: false, mouse: false, gamepad: false });
        assert_eq!(bar_note(&watching, Hands::Released, false, None), None);
        assert_eq!(bar_note(&watching, Hands::Released, true, None), Some(said));
    }

    #[test]
    fn the_statistics_say_how_the_stream_is_doing() {
        let mut counts = Counts {
            sent: Some([2560, 1440, 144]),
            codec: Some(Codec::H265),
            on_graphics_card: Some(true),
            ..Counts::default()
        };
        let mut statistics = Statistics::new(Some(counts), 0, 10.0);
        let lines = statistics_lines(&statistics, Some(counts), None);
        assert_eq!(lines.last(), Some(&("Arrived", "Counting…".to_owned())));

        for _ in 0..144 {
            counts.received(20_000, Some(0), Some(Duration::from_millis(5)));
            counts.decoded(Some(Duration::from_millis(9)));
        }
        // Not a second yet: nothing changes.
        statistics.update(Some(counts), 140, 10.5);
        assert_eq!(statistics.rates, None);
        statistics.update(Some(counts), 144, 11.0);
        let lines = statistics_lines(&statistics, Some(counts), Some(Duration::from_millis(3)));
        let said: Vec<_> = lines.iter().map(|(name, value)| format!("{name}: {value}")).collect();
        assert_eq!(
            said,
            [
                "Video: 2560 × 1440, 144 per second, H.265".to_owned(),
                "Decoder: On the graphics card".to_owned(),
                "Arrived: 144.0 per second, 23.0 Mbit/s".to_owned(),
                "Lost on the way: 0 in all, 0.0 % lately".to_owned(),
                "Decoded: 144.0 per second, 9.0 ms each".to_owned(),
                "Shown: 144.0 per second".to_owned(),
                "On the PC: 5.0 ms to capture and encode".to_owned(),
                "Network: 3 ms there and back".to_owned(),
            ]
        );
        // A backend that counts nothing says so.
        assert_eq!(statistics_lines(&statistics, None, None).len(), 1);
        // One that does not know who decodes says that.
        let unknown = Counts { on_graphics_card: None, ..counts };
        let lines = statistics_lines(&statistics, Some(unknown), None);
        assert_eq!(lines[1], ("Decoder", DECODER.to_owned()));
    }

    #[test]
    fn the_bar_says_what_the_microphone_just_did() {
        let mut view = view(InputAllowed::ALL);
        // Nothing to say when the pointer merely passes.
        assert_eq!(bar_note(&view, Hands::Pointer, false, None), None);
        // A stream that starts says how to get out of it.
        assert_eq!(bar_note(&view, Hands::Pointer, true, None), Some(note(&view, Hands::Pointer)));

        view.microphone = Mic::On;
        let said = bar_note(&view, Hands::Pointer, true, Some(News::Microphone(true)))
            .expect("a line about the microphone");
        assert_eq!(
            said.text,
            "The microphone is on: Salon hears this device. Ctrl+Alt+Shift+T turns it off."
        );
        assert!(!said.problem);
        // Later on, the lit switch says it, and the bar has other things to
        // say.
        assert_eq!(
            bar_note(&view, Hands::GameMouse, true, None),
            Some(note(&view, Hands::GameMouse))
        );
        assert_eq!(bar_note(&view, Hands::Pointer, false, None), None);

        view.microphone = Mic::Off;
        let said = bar_note(&view, Hands::Pointer, true, Some(News::Microphone(false)))
            .expect("a line about the microphone");
        assert_eq!(said.text, "The microphone is off: Salon no longer hears this device.");
    }

    #[test]
    fn the_bar_says_what_the_camera_just_did() {
        let mut view = view(InputAllowed::ALL);
        view.camera = Cam::On;
        // The camera opens and the PC is asked before anything leaves.
        let opening = [Some(News::Camera(None)), Some(News::Camera(Some(Filming::Opening)))];
        for news in opening {
            let said =
                bar_note(&view, Hands::Pointer, true, news).expect("a line about the camera");
            assert_eq!(said.text, "Opening the camera: Salon sees nothing yet.");
            assert!(!said.problem);
        }
        let sending = Some(News::Camera(Some(Filming::Sending([1280, 720]))));
        let said = bar_note(&view, Hands::Pointer, true, sending).expect("a line about the camera");
        assert_eq!(
            said.text,
            "The camera is on: Salon sees this device. Ctrl+Alt+Shift+C turns it off."
        );
        // Later on, the lit switch says it.
        assert_eq!(bar_note(&view, Hands::Pointer, false, sending), None);

        view.camera = Cam::Off;
        let said = bar_note(&view, Hands::Pointer, true, Some(News::Camera(None)))
            .expect("a line about the camera");
        assert_eq!(said.text, "The camera is off: Salon no longer sees this device.");
    }

    #[test]
    fn a_camera_that_failed_stays_said_and_the_one_that_just_failed_comes_first() {
        let mut view = view(InputAllowed::ALL);
        view.camera = Cam::Failed("This device has no camera.".into());
        let said = bar_note(&view, Hands::Pointer, false, None).expect("a problem is always said");
        assert_eq!(said, Note { text: "This device has no camera.".into(), problem: true });

        view.microphone = Mic::Failed("This device has no microphone.".into());
        let said = bar_note(&view, Hands::Pointer, false, None).expect("a problem is always said");
        assert_eq!(said.text, "This device has no microphone.");
        let said = bar_note(&view, Hands::Pointer, true, Some(News::Camera(None)))
            .expect("a problem is always said");
        assert_eq!(said.text, "This device has no camera.");
    }

    #[test]
    fn a_microphone_that_is_still_opening_is_not_said_to_be_heard() {
        // A browser asks the person before it hands a microphone over.
        let mut view = view(InputAllowed::ALL);
        view.microphone = Mic::On;
        let said = bar_note(&view, Hands::Pointer, true, Some(News::Microphone(false)))
            .expect("a line about the microphone");
        assert_eq!(said.text, "Opening the microphone: Salon hears nothing yet.");
        assert!(!said.problem);
    }

    #[test]
    fn a_microphone_that_failed_stays_said_and_what_the_pc_ignores_comes_first() {
        let mut view = view(InputAllowed::ALL);
        view.microphone = Mic::Failed("This device has no microphone.".into());
        let said = bar_note(&view, Hands::Pointer, false, None).expect("a problem is always said");
        assert_eq!(said, Note { text: "This device has no microphone.".into(), problem: true });

        view.input = InputAllowed { keyboard: false, ..InputAllowed::ALL };
        let said = bar_note(&view, Hands::Pointer, false, None).expect("a problem is always said");
        assert!(said.text.contains("\"Keyboard Input\""), "{}", said.text);
    }

    #[test]
    fn the_meter_fills_by_ratios_from_a_whisper_to_the_loudest() {
        assert_eq!(meter(0.0), 0.0);
        assert_eq!(meter(0.001), 0.0);
        assert_eq!(meter(1.0), 1.0);
        assert_eq!(meter(4.0), 1.0);
        // A voice: around a tenth of the loudest.
        assert!((meter(0.1) - 0.6).abs() < 0.001);
        // Half as loud is the same step down wherever it starts.
        let step = meter(0.5) - meter(0.25);
        assert!((meter(0.1) - meter(0.05) - step).abs() < 0.001);
    }

    #[test]
    fn the_bar_says_what_apollo_does_not_let_this_device_do() {
        let nothing = InputAllowed { keyboard: false, mouse: false, gamepad: false };
        let said = note(&view(nothing), Hands::Pointer);
        assert_eq!(
            said.text,
            "Salon lets this device watch only. To play, allow \"Keyboard Input\", \"Mouse \
             Input\" and \"Controller Input\" for it on Apollo's PIN page, under Device \
             Management."
        );
        assert!(said.problem);

        let no_keyboard = InputAllowed { keyboard: false, ..InputAllowed::ALL };
        let said = note(&view(no_keyboard), Hands::Pointer);
        assert!(said.problem);
        assert!(said.text.contains("\"Keyboard Input\"") && !said.text.contains("Mouse Input"));

        let no_mouse = InputAllowed { mouse: false, ..InputAllowed::ALL };
        let said = note(&view(no_mouse), Hands::GameMouse);
        assert!(said.problem);
        assert!(said.text.contains("\"Mouse Input\"") && !said.text.contains("Keyboard Input"));

        // No gamepad is no reason for alarm: many people have none.
        let no_gamepad = InputAllowed { gamepad: false, ..InputAllowed::ALL };
        let said = note(&view(no_gamepad), Hands::Pointer);
        assert!(!said.problem);
        assert!(said.text.ends_with(
            "Gamepads are ignored until you allow \"Controller Input\" for this device on \
             Apollo's PIN page, under Device Management."
        ));
    }
}
