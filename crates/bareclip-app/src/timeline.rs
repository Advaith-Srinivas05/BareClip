//! Converts between timeline time and screen position, and draws the timeline.

use bareclip_core::{
    time::{Ticks, format_timecode},
    timeline::{ClipEdge, ClipId, MediaId, TimeRange, Timeline},
};
use eframe::egui::{
    self, Align2, Button, Color32, CursorIcon, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke,
    UiBuilder, pos2, vec2,
};

use crate::{text, theme::Palette};

const RULER_HEIGHT: f32 = 24.0;
/// The row under the track, holding the scrollbar and the zoom buttons.
const STRIP_HEIGHT: f32 = 30.0;
const SCROLLBAR_HEIGHT: f32 = 10.0;
const MIN_THUMB_WIDTH: f32 = 28.0;
const ZOOM_BUTTON_SIZE: f32 = 26.0;
const MIN_LABEL_SPACING: f32 = 90.0;
/// Room a ruler label needs, so one near the edge is left out rather than cut off.
const RULER_LABEL_WIDTH: f32 = 60.0;
/// The logo's 30 degree cut, as used on the clip corners.
const CORNER_CUT_X: f32 = 10.0;
const CORNER_CUT_Y: f32 = 5.8;
const HANDLE_WIDTH: f32 = 7.0;
const HANDLE_GRAB_WIDTH: f32 = 10.0;
const ZOOM_STEP: f64 = 1.5;
/// Below this, a view of the timeline stops getting any closer.
const MIN_VISIBLE_SECONDS: f64 = 0.5;
/// The timeline always runs on past the video, so there is room to drag things into.
const MIN_EXTENT_SECONDS: f64 = 10.0;
const EXTENT_PER_PROJECT: i64 = 2;
/// Fitting leaves some of the empty timeline in view after the last clip.
const FIT_FRACTION: f64 = 0.75;
/// How close a dragged clip gets to something before it snaps to it.
const SNAP_PIXELS: f32 = 8.0;
/// Holding a drag this close to the edge of the view scrolls it.
const EDGE_SCROLL_ZONE: f32 = 28.0;
const EDGE_SCROLL_VIEWS_PER_SECOND: f64 = 0.8;
/// A gap narrower than this has no room for its button. Delete still removes it.
const GAP_BUTTON_WIDTH: f32 = 92.0;
const HATCH_SPACING: f32 = 8.0;

/// Maps a stretch of the timeline onto a stretch of the screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimeScale {
    pub visible_start: Ticks,
    pub visible: Ticks,
    pub left: f32,
    pub width: f32,
}

impl TimeScale {
    pub fn x_of(&self, time: Ticks) -> f32 {
        if self.visible.0 <= 0 {
            return self.left;
        }
        let fraction = (time.0 - self.visible_start.0) as f64 / self.visible.0 as f64;
        self.left + (fraction * f64::from(self.width)) as f32
    }

    pub fn time_at(&self, x: f32) -> Ticks {
        if self.width <= 0.0 {
            return self.visible_start;
        }
        let fraction = f64::from(x - self.left) / f64::from(self.width);
        Ticks(self.visible_start.0 + (fraction * self.visible.0 as f64).round() as i64)
    }

    /// The gap between ruler labels, in seconds: the smallest tidy step that keeps
    /// the labels from running into each other.
    pub fn label_step_seconds(&self) -> f64 {
        const STEPS: [f64; 15] = [
            0.1, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0,
            3600.0,
        ];
        let seconds = self.visible.as_seconds();
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

    /// How much time a stretch of pixels covers.
    fn time_of_pixels(&self, pixels: f32) -> Ticks {
        if self.width <= 0.0 {
            return Ticks(0);
        }
        Ticks((f64::from(pixels) / f64::from(self.width) * self.visible.0 as f64) as i64)
    }
}

/// A clip as the timeline draws it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipView {
    pub id: ClipId,
    pub start: Ticks,
    pub end: Ticks,
    pub label: String,
}

/// What the timeline needs to know to draw the project.
pub struct TimelineView<'a> {
    pub timeline: &'a Timeline,
    pub clips: &'a [ClipView],
    pub position: Ticks,
    pub selected: Option<ClipId>,
    pub selected_gap: Option<Ticks>,
}

/// A video being dragged from the media panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaDrag {
    pub media: MediaId,
    pub length: Ticks,
}

/// The part of a clip under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipPart {
    Handle(ClipEdge),
    Body,
}

/// Finds the clip and part at a screen position. Handles shrink on tiny clips, so
/// there is always some middle left to grab.
pub fn part_at(clips: &[ClipView], scale: &TimeScale, x: f32) -> Option<(usize, ClipPart)> {
    for (index, clip) in clips.iter().enumerate() {
        let left = scale.x_of(clip.start);
        let right = scale.x_of(clip.end);
        if x < left || x > right {
            continue;
        }
        let grab = HANDLE_GRAB_WIDTH.min((right - left) / 3.0);
        let part = if x <= left + grab {
            ClipPart::Handle(ClipEdge::Start)
        } else if x >= right - grab {
            ClipPart::Handle(ClipEdge::End)
        } else {
            ClipPart::Body
        };
        return Some((index, part));
    }
    None
}

/// Nudges something being dragged so one of its ends lines up with a nearby moment.
pub fn snap_start(wanted: Ticks, length: Ticks, targets: &[Ticks], tolerance: Ticks) -> Ticks {
    let mut best = wanted;
    let mut closest = tolerance.0;
    for target in targets {
        for candidate in [target.0, target.0 - length.0] {
            let distance = (candidate - wanted.0).abs();
            if distance <= closest && candidate >= 0 {
                closest = distance;
                best = Ticks(candidate);
            }
        }
    }
    best
}

/// How fast a drag near the edge of the view should scroll it, from -1 to 1.
fn edge_scroll_speed(x: f32, left: f32, width: f32) -> f64 {
    let into_left = (left + EDGE_SCROLL_ZONE - x) / EDGE_SCROLL_ZONE;
    let into_right = (x - (left + width - EDGE_SCROLL_ZONE)) / EDGE_SCROLL_ZONE;
    if into_left > 0.0 {
        -f64::from(into_left.min(1.0))
    } else if into_right > 0.0 {
        f64::from(into_right.min(1.0))
    } else {
        0.0
    }
}

/// Where the scrollbar's thumb sits: its left edge from the bar's start, and its width.
fn thumb_span(bar_width: f32, scroll: Ticks, visible: Ticks, extent: Ticks) -> (f32, f32) {
    if extent.0 <= 0 {
        return (0.0, bar_width);
    }
    let fraction = (visible.0 as f64 / extent.0 as f64).min(1.0);
    let width = ((fraction * f64::from(bar_width)) as f32)
        .max(MIN_THUMB_WIDTH)
        .min(bar_width);
    let room = extent.0 - visible.0;
    if room <= 0 {
        return (0.0, width);
    }
    let left = scroll.0 as f64 / room as f64 * f64::from(bar_width - width);
    (left as f32, width)
}

/// What the timeline widget is currently being used for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TimelineDrag {
    #[default]
    None,
    Scrub,
    Trim(ClipId, ClipEdge),
    /// `grab` is how far along the clip it was picked up.
    Move {
        id: ClipId,
        grab: Ticks,
    },
}

/// The timeline's view, kept between frames.
#[derive(Clone, Copy, Debug)]
pub struct TimelineState {
    /// How much of the timeline fits on screen.
    visible: Ticks,
    /// Set until the user zooms by hand, so adding clips keeps the whole project in view.
    auto_fit: bool,
    pub scroll: Ticks,
    pub drag: TimelineDrag,
    /// The total held still during a trim, so the view does not shift under the pointer.
    frozen_total: Option<Ticks>,
    /// Where things were drawn in the last frame.
    pub track: Rect,
    pub scale: TimeScale,
    pub scrollbar: Rect,
    pub zoom_buttons: [Rect; 3],
    /// The moment under the pointer, if it is over the timeline.
    pub hover_time: Option<Ticks>,
}

impl Default for TimelineState {
    fn default() -> Self {
        Self {
            visible: Ticks::from_seconds(MIN_EXTENT_SECONDS),
            auto_fit: true,
            scroll: Ticks(0),
            drag: TimelineDrag::None,
            frozen_total: None,
            track: Rect::NOTHING,
            scale: TimeScale {
                visible_start: Ticks(0),
                visible: Ticks(0),
                left: 0.0,
                width: 0.0,
            },
            scrollbar: Rect::NOTHING,
            zoom_buttons: [Rect::NOTHING; 3],
            hover_time: None,
        }
    }
}

impl TimelineState {
    /// How long the whole scrollable timeline is, including the empty part after the
    /// last clip.
    fn extent(total: Ticks) -> Ticks {
        Ticks((total.0 * EXTENT_PER_PROJECT).max(Ticks::from_seconds(MIN_EXTENT_SECONDS).0))
    }

    fn visible_for(&self, total: Ticks) -> Ticks {
        self.visible.min(Self::extent(total))
    }

    /// Keeps the view from scrolling past the ends of the timeline.
    fn clamp_scroll(&mut self, total: Ticks) {
        let furthest = (Self::extent(total).0 - self.visible_for(total).0).max(0);
        self.scroll = Ticks(self.scroll.0.clamp(0, furthest));
    }

    /// Zooms by a factor, keeping the moment under `anchor` where it is on screen.
    pub fn zoom_by(&mut self, factor: f64, anchor: Ticks, total: Ticks) {
        let before = self.visible_for(total);
        let anchor_fraction = if before.0 > 0 {
            (anchor.0 - self.scroll.0) as f64 / before.0 as f64
        } else {
            0.0
        };
        let closest = Ticks::from_seconds(MIN_VISIBLE_SECONDS).0;
        let furthest = Self::extent(total).0;
        self.visible = Ticks(((before.0 as f64 / factor) as i64).clamp(closest, furthest));
        self.auto_fit = false;
        self.scroll = Ticks(anchor.0 - (anchor_fraction * self.visible.0 as f64) as i64);
        self.clamp_scroll(total);
    }

    pub fn zoom_in(&mut self, anchor: Ticks, total: Ticks) {
        self.zoom_by(ZOOM_STEP, anchor, total);
    }

    pub fn zoom_out(&mut self, anchor: Ticks, total: Ticks) {
        self.zoom_by(1.0 / ZOOM_STEP, anchor, total);
    }

    /// Shows the whole project, with some of the empty timeline after it.
    pub fn fit(&mut self, total: Ticks) {
        let wanted = if total.0 > 0 {
            (total.0 as f64 / FIT_FRACTION) as i64
        } else {
            Self::extent(total).0
        };
        self.visible = Ticks(wanted.max(Ticks::from_seconds(MIN_VISIBLE_SECONDS).0));
        self.scroll = Ticks(0);
        self.auto_fit = true;
    }

    /// Keeps a new clip in view, unless the user has chosen their own zoom.
    pub fn content_added(&mut self, total: Ticks) {
        if self.auto_fit {
            self.fit(total);
        }
    }

    /// Scrolls just enough to bring a moment into view.
    pub fn follow(&mut self, position: Ticks, total: Ticks) {
        let visible = self.visible_for(total);
        if position < self.scroll || position.0 > self.scroll.0 + visible.0 {
            self.scroll = position;
            self.clamp_scroll(total);
        }
    }

    /// Moves the view by a stretch of the scrollbar's own length.
    fn scroll_by_thumb(&mut self, pixels: f32, bar_width: f32, total: Ticks) {
        let extent = Self::extent(total);
        let visible = self.visible_for(total);
        let (_, thumb) = thumb_span(bar_width, self.scroll, visible, extent);
        let travel = bar_width - thumb;
        if travel > 0.0 {
            let room = (extent.0 - visible.0) as f64;
            self.scroll = Ticks(self.scroll.0 + (f64::from(pixels / travel) * room) as i64);
            self.clamp_scroll(total);
        }
    }
}

/// Changes the user made on the timeline this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimelineEvents {
    pub seek: Option<Ticks>,
    pub select: Option<Option<ClipId>>,
    /// Comes with `select`: the dead space that was clicked on, if any.
    pub select_gap: Option<Ticks>,
    pub remove_gap: Option<Ticks>,
    /// A trim began, with the pointer's time when it did.
    pub trim_started: Option<(ClipId, ClipEdge, Ticks)>,
    pub trim_to: Option<(ClipId, ClipEdge, Ticks)>,
    pub trim_finished: bool,
    pub move_clip: Option<(ClipId, Ticks)>,
    pub drop_media: Option<(MediaId, Ticks)>,
}

fn clip_shape(rect: Rect, fill: Color32, stroke: Stroke) -> Shape {
    let cut = CORNER_CUT_X.min(rect.width() / 2.0);
    let points = vec![
        pos2(rect.left() + cut, rect.top()),
        pos2(rect.right(), rect.top()),
        pos2(rect.right(), rect.bottom() - CORNER_CUT_Y),
        pos2(rect.right() - cut, rect.bottom()),
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

fn draw_ruler(painter: &egui::Painter, palette: &Palette, scale: &TimeScale, rect: Rect) {
    let step = scale.label_step_seconds();
    let font = FontId::proportional(11.0);
    let first = (scale.visible_start.as_seconds() / step).floor() * step;
    let last = scale.visible_start.as_seconds() + scale.visible.as_seconds();
    let mut seconds = first;
    while seconds <= last {
        let x = scale.x_of(Ticks::from_seconds(seconds));
        if x >= rect.left() {
            painter.line_segment(
                [pos2(x, rect.top() + 6.0), pos2(x, rect.bottom())],
                Stroke::new(1.0, palette.border),
            );
            if x + RULER_LABEL_WIDTH <= rect.right() {
                let label = format_timecode(Ticks::from_seconds(seconds));
                let text = if step >= 1.0 {
                    label.split('.').next().unwrap_or(&label).to_owned()
                } else {
                    label
                };
                painter.text(
                    pos2(x + 6.0, rect.top() + 6.0),
                    Align2::LEFT_TOP,
                    text,
                    font.clone(),
                    palette.text_muted,
                );
            }
        }
        seconds += step;
    }
}

/// Diagonal lines, which mark a stretch as dead space on the ruler and the track.
fn draw_hatch(painter: &egui::Painter, rect: Rect, color: Color32) {
    let shown = rect.intersect(painter.clip_rect());
    if !shown.is_positive() {
        return;
    }
    let hatch = painter.with_clip_rect(shown);
    let height = shown.height();
    let stroke = Stroke::new(1.0, color);
    let mut x = shown.left() - height;
    while x < shown.right() {
        hatch.line_segment(
            [pos2(x, shown.bottom()), pos2(x + height, shown.top())],
            stroke,
        );
        x += HATCH_SPACING;
    }
}

/// A small label on the ruler, in the colours that stand out most from it.
fn draw_pill(painter: &egui::Painter, palette: &Palette, area: Rect, x: f32, text: String) {
    let galley = painter.layout_no_wrap(text, FontId::proportional(12.0), palette.surface);
    let size = galley.size() + vec2(12.0, 5.0);
    let left = (x - size.x / 2.0)
        .min(area.right() - size.x)
        .max(area.left());
    let rect = Rect::from_min_size(pos2(left, area.center().y - size.y / 2.0), size);
    painter.rect_filled(rect, 4.0, palette.text);
    painter.galley(rect.min + vec2(6.0, 2.5), galley, palette.surface);
}

fn draw_clip(painter: &egui::Painter, palette: &Palette, rect: Rect, label: &str, selected: bool) {
    let stroke = if selected {
        Stroke::new(2.0, palette.text)
    } else {
        Stroke::NONE
    };
    painter.add(clip_shape(rect, palette.clip_video, stroke));

    // A handle on each end shows the clip can be dragged shorter or longer.
    let width = HANDLE_WIDTH.min(rect.width() / 3.0);
    let inset = 8.0;
    for x in [rect.left() + 2.0, rect.right() - 2.0 - width] {
        painter.rect_filled(
            Rect::from_min_size(
                pos2(x, rect.top() + inset),
                vec2(width, (rect.height() - inset * 2.0).max(4.0)),
            ),
            2.0,
            palette.clip_video_text,
        );
    }

    if rect.width() > 60.0 {
        let text_rect = rect.shrink2(vec2(14.0, 0.0));
        painter
            .with_clip_rect(text_rect.intersect(painter.clip_rect()))
            .text(
                pos2(text_rect.left(), rect.center().y),
                Align2::LEFT_CENTER,
                label,
                FontId::proportional(12.0),
                palette.clip_video_text,
            );
    }
}

/// An outline of where a dragged clip or video will land.
fn draw_landing(painter: &egui::Painter, palette: &Palette, rect: Rect) {
    painter.add(clip_shape(
        rect,
        palette.clip_video.gamma_multiply(0.6),
        Stroke::new(2.0, palette.accent),
    ));
}

fn clip_rect(scale: &TimeScale, track: Rect, start: Ticks, end: Ticks) -> Rect {
    Rect::from_min_max(
        pos2(scale.x_of(start), track.top() + 2.0),
        pos2(scale.x_of(end) - 1.0, track.bottom() - 2.0),
    )
}

/// Everything a dragged clip's edges can line up with.
fn snap_targets(view: &TimelineView, moving: Option<ClipId>) -> Vec<Ticks> {
    let mut targets = vec![Ticks(0), view.position];
    for clip in view.clips {
        if Some(clip.id) != moving {
            targets.push(clip.start);
            targets.push(clip.end);
        }
    }
    targets
}

#[derive(Clone, Copy)]
enum ZoomIcon {
    Out,
    In,
    Fit,
}

fn zoom_button(
    ui: &mut egui::Ui,
    palette: &Palette,
    rect: Rect,
    icon: ZoomIcon,
    label: &str,
    hint: &str,
) -> egui::Response {
    let response = ui
        .interact(
            rect,
            egui::Id::unique(("timeline-zoom", label)),
            Sense::click(),
        )
        .on_hover_text(hint);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label.to_owned()));
    let painter = ui.painter_at(rect);
    if response.hovered() {
        painter.rect_filled(rect, 4.0, palette.raised);
    }
    let stroke = Stroke::new(1.6, palette.text);
    let center = rect.center();
    match icon {
        ZoomIcon::Out | ZoomIcon::In => {
            let lens = center - vec2(2.0, 2.0);
            painter.circle_stroke(lens, 6.5, stroke);
            painter.line_segment([lens + vec2(4.8, 4.8), lens + vec2(9.5, 9.5)], stroke);
            painter.line_segment([lens - vec2(3.0, 0.0), lens + vec2(3.0, 0.0)], stroke);
            if matches!(icon, ZoomIcon::In) {
                painter.line_segment([lens - vec2(0.0, 3.0), lens + vec2(0.0, 3.0)], stroke);
            }
        }
        ZoomIcon::Fit => {
            for side in [-1.0, 1.0] {
                let wall = center + vec2(side * 8.0, 0.0);
                painter.line_segment([wall - vec2(0.0, 6.0), wall + vec2(0.0, 6.0)], stroke);
                let tip = center + vec2(side * 5.5, 0.0);
                painter.line_segment([tip, tip - vec2(side * 3.0, 3.0)], stroke);
                painter.line_segment([tip, tip - vec2(side * 3.0, -3.0)], stroke);
            }
            painter.line_segment([center - vec2(5.5, 0.0), center + vec2(5.5, 0.0)], stroke);
        }
    }
    response
}

/// Draws the scrollbar, which shows how much of the whole timeline is in view.
fn scrollbar(
    ui: &mut egui::Ui,
    palette: &Palette,
    state: &mut TimelineState,
    bar: Rect,
    total: Ticks,
) {
    let response = ui.interact(
        bar,
        egui::Id::unique("timeline-scrollbar"),
        Sense::click_and_drag(),
    );
    let extent = TimelineState::extent(total);
    if response.dragged() {
        state.scroll_by_thumb(response.drag_delta().x, bar.width(), total);
    } else if response.clicked()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let (left, thumb) = thumb_span(bar.width(), state.scroll, state.visible_for(total), extent);
        let middle = bar.left() + left + thumb / 2.0;
        state.scroll_by_thumb(pointer.x - middle, bar.width(), total);
    }

    let (left, thumb) = thumb_span(bar.width(), state.scroll, state.visible_for(total), extent);
    let painter = ui.painter_at(bar);
    painter.rect_filled(bar, 5.0, palette.bg_app);
    let thumb_rect = Rect::from_min_size(bar.min + vec2(left, 0.0), vec2(thumb, bar.height()));
    let fill = if response.dragged() || response.hovered() {
        palette.text_muted
    } else {
        palette.border_strong
    };
    painter.rect_filled(thumb_rect, 5.0, fill);
    state.scrollbar = bar;
}

/// Draws the row under the track.
fn bottom_strip(
    ui: &mut egui::Ui,
    palette: &Palette,
    state: &mut TimelineState,
    strip: Rect,
    left: f32,
    view: &TimelineView,
) {
    let total = state
        .frozen_total
        .unwrap_or_else(|| view.timeline.duration());
    let size = vec2(ZOOM_BUTTON_SIZE, ZOOM_BUTTON_SIZE);
    let right = strip.right() - 8.0;
    let middle = strip.center().y;
    let button_at = |index: f32| {
        Rect::from_center_size(
            pos2(right - (index + 0.5) * (ZOOM_BUTTON_SIZE + 2.0), middle),
            size,
        )
    };
    let [out_rect, in_rect, fit_rect] = [button_at(2.0), button_at(1.0), button_at(0.0)];
    state.zoom_buttons = [out_rect, in_rect, fit_rect];

    // Zooming holds on to the playhead if it can be seen, or the middle of the view if not.
    let visible = state.visible_for(total);
    let in_view = view.position >= state.scroll && view.position.0 <= state.scroll.0 + visible.0;
    let anchor = if in_view {
        view.position
    } else {
        Ticks(state.scroll.0 + visible.0 / 2)
    };
    if zoom_button(
        ui,
        palette,
        out_rect,
        ZoomIcon::Out,
        "Zoom out",
        "Zoom out (Ctrl + mouse wheel)",
    )
    .clicked()
    {
        state.zoom_out(anchor, total);
    }
    if zoom_button(
        ui,
        palette,
        in_rect,
        ZoomIcon::In,
        "Zoom in",
        "Zoom in (Ctrl + mouse wheel)",
    )
    .clicked()
    {
        state.zoom_in(anchor, total);
    }
    if zoom_button(
        ui,
        palette,
        fit_rect,
        ZoomIcon::Fit,
        "Fit",
        "Fit the whole video in view",
    )
    .clicked()
    {
        state.fit(total);
    }

    let bar = Rect::from_min_max(
        pos2(left, middle - SCROLLBAR_HEIGHT / 2.0),
        pos2(out_rect.left() - 10.0, middle + SCROLLBAR_HEIGHT / 2.0),
    );
    scrollbar(ui, palette, state, bar, total);
}

/// Draws the timeline and reports what the user did with it.
pub fn timeline(
    ui: &mut egui::Ui,
    palette: &Palette,
    state: &mut TimelineState,
    view: &TimelineView,
) -> TimelineEvents {
    let total = view.timeline.duration();
    let clips = view.clips;
    let width = ui.available_width();
    // The gap between the two rows counts too, or the panel would creep taller each frame.
    let strip_room = STRIP_HEIGHT + ui.spacing().item_spacing.y;
    let height = (ui.available_height() - strip_room).max(70.0);
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let mut events = TimelineEvents::default();
    let inset = 8.0;
    let left = rect.left() + inset;
    let track_width = (rect.width() - inset * 2.0).max(1.0);

    let view_total = state.frozen_total.unwrap_or(total);
    state.clamp_scroll(view_total);

    if state.drag != TimelineDrag::None
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let speed = edge_scroll_speed(pointer.x, left, track_width);
        if speed != 0.0 {
            let delta = f64::from(ui.input(|input| input.stable_dt).min(0.1));
            let shift = speed * EDGE_SCROLL_VIEWS_PER_SECOND * delta;
            state.scroll =
                Ticks(state.scroll.0 + (shift * state.visible_for(view_total).0 as f64) as i64);
            state.clamp_scroll(view_total);
            ui.ctx().request_repaint();
        }
    }

    let scale = TimeScale {
        visible_start: state.scroll,
        visible: state.visible_for(view_total),
        left,
        width: track_width,
    };
    let ruler = Rect::from_min_size(rect.min, vec2(rect.width(), RULER_HEIGHT));
    let track = Rect::from_min_max(
        pos2(scale.left, ruler.bottom() + 4.0),
        pos2(scale.left + scale.width, rect.bottom() - 6.0),
    );
    state.track = track;
    state.scale = scale;

    let pointer_over = response
        .contains_pointer()
        .then(|| ui.input(|input| input.pointer.latest_pos()))
        .flatten();
    let hover_time = pointer_over.map(|pointer| Ticks(scale.time_at(pointer.x).0.max(0)));
    state.hover_time = hover_time;

    // Wheel over the timeline: Ctrl zooms, otherwise it scrolls.
    if let (Some(hover), Some(time)) = (pointer_over, hover_time) {
        let (delta, zoom_key) = ui.input(|input| {
            let scroll = input.smooth_scroll_delta;
            (scroll.x + scroll.y, input.modifiers.command)
        });
        if delta != 0.0 && rect.contains(hover) {
            if zoom_key {
                state.zoom_by(f64::from(delta * 0.005).exp(), time, view_total);
            } else {
                let shift = f64::from(-delta) / f64::from(scale.width) * scale.visible.0 as f64;
                state.scroll = Ticks(state.scroll.0 + shift as i64);
                state.clamp_scroll(view_total);
            }
        }
    }

    let hovered_part = pointer_over
        .filter(|hover: &Pos2| track.contains(*hover))
        .and_then(|hover| part_at(clips, &scale, hover.x));
    let hovered_gap = pointer_over
        .filter(|hover: &Pos2| track.contains(*hover))
        .and_then(|hover| view.timeline.gap_at(scale.time_at(hover.x)));
    if matches!(hovered_part, Some((_, ClipPart::Handle(_))))
        || matches!(state.drag, TimelineDrag::Trim(..))
    {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    } else if matches!(state.drag, TimelineDrag::Move { .. }) {
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    } else if matches!(hovered_part, Some((_, ClipPart::Body))) {
        ui.ctx().set_cursor_icon(CursorIcon::Grab);
    }

    if response.drag_started() {
        // A drag is only reported once the pointer has moved, so what was grabbed
        // depends on where the press began rather than where the pointer is now.
        let pointer = ui
            .input(|input| input.pointer.press_origin())
            .or_else(|| response.interact_pointer_pos());
        let part = pointer
            .filter(|pointer| track.contains(*pointer))
            .and_then(|pointer| part_at(clips, &scale, pointer.x));
        let time = pointer.map_or(Ticks(0), |pointer| scale.time_at(pointer.x));
        state.drag = match part {
            Some((index, ClipPart::Handle(edge))) => TimelineDrag::Trim(clips[index].id, edge),
            Some((index, ClipPart::Body)) => TimelineDrag::Move {
                id: clips[index].id,
                grab: Ticks(time.0 - clips[index].start.0),
            },
            None => TimelineDrag::Scrub,
        };
        match state.drag {
            TimelineDrag::Trim(id, edge) => {
                state.frozen_total = Some(total);
                events.select = Some(Some(id));
                events.trim_started = Some((id, edge, time));
            }
            TimelineDrag::Move { id, .. } => events.select = Some(Some(id)),
            _ => {}
        }
    }
    let tolerance = scale.time_of_pixels(SNAP_PIXELS);
    let mut move_landing = None;
    if let Some(pointer) = response.interact_pointer_pos() {
        let time = scale.time_at(pointer.x);
        match state.drag {
            TimelineDrag::Trim(id, edge) if response.dragged() => {
                events.trim_to = Some((id, edge, time));
            }
            TimelineDrag::Scrub if response.dragged() => {
                events.seek = Some(Ticks(time.0.clamp(0, total.0)));
            }
            TimelineDrag::Move { id, grab } => {
                if let Some(clip) = clips.iter().find(|clip| clip.id == id) {
                    let length = Ticks(clip.end.0 - clip.start.0);
                    let wanted = Ticks((time.0 - grab.0).max(0));
                    let snapped =
                        snap_start(wanted, length, &snap_targets(view, Some(id)), tolerance);
                    let landing = view.timeline.landing(Some(id), snapped, length);
                    move_landing = Some((id, landing, Ticks(landing.0 + length.0)));
                    if response.drag_stopped() {
                        events.move_clip = Some((id, snapped));
                    }
                }
            }
            _ => {}
        }
        if response.clicked() {
            events.seek = Some(Ticks(time.0.clamp(0, total.0)));
            let in_track = track.contains(pointer);
            let clicked_clip = in_track
                .then(|| part_at(clips, &scale, pointer.x))
                .flatten();
            events.select = Some(clicked_clip.map(|(index, _)| clips[index].id));
            if in_track && view.timeline.gap_at(time).is_some() {
                events.select_gap = Some(time);
            }
        }
    }
    if response.drag_stopped() {
        if matches!(state.drag, TimelineDrag::Trim(..)) {
            events.trim_finished = true;
        }
        state.drag = TimelineDrag::None;
        state.frozen_total = None;
    }

    // The ruler: the project's own length is tinted, and dead space within it hatched.
    let project_band = Rect::from_min_max(
        pos2(scale.x_of(Ticks(0)).max(rect.left()), ruler.top() + 2.0),
        pos2(scale.x_of(total).min(rect.right()), ruler.bottom() - 2.0),
    );
    if project_band.is_positive() {
        painter.rect_filled(project_band, 3.0, palette.accent.gamma_multiply(0.18));
    }
    let gaps = view.timeline.gaps();
    let selected_gap = view
        .selected_gap
        .and_then(|time| view.timeline.gap_at(time));
    let gap_rect = |gap: TimeRange, top: f32, bottom: f32| {
        Rect::from_min_max(
            pos2(scale.x_of(gap.start), top),
            pos2(scale.x_of(gap.end), bottom),
        )
    };
    for gap in &gaps {
        let band = gap_rect(*gap, ruler.top() + 2.0, ruler.bottom() - 2.0);
        painter.rect_filled(band, 0.0, palette.surface);
        draw_hatch(&painter, band, palette.border_strong);
    }
    draw_ruler(&painter, palette, &scale, ruler);

    painter.rect_filled(track, 3.0, palette.bg_app);
    painter.rect_stroke(
        track,
        3.0,
        Stroke::new(1.0, palette.border),
        egui::StrokeKind::Inside,
    );

    let track_painter = painter.with_clip_rect(track.expand(2.0));
    for gap in &gaps {
        let band = gap_rect(*gap, track.top() + 2.0, track.bottom() - 2.0);
        draw_hatch(&track_painter, band, palette.border);
        if selected_gap == Some(*gap) {
            track_painter.rect_stroke(
                band,
                2.0,
                Stroke::new(2.0, palette.accent),
                egui::StrokeKind::Inside,
            );
        } else if hovered_gap == Some(*gap) {
            track_painter.rect_stroke(
                band,
                2.0,
                Stroke::new(1.0, palette.border_strong),
                egui::StrokeKind::Inside,
            );
        }
    }
    for clip in clips {
        let shape = clip_rect(&scale, track, clip.start, clip.end);
        if shape.right() < track.left() || shape.left() > track.right() {
            continue;
        }
        let moving = move_landing.is_some_and(|(id, ..)| id == clip.id);
        let mut clip_painter = track_painter.clone();
        if moving {
            clip_painter.multiply_opacity(0.45);
        }
        draw_clip(
            &clip_painter,
            palette,
            shape,
            &clip.label,
            view.selected == Some(clip.id),
        );
    }
    if let Some((_, start, end)) = move_landing {
        draw_landing(
            &track_painter,
            palette,
            clip_rect(&scale, track, start, end),
        );
    }
    if clips.is_empty() {
        painter.text(
            track.center(),
            Align2::CENTER_CENTER,
            "Drag a video here, or press + on it",
            FontId::proportional(13.0),
            palette.text_muted,
        );
    }

    // Shows where a video dragged from the panel will land.
    let drop_at = |drag: &MediaDrag, time: Ticks| {
        snap_start(time, drag.length, &snap_targets(view, None), tolerance)
    };
    if let (Some(drag), Some(time)) = (response.dnd_hover_payload::<MediaDrag>(), hover_time) {
        let start = view
            .timeline
            .landing(None, drop_at(&drag, time), drag.length);
        let end = Ticks(start.0 + drag.length.0);
        draw_landing(
            &track_painter,
            palette,
            clip_rect(&scale, track, start, end),
        );
    }
    if let (Some(drag), Some(time)) = (response.dnd_release_payload::<MediaDrag>(), hover_time) {
        events.drop_media = Some((drag.media, drop_at(&drag, time)));
    }

    // The button on a gap, the same way a hovered or selected clip shows its handles.
    if let Some(gap) = hovered_gap.or(selected_gap)
        && state.drag == TimelineDrag::None
    {
        let band = gap_rect(gap, track.top(), track.bottom());
        if band.width() >= GAP_BUTTON_WIDTH + 8.0 {
            let place = Rect::from_center_size(band.center(), vec2(GAP_BUTTON_WIDTH, 26.0))
                .intersect(track);
            let mut child = ui.new_child(UiBuilder::new().max_rect(place));
            let button = Button::new(RichText::new("Remove gap").size(12.0));
            if child
                .add_sized(place.size(), button)
                .on_hover_text("Close this dead space (Delete when it is selected)")
                .clicked()
            {
                events.remove_gap = Some(gap.start);
            }
        }
    }

    if let Some(selected) = view
        .selected
        .and_then(|id| clips.iter().find(|clip| clip.id == id))
    {
        let middle = Ticks((selected.start.0 + selected.end.0) / 2);
        let x = scale.x_of(middle);
        if x > rect.left() && x < rect.right() {
            let length = text::clip_length(Ticks(selected.end.0 - selected.start.0));
            let pill = painter.layout_no_wrap(length, FontId::proportional(11.0), palette.text);
            let size = pill.size() + vec2(10.0, 3.0);
            let shape = Rect::from_center_size(pos2(x, ruler.center().y), size);
            painter.rect_filled(shape, 4.0, palette.raised);
            painter.rect_stroke(
                shape,
                4.0,
                Stroke::new(1.0, palette.accent),
                egui::StrokeKind::Inside,
            );
            painter.galley(shape.min + vec2(5.0, 1.5), pill, palette.text);
        }
    }

    let x = scale.x_of(view.position);
    if x >= track.left() - 1.0 && x <= track.right() + 1.0 {
        draw_playhead(
            &painter,
            palette,
            x,
            ruler.top() + 2.0,
            track.bottom() + 4.0,
        );
    }

    // The time under the pointer goes on top of everything else.
    if let (Some(hover), Some(time)) = (pointer_over, hover_time) {
        painter.line_segment(
            [pos2(hover.x, ruler.bottom()), pos2(hover.x, rect.bottom())],
            Stroke::new(1.0, palette.text_muted),
        );
        draw_pill(&painter, palette, ruler, hover.x, format_timecode(time));
    }

    let (strip, _) = ui.allocate_exact_size(vec2(width, STRIP_HEIGHT), Sense::hover());
    bottom_strip(ui, palette, state, strip, left, view);
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scale() -> TimeScale {
        TimeScale {
            visible_start: Ticks(0),
            visible: Ticks::from_seconds(100.0),
            left: 10.0,
            width: 1000.0,
        }
    }

    fn view(id: u64, start: f64, end: f64) -> ClipView {
        ClipView {
            id: ClipId(id),
            start: Ticks::from_seconds(start),
            end: Ticks::from_seconds(end),
            label: String::new(),
        }
    }

    fn seconds(value: f64) -> Ticks {
        Ticks::from_seconds(value)
    }

    #[test]
    fn time_and_position_convert_both_ways() {
        let scale = scale();

        assert_eq!(scale.x_of(Ticks::from_seconds(50.0)), 510.0);
        assert_eq!(scale.time_at(510.0), Ticks::from_seconds(50.0));
    }

    #[test]
    fn a_scrolled_view_starts_from_its_scroll_position() {
        let scrolled = TimeScale {
            visible_start: Ticks::from_seconds(20.0),
            visible: Ticks::from_seconds(10.0),
            left: 0.0,
            width: 100.0,
        };

        assert_eq!(scrolled.x_of(Ticks::from_seconds(25.0)), 50.0);
        assert_eq!(scrolled.time_at(100.0), Ticks::from_seconds(30.0));
    }

    #[test]
    fn an_empty_or_collapsed_view_does_not_divide_by_zero() {
        let empty = TimeScale {
            visible_start: Ticks(0),
            visible: Ticks(0),
            left: 10.0,
            width: 0.0,
        };

        assert_eq!(empty.x_of(Ticks(5)), 10.0);
        assert_eq!(empty.time_at(50.0), Ticks(0));
        assert_eq!(empty.label_step_seconds(), 0.1);
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
                visible_start: Ticks(0),
                visible: Ticks::from_seconds(seconds),
                left: 0.0,
                width,
            };
            let spacing = scale.label_step_seconds() / seconds * f64::from(width);

            assert!(
                spacing >= f64::from(MIN_LABEL_SPACING),
                "{seconds}s: {spacing}px"
            );
        }
    }

    #[test]
    fn each_end_of_a_clip_is_a_handle_and_the_middle_is_the_body() {
        let scale = scale();
        let clips = [view(1, 10.0, 30.0), view(2, 30.0, 60.0)];

        assert_eq!(
            part_at(&clips, &scale, 115.0),
            Some((0, ClipPart::Handle(ClipEdge::Start)))
        );
        assert_eq!(part_at(&clips, &scale, 205.0), Some((0, ClipPart::Body)));
        assert_eq!(
            part_at(&clips, &scale, 305.0),
            Some((0, ClipPart::Handle(ClipEdge::End)))
        );
        assert_eq!(part_at(&clips, &scale, 450.0), Some((1, ClipPart::Body)));
    }

    #[test]
    fn nothing_is_found_outside_the_clips() {
        let scale = scale();

        assert_eq!(part_at(&[view(1, 10.0, 30.0)], &scale, 700.0), None);
        assert_eq!(part_at(&[], &scale, 100.0), None);
    }

    #[test]
    fn a_tiny_clip_still_has_a_middle_to_grab() {
        let scale = scale();
        // Six pixels wide, narrower than two full handles.
        let clips = [view(1, 10.0, 10.6)];

        assert_eq!(part_at(&clips, &scale, 113.0), Some((0, ClipPart::Body)));
    }

    #[test]
    fn the_timeline_runs_on_past_the_video() {
        assert_eq!(TimelineState::extent(seconds(100.0)), seconds(200.0));
    }

    #[test]
    fn an_empty_or_short_project_still_has_a_timeline() {
        let least = seconds(MIN_EXTENT_SECONDS);

        assert_eq!(TimelineState::extent(Ticks(0)), least);
        assert_eq!(TimelineState::extent(seconds(1.0)), least);
    }

    #[test]
    fn fitting_shows_the_video_as_part_of_the_view() {
        let total = seconds(90.0);
        let mut state = TimelineState::default();
        state.zoom_by(8.0, seconds(70.0), total);

        state.fit(total);

        assert_eq!(state.scroll, Ticks(0));
        let visible = state.visible_for(total);
        let share = total.as_seconds() / visible.as_seconds();
        assert!((share - FIT_FRACTION).abs() < 0.001, "{share}");
    }

    #[test]
    fn zooming_keeps_the_anchor_in_the_same_place() {
        let total = seconds(100.0);
        let mut state = TimelineState::default();
        state.fit(total);
        let before = state.visible_for(total);
        let anchor = seconds(50.0);
        let fraction = anchor.0 as f64 / before.0 as f64;

        state.zoom_by(2.0, anchor, total);

        let after = state.visible_for(total);
        assert_eq!(after.0, before.0 / 2);
        let moved = (anchor.0 - state.scroll.0) as f64 / after.0 as f64;
        assert!((moved - fraction).abs() < 0.001);
    }

    #[test]
    fn zooming_out_goes_past_fitting_up_to_the_whole_timeline() {
        let total = seconds(100.0);
        let mut state = TimelineState::default();
        state.fit(total);
        let fitted = state.visible_for(total);

        state.zoom_out(Ticks(0), total);
        assert!(state.visible_for(total) > fitted);

        for _ in 0..30 {
            state.zoom_out(Ticks(0), total);
        }
        assert_eq!(state.visible_for(total), TimelineState::extent(total));
        assert_eq!(state.scroll, Ticks(0));
    }

    #[test]
    fn zooming_in_stops_at_its_limit() {
        let total = seconds(100.0);
        let mut state = TimelineState::default();

        for _ in 0..100 {
            state.zoom_in(Ticks(0), total);
        }

        assert_eq!(state.visible_for(total), seconds(MIN_VISIBLE_SECONDS));
    }

    #[test]
    fn scrolling_stops_at_the_ends_of_the_timeline() {
        let total = seconds(100.0);
        let mut state = TimelineState {
            visible: seconds(50.0),
            scroll: seconds(500.0),
            ..Default::default()
        };

        state.clamp_scroll(total);
        assert_eq!(state.scroll, seconds(150.0));

        state.scroll = seconds(-5.0);
        state.clamp_scroll(total);
        assert_eq!(state.scroll, Ticks(0));
    }

    #[test]
    fn a_new_clip_refits_until_the_user_chooses_a_zoom() {
        let mut state = TimelineState::default();

        state.content_added(seconds(30.0));
        assert_eq!(state.visible_for(seconds(30.0)), seconds(40.0));

        state.zoom_in(Ticks(0), seconds(30.0));
        let chosen = state.visible_for(seconds(90.0));
        state.content_added(seconds(90.0));
        assert_eq!(state.visible_for(seconds(90.0)), chosen);

        state.fit(seconds(90.0));
        state.content_added(seconds(120.0));
        assert_eq!(state.visible_for(seconds(120.0)), seconds(160.0));
    }

    #[test]
    fn the_view_holds_its_zoom_when_the_project_changes_length() {
        let mut state = TimelineState::default();
        state.fit(seconds(90.0));

        assert_eq!(state.visible_for(seconds(80.0)), seconds(120.0));
    }

    #[test]
    fn shrinking_the_project_never_shows_more_than_the_timeline() {
        let mut state = TimelineState::default();
        state.fit(seconds(900.0));

        assert_eq!(state.visible_for(seconds(1.0)), seconds(MIN_EXTENT_SECONDS));
    }

    #[test]
    fn following_only_scrolls_when_the_playhead_leaves_the_view() {
        let total = seconds(100.0);
        let mut state = TimelineState {
            visible: seconds(25.0),
            ..Default::default()
        };

        state.follow(seconds(10.0), total);
        assert_eq!(state.scroll, Ticks(0));

        state.follow(seconds(60.0), total);
        assert_eq!(state.scroll, seconds(60.0));
    }

    #[test]
    fn the_thumb_shows_how_much_of_the_timeline_is_in_view() {
        let (left, width) = thumb_span(400.0, Ticks(0), seconds(50.0), seconds(200.0));
        assert_eq!((left, width), (0.0, 100.0));

        let (left, width) = thumb_span(400.0, seconds(150.0), seconds(50.0), seconds(200.0));
        assert_eq!((left, width), (300.0, 100.0));
    }

    #[test]
    fn the_thumb_stays_big_enough_to_grab() {
        let (left, width) = thumb_span(400.0, seconds(199.0), seconds(1.0), seconds(200.0));

        assert_eq!(width, MIN_THUMB_WIDTH);
        assert_eq!(left, 400.0 - MIN_THUMB_WIDTH);
    }

    #[test]
    fn the_thumb_fills_the_bar_when_everything_is_in_view() {
        let (left, width) = thumb_span(400.0, Ticks(0), seconds(200.0), seconds(200.0));

        assert_eq!((left, width), (0.0, 400.0));
    }

    #[test]
    fn dragging_the_thumb_scrolls_across_the_whole_timeline() {
        let total = seconds(100.0);
        let mut state = TimelineState {
            visible: seconds(50.0),
            ..Default::default()
        };

        state.scroll_by_thumb(150.0, 400.0, total);
        assert_eq!(state.scroll, seconds(75.0));

        state.scroll_by_thumb(9000.0, 400.0, total);
        assert_eq!(state.scroll, seconds(150.0));
    }

    #[test]
    fn a_dragged_clip_snaps_to_things_nearby() {
        let tolerance = seconds(1.0);
        let targets = [seconds(10.0), seconds(40.0)];

        // Its start lines up with the first target.
        assert_eq!(
            snap_start(seconds(10.5), seconds(5.0), &targets, tolerance),
            seconds(10.0)
        );
        // Its end lines up with the second.
        assert_eq!(
            snap_start(seconds(35.5), seconds(5.0), &targets, tolerance),
            seconds(35.0)
        );
    }

    #[test]
    fn a_dragged_clip_stays_put_away_from_anything() {
        let targets = [seconds(10.0)];

        assert_eq!(
            snap_start(seconds(20.0), seconds(5.0), &targets, seconds(1.0)),
            seconds(20.0)
        );
    }

    #[test]
    fn snapping_prefers_the_closest_target() {
        let targets = [seconds(10.0), seconds(10.8)];

        assert_eq!(
            snap_start(seconds(10.6), seconds(5.0), &targets, seconds(1.0)),
            seconds(10.8)
        );
    }

    #[test]
    fn snapping_never_goes_before_the_start() {
        assert_eq!(
            snap_start(seconds(0.5), seconds(5.0), &[seconds(4.0)], seconds(1.0)),
            seconds(0.5)
        );
    }

    #[test]
    fn dragging_near_an_edge_scrolls_that_way_faster_the_closer_it_gets() {
        let (left, width) = (100.0, 800.0);

        assert_eq!(edge_scroll_speed(500.0, left, width), 0.0);
        assert!(edge_scroll_speed(110.0, left, width) < 0.0);
        assert!(edge_scroll_speed(890.0, left, width) > 0.0);
        assert!(edge_scroll_speed(895.0, left, width) > edge_scroll_speed(880.0, left, width));
        assert_eq!(edge_scroll_speed(2000.0, left, width), 1.0);
    }
}
