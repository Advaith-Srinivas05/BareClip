//! What can be done to clips on the picture itself: selecting them, moving, resizing and rotating
//! them, and the small toolbar that floats above whichever is selected.
//!
//! Sizes and places here are shares of the canvas, from 0 to 1, as they are in a `Placement`.
//! Turning only makes sense in pixels, because a canvas's width and height aren't equal.

use bareclip_core::{
    canvas::Colour,
    placement::{
        CanvasCorner, CropSides, FULL_TURN, LARGEST_SCALE, Placement, SMALLEST_SCALE, WHOLE,
    },
    sound::{ClipSound, FULL_VOLUME},
    speed::{FASTEST, NORMAL, SLOWEST, Speed},
    time::Ticks,
    timeline::ClipId,
    title::{TitleSpot, Way},
    tracks::{OrderMove, Tracks},
};
use eframe::egui::{self, CursorIcon, Pos2, Rect, Sense, Shape, Slider, Stroke, pos2, vec2};

use crate::{
    components::{ActionButton, ColourSwatch, IconButton, Kind, MenuItem, SwitchRow},
    icons, text,
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

/// Where a title's rotate handle goes. Coming inside would cover the words, and a caption near the
/// bottom of the picture is often too small to spare the room, so it tries each other side first.
fn title_rotate_handle_at(shown: &Shown, frame: Rect, area: Rect) -> Pos2 {
    let room = area.shrink(ROTATE_HANDLE_SIZE / 2.0);
    for (x, y) in [(0.0, 1.0), (1.0, 0.0), (-1.0, 0.0), (0.0, -1.0)] {
        let edge = on_screen(shown.at(x, y), frame);
        let (across, down) = shown.turned(x, y);
        let place = edge + vec2(across as f32, down as f32) * ROTATE_HANDLE_GAP;
        if room.contains(place) {
            return place;
        }
    }
    rotate_handle_at(shown, frame, area)
}

/// Where the selected clip's rotate handle goes.
fn selected_rotate_handle_at(shown: &Shown, view: &StageView) -> Pos2 {
    if view.selected_is_title {
        title_rotate_handle_at(shown, view.frame, view.area)
    } else {
        rotate_handle_at(shown, view.frame, view.area)
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

/// Which sides of a crop a handle moves, with -1 for the left or top and 1 for the right or bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CropHandle {
    across: i8,
    down: i8,
}

const CROP_HANDLES: [CropHandle; 8] = [
    CropHandle {
        across: -1,
        down: -1,
    },
    CropHandle {
        across: 0,
        down: -1,
    },
    CropHandle {
        across: 1,
        down: -1,
    },
    CropHandle { across: 1, down: 0 },
    CropHandle { across: 1, down: 1 },
    CropHandle { across: 0, down: 1 },
    CropHandle {
        across: -1,
        down: 1,
    },
    CropHandle {
        across: -1,
        down: 0,
    },
];
/// The least a crop can leave, each way, which matches what a placement holds to.
const SMALLEST_CROP_KEPT: f64 = 0.05;

/// The kept part of a picture, in its own terms from -1 to 1 each way, before turning.
fn crop_bounds(sides: CropSides) -> (f64, f64, f64, f64) {
    (
        sides.left * 2.0 - 1.0,
        sides.top * 2.0 - 1.0,
        1.0 - sides.right * 2.0,
        1.0 - sides.bottom * 2.0,
    )
}

/// Where a crop handle is on the whole picture.
fn crop_handle_at(handle: CropHandle, sides: CropSides, full: &Shown) -> (f64, f64) {
    let (left, top, right, bottom) = crop_bounds(sides);
    let along = |side: i8, low: f64, high: f64| match side {
        -1 => low,
        1 => high,
        _ => (low + high) / 2.0,
    };
    full.at(
        along(handle.across, left, right),
        along(handle.down, top, bottom),
    )
}

/// A crop with a handle dragged to a place on the canvas. Sides stop at the picture's edges, and
/// before they meet.
fn crop_dragged(
    handle: CropHandle,
    start: CropSides,
    full: &Shown,
    pointer: (f64, f64),
) -> CropSides {
    let (across, down) = full.unturned(
        pointer.0 * full.canvas.0 - full.center.0,
        pointer.1 * full.canvas.1 - full.center.1,
    );
    let x = (across / full.half.0.max(f64::EPSILON) + 1.0) / 2.0;
    let y = (down / full.half.1.max(f64::EPSILON) + 1.0) / 2.0;
    let mut sides = start;
    match handle.across {
        -1 => sides.left = x.clamp(0.0, (1.0 - SMALLEST_CROP_KEPT - start.right).max(0.0)),
        1 => sides.right = (1.0 - x).clamp(0.0, (1.0 - SMALLEST_CROP_KEPT - start.left).max(0.0)),
        _ => {}
    }
    match handle.down {
        -1 => sides.top = y.clamp(0.0, (1.0 - SMALLEST_CROP_KEPT - start.bottom).max(0.0)),
        1 => sides.bottom = (1.0 - y).clamp(0.0, (1.0 - SMALLEST_CROP_KEPT - start.top).max(0.0)),
        _ => {}
    }
    sides
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum DragKind {
    Move,
    Resize(Corner),
    Rotate,
    Crop(CropHandle),
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
    /// The clip being cropped, which shows whole until cropping ends.
    cropping: Option<ClipId>,
    /// A colour code partway through being typed in the bar's colour choices.
    art_colour_text: Option<String>,
}

impl StageState {
    /// Starts cropping a clip, which has to be selected and on the picture.
    pub fn begin_crop(&mut self, id: ClipId) {
        self.cropping = Some(id);
    }

    /// The clip being cropped, if any. It should be drawn whole, with its crop shown on top.
    pub fn cropping(&self) -> Option<ClipId> {
        self.cropping
    }

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
    /// The selected clip is a title, so it has no crop or flips, and its place comes from its
    /// own controls rather than picture-in-picture.
    pub selected_is_title: bool,
    /// The selected clip is something from the Content library, with its own bar.
    pub art: Option<ArtBar>,
}

/// What the bar above something from the Content library needs to know.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArtBar {
    /// What it is filled with, or its lines' colour if it is only lines.
    pub fill: Option<Colour>,
    /// How thick a shape's outline is. Only shapes have one to choose.
    pub outline: Option<u8>,
    /// It has a Shape panel for More options to open.
    pub has_more: bool,
    /// It covers the canvas, so it has no position to choose.
    pub places_itself: bool,
}

/// A change asked for on the bar above something from the Content library.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtChange {
    /// `None` leaves a shape empty.
    Fill(Option<Colour>),
    Outline(u8),
    Spot(TitleSpot),
    MoreOptions,
}

/// What happened on the stage this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StageEvents {
    /// `Some(None)` clears the selection.
    pub select: Option<Option<ClipId>>,
    pub place: Option<(ClipId, Placement)>,
    pub order: Option<(ClipId, OrderMove)>,
    pub sound: Option<(ClipId, ClipSound)>,
    pub speed: Option<(ClipId, Speed)>,
    pub detach: Option<ClipId>,
    /// A clip on the picture was right-clicked, and where, to open its menu there.
    pub menu: Option<(ClipId, Pos2)>,
    /// Something from the Content library was changed on its bar.
    pub art: Option<(ClipId, ArtChange)>,
}

/// Which of a clip's controls is showing in the right-hand panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LookTab {
    #[default]
    Filters,
    Effects,
    Colours,
    Fade,
    Speed,
    Audio,
    /// A title's words and how they look.
    Text,
    /// A shape's colours and outline, or a background's colours.
    Shape,
    /// How a shape comes in and goes out.
    Animate,
}

/// What the panel on the right of the window is showing, when it is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RightPanel {
    /// The project's captions, which need nothing selected.
    Captions,
    /// One of the selected clip's tools.
    Tool(LookTab),
}

/// What the Arrange controls need to know about a clip, wherever they are opened from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArrangeView {
    pub id: ClipId,
    pub placement: Placement,
    pub size: (u32, u32),
    pub canvas: (u32, u32),
    /// A title has no crop or flips, and its place comes from its own controls rather than
    /// picture-in-picture.
    pub is_title: bool,
    pub can_go_forward: bool,
    pub can_go_back: bool,
}

/// The selected clip's speed, if it can have one, for its controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectedSpeed {
    pub id: ClipId,
    pub speed: Speed,
    /// Whether it has sound, and so a pitch to keep.
    pub has_sound: bool,
}

/// A clip's sound, if it has any, for its controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectedSound {
    pub id: ClipId,
    pub sound: ClipSound,
    pub length: Ticks,
    /// A clip on a picture row with sound of its own can have it taken out.
    pub can_detach: bool,
    /// Only a clip on a row of sound can have noise taken out.
    pub can_clean: bool,
}

impl SelectedSound {
    /// How a clip's sound stands on these rows. Whether its file has any sound is up to the caller.
    pub fn of(tracks: &Tracks, id: ClipId) -> Option<Self> {
        let clip = tracks.clip(id)?;
        let on_sound_row = tracks.track_of(id).map(|row| tracks.is_sound_row(row))?;
        Some(Self {
            id,
            sound: clip.sound,
            length: clip.duration(),
            can_detach: !on_sound_row && !clip.sound.muted,
            can_clean: on_sound_row,
        })
    }
}

/// The crop handle at a place on the screen, on the clip being cropped.
fn crop_handle_near(clip: &StageClip, view: &StageView, pointer: Pos2) -> Option<CropHandle> {
    let full = Shown::of(
        clip.placement.uncropped(clip.size, view.canvas),
        clip.size,
        view.canvas,
    );
    let sides = clip.placement.crop_sides();
    CROP_HANDLES.into_iter().find(|handle| {
        on_screen(crop_handle_at(*handle, sides, &full), view.frame).distance(pointer)
            <= HANDLE_REACH
    })
}

/// What a press at this place on the canvas would grab, trying the selected clip's handles first.
fn grab_at(
    view: &StageView,
    cropping: Option<ClipId>,
    pointer: Pos2,
) -> Option<(ClipId, DragKind)> {
    if let Some(id) = cropping {
        // Only the crop's own handles do anything until cropping ends.
        let clip = view.clips.iter().find(|clip| clip.id == id)?;
        return crop_handle_near(clip, view, pointer).map(|handle| (id, DragKind::Crop(handle)));
    }
    if let Some(selected) = view
        .clips
        .iter()
        .find(|clip| Some(clip.id) == view.selected)
    {
        let shown = Shown::of(selected.placement, selected.size, view.canvas);
        let handle = selected_rotate_handle_at(&shown, view);
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
    // Cropping ends if its clip stops being selected or is no longer on the picture.
    state.cropping = state
        .cropping
        .filter(|id| view.selected == Some(*id) && view.clips.iter().any(|clip| clip.id == *id));
    let response = ui.interact(
        view.area,
        ui.scope_id().with("stage"),
        Sense::click_and_drag(),
    );

    if let Some(pointer) = response.hover_pos() {
        let cursor = match grab_at(view, state.cropping, pointer) {
            Some((_, DragKind::Crop(handle))) => Some(match (handle.across, handle.down) {
                (0, _) => CursorIcon::ResizeVertical,
                (_, 0) => CursorIcon::ResizeHorizontal,
                (across, down) if across == down => CursorIcon::ResizeNwSe,
                _ => CursorIcon::ResizeNeSw,
            }),
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
        match state
            .cropping
            .and_then(|id| view.clips.iter().find(|clip| clip.id == id))
        {
            // A click away from the picture being cropped means being done with it.
            Some(clip) => {
                let whole = clip.placement.uncropped(clip.size, view.canvas);
                if !Shown::of(whole, clip.size, view.canvas).contains(x, y) {
                    state.cropping = None;
                }
            }
            None => events.select = Some(clip_at(view.clips, view.canvas, x, y)),
        }
    }

    // Right-clicking a clip on the picture opens the same menu as on the timeline.
    if response.secondary_clicked()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let (x, y) = canvas_share(pointer, view.frame);
        if let Some(clip) = clip_at(view.clips, view.canvas, x, y) {
            events.select = Some(Some(clip));
            events.menu = Some((clip, pointer));
        }
    }

    if response.drag_started() {
        let origin = ui.input(|input| input.pointer.press_origin());
        if let Some(pointer) = origin
            && let Some((clip, kind)) = grab_at(view, state.cropping, pointer)
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
            DragKind::Crop(handle) => {
                let full = Shown::of(
                    drag.start.uncropped(clip.size, view.canvas),
                    clip.size,
                    view.canvas,
                );
                let sides = crop_dragged(handle, drag.start.crop_sides(), &full, at);
                drag.start.cropped_to(sides, clip.size, view.canvas)
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
        if state.cropping == Some(selected.id) {
            paint_crop(&painter, palette, view, selected, placement);
            crop_toolbar(
                ui,
                palette,
                view,
                state,
                selected,
                placement,
                corners,
                &mut events,
            );
            return events;
        }
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
        let handle = selected_rotate_handle_at(&shown, view);
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
        let box_around = around(&corners);
        toolbar(
            ui,
            palette,
            state,
            view,
            selected,
            placement,
            box_around,
            &mut events,
        );
    }
    events
}

/// How roomy the speed and sound controls are, in a menu or in the narrower right-hand panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlSize {
    slider: f32,
    choice: f32,
}

impl ControlSize {
    pub const MENU: Self = Self {
        slider: MORE_MENU_SLIDER_WIDTH,
        choice: (SPEED_MENU_WIDTH - SPEED_CHOICE_GAP * 3.0) / 4.0,
    };
    pub const PANEL: Self = Self {
        slider: 84.0,
        choice: (PANEL_CONTROLS_WIDTH - SPEED_CHOICE_GAP * 3.0) / 4.0,
    };
}

/// How wide the sound controls' popup is laid out, leaving room for labels and values.
const SOUND_LABEL_WIDTH: f32 = 58.0;
const SPEED_MENU_WIDTH: f32 = 250.0;
/// What is left of the right-hand panel inside its margins.
const PANEL_CONTROLS_WIDTH: f32 = 222.0;
const SPEED_BOX_WIDTH: f32 = 44.0;
/// As far as the fade sliders go. Longer fades can be typed in.
const FADE_SLIDER_SECONDS: f64 = 2.0;

fn sound_row_setup(ui: &mut egui::Ui, palette: &Palette, size: ControlSize) {
    // The theme has no rail colour of its own that shows up on a raised surface.
    let visuals = &mut ui.style_mut().visuals;
    visuals.widgets.inactive.bg_fill = palette.border_strong;
    visuals.widgets.hovered.bg_fill = palette.border_strong;
    visuals.widgets.active.bg_fill = palette.border_strong;
    visuals.selection.bg_fill = palette.accent;
    ui.spacing_mut().slider_width = size.slider;
}

fn sound_label(ui: &mut egui::Ui, palette: &Palette, text: &str) {
    ui.add_space(8.0);
    ui.add_sized(
        vec2(SOUND_LABEL_WIDTH, 18.0),
        egui::Label::new(egui::RichText::new(text).size(13.0).color(palette.text)),
    );
}

/// A fade's slider and the box to type a longer one in. Returns the new length in ticks.
fn fade_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    name: &str,
    current: i64,
    longest: Ticks,
    size: ControlSize,
) -> Option<i64> {
    let mut changed = None;
    ui.horizontal(|ui| {
        sound_row_setup(ui, palette, size);
        sound_label(ui, palette, name);
        let longest_seconds = longest.as_seconds();
        let mut seconds = Ticks(current).as_seconds().min(longest_seconds);
        let reach = FADE_SLIDER_SECONDS.min(longest_seconds).max(0.0);
        let mut on_slider = seconds.min(reach);
        let slider = ui
            .add_enabled(
                reach > 0.0,
                Slider::new(&mut on_slider, 0.0..=reach)
                    .show_value(false)
                    .step_by(0.1)
                    .trailing_fill(true),
            )
            .on_hover_text("Fades can reach up to half the clip");
        slider.widget_info(|| egui::WidgetInfo::slider(slider.enabled(), on_slider, name));
        if slider.changed() {
            seconds = on_slider;
            changed = Some(Ticks::from_seconds(seconds).0);
        }
        if slider.drag_stopped() || slider.clicked() {
            slider.surrender_focus();
        }
        let typed = ui.add(
            egui::DragValue::new(&mut seconds)
                .range(0.0..=longest_seconds)
                .speed(0.05)
                .max_decimals(2)
                .suffix(" s"),
        );
        if typed.changed() {
            changed = Some(Ticks::from_seconds(seconds).0);
        }
    });
    changed
}

/// The speeds one press away, in two rows of four.
const SPEED_CHOICES: [f64; 8] = [0.25, 0.5, 1.0, 1.5, 2.0, 4.0, 8.0, 16.0];
const SPEED_CHOICES_PER_ROW: usize = 4;
const SPEED_CHOICE_GAP: f32 = 4.0;

/// A speed from the slider, which moves in tenths. Finer ones can be typed.
fn slider_factor(factor: f64) -> f64 {
    ((factor * 10.0).round() / 10.0).clamp(slowest_factor(), fastest_factor())
}

fn slowest_factor() -> f64 {
    f64::from(SLOWEST) / f64::from(NORMAL)
}

fn fastest_factor() -> f64 {
    f64::from(FASTEST) / f64::from(NORMAL)
}

/// The speed someone typed, kept to two places and inside the limits. Either `1.5` or `1.5x` will
/// do, and anything that isn't a speed gives `None`, leaving the clip as it was.
fn typed_factor(typed: &str) -> Option<f64> {
    let typed = typed.trim();
    let number = typed.strip_suffix(['x', 'X']).unwrap_or(typed).trim_end();
    let factor: f64 = number.parse().ok()?;
    (factor.is_finite() && factor >= 0.0)
        .then(|| ((factor * 100.0).round() / 100.0).clamp(slowest_factor(), fastest_factor()))
}

/// The box that shows the speed and can be typed over. Pressing it selects the lot, ready to type
/// a new one, and it takes effect on Enter or on clicking away. Escape leaves things as they were.
fn speed_box(ui: &mut egui::Ui, factor: f64) -> Option<f64> {
    let id = ui.scope_id().with("speed-box");
    let showing = text::speed_name(factor);
    let mut typed = ui
        .data(|data| data.get_temp::<String>(id))
        .unwrap_or_else(|| showing.clone());
    let mut output = egui::TextEdit::singleline(&mut typed)
        .id(id)
        .desired_width(SPEED_BOX_WIDTH)
        .align(egui::Align2::CENTER_CENTER)
        .show(ui);
    let response = output
        .response
        .response
        .clone()
        .on_hover_text("Type any speed from 0.1 to 16");
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::TextInput, response.enabled(), "Speed value")
    });
    if response.gained_focus() {
        let everything = egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(typed.chars().count()),
        );
        output.state.cursor.set_char_range(Some(everything));
        output.state.store(ui.ctx(), id);
    }
    // A click away can close the menu around this box in the same frame, so it can't wait for
    // the focus to be lost on the next one.
    let finished = response.lost_focus() || (response.has_focus() && response.clicked_elsewhere());
    let escaped = ui.input(|input| input.key_pressed(egui::Key::Escape));
    if response.has_focus() && !finished {
        ui.data_mut(|data| data.insert_temp(id, typed.clone()));
    } else {
        ui.data_mut(|data| data.remove::<String>(id));
    }
    (finished && !escaped)
        .then(|| typed_factor(&typed))
        .flatten()
}

/// How fast a clip plays, and whether its voice keeps its pitch.
pub fn speed_menu(
    ui: &mut egui::Ui,
    palette: &Palette,
    selected: &SelectedSpeed,
    size: ControlSize,
    change: &mut Option<(ClipId, Speed)>,
) {
    let speed = selected.speed;
    let factor = speed.as_factor();
    ui.horizontal(|ui| {
        sound_row_setup(ui, palette, size);
        sound_label(ui, palette, "Speed");
        // Even steps along the slider are even multiples, so 0.5x and 2x sit either side of 1x.
        let mut along = factor.log2();
        let slider = ui
            .add(
                Slider::new(
                    &mut along,
                    slowest_factor().log2()..=fastest_factor().log2(),
                )
                .show_value(false)
                .trailing_fill(true),
            )
            .on_hover_text("Slower or faster than it was recorded");
        slider.widget_info(|| egui::WidgetInfo::slider(slider.enabled(), factor, "Clip speed"));
        if slider.changed() {
            *change = Some((selected.id, speed.with_factor(slider_factor(along.exp2()))));
        }
        if slider.drag_stopped() || slider.clicked() {
            slider.surrender_focus();
        }
        if let Some(typed) = speed_box(ui, factor) {
            *change = Some((selected.id, speed.with_factor(typed)));
        }
    });
    ui.add_space(4.0);
    ui.spacing_mut().item_spacing.y = SPEED_CHOICE_GAP;
    for row in SPEED_CHOICES.chunks(SPEED_CHOICES_PER_ROW) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = SPEED_CHOICE_GAP;
            for choice in row {
                let name = text::speed_name(*choice);
                let chosen = (factor - choice).abs() < 0.001;
                let button = ActionButton::new(palette, Kind::Ghost, &name)
                    .small()
                    .width(size.choice)
                    .selected(chosen);
                if ui.add(button).clicked() {
                    *change = Some((selected.id, speed.with_factor(*choice)));
                }
            }
        });
    }
    if selected.has_sound {
        ui.separator();
        let keep = MenuItem::choice(palette, "Keep pitch", speed.keep_pitch);
        if ui
            .add(keep)
            .on_hover_text("Voices stay at their own pitch. Off, sound rises and falls with speed, like a tape")
            .clicked()
        {
            *change = Some((
                selected.id,
                Speed {
                    keep_pitch: !speed.keep_pitch,
                    ..speed
                },
            ));
        }
    }
}

/// Volume, fades, mute and taking the sound out of a video.
pub fn sound_menu(
    ui: &mut egui::Ui,
    palette: &Palette,
    selected: &SelectedSound,
    size: ControlSize,
    events: &mut StageEvents,
) {
    let sound = selected.sound;
    ui.horizontal(|ui| {
        sound_row_setup(ui, palette, size);
        sound_label(ui, palette, "Volume");
        let percent = (f64::from(sound.volume) * 100.0 / f64::from(FULL_VOLUME)).round();
        let mut volume = percent;
        let slider = ui
            .add(
                Slider::new(&mut volume, 0.0..=200.0)
                    .show_value(false)
                    .step_by(1.0)
                    .trailing_fill(true),
            )
            .on_hover_text("100% is as it was recorded");
        slider.widget_info(|| egui::WidgetInfo::slider(slider.enabled(), volume, "Volume"));
        ui.label(
            egui::RichText::new(format!("{volume:.0}%"))
                .size(13.0)
                .color(palette.text_muted),
        );
        if slider.changed() {
            events.sound = Some((
                selected.id,
                ClipSound {
                    volume: (volume * f64::from(FULL_VOLUME) / 100.0).round() as u32,
                    ..sound
                },
            ));
        }
        // Holding on to the keyboard would stop the arrow keys moving the playhead.
        if slider.drag_stopped() || slider.clicked() {
            slider.surrender_focus();
        }
    });
    let longest = Ticks(selected.length.0 / 2);
    if let Some(fade_in) = fade_row(ui, palette, "Fade in", sound.fade_in, longest, size) {
        events.sound = Some((selected.id, ClipSound { fade_in, ..sound }));
    }
    if let Some(fade_out) = fade_row(ui, palette, "Fade out", sound.fade_out, longest, size) {
        events.sound = Some((selected.id, ClipSound { fade_out, ..sound }));
    }
    let on = sound.noise_suppression;
    // As wide as the rows above, which set the menu's width.
    let width = ui.min_rect().width();
    let row = ui
        .add_enabled(
            selected.can_clean,
            SwitchRow::new(palette, "Noise suppression", on)
                .with_info()
                .width(width),
        )
        .on_hover_text(text::NOISE_SUPPRESSION_HINT)
        .on_disabled_hover_text(text::NOISE_SUPPRESSION_ON_VIDEO);
    if row.clicked() {
        events.sound = Some((
            selected.id,
            ClipSound {
                noise_suppression: !on,
                ..sound
            },
        ));
    }
    ui.separator();
    let (icon, name) = if sound.muted {
        (icons::VOLUME_2, "Unmute clip")
    } else {
        (icons::VOLUME_X, "Mute clip")
    };
    if ui.add(MenuItem::new(palette, icon, name, "")).clicked() {
        events.sound = Some((
            selected.id,
            ClipSound {
                muted: !sound.muted,
                ..sound
            },
        ));
    }
    if selected.can_detach
        && ui
            .add(MenuItem::new(palette, icons::UNLINK, "Detach sound", ""))
            .on_hover_text("Puts this clip's sound on rows of its own, so it can be moved and changed by itself")
            .clicked()
    {
        events.detach = Some(selected.id);
    }
}

/// The rectangle around some points on the screen.
fn around(points: &[Pos2]) -> Rect {
    let lowest = |pick: fn(&Pos2) -> f32| points.iter().map(pick).fold(f32::MAX, f32::min);
    let highest = |pick: fn(&Pos2) -> f32| points.iter().map(pick).fold(f32::MIN, f32::max);
    Rect::from_min_max(
        pos2(lowest(|p| p.x), lowest(|p| p.y)),
        pos2(highest(|p| p.x), highest(|p| p.y)),
    )
}

/// Dims what a crop cuts away, and draws the kept part's outline and handles.
fn paint_crop(
    painter: &egui::Painter,
    palette: &Palette,
    view: &StageView,
    clip: &StageClip,
    placement: Placement,
) {
    let full = Shown::of(
        placement.uncropped(clip.size, view.canvas),
        clip.size,
        view.canvas,
    );
    let sides = placement.crop_sides();
    let (left, top, right, bottom) = crop_bounds(sides);
    let dim = palette.stage.gamma_multiply(0.65);
    let at = |x: f64, y: f64| on_screen(full.at(x, y), view.frame);
    for strip in [
        [(-1.0, -1.0), (left, -1.0), (left, 1.0), (-1.0, 1.0)],
        [(right, -1.0), (1.0, -1.0), (1.0, 1.0), (right, 1.0)],
        [(left, -1.0), (right, -1.0), (right, top), (left, top)],
        [(left, bottom), (right, bottom), (right, 1.0), (left, 1.0)],
    ] {
        painter.add(Shape::convex_polygon(
            strip.map(|(x, y)| at(x, y)).to_vec(),
            dim,
            Stroke::NONE,
        ));
    }
    painter.add(Shape::closed_line(
        [(left, top), (right, top), (right, bottom), (left, bottom)]
            .map(|(x, y)| at(x, y))
            .to_vec(),
        Stroke::new(2.0, palette.playhead),
    ));
    for handle in CROP_HANDLES {
        painter.circle(
            on_screen(crop_handle_at(handle, sides, &full), view.frame),
            HANDLE_SIZE / 2.0,
            palette.playhead,
            Stroke::new(1.5, palette.playhead_outline),
        );
    }
}

/// The bar while cropping: finish, or put back everything that was cut.
#[allow(clippy::too_many_arguments)]
fn crop_toolbar(
    ui: &mut egui::Ui,
    palette: &Palette,
    view: &StageView,
    state: &mut StageState,
    clip: &StageClip,
    placement: Placement,
    corners: [Pos2; 4],
    events: &mut StageEvents,
) {
    let width = TOOLBAR_PADDING * 2.0 + TOOLBAR_BUTTON * 2.0;
    let full = Shown::of(
        placement.uncropped(clip.size, view.canvas),
        clip.size,
        view.canvas,
    );
    let whole = CORNERS.map(|corner| on_screen(full.corner(corner), view.frame));
    let all: Vec<Pos2> = corners.iter().chain(&whole).copied().collect();
    let rect = toolbar_rect(width, around(&all), view.area, view.row_above_is_free);
    let id = ui.scope_id().with("crop-toolbar");
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
            let done = IconButton::new(palette, Kind::Ghost, icons::CHECK, "Finish cropping")
                .show_at(ui, id.with("done"), spot(0.0))
                .on_hover_text("Finish cropping");
            if done.clicked() {
                state.cropping = None;
            }
            let reset = IconButton::new(palette, Kind::Ghost, icons::ROTATE_CCW, "Reset crop")
                .show_at(ui, id.with("reset"), spot(1.0))
                .on_hover_text("Put back everything that was cut");
            if reset.clicked() {
                events.place = Some((clip.id, placement.uncropped(clip.size, view.canvas)));
            }
        });
}

/// The small bar above the selected clip. It only offers what works today. It sits in its own
/// layer, because when a clip fills the picture the only place that covers nothing is the row
/// above it, which the stage's own drawing can't reach.
#[allow(clippy::too_many_arguments)]
fn toolbar(
    ui: &mut egui::Ui,
    palette: &Palette,
    state: &mut StageState,
    view: &StageView,
    clip: &StageClip,
    placement: Placement,
    clip_rect: Rect,
    events: &mut StageEvents,
) {
    if let Some(bar) = view.art {
        art_toolbar(
            ui, palette, state, view, clip, placement, clip_rect, bar, events,
        );
        return;
    }
    let is_title = view.selected_is_title;
    let buttons = if is_title { 2.0 } else { 4.0 };
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

            if !is_title && button(ui, 0.0, icons::CROP, "Crop", "Crop").clicked() {
                state.cropping = Some(clip.id);
            }
            if button(
                ui,
                if is_title { 0.0 } else { 1.0 },
                icons::ROTATE_CW,
                "Rotate by 90 degrees",
                "Rotate by 90°",
            )
            .clicked()
            {
                events.place = Some((clip.id, placement.turned_quarter(clip.size, view.canvas)));
            }
            if !is_title {
                let flip = button(ui, 2.0, icons::FLIP_VERTICAL_2, "Flip", "Flip");
                egui::Popup::menu(&flip)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                    .show(|ui| flip_lines(ui, palette, clip.id, placement, events));
            }
            let more = button(ui, buttons - 1.0, icons::ELLIPSIS, "More", "More");
            egui::Popup::menu(&more)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    let arrange = ArrangeView {
                        id: clip.id,
                        placement,
                        size: clip.size,
                        canvas: view.canvas,
                        is_title,
                        can_go_forward: view.can_go_forward,
                        can_go_back: view.can_go_back,
                    };
                    arrange_controls(ui, palette, &arrange, events);
                });
        });
}

/// The bar above something from the Content library: its colour, a shape's outline, turning,
/// flipping, and the rest under More.
#[allow(clippy::too_many_arguments)]
fn art_toolbar(
    ui: &mut egui::Ui,
    palette: &Palette,
    state: &mut StageState,
    view: &StageView,
    clip: &StageClip,
    placement: Placement,
    clip_rect: Rect,
    bar: ArtBar,
    events: &mut StageEvents,
) {
    let has_outline = bar.outline.is_some();
    let buttons = if has_outline { 5.0 } else { 4.0 };
    let width = TOOLBAR_PADDING * 2.0 + TOOLBAR_BUTTON * buttons;
    let rect = toolbar_rect(width, clip_rect, view.area, view.row_above_is_free);
    let id = ui.scope_id().with("stage-art-toolbar");
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

            let shown = bar
                .fill
                .map_or(egui::Color32::TRANSPARENT, |[red, green, blue]| {
                    egui::Color32::from_rgb(red, green, blue)
                });
            let swatch = ui
                .put(
                    Rect::from_center_size(spot(0.0).center(), vec2(26.0, 26.0)),
                    ColourSwatch::new(palette, shown, "Colour", false),
                )
                .on_hover_text("Colour");
            if bar.fill.is_none() {
                ui.painter().line_segment(
                    [
                        swatch.rect.left_bottom() + vec2(4.0, -4.0),
                        swatch.rect.right_top() + vec2(-4.0, 4.0),
                    ],
                    Stroke::new(1.5, palette.text_muted),
                );
            }
            egui::Popup::menu(&swatch)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    if has_outline
                        && ui
                            .add(MenuItem::new(palette, icons::X, "No fill", ""))
                            .clicked()
                    {
                        events.art = Some((clip.id, ArtChange::Fill(None)));
                    }
                    let start = bar.fill.unwrap_or([255, 255, 255]);
                    if let Some(colour) = crate::components::colour_choices(
                        ui,
                        palette,
                        start,
                        &mut state.art_colour_text,
                    ) {
                        events.art = Some((clip.id, ArtChange::Fill(Some(colour))));
                    }
                });
            let mut next = 1.0;
            if let Some(outline) = bar.outline {
                let thickness = button(ui, next, icons::SQUARE, "Outline", "Outline");
                egui::Popup::menu(&thickness)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                    .show(|ui| {
                        for (width, name) in bareclip_core::art::OUTLINE_STEPS {
                            let icon = if width == outline {
                                icons::CHECK
                            } else {
                                icons::MINUS
                            };
                            if ui.add(MenuItem::new(palette, icon, name, "")).clicked() {
                                events.art = Some((clip.id, ArtChange::Outline(width)));
                            }
                        }
                    });
                next += 1.0;
            }
            if button(
                ui,
                next,
                icons::ROTATE_CW,
                "Rotate by 90 degrees",
                "Rotate by 90\u{b0}",
            )
            .clicked()
            {
                events.place = Some((clip.id, placement.turned_quarter(clip.size, view.canvas)));
            }
            let flip = button(ui, next + 1.0, icons::FLIP_VERTICAL_2, "Flip", "Flip");
            egui::Popup::menu(&flip)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                .show(|ui| flip_lines(ui, palette, clip.id, placement, events));
            let more = button(ui, buttons - 1.0, icons::ELLIPSIS, "More", "More");
            egui::Popup::menu(&more)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    if !bar.places_itself {
                        side_menu(ui, palette, icons::MOVE, "Position", |ui| {
                            position_lines(ui, palette, clip, placement, view.canvas, events);
                        });
                        ui.separator();
                    }
                    let arrange = ArrangeView {
                        id: clip.id,
                        placement,
                        size: clip.size,
                        canvas: view.canvas,
                        is_title: true,
                        can_go_forward: view.can_go_forward,
                        can_go_back: view.can_go_back,
                    };
                    arrange_controls(ui, palette, &arrange, events);
                    if bar.has_more {
                        ui.separator();
                        if ui
                            .add(MenuItem::new(palette, icons::ELLIPSIS, "More options", ""))
                            .clicked()
                        {
                            events.art = Some((clip.id, ArtChange::MoreOptions));
                        }
                    }
                });
        });
}

/// A line of a menu that opens more beside it, on being pointed at or pressed. What is picked
/// there leaves the menus open, as the rest of the menu does. Returns where the part beside it is
/// on screen, while it is open.
fn side_menu(
    ui: &mut egui::Ui,
    palette: &Palette,
    icon: icons::Icon,
    name: &str,
    content: impl FnOnce(&mut egui::Ui),
) -> Option<Rect> {
    use egui::containers::menu::{MenuConfig, SubMenu};
    let line = ui.add(MenuItem::opens_more(palette, icon, name));
    let config = MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    SubMenu::new()
        .config(config)
        .show(ui, &line, content)
        .map(|shown| shown.response.rect)
}

/// Where the Position menu puts something, left to right and then top to bottom.
pub const LINE_UP: [(Way, u8, &str); 6] = [
    (Way::Across, 0, "Left"),
    (Way::Across, 1, "Centre"),
    (Way::Across, 2, "Right"),
    (Way::Down, 0, "Top"),
    (Way::Down, 1, "Middle"),
    (Way::Down, 2, "Bottom"),
];

/// The Position menu: three places left to right and three top to bottom, each ticked while it
/// is lined up there. Picking one keeps where it is the other way.
fn position_lines(
    ui: &mut egui::Ui,
    palette: &Palette,
    clip: &StageClip,
    placement: Placement,
    canvas: (u32, u32),
    events: &mut StageEvents,
) {
    for (index, (way, along, name)) in LINE_UP.into_iter().enumerate() {
        if index == 3 {
            ui.separator();
        }
        let icon = match (way, along) {
            (Way::Across, 0) => icons::ALIGN_START_VERTICAL,
            (Way::Across, 1) => icons::ALIGN_CENTER_VERTICAL,
            (Way::Across, _) => icons::ALIGN_END_VERTICAL,
            (Way::Down, 0) => icons::ALIGN_START_HORIZONTAL,
            (Way::Down, 1) => icons::ALIGN_CENTER_HORIZONTAL,
            (Way::Down, _) => icons::ALIGN_END_HORIZONTAL,
        };
        let ticked = TitleSpot::lined_up_at(placement, clip.size, canvas, way) == Some(along);
        if !ui
            .add(MenuItem::ticked(palette, icon, name, ticked))
            .clicked()
        {
            continue;
        }
        // In a spot it stays there as it grows, so only the spot changes. Moved by hand, it only
        // moves the one way, unless that lands it in a spot.
        if let Some(spot) = TitleSpot::of(placement, clip.size, canvas) {
            events.art = Some((clip.id, ArtChange::Spot(spot.with(way, along))));
        } else {
            let moved = TitleSpot::lined_up(placement, clip.size, canvas, way, along);
            match TitleSpot::of(moved, clip.size, canvas) {
                Some(spot) => events.art = Some((clip.id, ArtChange::Spot(spot))),
                None => events.place = Some((clip.id, moved)),
            }
        }
    }
}

/// The two ways to flip a clip.
fn flip_lines(
    ui: &mut egui::Ui,
    palette: &Palette,
    id: ClipId,
    placement: Placement,
    events: &mut StageEvents,
) {
    let across = MenuItem::new(palette, icons::FLIP_VERTICAL_2, "Flip left to right", "");
    if ui.add(across).clicked() {
        events.place = Some((id, placement.flipped_across()));
    }
    let down = MenuItem::new(palette, icons::FLIP_HORIZONTAL_2, "Flip top to bottom", "");
    if ui.add(down).clicked() {
        events.place = Some((id, placement.flipped_down()));
    }
}

/// Everything in the Arrange part of a clip's right-click menu, which is the toolbar's choices
/// and its More menu together. Returns whether Crop was pressed, and where any side menu it
/// opened is on screen.
pub fn arrange_menu(
    ui: &mut egui::Ui,
    palette: &Palette,
    arrange: &ArrangeView,
    can_crop: bool,
    events: &mut StageEvents,
) -> (bool, Option<Rect>) {
    let mut crop = false;
    if !arrange.is_title {
        crop = ui
            .add_enabled(can_crop, MenuItem::new(palette, icons::CROP, "Crop", ""))
            .on_disabled_hover_text("Move the playhead inside this clip to crop it")
            .clicked();
    }
    let rotate = MenuItem::new(palette, icons::ROTATE_CW, "Rotate by 90 degrees", "");
    if ui.add(rotate).clicked() {
        let turned = arrange
            .placement
            .turned_quarter(arrange.size, arrange.canvas);
        events.place = Some((arrange.id, turned));
    }
    if !arrange.is_title {
        flip_lines(ui, palette, arrange.id, arrange.placement, events);
    }
    ui.separator();
    let beside = arrange_controls(ui, palette, arrange, events);
    (crop, beside)
}

/// What doesn't fit on the bar, or isn't needed as often.
/// Returns where any side menu it opened is on screen.
fn arrange_controls(
    ui: &mut egui::Ui,
    palette: &Palette,
    arrange: &ArrangeView,
    events: &mut StageEvents,
) -> Option<Rect> {
    let placement = arrange.placement;
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
                arrange.id,
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
    let mut beside: Option<Rect> = None;
    let mut widen = |rect: Option<Rect>| {
        beside = match (beside, rect) {
            (Some(a), Some(b)) => Some(a.union(b)),
            (a, b) => a.or(b),
        };
    };
    if !arrange.is_title {
        ui.separator();
        widen(side_menu(
            ui,
            palette,
            icons::PICTURE_IN_PICTURE_2,
            "Picture in picture",
            |ui| {
                for (corner, name) in CanvasCorner::ALL {
                    let line = MenuItem::new(palette, icons::PICTURE_IN_PICTURE_2, name, "");
                    if ui.add(line).clicked() {
                        let placed = placement.in_corner(corner, arrange.size, arrange.canvas);
                        events.place = Some((arrange.id, placed));
                    }
                }
            },
        ));
    }
    ui.separator();
    let orders = [
        (
            OrderMove::ToFront,
            icons::CHEVRONS_UP,
            "Bring to front",
            arrange.can_go_forward,
        ),
        (
            OrderMove::Forward,
            icons::ARROW_UP,
            "Bring forward",
            arrange.can_go_forward,
        ),
        (
            OrderMove::Backward,
            icons::ARROW_DOWN,
            "Send backward",
            arrange.can_go_back,
        ),
        (
            OrderMove::ToBack,
            icons::CHEVRONS_DOWN,
            "Send to back",
            arrange.can_go_back,
        ),
    ];
    widen(side_menu(ui, palette, icons::LAYERS, "Order", |ui| {
        for (order, icon, name, allowed) in orders {
            if ui
                .add_enabled(allowed, MenuItem::new(palette, icon, name, ""))
                .clicked()
            {
                events.order = Some((arrange.id, order));
            }
        }
    }));
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
        events.place = Some((arrange.id, Placement::default()));
    }
    beside
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_slider_moves_in_tenths_and_stays_inside_the_limits() {
        assert_eq!(slider_factor(1.34), 1.3);
        assert_eq!(slider_factor(1.0001), 1.0);
        assert_eq!(slider_factor(0.07), 0.1);
        assert_eq!(slider_factor(15.98), 16.0);
        for step in 1..=160 {
            let wanted = f64::from(step) / 10.0;
            assert_eq!(slider_factor(wanted), wanted);
        }
    }

    #[test]
    fn a_typed_speed_can_be_finer_than_the_slider_and_stays_inside_the_limits() {
        assert_eq!(typed_factor("0.25"), Some(0.25));
        assert_eq!(typed_factor(" 0.75x "), Some(0.75));
        assert_eq!(typed_factor("2X"), Some(2.0));
        assert_eq!(typed_factor("1.234"), Some(1.23));
        assert_eq!(typed_factor("50"), Some(16.0));
        assert_eq!(typed_factor("0.01"), Some(0.1));
    }

    #[test]
    fn anything_typed_that_isnt_a_speed_changes_nothing() {
        for typed in ["", "x", "fast", "-2", "1,5", "NaN", "inf"] {
            assert_eq!(typed_factor(typed), None, "{typed}");
        }
    }

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
    fn a_titles_rotate_handle_goes_beside_it_rather_than_over_its_words() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 360.0));
        let picture = (100, 20);
        let canvas = (640, 360);
        let at_the_bottom = Placement {
            center_y: WHOLE * 93 / 100,
            scale: WHOLE * 100 / 640,
            ..Placement::default()
        };
        let shown = Shown::of(at_the_bottom, picture, canvas);
        let handle = title_rotate_handle_at(&shown, area, area);
        let (left, right) = (
            on_screen(shown.at(-1.0, 0.0), area).x,
            on_screen(shown.at(1.0, 0.0), area).x,
        );
        assert!(
            handle.x > right || handle.x < left,
            "{handle:?} is over the words"
        );
        assert!(area.contains(handle));

        let in_the_middle = Placement {
            scale: WHOLE * 100 / 640,
            ..Placement::default()
        };
        let shown = Shown::of(in_the_middle, picture, canvas);
        let below = title_rotate_handle_at(&shown, area, area);
        assert!(
            below.y > on_screen(shown.at(0.0, 1.0), area).y,
            "below is first choice"
        );
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

    fn whole_picture(canvas: (u32, u32)) -> Shown {
        Shown::of(Placement::default(), canvas, canvas)
    }

    #[test]
    fn dragging_a_crop_handle_moves_only_its_own_sides() {
        let full = whole_picture((1000, 1000));
        let left_edge = CropHandle {
            across: -1,
            down: 0,
        };
        let corner = CropHandle { across: 1, down: 1 };

        let sides = crop_dragged(left_edge, CropSides::default(), &full, (0.25, 0.8));
        assert!((sides.left - 0.25).abs() < 1e-9, "{sides:?}");
        assert_eq!((sides.right, sides.top, sides.bottom), (0.0, 0.0, 0.0));

        let sides = crop_dragged(corner, CropSides::default(), &full, (0.75, 0.6));
        assert!((sides.right - 0.25).abs() < 1e-9, "{sides:?}");
        assert!((sides.bottom - 0.4).abs() < 1e-9, "{sides:?}");
        assert_eq!((sides.left, sides.top), (0.0, 0.0));
    }

    #[test]
    fn a_crop_stops_at_the_pictures_edge() {
        let full = whole_picture((1000, 1000));
        let left_edge = CropHandle {
            across: -1,
            down: 0,
        };

        let sides = crop_dragged(left_edge, CropSides::default(), &full, (-3.0, 0.5));

        assert_eq!(sides.left, 0.0);
    }

    #[test]
    fn a_crop_always_leaves_something() {
        let full = whole_picture((1000, 1000));
        let left_edge = CropHandle {
            across: -1,
            down: 0,
        };
        let start = CropSides {
            right: 0.3,
            ..CropSides::default()
        };

        let sides = crop_dragged(left_edge, start, &full, (5.0, 0.5));

        assert!((1.0 - sides.left - sides.right - SMALLEST_CROP_KEPT).abs() < 1e-9);
    }

    #[test]
    fn a_crop_on_a_turned_picture_goes_by_the_pictures_own_sides() {
        let canvas = (1000, 1000);
        let turned = Shown::of(
            Placement {
                rotation: 90_000,
                scale: WHOLE / 2,
                ..Placement::default()
            },
            canvas,
            canvas,
        );
        let left_edge = CropHandle {
            across: -1,
            down: 0,
        };

        // Turned a quarter clockwise, its left side is at the top. Half size spans 0.25 to 0.75,
        // so a pointer a quarter of the way down it is a quarter of the way in from that side.
        let sides = crop_dragged(left_edge, CropSides::default(), &turned, (0.5, 0.375));

        assert!((sides.left - 0.25).abs() < 1e-6, "{sides:?}");
    }

    #[test]
    fn crop_handles_sit_on_what_is_kept() {
        let full = whole_picture((1000, 1000));
        let sides = CropSides {
            left: 0.2,
            top: 0.1,
            right: 0.3,
            bottom: 0.4,
        };

        let top_left = crop_handle_at(
            CropHandle {
                across: -1,
                down: -1,
            },
            sides,
            &full,
        );
        let right_middle = crop_handle_at(CropHandle { across: 1, down: 0 }, sides, &full);

        assert!((top_left.0 - 0.2).abs() < 1e-6 && (top_left.1 - 0.1).abs() < 1e-6);
        assert!((right_middle.0 - 0.7).abs() < 1e-6 && (right_middle.1 - 0.35).abs() < 1e-6);
    }
}
