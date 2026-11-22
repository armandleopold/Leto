//! What does the work the interface asks for.
//!
//! The desktop app runs the client engine itself, on worker threads. A
//! browser page cannot open sockets, so the web build asks the bridge that
//! served the page, which runs the same engine next to the PC.

// The rules of the web build that need no browser, tested on the desktop.
#[cfg(any(target_arch = "wasm32", test))]
mod flow;
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
mod gamepad;
// Android tells of gamepads in the window's input events.
#[cfg(target_os = "android")]
#[path = "phone_pads.rs"]
mod gamepad;
#[cfg(not(target_arch = "wasm32"))]
mod leave;
#[cfg(not(target_arch = "wasm32"))]
mod native;
mod pads;
#[cfg(not(target_arch = "wasm32"))]
mod stream;
#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(not(target_arch = "wasm32"))]
pub use native::Backend;
#[cfg(target_arch = "wasm32")]
pub use web::Backend;

pub use pads::Press;

/// The cameras, microphones and sound outputs of this device, by the names
/// the system gives them, for the person to choose from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceNames {
    /// `None` until the cameras were looked at, which takes a moment.
    pub cameras: Option<Vec<String>>,
    pub microphones: Vec<String>,
    pub speakers: Vec<String>,
}

/// The test sound of the sound output, as far as it got.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
// A browser plays no test sound: the system chooses where its sound goes.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub enum Chime {
    #[default]
    Silent,
    Playing,
    /// It could not play, in words for the person.
    Failed(String),
}

/// Covers are shown small: anything wider is scaled down to this.
const COVER_WIDTH: u32 = 360;

/// Whether the backend takes requests, and what is in the way when it does
/// not.
#[derive(Debug, Clone, PartialEq, Eq)]
// The desktop app has its engine or it does not: nothing to wait for, and
// nobody to ask for a code.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub enum Door {
    /// The backend takes requests.
    Open,
    /// The page is reaching its bridge. `failed` once an attempt has: the
    /// bridge is not answering.
    Opening { failed: bool },
    /// The bridge does not know this browser, and wants the code it shows
    /// where it runs.
    Code {
        /// The code last given was not the one.
        refused: bool,
        /// The bridge is looking at a code.
        checking: bool,
    },
    /// The backend cannot work at all.
    Closed(Unavailable),
}

/// Why the backend can do nothing, in words for the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unavailable {
    pub title: &'static str,
    pub explanation: String,
}

/// A picture of the stream, as the backend hands it to the interface.
// Each build has one way to show pictures, and never makes the other.
#[allow(dead_code)]
pub enum Picture {
    /// A picture as the decoder gave it, for the window to draw: the
    /// graphics card makes it into pixels as it does.
    #[cfg(not(target_arch = "wasm32"))]
    Planes(leto_media::Planes),
    /// The backend shows the pictures itself, behind the window, and only
    /// says how large they are. The interface leaves a hole for them and
    /// says where it is with `Backend::place_picture`.
    Behind([usize; 2]),
}

/// How often the person sees the pictures of a stream, which is how many of
/// them are worth making into pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
// A browser slows down a page that is out of sight by itself.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub enum Pace {
    /// Every picture: the window has the person's attention.
    #[default]
    Full,
    /// One picture a second: another window has it, and this one is only
    /// glanced at.
    Glance,
    /// None: the window is out of sight.
    Hidden,
}

/// What the camera of this device shows, as the backend hands it to the
/// interface that tries the camera out.
pub struct Seen {
    pub picture: egui::ColorImage,
    /// The size the camera captures at. The picture may be smaller.
    pub size: [usize; 2],
}

/// How the camera of this device is doing on its way to a PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filming {
    /// The camera is opening, or the PC is being asked: nothing leaves yet.
    Opening,
    /// Pictures of this width and height leave for the PC.
    Sending([u32; 2]),
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
