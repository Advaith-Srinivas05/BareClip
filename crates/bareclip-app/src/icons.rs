//! The interface's symbols. They are Lucide's outlines, drawn as lines, so they stay
//! sharp at any size and take any colour from the palette.

use eframe::egui::{
    Color32, Painter, Pos2, Rect, Shape, Stroke, emath::GuiRounding as _, pos2, vec2,
};
pub use shadcn_egui_core::Icon;
use shadcn_egui_core::Seg;
pub use shadcn_egui_icons_lucide::{
    ARROW_DOWN, ARROW_UP, ARROW_UP_DOWN, AUDIO_LINES, CHECK, CHEVRON_DOWN, CHEVRONS_DOWN,
    CHEVRONS_UP, CIRCLE_ALERT, CIRCLE_CHECK, COPY, COPY_PLUS, CROP, ELLIPSIS, FILE_PLUS, FILM,
    FLIP_HORIZONTAL_2, FLIP_VERTICAL_2, FOLD_HORIZONTAL, FOLDER_OPEN, HEADPHONES, IMAGE,
    LIST_FILTER, MINUS, MONITOR, MOON, MOVE_HORIZONTAL, MUSIC, PAINT_BUCKET, PANEL_BOTTOM_CLOSE,
    PANEL_BOTTOM_OPEN, PANEL_LEFT_CLOSE, PAUSE, PENCIL, PICTURE_IN_PICTURE_2, PLAY, PLUS, RATIO,
    REDO_2, ROTATE_CCW, ROTATE_CW, SAVE, SCISSORS, SETTINGS, SKIP_BACK, SKIP_FORWARD, SQUARE, SUN,
    TARGET, TRASH_2, UNDO_2, UNLINK, UPLOAD, VOLUME_2, VOLUME_X, X, ZAP, ZOOM_IN, ZOOM_OUT,
};

/// Lucide draws its outlines 2 units wide on a 24 unit square.
const STROKE_SHARE: f32 = 2.0 / 24.0;
/// How many straight pieces stand in for one curve. These shapes are small on screen.
const CURVE_PIECES: usize = 10;

/// One unbroken line of a symbol, in screen positions.
#[derive(Debug)]
struct Line {
    points: Vec<Pos2>,
    closed: bool,
}

/// Adds a point unless it repeats the last one, which would leave a line with no direction.
fn push_point(points: &mut Vec<Pos2>, point: Pos2) {
    if points.last() != Some(&point) {
        points.push(point);
    }
}

/// Turns a symbol's path commands into lines that fill `rect`.
fn lines_of(segments: impl Iterator<Item = Seg>, icon_size: f32, rect: Rect) -> Vec<Line> {
    let scale = rect.width() / icon_size;
    let at = |x: f32, y: f32| rect.min + vec2(x, y) * scale;
    let mut lines = Vec::new();
    let mut points: Vec<Pos2> = Vec::new();
    let mut finish = |points: &mut Vec<Pos2>, closed: bool| {
        if !points.is_empty() {
            let mut finished = std::mem::take(points);
            if closed && finished.len() > 1 && finished.first() == finished.last() {
                finished.pop();
            }
            lines.push(Line {
                points: finished,
                closed,
            });
        }
    };
    for segment in segments {
        match segment {
            Seg::M(x, y) => {
                finish(&mut points, false);
                points.push(at(x, y));
            }
            Seg::L(x, y) => push_point(&mut points, at(x, y)),
            Seg::Q(cx, cy, x, y) => {
                let Some(&from) = points.last() else { continue };
                let (control, to) = (at(cx, cy), at(x, y));
                for piece in 1..=CURVE_PIECES {
                    let t = piece as f32 / CURVE_PIECES as f32;
                    let u = 1.0 - t;
                    let point = (u * u * from.to_vec2()
                        + 2.0 * u * t * control.to_vec2()
                        + t * t * to.to_vec2())
                    .to_pos2();
                    push_point(&mut points, point);
                }
            }
            Seg::C(ax, ay, bx, by, x, y) => {
                let Some(&from) = points.last() else { continue };
                let (first, second, to) = (at(ax, ay), at(bx, by), at(x, y));
                for piece in 1..=CURVE_PIECES {
                    let t = piece as f32 / CURVE_PIECES as f32;
                    let u = 1.0 - t;
                    let point = (u * u * u * from.to_vec2()
                        + 3.0 * u * u * t * first.to_vec2()
                        + 3.0 * u * t * t * second.to_vec2()
                        + t * t * t * to.to_vec2())
                    .to_pos2();
                    push_point(&mut points, point);
                }
            }
            Seg::Z => finish(&mut points, true),
        }
    }
    finish(&mut points, false);
    lines
}

fn draw(painter: &Painter, rect: Rect, icon: &Icon, color: Color32, solid: bool) {
    let rect = rect.round_to_pixels(painter.pixels_per_point());
    let width = rect.width() * STROKE_SHARE;
    for layer in icon.layers() {
        let color = color.gamma_multiply(layer.alpha);
        let stroke = Stroke::new(width, color);
        for line in lines_of(layer.segs, icon.size, rect) {
            if solid && line.closed {
                painter.add(Shape::convex_polygon(line.points, color, stroke));
            } else if line.closed {
                painter.add(Shape::closed_line(line.points, stroke));
            } else {
                // Round ends, which also keep a symbol's single dots visible.
                for end in [line.points.first(), line.points.last()]
                    .into_iter()
                    .flatten()
                {
                    painter.circle_filled(*end, width / 2.0, color);
                }
                if line.points.len() > 1 {
                    painter.add(Shape::line(line.points, stroke));
                }
            }
        }
    }
}

/// Draws a symbol's outline to fill `rect`, which should be square.
pub fn paint(painter: &Painter, rect: Rect, icon: &Icon, color: Color32) {
    draw(painter, rect, icon, color, false);
}

/// Draws a symbol filled in. Only for symbols made of simple closed shapes.
pub fn paint_solid(painter: &Painter, rect: Rect, icon: &Icon, color: Color32) {
    draw(painter, rect, icon, color, true);
}

/// A square of the given size around a point, for drawing a symbol in.
pub fn square_at(center: Pos2, side: f32) -> Rect {
    Rect::from_center_size(pos2(center.x, center.y), vec2(side, side))
}

#[cfg(test)]
mod tests {
    use super::*;

    const USED: [Icon; 59] = [
        ARROW_UP_DOWN,
        CHECK,
        CHEVRON_DOWN,
        CIRCLE_ALERT,
        CIRCLE_CHECK,
        COPY,
        COPY_PLUS,
        FILE_PLUS,
        FILM,
        IMAGE,
        FOLD_HORIZONTAL,
        FOLDER_OPEN,
        LIST_FILTER,
        MINUS,
        MONITOR,
        MOON,
        MOVE_HORIZONTAL,
        PANEL_BOTTOM_CLOSE,
        PANEL_BOTTOM_OPEN,
        PANEL_LEFT_CLOSE,
        PAUSE,
        PLAY,
        PLUS,
        RATIO,
        REDO_2,
        ROTATE_CCW,
        ROTATE_CW,
        SAVE,
        SCISSORS,
        SETTINGS,
        SKIP_BACK,
        SKIP_FORWARD,
        SQUARE,
        SUN,
        TARGET,
        TRASH_2,
        UNDO_2,
        UPLOAD,
        X,
        ZAP,
        ZOOM_IN,
        ZOOM_OUT,
        PAINT_BUCKET,
        CROP,
        ARROW_DOWN,
        ARROW_UP,
        CHEVRONS_DOWN,
        CHEVRONS_UP,
        PICTURE_IN_PICTURE_2,
        ELLIPSIS,
        FLIP_HORIZONTAL_2,
        FLIP_VERTICAL_2,
        PENCIL,
        VOLUME_2,
        VOLUME_X,
        HEADPHONES,
        MUSIC,
        AUDIO_LINES,
        UNLINK,
    ];

    fn lines_in(icon: &Icon, rect: Rect) -> Vec<Line> {
        icon.layers()
            .flat_map(|layer| lines_of(layer.segs, icon.size, rect))
            .collect()
    }

    #[test]
    fn every_symbol_is_outlines_at_full_strength() {
        for icon in &USED {
            for layer in icon.layers() {
                assert!(
                    layer.fill.is_none(),
                    "{} has a fill we do not draw",
                    icon.name
                );
                assert!((layer.alpha - 1.0).abs() < f32::EPSILON, "{}", icon.name);
            }
        }
    }

    #[test]
    fn every_symbol_stays_inside_its_square_at_each_size() {
        for side in [14.0, 16.0, 18.0, 20.0, 24.0] {
            let rect = Rect::from_min_size(pos2(100.0, 50.0), vec2(side, side));
            for icon in &USED {
                let lines = lines_in(icon, rect);
                assert!(!lines.is_empty(), "{} draws nothing", icon.name);
                for line in lines {
                    for point in line.points {
                        assert!(
                            rect.expand(0.01).contains(point),
                            "{} leaves its {side} square at {point:?}",
                            icon.name
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn closed_shapes_do_not_repeat_their_first_point() {
        let square = [
            Seg::M(0.0, 0.0),
            Seg::L(24.0, 0.0),
            Seg::L(24.0, 24.0),
            Seg::L(0.0, 0.0),
            Seg::Z,
        ];
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(24.0, 24.0));

        let lines = lines_of(square.into_iter(), 24.0, rect);

        assert_eq!(lines.len(), 1);
        assert!(lines[0].closed);
        assert_eq!(lines[0].points.len(), 3);
    }

    #[test]
    fn a_new_start_point_begins_a_new_line() {
        let two = [
            Seg::M(0.0, 0.0),
            Seg::L(1.0, 1.0),
            Seg::M(5.0, 5.0),
            Seg::L(6.0, 6.0),
        ];
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(24.0, 24.0));

        let lines = lines_of(two.into_iter(), 24.0, rect);

        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|line| !line.closed));
    }

    #[test]
    fn curves_end_exactly_where_they_should() {
        let curve = [
            Seg::M(0.0, 0.0),
            Seg::C(0.0, 8.0, 8.0, 16.0, 16.0, 16.0),
            Seg::Q(20.0, 16.0, 24.0, 24.0),
        ];
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(24.0, 24.0));

        let lines = lines_of(curve.into_iter(), 24.0, rect);

        let points = &lines[0].points;
        assert_eq!(points.len(), 1 + 2 * CURVE_PIECES);
        assert!((points[CURVE_PIECES] - pos2(16.0, 16.0)).length() < 0.001);
        assert!((*points.last().unwrap() - pos2(24.0, 24.0)).length() < 0.001);
    }

    #[test]
    fn a_curve_with_no_start_is_skipped() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(24.0, 24.0));

        let lines = lines_of([Seg::Q(1.0, 1.0, 2.0, 2.0)].into_iter(), 24.0, rect);

        assert!(lines.is_empty());
    }

    #[test]
    fn solid_symbols_are_only_closed_shapes() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(24.0, 24.0));
        for icon in [PLAY, PAUSE] {
            assert!(
                lines_in(&icon, rect).iter().all(|line| line.closed),
                "{} has an open line",
                icon.name
            );
        }
    }

    #[test]
    fn a_square_is_centred_on_its_point() {
        let square = square_at(pos2(40.0, 40.0), 16.0);

        assert_eq!(
            square,
            Rect::from_min_size(pos2(32.0, 32.0), vec2(16.0, 16.0))
        );
    }
}
