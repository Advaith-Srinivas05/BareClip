//! Converts between timeline time and screen position, and draws the timeline.

use bareclip_core::{
    speed::Speed,
    time::{Ticks, format_timecode},
    timeline::{ClipEdge, ClipId, MediaId, TimeRange},
    tracks::{TrackTarget, Tracks},
};
use bareclip_media::SoundShape;
use eframe::egui::{
    self, Align, Align2, Color32, CursorIcon, FontId, Galley, Layout, Mesh, Popup,
    PopupCloseBehavior, PopupKind, Pos2, Rect, RectAlign, Sense, Shape, Stroke, TextureHandle,
    UiBuilder, Vec2, pos2, vec2,
};
use std::{collections::HashMap, sync::Arc};

use crate::{
    components::{ActionButton, IconButton, Kind, MenuItem},
    icons::{self, Icon},
    stage, text,
    theme::Palette,
};

const RULER_HEIGHT: f32 = 24.0;
/// A track keeps this height however tall the timeline gets, so more height shows more space.
const TRACK_HEIGHT: f32 = 56.0;
const TRACK_GAP: f32 = 6.0;
/// The dashed lanes above and below the tracks, which a clip can be dropped on.
const NEW_LANE_HEIGHT: f32 = 26.0;
/// This much at the top and bottom of a track means a new track there rather than this one.
const TRACK_EDGE_ZONE: f32 = 7.0;
const TRACK_SCROLLBAR_WIDTH: f32 = 6.0;
/// Holding a drag this close to the top or bottom of the tracks scrolls them.
const TRACK_EDGE_SCROLL_ZONE: f32 = 18.0;
const TRACK_EDGE_SCROLL_SPEED: f32 = 240.0;
const INSERT_LINE_WIDTH: f32 = 3.0;
const SEEK_BAR_HEIGHT: f32 = 20.0;
const SEEK_HANDLE_RADIUS: f32 = 7.0;
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
/// A handle is this wide on screen, and the same width can be grabbed.
const HANDLE_WIDTH: f32 = 14.0;
const HANDLE_GRIP_WIDTH: f32 = 3.0;
const SELECTED_OUTLINE: f32 = 2.5;
const HOVERED_OUTLINE: f32 = 1.5;
/// A clip wider than this has room for a symbol in front of its name.
const CLIP_ICON_MIN_WIDTH: f32 = 96.0;
const CLIP_ICON_SIZE: f32 = 14.0;
const CLIP_LABEL_HEIGHT: f32 = 20.0;
const CLIP_LABEL_TOP: f32 = 3.0;
const CLIP_LABEL_BACKING: f32 = 0.72;
/// The sound's shape takes up this share of a clip, along its bottom.
const SOUND_BAND_SHARE: f32 = 0.3;
const SOUND_BAND_BACKING: f32 = 0.45;
const SOUND_COLUMN_WIDTH: f32 = 2.0;
const ZOOM_STEP: f64 = 1.5;
/// Below this, a view of the timeline stops getting any closer.
const MIN_VISIBLE_SECONDS: f64 = 0.5;
/// The timeline always runs on past the video, so there is room to drag things into.
const MIN_EXTENT_SECONDS: f64 = 10.0;
const EXTENT_PER_PROJECT: i64 = 2;
/// A new clip leaves some of the empty timeline in view, so there is room to drop the next.
const AUTO_FIT_FRACTION: f64 = 0.75;
/// How close a dragged clip gets to something before it snaps to it.
const SNAP_PIXELS: f32 = 8.0;
/// Holding a drag this close to the edge of the view scrolls it.
const EDGE_SCROLL_ZONE: f32 = 28.0;
const EDGE_SCROLL_VIEWS_PER_SECOND: f64 = 0.8;
/// A gap narrower than this has no room for its button. Delete still removes it.
const GAP_BUTTON_WIDTH: f32 = 104.0;
/// A gap too narrow for the button's name still has room for its symbol down to this.
const GAP_ICON_BUTTON_SIZE: f32 = 26.0;
const PLAYHEAD_HEAD_HEIGHT: f32 = 16.0;
const PLAYHEAD_HEAD_REACH: f32 = 8.0;
const PLAYHEAD_LINE_REACH: f32 = 4.0;
/// How strongly a hovered or selected gap stands out from the rest of the timeline.
const GAP_HIGHLIGHT: f32 = 0.07;
/// The selected clip's stretch of the ruler is tinted more than the rest of the project.
const PROJECT_TINT: f32 = 0.18;
const SELECTED_TINT: f32 = 0.3;
/// The time above the playhead while it is dragged sits this far over the ruler.
const BUBBLE_ROOM: f32 = 24.0;
const ZOOM_CARD_RADIUS: f32 = 8.0;
const SNAP_LINE_WIDTH: f32 = 1.5;
const HATCH_SPACING: f32 = 8.0;
/// The space between a trim's time and the clip, when the clip is too short to hold it.
const TRIM_TAG_GAP: f32 = 4.0;
/// How strongly the outline of a quiet label on the ruler shows.
const RULER_TAG_EDGE: f32 = 0.45;
const NEW_TRACK_HINT: &str = "Drag a clip here for a new track";
const ADD_AUDIO_HINT: &str = "Add audio";
/// Room left of the rows, so the playhead's head isn't cut off at the very start.
const TRACKS_LEFT: f32 = 10.0;
/// How long the pointer rests on a video before its name shows, and how it arrives.
const NAME_HOVER_SECONDS: f64 = 1.0;
const NAME_FADE_SECONDS: f32 = 0.15;
const NAME_RISE: f32 = 3.0;
/// How much of a clip has to show for a name that stays on it, or for its speed.
const KEPT_NAME_MIN_WIDTH: f32 = 60.0;
const SPEED_TAG_MIN_WIDTH: f32 = 44.0;
const FLYOUT_GAP: f32 = 4.0;
/// How far a menu's frame reaches past what is in it.
const MENU_FRAME_ROOM: f32 = 8.0;
/// A clip this narrow has no room for its speaker.
const SPEAKER_MIN_WIDTH: f32 = 72.0;
const SPEAKER_SIZE: f32 = 22.0;
const SPEAKER_ICON: f32 = 13.0;
/// How much of a clip of nothing but sound its shape can fill.
const SOUND_CLIP_BAND_SHARE: f32 = 0.6;
const MUTED_SOUND_SHAPE: f32 = 0.35;
const FADE_SHADE: f32 = 0.55;

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
    /// Which track it is on, counting from the top.
    pub track: usize,
    pub start: Ticks,
    pub end: Ticks,
    pub label: String,
    pub media: MediaId,
    /// The moment in its video that the clip starts from.
    pub source_start: Ticks,
    /// Whether its file has sound at all, and so a speaker to press.
    pub has_sound: bool,
    pub muted: bool,
    /// Whether it is on a row of sound, with no picture.
    pub sound_only: bool,
    /// Whether the settings ask for this clip's sound to be drawn on it.
    pub shows_waveform: bool,
    /// Whether it shows an image.
    pub is_still: bool,
    pub speed: Speed,
    /// How long its fades really are.
    pub fade_in: Ticks,
    pub fade_out: Ticks,
}

/// What a video has to show on its clips so far. Each picture is paired with where it is in
/// the video, and they stay in that order.
#[derive(Clone, Copy, Default)]
pub struct ClipFace<'a> {
    pub pictures: &'a [(Ticks, TextureHandle)],
    pub sound: Option<&'a SoundShape>,
}

/// The picture to show for a moment in a video: the last one at or before it.
fn picture_index_at(pictures: &[(Ticks, TextureHandle)], time: Ticks) -> usize {
    pictures
        .partition_point(|(at, _)| *at <= time)
        .saturating_sub(1)
}

/// The moment in a clip's video that a place on screen shows.
fn source_time_at(clip: &ClipView, scale: &TimeScale, x: f32) -> Ticks {
    let along = scale.time_of_pixels(x - scale.x_of(clip.start));
    Ticks(clip.source_start.0 + clip.speed.in_footage(along).0)
}

/// What the timeline needs to know to draw the project.
pub struct TimelineView<'a> {
    pub tracks: &'a Tracks,
    /// Every clip, in the order of its track and then its start.
    pub clips: &'a [ClipView],
    pub faces: &'a HashMap<MediaId, ClipFace<'a>>,
    pub position: Ticks,
    pub selected: Option<ClipId>,
    pub selected_gap: Option<Ticks>,
}

/// A video being dragged from the media panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaDrag {
    pub media: MediaId,
    pub length: Ticks,
    /// A file with no picture, which can only go on a row of sound.
    pub sound_only: bool,
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
        let grab = HANDLE_WIDTH.min((right - left) / 3.0);
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

/// Where the tracks sit on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackLayout {
    pub area: Rect,
    pub count: usize,
    pub scroll: f32,
}

impl TrackLayout {
    /// The dashed lanes only show on a single track. From two tracks on they are clutter,
    /// and a drag still makes a new track above, between or below, lighting up where.
    pub fn has_lanes(count: usize) -> bool {
        count == 1
    }

    /// How tall the tracks and any dashed lanes are together.
    fn content_height(count: usize) -> f32 {
        if count == 0 {
            return 0.0;
        }
        let gaps = count as f32 + 1.0;
        let lanes = if Self::has_lanes(count) {
            NEW_LANE_HEIGHT * 2.0
        } else {
            0.0
        };
        lanes + (TRACK_HEIGHT * count as f32) + (TRACK_GAP * gaps)
    }

    /// How far the tracks can scroll. Nothing scrolls while they all fit.
    fn furthest_scroll(count: usize, height: f32) -> f32 {
        (Self::content_height(count) - height).max(0.0)
    }

    /// The top of the first track. While everything fits the tracks sit in the middle.
    fn first_top(&self) -> f32 {
        let room = self.area.height() - Self::content_height(self.count);
        let content_top = if room > 0.0 {
            self.area.top() + room / 2.0
        } else {
            self.area.top() - self.scroll
        };
        let lane = if Self::has_lanes(self.count) {
            NEW_LANE_HEIGHT
        } else {
            0.0
        };
        content_top + lane + TRACK_GAP
    }

    pub fn track(&self, index: usize) -> Rect {
        let top = self.first_top() + index as f32 * (TRACK_HEIGHT + TRACK_GAP);
        Rect::from_min_size(
            pos2(self.area.left(), top),
            vec2(self.area.width(), TRACK_HEIGHT),
        )
    }

    /// Where the lane above is, or where one lights up during a drag once lanes are hidden.
    pub fn new_lane_above(&self) -> Rect {
        let top = self.first_top() - TRACK_GAP - NEW_LANE_HEIGHT;
        Rect::from_min_size(
            pos2(self.area.left(), top),
            vec2(self.area.width(), NEW_LANE_HEIGHT),
        )
    }

    /// Where the lane below is, or where one lights up during a drag once lanes are hidden.
    pub fn new_lane_below(&self) -> Rect {
        let top = self.first_top() + self.count as f32 * (TRACK_HEIGHT + TRACK_GAP);
        Rect::from_min_size(
            pos2(self.area.left(), top),
            vec2(self.area.width(), NEW_LANE_HEIGHT),
        )
    }

    /// The track at a height on screen, if there is one there.
    fn track_at(&self, y: f32) -> Option<usize> {
        (0..self.count).find(|index| {
            let track = self.track(*index);
            y >= track.top() && y <= track.bottom()
        })
    }

    /// What a drag at this height would land on. The middle of a track means that track.
    /// Its edges, the space between tracks and everything above or below them mean a new
    /// track there.
    pub fn target_at(&self, y: f32) -> TrackTarget {
        for index in 0..self.count {
            let track = self.track(index);
            if y < track.top() + TRACK_EDGE_ZONE {
                return TrackTarget::New(index);
            }
            if y <= track.bottom() - TRACK_EDGE_ZONE {
                return TrackTarget::Existing(index);
            }
        }
        TrackTarget::New(self.count)
    }

    /// What dragging a clip at this height would land on. Anywhere on its own track keeps
    /// it there, so sliding a clip along that was grabbed by its edge can't make a track.
    pub fn move_target_at(&self, y: f32, own_track: usize) -> TrackTarget {
        if self.track_at(y) == Some(own_track) {
            TrackTarget::Existing(own_track)
        } else {
            self.target_at(y)
        }
    }
}

/// The clip and part at a screen position.
fn clip_at<'a>(
    clips: &'a [ClipView],
    layout: &TrackLayout,
    scale: &TimeScale,
    pointer: Pos2,
) -> Option<(&'a ClipView, ClipPart)> {
    let track = layout.track_at(pointer.y)?;
    let first = clips.partition_point(|clip| clip.track < track);
    let end = clips.partition_point(|clip| clip.track <= track);
    let on_track = &clips[first..end];
    let (index, part) = part_at(on_track, scale, pointer.x)?;
    Some((&on_track[index], part))
}

/// Holding a drag near the top or bottom of the tracks moves through them.
fn scroll_toward_edge(ui: &egui::Ui, scroll: f32, furthest: f32, area: Rect, y: f32) -> f32 {
    if furthest <= 0.0 {
        return scroll;
    }
    let up = (area.top() + TRACK_EDGE_SCROLL_ZONE - y) / TRACK_EDGE_SCROLL_ZONE;
    let down = (y - (area.bottom() - TRACK_EDGE_SCROLL_ZONE)) / TRACK_EDGE_SCROLL_ZONE;
    let speed = if up > 0.0 {
        -up.min(1.0)
    } else if down > 0.0 {
        down.min(1.0)
    } else {
        return scroll;
    };
    ui.ctx().request_repaint();
    let delta = ui.input(|input| input.stable_dt).min(0.1);
    (scroll + speed * TRACK_EDGE_SCROLL_SPEED * delta).clamp(0.0, furthest)
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

/// What a placed clip's edges have lined up with, if anything.
fn snap_target(start: Ticks, length: Ticks, targets: &[Ticks]) -> Option<Ticks> {
    let end = Ticks(start.0 + length.0);
    targets
        .iter()
        .copied()
        .find(|target| *target == start || *target == end)
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

/// What a right-click on the timeline opened its menu on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuTarget {
    Clip(ClipId),
    Gap(Ticks),
}

/// What was picked from the timeline's menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Split,
    Duplicate(ClipId),
    Delete(ClipId),
    ToggleMute(ClipId),
    Detach(ClipId),
    RemoveGap(Ticks),
    RemoveAllGaps,
}

/// The part of the clip's menu that opens beside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuFlyout {
    Speed,
    Audio,
}

/// What is open beside the clip's menu.
#[derive(Clone, Copy, Debug, Default)]
struct MenuFlyouts {
    open: Option<MenuFlyout>,
    rect: Option<Rect>,
    /// The pointer is on its way across other lines to what is open, so they don't take over.
    heading_there: bool,
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
    /// How far down through the tracks the view has scrolled, in pixels.
    pub track_scroll: f32,
    /// Where things were drawn in the last frame. `track` is the top track, or the whole
    /// area while there are none.
    pub track: Rect,
    pub layout: TrackLayout,
    pub ruler: Rect,
    pub scale: TimeScale,
    pub scrollbar: Rect,
    pub zoom_buttons: [Rect; 3],
    /// The moment under the pointer, if it is over the timeline.
    pub hover_time: Option<Ticks>,
    /// The time shown under the pointer, which is only over empty parts of the timeline.
    pub hover_readout: Option<Ticks>,
    /// Where the clip being trimmed starts and ends, and which end has hold of the pointer.
    pub trim_readout: Option<(Ticks, Ticks, ClipEdge)>,
    /// Where the playhead was last sent during this drag.
    scrubbed_to: Option<Ticks>,
    pub menu_target: Option<MenuTarget>,
    /// What the last frame showed about whatever the pointer is doing.
    pub hovered_clip: Option<ClipId>,
    pub hovered_gap: Option<TimeRange>,
    pub snap_line: Option<Ticks>,
    /// The clip the pointer has rested on, and when it arrived.
    name_hover: Option<(ClipId, f64)>,
    /// The clip whose name the pointer has rested on for long enough to show.
    pub named_clip: Option<ClipId>,
    menu_flyouts: MenuFlyouts,
    /// How far the pointer was from the clip's edge when a trim began.
    trim_grab: Ticks,
    /// What a trimmed edge really lined up with in the last frame, which a target the
    /// clip's footage can't reach does not count as.
    pub trim_aligned: Option<Ticks>,
    pub playhead_bubble: bool,
}

impl Default for TimelineState {
    fn default() -> Self {
        Self {
            visible: Ticks::from_seconds(MIN_EXTENT_SECONDS),
            auto_fit: true,
            scroll: Ticks(0),
            drag: TimelineDrag::None,
            frozen_total: None,
            track_scroll: 0.0,
            track: Rect::NOTHING,
            ruler: Rect::NOTHING,
            layout: TrackLayout {
                area: Rect::NOTHING,
                count: 0,
                scroll: 0.0,
            },
            scale: TimeScale {
                visible_start: Ticks(0),
                visible: Ticks(0),
                left: 0.0,
                width: 0.0,
            },
            scrollbar: Rect::NOTHING,
            zoom_buttons: [Rect::NOTHING; 3],
            hover_time: None,
            hover_readout: None,
            trim_readout: None,
            scrubbed_to: None,
            menu_target: None,
            hovered_clip: None,
            hovered_gap: None,
            snap_line: None,
            name_hover: None,
            named_clip: None,
            menu_flyouts: MenuFlyouts::default(),
            trim_grab: Ticks(0),
            trim_aligned: None,
            playhead_bubble: false,
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

    /// Makes a share of the view show the whole project, from its start.
    fn show_project(&mut self, total: Ticks, share: f64) {
        let wanted = if total.0 > 0 {
            (total.0 as f64 / share) as i64
        } else {
            Self::extent(total).0
        };
        self.visible = Ticks(wanted.max(Ticks::from_seconds(MIN_VISIBLE_SECONDS).0));
        self.scroll = Ticks(0);
    }

    /// Makes the whole project fill the width of the view.
    pub fn fit(&mut self, total: Ticks) {
        self.show_project(total, 1.0);
        self.auto_fit = true;
    }

    /// Keeps a new clip in view, unless the user has chosen their own zoom. It leaves room
    /// after the last clip to drop the next one.
    pub fn content_added(&mut self, total: Ticks) {
        if self.auto_fit {
            self.show_project(total, AUTO_FIT_FRACTION);
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

    /// Keeps clips the same size on screen when the timeline gets wider or narrower, so
    /// it shows more or less time rather than stretching what is there.
    fn keep_zoom_for_width(&mut self, old_width: f32, new_width: f32, total: Ticks) {
        let ratio = f64::from(new_width) / f64::from(old_width);
        let closest = Ticks::from_seconds(MIN_VISIBLE_SECONDS).0;
        self.visible = Ticks(((self.visible_for(total).0 as f64 * ratio) as i64).max(closest));
        self.clamp_scroll(total);
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
    /// A clip's speaker was pressed.
    pub toggle_clip_mute: Option<ClipId>,
    /// The lane for adding audio was pressed.
    pub add_audio: bool,
    pub seek: Option<Ticks>,
    pub select: Option<Option<ClipId>>,
    /// Comes with `select`: the dead space that was clicked on, if any.
    pub select_gap: Option<Ticks>,
    pub remove_gap: Option<Ticks>,
    /// A trim began, with the pointer's time when it did.
    pub trim_started: Option<(ClipId, ClipEdge, Ticks)>,
    pub trim_to: Option<(ClipId, ClipEdge, Ticks)>,
    /// Comes with `trim_to`: the moment the edge snapped to, if it did.
    pub trim_snap: Option<Ticks>,
    pub trim_finished: bool,
    pub move_clip: Option<(ClipId, Ticks, TrackTarget)>,
    pub drop_media: Option<(MediaId, Ticks, TrackTarget)>,
    pub menu: Option<MenuAction>,
    /// The clip's menu changed its speed.
    pub speed: Option<(ClipId, Speed)>,
    pub collapse: bool,
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

/// Whether the pointer is on the playhead: its head at the top, or the line below it.
fn on_playhead(pointer: Pos2, x: f32, top: f32, bottom: f32) -> bool {
    let reach = if pointer.y <= top + PLAYHEAD_HEAD_HEIGHT {
        PLAYHEAD_HEAD_REACH
    } else {
        PLAYHEAD_LINE_REACH
    };
    (top..=bottom).contains(&pointer.y) && (pointer.x - x).abs() <= reach
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

/// The text of a small label on the ruler, and the size of the label around it.
fn pill_text(painter: &egui::Painter, palette: &Palette, text: String) -> (Arc<Galley>, Vec2) {
    let galley = painter.layout_no_wrap(text, FontId::proportional(12.0), palette.surface);
    let size = galley.size() + vec2(12.0, 5.0);
    (galley, size)
}

fn paint_pill(painter: &egui::Painter, palette: &Palette, rect: Rect, galley: Arc<Galley>) {
    painter.rect_filled(rect, 4.0, palette.text);
    painter.galley(rect.min + vec2(6.0, 2.5), galley, palette.surface);
}

/// A small label on the ruler, in the colours that stand out most from it.
fn draw_pill(painter: &egui::Painter, palette: &Palette, area: Rect, x: f32, text: String) {
    let (galley, size) = pill_text(painter, palette, text);
    let left = (x - size.x / 2.0)
        .min(area.right() - size.x)
        .max(area.left());
    let rect = Rect::from_min_size(pos2(left, area.center().y - size.y / 2.0), size);
    paint_pill(painter, palette, rect, galley);
}

/// A label's fill, words and outline on the ruler. Most are quiet, sitting back on the tinted
/// stretch of the selected clip. The end being dragged is lit, so it is the one the eye follows.
fn ruler_tag_colours(palette: &Palette, lit: bool) -> (Color32, Color32, Stroke) {
    if lit {
        (palette.accent, palette.accent_text, Stroke::NONE)
    } else {
        (
            palette.surface,
            palette.text,
            Stroke::new(1.0, palette.accent.gamma_multiply(RULER_TAG_EDGE)),
        )
    }
}

/// How large a label on the ruler is, and its words ready to draw.
fn ruler_tag(
    painter: &egui::Painter,
    palette: &Palette,
    text: String,
    lit: bool,
) -> (Arc<Galley>, Vec2) {
    let (_, words, _) = ruler_tag_colours(palette, lit);
    let galley = painter.layout_no_wrap(text, FontId::proportional(12.0), words);
    let size = galley.size() + vec2(10.0, 3.0);
    (galley, size)
}

fn paint_ruler_tag(
    painter: &egui::Painter,
    palette: &Palette,
    rect: Rect,
    galley: Arc<Galley>,
    lit: bool,
) {
    let (fill, words, edge) = ruler_tag_colours(palette, lit);
    painter.rect_filled(rect, 4.0, fill);
    painter.rect_stroke(rect, 4.0, edge, egui::StrokeKind::Inside);
    painter.galley(rect.min + vec2(5.0, 1.5), galley, words);
}

/// Where a trim's start and end times go: inside the clip's stretch of the ruler, one at each end,
/// or just outside it when it is too short to hold both. Either way they stay on the ruler.
fn trim_tag_rects(area: Rect, start_x: f32, end_x: f32, start: Vec2, end: Vec2) -> (Rect, Rect) {
    let fits = end_x - start_x >= start.x + end.x + TRIM_TAG_GAP;
    let (start_left, end_left) = if fits {
        (start_x, end_x - end.x)
    } else {
        (start_x - start.x - TRIM_TAG_GAP, end_x + TRIM_TAG_GAP)
    };
    let place = |left: f32, size: Vec2| {
        let left = left.min(area.right() - size.x).max(area.left());
        Rect::from_min_size(pos2(left, area.center().y - size.y / 2.0), size)
    };
    (place(start_left, start), place(end_left, end))
}

/// Where the time under the pointer shows, and what it says. It only runs along the project, so
/// past the last clip it waits at the end. An empty timeline has none, and neither has a project
/// scrolled right out of the view.
fn hover_mark(scale: &TimeScale, total: Ticks, view: Rect, pointer_x: f32) -> Option<(f32, Ticks)> {
    if total.0 <= 0 {
        return None;
    }
    let (start, end) = (scale.x_of(Ticks(0)), scale.x_of(total));
    let (x, time) = if pointer_x >= end {
        (end, total)
    } else if pointer_x <= start {
        (start, Ticks(0))
    } else {
        (pointer_x, scale.time_at(pointer_x))
    };
    (x >= view.left() - 1.0 && x <= view.right() + 1.0).then_some((x, time))
}

/// How a clip is showing what can be done with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClipLook {
    Rest,
    Hovered,
    Selected,
}

fn clip_look(selected: bool, hovered: bool) -> ClipLook {
    if selected {
        ClipLook::Selected
    } else if hovered {
        ClipLook::Hovered
    } else {
        ClipLook::Rest
    }
}

/// The two handles, which follow the clip's cut corners.
fn handle_shapes(rect: Rect) -> [Vec<Pos2>; 2] {
    let width = HANDLE_WIDTH.min(rect.width() / 3.0);
    let cut = CORNER_CUT_X.min(rect.width() / 2.0).min(width);
    [
        vec![
            pos2(rect.left() + cut, rect.top()),
            pos2(rect.left() + width, rect.top()),
            pos2(rect.left() + width, rect.bottom()),
            pos2(rect.left(), rect.bottom()),
            pos2(rect.left(), rect.top() + CORNER_CUT_Y),
        ],
        vec![
            pos2(rect.right() - width, rect.top()),
            pos2(rect.right(), rect.top()),
            pos2(rect.right(), rect.bottom() - CORNER_CUT_Y),
            pos2(rect.right() - cut, rect.bottom()),
            pos2(rect.right() - width, rect.bottom()),
        ],
    ]
}

/// Draws what a clip's video looks and sounds like across the clip. Both are read in the
/// background, so either can still be missing.
fn draw_face(
    painter: &egui::Painter,
    palette: &Palette,
    rect: Rect,
    clip: &ClipView,
    face: &ClipFace,
    scale: &TimeScale,
) {
    let visible = rect.intersect(painter.clip_rect());
    if !visible.is_positive() {
        return;
    }
    let shown = painter.with_clip_rect(visible);
    if let Some((_, first)) = face.pictures.first().filter(|_| !clip.sound_only) {
        let size = first.size_vec2();
        let tile = (rect.height() * size.x / size.y).max(1.0);
        let whole = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        let mut x = rect.left() + ((visible.left() - rect.left()) / tile).floor() * tile;
        while x < visible.right() {
            let time = source_time_at(clip, scale, x + tile / 2.0);
            let (_, picture) = &face.pictures[picture_index_at(face.pictures, time)];
            let place = Rect::from_min_size(pos2(x, rect.top()), vec2(tile, rect.height()));
            shown.image(picture.id(), place, whole, Color32::WHITE);
            x += tile;
        }
        // A picture is square-cornered, so the track shows through where the clip is cut.
        let cut = CORNER_CUT_X.min(rect.width() / 2.0);
        for corner in [
            [
                rect.left_top(),
                pos2(rect.left() + cut, rect.top()),
                pos2(rect.left(), rect.top() + CORNER_CUT_Y),
            ],
            [
                rect.right_bottom(),
                pos2(rect.right() - cut, rect.bottom()),
                pos2(rect.right(), rect.bottom() - CORNER_CUT_Y),
            ],
        ] {
            shown.add(Shape::convex_polygon(
                corner.to_vec(),
                palette.bg_app,
                Stroke::NONE,
            ));
        }
    }
    let Some(sound) = face
        .sound
        .filter(|sound| clip.shows_waveform && !sound.peaks.is_empty())
    else {
        return;
    };
    let share = if clip.sound_only {
        SOUND_CLIP_BAND_SHARE
    } else {
        SOUND_BAND_SHARE
    };
    let band_height = rect.height() * share;
    let band = rect.with_min_y(rect.bottom() - band_height);
    shown.rect_filled(band, 0.0, palette.stage.gamma_multiply(SOUND_BAND_BACKING));
    let shape_colour = if clip.muted {
        palette.clip_video_text.gamma_multiply(MUTED_SOUND_SHAPE)
    } else {
        palette.clip_video_text
    };
    let mut mesh = Mesh::default();
    let mut x = visible.left();
    while x < visible.right() {
        let start = source_time_at(clip, scale, x);
        let end = source_time_at(clip, scale, x + SOUND_COLUMN_WIDTH);
        let height = sound.peak_between(start, end) * band_height;
        if height > 0.5 {
            mesh.add_colored_rect(
                Rect::from_min_max(
                    pos2(x, band.bottom() - height),
                    pos2(x + SOUND_COLUMN_WIDTH - 0.5, band.bottom()),
                ),
                shape_colour,
            );
        }
        x += SOUND_COLUMN_WIDTH;
    }
    shown.add(Shape::mesh(mesh));
}

/// Shades what a fade takes away, so its length and direction can be seen on the clip.
fn draw_fades(
    painter: &egui::Painter,
    palette: &Palette,
    rect: Rect,
    clip: &ClipView,
    scale: &TimeScale,
) {
    let shade = palette.stage.gamma_multiply(FADE_SHADE);
    let one_pixel = scale.time_of_pixels(1.0).0.max(1) as f32;
    let fade_in = clip.fade_in.0 as f32 / one_pixel;
    let fade_out = clip.fade_out.0 as f32 / one_pixel;
    let clipped = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    if fade_in >= 1.0 {
        clipped.add(Shape::convex_polygon(
            vec![
                rect.left_top(),
                pos2(rect.left() + fade_in, rect.top()),
                rect.left_bottom(),
            ],
            shade,
            Stroke::NONE,
        ));
    }
    if fade_out >= 1.0 {
        clipped.add(Shape::convex_polygon(
            vec![
                pos2(rect.right() - fade_out, rect.top()),
                rect.right_top(),
                rect.right_bottom(),
            ],
            shade,
            Stroke::NONE,
        ));
    }
}

fn draw_clip(
    painter: &egui::Painter,
    palette: &Palette,
    rect: Rect,
    clip: &ClipView,
    (face, scale): (Option<&ClipFace>, &TimeScale),
    look: ClipLook,
) {
    let fill = if clip.sound_only {
        palette.clip_audio
    } else {
        palette.clip_video
    };
    painter.add(clip_shape(rect, fill, Stroke::NONE));
    if let Some(face) = face {
        draw_face(painter, palette, rect, clip, face, scale);
    }
    draw_fades(painter, palette, rect, clip, scale);

    // Handles only appear on a clip being pointed at or worked on, to keep the rest calm.
    let handle_colors = match look {
        ClipLook::Rest => None,
        ClipLook::Hovered => Some((palette.raised, palette.text)),
        ClipLook::Selected => Some((palette.accent, palette.accent_text)),
    };
    if let Some((fill, grip)) = handle_colors {
        let width = HANDLE_WIDTH.min(rect.width() / 3.0);
        let height = (rect.height() * 0.45).max(4.0);
        let middles = [rect.left() + width / 2.0, rect.right() - width / 2.0];
        for (shape, middle) in handle_shapes(rect).into_iter().zip(middles) {
            painter.add(Shape::convex_polygon(shape, fill, Stroke::NONE));
            painter.rect_filled(
                Rect::from_center_size(
                    pos2(middle, rect.center().y),
                    vec2(HANDLE_GRIP_WIDTH.min(width), height),
                ),
                1.5,
                grip,
            );
        }
    }
    let outline = match look {
        ClipLook::Rest => Stroke::NONE,
        ClipLook::Hovered => Stroke::new(HOVERED_OUTLINE, palette.text),
        // The accent is too close to the clip's own colour to stand out as an outline.
        ClipLook::Selected => Stroke::new(SELECTED_OUTLINE, palette.text),
    };
    painter.add(clip_shape(rect, Color32::TRANSPARENT, outline));

    // A long clip can start off to the left, so these stay with the part on show.
    let shown = rect.intersect(painter.clip_rect());
    draw_speed_tag(painter, palette, shown, clip);
    if keeps_its_name(clip) && shown.width() > KEPT_NAME_MIN_WIDTH {
        draw_clip_name(painter, palette, shown, shown.right() - HANDLE_WIDTH, clip);
    }
}

/// A video can be told by its pictures, so its name waits for the pointer. Sound and images have
/// less to go on, so theirs stays.
fn keeps_its_name(clip: &ClipView) -> bool {
    clip.sound_only || clip.is_still
}

/// What a clip says about its speed, which stays on show because nothing else gives it away.
fn speed_tag_text(clip: &ClipView) -> Option<String> {
    (!clip.speed.is_normal()).then(|| text::speed_name(clip.speed.as_factor()))
}

/// The speed tag's shape and text, at the start of the part of the clip on show.
fn speed_tag(
    painter: &egui::Painter,
    palette: &Palette,
    shown: Rect,
    clip: &ClipView,
) -> Option<(Rect, Arc<Galley>)> {
    let name = speed_tag_text(clip).filter(|_| shown.width() >= SPEED_TAG_MIN_WIDTH)?;
    let galley = painter.layout_no_wrap(name, FontId::proportional(11.0), palette.clip_video_text);
    let tag = Rect::from_min_size(
        pos2(
            shown.left() + HANDLE_WIDTH + 4.0,
            shown.top() + CLIP_LABEL_TOP,
        ),
        vec2(galley.size().x + 10.0, CLIP_LABEL_HEIGHT),
    );
    Some((tag, galley))
}

fn draw_speed_tag(painter: &egui::Painter, palette: &Palette, shown: Rect, clip: &ClipView) {
    if let Some((tag, galley)) = speed_tag(painter, palette, shown, clip) {
        let backing = palette.stage.gamma_multiply(CLIP_LABEL_BACKING);
        painter.rect_filled(tag, 4.0, backing);
        let top = tag.center().y - galley.size().y / 2.0;
        painter.galley(pos2(tag.left() + 5.0, top), galley, palette.clip_video_text);
    }
}

/// A clip's name on its own small backing, because it can be sitting on any picture. It starts
/// after any speed tag and is cut short at `right`.
fn draw_clip_name(
    painter: &egui::Painter,
    palette: &Palette,
    shown: Rect,
    right: f32,
    clip: &ClipView,
) {
    let after_tag = speed_tag(painter, palette, shown, clip).map(|(tag, _)| tag.right() + 4.0);
    let left = after_tag.unwrap_or(shown.left() + HANDLE_WIDTH + 4.0);
    let galley = painter.layout_no_wrap(
        clip.label.clone(),
        FontId::proportional(12.0),
        palette.clip_video_text,
    );
    let room = right - left;
    let has_icon = room > CLIP_ICON_MIN_WIDTH;
    let icon_room = if has_icon { CLIP_ICON_SIZE + 4.0 } else { 0.0 };
    let width = (galley.size().x + icon_room + 12.0).min(room);
    if width <= 0.0 {
        return;
    }
    let label = Rect::from_min_size(
        pos2(left, shown.top() + CLIP_LABEL_TOP),
        vec2(width, CLIP_LABEL_HEIGHT),
    );
    let backing = palette.stage.gamma_multiply(CLIP_LABEL_BACKING);
    let clipped = painter.with_clip_rect(label.intersect(painter.clip_rect()));
    clipped.rect_filled(label, 4.0, backing);
    let mut text_left = label.left() + 6.0;
    if has_icon {
        icons::paint(
            &clipped,
            icons::square_at(
                pos2(text_left + CLIP_ICON_SIZE / 2.0, label.center().y),
                CLIP_ICON_SIZE,
            ),
            if clip.sound_only {
                &icons::MUSIC
            } else if clip.is_still {
                &icons::IMAGE
            } else {
                &icons::FILM
            },
            palette.clip_video_text,
        );
        text_left += icon_room;
    }
    let top = label.center().y - galley.size().y / 2.0;
    clipped.galley(pos2(text_left, top), galley, palette.clip_video_text);
}

/// Keeps track of how long the pointer has rested on a clip, and shows the name when it has
/// rested for long enough. A wait asks for one more frame at the moment it ends, so nothing
/// repaints in between and an idle window stays idle.
fn follow_name_hover(ui: &egui::Ui, state: &mut TimelineState) {
    let now = ui.input(|input| input.time);
    state.name_hover = state.hovered_clip.map(|id| match state.name_hover {
        Some((was, since)) if was == id => (id, since),
        _ => (id, now),
    });
    state.named_clip = state.name_hover.and_then(|(id, since)| {
        let left = NAME_HOVER_SECONDS - (now - since);
        if left > 0.0 {
            ui.ctx().request_repaint_after_secs(left as f32);
            None
        } else {
            Some(id)
        }
    });
}

/// The names the pointer has brought up, over every clip so that a narrow clip's can spill past
/// it. Each fades in and out, and rises a little as it arrives.
fn draw_hovered_names(
    ui: &egui::Ui,
    painter: &egui::Painter,
    palette: &Palette,
    state: &TimelineState,
    clips: &[ClipView],
    (layout, scale): (&TrackLayout, &TimeScale),
) {
    for clip in clips {
        let opacity = ui.ctx().animate_bool_with_time(
            ui.scope_id().with(("clip-name", clip.id.0)),
            state.named_clip == Some(clip.id),
            NAME_FADE_SECONDS,
        );
        if opacity <= 0.0 {
            continue;
        }
        let shape = clip_rect(scale, layout.track(clip.track), clip.start, clip.end);
        let shown = shape.intersect(layout.area);
        if !shown.is_positive() {
            continue;
        }
        let mut fading = painter.clone();
        fading.multiply_opacity(opacity);
        let risen = shown.translate(vec2(0.0, (1.0 - opacity) * NAME_RISE));
        draw_clip_name(&fading, palette, risen, layout.area.right(), clip);
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

fn dashed_outline(painter: &egui::Painter, rect: Rect, stroke: Stroke) {
    let corners = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
        rect.left_top(),
    ];
    painter.extend(Shape::dashed_line(&corners, stroke, 6.0, 5.0));
}

/// A dashed lane that says a clip can be dropped here to make a new track. It only ever
/// offers what works, so it names text and audio once those tracks exist. These only show on a
/// single track, as [`TrackLayout::has_lanes`] says.
fn draw_new_track_lane(painter: &egui::Painter, palette: &Palette, lane: Rect, hint: &str) {
    dashed_outline(painter, lane, Stroke::new(1.0, palette.border_strong));
    let left = lane.left() + 10.0;
    let middle = lane.center().y;
    icons::paint(
        painter,
        icons::square_at(pos2(left + 6.0, middle), 12.0),
        &icons::PLUS,
        palette.text_muted,
    );
    painter.text(
        pos2(left + 18.0, middle),
        Align2::LEFT_CENTER,
        hint,
        FontId::proportional(12.0),
        palette.text_muted,
    );
}

/// Shows where a drag will land: on a track, or on a new one with a lane or line lit up.
fn draw_drop(
    painter: &egui::Painter,
    palette: &Palette,
    layout: &TrackLayout,
    scale: &TimeScale,
    target: TrackTarget,
    (start, end): (Ticks, Ticks),
) {
    let light_up = |lane: Rect| {
        painter.rect_filled(lane, 3.0, palette.accent.gamma_multiply(PROJECT_TINT));
        dashed_outline(painter, lane, Stroke::new(1.5, palette.accent));
        lane
    };
    // A lane with no room left to show, because the tracks fill the view, gets a line instead.
    let fits = |lane: Rect| layout.area.contains_rect(lane);
    let row = match target {
        TrackTarget::Existing(index) => layout.track(index),
        TrackTarget::New(_) if layout.count == 0 => layout.area,
        TrackTarget::New(0) if fits(layout.new_lane_above()) => light_up(layout.new_lane_above()),
        TrackTarget::New(index) if index >= layout.count && fits(layout.new_lane_below()) => {
            light_up(layout.new_lane_below())
        }
        TrackTarget::New(index) => {
            let middle = pos2(
                layout.area.center().x,
                layout.track(index).top() - TRACK_GAP / 2.0,
            );
            let width = layout.area.width();
            painter.rect_filled(
                Rect::from_center_size(middle, vec2(width, INSERT_LINE_WIDTH)),
                1.5,
                palette.accent,
            );
            Rect::from_center_size(middle, vec2(width, TRACK_HEIGHT * 0.6))
        }
    };
    draw_landing(painter, palette, clip_rect(scale, row, start, end));
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

fn zoom_button(
    ui: &mut egui::Ui,
    palette: &Palette,
    rect: Rect,
    icon: Icon,
    label: &str,
    hint: &str,
) -> egui::Response {
    IconButton::new(palette, Kind::Ghost, icon, label)
        .show_at(ui, egui::Id::unique(("timeline-zoom", label)), rect)
        .on_hover_text(hint)
}

/// A line of the clip's menu that opens more beside it, on being pointed at or pressed. Returns
/// where the part beside it is on screen, while it is open.
fn flyout_item(
    ui: &mut egui::Ui,
    (palette, icon, name): (&Palette, Icon, &str),
    which: MenuFlyout,
    flyouts: &mut MenuFlyouts,
    content: impl FnOnce(&mut egui::Ui),
) -> Option<Rect> {
    let item = ui.add(MenuItem::opens_more(palette, icon, name));
    if (item.hovered() && !flyouts.heading_there) || item.clicked() {
        flyouts.open = Some(which);
    }
    let shown = Popup::from_response(&item)
        .kind(PopupKind::Menu)
        .open(flyouts.open == Some(which))
        .align(RectAlign::RIGHT_START)
        .layout(Layout::top_down_justified(Align::Min))
        .style(egui::containers::menu::menu_style)
        .gap(FLYOUT_GAP)
        // The menu it belongs to looks after what a click does, because it can see both.
        .close_behavior(PopupCloseBehavior::IgnoreClicks)
        .show(content)?;
    Some(shown.response.rect)
}

/// Speed and sound for the clip the menu is on, which only offers what the clip can do: an image
/// has no speed, and a clip with no sound has none to change.
fn clip_flyouts(
    ui: &mut egui::Ui,
    palette: &Palette,
    clip: &ClipView,
    flyouts: &mut MenuFlyouts,
    events: &mut (Option<(ClipId, Speed)>, Option<MenuAction>),
) -> Option<Rect> {
    let mut beside = None;
    if !clip.is_still {
        let selected = stage::SelectedSpeed {
            id: clip.id,
            speed: clip.speed,
            has_sound: clip.has_sound,
        };
        beside = beside.or(flyout_item(
            ui,
            (palette, icons::GAUGE, "Speed"),
            MenuFlyout::Speed,
            flyouts,
            |ui| stage::speed_menu(ui, palette, &selected, &mut events.0),
        ));
    }
    if clip.has_sound {
        beside = beside.or(flyout_item(
            ui,
            (palette, icons::VOLUME_2, "Audio"),
            MenuFlyout::Audio,
            flyouts,
            |ui| {
                let (icon, name) = if clip.muted {
                    (icons::VOLUME_2, "Unmute")
                } else {
                    (icons::VOLUME_X, "Mute")
                };
                if ui.add(MenuItem::new(palette, icon, name, "")).clicked() {
                    events.1 = Some(MenuAction::ToggleMute(clip.id));
                }
                if !clip.sound_only
                    && !clip.muted
                    && ui
                        .add(MenuItem::new(palette, icons::UNLINK, "Detach sound", ""))
                        .on_hover_text("Puts this clip's sound on rows of its own, so it can be moved and changed by itself")
                        .clicked()
                {
                    events.1 = Some(MenuAction::Detach(clip.id));
                }
            },
        ));
    }
    beside
}

/// The menu that opens on a clip or a gap, reporting what was picked from it. Also returns where
/// any part open beside it is on screen.
fn timeline_menu(
    ui: &mut egui::Ui,
    palette: &Palette,
    view: &TimelineView,
    target: MenuTarget,
    (flyouts, speed): (&mut MenuFlyouts, &mut Option<(ClipId, Speed)>),
) -> (Option<MenuAction>, Option<Rect>) {
    let mut action = None;
    let mut beside = None;
    flyouts.heading_there = flyouts
        .rect
        .is_some_and(|rect| ui.input(|input| input.pointer.is_moving_towards_rect(&rect)));
    if flyouts.heading_there {
        // Stopping counts as no longer heading there, and stopping sends no event of its own.
        ui.ctx().request_repaint();
    }
    // Moving on to another line closes what is open beside the menu. A drag on a slider there
    // counts as no line being pointed at, wherever it wanders.
    let plain_line = |response: egui::Response, flyouts: &mut MenuFlyouts| {
        if response.hovered() && !flyouts.heading_there {
            flyouts.open = None;
        }
        response.clicked()
    };
    match target {
        MenuTarget::Clip(id) => {
            let under_playhead = view
                .tracks
                .clip(id)
                .is_some_and(|clip| view.position >= clip.start && view.position < clip.end());
            let split = ui
                .add_enabled(
                    under_playhead,
                    MenuItem::new(palette, icons::SCISSORS, "Split", "S"),
                )
                .on_disabled_hover_text("Move the playhead inside this clip to split it");
            if plain_line(split, flyouts) {
                action = Some(MenuAction::Split);
            }
            let duplicate = ui.add(MenuItem::new(
                palette,
                icons::COPY_PLUS,
                "Duplicate",
                "Ctrl+D",
            ));
            if plain_line(duplicate, flyouts) {
                action = Some(MenuAction::Duplicate(id));
            }
            let delete = ui.add(MenuItem::new(palette, icons::TRASH_2, "Delete", "Del"));
            if plain_line(delete, flyouts) {
                action = Some(MenuAction::Delete(id));
            }
            if let Some(clip) = view.clips.iter().find(|clip| clip.id == id) {
                let mut picked = (None, None);
                beside = clip_flyouts(ui, palette, clip, flyouts, &mut picked);
                *speed = picked.0;
                action = action.or(picked.1);
            }
        }
        MenuTarget::Gap(time) => {
            let remove = ui.add(MenuItem::new(palette, icons::X, "Remove gap", "Del"));
            if plain_line(remove, flyouts) {
                action = Some(MenuAction::RemoveGap(time));
            }
        }
    }
    ui.separator();
    let has_gaps = !view.tracks.gaps().is_empty();
    let remove_all = ui.add_enabled(
        has_gaps,
        MenuItem::new(palette, icons::X, "Remove all gaps", ""),
    );
    if plain_line(remove_all, flyouts) {
        action = Some(MenuAction::RemoveAllGaps);
    }
    flyouts.rect = beside;
    (action, beside)
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

/// The bar that moves through tracks that don't all fit.
fn track_scrollbar(
    ui: &mut egui::Ui,
    palette: &Palette,
    state: &mut TimelineState,
    area: Rect,
    furthest: f32,
) {
    let bar = Rect::from_min_max(
        pos2(area.right() - TRACK_SCROLLBAR_WIDTH, area.top()),
        area.right_bottom(),
    );
    let response = ui.interact(
        bar,
        egui::Id::unique("timeline-track-scrollbar"),
        Sense::click_and_drag(),
    );
    let content = area.height() + furthest;
    let thumb = (area.height() / content * area.height()).max(MIN_THUMB_WIDTH);
    let travel = (area.height() - thumb).max(1.0);
    if response.dragged() {
        state.track_scroll =
            (state.track_scroll + response.drag_delta().y / travel * furthest).clamp(0.0, furthest);
    }
    let top = area.top() + state.track_scroll / furthest * travel;
    let fill = if response.dragged() || response.hovered() {
        palette.text_muted
    } else {
        palette.border_strong
    };
    ui.painter().rect_filled(
        Rect::from_min_size(pos2(bar.left(), top), vec2(TRACK_SCROLLBAR_WIDTH, thumb)),
        3.0,
        fill,
    );
}

/// Draws the row under the tracks, and says if the collapse button was pressed.
fn bottom_strip(
    ui: &mut egui::Ui,
    palette: &Palette,
    state: &mut TimelineState,
    strip: Rect,
    left: f32,
    view: &TimelineView,
) -> bool {
    let total = state.frozen_total.unwrap_or_else(|| view.tracks.duration());
    let size = vec2(ZOOM_BUTTON_SIZE, ZOOM_BUTTON_SIZE);
    let right = strip.right() - 8.0;
    let middle = strip.center().y;
    let button_at = |index: f32| {
        Rect::from_center_size(
            pos2(right - (index + 0.5) * (ZOOM_BUTTON_SIZE + 2.0), middle),
            size,
        )
    };
    let [out_rect, in_rect, fit_rect, collapse_rect] = [
        button_at(3.0),
        button_at(2.0),
        button_at(1.0),
        button_at(0.0),
    ];
    state.zoom_buttons = [out_rect, in_rect, fit_rect];

    // The card has its own row, so it never sits on top of a clip.
    ui.painter().rect(
        out_rect.union(collapse_rect).expand(2.0),
        ZOOM_CARD_RADIUS,
        palette.raised,
        Stroke::new(1.0, palette.border),
        egui::StrokeKind::Inside,
    );

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
        icons::ZOOM_OUT,
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
        icons::ZOOM_IN,
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
        icons::MOVE_HORIZONTAL,
        "Fit",
        "Fit the whole video to the width of the timeline",
    )
    .clicked()
    {
        state.fit(total);
    }
    let collapse = zoom_button(
        ui,
        palette,
        collapse_rect,
        icons::PANEL_BOTTOM_CLOSE,
        "Collapse timeline",
        "Collapse timeline (Ctrl+Alt+3)",
    )
    .clicked();

    let bar = Rect::from_min_max(
        pos2(left, middle - SCROLLBAR_HEIGHT / 2.0),
        pos2(out_rect.left() - 12.0, middle + SCROLLBAR_HEIGHT / 2.0),
    );
    // The whole timeline in view leaves nowhere to scroll to.
    if state.visible_for(total) < TimelineState::extent(total) {
        scrollbar(ui, palette, state, bar, total);
    } else {
        state.scrollbar = Rect::NOTHING;
    }
    collapse
}

/// The speaker on each clip with sound, which mutes it.
fn speakers(
    ui: &mut egui::Ui,
    palette: &Palette,
    state: &TimelineState,
    clips: &[ClipView],
    layout: &TrackLayout,
    scale: &TimeScale,
    events: &mut TimelineEvents,
) {
    if state.drag != TimelineDrag::None {
        return;
    }
    for clip in clips.iter().filter(|clip| clip.has_sound) {
        let shape = clip_rect(scale, layout.track(clip.track), clip.start, clip.end);
        let visible = shape.intersect(layout.area);
        if visible.width() < SPEAKER_MIN_WIDTH || visible.height() < shape.height() {
            continue;
        }
        let place = Rect::from_center_size(
            pos2(
                visible.left() + HANDLE_WIDTH + 4.0 + SPEAKER_SIZE / 2.0,
                shape.bottom() - SPEAKER_SIZE / 2.0 - 3.0,
            ),
            vec2(SPEAKER_SIZE, SPEAKER_SIZE),
        );
        // On its own backing, because it can be sitting on any picture.
        ui.painter()
            .rect_filled(place, 4.0, palette.stage.gamma_multiply(CLIP_LABEL_BACKING));
        let (icon, name) = if clip.muted {
            (icons::VOLUME_X, "Unmute clip")
        } else {
            (icons::VOLUME_2, "Mute clip")
        };
        let button = IconButton::new(palette, Kind::Ghost, icon, name)
            .size(SPEAKER_SIZE, SPEAKER_ICON)
            .show_at(ui, ui.scope_id().with(("clip-speaker", clip.id.0)), place)
            .on_hover_text(name);
        if button.clicked() {
            events.toggle_clip_mute = Some(clip.id);
        }
    }
}

/// Draws the timeline and reports what the user did with it.
pub fn timeline(
    ui: &mut egui::Ui,
    palette: &Palette,
    state: &mut TimelineState,
    view: &TimelineView,
) -> TimelineEvents {
    let total = view.tracks.duration();
    let clips = view.clips;
    let width = ui.available_width();
    // The gap between the two rows counts too, or the panel would creep taller each frame.
    let strip_room = STRIP_HEIGHT + ui.spacing().item_spacing.y;
    let height = (ui.available_height() - strip_room).max(70.0);
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let mut events = TimelineEvents::default();
    let inset = 8.0;
    let left = rect.left() + TRACKS_LEFT;
    let track_width = (rect.width() - TRACKS_LEFT - inset).max(1.0);

    let view_total = state.frozen_total.unwrap_or(total);
    if state.scale.width > 0.0 && (state.scale.width - track_width).abs() > 0.01 {
        state.keep_zoom_for_width(state.scale.width, track_width, view_total);
    }
    state.clamp_scroll(view_total);
    state.snap_line = None;

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
    let area = Rect::from_min_max(
        pos2(scale.left, ruler.bottom() + 4.0),
        pos2(scale.left + scale.width, rect.bottom() - 6.0),
    );
    let track_count = view.tracks.tracks().len();
    let furthest = TrackLayout::furthest_scroll(track_count, area.height());
    state.track_scroll = state.track_scroll.clamp(0.0, furthest);
    let layout = TrackLayout {
        area,
        count: track_count,
        scroll: state.track_scroll,
    };
    state.layout = layout;
    state.ruler = ruler;
    state.track = if track_count == 0 {
        area
    } else {
        layout.track(0)
    };
    state.scale = scale;

    let pointer_over = response
        .contains_pointer()
        .then(|| ui.input(|input| input.pointer.latest_pos()))
        .flatten();
    let hover_time = pointer_over.map(|pointer| Ticks(scale.time_at(pointer.x).0.max(0)));
    state.hover_time = hover_time;

    // Wheel over the timeline: Ctrl zooms. Otherwise a wheel moves through tracks that don't
    // all fit, or else along the timeline. Sideways scrolling is always along the timeline.
    if let (Some(hover), Some(time)) = (pointer_over, hover_time) {
        let (across, down, zoom_key) = ui.input(|input| {
            let scroll = input.smooth_scroll_delta;
            (scroll.x, scroll.y, input.modifiers.command)
        });
        if (across != 0.0 || down != 0.0) && rect.contains(hover) {
            if zoom_key {
                state.zoom_by(f64::from((across + down) * 0.005).exp(), time, view_total);
            } else {
                let through_tracks = furthest > 0.0 && area.contains(hover);
                let along = if through_tracks {
                    state.track_scroll = (state.track_scroll - down).clamp(0.0, furthest);
                    across
                } else {
                    across + down
                };
                let shift = f64::from(-along) / f64::from(scale.width) * scale.visible.0 as f64;
                state.scroll = Ticks(state.scroll.0 + shift as i64);
                state.clamp_scroll(view_total);
            }
        }
    }

    let hovered_part = pointer_over
        .filter(|hover: &Pos2| area.contains(*hover))
        .and_then(|hover| clip_at(clips, &layout, &scale, hover));
    let hovered_gap = pointer_over
        .filter(|hover: &Pos2| {
            state.drag == TimelineDrag::None && (area.contains(*hover) || ruler.contains(*hover))
        })
        .and_then(|hover| view.tracks.gap_at(scale.time_at(hover.x)));
    state.hovered_gap = hovered_gap;
    state.hovered_clip = hovered_part
        .filter(|_| state.drag == TimelineDrag::None)
        .map(|(clip, _)| clip.id);
    follow_name_hover(ui, state);
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
            .filter(|pointer| area.contains(*pointer))
            .and_then(|pointer| clip_at(clips, &layout, &scale, pointer));
        let time = pointer.map_or(Ticks(0), |pointer| scale.time_at(pointer.x));
        state.drag = match part {
            Some((clip, ClipPart::Handle(edge))) => {
                let edge_time = match edge {
                    ClipEdge::Start => clip.start,
                    ClipEdge::End => clip.end,
                };
                state.trim_grab = Ticks(time.0 - edge_time.0);
                TimelineDrag::Trim(clip.id, edge)
            }
            Some((clip, ClipPart::Body)) => TimelineDrag::Move {
                id: clip.id,
                grab: Ticks(time.0 - clip.start.0),
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
                let targets = snap_targets(view, Some(id));
                let wanted = Ticks(time.0 - state.trim_grab.0);
                let snapped = snap_start(wanted, Ticks(0), &targets, tolerance);
                events.trim_to = Some((id, edge, Ticks(snapped.0 + state.trim_grab.0)));
                events.trim_snap = targets.contains(&snapped).then_some(snapped);
            }
            TimelineDrag::Scrub if response.dragged() => {
                // Only when it moves, because seeking again would stop the exact frame from
                // ever being reached while the pointer rests.
                let target = Ticks(time.0.clamp(0, total.0));
                if state.scrubbed_to != Some(target) {
                    state.scrubbed_to = Some(target);
                    events.seek = Some(target);
                }
            }
            TimelineDrag::Move { id, grab } => {
                if let Some(clip) = clips.iter().find(|clip| clip.id == id) {
                    let length = Ticks(clip.end.0 - clip.start.0);
                    let wanted = Ticks((time.0 - grab.0).max(0));
                    let targets = snap_targets(view, Some(id));
                    let snapped = snap_start(wanted, length, &targets, tolerance);
                    let target = view.tracks.target_for(
                        layout.move_target_at(pointer.y, clip.track),
                        clip.sound_only,
                    );
                    let landing = view.tracks.landing(target, Some(id), snapped, length);
                    move_landing = Some((id, landing, Ticks(landing.0 + length.0), target));
                    state.snap_line = snap_target(landing, length, &targets);
                    state.track_scroll =
                        scroll_toward_edge(ui, state.track_scroll, furthest, area, pointer.y);
                    if response.drag_stopped() {
                        events.move_clip = Some((id, snapped, target));
                    }
                }
            }
            _ => {}
        }
        if response.clicked()
            && TrackLayout::has_lanes(track_count)
            && layout.new_lane_below().contains(pointer)
        {
            events.add_audio = true;
        } else if response.clicked() {
            events.seek = Some(Ticks(time.0.clamp(0, total.0)));
            let in_area = area.contains(pointer);
            let clicked_clip = in_area
                .then(|| clip_at(clips, &layout, &scale, pointer))
                .flatten();
            events.select = Some(clicked_clip.map(|(clip, _)| clip.id));
            if in_area && view.tracks.gap_at(time).is_some() {
                events.select_gap = Some(time);
            }
        }
    }
    if matches!(state.drag, TimelineDrag::Trim(..)) {
        state.snap_line = state.trim_aligned;
        // The trim is applied after this frame is drawn, so the result needs one more.
        ui.ctx().request_repaint();
    }
    if response.drag_stopped() {
        if matches!(state.drag, TimelineDrag::Trim(..)) {
            events.trim_finished = true;
        }
        state.snap_line = None;
        state.trim_aligned = None;
        state.drag = TimelineDrag::None;
        state.scrubbed_to = None;
        state.frozen_total = None;
    }

    // A right-click opens the menu on what is under it, without moving the playhead.
    if response.secondary_clicked()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let time = scale.time_at(pointer.x);
        let clip = area
            .contains(pointer)
            .then(|| clip_at(clips, &layout, &scale, pointer))
            .flatten();
        state.menu_target = if let Some((clip, _)) = clip {
            events.select = Some(Some(clip.id));
            Some(MenuTarget::Clip(clip.id))
        } else if view.tracks.gap_at(time).is_some() {
            events.select = Some(None);
            events.select_gap = Some(time);
            Some(MenuTarget::Gap(time))
        } else {
            None
        };
        state.menu_flyouts = MenuFlyouts::default();
    }

    // The ruler: the project's own length is tinted, and dead space within it hatched.
    let project_band = Rect::from_min_max(
        pos2(scale.x_of(Ticks(0)).max(rect.left()), ruler.top() + 2.0),
        pos2(scale.x_of(total).min(rect.right()), ruler.bottom() - 2.0),
    );
    if project_band.is_positive() {
        painter.rect_filled(
            project_band,
            3.0,
            palette.accent.gamma_multiply(PROJECT_TINT),
        );
    }
    if let Some(selected) = view
        .selected
        .and_then(|id| clips.iter().find(|clip| clip.id == id))
    {
        let band = Rect::from_min_max(
            pos2(
                scale.x_of(selected.start).max(rect.left()),
                ruler.top() + 2.0,
            ),
            pos2(
                scale.x_of(selected.end).min(rect.right()),
                ruler.bottom() - 2.0,
            ),
        );
        if band.is_positive() {
            painter.rect_filled(band, 3.0, palette.accent.gamma_multiply(SELECTED_TINT));
        }
    }
    let gaps = view.tracks.gaps();
    let selected_gap = view.selected_gap.and_then(|time| view.tracks.gap_at(time));
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

    let area_painter = painter.with_clip_rect(area.expand2(vec2(2.0, 0.0)).intersect(rect));
    // Where dead space is hatched: from the first track down to the last.
    let (stack_top, stack_bottom) = if track_count == 0 {
        (area.top(), area.bottom())
    } else {
        (
            layout.track(0).top(),
            layout.track(track_count - 1).bottom(),
        )
    };
    for index in 0..track_count {
        let track = layout.track(index);
        area_painter.rect_filled(track, 3.0, palette.bg_app);
        area_painter.rect_stroke(
            track,
            3.0,
            Stroke::new(1.0, palette.border),
            egui::StrokeKind::Inside,
        );
    }
    if TrackLayout::has_lanes(track_count) {
        draw_new_track_lane(
            &area_painter,
            palette,
            layout.new_lane_above(),
            NEW_TRACK_HINT,
        );
        let below = layout.new_lane_below();
        draw_new_track_lane(&area_painter, palette, below, ADD_AUDIO_HINT);
        if pointer_over.is_some_and(|pointer| below.contains(pointer)) {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
    }

    for gap in &gaps {
        let band = gap_rect(*gap, stack_top + 2.0, stack_bottom - 2.0);
        draw_hatch(&area_painter, band, palette.border);
        // A gap being pointed at or worked on stands out from the ruler down through the track.
        let selected = selected_gap == Some(*gap);
        if selected || hovered_gap == Some(*gap) {
            let full = gap_rect(*gap, ruler.top() + 2.0, stack_bottom - 2.0);
            painter.rect_filled(full, 2.0, palette.text.gamma_multiply(GAP_HIGHLIGHT));
            draw_hatch(&painter, full, palette.border_strong);
            let outline = if selected {
                Stroke::new(2.0, palette.accent)
            } else {
                Stroke::new(1.0, palette.border_strong)
            };
            painter.rect_stroke(full, 2.0, outline, egui::StrokeKind::Inside);
        }
    }
    for clip in clips {
        let shape = clip_rect(&scale, layout.track(clip.track), clip.start, clip.end);
        if shape.right() < area.left() || shape.left() > area.right() {
            continue;
        }
        let moving = move_landing.is_some_and(|(id, ..)| id == clip.id);
        let mut clip_painter = area_painter.clone();
        if moving {
            clip_painter.multiply_opacity(0.45);
        }
        draw_clip(
            &clip_painter,
            palette,
            shape,
            clip,
            (view.faces.get(&clip.media), &scale),
            clip_look(
                view.selected == Some(clip.id),
                state.hovered_clip == Some(clip.id),
            ),
        );
    }
    draw_hovered_names(ui, &area_painter, palette, state, clips, (&layout, &scale));
    speakers(ui, palette, state, clips, &layout, &scale, &mut events);
    if let Some((_, start, end, target)) = move_landing {
        draw_drop(
            &area_painter,
            palette,
            &layout,
            &scale,
            target,
            (start, end),
        );
    }
    if clips.is_empty() {
        painter.text(
            area.center(),
            Align2::CENTER_CENTER,
            "Drag a video or sound here, or press + on it",
            FontId::proportional(13.0),
            palette.text_muted,
        );
    }

    // Shows where a video dragged from the panel will land.
    let drop_at = |drag: &MediaDrag, time: Ticks| {
        snap_start(time, drag.length, &snap_targets(view, None), tolerance)
    };
    let dragged_pointer = ui.input(|input| input.pointer.latest_pos());
    if let (Some(drag), Some(time), Some(pointer)) = (
        response.dnd_hover_payload::<MediaDrag>(),
        hover_time,
        dragged_pointer,
    ) {
        let target = view
            .tracks
            .target_for(layout.target_at(pointer.y), drag.sound_only);
        let start = view
            .tracks
            .landing(target, None, drop_at(&drag, time), drag.length);
        let end = Ticks(start.0 + drag.length.0);
        state.snap_line = snap_target(start, drag.length, &snap_targets(view, None));
        state.track_scroll = scroll_toward_edge(ui, state.track_scroll, furthest, area, pointer.y);
        draw_drop(
            &area_painter,
            palette,
            &layout,
            &scale,
            target,
            (start, end),
        );
    }
    if let (Some(drag), Some(time), Some(pointer)) = (
        response.dnd_release_payload::<MediaDrag>(),
        hover_time,
        dragged_pointer,
    ) {
        events.drop_media = Some((
            drag.media,
            drop_at(&drag, time),
            layout.target_at(pointer.y),
        ));
    }

    if furthest > 0.0 {
        track_scrollbar(ui, palette, state, area, furthest);
    }

    // The button on a gap, the same way a hovered or selected clip shows its handles.
    let mut on_gap_button = false;
    if let Some(gap) = hovered_gap.or(selected_gap)
        && state.drag == TimelineDrag::None
    {
        let band = gap_rect(gap, stack_top, stack_bottom).intersect(area);
        let hint = "Close this dead space (Delete when it is selected)";
        if band.width() >= GAP_BUTTON_WIDTH + 8.0 {
            let place =
                Rect::from_center_size(band.center(), vec2(GAP_BUTTON_WIDTH, 26.0)).intersect(area);
            let mut child = ui.new_child(UiBuilder::new().max_rect(place));
            let button = ActionButton::new(palette, Kind::Secondary, "Remove gap")
                .icon(icons::X)
                .small()
                .width(place.width());
            let button = child.add(button).on_hover_text(hint);
            on_gap_button = button.contains_pointer();
            if button.clicked() {
                events.remove_gap = Some(gap.start);
            }
        } else if band.width() >= GAP_ICON_BUTTON_SIZE + 4.0 {
            let size = vec2(GAP_ICON_BUTTON_SIZE, GAP_ICON_BUTTON_SIZE);
            let place = Rect::from_center_size(band.center(), size).intersect(area);
            let button = IconButton::new(palette, Kind::Secondary, icons::X, "Remove gap")
                .size(GAP_ICON_BUTTON_SIZE, 14.0)
                .show_at(ui, egui::Id::unique("timeline-gap-remove"), place);
            let button = button.on_hover_text(hint);
            on_gap_button = button.contains_pointer();
            if button.clicked() {
                events.remove_gap = Some(gap.start);
            }
        }
    }

    // While trimming, the ruler also says where the clip now starts and ends.
    let trim_tags = match state.drag {
        TimelineDrag::Trim(id, edge) => clips.iter().find(|clip| clip.id == id).map(|clip| {
            let tag = |time, lit| ruler_tag(&painter, palette, text::nearest_second(time), lit);
            let (start, start_size) = tag(clip.start, edge == ClipEdge::Start);
            let (end, end_size) = tag(clip.end, edge == ClipEdge::End);
            let (start_rect, end_rect) = trim_tag_rects(
                ruler,
                scale.x_of(clip.start),
                scale.x_of(clip.end),
                start_size,
                end_size,
            );
            state.trim_readout = Some((clip.start, clip.end, edge));
            (start_rect, start, end_rect, end, edge)
        }),
        _ => None,
    };
    if trim_tags.is_none() {
        state.trim_readout = None;
    }
    if let Some(selected) = view
        .selected
        .and_then(|id| clips.iter().find(|clip| clip.id == id))
    {
        let middle = Ticks((selected.start.0 + selected.end.0) / 2);
        let x = scale.x_of(middle);
        if x > rect.left() && x < rect.right() {
            let length = text::clip_length(Ticks(selected.end.0 - selected.start.0));
            let (pill, size) = ruler_tag(&painter, palette, length, false);
            let shape = Rect::from_center_size(pos2(x, ruler.center().y), size);
            let crowded = trim_tags.as_ref().is_some_and(|(start, _, end, ..)| {
                start.intersects(shape) || end.intersects(shape)
            });
            if !crowded {
                paint_ruler_tag(&painter, palette, shape, pill, false);
            }
        }
    }
    if let Some(line) = state.snap_line {
        painter.line_segment(
            [
                pos2(scale.x_of(line), ruler.top()),
                pos2(scale.x_of(line), area.bottom()),
            ],
            Stroke::new(SNAP_LINE_WIDTH, palette.accent),
        );
    }

    let x = scale.x_of(view.position);
    if x >= area.left() - 1.0 && x <= area.right() + 1.0 {
        draw_playhead(&painter, palette, x, ruler.top() + 2.0, area.bottom() + 4.0);
    }
    // The end being dragged goes last, so it stays whole if the two ever meet.
    if let Some((start_rect, start, end_rect, end, edge)) = trim_tags {
        if edge == ClipEdge::Start {
            paint_ruler_tag(&painter, palette, end_rect, end, false);
            paint_ruler_tag(&painter, palette, start_rect, start, true);
        } else {
            paint_ruler_tag(&painter, palette, start_rect, start, false);
            paint_ruler_tag(&painter, palette, end_rect, end, true);
        }
    }

    // The time under the pointer goes on top of everything else, but only over empty parts
    // of the timeline. Anything that can be used has the pointer's attention instead: whatever
    // sits on a track, the playhead, and controls such as a gap's button. It also stays away
    // while a drag is showing times of its own.
    let on_something = pointer_over.is_some_and(|hover| {
        on_gap_button
            || !response.hovered()
            || (area.contains(hover) && clip_at(clips, &layout, &scale, hover).is_some())
            || on_playhead(
                hover,
                scale.x_of(view.position),
                ruler.top() + 2.0,
                area.bottom() + 4.0,
            )
    });
    state.hover_readout = None;
    if state.drag == TimelineDrag::None
        && !on_something
        && let Some(hover) = pointer_over
        && let Some((x, time)) = hover_mark(&scale, total, area, hover.x)
    {
        state.hover_readout = Some(time);
        painter.line_segment(
            [pos2(x, ruler.bottom()), pos2(x, rect.bottom())],
            Stroke::new(1.0, palette.text_muted),
        );
        draw_pill(&painter, palette, ruler, x, format_timecode(time));
    }

    // Moving the playhead by hand shows its time above it, leaving its head uncovered.
    state.playhead_bubble = state.drag == TimelineDrag::Scrub;
    if state.playhead_bubble {
        let area = Rect::from_min_max(
            pos2(rect.left(), ruler.top() - BUBBLE_ROOM),
            pos2(rect.right(), ruler.top() - 2.0),
        );
        draw_pill(
            ui.painter(),
            palette,
            area,
            scale.x_of(view.position),
            format_timecode(view.position),
        );
    }

    if let Some(target) = state.menu_target {
        let just_opened = response.secondary_clicked();
        let flyouts = &mut state.menu_flyouts;
        let speed = &mut events.speed;
        let menu = &mut events.menu;
        // The menu has to judge clicks itself. Left to the toolkit, a click beside it on the
        // speed controls would count as outside it.
        Popup::context_menu(&response)
            .close_behavior(PopupCloseBehavior::IgnoreClicks)
            .show(|ui| {
                let (picked, beside) = timeline_menu(ui, palette, view, target, (flyouts, speed));
                *menu = picked;
                let frame = ui.min_rect().expand(MENU_FRAME_ROOM);
                let clicked_at = ui.input(|input| {
                    input
                        .pointer
                        .any_click()
                        .then(|| input.pointer.interact_pos())
                        .flatten()
                });
                let outside = clicked_at.is_some_and(|at| {
                    !frame.contains(at) && !beside.is_some_and(|rect| rect.contains(at))
                });
                if picked.is_some() || (outside && !just_opened) {
                    ui.close();
                }
            });
        if !response.context_menu_opened() {
            state.menu_target = None;
            state.menu_flyouts = MenuFlyouts::default();
        }
    }

    let (strip, _) = ui.allocate_exact_size(vec2(width, STRIP_HEIGHT), Sense::hover());
    events.collapse = bottom_strip(ui, palette, state, strip, left, view);
    events
}

/// The seek bar that stands in for the timeline while it is collapsed. Returns where the
/// user moved the playhead to, and says whether it is being held.
pub fn seek_bar(
    ui: &mut egui::Ui,
    palette: &Palette,
    width: f32,
    position: Ticks,
    total: Ticks,
    held: &mut bool,
) -> Option<Ticks> {
    let (rect, response) =
        ui.allocate_exact_size(vec2(width, SEEK_BAR_HEIGHT), Sense::click_and_drag());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, true, "Seek"));
    let left = rect.left() + SEEK_HANDLE_RADIUS;
    let room = (rect.width() - SEEK_HANDLE_RADIUS * 2.0).max(1.0);
    let scale = TimeScale {
        visible_start: Ticks(0),
        visible: total,
        left,
        width: room,
    };
    let line = Rect::from_center_size(rect.center(), vec2(room, 3.0));
    ui.painter().rect_filled(line, 1.5, palette.border_strong);
    if total.0 > 0 {
        let x = scale.x_of(position.min(total));
        ui.painter().rect_filled(
            Rect::from_min_max(line.min, pos2(x, line.bottom())),
            1.5,
            palette.playhead,
        );
        ui.painter().circle_filled(
            pos2(x, rect.center().y),
            SEEK_HANDLE_RADIUS,
            palette.playhead,
        );
    }
    *held = response.dragged();
    // Only when it moves, because seeking again would stop the exact frame from ever being
    // reached while the pointer rests.
    let moved = response.dragged() && response.drag_delta() != vec2(0.0, 0.0);
    if total.0 <= 0 || !(moved || response.clicked()) {
        return None;
    }
    let pointer = response.interact_pointer_pos()?;
    Some(Ticks(scale.time_at(pointer.x).0.clamp(0, total.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_playhead_is_wider_at_its_head_than_along_its_line() {
        let (x, top, bottom) = (100.0, 10.0, 200.0);

        assert!(
            on_playhead(pos2(107.0, 15.0), x, top, bottom),
            "on the head"
        );
        assert!(
            !on_playhead(pos2(107.0, 60.0), x, top, bottom),
            "beside the line"
        );
        assert!(
            on_playhead(pos2(103.0, 60.0), x, top, bottom),
            "on the line"
        );
        assert!(!on_playhead(pos2(100.0, 205.0), x, top, bottom), "below it");
    }

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
            track: 0,
            start: Ticks::from_seconds(start),
            end: Ticks::from_seconds(end),
            label: String::new(),
            media: MediaId(1),
            source_start: Ticks(0),
            has_sound: false,
            muted: false,
            sound_only: false,
            shows_waveform: false,
            is_still: false,
            speed: Speed::default(),
            fade_in: Ticks(0),
            fade_out: Ticks(0),
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
    fn fitting_makes_the_video_fill_the_view() {
        let total = seconds(90.0);
        let mut state = TimelineState::default();
        state.zoom_by(8.0, seconds(70.0), total);

        state.fit(total);

        assert_eq!(state.scroll, Ticks(0));
        assert_eq!(state.visible_for(total), total);
    }

    #[test]
    fn a_new_clip_leaves_room_after_it_but_fitting_does_not() {
        let total = seconds(90.0);
        let mut state = TimelineState::default();

        state.content_added(total);
        let share = total.as_seconds() / state.visible_for(total).as_seconds();
        assert!((share - AUTO_FIT_FRACTION).abs() < 0.001, "{share}");

        state.fit(total);
        assert_eq!(state.visible_for(total), total);
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

        assert_eq!(state.visible_for(seconds(80.0)), seconds(90.0));
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
    fn a_snapped_clip_reports_what_it_lined_up_with() {
        let targets = [seconds(10.0), seconds(40.0)];

        assert_eq!(
            snap_target(seconds(10.0), seconds(5.0), &targets),
            Some(seconds(10.0))
        );
        assert_eq!(
            snap_target(seconds(35.0), seconds(5.0), &targets),
            Some(seconds(40.0))
        );
        assert_eq!(snap_target(seconds(20.0), seconds(5.0), &targets), None);
    }

    #[test]
    fn only_a_clip_being_used_shows_how_it_can_be_used() {
        assert_eq!(clip_look(false, false), ClipLook::Rest);
        assert_eq!(clip_look(false, true), ClipLook::Hovered);
        assert_eq!(clip_look(true, false), ClipLook::Selected);
        assert_eq!(clip_look(true, true), ClipLook::Selected);
    }

    #[test]
    fn handles_stay_inside_their_clip_at_any_width() {
        for width in [6.0, 20.0, 45.0, 400.0] {
            let rect = Rect::from_min_size(pos2(100.0, 50.0), vec2(width, 40.0));
            for shape in handle_shapes(rect) {
                for point in shape {
                    assert!(rect.expand(0.01).contains(point), "{width}: {point:?}");
                }
            }
        }
    }

    #[test]
    fn handles_can_be_grabbed_as_wide_as_they_look() {
        let scale = scale();
        let clips = [view(1, 10.0, 60.0)];
        let left = scale.x_of(clips[0].start);
        let right = scale.x_of(clips[0].end);

        assert_eq!(
            part_at(&clips, &scale, left + HANDLE_WIDTH - 0.5),
            Some((0, ClipPart::Handle(ClipEdge::Start)))
        );
        assert_eq!(
            part_at(&clips, &scale, right - HANDLE_WIDTH + 0.5),
            Some((0, ClipPart::Handle(ClipEdge::End)))
        );
        assert_eq!(
            part_at(&clips, &scale, left + HANDLE_WIDTH + 1.0),
            Some((0, ClipPart::Body))
        );
    }

    #[test]
    fn a_wider_timeline_shows_more_time_at_the_same_zoom() {
        let total = seconds(100.0);
        let mut state = TimelineState::default();
        state.fit(total);
        let before = state.visible_for(total);

        state.keep_zoom_for_width(800.0, 1000.0, total);
        let wider = state.visible_for(total);
        assert!((wider.as_seconds() / before.as_seconds() - 1.25).abs() < 0.001);

        state.keep_zoom_for_width(1000.0, 800.0, total);
        let back = state.visible_for(total);
        assert!((back.as_seconds() - before.as_seconds()).abs() < 0.001);
    }

    #[test]
    fn a_place_on_a_clip_shows_the_footage_there_wherever_the_clip_is_trimmed_to() {
        let scale = scale();
        let trimmed = ClipView {
            source_start: seconds(30.0),
            ..view(1, 10.0, 60.0)
        };

        // Ten pixels a second, starting at 10 s on a scale that starts at x = 10.
        assert_eq!(source_time_at(&trimmed, &scale, 110.0), seconds(30.0));
        assert_eq!(source_time_at(&trimmed, &scale, 210.0), seconds(40.0));
    }

    fn layout(count: usize, height: f32) -> TrackLayout {
        TrackLayout {
            area: Rect::from_min_size(pos2(0.0, 100.0), vec2(800.0, height)),
            count,
            scroll: 0.0,
        }
    }

    #[test]
    fn tracks_keep_their_height_when_the_timeline_gets_taller() {
        let short = layout(2, 200.0).track(0);
        let tall = layout(2, 700.0).track(0);

        assert_eq!(short.height(), TRACK_HEIGHT);
        assert_eq!(tall.height(), TRACK_HEIGHT);
    }

    #[test]
    fn only_a_single_track_has_lanes() {
        assert!(!TrackLayout::has_lanes(0));
        assert!(TrackLayout::has_lanes(1));
        assert!(!TrackLayout::has_lanes(2));
        assert!(!TrackLayout::has_lanes(5));

        // With no lanes in the way the tracks don't need room for them, and just fit.
        let snug = TRACK_HEIGHT * 3.0 + TRACK_GAP * 4.0;
        assert_eq!(TrackLayout::furthest_scroll(3, snug), 0.0);
        let first = layout(3, snug).track(0).top();
        assert!((first - (100.0 + TRACK_GAP)).abs() < 0.01, "{first}");
    }

    #[test]
    fn dragging_past_either_end_of_several_tracks_still_means_a_new_track() {
        let tracks = layout(3, 500.0);

        assert_eq!(
            tracks.target_at(tracks.track(0).top() - 1.0),
            TrackTarget::New(0)
        );
        assert_eq!(
            tracks.target_at(tracks.track(2).bottom() + 1.0),
            TrackTarget::New(3)
        );
        // Even for a clip already on the top track, which needs the pointer clear of it.
        assert_eq!(
            tracks.move_target_at(tracks.track(0).top() - 1.0, 0),
            TrackTarget::New(0)
        );
    }

    #[test]
    fn trim_times_sit_inside_the_ends_of_the_clips_stretch_of_the_ruler() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 24.0));
        let size = vec2(40.0, 18.0);

        let (start, end) = trim_tag_rects(area, 300.0, 700.0, size, size);

        assert_eq!(start.left(), 300.0);
        assert_eq!(end.right(), 700.0);
        assert!((start.center().y - area.center().y).abs() < 0.01);
    }

    #[test]
    fn trim_times_move_outside_a_clip_too_short_to_hold_them() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 24.0));
        let size = vec2(40.0, 18.0);

        let (start, end) = trim_tag_rects(area, 300.0, 340.0, size, size);

        assert_eq!(start.right(), 300.0 - TRIM_TAG_GAP);
        assert_eq!(end.left(), 340.0 + TRIM_TAG_GAP);
        assert!(!start.intersects(end));
    }

    #[test]
    fn trim_times_stay_on_the_ruler_at_its_ends() {
        let area = Rect::from_min_size(pos2(50.0, 0.0), vec2(1000.0, 24.0));
        let size = vec2(40.0, 18.0);

        let (start, end) = trim_tag_rects(area, -200.0, 1500.0, size, size);

        assert_eq!(start.left(), area.left());
        assert_eq!(end.right(), area.right());
    }

    #[test]
    fn labels_on_the_ruler_can_be_read_quiet_or_lit_in_both_themes() {
        for palette in [&crate::theme::DARK, &crate::theme::LIGHT] {
            for lit in [false, true] {
                let (fill, words, _) = ruler_tag_colours(palette, lit);
                assert!(
                    crate::theme::tests::contrast(words, fill) >= 4.5,
                    "lit: {lit}"
                );
            }
            let (quiet, ..) = ruler_tag_colours(palette, false);
            let (lit, ..) = ruler_tag_colours(palette, true);
            assert_ne!(quiet, lit);
        }
    }

    fn mark_scale() -> TimeScale {
        TimeScale {
            left: 100.0,
            width: 800.0,
            visible_start: Ticks(0),
            visible: seconds(80.0),
        }
    }

    #[test]
    fn the_time_under_the_pointer_follows_it_along_the_project() {
        let scale = mark_scale();
        let view = Rect::from_min_size(pos2(100.0, 0.0), vec2(800.0, 100.0));
        let x = scale.x_of(seconds(10.0));

        let (at, time) = hover_mark(&scale, seconds(45.0), view, x).expect("it shows");

        assert_eq!(at, x);
        assert!((time.as_seconds() - 10.0).abs() < 0.01);
    }

    #[test]
    fn the_time_under_the_pointer_stops_at_the_end_of_the_last_clip() {
        let scale = mark_scale();
        let view = Rect::from_min_size(pos2(100.0, 0.0), vec2(800.0, 100.0));
        let end = scale.x_of(seconds(45.0));

        for pointer in [end, end + 1.0, 890.0] {
            assert_eq!(
                hover_mark(&scale, seconds(45.0), view, pointer),
                Some((end, seconds(45.0)))
            );
        }
    }

    #[test]
    fn the_time_under_the_pointer_has_nowhere_to_show_without_a_project() {
        let scale = mark_scale();
        let view = Rect::from_min_size(pos2(100.0, 0.0), vec2(800.0, 100.0));

        assert_eq!(hover_mark(&scale, Ticks(0), view, 300.0), None);

        // Scrolled so that the project's end is already off the left of the view.
        let later = TimeScale {
            visible_start: seconds(60.0),
            ..scale
        };
        assert_eq!(hover_mark(&later, seconds(45.0), view, 300.0), None);
    }

    #[test]
    fn a_taller_timeline_shows_more_space_around_tracks_kept_in_the_middle() {
        let tall = layout(1, 700.0);

        let above = tall.new_lane_above().top() - tall.area.top();
        let below = tall.area.bottom() - tall.new_lane_below().bottom();

        assert!(above > 100.0, "{above}");
        assert!((above - below).abs() < 0.01);
    }

    #[test]
    fn the_new_track_lanes_sit_above_and_below_the_track() {
        let tracks = layout(1, 400.0);

        assert!(tracks.new_lane_above().bottom() < tracks.track(0).top());
        assert!(tracks.track(0).bottom() < tracks.new_lane_below().top());
    }

    #[test]
    fn the_middle_of_a_track_means_that_track() {
        let tracks = layout(2, 400.0);

        assert_eq!(
            tracks.target_at(tracks.track(0).center().y),
            TrackTarget::Existing(0)
        );
        assert_eq!(
            tracks.target_at(tracks.track(1).center().y),
            TrackTarget::Existing(1)
        );
    }

    #[test]
    fn between_tracks_above_and_below_mean_a_new_track_there() {
        let tracks = layout(2, 400.0);
        let between = (tracks.track(0).bottom() + tracks.track(1).top()) / 2.0;

        assert_eq!(
            tracks.target_at(tracks.new_lane_above().center().y),
            TrackTarget::New(0)
        );
        assert_eq!(tracks.target_at(between), TrackTarget::New(1));
        assert_eq!(
            tracks.target_at(tracks.new_lane_below().center().y),
            TrackTarget::New(2)
        );
    }

    #[test]
    fn the_edges_of_a_track_also_mean_a_new_track() {
        let tracks = layout(2, 400.0);

        assert_eq!(
            tracks.target_at(tracks.track(1).top() + 1.0),
            TrackTarget::New(1)
        );
        assert_eq!(
            tracks.target_at(tracks.track(1).bottom() - 1.0),
            TrackTarget::New(2)
        );
    }

    #[test]
    fn sliding_a_clip_along_its_own_track_never_makes_a_new_one() {
        let tracks = layout(2, 400.0);
        let edge = tracks.track(1).top() + 1.0;

        assert_eq!(tracks.move_target_at(edge, 1), TrackTarget::Existing(1));
        assert_eq!(tracks.move_target_at(edge, 0), TrackTarget::New(1));
    }

    #[test]
    fn a_dragged_clip_can_still_leave_for_the_space_beside_its_track() {
        let tracks = layout(2, 400.0);
        let between = (tracks.track(0).bottom() + tracks.track(1).top()) / 2.0;

        assert_eq!(tracks.move_target_at(between, 0), TrackTarget::New(1));
        assert_eq!(
            tracks.move_target_at(tracks.track(1).center().y, 0),
            TrackTarget::Existing(1)
        );
    }

    #[test]
    fn with_no_tracks_any_drop_makes_the_first_one() {
        assert_eq!(layout(0, 400.0).target_at(200.0), TrackTarget::New(0));
    }

    #[test]
    fn tracks_only_scroll_when_they_do_not_all_fit() {
        assert_eq!(TrackLayout::furthest_scroll(2, 400.0), 0.0);
        assert!(TrackLayout::furthest_scroll(8, 200.0) > 0.0);
    }

    #[test]
    fn scrolling_moves_the_tracks_up_the_screen() {
        let still = layout(8, 200.0);
        let scrolled = TrackLayout {
            scroll: 50.0,
            ..still
        };

        assert_eq!(scrolled.track(0).top(), still.track(0).top() - 50.0);
    }

    #[test]
    fn a_pointer_finds_the_clip_on_the_track_it_is_over() {
        let tracks = layout(2, 400.0);
        let scale = scale();
        let high = view(1, 10.0, 30.0);
        let low = ClipView {
            track: 1,
            ..view(2, 10.0, 30.0)
        };
        let clips = [high, low];
        let x = scale.x_of(seconds(20.0));

        let found = |y: f32| clip_at(&clips, &tracks, &scale, pos2(x, y)).map(|(clip, _)| clip.id);

        assert_eq!(found(tracks.track(0).center().y), Some(ClipId(1)));
        assert_eq!(found(tracks.track(1).center().y), Some(ClipId(2)));
        assert_eq!(found(tracks.new_lane_below().center().y), None);
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
