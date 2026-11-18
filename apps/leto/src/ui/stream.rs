//! An app of a PC, streamed: the picture fills the window, and the keyboard
//! and the mouse of this device go to the PC.

use egui::{
    Align, Align2, Color32, CursorIcon, Event, FontId, Id, Key, Layout, Margin, Order, Pos2, Rect,
    RichText, Sense, Stroke, TextFormat, TextureHandle, TextureOptions, Ui, UiBuilder, Vec2,
    text::{LayoutJob, TextWrapping},
    vec2,
};
use leto_core::input::Input;

use super::{icons, widgets};
use crate::{
    backend::Picture,
    input::{Controls, Shortcut},
    state::{Mic, State, StreamView},
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

/// The quietest sound the microphone's meter shows, in decibels below the
/// loudest a microphone captures.
const METER_FLOOR: f32 = -50.0;

/// A key that went down or came up while a stream had the keyboard.
struct KeyStroke {
    /// The key by what the layout prints on it.
    printed: Key,
    /// The key by where it sits, when the system says.
    place: Option<Key>,
    down: bool,
}

/// What the pictures of the stream are shown on.
enum Surface {
    /// The window draws them.
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
            Self::Behind(size) => *size,
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
    /// The last thing worth a line in the bar was about the microphone.
    talks_of_microphone: bool,
}

impl Player {
    /// Shows `picture` from now on.
    pub fn picture(&mut self, ctx: &egui::Context, picture: Picture) {
        match (picture, &mut self.surface) {
            (Picture::Pixels(pixels), Some(Surface::Texture(texture))) => {
                texture.set(pixels, TextureOptions::LINEAR);
            }
            (Picture::Pixels(pixels), _) => {
                let texture = ctx.load_texture("stream", pixels, TextureOptions::LINEAR);
                self.surface = Some(Surface::Texture(texture));
            }
            (Picture::Behind(size), _) => self.surface = Some(Surface::Behind(size)),
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
            Event::Key { key, physical_key, pressed, .. } => {
                self.strokes.push(KeyStroke {
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
        *self = Self::default();
    }
}

/// What the backend says of the sound of a stream, both ways.
#[derive(Clone, Copy)]
pub struct Sound<'a> {
    /// Why the stream has no sound, when it has none.
    pub problem: Option<&'a str>,
    /// How loud the microphone is, from 0 to 1, while it captures. `None`
    /// with the switch on means it is still opening: a browser asks the
    /// person first.
    pub voice: Option<f32>,
}

/// Draws the stream and returns what the person did since the last frame,
/// to send to the PC in this order.
pub fn show(
    ui: &mut Ui,
    state: &mut State,
    view: &StreamView,
    player: &mut Player,
    sound: Sound<'_>,
) -> Vec<Input> {
    let window = ui.max_rect();
    let (now, focused) = ui.input(|input| (input.time, input.focused));
    let fills_screen = window::fills_screen(ui.ctx());
    let mut asked = Asked::default();
    // Once connected, every key is the PC's, the system's shortcuts too
    // where the system lets go of them.
    window::keep_keyboard(view.connected);

    player.hole = None;
    let picture = player.surface.as_ref().map(|surface| {
        let size = surface.size();
        let place = fit(vec2(size[0] as f32, size[1] as f32), window);
        match surface {
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
        asked.leave |= waiting(ui, window, view);
    }

    if view.connected {
        let bar_until = *player.bar_until.get_or_insert(now + BAR_STAYS);
        let mut refused = false;

        for stroke in player.strokes.drain(..) {
            refused |= stroke.down && !view.input.keyboard;
            match player.controls.key(stroke.printed, stroke.place, stroke.down) {
                Some(Shortcut::Leave) => asked.leave = true,
                Some(Shortcut::FullScreen) => asked.fill = true,
                Some(Shortcut::GameMouse) => asked.game_mouse = true,
                Some(Shortcut::Microphone) => asked.microphone = true,
                None => {}
            }
        }
        if let Some((place, size)) = picture {
            refused |= mouse(ui, player, place, size) && !view.input.mouse;
        }
        if !focused && player.controls.holds_anything() {
            // Another window has the keyboard: nothing may stay held down
            // on the PC.
            player.controls.release_all();
        }
        if refused {
            player.bar_until = Some(bar_until.max(now + BAR_RETURNS));
        }
        let capturing = sound.voice.is_some();
        if player.microphone_was.as_ref() != Some(&view.microphone) || player.captured != capturing
        {
            // The bar says what the microphone just did. That a PC takes
            // one is not news: its switch is there to see.
            let was_on = player.microphone_was == Some(Mic::On);
            if was_on || matches!(view.microphone, Mic::On | Mic::Failed(_)) {
                player.talks_of_microphone = true;
                player.bar_until = Some(bar_until.max(now + BAR_RETURNS));
            }
            player.microphone_was = Some(view.microphone.clone());
            player.captured = capturing;
        }
        if view.microphone == Mic::On {
            // The meter moves, and a microphone that stops is noticed, even
            // while the picture stands still.
            ui.ctx().request_repaint_after_secs(0.05);
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

    overlay(ui, window, view, player, sound, now, &mut asked);

    if asked.microphone {
        state.switch_microphone();
    }
    if asked.game_mouse {
        player.game_mouse = !player.game_mouse;
        // The bar says what just changed, and how to change it back.
        player.bar_until = Some(now + BAR_RETURNS);
        player.talks_of_microphone = false;
    }
    if player.pointer_kept && window::pointer_lost() {
        // A browser gives the pointer back when Escape is pressed, whoever
        // asked for it: the game mouse is over.
        player.pointer_kept = false;
        player.game_mouse = false;
        player.bar_until = Some(now + BAR_RETURNS);
        player.talks_of_microphone = false;
    }
    let keep = player.game_mouse && view.connected && focused && !asked.leave;
    if keep != player.pointer_kept {
        player.pointer_kept = keep;
        window::keep_pointer(ui.ctx(), keep);
    }
    if asked.fill {
        player.filled_screen = !fills_screen;
        window::fill_screen(ui.ctx(), !fills_screen);
    }
    if asked.leave {
        player.controls.release_all();
        state.leave_stream();
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
    sound: Sound<'_>,
    now: f64,
    asked: &mut Asked,
) {
    let has_picture = player.surface.is_some();
    let (idle, pointer) =
        ui.input(|input| (input.pointer.time_since_last_movement(), input.pointer.hover_pos()));
    // With the game mouse the pointer is hidden and going nowhere: it opens
    // nothing.
    let pointing = pointer.filter(|_| !player.game_mouse);
    let grip_zone =
        Rect::from_center_size(window.center_top() + vec2(0.0, GRIP_ZONE.y / 2.0), GRIP_ZONE);
    let on_bar = pointing.is_some_and(|pointer| {
        grip_zone.contains(pointer)
            || player.bar.is_some_and(|bar| bar.expand(BAR_MARGIN).contains(pointer))
    });
    let timed = player.bar_until.is_some_and(|until| now < until);
    if !timed {
        // What the bar said about the microphone has been said.
        player.talks_of_microphone = false;
    }
    let wanted = !has_picture || timed || on_bar;
    let opacity = ui.ctx().animate_bool_with_time(Id::unique("stream-bar"), wanted, 0.2);

    if opacity > 0.0 {
        let content = Bar {
            view,
            picture: player.surface.as_ref().map(Surface::size),
            sound_problem: sound.problem,
            game_mouse: player.game_mouse,
            voice: sound.voice.map(meter),
            note: bar_note(
                view,
                player.game_mouse,
                timed,
                player.talks_of_microphone.then_some(sound.voice.is_some()),
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

    if let (Mic::On, Some(level)) = (&view.microphone, sound.voice)
        && opacity < 1.0
    {
        // The PC hears this device: that shows for as long as it does,
        // whether the bar is there to say so or not.
        live(ui, window, meter(level), 1.0 - opacity);
    }
    if !view.connected || !has_picture {
        return;
    }
    if player.game_mouse || (pointer.is_some() && !on_bar) {
        // The PC draws its own pointer in the picture.
        ui.ctx().set_cursor_icon(CursorIcon::None);
    }
    if pointing.is_some() && opacity == 0.0 && idle < GRIP_STAYS {
        grip(ui, window);
        ui.ctx().request_repaint_after_secs(0.25);
    }
}

/// A small handle at the top of the picture: where to rest the pointer to
/// get the bar.
fn grip(ui: &Ui, window: Rect) {
    let rect = Rect::from_center_size(window.center_top() + vec2(0.0, 7.0), vec2(44.0, 5.0));
    let painter = ui.painter();
    painter.rect_filled(rect.expand(1.0), 4, Color32::from_black_alpha(120));
    painter.rect_filled(rect, 3, theme::MOON.gamma_multiply(0.55));
}

/// A small microphone in a corner of the picture, filled with what it
/// hears: the sign that the PC is listening.
fn live(ui: &Ui, window: Rect, level: f32, opacity: f32) {
    let rect = Rect::from_center_size(window.right_top() + vec2(-28.0, 28.0), Vec2::splat(32.0));
    let painter =
        ui.ctx().layer_painter(egui::LayerId::new(Order::Foreground, Id::unique("stream-live")));
    let ground = Color32::from_black_alpha(170).gamma_multiply(opacity);
    painter.circle_filled(rect.center(), rect.width() / 2.0, ground);
    icons::microphone(&painter, rect.shrink(8.0), theme::SUN.gamma_multiply(opacity), level);
}

/// How full the microphone's meter is for a level from 0 to 1. Ears hear in
/// ratios, so the meter does too: each halving of the level takes the same
/// slice off it.
fn meter(level: f32) -> f32 {
    if level <= 0.0 {
        return 0.0;
    }
    let decibels = 20.0 * level.log10();
    ((decibels - METER_FLOOR) / -METER_FLOOR).clamp(0.0, 1.0)
}

/// What is shown until the first picture. Returns true when the person
/// gives up waiting.
fn waiting(ui: &mut Ui, window: Rect, view: &StreamView) -> bool {
    let text = if view.connected {
        "Waiting for the first picture…".to_owned()
    } else {
        format!("Starting {} on {}…", view.app.title, view.host.name)
    };
    let area = Rect::from_center_size(window.center(), vec2(window.width().min(420.0), 120.0));
    let layout = Layout::top_down(Align::Center);
    ui.scope_builder(UiBuilder::new().max_rect(area).layout(layout), |ui| {
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
                            let others = if narrow { 210.0 } else { 330.0 };
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
/// own, because something changed. `microphone` is set when the change was
/// the microphone's, and says whether it captures.
fn bar_note(
    view: &StreamView,
    game_mouse: bool,
    timed: bool,
    microphone: Option<bool>,
) -> Option<Note> {
    let host = &view.host.name;
    if view.connected && !(view.input.keyboard && view.input.mouse) {
        // What does not work stays said for as long as the bar shows.
        return Some(note(view, game_mouse));
    }
    let news = |text: String| Some(Note { text, problem: false });
    match (&view.microphone, microphone.filter(|_| timed)) {
        (Mic::Failed(why), _) => return Some(Note { text: why.clone(), problem: true }),
        (Mic::On, Some(true)) => {
            return news(format!(
                "The microphone is on: {host} hears this device. {} turns it off.",
                Shortcut::Microphone.keys()
            ));
        }
        (Mic::On, Some(false)) => {
            return news(format!("Opening the microphone: {host} hears nothing yet."));
        }
        (Mic::Off, Some(_)) => {
            return news(format!("The microphone is off: {host} no longer hears this device."));
        }
        _ => {}
    }
    timed.then(|| note(view, game_mouse))
}

/// What the bar has to say about playing on this PC.
fn note(view: &StreamView, game_mouse: bool) -> Note {
    let host = &view.host.name;
    let allowed = view.input;
    let apollo = "on Apollo's PIN page, under Device Management";
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
            let mut text = if game_mouse {
                format!(
                    "Game mouse on: the mouse turns the view. {} gives the pointer back.",
                    Shortcut::GameMouse.keys()
                )
            } else {
                format!(
                    "Keyboard and mouse now go to {host}. {} brings you back.",
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
        input::InputAllowed,
        model::{AppSummary, HostId, HostKind, HostSummary, Reachability},
    };

    use super::*;

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
        }
    }

    #[test]
    fn the_bar_says_how_to_get_out_of_the_stream() {
        let said = note(&view(InputAllowed::ALL), false);
        assert_eq!(
            said.text,
            "Keyboard and mouse now go to Salon. Ctrl+Alt+Shift+Q brings you back."
        );
        assert!(!said.problem);

        let said = note(&view(InputAllowed::ALL), true);
        assert_eq!(
            said.text,
            "Game mouse on: the mouse turns the view. Ctrl+Alt+Shift+M gives the pointer back."
        );
    }

    #[test]
    fn the_bar_says_what_the_microphone_just_did() {
        let mut view = view(InputAllowed::ALL);
        // Nothing to say when the pointer merely passes.
        assert_eq!(bar_note(&view, false, false, None), None);
        // A stream that starts says how to get out of it.
        assert_eq!(bar_note(&view, false, true, None), Some(note(&view, false)));

        view.microphone = Mic::On;
        let said = bar_note(&view, false, true, Some(true)).expect("a line about the microphone");
        assert_eq!(
            said.text,
            "The microphone is on: Salon hears this device. Ctrl+Alt+Shift+T turns it off."
        );
        assert!(!said.problem);
        // Later on, the lit switch says it, and the bar has other things to
        // say.
        assert_eq!(bar_note(&view, true, true, None), Some(note(&view, true)));
        assert_eq!(bar_note(&view, false, false, None), None);

        view.microphone = Mic::Off;
        let said = bar_note(&view, false, true, Some(false)).expect("a line about the microphone");
        assert_eq!(said.text, "The microphone is off: Salon no longer hears this device.");
    }

    #[test]
    fn a_microphone_that_is_still_opening_is_not_said_to_be_heard() {
        // A browser asks the person before it hands a microphone over.
        let mut view = view(InputAllowed::ALL);
        view.microphone = Mic::On;
        let said = bar_note(&view, false, true, Some(false)).expect("a line about the microphone");
        assert_eq!(said.text, "Opening the microphone: Salon hears nothing yet.");
        assert!(!said.problem);
    }

    #[test]
    fn a_microphone_that_failed_stays_said_and_what_the_pc_ignores_comes_first() {
        let mut view = view(InputAllowed::ALL);
        view.microphone = Mic::Failed("This device has no microphone.".into());
        let said = bar_note(&view, false, false, None).expect("a problem is always said");
        assert_eq!(said, Note { text: "This device has no microphone.".into(), problem: true });

        view.input = InputAllowed { keyboard: false, ..InputAllowed::ALL };
        let said = bar_note(&view, false, false, None).expect("a problem is always said");
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
        let said = note(&view(nothing), false);
        assert_eq!(
            said.text,
            "Salon lets this device watch only. To play, allow \"Keyboard Input\", \"Mouse \
             Input\" and \"Controller Input\" for it on Apollo's PIN page, under Device \
             Management."
        );
        assert!(said.problem);

        let no_keyboard = InputAllowed { keyboard: false, ..InputAllowed::ALL };
        let said = note(&view(no_keyboard), false);
        assert!(said.problem);
        assert!(said.text.contains("\"Keyboard Input\"") && !said.text.contains("Mouse Input"));

        let no_mouse = InputAllowed { mouse: false, ..InputAllowed::ALL };
        let said = note(&view(no_mouse), true);
        assert!(said.problem);
        assert!(said.text.contains("\"Mouse Input\"") && !said.text.contains("Keyboard Input"));

        // No gamepad is no reason for alarm: many people have none.
        let no_gamepad = InputAllowed { gamepad: false, ..InputAllowed::ALL };
        let said = note(&view(no_gamepad), false);
        assert!(!said.problem);
        assert!(said.text.ends_with(
            "Gamepads are ignored until you allow \"Controller Input\" for this device on \
             Apollo's PIN page, under Device Management."
        ));
    }
}
