//! The person's leave to use the camera or the microphone, which Android
//! asks for in a question over the app. Elsewhere it is always there.

use std::time::{Duration, Instant};

use leto_system::permission::{self, Need};

/// How long a question that never showed is waited for. Android answers
/// for the person, without showing it, once they said no twice.
const UNSEEN: Duration = Duration::from_secs(5);

/// Leave for one use of the camera or the microphone.
pub(super) struct Leave {
    need: Need,
    asked: Option<Asked>,
}

struct Asked {
    at: Instant,
    /// The question took the window's focus: it is showing, or was.
    showed: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Answer {
    Allowed,
    /// The question is waiting for the person.
    Waiting,
    /// The person said no.
    Refused(String),
}

impl Leave {
    pub(super) fn new(need: Need) -> Self {
        Self { need, asked: None }
    }

    /// Whether it may be used now, asking the person the first time. Asked
    /// every frame while the camera or the microphone is wanted, with
    /// whether the window has the focus; `forget` once it no longer is.
    pub(super) fn check(&mut self, focused: bool) -> Answer {
        if permission::allowed(self.need) {
            self.asked = None;
            return Answer::Allowed;
        }
        let Some(asked) = &mut self.asked else {
            permission::ask(self.need);
            self.asked = Some(Asked { at: Instant::now(), showed: false });
            return Answer::Waiting;
        };
        if !focused {
            asked.showed = true;
            return Answer::Waiting;
        }
        if !asked.showed && asked.at.elapsed() < UNSEEN {
            return Answer::Waiting;
        }
        // Answered, and not yes. Asked again when wanted again.
        self.asked = None;
        let what = match self.need {
            Need::Camera => "the camera",
            Need::Microphone => "the microphone",
        };
        Answer::Refused(format!(
            "Leto may not use {what}. Allow it in the system's settings for Leto, or when it asks."
        ))
    }

    pub(super) fn forget(&mut self) {
        self.asked = None;
    }
}
