//! Converts between timeline time and screen position, and draws the timeline.

use bareclip_core::{
    cut::{CutList, RangeEdge, TimeRange},
    time::{Ticks, format_timecode},
};
use eframe::egui::{
    self, Color32, CursorIcon, FontId, Pos2, Rect, Sense, Shape, Stroke, pos2, vec2,
};

use crate::theme::Palette;

const RULER_HEIGHT: f32 = 24.0;
const MIN_LABEL_SPACING: f32 = 90.0;
/// Room a ruler label needs, so one near the edge is left out rather than cut off.
const RULER_LABEL_WIDTH: f32 = 60.0;
const EDGE_GRAB_DISTANCE: f32 = 6.0;
/// The logo's 30 degree cut, as used on the clip corners.
const CORNER_CUT_X: f32 = 10.0;
const CORNER_CUT_Y: f32 = 5.8;
const HANDLE_WIDTH: f32 = 5.0;
const MIN_CLEAN_POINT_SPACING: f32 = 5.0;

/// Maps the whole video onto a stretch of the screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimeScale {
    pub duration: Ticks,
    pub left: f32,
    pub width: f32,
}

impl TimeScale {
    pub fn x_of(&self, time: Ticks) -> f32 {
        if self.duration.0 <= 0 {
            return self.left;
        }
        self.left + (time.0 as f64 / self.duration.0 as f64 * f64::from(self.width)) as f32
    }

    pub fn time_at(&self, x: f32) -> Ticks {
        if self.width <= 0.0 {
            return Ticks(0);
        }
        let fraction = (f64::from(x - self.left) / f64::from(self.width)).clamp(0.0, 1.0);
        Ticks((fraction * self.duration.0 as f64).round() as i64)
    }

    /// The gap between ruler labels, in seconds: the smallest tidy step that keeps
    /// the labels from running into each other.
    pub fn label_step_seconds(&self) -> f64 {
        const STEPS: [f64; 14] = [
            0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0,
        ];
        let seconds = self.duration.as_seconds();
        if seconds <= 0.0 || self.width <= 0.0 {
            return STEPS[0];
        }
        let wanted = seconds * f64::from(MIN_LABEL_SPACING) / f64::from(self.width);
        STEPS
            .iter()
            .copied()
            .find(|step| *step >= wanted)
            .unwrap_or(STEPS[STEPS.len() - 1])
    }
}

/// The section edge within reach of a screen position, if any. The nearest wins.
pub fn edge_near(ranges: &[TimeRange], scale: &TimeScale, x: f32) -> Option<(usize, RangeEdge)> {
    let mut best: Option<(f32, usize, RangeEdge)> = None;
    for (index, range) in ranges.iter().enumerate() {
        for (edge, time) in [(RangeEdge::Start, range.start), (RangeEdge::End, range.end)] {
            let distance = (scale.x_of(time) - x).abs();
            if distance <= EDGE_GRAB_DISTANCE && best.is_none_or(|(nearest, ..)| distance < nearest)
            {
                best = Some((distance, index, edge));
            }
        }
    }
    best.map(|(_, index, edge)| (index, edge))
}

/// What the timeline widget is currently being used for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TimelineDrag {
    #[default]
    None,
    Scrub,
    Edge(usize, RangeEdge),
}

/// Changes the user made on the timeline this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimelineOutput {
    pub seek: Option<Ticks>,
}

fn clip_shape(rect: Rect, fill: Color32, stroke: Stroke) -> Shape {
    let points = vec![
        pos2(
            rect.left() + CORNER_CUT_X.min(rect.width() / 2.0),
            rect.top(),
        ),
        pos2(rect.right(), rect.top()),
        pos2(rect.right(), rect.bottom() - CORNER_CUT_Y),
        pos2(
            rect.right() - CORNER_CUT_X.min(rect.width() / 2.0),
            rect.bottom(),
        ),
        pos2(rect.left(), rect.bottom()),
        pos2(rect.left(), rect.top() + CORNER_CUT_Y),
    ];
    Shape::convex_polygon(points, fill, stroke)
}

/// The playhead's head is the hexagon from the logo.
fn draw_playhead(painter: &egui::Painter, palette: &Palette, x: f32, top: f32, bottom: f32) {
    painter.rect_filled(
        Rect::from_min_max(pos2(x - 2.0, top + 6.0), pos2(x + 2.0, bottom)),
        0.0,
        palette.playhead_outline,
    );
    painter.rect_filled(
        Rect::from_min_max(pos2(x - 1.0, top + 6.0), pos2(x + 1.0, bottom)),
        0.0,
        palette.playhead,
    );
    let hexagon = vec![
        pos2(x, top),
        pos2(x + 7.0, top + 4.0),
        pos2(x + 7.0, top + 12.0),
        pos2(x, top + 16.0),
        pos2(x - 7.0, top + 12.0),
        pos2(x - 7.0, top + 4.0),
    ];
    painter.add(Shape::convex_polygon(
        hexagon,
        palette.playhead,
        Stroke::new(1.0, palette.playhead_outline),
    ));
}

fn draw_ruler(ui: &egui::Ui, palette: &Palette, scale: &TimeScale, rect: Rect) {
    let painter = ui.painter();
    let step = scale.label_step_seconds();
    let total = scale.duration.as_seconds();
    let font = FontId::proportional(11.0);
    let mut seconds = 0.0;
    while seconds <= total {
        let x = scale.x_of(Ticks::from_seconds(seconds));
        painter.line_segment(
            [pos2(x, rect.top() + 6.0), pos2(x, rect.bottom())],
            Stroke::new(1.0, palette.border),
        );
        if x + RULER_LABEL_WIDTH > rect.right() {
            break;
        }
        let label = format_timecode(Ticks::from_seconds(seconds));
        let short = label.split('.').next().unwrap_or(&label).to_owned();
        painter.text(
            pos2(x + 6.0, rect.top() + 6.0),
            egui::Align2::LEFT_TOP,
            short,
            font.clone(),
            palette.text_muted,
        );
        seconds += step;
    }
}

/// Draws the timeline and handles dragging on it.
pub fn timeline(
    ui: &mut egui::Ui,
    palette: &Palette,
    cuts: &mut CutList,
    position: Ticks,
    duration: Ticks,
    clean_points: &[Ticks],
    drag: &mut TimelineDrag,
) -> TimelineOutput {
    let size = vec2(ui.available_width(), ui.available_height().max(70.0));
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let mut output = TimelineOutput::default();

    let inset = 8.0;
    let scale = TimeScale {
        duration,
        left: rect.left() + inset,
        width: (rect.width() - inset * 2.0).max(1.0),
    };
    let ruler = Rect::from_min_size(rect.min, vec2(rect.width(), RULER_HEIGHT));
    let track = Rect::from_min_max(
        pos2(scale.left, ruler.bottom() + 4.0),
        pos2(scale.left + scale.width, rect.bottom() - 8.0),
    );

    // Hovering near an edge offers to move it.
    let hover_edge = response
        .hover_pos()
        .filter(|hover: &Pos2| track.contains(*hover))
        .and_then(|hover| edge_near(cuts.ranges(), &scale, hover.x));
    if hover_edge.is_some() || matches!(*drag, TimelineDrag::Edge(..)) {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    }

    if response.drag_started() {
        *drag = match response.interact_pointer_pos() {
            Some(pointer) if track.contains(pointer) => edge_near(cuts.ranges(), &scale, pointer.x)
                .map_or(TimelineDrag::Scrub, |(index, edge)| {
                    TimelineDrag::Edge(index, edge)
                }),
            _ => TimelineDrag::Scrub,
        };
    }
    if let Some(pointer) = response.interact_pointer_pos() {
        let time = scale.time_at(pointer.x);
        match *drag {
            TimelineDrag::Edge(index, edge) if response.dragged() => {
                cuts.move_edge(index, edge, time);
                output.seek = Some(time);
            }
            TimelineDrag::Scrub if response.dragged() => output.seek = Some(time),
            _ if response.clicked() => output.seek = Some(time),
            _ => {}
        }
    }
    if response.drag_stopped() {
        *drag = TimelineDrag::None;
    }

    draw_ruler(ui, palette, &scale, ruler);

    // The whole video, as a quiet background for the sections kept from it.
    painter.rect_filled(track, 3.0, palette.bg_app);
    painter.rect_stroke(
        track,
        3.0,
        Stroke::new(1.0, palette.border),
        egui::StrokeKind::Inside,
    );

    for (index, range) in cuts.ranges().iter().enumerate() {
        let section = Rect::from_min_max(
            pos2(scale.x_of(range.start), track.top()),
            pos2(scale.x_of(range.end), track.bottom()),
        );
        painter.add(clip_shape(section, palette.clip_video, Stroke::NONE));
        let is_active = hover_edge.is_some_and(|(edge_index, _)| edge_index == index)
            || matches!(*drag, TimelineDrag::Edge(edge_index, _) if edge_index == index);
        if is_active {
            for x in [section.left(), section.right() - HANDLE_WIDTH] {
                painter.rect_filled(
                    Rect::from_min_size(
                        pos2(x, section.top() + 8.0),
                        vec2(HANDLE_WIDTH, (section.height() - 16.0).max(4.0)),
                    ),
                    2.0,
                    palette.clip_video_text,
                );
            }
        }
    }

    if let Some(start) = cuts.pending_in() {
        let pending = Rect::from_min_max(
            pos2(scale.x_of(start), track.top()),
            pos2(scale.left + scale.width, track.bottom()),
        );
        painter.rect_filled(pending, 0.0, palette.clip_video.gamma_multiply(0.35));
        painter.line_segment(
            [pending.left_top(), pending.left_bottom()],
            Stroke::new(2.0, palette.accent),
        );
    }

    // Clean points are where cuts cost nothing, so they are worth seeing. They are
    // left out when so close together that they would only blur into a band.
    if let [first, second, ..] = clean_points
        && (scale.x_of(*second) - scale.x_of(*first)).abs() >= MIN_CLEAN_POINT_SPACING
    {
        for point in clean_points {
            let x = scale.x_of(*point);
            painter.line_segment(
                [pos2(x, track.bottom() - 6.0), pos2(x, track.bottom())],
                Stroke::new(1.0, palette.text_muted),
            );
        }
    }

    draw_playhead(
        &painter,
        palette,
        scale.x_of(position),
        ruler.top() + 2.0,
        track.bottom() + 4.0,
    );
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scale() -> TimeScale {
        TimeScale {
            duration: Ticks::from_seconds(100.0),
            left: 10.0,
            width: 1000.0,
        }
    }

    fn section(start: f64, end: f64) -> TimeRange {
        TimeRange {
            start: Ticks::from_seconds(start),
            end: Ticks::from_seconds(end),
        }
    }

    #[test]
    fn time_and_position_convert_both_ways() {
        let scale = scale();

        assert_eq!(scale.x_of(Ticks::from_seconds(50.0)), 510.0);
        assert_eq!(scale.time_at(510.0), Ticks::from_seconds(50.0));
    }

    #[test]
    fn positions_outside_the_timeline_stop_at_its_ends() {
        let scale = scale();

        assert_eq!(scale.time_at(-50.0), Ticks(0));
        assert_eq!(scale.time_at(5000.0), scale.duration);
    }

    #[test]
    fn an_empty_video_does_not_divide_by_zero() {
        let empty = TimeScale {
            duration: Ticks(0),
            left: 10.0,
            width: 100.0,
        };

        assert_eq!(empty.x_of(Ticks(5)), 10.0);
        assert_eq!(empty.time_at(50.0), Ticks(0));
        assert_eq!(empty.label_step_seconds(), 0.5);
    }

    #[test]
    fn a_collapsed_timeline_does_not_divide_by_zero() {
        let collapsed = TimeScale {
            duration: Ticks::from_seconds(10.0),
            left: 0.0,
            width: 0.0,
        };

        assert_eq!(collapsed.time_at(5.0), Ticks(0));
        assert_eq!(collapsed.label_step_seconds(), 0.5);
    }

    #[test]
    fn ruler_labels_never_crowd_each_other() {
        for (seconds, width) in [
            (10.0, 1000.0),
            (100.0, 1000.0),
            (600.0, 1200.0),
            (7200.0, 800.0),
        ] {
            let scale = TimeScale {
                duration: Ticks::from_seconds(seconds),
                left: 0.0,
                width,
            };
            let step = scale.label_step_seconds();
            let spacing = step / seconds * f64::from(width);

            assert!(
                spacing >= f64::from(MIN_LABEL_SPACING),
                "{seconds}s: {spacing}px"
            );
        }
    }

    #[test]
    fn very_long_videos_use_the_largest_step() {
        let scale = TimeScale {
            duration: Ticks::from_seconds(360_000.0),
            left: 0.0,
            width: 800.0,
        };

        assert_eq!(scale.label_step_seconds(), 3600.0);
    }

    #[test]
    fn edges_are_found_within_reach() {
        let scale = scale();
        let ranges = [section(10.0, 20.0), section(50.0, 60.0)];

        assert_eq!(
            edge_near(&ranges, &scale, 112.0),
            Some((0, RangeEdge::Start))
        );
        assert_eq!(edge_near(&ranges, &scale, 208.0), Some((0, RangeEdge::End)));
        assert_eq!(edge_near(&ranges, &scale, 610.0), Some((1, RangeEdge::End)));
    }

    #[test]
    fn positions_away_from_edges_find_nothing() {
        let scale = scale();

        assert_eq!(edge_near(&[section(10.0, 20.0)], &scale, 160.0), None);
        assert_eq!(edge_near(&[], &scale, 160.0), None);
    }

    #[test]
    fn the_nearest_of_two_close_edges_wins() {
        let scale = scale();
        let ranges = [section(10.0, 20.0), section(20.5, 30.0)];

        assert_eq!(edge_near(&ranges, &scale, 209.0), Some((0, RangeEdge::End)));
        assert_eq!(
            edge_near(&ranges, &scale, 215.5),
            Some((1, RangeEdge::Start))
        );
    }
}
