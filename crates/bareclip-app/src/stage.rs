//! What can be done to clips on the picture itself: selecting them, moving and resizing them, and
//! the small toolbar that floats above whichever is selected.
//!
//! Sizes and places here are shares of the canvas, from 0 to 1, as they are in a `Placement`.

use bareclip_core::{
    placement::{LARGEST_SCALE, Placement, SMALLEST_SCALE, WHOLE, fit_inside},
    timeline::ClipId,
};
use eframe::egui::{self, CursorIcon, Pos2, Rect, Sense, Slider, Stroke, pos2, vec2};

use crate::{
    components::{IconButton, Kind},
    icons,
    theme::Palette,
};

const HANDLE_SIZE: f32 = 12.0;
/// How far from a corner still grabs it.
const HANDLE_REACH: f32 = 10.0;
/// How close an edge has to come, in screen points, before it snaps.
const SNAP_DISTANCE: f32 = 8.0;
/// How much of a picture has to stay on the canvas, so it can always be grabbed again.
const SHARE_KEPT_ON_CANVAS: f64 = 0.02;

const TOOLBAR_HEIGHT: f32 = 36.0;
const TOOLBAR_WIDTH: f32 = 264.0;
const TOOLBAR_GAP: f32 = 8.0;
/// How far above the video area the toolbar can go, into the row holding undo and Export.
const ROW_ABOVE: f32 = 44.0;

/// A clip that could show at the playhead, for the stage to work with. Lowest first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StageClip {
    pub id: ClipId,
    pub placement: Placement,
    /// Its video's full size.
    pub size: (u32, u32),
    /// False if something completely hides it, so clicks go through to what is on top.
    pub visible: bool,
}

/// A place on the canvas, in shares of it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Area {
    left: f64,
    top: f64,
    width: f64,
    height: f64,
}

impl Area {
    fn right(&self) -> f64 {
        self.left + self.width
    }

    fn bottom(&self) -> f64 {
        self.top + self.height
    }

    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.left && x <= self.right() && y >= self.top && y <= self.bottom()
    }
}

fn share(value: i32) -> f64 {
    f64::from(value) / f64::from(WHOLE)
}

fn whole_number(value: f64) -> i32 {
    (value * f64::from(WHOLE)).round() as i32
}

/// A picture's full size at its normal scale, in shares of the canvas.
fn normal_size(size: (u32, u32), canvas: (u32, u32)) -> (f64, f64) {
    let fitted = fit_inside(size, canvas);
    (
        f64::from(fitted.width) / f64::from(canvas.0.max(1)),
        f64::from(fitted.height) / f64::from(canvas.1.max(1)),
    )
}

fn area_of(placement: Placement, size: (u32, u32), canvas: (u32, u32)) -> Area {
    let (normal_width, normal_height) = normal_size(size, canvas);
    let (width, height) = (
        normal_width * share(placement.scale),
        normal_height * share(placement.scale),
    );
    Area {
        left: share(placement.center_x) - width / 2.0,
        top: share(placement.center_y) - height / 2.0,
        width,
        height,
    }
}

/// A line something lined up with, as a share of the canvas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Guide {
    Across(f64),
    Down(f64),
}

/// The lines things snap to across and down: the canvas's edges and centre, and other clips' edges.
fn snap_targets(clips: &[StageClip], moving: ClipId, canvas: (u32, u32)) -> (Vec<f64>, Vec<f64>) {
    let mut across = vec![0.0, 0.5, 1.0];
    let mut down = vec![0.0, 0.5, 1.0];
    for clip in clips {
        if clip.id == moving || !clip.visible {
            continue;
        }
        let area = area_of(clip.placement, clip.size, canvas);
        across.extend([area.left, area.right()]);
        down.extend([area.top, area.bottom()]);
    }
    (across, down)
}

/// The smallest shift that lines one of `edges` up with one of `targets`, if any is within reach,
/// and the line it lands on.
fn nearest_snap(edges: &[f64], targets: &[f64], reach: f64) -> Option<(f64, f64)> {
    let mut best: Option<(f64, f64)> = None;
    for edge in edges {
        for target in targets {
            let shift = target - edge;
            if shift.abs() <= reach && best.is_none_or(|(found, _)| shift.abs() < found.abs()) {
                best = Some((shift, *target));
            }
        }
    }
    best
}

/// How far a pointer's snapping reaches, in shares of the canvas, across and down.
fn reach_on(frame: Rect) -> (f64, f64) {
    (
        f64::from(SNAP_DISTANCE / frame.width().max(1.0)),
        f64::from(SNAP_DISTANCE / frame.height().max(1.0)),
    )
}

/// Keeps a bit of the picture on the canvas, so it can't be lost off the edge.
fn keep_on_canvas(placement: Placement, size: (u32, u32), canvas: (u32, u32)) -> Placement {
    let area = area_of(placement, size, canvas);
    let keep = SHARE_KEPT_ON_CANVAS;
    let centre = |value: f64, length: f64| {
        let lowest = keep - length / 2.0;
        let highest = 1.0 - keep + length / 2.0;
        value.clamp(lowest, highest.max(lowest))
    };
    Placement {
        center_x: whole_number(centre(share(placement.center_x), area.width)),
        center_y: whole_number(centre(share(placement.center_y), area.height)),
        ..placement
    }
}

/// A clip dragged by this much from where it was, snapped to anything close, and the lines it
/// lined up with.
fn moved(
    start: Placement,
    size: (u32, u32),
    canvas: (u32, u32),
    by: (f64, f64),
    targets: &(Vec<f64>, Vec<f64>),
    reach: (f64, f64),
) -> (Placement, Vec<Guide>) {
    let area = area_of(start, size, canvas);
    let (left, top) = (area.left + by.0, area.top + by.1);
    let mut guides = Vec::new();

    let across = [left, left + area.width / 2.0, left + area.width];
    let shift_x = nearest_snap(&across, &targets.0, reach.0).map_or(0.0, |(shift, line)| {
        guides.push(Guide::Across(line));
        shift
    });
    let down = [top, top + area.height / 2.0, top + area.height];
    let shift_y = nearest_snap(&down, &targets.1, reach.1).map_or(0.0, |(shift, line)| {
        guides.push(Guide::Down(line));
        shift
    });

    let placement = keep_on_canvas(
        Placement {
            center_x: whole_number(left + shift_x + area.width / 2.0),
            center_y: whole_number(top + shift_y + area.height / 2.0),
            ..start
        },
        size,
        canvas,
    );
    (placement, guides)
}

/// Which corner of a picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

const CORNERS: [Corner; 4] = [
    Corner::TopLeft,
    Corner::TopRight,
    Corner::BottomLeft,
    Corner::BottomRight,
];

impl Corner {
    /// Which way this corner sits from the middle of the picture, across and down.
    fn direction(self) -> (f64, f64) {
        match self {
            Self::TopLeft => (-1.0, -1.0),
            Self::TopRight => (1.0, -1.0),
            Self::BottomLeft => (-1.0, 1.0),
            Self::BottomRight => (1.0, 1.0),
        }
    }

    fn of(self, rect: Rect) -> Pos2 {
        match self {
            Self::TopLeft => rect.left_top(),
            Self::TopRight => rect.right_top(),
            Self::BottomLeft => rect.left_bottom(),
            Self::BottomRight => rect.right_bottom(),
        }
    }

    fn cursor(self) -> CursorIcon {
        match self {
            Self::TopLeft | Self::BottomRight => CursorIcon::ResizeNwSe,
            Self::TopRight | Self::BottomLeft => CursorIcon::ResizeNeSw,
        }
    }
}

/// A clip resized by dragging a corner to a place on the canvas. The opposite corner stays where
/// it was and the shape is kept, so the pointer sets one number, the scale.
fn resized(
    start: Placement,
    size: (u32, u32),
    canvas: (u32, u32),
    corner: Corner,
    pointer: (f64, f64),
    targets: &(Vec<f64>, Vec<f64>),
    reach: (f64, f64),
) -> (Placement, Vec<Guide>) {
    let area = area_of(start, size, canvas);
    let (normal_width, normal_height) = normal_size(size, canvas);
    let (across, down) = corner.direction();
    let anchor = (
        share(start.center_x) - across * area.width / 2.0,
        share(start.center_y) - down * area.height / 2.0,
    );

    // Whichever way the pointer has gone further sets the size.
    let scale_for_x = |x: f64| across * (x - anchor.0) / normal_width;
    let scale_for_y = |y: f64| down * (y - anchor.1) / normal_height;
    let mut scale = scale_for_x(pointer.0).max(scale_for_y(pointer.1));

    let mut guides = Vec::new();
    let corner_x = anchor.0 + across * scale * normal_width;
    let corner_y = anchor.1 + down * scale * normal_height;
    let snap_x = nearest_snap(&[corner_x], &targets.0, reach.0);
    let snap_y = nearest_snap(&[corner_y], &targets.1, reach.1);
    match (snap_x, snap_y) {
        (Some((shift_x, x)), Some((shift_y, _))) if shift_x.abs() <= shift_y.abs() => {
            scale = scale_for_x(x);
            guides.push(Guide::Across(x));
        }
        (_, Some((_, y))) => {
            scale = scale_for_y(y);
            guides.push(Guide::Down(y));
        }
        (Some((_, x)), None) => {
            scale = scale_for_x(x);
            guides.push(Guide::Across(x));
        }
        (None, None) => {}
    }

    let scale = whole_number(scale).clamp(SMALLEST_SCALE, LARGEST_SCALE);
    if scale == SMALLEST_SCALE || scale == LARGEST_SCALE {
        guides.clear();
    }
    let (width, height) = (normal_width * share(scale), normal_height * share(scale));
    let placement = Placement {
        center_x: whole_number(anchor.0 + across * width / 2.0),
        center_y: whole_number(anchor.1 + down * height / 2.0),
        scale,
        ..start
    };
    (placement, guides)
}

/// The topmost visible clip at a place on the canvas.
fn clip_at(clips: &[StageClip], canvas: (u32, u32), x: f64, y: f64) -> Option<ClipId> {
    clips
        .iter()
        .rev()
        .filter(|clip| clip.visible)
        .find(|clip| area_of(clip.placement, clip.size, canvas).contains(x, y))
        .map(|clip| clip.id)
}

fn screen_rect(area: Area, frame: Rect) -> Rect {
    Rect::from_min_size(
        pos2(
            frame.left() + area.left as f32 * frame.width(),
            frame.top() + area.top as f32 * frame.height(),
        ),
        vec2(
            area.width as f32 * frame.width(),
            area.height as f32 * frame.height(),
        ),
    )
}

fn canvas_share(pointer: Pos2, frame: Rect) -> (f64, f64) {
    (
        f64::from((pointer.x - frame.left()) / frame.width().max(1.0)),
        f64::from((pointer.y - frame.top()) / frame.height().max(1.0)),
    )
}

/// Where the toolbar goes: just above the clip and centred on it, staying on the video area. When
/// the row above the picture is free it can go up there, so it never has to cover a full-size clip.
fn toolbar_rect(clip: Rect, area: Rect, row_above_is_free: bool) -> Rect {
    let size = vec2(TOOLBAR_WIDTH, TOOLBAR_HEIGHT);
    let highest = if row_above_is_free {
        area.top() - ROW_ABOVE
    } else {
        area.top()
    };
    let wanted = pos2(
        clip.center().x - size.x / 2.0,
        clip.top() - TOOLBAR_GAP - size.y,
    );
    let at = pos2(
        wanted
            .x
            .clamp(area.left(), (area.right() - size.x).max(area.left())),
        wanted
            .y
            .clamp(highest, (area.bottom() - size.y).max(highest)),
    );
    Rect::from_min_size(at, size)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum DragKind {
    Move,
    Resize(Corner),
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct StageDrag {
    clip: ClipId,
    kind: DragKind,
    start: Placement,
    /// Where the pointer was when the drag began, in shares of the canvas.
    from: (f64, f64),
}

#[derive(Debug, Default)]
pub struct StageState {
    drag: Option<StageDrag>,
    guides: Vec<Guide>,
    /// Where the canvas was on screen in the last frame.
    pub frame: Option<Rect>,
}

impl StageState {
    /// Whether a line is showing what something lined up with.
    #[cfg(test)]
    pub fn has_guides(&self) -> bool {
        !self.guides.is_empty()
    }
}

/// What the stage needs to know to draw this frame.
pub struct StageView<'a> {
    /// Where the canvas is on screen.
    pub frame: Rect,
    /// The whole video area, which the toolbar has to stay inside.
    pub area: Rect,
    pub canvas: (u32, u32),
    pub clips: &'a [StageClip],
    pub selected: Option<ClipId>,
    /// Nothing is being said in the row above the picture, so the toolbar can use it.
    pub row_above_is_free: bool,
}

/// What happened on the stage this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StageEvents {
    /// `Some(None)` clears the selection.
    pub select: Option<Option<ClipId>>,
    pub place: Option<(ClipId, Placement)>,
}

/// What a press at this place on the canvas would grab, trying the selected clip's handles first.
fn grab_at(view: &StageView, pointer: Pos2) -> Option<(ClipId, DragKind)> {
    if let Some(selected) = view
        .clips
        .iter()
        .find(|clip| Some(clip.id) == view.selected)
    {
        let rect = screen_rect(
            area_of(selected.placement, selected.size, view.canvas),
            view.frame,
        );
        let corner = CORNERS
            .into_iter()
            .find(|corner| corner.of(rect).distance(pointer) <= HANDLE_REACH);
        if let Some(corner) = corner {
            return Some((selected.id, DragKind::Resize(corner)));
        }
        if rect.contains(pointer) {
            return Some((selected.id, DragKind::Move));
        }
    }
    let (x, y) = canvas_share(pointer, view.frame);
    clip_at(view.clips, view.canvas, x, y).map(|id| (id, DragKind::Move))
}

/// Draws the selected clip's outline and handles, and does what the pointer asks.
pub fn show(
    ui: &mut egui::Ui,
    palette: &Palette,
    state: &mut StageState,
    view: &StageView,
) -> StageEvents {
    let mut events = StageEvents::default();
    state.frame = Some(view.frame);
    let response = ui.interact(
        view.area,
        ui.scope_id().with("stage"),
        Sense::click_and_drag(),
    );

    if let Some(pointer) = response.hover_pos() {
        let cursor = match grab_at(view, pointer) {
            Some((_, DragKind::Resize(corner))) => Some(corner.cursor()),
            Some((_, DragKind::Move)) => Some(CursorIcon::Move),
            None => None,
        };
        if let Some(cursor) = cursor {
            ui.ctx().set_cursor_icon(cursor);
        }
    }

    if response.clicked()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let (x, y) = canvas_share(pointer, view.frame);
        events.select = Some(clip_at(view.clips, view.canvas, x, y));
    }

    if response.drag_started() {
        let origin = ui.input(|input| input.pointer.press_origin());
        if let Some(pointer) = origin
            && let Some((clip, kind)) = grab_at(view, pointer)
            && let Some(found) = view.clips.iter().find(|found| found.id == clip)
        {
            state.drag = Some(StageDrag {
                clip,
                kind,
                start: found.placement,
                from: canvas_share(pointer, view.frame),
            });
            events.select = Some(Some(clip));
        }
    }

    state.guides.clear();
    if let (Some(drag), Some(pointer)) = (state.drag, response.interact_pointer_pos())
        && let Some(clip) = view.clips.iter().find(|clip| clip.id == drag.clip)
    {
        let at = canvas_share(pointer, view.frame);
        let targets = snap_targets(view.clips, drag.clip, view.canvas);
        let reach = reach_on(view.frame);
        let (placement, guides) = match drag.kind {
            DragKind::Move => moved(
                drag.start,
                clip.size,
                view.canvas,
                (at.0 - drag.from.0, at.1 - drag.from.1),
                &targets,
                reach,
            ),
            DragKind::Resize(corner) => resized(
                drag.start,
                clip.size,
                view.canvas,
                corner,
                at,
                &targets,
                reach,
            ),
        };
        state.guides = guides;
        events.place = Some((drag.clip, placement));
    }
    if response.drag_stopped() {
        state.drag = None;
        state.guides.clear();
    }

    let painter = ui.painter_at(view.area);
    let guide = Stroke::new(1.0, palette.playhead);
    for line in &state.guides {
        match *line {
            Guide::Across(x) => {
                let x = view.frame.left() + x as f32 * view.frame.width();
                painter.vline(x, view.frame.y_range(), guide);
            }
            Guide::Down(y) => {
                let y = view.frame.top() + y as f32 * view.frame.height();
                painter.hline(view.frame.x_range(), y, guide);
            }
        }
    }

    if let Some(selected) = view
        .clips
        .iter()
        .find(|clip| Some(clip.id) == view.selected)
    {
        // The newest place, so the outline doesn't trail the picture by a frame.
        let placement = events
            .place
            .filter(|(id, _)| *id == selected.id)
            .map_or(selected.placement, |(_, placement)| placement);
        let rect = screen_rect(area_of(placement, selected.size, view.canvas), view.frame);
        painter.rect_stroke(
            rect,
            0.0,
            Stroke::new(2.0, palette.playhead),
            egui::StrokeKind::Middle,
        );
        for corner in CORNERS {
            let center = corner.of(rect);
            painter.circle(
                center,
                HANDLE_SIZE / 2.0,
                palette.playhead,
                Stroke::new(1.5, palette.playhead_outline),
            );
        }
        toolbar(ui, palette, view, selected, placement, rect, &mut events);
    }
    events
}

/// The small bar above the selected clip. It only offers what works today. It sits in its own
/// layer, because when a clip fills the picture the only place that covers nothing is the row
/// above it, which the stage's own drawing can't reach.
fn toolbar(
    ui: &mut egui::Ui,
    palette: &Palette,
    view: &StageView,
    clip: &StageClip,
    placement: Placement,
    clip_rect: Rect,
    events: &mut StageEvents,
) {
    let rect = toolbar_rect(clip_rect, view.area, view.row_above_is_free);
    let id = ui.scope_id().with("stage-toolbar");
    egui::Area::new(id)
        .order(egui::Order::Middle)
        .fixed_pos(rect.min)
        .fade_in(false)
        .constrain(false)
        .show(ui.ctx(), |ui| {
            ui.set_min_size(rect.size());
            let painter = ui.painter().clone();
            painter.rect(
                rect,
                8.0,
                palette.raised,
                Stroke::new(1.0, palette.border),
                egui::StrokeKind::Inside,
            );

            let middle = rect.center().y;
            painter.text(
                pos2(rect.left() + 12.0, middle),
                egui::Align2::LEFT_CENTER,
                "Opacity",
                egui::FontId::proportional(13.0),
                palette.text,
            );
            let percent = (share(placement.opacity) * 100.0).round() as u32;
            painter.text(
                pos2(rect.right() - 40.0, middle),
                egui::Align2::RIGHT_CENTER,
                format!("{percent}%"),
                egui::FontId::proportional(13.0),
                palette.text_muted,
            );

            // The theme has no rail colour of its own that shows up on a raised surface.
            let visuals = &mut ui.style_mut().visuals;
            visuals.widgets.inactive.bg_fill = palette.border_strong;
            visuals.widgets.hovered.bg_fill = palette.border_strong;
            visuals.widgets.active.bg_fill = palette.border_strong;
            visuals.selection.bg_fill = palette.accent;
            let slider_rect = Rect::from_min_max(
                pos2(rect.left() + 66.0, rect.top()),
                pos2(rect.right() - 80.0, rect.bottom()),
            );
            ui.spacing_mut().slider_width = slider_rect.width();
            let mut opacity = f64::from(percent);
            let slider = ui
                .put(
                    slider_rect,
                    Slider::new(&mut opacity, 0.0..=100.0)
                        .show_value(false)
                        .step_by(1.0)
                        .trailing_fill(true),
                )
                .on_hover_text("How see-through this clip is");
            slider.widget_info(|| egui::WidgetInfo::slider(slider.enabled(), opacity, "Opacity"));
            if slider.changed() {
                events.place = Some((
                    clip.id,
                    Placement {
                        opacity: (opacity * f64::from(WHOLE) / 100.0).round() as i32,
                        ..placement
                    },
                ));
            }
            // Holding on to the keyboard would stop the arrow keys moving the playhead.
            if slider.drag_stopped() || slider.clicked() {
                slider.surrender_focus();
            }

            let reset_rect =
                Rect::from_center_size(pos2(rect.right() - 20.0, middle), vec2(28.0, 28.0));
            let reset = IconButton::new(
                palette,
                Kind::Ghost,
                icons::ROTATE_CCW,
                "Reset size and place",
            )
            .show_at(ui, id.with("reset"), reset_rect)
            .on_hover_text("Reset size and place");
            if reset.clicked() {
                events.place = Some((clip.id, Placement::default()));
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANVAS: (u32, u32) = (1920, 1080);
    /// No snapping unless a test asks for it.
    const NO_REACH: (f64, f64) = (0.0, 0.0);
    const NO_LINES: fn() -> (Vec<f64>, Vec<f64>) = || (Vec::new(), Vec::new());

    fn half_size() -> Placement {
        Placement {
            scale: WHOLE / 2,
            ..Placement::default()
        }
    }

    fn stage_clip(id: u64, placement: Placement) -> StageClip {
        StageClip {
            id: ClipId(id),
            placement,
            size: CANVAS,
            visible: true,
        }
    }

    #[test]
    fn a_normal_clip_covers_the_whole_canvas() {
        let area = area_of(Placement::default(), CANVAS, CANVAS);

        assert_eq!(
            area,
            Area {
                left: 0.0,
                top: 0.0,
                width: 1.0,
                height: 1.0,
            }
        );
    }

    #[test]
    fn a_tall_clip_only_covers_its_middle_of_a_wide_canvas() {
        let area = area_of(Placement::default(), (1080, 1920), CANVAS);

        assert!((area.width - 0.316).abs() < 0.001, "{area:?}");
        assert_eq!(area.height, 1.0);
        assert!((area.left + area.width / 2.0 - 0.5).abs() < 0.001);
    }

    #[test]
    fn moving_goes_as_far_as_the_pointer_did() {
        let (placement, guides) = moved(
            half_size(),
            CANVAS,
            CANVAS,
            (0.1, -0.2),
            &NO_LINES(),
            NO_REACH,
        );

        assert_eq!(placement.center_x, 600_000);
        assert_eq!(placement.center_y, 300_000);
        assert_eq!(placement.scale, WHOLE / 2);
        assert!(guides.is_empty());
    }

    #[test]
    fn moving_close_to_the_middle_snaps_there_exactly_and_shows_a_line() {
        let targets = snap_targets(&[], ClipId(1), CANVAS);

        let (placement, guides) = moved(
            half_size(),
            CANVAS,
            CANVAS,
            (0.004, 0.2),
            &targets,
            (0.005, 0.005),
        );

        assert_eq!(placement.center_x, WHOLE / 2);
        assert_eq!(guides, [Guide::Across(0.5)]);
    }

    #[test]
    fn an_edge_snaps_to_the_canvas_edge() {
        let targets = snap_targets(&[], ClipId(1), CANVAS);

        // Half size in the middle has its left edge a quarter of the way in.
        let (placement, _) = moved(
            half_size(),
            CANVAS,
            CANVAS,
            (-0.246, 0.0),
            &targets,
            (0.005, 0.005),
        );

        assert_eq!(placement.center_x, WHOLE / 4);
    }

    #[test]
    fn an_edge_snaps_to_another_clips_edge() {
        let other = stage_clip(
            2,
            Placement {
                center_x: 750_000,
                center_y: 750_000,
                scale: WHOLE / 4,
                ..Placement::default()
            },
        );
        let targets = snap_targets(&[other], ClipId(1), CANVAS);
        assert!(targets.0.contains(&0.625));

        // Half size at 0.5 has its right edge at 0.75. It goes to the other clip's left edge.
        let (placement, guides) = moved(
            half_size(),
            CANVAS,
            CANVAS,
            (-0.122, 0.0),
            &targets,
            (0.005, 0.005),
        );

        assert_eq!(placement.center_x, 375_000);
        assert!(guides.contains(&Guide::Across(0.625)));
    }

    #[test]
    fn a_hidden_clip_gives_nothing_to_snap_to() {
        let hidden = StageClip {
            visible: false,
            ..stage_clip(
                2,
                Placement {
                    center_x: 300_000,
                    scale: WHOLE / 4,
                    ..Placement::default()
                },
            )
        };

        let targets = snap_targets(&[hidden], ClipId(1), CANVAS);

        assert_eq!(targets.0, [0.0, 0.5, 1.0]);
    }

    #[test]
    fn a_clip_cant_be_lost_off_the_canvas() {
        let (far, _) = moved(
            half_size(),
            CANVAS,
            CANVAS,
            (50.0, -50.0),
            &NO_LINES(),
            NO_REACH,
        );

        let area = area_of(far, CANVAS, CANVAS);
        assert!(area.left < 1.0 && area.bottom() > 0.0, "{area:?}");
        assert!(
            (1.0 - area.left - SHARE_KEPT_ON_CANVAS).abs() < 0.001,
            "{area:?}"
        );
        assert!(
            (area.bottom() - SHARE_KEPT_ON_CANVAS).abs() < 0.001,
            "{area:?}"
        );
    }

    #[test]
    fn a_clip_can_go_partly_off_the_canvas() {
        let (placement, _) = moved(
            half_size(),
            CANVAS,
            CANVAS,
            (0.5, 0.0),
            &NO_LINES(),
            NO_REACH,
        );

        assert_eq!(placement.center_x, WHOLE);
    }

    #[test]
    fn resizing_leaves_the_opposite_corner_where_it_was() {
        let start = half_size();
        let before = area_of(start, CANVAS, CANVAS);

        let (placement, _) = resized(
            start,
            CANVAS,
            CANVAS,
            Corner::BottomRight,
            (0.5, 0.5),
            &NO_LINES(),
            NO_REACH,
        );

        let after = area_of(placement, CANVAS, CANVAS);
        assert_eq!(placement.scale, WHOLE / 4);
        assert!((after.left - before.left).abs() < 1e-5);
        assert!((after.top - before.top).abs() < 1e-5);
    }

    #[test]
    fn every_corner_keeps_its_opposite_corner_still() {
        for (corner, pointer) in [
            (Corner::TopLeft, (0.1, 0.1)),
            (Corner::TopRight, (0.9, 0.1)),
            (Corner::BottomLeft, (0.1, 0.9)),
            (Corner::BottomRight, (0.9, 0.9)),
        ] {
            let start = half_size();
            let before = area_of(start, CANVAS, CANVAS);

            let (placement, _) = resized(
                start,
                CANVAS,
                CANVAS,
                corner,
                pointer,
                &NO_LINES(),
                NO_REACH,
            );

            let after = area_of(placement, CANVAS, CANVAS);
            let opposite = |area: Area| {
                let (across, down) = corner.direction();
                (
                    if across > 0.0 {
                        area.left
                    } else {
                        area.right()
                    },
                    if down > 0.0 { area.top } else { area.bottom() },
                )
            };
            assert!(placement.scale > WHOLE / 2, "{corner:?}");
            assert!(
                (opposite(after).0 - opposite(before).0).abs() < 1e-5,
                "{corner:?}"
            );
            assert!(
                (opposite(after).1 - opposite(before).1).abs() < 1e-5,
                "{corner:?}"
            );
        }
    }

    #[test]
    fn resizing_keeps_the_pictures_shape() {
        let (placement, _) = resized(
            Placement::default(),
            CANVAS,
            CANVAS,
            Corner::BottomRight,
            (0.3, 0.9),
            &NO_LINES(),
            NO_REACH,
        );

        let area = area_of(placement, CANVAS, CANVAS);
        assert!((area.width - area.height).abs() < 1e-5, "{area:?}");
    }

    #[test]
    fn resizing_stops_at_the_smallest_and_largest_sizes() {
        let shrink = |pointer| {
            resized(
                half_size(),
                CANVAS,
                CANVAS,
                Corner::BottomRight,
                pointer,
                &NO_LINES(),
                NO_REACH,
            )
            .0
            .scale
        };

        assert_eq!(shrink((-50.0, -50.0)), SMALLEST_SCALE);
        assert_eq!(shrink((50.0, 50.0)), LARGEST_SCALE);
    }

    #[test]
    fn a_dragged_corner_snaps_to_a_line_keeping_the_opposite_corner_still() {
        let targets = snap_targets(&[], ClipId(1), CANVAS);
        // Half size in the middle, with its bottom right corner dragged almost to the canvas's.
        let (placement, guides) = resized(
            half_size(),
            CANVAS,
            CANVAS,
            Corner::BottomRight,
            (0.997, 0.5),
            &targets,
            (0.005, 0.005),
        );

        let area = area_of(placement, CANVAS, CANVAS);
        assert!((area.right() - 1.0).abs() < 1e-5, "{area:?}");
        assert!((area.left - 0.25).abs() < 1e-5, "{area:?}");
        assert_eq!(guides, [Guide::Across(1.0)]);
    }

    #[test]
    fn the_snap_nearest_of_all_wins() {
        let (shift, line) = nearest_snap(&[0.0, 0.5], &[0.01, 0.502], 0.05).expect("a snap");
        assert!((shift - 0.002).abs() < 1e-9);
        assert_eq!(line, 0.502);
        assert_eq!(nearest_snap(&[0.0], &[0.5], 0.05), None);
    }

    #[test]
    fn a_click_lands_on_the_topmost_clip_there() {
        let clips = [
            stage_clip(1, Placement::default()),
            stage_clip(2, half_size()),
        ];

        assert_eq!(clip_at(&clips, CANVAS, 0.5, 0.5), Some(ClipId(2)));
        assert_eq!(clip_at(&clips, CANVAS, 0.1, 0.1), Some(ClipId(1)));
    }

    #[test]
    fn a_click_where_nothing_is_drawn_lands_on_nothing() {
        let clips = [stage_clip(2, half_size())];

        assert_eq!(clip_at(&clips, CANVAS, 0.1, 0.1), None);
        assert_eq!(clip_at(&clips, CANVAS, 2.0, 0.5), None);
    }

    #[test]
    fn a_click_goes_through_a_hidden_clip() {
        let mut hidden = stage_clip(2, Placement::default());
        hidden.visible = false;
        let clips = [hidden];

        assert_eq!(clip_at(&clips, CANVAS, 0.5, 0.5), None);
    }

    #[test]
    fn the_toolbar_sits_above_the_clip_when_there_is_room() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        let clip = Rect::from_min_size(pos2(200.0, 200.0), vec2(400.0, 200.0));

        let toolbar = toolbar_rect(clip, area, true);

        assert_eq!(toolbar.bottom(), 200.0 - TOOLBAR_GAP);
        assert_eq!(toolbar.center().x, clip.center().x);
    }

    #[test]
    fn the_toolbar_goes_up_into_the_row_above_rather_than_covering_a_full_size_clip() {
        let area = Rect::from_min_size(pos2(100.0, 100.0), vec2(800.0, 600.0));

        let toolbar = toolbar_rect(area, area, true);

        assert_eq!(toolbar.top(), 100.0 - ROW_ABOVE);
        assert!(toolbar.bottom() <= area.top());
    }

    #[test]
    fn the_toolbar_stays_below_the_row_above_while_something_is_being_said_there() {
        let area = Rect::from_min_size(pos2(100.0, 100.0), vec2(800.0, 600.0));

        assert_eq!(toolbar_rect(area, area, false).top(), 100.0);
    }

    #[test]
    fn the_toolbar_stays_in_the_video_area_when_a_clip_runs_off_it() {
        let area = Rect::from_min_size(pos2(100.0, 100.0), vec2(800.0, 600.0));
        for clip in [
            Rect::from_min_size(pos2(120.0, -300.0), vec2(400.0, 300.0)),
            Rect::from_min_size(pos2(5000.0, 5000.0), vec2(40.0, 30.0)),
            Rect::from_min_size(pos2(-5000.0, -5000.0), vec2(40.0, 30.0)),
        ] {
            let toolbar = toolbar_rect(clip, area, false);

            assert!(area.contains_rect(toolbar), "{clip:?} {toolbar:?}");
        }
    }
}
