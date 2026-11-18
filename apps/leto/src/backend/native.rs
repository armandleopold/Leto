//! The backend of the desktop app: the client engine on worker threads.

use std::sync::{Arc, Mutex, PoisonError, mpsc};

use leto_client::{
    Client, Service, Shell,
    engine::{self, ImageFile},
    input::Input,
    service::{explain, sentence},
};

use super::{
    Picture, Unavailable, gamepad,
    stream::{Outlet, Screen, Sound},
};
use crate::state::{Event, Request};

/// Covers are shown small: anything wider is scaled down to this.
const COVER_WIDTH: u32 = 360;

pub struct Backend {
    /// The engine at work for this window, or why it could not start.
    service: Result<Arc<Service<Desk>>, String>,
    events: mpsc::Receiver<Event>,
}

/// What the engine works for on the desktop: this window.
struct Desk {
    events: mpsc::Sender<Event>,
    ctx: egui::Context,
    /// Where the stream on screen leaves its pictures.
    outlet: Mutex<Arc<Outlet>>,
}

impl Desk {
    fn outlet(&self) -> Arc<Outlet> {
        Arc::clone(&self.outlet.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl Shell for Desk {
    type Video = Screen;
    type Audio = Sound;

    fn tell(&self, event: engine::Event) {
        // Covers are decoded here, on the engine's thread, not while drawing.
        let event = event.with_covers(|file: ImageFile| decode_cover(&file.0));
        // The interface is gone when the app is closing.
        let _ = self.events.send(event);
        self.ctx.request_repaint();
    }

    fn sinks(&self) -> Result<(Screen, Sound), String> {
        let outlet = self.outlet();
        let screen =
            Screen::new(&outlet, &self.ctx).map_err(|error| sentence(&error.to_string()))?;
        Ok((screen, Sound::new(&outlet)))
    }
}

impl Backend {
    pub fn new(ctx: &egui::Context) -> Self {
        let (sender, events) = mpsc::channel();
        let desk = Desk { events: sender, ctx: ctx.clone(), outlet: Mutex::default() };
        let service = Client::open_default()
            .map(|client| Arc::new(Service::new(Arc::new(client), desk)))
            .map_err(|error| explain(&error));
        match &service {
            Ok(service) => {
                // Gamepads go to the stream that is connected, whichever it is.
                let (asked, sent) = (Arc::clone(service), Arc::clone(service));
                gamepad::watch(move || asked.streaming(), move |input| sent.input([input]));
            }
            Err(error) => log::error!("the client engine did not start: {error}"),
        }
        Self { service, events }
    }

    pub fn unavailable(&self) -> Option<Unavailable> {
        let explanation = self.service.as_ref().err()?.clone();
        Some(Unavailable { title: "Leto could not start", explanation })
    }

    /// The news that arrived since the last call.
    pub fn events(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    /// The latest picture of the stream that nobody has shown yet.
    pub fn take_picture(&self) -> Option<Picture> {
        let outlet = self.service.as_ref().ok()?.shell().outlet();
        outlet.take_picture().map(Picture::Pixels)
    }

    /// The window draws the pictures itself: nothing to place.
    pub fn place_picture(&self, _place: Option<egui::Rect>) {}

    /// Why the stream has no sound, when it has none.
    pub fn sound_problem(&self) -> Option<String> {
        let outlet = self.service.as_ref().ok()?.shell().outlet();
        outlet.sound_problem()
    }

    /// Passes what the person did on to the PC being watched, in order.
    pub fn input(&self, inputs: Vec<Input>) {
        if let Ok(service) = &self.service {
            service.input(inputs);
        }
    }

    /// Starts `request` on a worker thread. The outcome arrives as events.
    pub fn send(&self, request: Request) {
        let Ok(service) = &self.service else { return };
        if matches!(request, Request::Play { .. }) {
            // A new stream leaves its pictures in a place of its own: what
            // an earlier one left is not for this one.
            let desk = service.shell();
            *desk.outlet.lock().unwrap_or_else(PoisonError::into_inner) = Arc::default();
        }
        service.ask(request);
    }
}

/// Turns the bytes of an image file into pixels, scaled down for the grid.
fn decode_cover(bytes: &[u8]) -> Option<egui::ColorImage> {
    let image = image::load_from_memory(bytes).ok()?.into_rgba8();
    let image = if image.width() > COVER_WIDTH {
        let height = u64::from(image.height()) * u64::from(COVER_WIDTH) / u64::from(image.width());
        let height = u32::try_from(height).ok()?.max(1);
        image::imageops::resize(&image, COVER_WIDTH, height, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    let size = [usize::try_from(image.width()).ok()?, usize::try_from(image.height()).ok()?];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wide_cover_is_scaled_down_and_a_broken_one_is_dropped() {
        let wide = image::RgbaImage::from_pixel(720, 1080, image::Rgba([255, 107, 53, 255]));
        let mut file = std::io::Cursor::new(Vec::new());
        wide.write_to(&mut file, image::ImageFormat::Png).expect("a PNG can be written to memory");

        let cover = decode_cover(file.get_ref()).expect("a valid PNG decodes");
        assert_eq!(cover.size, [360, 540]);
        assert_eq!(cover.pixels[0], egui::Color32::from_rgb(255, 107, 53));

        assert!(decode_cover(b"not an image").is_none());
    }
}
