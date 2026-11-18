//! An app of a PC, streamed: the picture fills the window, and the keyboard
//! and the mouse of this device go to the PC.

use egui::{
    Align, Align2, Color32, CursorGrab, CursorIcon, Event, FontId, Id, Key, Layout, Margin, Order,
    Pos2, Rect, RichText, Sense, Stroke, TextFormat, TextureHandle, TextureOptions, Ui, UiBuilder,
    Vec2, ViewportCommand,
    text::{LayoutJob, TextWrapping},
    vec2,
};
use leto_core::input::Input;

use super::{icons, widgets};
use crate::{
    input::{Controls, Shortcut},
    state::{State, StreamView},
    theme,
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

/// How the pointer is kept in the window for the game mouse. Each system
/// offers one way only.
const KEEP_POINTER: CursorGrab = if cfg!(any(target_os = "macos", target_arch = "wasm32")) {
    CursorGrab::Locked
} else {
    CursorGrab::Confined
};

/// A key that went down or came up while a stream had the keyboard.
struct KeyStroke {
    /// The key by what the layout prints on it.
    printed: Key,
    /// The key by where it sits, when the system says.
    place: Option<Key>,
    down: bool,
}

/// The stream on screen: its picture, and what the person does to it.
#[derive(Default)]
pub struct Player {
    texture: Option<TextureHandle>,
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
}

impl Player {
    /// Shows `picture` from now on.
    pub fn picture(&mut self, ctx: &egui::Context, picture: egui::ColorImage) {
        match &mut self.texture {
            Some(texture) => texture.set(picture, TextureOptions::LINEAR),
            None => {
                self.texture = Some(ctx.load_texture("stream", picture, TextureOptions::LINEAR));
            }
        }
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
            ctx.send_viewport_cmd(ViewportCommand::Fullscreen(false));
        }
        if self.pointer_kept {
            ctx.send_viewport_cmd(ViewportCommand::CursorGrab(CursorGrab::None));
        }
        *self = Self::default();
    }
}

/// Draws the stream and returns what the person did since the last frame,
/// to send to the PC in this order.
pub fn show(
    ui: &mut Ui,
    state: &mut State,
    view: &StreamView,
    player: &mut Player,
    sound_problem: Option<&str>,
) -> Vec<Input> {
    let window = ui.max_rect();
    let (now, focused, fills_screen) =
        ui.input(|input| (input.time, input.focused, input.viewport().fullscreen.unwrap_or(false)));
    let mut asked = Asked::default();

    ui.painter().rect_filled(window, 0, Color32::BLACK);
    let picture = player.texture.as_ref().map(|texture| {
        let place = fit(texture.size_vec2(), window);
        egui::Image::new((texture.id(), place.size())).paint_at(ui, place);
        (place, texture.size())
    });
    if picture.is_none() {
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
    } else {
        // Nothing to play with yet: the keys are still Leto's.
        player.strokes.clear();
        let (escape, f11) =
            ui.input(|input| (input.key_pressed(Key::Escape), input.key_pressed(Key::F11)));
        asked.fill |= f11;
        // The way out of the full screen first, then out of the stream.
        if escape {
            if fills_screen { asked.fill = true } else { asked.leave = true }
        }
    }

    overlay(ui, window, view, player, sound_problem, now, &mut asked);

    if asked.game_mouse {
        player.game_mouse = !player.game_mouse;
        // The bar says what just changed, and how to change it back.
        player.bar_until = Some(now + BAR_RETURNS);
    }
    let keep = player.game_mouse && view.connected && focused && !asked.leave;
    if keep != player.pointer_kept {
        player.pointer_kept = keep;
        let grab = if keep { KEEP_POINTER } else { CursorGrab::None };
        ui.ctx().send_viewport_cmd(ViewportCommand::CursorGrab(grab));
    }
    if asked.fill {
        player.filled_screen = !fills_screen;
        ui.ctx().send_viewport_cmd(ViewportCommand::Fullscreen(!fills_screen));
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
    sound_problem: Option<&str>,
    now: f64,
    asked: &mut Asked,
) {
    let has_picture = player.texture.is_some();
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
    let wanted = !has_picture || timed || on_bar;
    let opacity = ui.ctx().animate_bool_with_time(Id::unique("stream-bar"), wanted, 0.2);

    if opacity > 0.0 {
        let content = Bar {
            view,
            picture: player.texture.as_ref().map(TextureHandle::size),
            sound_problem,
            game_mouse: player.game_mouse,
            // A note under the buttons when there is something to say, not
            // every time the pointer passes.
            note: (timed || view.connected && !(view.input.keyboard && view.input.mouse))
                .then(|| note(view, player.game_mouse)),
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
                                } else {
                                    "Full screen (F11)".to_owned()
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
                    "Your keyboard and mouse now go to {host}. {} brings you back.",
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
        }
    }

    #[test]
    fn the_bar_says_how_to_get_out_of_the_stream() {
        let said = note(&view(InputAllowed::ALL), false);
        assert_eq!(
            said.text,
            "Your keyboard and mouse now go to Salon. Ctrl+Alt+Shift+Q brings you back."
        );
        assert!(!said.problem);

        let said = note(&view(InputAllowed::ALL), true);
        assert_eq!(
            said.text,
            "Game mouse on: the mouse turns the view. Ctrl+Alt+Shift+M gives the pointer back."
        );
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
