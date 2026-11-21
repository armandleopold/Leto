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

/// A computer mouse seen from above: the game mouse.
pub fn mouse(painter: &Painter, rect: Rect, color: Color32) {
    let stroke = stroke(rect, color);
    let body = Rect::from_min_max(at(rect, 0.2, 0.02), at(rect, 0.8, 0.98));
    painter.rect_stroke(body, rect.width() * 0.3, stroke, StrokeKind::Inside);
    painter.line_segment([at(rect, 0.5, 0.2), at(rect, 0.5, 0.42)], stroke);
}

/// A camera: its body, and the lens that sticks out to the right.
pub fn camera(painter: &Painter, rect: Rect, color: Color32) {
    let stroke = stroke(rect, color);
    let body = Rect::from_min_max(at(rect, 0.02, 0.22), at(rect, 0.68, 0.78));
    painter.rect_stroke(body, rect.width() * 0.12, stroke, StrokeKind::Inside);
    let lens = vec![at(rect, 0.68, 0.5), at(rect, 0.98, 0.28), at(rect, 0.98, 0.72)];
    painter.add(egui::Shape::closed_line(lens, stroke));
}

/// A microphone on its stand. `level`, from 0 to 1, fills it from the bottom
/// with what it hears.
pub fn microphone(painter: &Painter, rect: Rect, color: Color32, level: f32) {
    let stroke = stroke(rect, color);
    let head = Rect::from_min_max(at(rect, 0.33, 0.02), at(rect, 0.67, 0.62));
    let round = head.width() / 2.0;
    if level > 0.0 {
        // What it hears, inside the head: the part below the line is full.
        let line = head.bottom() - head.height() * level.min(1.0);
        let full = painter.with_clip_rect(Rect::from_min_max(
            Pos2::new(head.left(), line.max(head.top())),
            head.max,
        ));
        full.rect_filled(head, round, color);
    }
    painter.rect_stroke(head, round, stroke, StrokeKind::Inside);

    // The holder: half a circle under the head.
    let center = at(rect, 0.5, 0.42);
    let radius = rect.width() * 0.36;
    let holder: Vec<Pos2> = (0..=16)
        .map(|step| {
            let angle = std::f32::consts::PI * step as f32 / 16.0;
            center + vec2(angle.cos(), angle.sin()) * radius
        })
        .collect();
    painter.line(holder, stroke);
    painter.line_segment([Pos2::new(center.x, center.y + radius), at(rect, 0.5, 0.96)], stroke);
    painter.line_segment([at(rect, 0.3, 0.96), at(rect, 0.7, 0.96)], stroke);
}

/// A loudspeaker, and its sound going out to the right.
pub fn speaker(painter: &Painter, rect: Rect, color: Color32) {
    let stroke = stroke(rect, color);
    let cone = vec![
        at(rect, 0.04, 0.36),
        at(rect, 0.22, 0.36),
        at(rect, 0.48, 0.12),
        at(rect, 0.48, 0.88),
        at(rect, 0.22, 0.64),
        at(rect, 0.04, 0.64),
    ];
    painter.add(egui::Shape::closed_line(cone, stroke));
    let center = at(rect, 0.5, 0.5);
    for radius in [0.2, 0.4] {
        let wave: Vec<Pos2> = (-6..=6)
            .map(|step| {
                let angle = step as f32 / 6.0 * 0.85;
                center + vec2(angle.cos(), angle.sin()) * rect.width() * radius
            })
            .collect();
        painter.line(wave, stroke);
    }
}

/// A cog wheel: the settings.
pub fn gear(painter: &Painter, rect: Rect, color: Color32) {
    let stroke = stroke(rect, color);
    let center = rect.center();
    let size = rect.width();
    painter.circle_stroke(center, size * 0.3, stroke);
    painter.circle_stroke(center, size * 0.11, stroke);
    // Eight teeth around the wheel, thicker than its line.
    let tooth = Stroke::new(stroke.width * 1.9, color);
    for step in 0..8 {
        let angle = std::f32::consts::FRAC_PI_4 * step as f32;
        let out = vec2(angle.cos(), angle.sin());
        painter.line_segment([center + out * size * 0.3, center + out * size * 0.47], tooth);
    }
}
