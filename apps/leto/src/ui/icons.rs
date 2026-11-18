//! Small icons drawn with lines, so they stay sharp at any size and need no
//! icon font.

use egui::{Color32, Painter, Pos2, Rect, Stroke, StrokeKind, pos2, vec2};

/// Draws an icon inside `rect` in `color`.
pub type Icon = fn(&Painter, Rect, Color32);

fn stroke(rect: Rect, color: Color32) -> Stroke {
    Stroke::new((rect.width() / 11.0).clamp(1.5, 2.5), color)
}

/// A point inside `rect`, from fractions of its width and height.
fn at(rect: Rect, x: f32, y: f32) -> Pos2 {
    rect.min + vec2(rect.width() * x, rect.height() * y)
}

/// A screen on its stand: a PC.
pub fn monitor(painter: &Painter, rect: Rect, color: Color32) {
    let stroke = stroke(rect, color);
    let screen = Rect::from_min_max(at(rect, 0.04, 0.12), at(rect, 0.96, 0.70));
    painter.rect_stroke(screen, rect.width() / 9.0, stroke, StrokeKind::Inside);
    painter.line_segment([at(rect, 0.5, 0.70), at(rect, 0.5, 0.88)], stroke);
    painter.line_segment([at(rect, 0.28, 0.88), at(rect, 0.72, 0.88)], stroke);
}

pub fn plus(painter: &Painter, rect: Rect, color: Color32) {
    let stroke = stroke(rect, color);
    painter.line_segment([at(rect, 0.5, 0.12), at(rect, 0.5, 0.88)], stroke);
    painter.line_segment([at(rect, 0.12, 0.5), at(rect, 0.88, 0.5)], stroke);
}

/// An arrowhead pointing left: back.
pub fn back(painter: &Painter, rect: Rect, color: Color32) {
    let points = vec![at(rect, 0.62, 0.14), at(rect, 0.28, 0.5), at(rect, 0.62, 0.86)];
    painter.line(points, stroke(rect, color));
}

/// Three dots: more actions.
pub fn more(painter: &Painter, rect: Rect, color: Color32) {
    for x in [0.14, 0.5, 0.86] {
        painter.circle_filled(at(rect, x, 0.5), rect.width() / 11.0, color);
    }
}

/// A circular arrow: search again.
pub fn refresh(painter: &Painter, rect: Rect, color: Color32) {
    let stroke = stroke(rect, color);
    let center = rect.center();
    let radius = rect.width() * 0.42;
    let direction = |degrees: f32| {
        let angle = degrees.to_radians();
        vec2(angle.cos(), angle.sin())
    };
    // Clockwise from the right, leaving a gap before coming back around.
    let (from, to) = (20.0, 300.0);
    let arc: Vec<Pos2> = (0..=32)
        .map(|step| center + direction(from + (to - from) * step as f32 / 32.0) * radius)
        .collect();
    painter.line(arc, stroke);

    // The head sits at the end of the arc and points the way it turns.
    let tip = center + direction(to) * radius;
    let backwards = direction(to - 90.0);
    let wing = rect.width() * 0.3;
    let spread = |degrees: f32| {
        let angle = degrees.to_radians();
        let (sin, cos) = angle.sin_cos();
        tip + vec2(backwards.x * cos - backwards.y * sin, backwards.x * sin + backwards.y * cos)
            * wing
    };
    painter.line(vec![spread(38.0), tip, spread(-38.0)], stroke);
}

/// The letter i in a circle: a note.
pub fn info(painter: &Painter, rect: Rect, color: Color32) {
    let stroke = stroke(rect, color);
    painter.circle_stroke(rect.center(), rect.width() * 0.46, stroke);
    painter.line_segment([at(rect, 0.5, 0.46), at(rect, 0.5, 0.72)], stroke);
    painter.circle_filled(pos2(rect.center().x, at(rect, 0.5, 0.29).y), stroke.width * 0.75, color);
}

/// Four corners pulling apart: fill the screen.
pub fn expand(painter: &Painter, rect: Rect, color: Color32) {
    let stroke = stroke(rect, color);
    for (x, y) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
        let (inward_x, inward_y) = (0.5 - x, 0.5 - y);
        let corner = at(rect, 0.5 - inward_x * 0.84, 0.5 - inward_y * 0.84);
        let along_x = at(rect, 0.5 - inward_x * 0.2, 0.5 - inward_y * 0.84);
        let along_y = at(rect, 0.5 - inward_x * 0.84, 0.5 - inward_y * 0.2);
        painter.line(vec![along_x, corner, along_y], stroke);
    }
}
