//! The window's frame, where Leto draws it itself: on Windows, the window
//! has no title bar of the system's. Leto's own top bar takes its place: it
//! moves the window when dragged and maximizes it when double-clicked, and
//! its buttons minimize, maximize and close the window. Its edges resize
//! it. Elsewhere the system's frame stays, and none of this shows.

use egui::{
    Color32, Context, CursorIcon, Direction, Id, Painter, PointerButton, Pos2, Rect, Response,
    Role, Sense, Stroke, Ui, ViewportCommand, WidgetInfo, pos2, vec2, viewport::ResizeDirection,
};

use leto_core::platform::Platform;

use crate::theme;

/// Whether Leto draws the window's frame itself.
pub const OWN: bool = matches!(Platform::current(), Some(Platform::Windows));

/// How wide a button of the frame is at the top of the window, as Windows
/// makes its own.
pub const BUTTON_WIDTH: f32 = 46.0;

/// How near an edge of the window the pointer resizes it, in points.
const EDGE: f32 = 5.0;

/// The red of a close button under the pointer, as Windows draws it.
const CLOSE_HOVER: Color32 = Color32::from_rgb(0xC4, 0x2B, 0x1C);

/// How the window lies on the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Lying {
    maximized: bool,
    full_screen: bool,
}

impl Lying {
    fn of(ctx: &Context) -> Self {
        ctx.input(|input| {
            let viewport = input.viewport();
            Self {
                maximized: viewport.maximized.unwrap_or(false),
                full_screen: viewport.fullscreen.unwrap_or(false),
            }
        })
    }

    /// Whether the window fills the screen, or its part for windows: its
    /// edges are the screen's then, and resize nothing.
    fn fills(self) -> bool {
        self.maximized || self.full_screen
    }
}

/// Makes `rect`, the bar at the top of the window, move the window when
/// dragged, and maximize it or give the screen back when double-clicked.
/// Called before anything is put on the bar, which stays pressable.
pub fn drag_by(ui: &Ui, rect: Rect, id: Id) {
    if !OWN {
        return;
    }
    let response = ui.interact(rect, id, Sense::click_and_drag());
    let lying = Lying::of(ui.ctx());
    if response.double_clicked() {
        toggle_maximized(ui.ctx(), lying);
    } else if response.drag_started_by(PointerButton::Primary)
        && !lying.full_screen
        && edge(ui.ctx().viewport_rect(), response.interact_pointer_pos(), lying).is_none()
    {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
}

/// The buttons that minimize, maximize and close the window, in that order
/// on screen: each of `size`, lit with corners of `radius`.
pub fn buttons(ui: &mut Ui, size: egui::Vec2, radius: u8) {
    if !OWN {
        return;
    }
    let ctx = ui.ctx().clone();
    let lying = Lying::of(&ctx);
    let middle = if lying.fills() { Look::Restore } else { Look::Maximize };
    let mut looks = [Look::Minimize, middle, Look::Close];
    if ui.layout().main_dir() == Direction::RightToLeft {
        looks.reverse();
    }
    for look in looks {
        if !button(ui, size, radius, look).clicked() {
            continue;
        }
        match look {
            Look::Minimize => ctx.send_viewport_cmd(ViewportCommand::Minimized(true)),
            Look::Maximize | Look::Restore => toggle_maximized(&ctx, lying),
            Look::Close => ctx.send_viewport_cmd(ViewportCommand::Close),
        }
    }
}

/// Lets the edges of the window resize it. Called last in a frame, so that
/// the pointer shows that it would, whatever is underneath.
pub fn edges(ctx: &Context) {
    if !OWN {
        return;
    }
    let lying = Lying::of(ctx);
    let pointer = ctx.input(|input| input.pointer.hover_pos());
    let Some(direction) = edge(ctx.viewport_rect(), pointer, lying) else { return };
    ctx.set_cursor_icon(cursor(direction));
    if ctx.input(|input| input.pointer.button_pressed(PointerButton::Primary)) {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(direction));
    }
}

fn toggle_maximized(ctx: &Context, lying: Lying) {
    if lying.full_screen {
        ctx.send_viewport_cmd(ViewportCommand::Fullscreen(false));
    } else {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!lying.maximized));
    }
}

/// Which way the window is resized from `pointer`, if it is on an edge of
/// `window`. A window that fills the screen has no edges to pull.
fn edge(window: Rect, pointer: Option<Pos2>, lying: Lying) -> Option<ResizeDirection> {
    let pointer = pointer.filter(|_| !lying.fills())?;
    if !window.contains(pointer) {
        return None;
    }
    // The corners are wider than the edges, as they are on Windows.
    let corner = EDGE * 3.0;
    let left = pointer.x < window.left() + EDGE;
    let right = pointer.x > window.right() - EDGE;
    let top = pointer.y < window.top() + EDGE;
    let bottom = pointer.y > window.bottom() - EDGE;
    let near_left = pointer.x < window.left() + corner;
    let near_right = pointer.x > window.right() - corner;
    let near_top = pointer.y < window.top() + corner;
    let near_bottom = pointer.y > window.bottom() - corner;
    Some(match () {
        () if (top && near_left) || (left && near_top) => ResizeDirection::NorthWest,
        () if (top && near_right) || (right && near_top) => ResizeDirection::NorthEast,
        () if (bottom && near_left) || (left && near_bottom) => ResizeDirection::SouthWest,
        () if (bottom && near_right) || (right && near_bottom) => ResizeDirection::SouthEast,
        () if top => ResizeDirection::North,
        () if bottom => ResizeDirection::South,
        () if left => ResizeDirection::West,
        () if right => ResizeDirection::East,
        () => return None,
    })
}

fn cursor(direction: ResizeDirection) -> CursorIcon {
    match direction {
        ResizeDirection::North | ResizeDirection::South => CursorIcon::ResizeVertical,
        ResizeDirection::East | ResizeDirection::West => CursorIcon::ResizeHorizontal,
        ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::ResizeNwSe,
        ResizeDirection::NorthEast | ResizeDirection::SouthWest => CursorIcon::ResizeNeSw,
    }
}

/// What a button of the frame shows.
#[derive(Clone, Copy)]
enum Look {
    Minimize,
    Maximize,
    Restore,
    Close,
}

fn button(ui: &mut Ui, size: egui::Vec2, radius: u8, look: Look) -> Response {
    let name = match look {
        Look::Minimize => "Minimize",
        Look::Maximize => "Maximize",
        Look::Restore => "Restore",
        Look::Close => "Close",
    };
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, name));
    let lit = response.hovered() || response.has_focus();
    let painter = ui.painter();
    let color = match look {
        Look::Close if lit => {
            painter.rect_filled(rect, radius, CLOSE_HOVER);
            Color32::WHITE
        }
        _ if lit => {
            painter.rect_filled(rect, radius, theme::SURFACE_HOVER);
            theme::MOON
        }
        _ => theme::DIM,
    };
    glyph(painter, rect.center(), look, color);
    response.on_hover_text(name)
}

/// The sign on a button: thin lines, ten points across, as on Windows.
fn glyph(painter: &Painter, center: Pos2, look: Look, color: Color32) {
    let stroke = Stroke::new(1.0, color);
    let half = 5.0;
    match look {
        Look::Minimize => {
            painter.line_segment([center + vec2(-half, 0.0), center + vec2(half, 0.0)], stroke);
        }
        Look::Maximize => {
            let square = Rect::from_center_size(center, vec2(2.0 * half, 2.0 * half));
            painter.rect_stroke(square, 1.0, stroke, egui::StrokeKind::Inside);
        }
        Look::Restore => {
            // A window in front of another.
            let front = Rect::from_min_size(center + vec2(-half, -half + 2.0), vec2(8.0, 8.0));
            painter.rect_stroke(front, 1.0, stroke, egui::StrokeKind::Inside);
            let behind = [
                pos2(front.left() + 2.0, front.top()),
                pos2(front.left() + 2.0, front.top() - 2.0),
                pos2(front.right() + 2.0, front.top() - 2.0),
                pos2(front.right() + 2.0, front.bottom() - 2.0),
                pos2(front.right(), front.bottom() - 2.0),
            ];
            painter.add(egui::Shape::line(behind.to_vec(), stroke));
        }
        Look::Close => {
            let (a, b) = (vec2(half, half), vec2(half, -half));
            painter.line_segment([center - a, center + a], stroke);
            painter.line_segment([center - b, center + b], stroke);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(960.0, 600.0));
    const FREE: Lying = Lying { maximized: false, full_screen: false };

    fn at(x: f32, y: f32) -> Option<ResizeDirection> {
        edge(WINDOW, Some(pos2(x, y)), FREE)
    }

    #[test]
    fn the_edges_of_the_window_resize_it_the_way_they_face() {
        assert_eq!(at(480.0, 2.0), Some(ResizeDirection::North));
        assert_eq!(at(480.0, 598.0), Some(ResizeDirection::South));
        assert_eq!(at(1.0, 300.0), Some(ResizeDirection::West));
        assert_eq!(at(958.0, 300.0), Some(ResizeDirection::East));
        // The corners, a little way along each edge too.
        assert_eq!(at(1.0, 1.0), Some(ResizeDirection::NorthWest));
        assert_eq!(at(10.0, 2.0), Some(ResizeDirection::NorthWest));
        assert_eq!(at(958.0, 10.0), Some(ResizeDirection::NorthEast));
        assert_eq!(at(2.0, 590.0), Some(ResizeDirection::SouthWest));
        assert_eq!(at(950.0, 598.0), Some(ResizeDirection::SouthEast));
    }

    #[test]
    fn inside_the_window_or_out_of_it_nothing_is_resized() {
        assert_eq!(at(480.0, 300.0), None);
        assert_eq!(at(6.0, 300.0), None);
        assert_eq!(at(-1.0, 300.0), None);
        assert_eq!(edge(WINDOW, None, FREE), None);
    }

    #[test]
    fn a_window_that_fills_the_screen_has_no_edges_to_pull() {
        let maximized = Lying { maximized: true, full_screen: false };
        assert_eq!(edge(WINDOW, Some(pos2(1.0, 1.0)), maximized), None);
        let full = Lying { maximized: false, full_screen: true };
        assert_eq!(edge(WINDOW, Some(pos2(480.0, 598.0)), full), None);
    }
}
