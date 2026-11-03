//! What can be done to clips on the picture itself: selecting them, moving, resizing and rotating
//! them, and the small toolbar that floats above whichever is selected.
//!
//! Sizes and places here are shares of the canvas, from 0 to 1, as they are in a `Placement`.
//! Turning only makes sense in pixels, because a canvas's width and height aren't equal.

use bareclip_core::{
    placement::{CanvasCorner, FULL_TURN, LARGEST_SCALE, Placement, SMALLEST_SCALE, WHOLE},
    timeline::ClipId,
    tracks::OrderMove,
};
use eframe::egui::{self, CursorIcon, Pos2, Rect, Sense, Shape, Slider, Stroke, pos2, vec2};

use crate::{
    components::{IconButton, Kind, MenuItem},
    icons,
    theme::Palette,
};

const HANDLE_SIZE: f32 = 12.0;
/// How far from a corner still grabs it.
const HANDLE_REACH: f32 = 10.0;
/// How far the rotate handle sits from the picture's bottom edge.
const ROTATE_HANDLE_GAP: f32 = 26.0;
const ROTATE_HANDLE_SIZE: f32 = 24.0;
/// How close an edge has to come, in screen points, before it snaps.
const SNAP_DISTANCE: f32 = 8.0;
/// How close a rotation has to come to an eighth of a turn before it snaps, in thousandths of a
/// degree.
const ROTATION_SNAP: i32 = 3_000;
const EIGHTH_TURN: i32 = FULL_TURN / 8;
/// How much of a picture has to stay on the canvas, so it can always be grabbed again.
const SHARE_KEPT_ON_CANVAS: f64 = 0.02;

const TOOLBAR_HEIGHT: f32 = 36.0;
const TOOLBAR_BUTTON: f32 = 30.0;
const TOOLBAR_GAP: f32 = 8.0;
const TOOLBAR_PADDING: f32 = 4.0;
const MORE_MENU_SLIDER_WIDTH: f32 = 120.0;
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

/// A place on the canvas, in shares of it. For a turned picture, the box around it.
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
}

fn share(value: i32) -> f64 {
    f64::from(value) / f64::from(WHOLE)
}

fn whole_number(value: f64) -> i32 {
    (value * f64::from(WHOLE)).round() as i32
}

/// A picture as it really sits on the canvas, in pixels: where its middle is, how big it is before
/// turning, and how it is turned.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Shown {
    center: (f64, f64),
    half: (f64, f64),
    sin: f64,
    cos: f64,
    canvas: (f64, f64),
}

impl Shown {
    fn of(placement: Placement, size: (u32, u32), canvas: (u32, u32)) -> Self {
        let rect = placement.rect(size, canvas);
        let radians = placement.degrees().to_radians();
        Self {
            center: (
                f64::from(rect.x) + f64::from(rect.width) / 2.0,
                f64::from(rect.y) + f64::from(rect.height) / 2.0,
            ),
            half: (f64::from(rect.width) / 2.0, f64::from(rect.height) / 2.0),
            sin: radians.sin(),
            cos: radians.cos(),
            canvas: (f64::from(canvas.0.max(1)), f64::from(canvas.1.max(1))),
        }
    }

    /// From the picture's own directions to the canvas's.
    fn turned(&self, x: f64, y: f64) -> (f64, f64) {
        (x * self.cos - y * self.sin, x * self.sin + y * self.cos)
    }

    /// From the canvas's directions to the picture's own.
    fn unturned(&self, x: f64, y: f64) -> (f64, f64) {
        (x * self.cos + y * self.sin, -x * self.sin + y * self.cos)
    }

    /// A place given in the picture's own terms, from -1 to 1 each way, in shares of the canvas.
    fn at(&self, x: f64, y: f64) -> (f64, f64) {
        let (across, down) = self.turned(x * self.half.0, y * self.half.1);
        (
            (self.center.0 + across) / self.canvas.0,
            (self.center.1 + down) / self.canvas.1,
        )
    }

    /// Whether a place given in shares of the canvas is on the picture.
    fn contains(&self, x: f64, y: f64) -> bool {
        let (across, down) = self.unturned(
            x * self.canvas.0 - self.center.0,
            y * self.canvas.1 - self.center.1,
        );
        across.abs() <= self.half.0 && down.abs() <= self.half.1
    }

    fn corner(&self, corner: Corner) -> (f64, f64) {
        let (x, y) = corner.direction();
        self.at(x, y)
    }
}

fn area_of(placement: Placement, size: (u32, u32), canvas: (u32, u32)) -> Area {
    let shown = Shown::of(placement, size, canvas);
    let corners = CORNERS.map(|corner| shown.corner(corner));
    let lowest = |pick: fn(&(f64, f64)) -> f64| corners.iter().map(pick).fold(f64::MAX, f64::min);
    let highest = |pick: fn(&(f64, f64)) -> f64| corners.iter().map(pick).fold(f64::MIN, f64::max);
    let (left, top) = (lowest(|c| c.0), lowest(|c| c.1));
    Area {
        left,
        top,
        width: highest(|c| c.0) - left,
        height: highest(|c| c.1) - top,
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
    Corner::BottomRight,
    Corner::BottomLeft,
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

    /// The arrows to show for a corner, going by which way it really points once turned.
    fn cursor(self, shown: &Shown) -> CursorIcon {
        let (x, y) = self.direction();
        let (across, down) = shown.turned(x, y);
        if across * down > 0.0 {
            CursorIcon::ResizeNwSe
        } else {
            CursorIcon::ResizeNeSw
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
    let shown = Shown::of(start, size, canvas);
    let scale_now = share(start.scale).max(f64::EPSILON);
    // How big the picture is at its normal scale, in pixels.
    let normal = (
        shown.half.0 * 2.0 / scale_now,
        shown.half.1 * 2.0 / scale_now,
    );
    let (across, down) = corner.direction();
    let (from_middle_x, from_middle_y) = shown.turned(across * shown.half.0, down * shown.half.1);
    let anchor = (
        shown.center.0 - from_middle_x,
        shown.center.1 - from_middle_y,
    );

    // Whichever way the pointer has gone further sets the size.
    let (reach_x, reach_y) = shown.unturned(
        pointer.0 * shown.canvas.0 - anchor.0,
        pointer.1 * shown.canvas.1 - anchor.1,
    );
    let mut scale = (across * reach_x / normal.0).max(down * reach_y / normal.1);

    // Lining up only means something for a picture that isn't turned.
    let mut guides = Vec::new();
    if start.rotation == 0 {
        let scale_for_x = |x: f64| across * (x * shown.canvas.0 - anchor.0) / normal.0;
        let scale_for_y = |y: f64| down * (y * shown.canvas.1 - anchor.1) / normal.1;
        let corner_x = (anchor.0 + across * scale * normal.0) / shown.canvas.0;
        let corner_y = (anchor.1 + down * scale * normal.1) / shown.canvas.1;
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
    }

    let scale = whole_number(scale).clamp(SMALLEST_SCALE, LARGEST_SCALE);
    if scale == SMALLEST_SCALE || scale == LARGEST_SCALE {
        guides.clear();
    }
    let half = (normal.0 * share(scale) / 2.0, normal.1 * share(scale) / 2.0);
    let (to_middle_x, to_middle_y) = shown.turned(across * half.0, down * half.1);
    let placement = Placement {
        center_x: whole_number((anchor.0 + to_middle_x) / shown.canvas.0),
        center_y: whole_number((anchor.1 + to_middle_y) / shown.canvas.1),
        scale,
        ..start
    };
    (placement, guides)
}

/// Which way a place on the canvas lies from the picture's middle, in thousandths of a degree,
/// going clockwise from straight up.
fn bearing(shown: &Shown, x: f64, y: f64) -> f64 {
    let across = x * shown.canvas.0 - shown.center.0;
    let down = y * shown.canvas.1 - shown.center.1;
    across.atan2(-down).to_degrees() * 1000.0
}

/// A clip turned by the rotate handle: by however far the pointer has gone around its middle since
/// the drag began, snapping to each eighth of a turn. Says if it snapped.
fn rotated(
    start: Placement,
    size: (u32, u32),
    canvas: (u32, u32),
    from: (f64, f64),
    pointer: (f64, f64),
) -> (Placement, bool) {
    let shown = Shown::of(start, size, canvas);
    let by = bearing(&shown, pointer.0, pointer.1) - bearing(&shown, from.0, from.1);
    let wanted = (f64::from(start.rotation) + by).round() as i32;
    let wanted = wanted.rem_euclid(FULL_TURN);
    let nearest = ((wanted + EIGHTH_TURN / 2) / EIGHTH_TURN) * EIGHTH_TURN;
    let snapped = (wanted - nearest).abs() <= ROTATION_SNAP;
    let rotation = if snapped { nearest } else { wanted };
    (
        Placement {
            rotation: rotation.rem_euclid(FULL_TURN),
            ..start
        },
        snapped,
    )
}

/// The topmost visible clip at a place on the canvas.
fn clip_at(clips: &[StageClip], canvas: (u32, u32), x: f64, y: f64) -> Option<ClipId> {
    clips
        .iter()
        .rev()
        .filter(|clip| clip.visible)
        .find(|clip| Shown::of(clip.placement, clip.size, canvas).contains(x, y))
        .map(|clip| clip.id)
}

fn on_screen(place: (f64, f64), frame: Rect) -> Pos2 {
    pos2(
        frame.left() + place.0 as f32 * frame.width(),
        frame.top() + place.1 as f32 * frame.height(),
    )
}

fn canvas_share(pointer: Pos2, frame: Rect) -> (f64, f64) {
    (
        f64::from((pointer.x - frame.left()) / frame.width().max(1.0)),
        f64::from((pointer.y - frame.top()) / frame.height().max(1.0)),
    )
}

/// Where the rotate handle goes: below the picture, the way it faces. If that would be off the
/// video area, it comes inside the picture instead, because it has to stay within reach.
fn rotate_handle_at(shown: &Shown, frame: Rect, area: Rect) -> Pos2 {
    let edge = on_screen(shown.at(0.0, 1.0), frame);
    let (x, y) = shown.turned(0.0, 1.0);
    let outward = vec2(x as f32, y as f32) * ROTATE_HANDLE_GAP;
    if area
        .shrink(ROTATE_HANDLE_SIZE / 2.0)
        .contains(edge + outward)
    {
        edge + outward
    } else {
        edge - outward
    }
}

/// Where a toolbar this wide goes: just above the clip and centred on it, staying on the video
/// area. When the row above the picture is free it can go up there, so it never has to cover a
/// full-size clip.
fn toolbar_rect(width: f32, clip: Rect, area: Rect, row_above_is_free: bool) -> Rect {
    let size = vec2(width, TOOLBAR_HEIGHT);
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
    Rotate,
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
    /// A rotation that snapped to an eighth of a turn, to show as a number while it is held.
    rotation_snapped: bool,
    /// Where the canvas was on screen in the last frame.
    pub frame: Option<Rect>,
    /// Where the rotate handle was in the last frame.
    pub rotate_handle: Option<Pos2>,
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
    /// Whether the selected clip can go further forward, and further back.
    pub can_go_forward: bool,
    pub can_go_back: bool,
}

/// What happened on the stage this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StageEvents {
    /// `Some(None)` clears the selection.
    pub select: Option<Option<ClipId>>,
    pub place: Option<(ClipId, Placement)>,
    pub order: Option<(ClipId, OrderMove)>,
}

/// What a press at this place on the canvas would grab, trying the selected clip's handles first.
fn grab_at(view: &StageView, pointer: Pos2) -> Option<(ClipId, DragKind)> {
    if let Some(selected) = view
        .clips
        .iter()
        .find(|clip| Some(clip.id) == view.selected)
    {
        let shown = Shown::of(selected.placement, selected.size, view.canvas);
        let handle = rotate_handle_at(&shown, view.frame, view.area);
        if handle.distance(pointer) <= ROTATE_HANDLE_SIZE / 2.0 {
            return Some((selected.id, DragKind::Rotate));
        }
        let corner = CORNERS.into_iter().find(|corner| {
            on_screen(shown.corner(*corner), view.frame).distance(pointer) <= HANDLE_REACH
        });
        if let Some(corner) = corner {
            return Some((selected.id, DragKind::Resize(corner)));
        }
        let (x, y) = canvas_share(pointer, view.frame);
        if shown.contains(x, y) {
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
    state.rotate_handle = None;
    let response = ui.interact(
        view.area,
        ui.scope_id().with("stage"),
        Sense::click_and_drag(),
    );

    if let Some(pointer) = response.hover_pos() {
        let cursor = match grab_at(view, pointer) {
            Some((id, DragKind::Resize(corner))) => view
                .clips
                .iter()
                .find(|clip| clip.id == id)
                .map(|clip| corner.cursor(&Shown::of(clip.placement, clip.size, view.canvas))),
            Some((_, DragKind::Rotate)) => Some(CursorIcon::Grab),
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
    state.rotation_snapped = false;
    if let (Some(drag), Some(pointer)) = (state.drag, response.interact_pointer_pos())
        && let Some(clip) = view.clips.iter().find(|clip| clip.id == drag.clip)
    {
        let at = canvas_share(pointer, view.frame);
        let targets = snap_targets(view.clips, drag.clip, view.canvas);
        let reach = reach_on(view.frame);
        let placement = match drag.kind {
            DragKind::Move => {
                let (placement, guides) = moved(
                    drag.start,
                    clip.size,
                    view.canvas,
                    (at.0 - drag.from.0, at.1 - drag.from.1),
                    &targets,
                    reach,
                );
                state.guides = guides;
                placement
            }
            DragKind::Resize(corner) => {
                let (placement, guides) = resized(
                    drag.start,
                    clip.size,
                    view.canvas,
                    corner,
                    at,
                    &targets,
                    reach,
                );
                state.guides = guides;
                placement
            }
            DragKind::Rotate => {
                let (placement, snapped) =
                    rotated(drag.start, clip.size, view.canvas, drag.from, at);
                state.rotation_snapped = snapped;
                placement
            }
        };
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
        let shown = Shown::of(placement, selected.size, view.canvas);
        let corners = CORNERS.map(|corner| on_screen(shown.corner(corner), view.frame));
        painter.add(Shape::closed_line(
            corners.to_vec(),
            Stroke::new(2.0, palette.playhead),
        ));
        for center in corners {
            painter.circle(
                center,
                HANDLE_SIZE / 2.0,
                palette.playhead,
                Stroke::new(1.5, palette.playhead_outline),
            );
        }
        let handle = rotate_handle_at(&shown, view.frame, view.area);
        state.rotate_handle = Some(handle);
        painter.circle(
            handle,
            ROTATE_HANDLE_SIZE / 2.0,
            palette.raised,
            Stroke::new(1.5, palette.playhead),
        );
        icons::paint(
            &painter,
            icons::square_at(handle, 14.0),
            &icons::ROTATE_CW,
            palette.text,
        );
        if state.drag.is_some_and(|drag| drag.kind == DragKind::Rotate) {
            painter.text(
                handle + vec2(ROTATE_HANDLE_SIZE, 0.0),
                egui::Align2::LEFT_CENTER,
                format!("{:.0}°", placement.degrees()),
                egui::FontId::proportional(13.0),
                palette.text,
            );
        }
        let lowest = |pick: fn(&Pos2) -> f32| corners.iter().map(pick).fold(f32::MAX, f32::min);
        let highest = |pick: fn(&Pos2) -> f32| corners.iter().map(pick).fold(f32::MIN, f32::max);
        let around = Rect::from_min_max(
            pos2(lowest(|c| c.x), lowest(|c| c.y)),
            pos2(highest(|c| c.x), highest(|c| c.y)),
        );
        toolbar(ui, palette, view, selected, placement, around, &mut events);
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
    let buttons = 4.0;
    let width = TOOLBAR_PADDING * 2.0 + TOOLBAR_BUTTON * buttons;
    let rect = toolbar_rect(width, clip_rect, view.area, view.row_above_is_free);
    let id = ui.scope_id().with("stage-toolbar");
    egui::Area::new(id)
        .order(egui::Order::Middle)
        .fixed_pos(rect.min)
        .fade_in(false)
        .constrain(false)
        .show(ui.ctx(), |ui| {
            ui.set_min_size(rect.size());
            ui.painter().rect(
                rect,
                8.0,
                palette.raised,
                Stroke::new(1.0, palette.border),
                egui::StrokeKind::Inside,
            );
            let spot = |index: f32| {
                Rect::from_min_size(
                    pos2(
                        rect.left() + TOOLBAR_PADDING + TOOLBAR_BUTTON * index,
                        rect.center().y - TOOLBAR_BUTTON / 2.0,
                    ),
                    vec2(TOOLBAR_BUTTON, TOOLBAR_BUTTON),
                )
            };
            let button =
                |ui: &mut egui::Ui, index: f32, icon: icons::Icon, name: &str, hint: &str| {
                    IconButton::new(palette, Kind::Ghost, icon, name)
                        .show_at(ui, id.with(name), spot(index))
                        .on_hover_text(hint)
                };

            if button(
                ui,
                0.0,
                icons::ROTATE_CW,
                "Rotate by 90 degrees",
                "Rotate by 90°",
            )
            .clicked()
            {
                events.place = Some((clip.id, placement.turned_quarter(clip.size, view.canvas)));
            }
            if button(
                ui,
                1.0,
                icons::FLIP_HORIZONTAL_2,
                "Flip left to right",
                "Flip left to right",
            )
            .clicked()
            {
                events.place = Some((clip.id, placement.flipped_across()));
            }
            if button(
                ui,
                2.0,
                icons::FLIP_VERTICAL_2,
                "Flip top to bottom",
                "Flip top to bottom",
            )
            .clicked()
            {
                events.place = Some((clip.id, placement.flipped_down()));
            }
            let more = button(ui, 3.0, icons::ELLIPSIS, "More", "More");
            egui::Popup::menu(&more)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    more_menu(ui, palette, view, clip, placement, events);
                });
        });
}

fn menu_heading(ui: &mut egui::Ui, palette: &Palette, text: &str) {
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(text)
                .size(12.0)
                .color(palette.text_muted),
        );
    });
}

/// What doesn't fit on the bar, or isn't needed as often.
fn more_menu(
    ui: &mut egui::Ui,
    palette: &Palette,
    view: &StageView,
    clip: &StageClip,
    placement: Placement,
    events: &mut StageEvents,
) {
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("Opacity")
                .size(13.0)
                .color(palette.text),
        );
        // The theme has no rail colour of its own that shows up on a raised surface.
        let visuals = &mut ui.style_mut().visuals;
        visuals.widgets.inactive.bg_fill = palette.border_strong;
        visuals.widgets.hovered.bg_fill = palette.border_strong;
        visuals.widgets.active.bg_fill = palette.border_strong;
        visuals.selection.bg_fill = palette.accent;
        ui.spacing_mut().slider_width = MORE_MENU_SLIDER_WIDTH;
        let percent = (share(placement.opacity) * 100.0).round() as u32;
        let mut opacity = f64::from(percent);
        let slider = ui
            .add(
                Slider::new(&mut opacity, 0.0..=100.0)
                    .show_value(false)
                    .step_by(1.0)
                    .trailing_fill(true),
            )
            .on_hover_text("How see-through this clip is");
        slider.widget_info(|| egui::WidgetInfo::slider(slider.enabled(), opacity, "Opacity"));
        ui.label(
            egui::RichText::new(format!("{percent}%"))
                .size(13.0)
                .color(palette.text_muted),
        );
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
    });
    ui.separator();
    menu_heading(ui, palette, "Picture in picture");
    for (corner, name) in CanvasCorner::ALL {
        if ui
            .add(MenuItem::new(
                palette,
                icons::PICTURE_IN_PICTURE_2,
                name,
                "",
            ))
            .clicked()
        {
            let placed = placement.in_corner(corner, clip.size, view.canvas);
            events.place = Some((clip.id, placed));
        }
    }
    ui.separator();
    menu_heading(ui, palette, "Order");
    let orders = [
        (
            OrderMove::ToFront,
            icons::CHEVRONS_UP,
            "Bring to front",
            view.can_go_forward,
        ),
        (
            OrderMove::Forward,
            icons::ARROW_UP,
            "Bring forward",
            view.can_go_forward,
        ),
        (
            OrderMove::Backward,
            icons::ARROW_DOWN,
            "Send backward",
            view.can_go_back,
        ),
        (
            OrderMove::ToBack,
            icons::CHEVRONS_DOWN,
            "Send to back",
            view.can_go_back,
        ),
    ];
    for (order, icon, name, allowed) in orders {
        if ui
            .add_enabled(allowed, MenuItem::new(palette, icon, name, ""))
            .clicked()
        {
            events.order = Some((clip.id, order));
        }
    }
    ui.separator();
    if ui
        .add(MenuItem::new(
            palette,
            icons::ROTATE_CCW,
            "Reset size and place",
            "",
        ))
        .clicked()
    {
        events.place = Some((clip.id, Placement::default()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANVAS: (u32, u32) = (1920, 1080);
    const WIDTH: f32 = 200.0;
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

        let toolbar = toolbar_rect(WIDTH, clip, area, true);

        assert_eq!(toolbar.bottom(), 200.0 - TOOLBAR_GAP);
        assert_eq!(toolbar.center().x, clip.center().x);
    }

    #[test]
    fn the_toolbar_goes_up_into_the_row_above_rather_than_covering_a_full_size_clip() {
        let area = Rect::from_min_size(pos2(100.0, 100.0), vec2(800.0, 600.0));

        let toolbar = toolbar_rect(WIDTH, area, area, true);

        assert_eq!(toolbar.top(), 100.0 - ROW_ABOVE);
        assert!(toolbar.bottom() <= area.top());
    }

    #[test]
    fn the_toolbar_stays_below_the_row_above_while_something_is_being_said_there() {
        let area = Rect::from_min_size(pos2(100.0, 100.0), vec2(800.0, 600.0));

        assert_eq!(toolbar_rect(WIDTH, area, area, false).top(), 100.0);
    }

    #[test]
    fn the_toolbar_stays_in_the_video_area_when_a_clip_runs_off_it() {
        let area = Rect::from_min_size(pos2(100.0, 100.0), vec2(800.0, 600.0));
        for clip in [
            Rect::from_min_size(pos2(120.0, -300.0), vec2(400.0, 300.0)),
            Rect::from_min_size(pos2(5000.0, 5000.0), vec2(40.0, 30.0)),
            Rect::from_min_size(pos2(-5000.0, -5000.0), vec2(40.0, 30.0)),
        ] {
            let toolbar = toolbar_rect(WIDTH, clip, area, false);

            assert!(area.contains_rect(toolbar), "{clip:?} {toolbar:?}");
        }
    }

    #[test]
    fn a_turned_clip_is_clicked_where_it_really_is() {
        let square = StageClip {
            size: (1000, 1000),
            ..stage_clip(
                1,
                Placement {
                    scale: WHOLE / 4,
                    rotation: 45_000,
                    ..Placement::default()
                },
            )
        };
        let canvas = (1000, 1000);
        // A quarter-size square on its point, in the middle of a square canvas.
        let clips = [square];

        assert_eq!(clip_at(&clips, canvas, 0.5, 0.5), Some(ClipId(1)));
        assert_eq!(
            clip_at(&clips, canvas, 0.66, 0.5),
            Some(ClipId(1)),
            "its point"
        );
        assert_eq!(clip_at(&clips, canvas, 0.62, 0.62), None, "beside its side");
    }

    #[test]
    fn the_box_around_a_turned_clip_is_what_snaps() {
        let upright = Placement {
            rotation: 90_000,
            scale: WHOLE / 2,
            ..Placement::default()
        };

        let area = area_of(upright, (1000, 500), (1000, 1000));

        // Half of 1000 x 500 is 500 x 250, which stands 250 wide and 500 tall once turned.
        assert!((area.width - 0.25).abs() < 1e-6, "{area:?}");
        assert!((area.height - 0.5).abs() < 1e-6, "{area:?}");
    }

    #[test]
    fn resizing_a_turned_clip_leaves_its_opposite_corner_still() {
        let start = Placement {
            rotation: 30_000,
            scale: WHOLE / 2,
            ..Placement::default()
        };
        let before = Shown::of(start, CANVAS, CANVAS).corner(Corner::TopLeft);
        let dragged_from = Shown::of(start, CANVAS, CANVAS).corner(Corner::BottomRight);

        let (placement, guides) = resized(
            start,
            CANVAS,
            CANVAS,
            Corner::BottomRight,
            (dragged_from.0 + 0.05, dragged_from.1 + 0.05),
            &NO_LINES(),
            NO_REACH,
        );

        let after = Shown::of(placement, CANVAS, CANVAS).corner(Corner::TopLeft);
        assert!(placement.scale > start.scale, "{placement:?}");
        assert!((after.0 - before.0).abs() < 1e-4, "{after:?} {before:?}");
        assert!((after.1 - before.1).abs() < 1e-4, "{after:?} {before:?}");
        assert!(guides.is_empty(), "a turned clip lines up with nothing");
    }

    #[test]
    fn dragging_around_a_clips_middle_turns_it_by_that_much() {
        // From straight below the middle round to straight left is a quarter turn clockwise.
        let (placement, snapped) =
            rotated(Placement::default(), CANVAS, CANVAS, (0.5, 0.9), (0.1, 0.5));

        assert_eq!(placement.rotation, 90_000);
        assert!(snapped);
    }

    #[test]
    fn a_turn_near_an_eighth_snaps_to_it_and_any_other_stays_where_it_is() {
        let at_degrees = |degrees: f64| {
            let radians = degrees.to_radians();
            // On a square canvas, so that shares and pixels agree.
            rotated(
                Placement::default(),
                (1000, 1000),
                (1000, 1000),
                (0.5, 0.1),
                (0.5 + 0.4 * radians.sin(), 0.5 - 0.4 * radians.cos()),
            )
        };

        let (close, snapped) = at_degrees(46.5);
        assert_eq!(close.rotation, 45_000);
        assert!(snapped);

        let (free, snapped) = at_degrees(60.0);
        assert!((free.rotation - 60_000).abs() < 100, "{free:?}");
        assert!(!snapped);
    }

    #[test]
    fn turning_keeps_a_clips_size_and_place() {
        let start = Placement {
            center_x: 300_000,
            scale: WHOLE / 2,
            ..Placement::default()
        };

        let (turned, _) = rotated(start, CANVAS, CANVAS, (0.3, 0.9), (0.9, 0.5));

        assert_eq!(turned.center_x, start.center_x);
        assert_eq!(turned.scale, start.scale);
    }

    #[test]
    fn the_rotate_handle_comes_inside_when_the_picture_fills_the_area() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 450.0));
        let filling = Shown::of(Placement::default(), (1600, 900), (1600, 900));

        let handle = rotate_handle_at(&filling, area, area);

        assert!(area.contains(handle));
        assert_eq!(handle.y, 450.0 - ROTATE_HANDLE_GAP);
    }

    #[test]
    fn the_rotate_handle_sits_below_a_small_picture() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 450.0));
        let small = Shown::of(
            Placement {
                scale: WHOLE / 2,
                ..Placement::default()
            },
            (1600, 900),
            (1600, 900),
        );

        let handle = rotate_handle_at(&small, area, area);

        assert_eq!(handle.y, 337.5 + ROTATE_HANDLE_GAP);
    }
}
