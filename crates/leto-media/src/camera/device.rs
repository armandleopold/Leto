//! A camera of the system, read through what the system has for cameras.

use nokhwa::{
    NokhwaError, native_api_backend, query,
    utils::{
        CameraFormat, CameraInfo, FrameFormat, RequestedFormat, RequestedFormatType, Resolution,
    },
};

use super::{
    Source,
    format::{self, Offer},
    pixels::{Layout, Raw},
};
use crate::Error;

/// The layouts Leto reads, as the system's side calls them.
const READ: &[FrameFormat] =
    &[FrameFormat::YUYV, FrameFormat::NV12, FrameFormat::MJPEG, FrameFormat::GRAY];

pub(super) struct Device {
    camera: nokhwa::Camera,
    layout: Layout,
    width: usize,
    height: usize,
}

impl Device {
    /// Opens the camera called `name`, or the first camera of the system
    /// that has colour, or the first one when none has: laptops that
    /// recognise a face list a second camera for it, which sees in grey.
    pub(super) fn open(name: Option<&str>) -> Result<Self, Error> {
        let listed = list()?;
        super::remember(named(&listed));

        let wanted = name.and_then(|name| {
            let found = listed.iter().find(|info| info.human_name() == name);
            if found.is_none() {
                log::info!("the camera \"{name}\" is not there: Leto takes another");
            }
            found
        });
        let mut grey = None;
        let mut failure = None;
        let mut chosen = None;
        if let Some(info) = wanted {
            // The one the person chose, even in grey: what goes wrong with
            // it is said, not hidden behind another camera.
            chosen = Some(Self::prepare(info)?);
        }
        for info in listed.iter().filter(|_| chosen.is_none()) {
            match Self::prepare(info) {
                Ok(device) if device.layout != Layout::Gray => {
                    chosen = Some(device);
                    break;
                }
                Ok(device) => {
                    grey.get_or_insert(device);
                }
                Err(error) => {
                    failure.get_or_insert(error);
                }
            }
        }
        let mut device = match (chosen.or(grey), failure) {
            (Some(device), _) => device,
            (None, Some(failure)) => return Err(failure),
            (None, None) => return Err(Error::Camera("this device has no camera".to_owned())),
        };
        device.camera.open_stream().map_err(|error| not_opened(&error))?;
        let format = device.camera.camera_format();
        log::info!(
            "the camera \"{}\" captures {}x{} at {} a second as {}",
            device.camera.info().human_name(),
            format.width(),
            format.height(),
            format.frame_rate(),
            format.format(),
        );
        Ok(device)
    }

    /// Gets a camera ready in the format Leto captures in, without starting
    /// it.
    fn prepare(info: &CameraInfo) -> Result<Self, Error> {
        // Any format Leto reads, to begin with: the one to capture in is
        // chosen once the camera has said which ones it has.
        let any = RequestedFormat::with_formats(RequestedFormatType::None, READ);
        let mut camera =
            nokhwa::Camera::new(info.index().clone(), any).map_err(|error| not_opened(&error))?;
        let offered = camera.compatible_camera_formats().map_err(|error| not_opened(&error))?;
        let offered: Vec<Offer> = offered
            .iter()
            .filter_map(|format| {
                Some(Offer {
                    layout: layout(format.format())?,
                    width: format.width(),
                    height: format.height(),
                    rate: format.frame_rate(),
                })
            })
            .collect();
        if let Some(chosen) = format::choose(&offered) {
            let wanted = CameraFormat::new(
                Resolution::new(chosen.width, chosen.height),
                system_name(chosen.layout),
                chosen.rate,
            );
            if wanted != camera.camera_format() {
                let exactly =
                    RequestedFormat::with_formats(RequestedFormatType::Exact(wanted), READ);
                camera.set_camera_requset(exactly).map_err(|error| not_opened(&error))?;
            }
        }

        let format = camera.camera_format();
        let layout = layout(format.format()).ok_or_else(|| {
            Error::Camera(format!(
                "it sends pictures as {}, which Leto does not read",
                format.format()
            ))
        })?;
        Ok(Self {
            camera,
            layout,
            width: format.width() as usize,
            height: format.height() as usize,
        })
    }
}

impl Source for Device {
    fn next(&mut self) -> Result<Raw<'_>, Error> {
        let data = self.camera.frame_raw().map_err(|error| {
            log::warn!("the camera gave no picture: {error}");
            Error::Camera(
                cause(&error.to_string())
                    .unwrap_or("it stopped, and may have been unplugged")
                    .to_owned(),
            )
        })?;
        Ok(Raw { layout: self.layout, width: self.width, height: self.height, data })
    }
}

/// The names of the system's cameras.
pub(super) fn names() -> Result<Vec<String>, Error> {
    Ok(named(&list()?))
}

/// The names of `listed`, each once: a camera is picked by its name, so of
/// two that share one, the first is used.
fn named(listed: &[CameraInfo]) -> Vec<String> {
    let mut names = Vec::new();
    for name in listed.iter().map(CameraInfo::human_name) {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

fn list() -> Result<Vec<CameraInfo>, Error> {
    let system = native_api_backend()
        .ok_or_else(|| Error::Camera("this system has none that Leto can use".to_owned()))?;
    query(system).map_err(|error| not_opened(&error))
}

fn layout(format: FrameFormat) -> Option<Layout> {
    match format {
        FrameFormat::MJPEG => Some(Layout::Jpeg),
        FrameFormat::YUYV => Some(Layout::Yuyv),
        FrameFormat::NV12 => Some(Layout::Nv12),
        FrameFormat::GRAY => Some(Layout::Gray),
        FrameFormat::RAWRGB | FrameFormat::RAWBGR => None,
    }
}

fn system_name(layout: Layout) -> FrameFormat {
    match layout {
        Layout::Jpeg => FrameFormat::MJPEG,
        Layout::Yuyv => FrameFormat::YUYV,
        Layout::Nv12 => FrameFormat::NV12,
        Layout::Gray => FrameFormat::GRAY,
    }
}

fn not_opened(error: &NokhwaError) -> Error {
    let said = error.to_string();
    Error::Camera(cause(&said).map_or(said.clone(), str::to_owned))
}

/// Why a camera cannot be used, when what the system `said` is one of the
/// reasons a person can do something about.
fn cause(said: &str) -> Option<&'static str> {
    let said = said.to_lowercase();
    let says = |signs: &[&str]| signs.iter().any(|sign| said.contains(sign));
    // The words of Linux and of Windows, and the numbers Windows gives when
    // it has no words.
    if says(&["denied", "0x80070005"]) {
        Some("the system does not let Leto use it. Allow it in the system's privacy settings")
    } else if says(&["busy", "preempted", "0xc00d3704", "0xc00d3ea3"]) {
        Some("another app is using it")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_person_can_do_something_about_is_said_in_their_words() {
        for said in [
            "Could not open device: Access is denied. (0x80070005)",
            "Failed to open /dev/video0: Permission denied (os error 13)",
        ] {
            assert!(cause(said).is_some_and(|why| why.contains("privacy settings")), "{said}");
        }
        for said in [
            "Could not open stream: Device or resource busy (os error 16)",
            "Could not read frame: (0xC00D3704)",
        ] {
            assert_eq!(cause(said), Some("another app is using it"), "{said}");
        }
        assert_eq!(cause("Could not read frame: No such device (os error 19)"), None);
    }

    #[test]
    fn every_layout_leto_reads_has_a_name_on_the_system_side() {
        for format in READ {
            assert_eq!(layout(*format).map(system_name), Some(*format));
        }
        assert_eq!(layout(FrameFormat::RAWRGB), None);
    }
}
