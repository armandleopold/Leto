//! The backend of the web build: a placeholder until the bridge exists.

use super::Unavailable;
use leto_core::input::Input;

use crate::state::{Event, Request};

pub struct Backend;

impl Backend {
    pub fn new(_ctx: &egui::Context) -> Self {
        Self
    }

    pub fn unavailable(&self) -> Option<Unavailable> {
        Some(Unavailable {
            title: "The web client is not connected yet",
            explanation: "A browser cannot talk to a PC directly. A small bridge next to the PC \
                          does it, and that bridge is the next part of this step. Until then, \
                          use the desktop app."
                .to_owned(),
        })
    }

    pub fn send(&self, _request: Request) {}

    pub fn input(&self, _inputs: Vec<Input>) {}

    pub fn events(&self) -> Vec<Event> {
        Vec::new()
    }

    pub fn take_picture(&self) -> Option<egui::ColorImage> {
        None
    }

    pub fn sound_problem(&self) -> Option<String> {
        None
    }
}
