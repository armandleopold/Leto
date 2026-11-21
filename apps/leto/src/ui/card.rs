//! The stream's pictures drawn by the graphics card, as the window draws
//! everything else.
//!
//! The backend hands pictures over as a decoder gives them, in planes. They
//! go to the card as they are when the window draws, and the card turns
//! them into pixels: the processor no longer does, picture after picture,
//! and less than half the bytes go to the card. Where the card cannot,
//! [`OnCard`] does not exist and the stream screen makes pixels as before.
//!
//! In a browser the page shows the pictures itself, behind the window.

#[cfg(not(target_arch = "wasm32"))]
pub use native::OnCard;

/// Nothing in a browser.
#[cfg(target_arch = "wasm32")]
#[derive(Clone)]
pub enum OnCard {}

#[cfg(target_arch = "wasm32")]
impl OnCard {
    pub fn paint(&self, _place: egui::Rect) -> egui::Shape {
        match *self {}
    }

    pub fn keeps(&self) -> bool {
        match *self {}
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::{
        sync::{
            Arc, Mutex, MutexGuard, PoisonError,
            atomic::{AtomicBool, Ordering},
        },
        time::{Duration, Instant},
    };

    use eframe::{egui_glow, glow};
    use leto_media::{Card, Planes};

    /// The card, shared between the stream screen, which hands it pictures,
    /// and the drawing, which puts them on it.
    #[derive(Clone)]
    pub struct OnCard {
        inner: Arc<Mutex<Inner>>,
        /// The card turned down what it draws with: the stream screen
        /// makes pixels instead.
        failed: Arc<AtomicBool>,
        /// The card draws pictures that stayed on it since they were
        /// decoded. Known at the first drawing, and no longer so once
        /// one could not be drawn.
        keeps: Arc<AtomicBool>,
        /// The drawing waits for the screen first, with V-Sync.
        wait: bool,
    }

    #[derive(Default)]
    struct Inner {
        /// Made at the first drawing, on the thread that draws.
        card: Option<Card>,
        /// The picture to put on the card at the next drawing.
        pending: Option<Planes>,
        /// The drawings since the log was last told of them: how many, how
        /// long they took in all, and when it was told.
        drawn: u32,
        drawing: Duration,
        told: Option<Instant>,
    }

    /// How often the log is told how long drawing a picture takes.
    const TOLD_EVERY: Duration = Duration::from_secs(10);

    impl OnCard {
        /// A card for a window that draws with `gl`, if it can draw
        /// pictures from planes. With `vsync`, each drawing of a picture
        /// waits until the one before is on the screen, so that none waits
        /// behind others there.
        pub fn new(gl: Option<&glow::Context>, vsync: bool) -> Option<Self> {
            let gl = gl?;
            if !Card::fits(gl) {
                log::info!("the graphics card draws no pictures from planes: the processor does");
                return None;
            }
            Some(Self {
                inner: Arc::default(),
                failed: Arc::default(),
                keeps: Arc::default(),
                wait: vsync,
            })
        }

        /// Whether the card draws the pictures.
        pub fn works(&self) -> bool {
            !self.failed.load(Ordering::Relaxed)
        }

        /// Whether the card draws pictures that stayed on it since they
        /// were decoded: the decoder may leave them there.
        pub fn keeps(&self) -> bool {
            self.works() && self.keeps.load(Ordering::Relaxed)
        }

        /// A picture that stayed on the card could not be drawn: the next
        /// ones come through memory, as before, until Leto starts again.
        fn keeps_none(&self, error: &leto_media::Error) {
            if self.keeps.swap(false, Ordering::Relaxed) {
                log::warn!(
                    "pictures do not stay on the graphics card, and come through memory as \
                     before: {error}"
                );
            }
        }

        /// Shows `planes` from the next drawing on. A picture not drawn yet
        /// is replaced: the newest wins.
        pub fn show(&self, planes: Planes) {
            self.lock().pending = Some(planes);
        }

        /// What draws the picture shown last at `place`.
        pub fn paint(&self, place: egui::Rect) -> egui::Shape {
            let this = self.clone();
            let callback = egui_glow::CallbackFn::new(move |_, painter| this.draw(painter.gl()));
            egui::Shape::Callback(egui::PaintCallback { rect: place, callback: Arc::new(callback) })
        }

        /// Takes off the card what it drew with. Called once, as the app
        /// ends.
        pub fn destroy(&self, gl: &glow::Context) {
            if let Some(card) = self.lock().card.take() {
                card.destroy(gl);
            }
        }

        fn draw(&self, gl: &glow::Context) {
            if !self.works() {
                return;
            }
            if self.wait {
                Card::wait(gl);
            }
            let mut inner = self.lock();
            let Inner { card, pending, drawn, drawing, told } = &mut *inner;
            if card.is_none() {
                match Card::new(gl) {
                    Ok(made) => {
                        log::info!("the graphics card makes the pictures into pixels");
                        let keeps = Card::keeps(gl);
                        log::debug!(
                            "pictures decoded on the graphics card {} stay there to be drawn",
                            if keeps { "can" } else { "cannot" }
                        );
                        self.keeps.store(keeps, Ordering::Relaxed);
                        *card = Some(made);
                    }
                    Err(error) => {
                        log::warn!("the processor makes the pictures into pixels: {error}");
                        self.failed.store(true, Ordering::Relaxed);
                        return;
                    }
                }
            }
            let Some(card) = card else { return };
            let start = Instant::now();
            if let Some(planes) = pending.take()
                && let Err(error) = card.show(gl, &planes)
            {
                self.keeps_none(&error);
            }
            if let Err(error) = card.draw(gl) {
                self.keeps_none(&error);
            }
            // What the window's side of a picture costs, which the
            // statistics of decoding do not see.
            *drawn += 1;
            *drawing += start.elapsed();
            let since = *told.get_or_insert(start);
            if since.elapsed() >= TOLD_EVERY {
                log::debug!(
                    "the window drew {drawn} times in {:.0} s, giving the card each drawing in \
                     {:.2} ms",
                    since.elapsed().as_secs_f64(),
                    drawing.as_secs_f64() * 1000.0 / f64::from(*drawn)
                );
                (*drawn, *drawing, *told) = (0, Duration::ZERO, Some(Instant::now()));
            }
        }

        fn lock(&self) -> MutexGuard<'_, Inner> {
            self.inner.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }
}
