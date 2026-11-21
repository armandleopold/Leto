//! What a stream asks of the window it is shown in: to fill the screen, to
//! keep the pointer, to leave the keyboard to the PC, and what the screen it
//! fills is like.
//!
//! A desktop window is asked through egui, and the system for what egui
//! does not do. A browser page is not egui's to command: the browser is
//! asked itself, and may refuse or change its mind when the person presses
//! Escape.

pub use leto_system::keyboard::Stroke;

#[cfg(not(target_arch = "wasm32"))]
pub use desktop::*;
#[cfg(target_arch = "wasm32")]
pub use page::*;

#[cfg(not(target_arch = "wasm32"))]
mod desktop {
    use egui::{Context, CursorGrab, ViewportCommand};
    use leto_core::{engine::ScreenMode, input::Key};
    use leto_system::{display, keyboard};

    use super::Stroke;

    /// How the pointer is kept in the window. Each system offers one way
    /// only.
    const KEEP_POINTER: CursorGrab =
        if cfg!(target_os = "macos") { CursorGrab::Locked } else { CursorGrab::Confined };

    /// Whether the window fills the screen.
    pub fn fills_screen(ctx: &Context) -> bool {
        ctx.input(|input| input.viewport().fullscreen.unwrap_or(false))
    }

    /// Makes the window fill the screen, or gives the screen back.
    pub fn fill_screen(ctx: &Context, fill: bool) {
        ctx.send_viewport_cmd(ViewportCommand::Fullscreen(fill));
    }

    /// Keeps the pointer inside the window, hidden, or lets it go.
    pub fn keep_pointer(ctx: &Context, keep: bool) {
        let grab = if keep { KEEP_POINTER } else { CursorGrab::None };
        ctx.send_viewport_cmd(ViewportCommand::CursorGrab(grab));
    }

    /// Whether the pointer was taken back since it was last kept. A desktop
    /// window keeps what it was given.
    pub fn pointer_lost() -> bool {
        false
    }

    /// Starts listening to the system. Called once, when the app starts.
    pub fn watch(ctx: &Context) {
        // A key taken for the stream is passed on at the next frame.
        let ctx = ctx.clone();
        keyboard::set_wake(move || ctx.request_repaint());
    }

    /// Leaves every key to the stream, the system's shortcuts too, or gives
    /// them back. Where the system does not let go of its shortcuts, the
    /// stream gets the keys the window gets. Cheap to call every frame.
    pub fn keep_keyboard(keep: bool) {
        keyboard::capture(keep);
    }

    /// A key reached the window although the system was to keep it for the
    /// stream: the system is asked again.
    pub fn slipped() {
        keyboard::slipped();
    }

    /// The keys the system gave the stream since the last call, in order.
    /// They never reach the window.
    pub fn keys() -> Vec<Stroke> {
        keyboard::take()
    }

    /// The letter this device's layout prints on the key at `place`, named
    /// by where it sits on a US keyboard, where the system tells.
    pub fn printed_letter(place: Key) -> Option<char> {
        keyboard::letter_at(place)
    }

    /// The screen the window is on: its pixels and its pace as the system
    /// says, or as egui does, at 60 pictures per second.
    pub fn screen(ctx: &Context) -> Option<ScreenMode> {
        let (scale, monitor) = ctx.input(|input| {
            let viewport = input.viewport();
            (viewport.native_pixels_per_point, viewport.monitor_size)
        });
        let scale = scale.unwrap_or(1.0);
        if let Some(mode) = display::of_active_window() {
            let display::Mode { width, height, refresh_hz } = mode;
            return Some(ScreenMode { width, height, refresh_hz });
        }
        let size = monitor? * scale;
        let (width, height) = (size.x.round() as u32, size.y.round() as u32);
        // A phone held upright streams on its side: it turns to landscape
        // for a stream, and the PC is asked for a landscape screen.
        #[cfg(target_os = "android")]
        let (width, height) = phone(width, height, phone_decodes_whole_screens());
        Some(ScreenMode { width, height, refresh_hz: 60 })
    }

    /// Whether this phone decodes H.264 on its video hardware, which keeps
    /// up with pictures of its whole screen.
    #[cfg(target_os = "android")]
    fn phone_decodes_whole_screens() -> bool {
        leto_media::decodable().hardware.contains(leto_core::engine::Codec::H264)
    }

    /// The most a phone without a hardware decoder asks for on its short
    /// side. OpenH264 on its processor falls behind at its whole screen
    /// (2400 by 1080 and the like): the first picture came and no other.
    #[cfg(any(target_os = "android", test))]
    const PHONE_SHORT: u32 = 720;

    /// What a phone asks for: its whole screen in landscape when it
    /// `decodes_whole` screens; otherwise at most [`PHONE_SHORT`] high, with
    /// the shape of its screen and a width the encoders take (a multiple
    /// of 8).
    #[cfg(any(target_os = "android", test))]
    fn phone(width: u32, height: u32, decodes_whole: bool) -> (u32, u32) {
        let (long, short) = (width.max(height), width.min(height));
        if decodes_whole || short <= PHONE_SHORT {
            return (long, short);
        }
        let long = (u64::from(long) * u64::from(PHONE_SHORT) / u64::from(short)) as u32;
        ((long / 8 * 8).max(8), PHONE_SHORT)
    }

    #[cfg(test)]
    mod tests {
        use super::phone;

        #[test]
        fn a_phone_decoding_on_its_processor_asks_for_720_lines() {
            assert_eq!(phone(1080, 2400, false), (1600, 720));
            assert_eq!(phone(2400, 1080, false), (1600, 720));
            assert_eq!(phone(1440, 3120, false), (1560, 720));
            assert_eq!(phone(1080, 2340, false), (1560, 720));
            // A small screen is asked for as it is.
            assert_eq!(phone(320, 640, false), (640, 320));
        }

        #[test]
        fn a_phone_that_decodes_on_its_hardware_asks_for_its_whole_screen() {
            assert_eq!(phone(1080, 2400, true), (2400, 1080));
            assert_eq!(phone(2412, 1080, true), (2412, 1080));
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod page {
    use std::cell::{Cell, RefCell};

    use egui::{Context, Vec2, vec2};
    use js_sys::{Function, Reflect};
    use leto_core::engine::ScreenMode;
    use wasm_bindgen::{JsCast as _, JsValue, closure::Closure};
    use web_sys::{AddEventListenerOptions, Document, KeyboardEvent, MouseEvent};

    /// What became of the pointer.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Pointer {
        Free,
        /// The browser was asked to keep it, and has not answered.
        Asked,
        Kept,
        /// The browser took it back: the person pressed Escape, or the
        /// browser refused.
        Lost,
    }

    thread_local! {
        /// Whether the browser's own shortcuts are kept from acting.
        static KEYBOARD_KEPT: Cell<bool> = const { Cell::new(false) };
        static POINTER: Cell<Pointer> = const { Cell::new(Pointer::Free) };
        /// How the mouse moved while the pointer was kept.
        static MOVES: RefCell<Vec<Vec2>> = const { RefCell::new(Vec::new()) };
    }

    fn document() -> Option<Document> {
        web_sys::window()?.document()
    }

    /// Starts listening to the page. Called once, when the app starts.
    pub fn watch(ctx: &Context) {
        let Some(document) = document() else { return };

        // Before anything else hears of a key: a stream's F5 is the PC's,
        // not the browser's way to load the page again.
        let keys = Closure::<dyn FnMut(KeyboardEvent)>::new(|event: KeyboardEvent| {
            if KEYBOARD_KEPT.get() {
                event.prevent_default();
            }
        });
        let first = AddEventListenerOptions::new();
        first.set_capture(true);
        listen(&document, "keydown", keys.as_ref(), Some(&first));
        // These listen for as long as the page lives.
        keys.forget();

        let repaint = ctx.clone();
        let moves = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
            if POINTER.get() == Pointer::Kept {
                let moved = vec2(event.movement_x() as f32, event.movement_y() as f32);
                MOVES.with_borrow_mut(|moves| moves.push(moved));
                repaint.request_repaint();
            }
        });
        listen(&document, "mousemove", moves.as_ref(), None);
        moves.forget();

        let (page, repaint) = (document.clone(), ctx.clone());
        let changed = Closure::<dyn FnMut()>::new(move || {
            let kept = page.pointer_lock_element().is_some();
            let was = POINTER.get();
            POINTER.set(match (kept, was) {
                (true, _) => Pointer::Kept,
                (false, Pointer::Free) => Pointer::Free,
                (false, _) => Pointer::Lost,
            });
            repaint.request_repaint();
        });
        listen(&document, "pointerlockchange", changed.as_ref(), None);
        changed.forget();

        let repaint = ctx.clone();
        let refused = Closure::<dyn FnMut()>::new(move || {
            if POINTER.get() != Pointer::Free {
                POINTER.set(Pointer::Lost);
            }
            repaint.request_repaint();
        });
        listen(&document, "pointerlockerror", refused.as_ref(), None);
        refused.forget();
    }

    fn listen(
        document: &Document,
        event: &str,
        listener: &JsValue,
        options: Option<&AddEventListenerOptions>,
    ) {
        let listener = listener.unchecked_ref();
        let added = match options {
            Some(options) => document
                .add_event_listener_with_callback_and_add_event_listener_options(
                    event, listener, options,
                ),
            None => document.add_event_listener_with_callback(event, listener),
        };
        if added.is_err() {
            log::warn!("the page does not let Leto listen to {event}");
        }
    }

    /// Whether the page fills the screen.
    pub fn fills_screen(_ctx: &Context) -> bool {
        document().is_some_and(|document| document.fullscreen_element().is_some())
    }

    /// Makes the page fill the screen, or gives the screen back. A browser
    /// only does the first when the person has just clicked or pressed
    /// something.
    pub fn fill_screen(ctx: &Context, fill: bool) {
        let Some(document) = document() else { return };
        if fill == fills_screen(ctx) {
            return;
        }
        if fill {
            if let Some(page) = document.document_element()
                && page.request_fullscreen().is_err()
            {
                log::warn!("the browser did not let the page fill the screen");
            }
        } else {
            document.exit_fullscreen();
        }
        // On a page that fills the screen, Chrome and Edge can leave Escape
        // and the system's shortcuts to the page: to the PC, that is.
        keyboard_lock(fill);
    }

    /// Asks the browser for the keys it keeps for itself, or gives them
    /// back. Not every browser has this: the others keep them.
    fn keyboard_lock(lock: bool) {
        let Some(window) = web_sys::window() else { return };
        let Ok(keyboard) = Reflect::get(&window.navigator(), &"keyboard".into()) else { return };
        let name = if lock { "lock" } else { "unlock" };
        if let Ok(method) = Reflect::get(&keyboard, &name.into())
            && let Some(method) = method.dyn_ref::<Function>()
        {
            // The answer changes nothing Leto does.
            let _ = method.call0(&keyboard);
        }
    }

    /// Keeps the pointer on the page, hidden, or lets it go. The browser
    /// answers later, and may let go of it on its own.
    pub fn keep_pointer(_ctx: &Context, keep: bool) {
        let Some(document) = document() else { return };
        if keep {
            if let Some(canvas) = document.get_element_by_id(crate::CANVAS_ID) {
                POINTER.set(Pointer::Asked);
                canvas.request_pointer_lock();
            }
        } else {
            POINTER.set(Pointer::Free);
            MOVES.with_borrow_mut(Vec::clear);
            if document.pointer_lock_element().is_some() {
                document.exit_pointer_lock();
            }
        }
    }

    /// Whether the browser took the pointer back since it was last kept.
    /// Says so once.
    pub fn pointer_lost() -> bool {
        let lost = POINTER.get() == Pointer::Lost;
        if lost {
            POINTER.set(Pointer::Free);
        }
        lost
    }

    /// Keeps the browser's own shortcuts from acting, or lets them.
    pub fn keep_keyboard(keep: bool) {
        KEYBOARD_KEPT.set(keep);
    }

    /// A page gets every key from its browser, which keeps none from it.
    pub fn slipped() {}

    /// Keys taken from the system: a page gets every key it gets from its
    /// browser.
    pub fn keys() -> Vec<super::Stroke> {
        Vec::new()
    }

    /// A browser tells what a key types, and nothing of its layout.
    pub fn printed_letter(_place: leto_core::input::Key) -> Option<char> {
        None
    }

    /// The screen of this device, lying down: a PC makes a screen for a
    /// phone as wide as the phone is long. A browser does not tell its pace.
    pub fn screen(_ctx: &Context) -> Option<ScreenMode> {
        let window = web_sys::window()?;
        let screen = window.screen().ok()?;
        let ratio = window.device_pixel_ratio();
        let (width, height) = (screen.width().ok()?, screen.height().ok()?);
        let pixels = |points: i32| (f64::from(points) * ratio).round() as u32;
        let (long, short) = (pixels(width.max(height)), pixels(width.min(height)));
        Some(ScreenMode { width: long, height: short, refresh_hz: 60 })
    }

    /// How the mouse moved while the pointer was kept, since the last call.
    pub fn mouse_moves() -> Vec<Vec2> {
        MOVES.with_borrow_mut(std::mem::take)
    }
}
