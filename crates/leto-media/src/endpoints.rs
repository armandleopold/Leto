//! The system's microphones and speakers, by the names it gives them.
//!
//! A name is what the system shows for a device, such as "Speakers (Realtek
//! Audio)", and what a person picks one by. Leto keeps the name: a device
//! that is gone the next time is replaced by the system's default.

use cpal::traits::{DeviceTrait, HostTrait};

use crate::Error;

/// The names of the microphones the system has.
pub fn microphones() -> Vec<String> {
    names(cpal::default_host().input_devices().ok())
}

/// The names of the sound outputs the system has.
pub fn speakers() -> Vec<String> {
    names(cpal::default_host().output_devices().ok())
}

/// In the system's order, each name once: a device is picked by its name,
/// so of two that share one, the first is used.
fn names(devices: Option<impl Iterator<Item = cpal::Device>>) -> Vec<String> {
    let mut names = Vec::new();
    for name in devices.into_iter().flatten().filter_map(|device| name(&device)) {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

fn name(device: &cpal::Device) -> Option<String> {
    device.description().ok().map(|description| description.name().to_owned())
}

/// The microphone named `wanted`, or the system's default one when it has
/// none by that name or none is wanted.
pub(crate) fn input(wanted: Option<&str>) -> Result<cpal::Device, Error> {
    let host = cpal::default_host();
    if let Some(wanted) = wanted {
        let found = host
            .input_devices()
            .ok()
            .into_iter()
            .flatten()
            .find(|device| name(device).as_deref() == Some(wanted));
        match found {
            Some(device) => return Ok(device),
            None => log::info!("the microphone \"{wanted}\" is not there: the default one is used"),
        }
    }
    host.default_input_device()
        .ok_or_else(|| Error::Microphone("this device has no microphone".to_owned()))
}

/// The sound output named `wanted`, or the system's default one when it has
/// none by that name or none is wanted.
pub(crate) fn output(wanted: Option<&str>) -> Result<cpal::Device, Error> {
    let host = cpal::default_host();
    if let Some(wanted) = wanted {
        let found = host
            .output_devices()
            .ok()
            .into_iter()
            .flatten()
            .find(|device| name(device).as_deref() == Some(wanted));
        match found {
            Some(device) => return Ok(device),
            None => {
                log::info!("the sound output \"{wanted}\" is not there: the default one is used")
            }
        }
    }
    host.default_output_device()
        .ok_or_else(|| Error::Speaker("this device has no sound output".to_owned()))
}
