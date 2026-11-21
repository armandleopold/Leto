//! The settings, after those of Moonlight: what every stream is like, and
//! how this device's keyboard, mouse and gamepads behave in one. A change
//! counts from the next stream on, and is kept at once.

use egui::{Align, FontId, Layout, Pos2, Rect, RichText, Sense, Stroke, Ui, UiBuilder, vec2};
use leto_core::engine::{BITRATES_KBPS, Codec, Decodable, ScreenMode};

use super::{tunnel, widgets};
use crate::{
    input::{HAS_POINTER, Shortcut},
    settings::{CODECS, Capture, Decoder, FRAME_RATES, RESOLUTIONS, SCALES, Settings, codec_name},
    state::TunnelForm,
    theme,
};

/// What the person asked for in the settings, beyond what they changed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Asked {
    /// To go back to where they were.
    pub back: bool,
    pub tunnel: Option<tunnel::Asked>,
}

/// Widest the settings get, in points: a name stays near its control.
const WIDTH: f32 = 820.0;
/// Under this width a control goes under its name instead of beside it.
const SIDE_BY_SIDE: f32 = 420.0;
const GAP: f32 = 16.0;
/// Height of the row a control sits in.
const CONTROL_HEIGHT: f32 = 36.0;
/// Width of a list to choose from, and of the bit rate's slider.
const LIST_WIDTH: f32 = 190.0;
const SLIDER_WIDTH: f32 = 290.0;
const SWITCH_WIDTH: f32 = 44.0;

/// Choices that only a desktop window has: a page cannot fill the screen
/// before a click, chooses no V-Sync, and gets the system's shortcuts only
/// as its browser lets it.
const DESKTOP: bool = cfg!(not(target_arch = "wasm32"));

/// Draws the settings. `native` is the screen the window is on, when it
/// knows, `decodable` what this device decodes, `tunnel` Leto's own
/// WireGuard tunnel where this device can have one, and `back_to` names
/// where the back button leads.
pub fn show(
    ui: &mut Ui,
    settings: &mut Settings,
    native: Option<ScreenMode>,
    decodable: Decodable,
    tunnel: Option<&mut TunnelForm>,
    back_to: &str,
) -> Asked {
    let mut asked = Asked::default();
    if widgets::back_button(ui, back_to).clicked() {
        asked.back = true;
        return asked;
    }
    ui.add_space(4.0);
    widgets::title(ui, "Settings");
    ui.label(widgets::dim("They count from the next stream on, and Leto keeps them."));
    ui.add_space(12.0);

    let column = Rect::from_min_size(
        ui.cursor().min,
        vec2(WIDTH.min(ui.available_width()), ui.available_height().max(0.0)),
    );
    ui.scope_builder(
        UiBuilder::new().max_rect(column).layout(Layout::top_down(Align::Min)),
        |ui| {
            picture_and_sound(ui, settings, native);
            hands_and_more(ui, settings, decodable);
            if let Some(form) = tunnel {
                card(ui, "WireGuard tunnel", |ui| asked.tunnel = tunnel::show(ui, form));
            }
            ui.add_space(4.0);
            let defaults = settings.defaults();
            let changed = *settings != defaults;
            let reset =
                ui.add_enabled_ui(changed, |ui| widgets::quiet_button(ui, "Restore the defaults"));
            if reset.inner.clicked() {
                *settings = defaults;
            }
        },
    );
    asked
}

fn picture_and_sound(ui: &mut Ui, settings: &mut Settings, native: Option<ScreenMode>) {
    card(ui, "Video", |ui| {
        let screen = native.map(|native| format!("{} × {}", native.width, native.height));
        let about = screen.as_ref().map_or_else(
            || "Native is this screen's.".to_owned(),
            |size| format!("Native is this screen's: {size}."),
        );
        row(ui, "Resolution", &about, LIST_WIDTH, |ui| {
            let shown = |size: Option<[u32; 2]>| match size {
                None => "Native".to_owned(),
                Some([width, height]) => resolution_name(width, height),
            };
            egui::ComboBox::from_id_salt("resolution")
                .width(LIST_WIDTH)
                .selected_text(shown(settings.resolution))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut settings.resolution, None, shown(None));
                    for size in RESOLUTIONS {
                        ui.selectable_value(
                            &mut settings.resolution,
                            Some(size),
                            shown(Some(size)),
                        );
                    }
                });
        });

        if settings.resolution.is_none() {
            let about = match settings.screen(native) {
                Some(mode) if settings.scale != 100 => format!(
                    "Text and windows on the PC are larger, and the stream is {} × {}.",
                    mode.width, mode.height
                ),
                _ => {
                    "Text and windows on the PC are larger, with fewer pixels to stream.".to_owned()
                }
            };
            row(ui, "Scale", &about, LIST_WIDTH, |ui| {
                egui::ComboBox::from_id_salt("scale")
                    .width(LIST_WIDTH)
                    .selected_text(format!("{}%", settings.scale))
                    .show_ui(ui, |ui| {
                        for scale in SCALES {
                            ui.selectable_value(&mut settings.scale, scale, format!("{scale}%"));
                        }
                    });
            });
        }

        let about = native.map_or_else(
            || "Native is 60 FPS: this window does not know its screen's.".to_owned(),
            |native| format!("Native is this screen's: {} FPS.", native.refresh_hz),
        );
        row(ui, "Frame rate", &about, LIST_WIDTH, |ui| {
            let shown = |fps: Option<u32>| {
                fps.map_or_else(|| "Native".to_owned(), |fps| format!("{fps} FPS"))
            };
            egui::ComboBox::from_id_salt("frame rate")
                .width(LIST_WIDTH)
                .selected_text(shown(settings.frame_rate))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut settings.frame_rate, None, shown(None));
                    for fps in FRAME_RATES {
                        ui.selectable_value(&mut settings.frame_rate, Some(fps), shown(Some(fps)));
                    }
                });
        });

        let automatic = settings.automatic_bitrate(native);
        let mut chosen_by_leto = settings.bitrate_kbps.is_none();
        let about =
            format!("Leto asks for {} for this resolution and frame rate.", megabits(automatic));
        row(ui, "Automatic bitrate", &about, SWITCH_WIDTH, |ui| {
            if widgets::switch(ui, &mut chosen_by_leto, "Automatic bitrate").changed() {
                settings.bitrate_kbps = (!chosen_by_leto).then_some(automatic);
            }
        });
        if let Some(kbps) = &mut settings.bitrate_kbps {
            let about = "More is sharper and needs a faster network.";
            row(ui, "Video bitrate", about, SLIDER_WIDTH, |ui| {
                // The rail, then the number, and the rail shows on a card.
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.spacing_mut().slider_width = SLIDER_WIDTH - 130.0;
                    ui.visuals_mut().widgets.inactive.bg_fill = theme::ground();
                    let mut mbps = f64::from(*kbps) / 1000.0;
                    let range =
                        f64::from(BITRATES_KBPS.0) / 1000.0..=f64::from(BITRATES_KBPS.1) / 1000.0;
                    let slider = egui::Slider::new(&mut mbps, range)
                        .logarithmic(true)
                        .trailing_fill(true)
                        .fixed_decimals(1)
                        .suffix(" Mbit/s");
                    if ui.add(slider).changed() {
                        *kbps = ((mbps * 1000.0).round() as u32)
                            .clamp(BITRATES_KBPS.0, BITRATES_KBPS.1);
                    }
                });
            });
        }

        if DESKTOP {
            let about = format!(
                "{} switches between the full screen and a window.",
                Shortcut::FullScreen.keys()
            );
            row(ui, "Full screen when a stream starts", &about, SWITCH_WIDTH, |ui| {
                widgets::switch(ui, &mut settings.full_screen, "Full screen when a stream starts");
            });
            let about = "Shows whole pictures only. Off can win a few milliseconds. Counts from Leto's next start.";
            row(ui, "V-Sync", about, SWITCH_WIDTH, |ui| {
                widgets::switch(ui, &mut settings.vsync, "V-Sync");
            });
        }
        let about = "Apollo then makes a screen with the stream's resolution and frame rate. Other PCs ignore this.";
        row(ui, "A screen of its own on the PC", about, SWITCH_WIDTH, |ui| {
            widgets::switch(ui, &mut settings.virtual_display, "A screen of its own on the PC");
        });
    });

    card(ui, "Audio", |ui| {
        row(
            ui,
            "Mute the PC's speakers",
            "The sound plays on this device only.",
            SWITCH_WIDTH,
            |ui| {
                widgets::switch(ui, &mut settings.mute_pc, "Mute the PC's speakers");
            },
        );
        let about = "Leto is silent while another window is in front.";
        row(ui, "Mute Leto in the background", about, SWITCH_WIDTH, |ui| {
            widgets::switch(ui, &mut settings.mute_in_background, "Mute Leto in the background");
        });
    });

    card(ui, "PC", |ui| {
        let about = "The PC may change a game's settings to suit the stream.";
        row(ui, "Optimize game settings", about, SWITCH_WIDTH, |ui| {
            widgets::switch(ui, &mut settings.optimize_games, "Optimize game settings");
        });
        let about = "Leaving a stream closes its app on the PC, instead of leaving it running.";
        row(ui, "Quit the app when leaving", about, SWITCH_WIDTH, |ui| {
            widgets::switch(ui, &mut settings.quit_on_leave, "Quit the app when leaving");
        });
    });
}

fn hands_and_more(ui: &mut Ui, settings: &mut Settings, decodable: Decodable) {
    card(ui, "Keyboard and mouse", |ui| {
        if DESKTOP {
            let about = format!(
                "The Windows key, Alt+Tab and the system's other shortcuts go to the PC. {} takes them back.",
                Shortcut::Release.keys()
            );
            row(ui, "Capture system shortcuts", &about, LIST_WIDTH, |ui| {
                egui::ComboBox::from_id_salt("capture")
                    .width(LIST_WIDTH)
                    .selected_text(settings.capture_shortcuts.name())
                    .show_ui(ui, |ui| {
                        for capture in Capture::ALL {
                            ui.selectable_value(
                                &mut settings.capture_shortcuts,
                                capture,
                                capture.name(),
                            );
                        }
                    });
            });
        }
        // A phone has no pointer of its own to show: the PC keeps drawing its
        // own there, and there is nothing to choose.
        if HAS_POINTER {
            let about = format!(
                "The pointer moves here at once and the PC stops drawing its own. It keeps the \
                 shape of an arrow. Off, the PC draws its pointer in the picture. {} switches \
                 during a stream.",
                Shortcut::Pointer.keys()
            );
            row(ui, "This device's pointer", &about, SWITCH_WIDTH, |ui| {
                widgets::switch(ui, &mut settings.local_pointer, "This device's pointer");
            });
        }
        let about = "The left button clicks right on the PC, and the right one left.";
        row(ui, "Swap the mouse buttons", about, SWITCH_WIDTH, |ui| {
            widgets::switch(ui, &mut settings.swap_mouse_buttons, "Swap the mouse buttons");
        });
        row(
            ui,
            "Reverse scrolling",
            "The wheel scrolls the other way on the PC.",
            SWITCH_WIDTH,
            |ui| {
                widgets::switch(ui, &mut settings.reverse_scroll, "Reverse scrolling");
            },
        );
    });

    card(ui, "Gamepads", |ui| {
        let about = "For gamepads that have A on the right and B at the bottom.";
        row(ui, "Swap A and B, X and Y", about, SWITCH_WIDTH, |ui| {
            widgets::switch(ui, &mut settings.swap_gamepad_buttons, "Swap A and B, X and Y");
        });
    });

    card(ui, "Advanced", |ui| {
        // A browser decodes as it sees fit, and gets H.264 from the bridge.
        if DESKTOP {
            decoding(ui, settings, decodable);
        }
        // Only Windows decodes on the graphics card for Leto.
        if cfg!(windows) {
            let about = "Being tried. Pictures decoded on the graphics card are drawn from where \
                         they are instead of coming through memory first, which takes less time. \
                         Turn it off if the picture turns black or wrong.";
            row(ui, "Pictures stay on the graphics card", about, SWITCH_WIDTH, |ui| {
                let name = "Pictures stay on the graphics card";
                widgets::switch(ui, &mut settings.keep_on_card, name);
            });
            let about = "Being tried. The graphics card that decodes the pictures shows them \
                         itself, with Direct3D 11 as Moonlight does, without waiting for the \
                         window to draw. Turn it off if the picture turns black or wrong.";
            row(ui, "Pictures go straight to the screen", about, SWITCH_WIDTH, |ui| {
                let name = "Pictures go straight to the screen";
                widgets::switch(ui, &mut settings.straight_to_screen, name);
            });
        }
        let about = format!("{} shows or hides them during a stream.", Shortcut::Statistics.keys());
        row(ui, "Statistics from the start", &about, SWITCH_WIDTH, |ui| {
            widgets::switch(ui, &mut settings.statistics, "Statistics from the start");
        });
    });
}

/// Who decodes the pictures, and in which codec.
fn decoding(ui: &mut Ui, settings: &mut Settings, decodable: Decodable) {
    let about = "Automatic decodes on the graphics card when it can. Software leaves it to the \
                 processor, which takes more of it.";
    row(ui, "Video decoder", about, LIST_WIDTH, |ui| {
        egui::ComboBox::from_id_salt("decoder")
            .width(LIST_WIDTH)
            .selected_text(settings.decoder.name())
            .show_ui(ui, |ui| {
                for decoder in Decoder::ALL {
                    ui.selectable_value(&mut settings.decoder, decoder, decoder.name());
                }
            });
    });
    row(ui, "Video codec", &codec_about(decodable), LIST_WIDTH, |ui| {
        egui::ComboBox::from_id_salt("codec")
            .width(LIST_WIDTH)
            .selected_text(codec_name(settings.codec))
            .show_ui(ui, |ui| {
                for codec in CODECS {
                    let decodes = codec.is_none_or(|codec| decodable.any.contains(codec));
                    let choice = ui.add_enabled_ui(decodes, |ui| {
                        ui.selectable_value(&mut settings.codec, codec, codec_name(codec))
                    });
                    choice.inner.on_disabled_hover_text("This device does not decode it.");
                }
            });
    });
}

/// What the automatic codec is on this device.
fn codec_about(decodable: Decodable) -> String {
    let better: Vec<_> = decodable
        .hardware
        .best_first()
        .filter(|codec| *codec != Codec::H264)
        .map(Codec::name)
        .collect();
    if better.is_empty() {
        "Automatic is H.264: the graphics card here decodes neither AV1 nor H.265 for Leto."
            .to_owned()
    } else {
        format!(
            "Automatic asks the PC for {}, which the graphics card decodes, or else H.264.",
            better.join(" or ")
        )
    }
}

/// A picture size as people know it.
fn resolution_name(width: u32, height: u32) -> String {
    let short = match [width, height] {
        [1280, 720] => " (720p)",
        [1920, 1080] => " (1080p)",
        [2560, 1440] => " (1440p)",
        [3840, 2160] => " (4K)",
        _ => "",
    };
    format!("{width} × {height}{short}")
}

fn megabits(kbps: u32) -> String {
    let mbps = f64::from(kbps) / 1000.0;
    if mbps.fract() == 0.0 { format!("{mbps:.0} Mbit/s") } else { format!("{mbps:.1} Mbit/s") }
}

/// A group of settings, under its name.
fn card(ui: &mut Ui, name: &str, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(theme::surface())
        .stroke(Stroke::new(1.0, theme::line()))
        .corner_radius(theme::CARD_RADIUS)
        .inner_margin(egui::Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(name).size(18.0));
            ui.add_space(2.0);
            add(ui);
        });
    ui.add_space(12.0);
}

/// One setting: its name and what it does, with the control that changes it
/// on the right, or under them in a narrow window.
fn row(ui: &mut Ui, name: &str, about: &str, control_width: f32, control: impl FnOnce(&mut Ui)) {
    let width = ui.available_width();
    let beside = width >= SIDE_BY_SIDE;
    let text_width = if beside { (width - control_width - GAP).max(80.0) } else { width };
    let name = ui.painter().layout(
        name.to_owned(),
        FontId::proportional(theme::BODY),
        theme::text(),
        text_width,
    );
    let about = ui.painter().layout(
        about.to_owned(),
        FontId::proportional(theme::SMALL),
        theme::dim(),
        text_width,
    );
    let text_height = name.size().y + 2.0 + about.size().y;
    let height = if beside { text_height.max(CONTROL_HEIGHT) } else { text_height };
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let top = rect.top() + (height - text_height) / 2.0;
    let below = top + name.size().y + 2.0;
    ui.painter().galley(Pos2::new(rect.left(), top), name, theme::text());
    ui.painter().galley(Pos2::new(rect.left(), below), about, theme::dim());

    let place = if beside {
        Rect::from_min_size(
            Pos2::new(rect.right() - control_width, rect.center().y - CONTROL_HEIGHT / 2.0),
            vec2(control_width, CONTROL_HEIGHT),
        )
    } else {
        ui.allocate_exact_size(vec2(width, CONTROL_HEIGHT), Sense::hover()).0
    };
    // The row took its room already: the control goes in it.
    let layout = if beside {
        Layout::right_to_left(Align::Center)
    } else {
        Layout::left_to_right(Align::Center)
    };
    let mut inside = ui.new_child(UiBuilder::new().max_rect(place).layout(layout));
    control(&mut inside);
    ui.add_space(4.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_automatic_codec_is_said_as_it_is_here() {
        use leto_core::engine::Codecs;
        let card = Codecs::of(Codec::Av1).with(Codec::H265).with(Codec::H264);
        let decodable = Decodable { hardware: card, any: card };
        assert_eq!(
            codec_about(decodable),
            "Automatic asks the PC for AV1 or H.265, which the graphics card decodes, or else H.264."
        );
        assert!(codec_about(Decodable::SOFTWARE).starts_with("Automatic is H.264"));
    }

    #[test]
    fn sizes_and_rates_are_named_as_people_know_them() {
        assert_eq!(resolution_name(3840, 2160), "3840 × 2160 (4K)");
        assert_eq!(resolution_name(1920, 1200), "1920 × 1200");
        assert_eq!(megabits(20_000), "20 Mbit/s");
        assert_eq!(megabits(15_500), "15.5 Mbit/s");
    }

    /// Every setting of the default list fits a window as narrow as Leto's
    /// smallest, and as wide as a large one, without a panic.
    #[test]
    fn the_settings_draw_in_a_narrow_window_and_a_wide_one() {
        for width in [480.0, 1400.0] {
            let ctx = egui::Context::default();
            let mut settings = Settings { bitrate_kbps: Some(30_000), ..Settings::default() };
            let native = Some(ScreenMode { width: 2560, height: 1440, refresh_hz: 144 });
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(width, 900.0))),
                ..Default::default()
            };
            let mut tunnel = TunnelForm::default();
            let mut asked = None;
            let mut output = ctx.run_ui(input, |ui| {
                let tunnel = Some(&mut tunnel);
                asked = Some(show(ui, &mut settings, native, Decodable::SOFTWARE, tunnel, "PCs"));
            });
            // Nothing draws the frame: its textures go nowhere.
            output.textures_delta.clear();
            assert_eq!(asked, Some(Asked::default()));
            assert_eq!(settings.bitrate_kbps, Some(30_000));
        }
    }
}
