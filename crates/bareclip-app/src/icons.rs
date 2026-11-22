//! The interface's symbols. They are Lucide's outlines, drawn as lines, so they stay
//! sharp at any size and take any colour from the palette.

use eframe::egui::{
    Color32, Painter, Pos2, Rect, Shape, Stroke, emath::GuiRounding as _, pos2, vec2,
};
pub use shadcn_egui_core::Icon;
use shadcn_egui_core::Seg;
pub use shadcn_egui_icons_lucide::{
    ALIGN_CENTER_HORIZONTAL, ALIGN_CENTER_VERTICAL, ALIGN_END_HORIZONTAL, ALIGN_END_VERTICAL,
    ALIGN_START_HORIZONTAL, ALIGN_START_VERTICAL, APERTURE, ARROW_DOWN, ARROW_LEFT, ARROW_UP,
    ARROW_UP_DOWN, AUDIO_LINES, BLEND, CAMERA, CAPTIONS, CHECK, CHEVRON_DOWN, CHEVRON_LEFT,
    CHEVRON_RIGHT, CHEVRONS_DOWN, CHEVRONS_UP, CIRCLE_ALERT, CIRCLE_CHECK, CLIPBOARD_PASTE, CLOCK,
    CONTRAST, COPY, COPY_PLUS, CROP, ELLIPSIS, FILE_PLUS, FILM, FLIP_HORIZONTAL_2, FLIP_VERTICAL_2,
    FOLD_HORIZONTAL, FOLDER_OPEN, GAUGE, GRIP_VERTICAL, IMAGE, INFO, KEYBOARD, LAYERS, LIBRARY_BIG,
    LIST_FILTER, MAXIMIZE, MAXIMIZE_2, MIC, MIC_OFF, MINIMIZE, MINUS, MONITOR, MOON, MOVE,
    MOVE_HORIZONTAL, MUSIC, PAINT_BUCKET, PANEL_BOTTOM_CLOSE, PANEL_BOTTOM_OPEN, PANEL_LEFT_CLOSE,
    PAUSE, PENCIL, PICTURE_IN_PICTURE_2, PLAY, PLUS, RATIO, REDO, REDO_2, REPEAT, ROTATE_CCW,
    ROTATE_CW, SAVE, SCALING, SCISSORS, SETTINGS, SHAPES, SKIP_BACK, SKIP_FORWARD,
    SLIDERS_HORIZONTAL, SPARKLES, SQUARE, SUN, SWATCH_BOOK, TARGET, TRASH_2, TYPE, UNDO, UNDO_2,
    UNLINK, UPLOAD, VIDEO, VOLUME, VOLUME_1, VOLUME_2, VOLUME_X, WAND_SPARKLES, WEBCAM, WEBCAM_OFF,
    X, ZAP, ZOOM_IN, ZOOM_OUT,
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

/// Turns a symbol's path commands into lines that fill `rect`. A rect shorter than it is wide
/// squashes the symbol.
fn lines_of(segments: impl Iterator<Item = Seg>, icon_size: f32, rect: Rect) -> Vec<Line> {
    let scale = rect.size() / icon_size;
    let at = |x: f32, y: f32| rect.min + vec2(x * scale.x, y * scale.y);
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

/// Draws a symbol's outline to fill `rect`, which should be square, or shorter than it is wide
/// to squash the symbol, as the jump buttons lower Undo's arch. Its lines stay as thick.
pub fn paint(painter: &Painter, rect: Rect, icon: &Icon, color: Color32) {
    draw(painter, rect, icon, color, false);
}

/// Draws a symbol filled in. Only for symbols made of simple closed shapes.
pub fn paint_solid(painter: &Painter, rect: Rect, icon: &Icon, color: Color32) {
    draw(painter, rect, icon, color, true);
}

/// BareClip's small mark, a hexagon around a play triangle, in the brand's units. These are
/// `SMALL_OUTER`, `SMALL_INNER` and `SMALL_PLAY` from `scripts/brand_geometry.py`, without their
/// slight corner rounding, which can't be seen at a symbol's size.
const MARK_OUTER: [(f32, f32); 6] = [
    (405.0, 30.0),
    (930.0, 328.0),
    (930.0, 768.0),
    (405.0, 1072.0),
    (30.0, 852.0),
    (30.0, 247.0),
];
const MARK_INNER: [(f32, f32); 6] = [
    (405.0, 172.0),
    (800.0, 395.0),
    (800.0, 702.0),
    (405.0, 930.0),
    (160.0, 788.0),
    (160.0, 310.0),
];
const MARK_PLAY: [(f32, f32); 3] = [(330.0, 360.0), (690.0, 552.0), (330.0, 742.0)];
/// How wide the mark's ring is, the room between its outer and inner hexagons.
const MARK_RING: f32 = 130.0;
/// The mark's bounds in its units.
const MARK_SIZE: (f32, f32) = (900.0, 1042.0);
const MARK_MIDDLE: (f32, f32) = (480.0, 551.0);

/// The points of BareClip's small mark fitted into `rect`: the middle line of its ring, and its
/// play triangle, with how wide the ring is drawn.
fn mark_shape(rect: Rect) -> ([Pos2; 6], [Pos2; 3], f32) {
    let scale = (rect.width() / MARK_SIZE.0).min(rect.height() / MARK_SIZE.1);
    let middle = pos2(MARK_MIDDLE.0, MARK_MIDDLE.1);
    let at = |(x, y): (f32, f32)| rect.center() + (pos2(x, y) - middle) * scale;
    let ring = std::array::from_fn(|index| {
        let (outer, inner) = (MARK_OUTER[index], MARK_INNER[index]);
        at(((outer.0 + inner.0) / 2.0, (outer.1 + inner.1) / 2.0))
    });
    let play = MARK_PLAY.map(at);
    (ring, play, MARK_RING * scale)
}

/// Draws BareClip's small mark in one colour to fill `rect`, for a button that opens something
/// in BareClip. It is the brand's own shape, not a Lucide symbol.
pub fn paint_mark(painter: &Painter, rect: Rect, color: Color32) {
    let (ring, play, width) = mark_shape(rect);
    painter.add(Shape::closed_line(ring.to_vec(), Stroke::new(width, color)));
    painter.add(Shape::convex_polygon(play.to_vec(), color, Stroke::NONE));
}

/// A square of the given size around a point, for drawing a symbol in.
pub fn square_at(center: Pos2, side: f32) -> Rect {
    Rect::from_center_size(pos2(center.x, center.y), vec2(side, side))
}

#[cfg(test)]
mod tests {
    use super::*;

    const USED: [Icon; 100] = [
        CAMERA,
        MAXIMIZE_2,
        REPEAT,
        MAXIMIZE,
        MINIMIZE,
        WEBCAM,
        WEBCAM_OFF,
        VIDEO,
        MIC,
        MIC_OFF,
        GRIP_VERTICAL,
        CHEVRON_LEFT,
        LAYERS,
        ALIGN_CENTER_HORIZONTAL,
        ALIGN_CENTER_VERTICAL,
        ALIGN_END_HORIZONTAL,
        ALIGN_END_VERTICAL,
        ALIGN_START_HORIZONTAL,
        ALIGN_START_VERTICAL,
        MOVE,
        APERTURE,
        LIBRARY_BIG,
        SHAPES,
        WAND_SPARKLES,
        CONTRAST,
        SWATCH_BOOK,
        CAPTIONS,
        SCALING,
        SPARKLES,
        ARROW_UP_DOWN,
        CHECK,
        ARROW_LEFT,
        BLEND,
        SLIDERS_HORIZONTAL,
        CHEVRON_DOWN,
        CHEVRON_RIGHT,
        CIRCLE_ALERT,
        CIRCLE_CHECK,
        COPY,
        COPY_PLUS,
        CLIPBOARD_PASTE,
        CLOCK,
        TYPE,
        FILE_PLUS,
        FILM,
        GAUGE,
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
        REDO,
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
        UNDO,
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
        VOLUME,
        VOLUME_1,
        VOLUME_2,
        VOLUME_X,
        MUSIC,
        AUDIO_LINES,
        UNLINK,
        INFO,
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

    #[test]
    fn the_mark_fits_its_square_with_the_play_inside_the_ring() {
        let square = Rect::from_min_size(pos2(10.0, 20.0), vec2(16.0, 16.0));
        let (ring, play, width) = mark_shape(square);
        // The ring's outer edge, half its width out from its middle line, stays in the square.
        let outer = Rect::from_points(&ring).expand(width / 2.0);
        assert!(
            outer.width() <= 16.01 && outer.height() <= 16.01,
            "{outer:?}"
        );
        assert!((outer.center() - square.center()).length() < 0.5, "centred");
        assert!((outer.height() - 16.0).abs() < 1.0, "as tall as the square");
        let inside = Rect::from_points(&ring).shrink(width / 2.0);
        for point in play {
            assert!(
                inside.contains(point),
                "the play triangle is inside the ring"
            );
        }
        assert!(
            width > 1.5 && width < 2.5,
            "about as thick as a symbol's line: {width}"
        );
    }
}
