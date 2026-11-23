//! A voice that arrives on a PC, played where the apps of that PC hear a
//! microphone.
//!
//! Windows has no way for a program to make a microphone: a sound device
//! comes from a driver. A virtual cable is such a driver, which shows two
//! devices: a sound output, and a microphone that hears whatever is played
//! on that output. Leto plays the voice of a device on the output, and the
//! apps of the PC pick the microphone.

use crate::{Error, endpoints, sound::SoundDecoder, speaker::Speaker, voice::VOICE_RATE};

/// The cables Leto knows: part of the name of their sound output, and the
/// microphone the apps of the PC pick to hear it.
const KNOWN: [(&str, &str); 2] = [
    // VB-CABLE, free, from vb-audio.com.
    ("CABLE Input", "CABLE Output (VB-Audio Virtual Cable)"),
    // Comes with Steam's Remote Play.
    ("Steam Streaming Microphone", "Microphone (Steam Streaming Microphone)"),
];

/// A cable of this PC: the name of its sound output, and the name of the
/// microphone the apps of the PC hear it by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub output: String,
    pub microphone: String,
}

/// The first cable Leto knows among the sound outputs of this PC.
pub fn find() -> Option<Found> {
    among(&endpoints::speakers())
}

/// The first cable Leto knows among `outputs`.
pub fn among(outputs: &[String]) -> Option<Found> {
    KNOWN.iter().find_map(|(part, microphone)| {
        let output = outputs.iter().find(|name| holds(name, part))?;
        Some(Found { output: output.clone(), microphone: (*microphone).to_owned() })
    })
}

/// Whether `name` holds `part`, whatever their case: a system or a driver
/// may spell a device's name its own way.
fn holds(name: &str, part: &str) -> bool {
    name.to_lowercase().contains(&part.to_lowercase())
}

/// The sound output of this name as a cable, when the PC has it: one the
/// person chose, which Leto may not know.
pub fn named(output: &str) -> Option<Found> {
    endpoints::speakers().iter().any(|name| name == output).then(|| chosen(output))
}

/// The output of this name as a cable, heard by the microphone that goes
/// with it when Leto knows the cable, and by a name that says so when not.
fn chosen(output: &str) -> Found {
    let microphone = KNOWN
        .iter()
        .find(|(part, _)| holds(output, part))
        .map_or_else(|| format!("the microphone of {output}"), |(_, mic)| (*mic).to_owned());
    Found { output: output.to_owned(), microphone }
}

/// A voice on its way into a cable.
pub struct Cable {
    decoder: SoundDecoder,
    speaker: Speaker,
    stereo: Vec<f32>,
}

impl Cable {
    /// Opens the sound output of this name, which has to be there: a voice
    /// must never come out of the speakers of the PC instead.
    pub fn open(output: &str) -> Result<Self, Error> {
        if !endpoints::speakers().iter().any(|name| name == output) {
            return Err(Error::Speaker(format!("the sound output \"{output}\" is not there")));
        }
        Ok(Self {
            decoder: SoundDecoder::new(VOICE_RATE, 1, 1, 0, &[0])?,
            speaker: Speaker::open(VOICE_RATE, Some(output))?,
            stereo: Vec::new(),
        })
    }

    /// Plays one Opus frame of a voice: one channel at 48 kHz. Returns how
    /// loud it was, from 0 to 1, or `None` once the cable is gone: it is
    /// not looked for again, since what would be found by its name could be
    /// the speakers.
    pub fn play(&mut self, frame: &[u8]) -> Option<f32> {
        if self.speaker.is_lost() {
            return None;
        }
        let Ok(samples) = self.decoder.decode(frame) else {
            // Damaged on the way: a moment of silence.
            return Some(0.0);
        };
        let loudest = samples.iter().fold(0.0f32, |loudest, sample| loudest.max(sample.abs()));
        self.stereo.clear();
        self.stereo.extend(samples.iter().flat_map(|sample| [*sample, *sample]));
        self.speaker.play(&self.stereo);
        Some(loudest.min(1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cable_is_known_by_its_output_and_heard_by_its_microphone() {
        let outputs =
            |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect::<Vec<_>>();
        assert_eq!(among(&outputs(&["Speakers (Realtek Audio)"])), None);
        // The speakers Steam and Apollo add are no microphone.
        assert_eq!(among(&outputs(&["Speakers (Steam Streaming Speakers)"])), None);
        assert_eq!(
            among(&outputs(&[
                "Speakers (Realtek Audio)",
                "Speakers (Steam Streaming Microphone)",
                "CABLE Input (VB-Audio Virtual Cable)",
            ])),
            Some(Found {
                output: "CABLE Input (VB-Audio Virtual Cable)".to_owned(),
                microphone: "CABLE Output (VB-Audio Virtual Cable)".to_owned(),
            })
        );
        assert_eq!(
            among(&outputs(&["Speakers (Steam Streaming Microphone)"]))
                .map(|found| found.microphone),
            Some("Microphone (Steam Streaming Microphone)".to_owned())
        );
        // Whatever the case of the name.
        assert_eq!(
            among(&outputs(&["Haut-parleurs (STEAM Streaming Microphone)"]))
                .map(|found| found.microphone),
            Some("Microphone (Steam Streaming Microphone)".to_owned())
        );
    }

    #[test]
    fn a_chosen_output_is_heard_by_its_own_microphone_when_leto_knows_it() {
        assert_eq!(
            chosen("Haut-parleurs (Steam Streaming Microphone)").microphone,
            "Microphone (Steam Streaming Microphone)"
        );
        assert_eq!(
            chosen("Voicemeeter Input (VB-Audio Voicemeeter VAIO)").microphone,
            "the microphone of Voicemeeter Input (VB-Audio Voicemeeter VAIO)"
        );
    }
}
