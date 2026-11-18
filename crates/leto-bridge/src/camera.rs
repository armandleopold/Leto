//! A browser's camera, on its way to the PC that browser is watching.
//!
//! The browser encodes its camera itself and hands over finished pictures.
//! The bridge asks the PC whether it takes them, the way the desktop app
//! does for its own camera, and passes them on over the return channel,
//! sealed (`docs/return-channel.md`). Whether a camera is on is the
//! browser's to say: the bridge never asks a PC for one by itself.

use std::{sync::Arc, time::Duration};

use leto_client::{CameraSettings, Service, Shell};
use leto_core::bridge::{Down, LONGEST_CAMERA_PICTURE};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// What a browser is told when its pictures have nowhere to go any more.
const OVER: &str = "The stream ended, and the camera with it.";

/// What a PC said about a camera: the number the engine gave it, or why the
/// PC does not take it, in words for the person.
pub type Answer = Result<u64, String>;

/// The camera of one browser.
pub struct Camera {
    state: State,
    /// Where the answer of a PC comes back to the browser's link.
    answers: UnboundedSender<Answer>,
}

enum State {
    Off,
    /// The PC is being asked about the camera the browser gave this number,
    /// which takes a moment. One question is on its way at a time: what the
    /// browser says in the meantime waits in `then`.
    Asked {
        number: u32,
        then: Then,
    },
    /// The PC takes it, as the camera of this number of the engine's.
    On {
        number: u32,
        camera: u64,
    },
}

/// What is left to do once a PC has answered.
enum Then {
    /// Tell the browser.
    Tell,
    /// Nothing: the browser no longer wants to send its camera.
    Stop,
    /// Ask again, for what the browser asked since.
    Ask { number: u32, sending: CameraSettings },
}

impl Camera {
    /// A camera that is off, and where the answers of PCs arrive: the
    /// browser's link waits there and hands each to [`Self::answered`].
    pub fn new() -> (Self, UnboundedReceiver<Answer>) {
        let (answers, arriving) = unbounded_channel();
        (Self { state: State::Off, answers }, arriving)
    }

    /// The browser would like to send its camera to the PC it is watching,
    /// like `sending`. What the PC says comes back as an [`Answer`].
    pub fn on<S: Shell>(
        &mut self,
        service: &Arc<Service<S>>,
        number: u32,
        sending: CameraSettings,
    ) {
        match &mut self.state {
            State::Asked { then, .. } => *then = Then::Ask { number, sending },
            State::On { camera, .. } => {
                service.camera_close(*camera);
                self.ask(service, number, sending);
            }
            State::Off => self.ask(service, number, sending),
        }
    }

    /// The browser stopped sending its camera, or no longer wants to.
    pub fn off<S: Shell>(&mut self, service: &Service<S>) {
        match &mut self.state {
            State::Asked { then, .. } => *then = Then::Stop,
            State::On { camera, .. } => {
                service.camera_close(*camera);
                self.state = State::Off;
            }
            State::Off => {}
        }
    }

    /// The PC answered. Returns what to tell the browser, when it still
    /// waits for that answer.
    pub fn answered<S: Shell>(
        &mut self,
        service: &Arc<Service<S>>,
        answer: Answer,
    ) -> Option<Down> {
        let State::Asked { number, then } = std::mem::replace(&mut self.state, State::Off) else {
            // No PC is asked without the state saying so. Should one answer
            // all the same, nobody sends it anything.
            if let Ok(camera) = answer {
                service.camera_close(camera);
            }
            return None;
        };
        match then {
            Then::Tell => {
                if let Ok(camera) = answer {
                    self.state = State::On { number, camera };
                }
                Some(Down::Camera { number, answer: answer.map(drop) })
            }
            Then::Stop => {
                if let Ok(camera) = answer {
                    service.camera_close(camera);
                }
                None
            }
            Then::Ask { number, sending } => {
                // Should the PC refuse what is asked now, it does not go on
                // waiting for what it agreed to before.
                if let Ok(camera) = answer {
                    service.camera_close(camera);
                }
                self.ask(service, number, sending);
                None
            }
        }
    }

    /// Passes one picture of the browser's camera on to the PC that agreed
    /// to take it. Pictures that come when no PC did go nowhere. Returns
    /// what to tell the browser when this one found the camera stopped: the
    /// stream it went with is over.
    pub fn picture<S: Shell>(
        &mut self,
        service: &Service<S>,
        at: u32,
        keyframe: bool,
        data: &[u8],
    ) -> Option<Down> {
        let State::On { number, camera } = self.state else { return None };
        if data.is_empty() || data.len() > LONGEST_CAMERA_PICTURE {
            return None;
        }
        // A picture that did not leave whole is lost: the next one that
        // decodes on its own, two seconds later at most, repairs it.
        if service.camera(camera, Duration::from_millis(at.into()), keyframe, data).is_some() {
            return None;
        }
        self.state = State::Off;
        Some(Down::Camera { number, answer: Err(OVER.to_owned()) })
    }

    /// Asks the PC, on a thread that may wait for it.
    fn ask<S: Shell>(&mut self, service: &Arc<Service<S>>, number: u32, sending: CameraSettings) {
        self.state = State::Asked { number, then: Then::Tell };
        let (service, answers) = (Arc::clone(service), self.answers.clone());
        tokio::task::spawn_blocking(move || {
            // Nobody waits for the answer of a browser that left meanwhile.
            let _ = answers.send(service.camera_open(sending));
        });
    }
}
