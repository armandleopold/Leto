//! A pane inside Leto's window that something other than the window draws
//! on: the pictures of a stream, put straight on the screen by the graphics
//! card that decoded them.
//!
//! It is a window of its own inside the other, over what that one draws.
//! It takes no click and no key: they go to Leto's window as if the pane
//! were not there. Where Leto's window has something to show over the
//! pictures, holes are cut in the pane.
//!
//! Only Windows has it. Elsewhere none opens.

/// A rectangle in pixels of the inside of Leto's window: left, top, width,
/// height.
pub type Place = [i32; 4];

/// The pane. It is closed when dropped.
///
/// It belongs to the thread that opened it, which must be the one of Leto's
/// window: that thread hears what Windows tells the pane.
pub struct Pane {
    #[cfg(windows)]
    inner: windows::Pane,
    /// Nothing opens one elsewhere.
    #[cfg(not(windows))]
    never: std::convert::Infallible,
}

impl Pane {
    /// Opens a pane inside the window `parent`, the handle Windows gave
    /// Leto's window, hidden until it is placed. Called on the thread of
    /// that window. `None` where there are none, or when Windows refuses.
    pub fn open(parent: isize) -> Option<Self> {
        #[cfg(windows)]
        return windows::Pane::open(parent).map(|inner| Self { inner });
        #[cfg(not(windows))]
        {
            let _ = parent;
            None
        }
    }

    /// The handle Windows gave the pane, for what draws on it.
    pub fn handle(&self) -> isize {
        #[cfg(windows)]
        return self.inner.handle();
        #[cfg(not(windows))]
        match self.never {}
    }

    /// Shows the pane at `place` with `holes` cut out of it, or hides it
    /// for `None`. Asking for what it already is costs nothing.
    pub fn place(&mut self, place: Option<Place>, holes: &[Place]) {
        #[cfg(windows)]
        self.inner.place(place, holes);
        #[cfg(not(windows))]
        {
            let _ = (place, holes);
            match self.never {}
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::{ptr, sync::Once};

    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        Graphics::Gdi::{CombineRgn, CreateRectRgn, DeleteObject, RGN_DIFF, SetWindowRgn},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, HTTRANSPARENT, HWND_TOP,
            RegisterClassW, SW_HIDE, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetWindowPos, ShowWindow,
            WM_ERASEBKGND, WM_NCHITTEST, WNDCLASSW, WS_CHILD, WS_CLIPSIBLINGS, WS_DISABLED,
            WS_EX_NOPARENTNOTIFY,
        },
    };

    use super::Place;

    /// The name of the kind of window a pane is, ending in a zero.
    const CLASS: [u16; 10] = {
        let name = b"LetoPane\0\0";
        let mut wide = [0u16; 10];
        let mut at = 0;
        while at < name.len() {
            wide[at] = name[at] as u16;
            at += 1;
        }
        wide
    };

    /// What a pane answers Windows: it is not there for the pointer, and
    /// nothing of it is painted but what is drawn on it.
    unsafe extern "system" fn answer(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            // The window underneath is the one pointed at.
            WM_NCHITTEST => HTTRANSPARENT as LRESULT,
            // Erased already: nothing flashes before the first picture.
            WM_ERASEBKGND => 1,
            // SAFETY: what Windows passed in, handed back for the rest.
            _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
        }
    }

    pub struct Pane {
        window: HWND,
        /// Where it is and what is cut out of it, as Windows was told.
        placed: Option<(Place, Vec<Place>)>,
    }

    impl Pane {
        pub fn open(parent: isize) -> Option<Self> {
            static KIND: Once = Once::new();
            // SAFETY: the program's own module, which always is.
            let program = unsafe { GetModuleHandleW(ptr::null()) };
            KIND.call_once(|| {
                let kind = WNDCLASSW {
                    lpfnWndProc: Some(answer),
                    hInstance: program,
                    lpszClassName: CLASS.as_ptr(),
                    ..Default::default()
                };
                // SAFETY: a kind of window described in full, with a name
                // and an answer that both live as long as the program.
                if unsafe { RegisterClassW(&kind) } == 0 {
                    log::debug!("Windows did not take the kind of window a pane is");
                }
            });
            // SAFETY: a window of the kind above inside `parent`, which the
            // caller says is Leto's window, on its thread. It starts out
            // hidden, and disabled: Windows gives it no click and no key.
            let window = unsafe {
                CreateWindowExW(
                    WS_EX_NOPARENTNOTIFY,
                    CLASS.as_ptr(),
                    ptr::null(),
                    WS_CHILD | WS_DISABLED | WS_CLIPSIBLINGS,
                    0,
                    0,
                    1,
                    1,
                    parent as HWND,
                    ptr::null_mut(),
                    program,
                    ptr::null(),
                )
            };
            if window.is_null() {
                log::debug!("Windows opened no pane inside Leto's window");
                return None;
            }
            Some(Self { window, placed: None })
        }

        pub fn handle(&self) -> isize {
            self.window as isize
        }

        pub fn place(&mut self, place: Option<Place>, holes: &[Place]) {
            let Some(place) = place.filter(|place| place[2] > 0 && place[3] > 0) else {
                if self.placed.take().is_some() {
                    // SAFETY: the pane, which lives as long as this does.
                    unsafe { ShowWindow(self.window, SW_HIDE) };
                }
                return;
            };
            if self.placed.as_ref().is_some_and(|(at, cut)| *at == place && cut == holes) {
                return;
            }
            let [left, top, width, height] = place;
            // SAFETY: the pane, moved and shown without taking the keys
            // from Leto's window; shapes made here and either given to
            // Windows with the pane or deleted here.
            unsafe {
                SetWindowPos(
                    self.window,
                    HWND_TOP,
                    left,
                    top,
                    width,
                    height,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
                if holes.is_empty() {
                    SetWindowRgn(self.window, ptr::null_mut(), 1);
                } else {
                    // What is left of the pane, in its own pixels.
                    let shape = CreateRectRgn(0, 0, width, height);
                    for hole in holes {
                        let (x, y) = (hole[0] - left, hole[1] - top);
                        let cut = CreateRectRgn(x, y, x + hole[2], y + hole[3]);
                        CombineRgn(shape, shape, cut, RGN_DIFF);
                        DeleteObject(cut);
                    }
                    // Windows owns the shape from here on.
                    if SetWindowRgn(self.window, shape, 1) == 0 {
                        DeleteObject(shape);
                    }
                }
            }
            self.placed = Some((place, holes.to_vec()));
        }
    }

    impl Drop for Pane {
        fn drop(&mut self) {
            // SAFETY: the pane, closed once, on the thread that opened it:
            // nothing can send it elsewhere, a handle is not `Send`.
            unsafe { DestroyWindow(self.window) };
        }
    }
}
