//! Cut, copy and paste in Leto's own text fields.
//!
//! eframe can turn Ctrl+C, Ctrl+X and Ctrl+V into clipboard events itself,
//! but it then keeps those keys from everything else, and a stream needs
//! them: copying and pasting on the PC is done with the same keys. So eframe
//! is told to leave them alone, and this does its work whenever no stream
//! has the keyboard.

use egui::{Event, Key, Modifiers, RawInput};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Cut,
    Copy,
    Paste,
}

/// The clipboard command a key press means, by the rules eframe follows.
fn command(key: Key, modifiers: Modifiers) -> Option<Command> {
    let windows = cfg!(target_os = "windows");
    let with_command = |letter| modifiers.command_only() && key == letter;
    if key == Key::Cut || with_command(Key::X) || (windows && modifiers.shift && key == Key::Delete)
    {
        Some(Command::Cut)
    } else if key == Key::Copy
        || with_command(Key::C)
        || (windows && modifiers.ctrl && key == Key::Insert)
    {
        Some(Command::Copy)
    } else if key == Key::Paste
        || with_command(Key::V)
        || (windows && modifiers.shift && key == Key::Insert)
    {
        Some(Command::Paste)
    } else {
        None
    }
}

/// Replaces the key presses that mean cut, copy or paste with the events
/// text fields understand.
pub fn shortcuts(raw: &mut RawInput) {
    let means_command = |event: &Event| match event {
        Event::Key { key, pressed: true, modifiers, .. } => command(*key, *modifiers),
        _ => None,
    };
    if !raw.events.iter().any(|event| means_command(event).is_some()) {
        return;
    }
    let events = std::mem::take(&mut raw.events);
    for event in events {
        match means_command(&event) {
            None => raw.events.push(event),
            Some(Command::Cut) => raw.events.push(Event::Cut),
            Some(Command::Copy) => raw.events.push(Event::Copy),
            Some(Command::Paste) => raw.events.extend(pasted().map(Event::Paste)),
        }
    }
}

/// The text on the clipboard, when there is some.
#[cfg(not(target_os = "android"))]
pub fn pasted() -> Option<String> {
    let text = arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text());
    match text {
        Ok(text) if !text.is_empty() => Some(text.replace("\r\n", "\n")),
        Ok(_) => None,
        Err(error) => {
            log::debug!("nothing to paste: {error}");
            None
        }
    }
}

/// The text on the clipboard, when there is some: Android's is reached
/// through Java.
#[cfg(target_os = "android")]
pub fn pasted() -> Option<String> {
    leto_system::clipboard::text().map(|text| text.replace("\r\n", "\n"))
}

/// Puts `text` on the clipboard: the system's own where egui cannot reach
/// it, egui's elsewhere.
pub fn copy(ctx: &egui::Context, text: String) {
    if !leto_system::clipboard::copy(&text) {
        ctx.copy_text(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(key: Key, modifiers: Modifiers) -> Event {
        Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
    }

    #[test]
    fn the_usual_keys_mean_cut_copy_and_paste() {
        assert_eq!(command(Key::X, Modifiers::COMMAND), Some(Command::Cut));
        assert_eq!(command(Key::C, Modifiers::COMMAND), Some(Command::Copy));
        assert_eq!(command(Key::V, Modifiers::COMMAND), Some(Command::Paste));
        assert_eq!(command(Key::Copy, Modifiers::NONE), Some(Command::Copy));
        // A plain letter, or a shortcut with more held, is something else.
        assert_eq!(command(Key::C, Modifiers::NONE), None);
        assert_eq!(command(Key::C, Modifiers::COMMAND | Modifiers::SHIFT), None);
        assert_eq!(command(Key::A, Modifiers::COMMAND), None);
    }

    #[test]
    fn only_the_press_of_a_clipboard_key_is_replaced() {
        let release = Event::Key {
            key: Key::C,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        };
        let mut raw = RawInput {
            events: vec![
                press(Key::A, Modifiers::NONE),
                press(Key::C, Modifiers::COMMAND),
                release.clone(),
                press(Key::X, Modifiers::COMMAND),
            ],
            ..Default::default()
        };
        shortcuts(&mut raw);
        assert_eq!(raw.events, [press(Key::A, Modifiers::NONE), Event::Copy, release, Event::Cut]);
    }
}
