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

/// A screen on its keyboard: a laptop.
pub fn laptop(painter: &Painter, rect: Rect, color: Color32) {
    let stroke = stroke(rect, color);
    let screen = Rect::from_min_max(at(rect, 0.14, 0.16), at(rect, 0.86, 0.68));
    painter.rect_stroke(screen, rect.width() / 12.0, stroke, StrokeKind::Inside);
    painter.line_segment([at(rect, 0.02, 0.84), at(rect, 0.98, 0.84)], stroke);
}

/// A tall screen with its line at the bottom: a phone.
pub fn phone(painter: &Painter, rect: Rect, color: Color32) {
    let stroke = stroke(rect, color);
    let body = Rect::from_min_max(at(rect, 0.27, 0.04), at(rect, 0.73, 0.96));
    painter.rect_stroke(body, rect.width() / 8.0, stroke, StrokeKind::Inside);
    painter.line_segment([at(rect, 0.43, 0.80), at(rect, 0.57, 0.80)], stroke);
}

/// A wide screen with its button: a tablet.
pub fn tablet(painter: &Painter, rect: Rect, color: Color32) {
    let stroke = stroke(rect, color);
    let body = Rect::from_min_max(at(rect, 0.04, 0.16), at(rect, 0.96, 0.84));
    painter.rect_stroke(body, rect.width() / 9.0, stroke, StrokeKind::Inside);
    painter.circle_filled(at(rect, 0.84, 0.5), stroke.width * 0.75, color);
}

/// A wide screen on two feet: a television.
pub fn television(painter: &Painter, rect: Rect, color: Color32) {
    let stroke = stroke(rect, color);
    let screen = Rect::from_min_max(at(rect, 0.02, 0.16), at(rect, 0.98, 0.74));
    painter.rect_stroke(screen, rect.width() / 12.0, stroke, StrokeKind::Inside);
    painter.line_segment([at(rect, 0.30, 0.74), at(rect, 0.22, 0.90)], stroke);
    painter.line_segment([at(rect, 0.70, 0.74), at(rect, 0.78, 0.90)], stroke);
}

/// A gamepad, with its cross and two buttons: a console.
pub fn gamepad(painter: &Painter, rect: Rect, color: Color32) {
    let stroke = stroke(rect, color);
    let body = Rect::from_min_max(at(rect, 0.02, 0.24), at(rect, 0.98, 0.78));
    painter.rect_stroke(body, rect.height() * 0.27, stroke, StrokeKind::Inside);
    painter.line_segment([at(rect, 0.20, 0.51), at(rect, 0.40, 0.51)], stroke);
    painter.line_segment([at(rect, 0.30, 0.41), at(rect, 0.30, 0.61)], stroke);
    painter.circle_filled(at(rect, 0.64, 0.57), stroke.width * 0.8, color);
    painter.circle_filled(at(rect, 0.77, 0.45), stroke.width * 0.8, color);
}

/// The icon of a device that reaches this PC.
pub fn client(icon: crate::settings::ClientIcon) -> Icon {
    use crate::settings::ClientIcon;
    match icon {
        ClientIcon::Computer => monitor,
        ClientIcon::Laptop => laptop,
        ClientIcon::Phone => phone,
        ClientIcon::Tablet => tablet,
        ClientIcon::Television => television,
        ClientIcon::Console => gamepad,
    }
}

/// A house: the PC Leto runs on.
pub fn home(painter: &Painter, rect: Rect, color: Color32) {
    let stroke = stroke(rect, color);
    // The roof, then the walls under it, with the door.
    painter.line_segment([at(rect, 0.06, 0.52), at(rect, 0.5, 0.12)], stroke);
    painter.line_segment([at(rect, 0.5, 0.12), at(rect, 0.94, 0.52)], stroke);
    painter.line_segment([at(rect, 0.2, 0.42), at(rect, 0.2, 0.9)], stroke);
    painter.line_segment([at(rect, 0.8, 0.42), at(rect, 0.8, 0.9)], stroke);
    painter.line_segment([at(rect, 0.2, 0.9), at(rect, 0.8, 0.9)], stroke);
    painter.line_segment([at(rect, 0.5, 0.9), at(rect, 0.5, 0.64)], stroke);
}

/// Three boxes joined by wires: a local network.
pub fn network(painter: &Painter, rect: Rect, color: Color32) {
    let stroke = Stroke::new((rect.width() / 10.0).clamp(1.2, 2.0), color);
    let radius = rect.width() / 14.0;
    let boxes = [
        Rect::from_min_max(at(rect, 0.34, 0.04), at(rect, 0.66, 0.30)),
        Rect::from_min_max(at(rect, 0.02, 0.70), at(rect, 0.34, 0.96)),
        Rect::from_min_max(at(rect, 0.66, 0.70), at(rect, 0.98, 0.96)),
    ];
    for box_ in boxes {
        painter.rect_stroke(box_, radius, stroke, StrokeKind::Inside);
    }
    painter.line_segment([at(rect, 0.5, 0.30), at(rect, 0.5, 0.50)], stroke);
    painter.line(
        vec![
            at(rect, 0.18, 0.70),
            at(rect, 0.18, 0.50),
            at(rect, 0.82, 0.50),
            at(rect, 0.82, 0.70),
        ],
        stroke,
    );
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

/// A triangle with an exclamation mark: something went wrong.
pub fn warning(painter: &Painter, rect: Rect, color: Color32) {
    let stroke = stroke(rect, color);
    let corners = vec![at(rect, 0.5, 0.08), at(rect, 0.96, 0.9), at(rect, 0.04, 0.9)];
    painter.add(egui::Shape::closed_line(corners, stroke));
    painter.line_segment([at(rect, 0.5, 0.38), at(rect, 0.5, 0.62)], stroke);
    painter.circle_filled(at(rect, 0.5, 0.76), stroke.width * 0.75, color);
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

/// A keyboard: the one on the screen of a phone.
pub fn keyboard(painter: &Painter, rect: Rect, color: Color32) {
    let stroke = stroke(rect, color);
    let body = Rect::from_min_max(at(rect, 0.02, 0.2), at(rect, 0.98, 0.8));
    painter.rect_stroke(body, rect.width() * 0.08, stroke, StrokeKind::Inside);
    for row in [0.38, 0.52] {
        for column in [0.22, 0.38, 0.54, 0.7] {
            painter.circle_filled(at(rect, column + 0.04, row), stroke.width * 0.6, color);
        }
    }
    painter.line_segment([at(rect, 0.3, 0.67), at(rect, 0.7, 0.67)], stroke);
}

/// The arrow of a pointer, as wide and high as `rect`.
fn arrow(rect: Rect) -> Vec<Pos2> {
    [
        (0.16, 0.02),
        (0.16, 0.84),
        (0.38, 0.64),
        (0.54, 0.98),
        (0.68, 0.91),
        (0.53, 0.59),
        (0.82, 0.59),
    ]
    .into_iter()
    .map(|(x, y)| at(rect, x, y))
    .collect()
}

/// A pointer: this device's own.
pub fn pointer(painter: &Painter, rect: Rect, color: Color32) {
    painter.add(egui::Shape::closed_line(arrow(rect), stroke(rect, color)));
}

/// A pointer inside a picture: the one the PC draws.
pub fn pointer_in_picture(painter: &Painter, rect: Rect, color: Color32) {
    let stroke = stroke(rect, color);
    let picture = Rect::from_min_max(at(rect, 0.0, 0.08), at(rect, 1.0, 0.92));
    painter.rect_stroke(picture, rect.width() * 0.1, stroke, StrokeKind::Inside);
    let inside = Rect::from_min_max(at(rect, 0.3, 0.26), at(rect, 0.78, 0.74));
    painter.add(egui::Shape::convex_polygon(arrow(inside), color, egui::Stroke::NONE));
}

/// A padlock, closed or with its shackle swung open.
pub fn padlock(painter: &Painter, rect: Rect, color: Color32, closed: bool) {
    let stroke = stroke(rect, color);
    let body = Rect::from_min_max(at(rect, 0.16, 0.46), at(rect, 0.84, 0.98));
    painter.rect_stroke(body, rect.width() * 0.12, stroke, StrokeKind::Inside);
    painter.circle_filled(at(rect, 0.5, 0.72), stroke.width * 0.8, color);
    // The shackle: two legs and the bow between them. Open, it is lifted
    // and its right leg hangs free of the body.
    let (top, left, right) = if closed { (0.04, 0.3, 0.7) } else { (0.0, 0.3, 0.7) };
    let bow = (right - left) / 2.0;
    let mut points = vec![at(rect, left, 0.46), at(rect, left, top + bow)];
    for step in 1..=7 {
        let angle = std::f32::consts::PI * (1.0 - step as f32 / 8.0);
        points.push(at(rect, 0.5 + bow * angle.cos(), top + bow - bow * angle.sin()));
    }
    points.push(at(rect, right, top + bow));
    points.push(at(rect, right, if closed { 0.46 } else { top + bow + 0.08 }));
    painter.line(points, stroke);
}

/// A camera: its body, and the lens that sticks out to the right.
pub fn camera(painter: &Painter, rect: Rect, color: Color32) {
    let stroke = stroke(rect, color);
    let body = Rect::from_min_max(at(rect, 0.02, 0.22), at(rect, 0.68, 0.78));
    painter.rect_stroke(body, rect.width() * 0.12, stroke, StrokeKind::Inside);
    let lens = vec![at(rect, 0.68, 0.5), at(rect, 0.98, 0.28), at(rect, 0.98, 0.72)];
    painter.add(egui::Shape::closed_line(lens, stroke));
}

/// A line across an icon drawn in `rect`, for something that is off: from
/// its top left corner to its bottom right one.
pub fn slash(painter: &Painter, rect: Rect, color: Color32) {
    painter.line_segment([at(rect, 0.0, 0.0), at(rect, 1.0, 1.0)], stroke(rect, color));
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
