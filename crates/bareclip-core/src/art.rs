//! Things BareClip draws itself to drop into a video: shapes, annotations, frames, overlays and
//! backgrounds. Each one rides on a title's clip, so every rule for trimming, splitting, moving,
//! saving and exporting titles applies to it as it stands. This holds what each item is called and
//! how it moves. What it looks like is drawn in `bareclip-text`.

use serde::{Deserialize, Serialize};

use crate::{
    canvas::Colour,
    motion::{Pose, Stage, ease_in, ease_out},
    placement::WHOLE,
};

/// The thickest an outline can be.
pub const THICKEST_OUTLINE: u8 = 50;
/// The outline buttons: None, Thin, Medium and Thick.
pub const OUTLINE_STEPS: [(u8, &str); 4] = [(0, "None"), (2, "Thin"), (6, "Medium"), (12, "Thick")];

/// Which part of the library something comes from, which decides how it is added and what can be
/// changed about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ArtKind {
    Shape,
    Annotation,
    Frame,
    Overlay,
    Background,
}

impl ArtKind {
    /// In the order the library lists them.
    pub const ALL: [Self; 5] = [
        Self::Shape,
        Self::Annotation,
        Self::Frame,
        Self::Overlay,
        Self::Background,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Shape => "Shapes",
            Self::Annotation => "Annotations",
            Self::Frame => "Frames & borders",
            Self::Overlay => "Overlays",
            Self::Background => "Backgrounds",
        }
    }

    /// Whether it covers the whole canvas, rather than being something placed on it.
    pub fn fills_canvas(self) -> bool {
        matches!(self, Self::Frame | Self::Overlay | Self::Background)
    }

    /// Whether a new one lasts the whole project, from its start.
    pub fn lasts_whole_project(self) -> bool {
        matches!(self, Self::Frame | Self::Overlay)
    }

    /// How long a new one lasts, when it doesn't last the whole project.
    pub fn seconds(self) -> i64 {
        match self {
            Self::Annotation => 4,
            _ => 5,
        }
    }

    /// Whether adding one also gives it a card in My media. Shapes don't get one.
    pub fn gets_media_card(self) -> bool {
        self != Self::Shape
    }
}

/// The motion a shape's Natural choice plays, which suits the shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Natural {
    /// Pops in a little past its size and settles.
    Pop,
    /// Its outline draws itself round, clockwise from the top.
    DrawRound,
    /// Drops in, stretched tall and then squashed wide, and settles.
    Squash,
    /// Slides a third of its own length the way it points, and carries on that way to leave.
    SlidePointing,
    /// Pops out from its tail or point.
    PopFromTail,
    /// Opens like a flower, growing, turning and fading in.
    Bloom,
    /// Spins in from a quarter turn back while growing.
    SpinIn,
    /// Wobbles in like jelly.
    Wobble,
    /// Draws itself along its stroke, and rubs out from where it started.
    DrawAlong,
    /// Its cells fade in, sweeping left to right.
    CellsSweep,
}

/// How a shape comes in and goes out, chosen in its Animate panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Animate {
    #[default]
    None,
    Fade,
    SmoothSlide,
    BounceSlide,
    SmoothScale,
    BounceScale,
    Natural,
}

impl Animate {
    /// In the panel's order, with their names.
    pub const ALL: [(Self, &'static str); 7] = [
        (Self::None, "None"),
        (Self::Fade, "Fade"),
        (Self::SmoothSlide, "Smooth slide"),
        (Self::BounceSlide, "Bounce slide"),
        (Self::SmoothScale, "Smooth scale"),
        (Self::BounceScale, "Bounce scale"),
        (Self::Natural, "Natural"),
    ];

    pub fn name(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(animate, _)| *animate == self)
            .map_or("None", |(_, name)| name)
    }
}

/// One thing in the library.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArtItem {
    /// What a project calls it. Never changes once released.
    pub key: &'static str,
    pub name: &'static str,
    pub kind: ArtKind,
    pub collection: &'static str,
    pub natural: Natural,
    /// What it starts filled with, if it is filled.
    pub fill: Option<Colour>,
    /// The outline's colour, or a second colour for a background.
    pub second: Colour,
    pub outline: u8,
    /// Which way it points, for an arrow: across and down.
    pub points: [f32; 2],
    /// Where its tail is, as shares of its picture across and down, for something that pops out of
    /// it.
    pub tail: [f32; 2],
}

const SHAPE_GREY: Colour = [142, 140, 168];
const WHITE: Colour = [255, 255, 255];
const BLACK: Colour = [0, 0, 0];
const MARKER_RED: Colour = [230, 32, 32];
const MIDDLE: [f32; 2] = [0.5, 0.5];
const RIGHT: [f32; 2] = [1.0, 0.0];

const fn item(
    key: &'static str,
    name: &'static str,
    kind: ArtKind,
    collection: &'static str,
    natural: Natural,
) -> ArtItem {
    ArtItem {
        key,
        name,
        kind,
        collection,
        natural,
        fill: Some(SHAPE_GREY),
        second: WHITE,
        outline: 0,
        points: [0.0, 0.0],
        tail: MIDDLE,
    }
}

const fn shape(key: &'static str, name: &'static str, collection: &'static str) -> ArtItem {
    let natural = match collection.as_bytes() {
        b"Outline" => Natural::DrawRound,
        b"Rounded" => Natural::Squash,
        b"Organic" => Natural::Wobble,
        b"Hand drawn" => Natural::DrawAlong,
        b"Pattern" => Natural::CellsSweep,
        _ => Natural::Pop,
    };
    item(key, name, ArtKind::Shape, collection, natural)
}

/// A shape drawn only as its outline.
const fn outlined(mut item: ArtItem, width: u8) -> ArtItem {
    item.fill = None;
    item.second = SHAPE_GREY;
    item.outline = width;
    item
}

const fn with(mut item: ArtItem, natural: Natural) -> ArtItem {
    item.natural = natural;
    item
}

const fn pointing(mut item: ArtItem, points: [f32; 2]) -> ArtItem {
    item.natural = Natural::SlidePointing;
    item.points = points;
    item
}

const fn tailed(mut item: ArtItem, tail: [f32; 2]) -> ArtItem {
    item.natural = Natural::PopFromTail;
    item.tail = tail;
    item
}

const fn annotation(key: &'static str, name: &'static str, collection: &'static str) -> ArtItem {
    let mut item = item(
        key,
        name,
        ArtKind::Annotation,
        collection,
        Natural::DrawAlong,
    );
    item.fill = Some(MARKER_RED);
    item.outline = 6;
    item
}

const fn frame(key: &'static str, name: &'static str, collection: &'static str) -> ArtItem {
    let mut item = item(key, name, ArtKind::Frame, collection, Natural::Pop);
    item.fill = Some(WHITE);
    item
}

const fn coloured(mut item: ArtItem, fill: Colour) -> ArtItem {
    item.fill = Some(fill);
    item
}

const fn overlay(
    key: &'static str,
    name: &'static str,
    collection: &'static str,
    tint: Colour,
) -> ArtItem {
    let mut item = item(key, name, ArtKind::Overlay, collection, Natural::Pop);
    item.fill = Some(tint);
    item
}

const fn background(
    key: &'static str,
    name: &'static str,
    collection: &'static str,
    first: Colour,
    second: Colour,
) -> ArtItem {
    let mut item = item(key, name, ArtKind::Background, collection, Natural::Pop);
    item.fill = Some(first);
    item.second = second;
    item
}

/// Everything in the library, in the order it is listed.
pub const CATALOGUE: &[ArtItem] = &[
    // Shapes.
    shape("square", "Square", "Basic"),
    shape("rectangle", "Rectangle", "Basic"),
    shape("circle", "Circle", "Basic"),
    shape("oval", "Oval", "Basic"),
    shape("triangle", "Triangle", "Basic"),
    shape("right-triangle", "Right triangle", "Basic"),
    shape("diamond", "Diamond", "Basic"),
    shape("pentagon", "Pentagon", "Basic"),
    shape("hexagon", "Hexagon", "Basic"),
    shape("octagon", "Octagon", "Basic"),
    shape("pill", "Pill", "Basic"),
    shape("arch", "Arch", "Basic"),
    shape("line", "Line", "Basic"),
    shape("parallelogram", "Parallelogram", "Basic"),
    shape("star", "Star", "Basic"),
    shape("sparkle", "Sparkle", "Basic"),
    shape("six-star", "Six-pointed star", "Basic"),
    shape("burst", "Burst", "Basic"),
    shape("semicircle", "Semicircle", "Basic"),
    shape("quarter-circle", "Quarter circle", "Basic"),
    shape("heart", "Heart", "Basic"),
    shape("plus", "Plus", "Basic"),
    outlined(shape("outline-square", "Square outline", "Outline"), 6),
    outlined(
        shape("outline-rectangle", "Rectangle outline", "Outline"),
        6,
    ),
    outlined(shape("outline-circle", "Circle outline", "Outline"), 6),
    outlined(shape("outline-triangle", "Triangle outline", "Outline"), 6),
    outlined(shape("outline-diamond", "Diamond outline", "Outline"), 6),
    outlined(shape("outline-pentagon", "Pentagon outline", "Outline"), 6),
    outlined(shape("outline-hexagon", "Hexagon outline", "Outline"), 6),
    outlined(shape("outline-star", "Star outline", "Outline"), 6),
    outlined(shape("outline-heart", "Heart outline", "Outline"), 6),
    shape("rounded-square", "Rounded square", "Rounded"),
    shape("rounded-rectangle", "Rounded rectangle", "Rounded"),
    shape("squircle", "Squircle", "Rounded"),
    shape("rounded-triangle", "Rounded triangle", "Rounded"),
    shape("rounded-diamond", "Rounded diamond", "Rounded"),
    shape("rounded-pentagon", "Rounded pentagon", "Rounded"),
    pointing(shape("arrow", "Arrow", "Callouts"), RIGHT),
    pointing(shape("thin-arrow", "Thin arrow", "Callouts"), RIGHT),
    pointing(shape("double-arrow", "Double arrow", "Callouts"), RIGHT),
    pointing(shape("chevron", "Chevron", "Callouts"), RIGHT),
    pointing(shape("curved-arrow", "Curved arrow", "Callouts"), RIGHT),
    tailed(shape("tag", "Tag", "Callouts"), [1.0, 0.5]),
    tailed(shape("speech-box", "Speech box", "Callouts"), [0.22, 1.0]),
    tailed(
        shape("speech-bubble", "Speech bubble", "Callouts"),
        [0.25, 1.0],
    ),
    tailed(shape("banner", "Banner", "Callouts"), [0.5, 0.5]),
    with(shape("flower", "Flower", "Geometric"), Natural::Bloom),
    with(
        shape("eight-petals", "Eight petals", "Geometric"),
        Natural::Bloom,
    ),
    with(shape("daisy", "Daisy", "Geometric"), Natural::Bloom),
    with(shape("scallop", "Scallop", "Geometric"), Natural::Bloom),
    with(shape("clover", "Clover", "Geometric"), Natural::Bloom),
    with(shape("asterisk", "Asterisk", "Geometric"), Natural::SpinIn),
    with(
        shape("starburst", "Starburst", "Geometric"),
        Natural::SpinIn,
    ),
    with(shape("cog", "Cog", "Geometric"), Natural::SpinIn),
    shape("blob", "Blob", "Organic"),
    shape("puddle", "Puddle", "Organic"),
    shape("cloud", "Cloud", "Organic"),
    shape("pebble", "Pebble", "Organic"),
    shape("leaf", "Leaf", "Organic"),
    shape("drop", "Drop", "Organic"),
    outlined(shape("rough-oval", "Rough oval", "Hand drawn"), 6),
    outlined(shape("double-oval", "Double oval", "Hand drawn"), 5),
    outlined(shape("wavy-line", "Wavy line", "Hand drawn"), 8),
    outlined(shape("zigzag", "Zigzag", "Hand drawn"), 8),
    outlined(shape("swoosh", "Swoosh", "Hand drawn"), 10),
    outlined(shape("hatching", "Hatching", "Hand drawn"), 6),
    shape("checks", "Checks", "Pattern"),
    shape("dots", "Dots", "Pattern"),
    shape("grid", "Grid", "Pattern"),
    // Annotations.
    annotation("marker-circle", "Marker circle", "Circles & ovals"),
    annotation("marker-oval", "Wide oval", "Circles & ovals"),
    annotation("double-circle", "Double circle", "Circles & ovals"),
    annotation("bubble-round", "Round bubble", "Speech bubbles"),
    annotation("bubble-box", "Box bubble", "Speech bubbles"),
    annotation("marker-arrow", "Curved arrow", "Arrows"),
    annotation("straight-arrow", "Straight arrow", "Arrows"),
    annotation("loop-arrow", "Loop arrow", "Arrows"),
    annotation("tick", "Tick", "Checks & crosses"),
    annotation("cross", "Cross", "Checks & crosses"),
    annotation("ticked-box", "Ticked box", "Checks & crosses"),
    annotation("burst-lines", "Burst lines", "Stars & bursts"),
    annotation("drawn-star", "Drawn star", "Stars & bursts"),
    annotation("underline", "Underline", "Underlines & scribbles"),
    annotation(
        "double-underline",
        "Double underline",
        "Underlines & scribbles",
    ),
    annotation("squiggle", "Squiggle", "Underlines & scribbles"),
    annotation("drawn-box", "Drawn box", "Squares & rectangles"),
    annotation("brackets", "Brackets", "Squares & rectangles"),
    // Frames and borders.
    frame("solid-border", "Solid border", "Basic"),
    frame("double-border", "Double border", "Basic"),
    frame("rounded-border", "Rounded border", "Basic"),
    frame("dashed-border", "Dashed border", "Basic"),
    frame("corners", "Corners", "Corners"),
    frame("round-corners", "Round corners", "Corners"),
    coloured(frame("film-strip", "Film strip", "Film & photo"), BLACK),
    frame("instant-photo", "Instant photo", "Film & photo"),
    frame("viewfinder", "Viewfinder", "Camera"),
    coloured(frame("neon-glow", "Neon glow", "Glow"), [255, 64, 200]),
    coloured(frame("cool-glow", "Cool glow", "Glow"), [40, 200, 255]),
    coloured(frame("phone", "Phone", "Devices"), [24, 24, 30]),
    coloured(frame("laptop", "Laptop", "Devices"), [24, 24, 30]),
    coloured(frame("monitor", "Monitor", "Devices"), [24, 24, 30]),
    // Overlays.
    overlay("light-leak", "Light leak", "Light", [255, 140, 40]),
    overlay("bokeh", "Bokeh", "Light", [255, 214, 150]),
    overlay("dust", "Dust and scratches", "Film", [235, 225, 200]),
    overlay("sparkles", "Sparkles", "Sparkle", [255, 236, 170]),
    overlay("snow", "Snow", "Weather", WHITE),
    overlay("rain", "Rain", "Weather", [200, 215, 235]),
    // Backgrounds.
    background(
        "sunset",
        "Sunset",
        "Gradients",
        [255, 94, 98],
        [255, 195, 113],
    ),
    background("ocean", "Ocean", "Gradients", [72, 52, 212], [0, 212, 255]),
    background(
        "spotlight",
        "Spotlight",
        "Gradients",
        [70, 70, 110],
        [10, 10, 20],
    ),
    background(
        "split",
        "Diagonal split",
        "Two colours",
        [20, 20, 24],
        [255, 200, 0],
    ),
    background(
        "halves",
        "Halves",
        "Two colours",
        [30, 60, 160],
        [240, 240, 240],
    ),
    background(
        "soft-shapes",
        "Soft shapes",
        "Soft",
        [250, 220, 210],
        [255, 140, 160],
    ),
    background(
        "dot-grid",
        "Dot grid",
        "Patterns",
        [24, 24, 34],
        [90, 90, 120],
    ),
    background(
        "stripes",
        "Stripes",
        "Patterns",
        [245, 235, 220],
        [240, 120, 90],
    ),
    background(
        "graph-paper",
        "Graph paper",
        "Patterns",
        [250, 250, 245],
        [150, 190, 230],
    ),
];

/// The item a project calls by this name, if there is one.
pub fn item_named(key: &str) -> Option<&'static ArtItem> {
    CATALOGUE.iter().find(|item| item.key == key)
}

/// The collections of one kind, in the order the library lists them.
pub fn collections(kind: ArtKind) -> Vec<&'static str> {
    let mut found: Vec<&'static str> = Vec::new();
    for item in CATALOGUE.iter().filter(|item| item.kind == kind) {
        if !found.contains(&item.collection) {
            found.push(item.collection);
        }
    }
    found
}

/// The items in one collection of one kind.
pub fn items_in(kind: ArtKind, collection: &str) -> Vec<&'static ArtItem> {
    CATALOGUE
        .iter()
        .filter(|item| item.kind == kind && item.collection == collection)
        .collect()
}

/// Whether an item matches what was typed into the search box: every word has to be found in its
/// name, its collection or its kind, ignoring case.
pub fn matches_search(item: &ArtItem, search: &str) -> bool {
    let words = format!("{} {} {}", item.name, item.collection, item.kind.name()).to_lowercase();
    search
        .split_whitespace()
        .all(|word| words.contains(&word.to_lowercase()))
}

/// One drawn item on a clip, as a project keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Art {
    /// The item's key in the catalogue.
    pub item: String,
    /// What it is filled with, or the colour of its lines for something made only of lines.
    /// `None` leaves a shape empty.
    pub fill: Option<Colour>,
    /// The outline's colour, or the second colour of a background.
    pub second: Colour,
    /// How thick the outline is, from 0 to `THICKEST_OUTLINE`, in two-hundredths of the shape's
    /// longer side.
    pub outline: u8,
    pub animate: Animate,
}

impl Default for Art {
    fn default() -> Self {
        Self::of(&CATALOGUE[0])
    }
}

impl Art {
    /// A new one of this item, as it starts.
    pub fn of(item: &ArtItem) -> Self {
        Self {
            item: item.key.to_owned(),
            fill: item.fill,
            second: item.second,
            outline: item.outline,
            animate: Animate::None,
        }
    }

    /// The catalogue's entry for it, or the first shape if the project names one this version
    /// doesn't have.
    pub fn entry(&self) -> &'static ArtItem {
        item_named(&self.item).unwrap_or(&CATALOGUE[0])
    }

    pub fn kind(&self) -> ArtKind {
        self.entry().kind
    }

    /// The same with anything out of range brought back in, in case a file was edited by hand.
    pub fn checked(mut self) -> Self {
        self.outline = self.outline.min(THICKEST_OUTLINE);
        if item_named(&self.item).is_none() {
            self.item = CATALOGUE[0].key.to_owned();
        }
        self
    }

    /// The colour of its lines: an outline's, or for something made only of lines, its own.
    pub fn line_colour(&self) -> Colour {
        match self.kind() {
            ArtKind::Shape => self.second,
            _ => self.fill.unwrap_or(self.second),
        }
    }

    /// How a new one starts out, as a share of the canvas's shorter side.
    pub fn starting_size(&self) -> i32 {
        if self.kind().fills_canvas() {
            WHOLE
        } else {
            WHOLE * 2 / 5
        }
    }

    /// How it moves.
    pub fn motion(&self) -> ArtMotion {
        let entry = self.entry();
        ArtMotion {
            kind: entry.kind,
            animate: self.animate,
            natural: entry.natural,
            points: entry.points,
            tail: entry.tail,
        }
    }
}

/// How long each motion takes to come in, in seconds. Going out takes the same.
fn seconds_for(animate: Animate, natural: Natural) -> f32 {
    match animate {
        Animate::None => 0.0,
        Animate::BounceSlide => 0.7,
        Animate::BounceScale => 0.6,
        Animate::Natural => match natural {
            Natural::Pop => 0.4,
            Natural::DrawRound | Natural::CellsSweep | Natural::SpinIn => 0.6,
            Natural::Squash | Natural::SlidePointing | Natural::PopFromTail => 0.5,
            Natural::Wobble => 0.7,
            Natural::Bloom | Natural::DrawAlong => 0.8,
        },
        _ => 0.5,
    }
}

/// How long an annotation takes to draw itself in.
const DRAWING_IN_SECONDS: f32 = 0.8;
/// Coming in and going out together never take more than this share of a clip.
const MOST_OF_A_CLIP: f32 = 0.8;
/// How many different pictures a second something drawn afresh can have.
const FRAMES_A_SECOND: f64 = 60.0;
/// An overlay's pictures change half as often, like the footage overlays usually are, because
/// each one covers the canvas and takes several milliseconds to draw.
const OVERLAY_FRAMES_A_SECOND: f64 = 30.0;
/// How far past its place a bouncing slide runs, as a share of the way it came.
const BOUNCE_PAST: f32 = 0.08;

/// What a frame drawn afresh needs to know, beyond the item itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArtFrame {
    /// How much of its lines are drawn, from where they start. 1 is all of them.
    pub drawn: f32,
    /// How much of its lines is rubbed out, from where they start.
    pub rubbed: f32,
    /// How much wider and taller it is drawn than at rest.
    pub stretch: [f32; 2],
    /// How far a sweep across its cells has got, from 0 to 1.
    pub swept: f32,
    /// How many seconds into its clip it is, for something that keeps moving.
    pub seconds: f32,
}

impl ArtFrame {
    pub const REST: Self = Self {
        drawn: 1.0,
        rubbed: 0.0,
        stretch: [1.0, 1.0],
        swept: 1.0,
        seconds: 0.0,
    };
}

/// How a drawn item moves over its clip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArtMotion {
    pub kind: ArtKind,
    pub animate: Animate,
    pub natural: Natural,
    pub points: [f32; 2],
    pub tail: [f32; 2],
}

/// Goes from 0 to `peak` and back to 1, settling at `dip` on the way, as `t` goes from 0 to 1.
fn bounce(t: f32, peak: f32, dip: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
    if t < 0.5 {
        peak * smooth(t / 0.5)
    } else if t < 0.75 {
        peak + (dip - peak) * smooth((t - 0.5) / 0.25)
    } else {
        dip + (1.0 - dip) * smooth((t - 0.75) / 0.25)
    }
}

/// Runs a little past 1 and settles back, by `past` at most.
fn run_past(t: f32, past: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    // At 1.4 this goes about 8% past.
    let back = 1.4 * past / BOUNCE_PAST;
    let t = t - 1.0;
    1.0 + (back + 1.0) * t.powi(3) + back * t.powi(2)
}

impl ArtMotion {
    /// Whether it draws a new picture while it plays, rather than only moving the one it has.
    fn redraws(&self) -> bool {
        match self.kind {
            ArtKind::Annotation | ArtKind::Overlay => true,
            ArtKind::Shape => {
                self.animate == Animate::Natural
                    && matches!(
                        self.natural,
                        Natural::DrawRound
                            | Natural::Squash
                            | Natural::Wobble
                            | Natural::DrawAlong
                            | Natural::CellsSweep
                    )
            }
            ArtKind::Frame | ArtKind::Background => false,
        }
    }

    fn enter_seconds(&self) -> f32 {
        match self.kind {
            ArtKind::Shape => seconds_for(self.animate, self.natural),
            ArtKind::Annotation => DRAWING_IN_SECONDS,
            _ => 0.0,
        }
    }

    fn leave_seconds(&self) -> f32 {
        match self.kind {
            ArtKind::Shape => seconds_for(self.animate, self.natural),
            _ => 0.0,
        }
    }

    pub fn is_still(&self) -> bool {
        self.kind != ArtKind::Overlay && self.enter_seconds() == 0.0 && self.leave_seconds() == 0.0
    }

    pub fn draws_frames(&self) -> bool {
        self.redraws()
    }

    /// How far along a clip of this length it is, with coming in and going out squeezed to fit.
    pub fn stage(&self, seconds: f32, length: f32) -> Stage {
        let length = length.max(0.001);
        let wanted = self.enter_seconds() + self.leave_seconds();
        let squeeze = if wanted > length * MOST_OF_A_CLIP {
            length * MOST_OF_A_CLIP / wanted
        } else {
            1.0
        };
        let enter = self.enter_seconds() * squeeze;
        let leave = self.leave_seconds() * squeeze;
        Stage {
            entering: if enter > 0.0 {
                (seconds / enter).clamp(0.0, 1.0)
            } else {
                1.0
            },
            leaving: if leave > 0.0 {
                ((seconds - (length - leave)) / leave).clamp(0.0, 1.0)
            } else {
                0.0
            },
            through: (seconds / length).clamp(0.0, 1.0),
            seconds,
            length,
        }
    }

    /// Whether this moment needs a picture of its own, not the resting one moved about.
    pub fn draws_frame_at(&self, stage: Stage) -> bool {
        if !self.redraws() {
            return false;
        }
        self.kind == ArtKind::Overlay || stage.entering < 1.0 || stage.leaving > 0.0
    }

    /// Tells frames apart, so one already drawn isn't drawn again.
    pub fn frame_key(&self, stage: Stage) -> u64 {
        let rate = if self.kind == ArtKind::Overlay {
            OVERLAY_FRAMES_A_SECOND
        } else {
            FRAMES_A_SECOND
        };
        (f64::from(stage.seconds.max(0.0)) * rate) as u64
    }

    /// The moment a frame drawn afresh is drawn for, so every moment with the same key looks the
    /// same in the preview and the export.
    pub fn frame_seconds(&self, stage: Stage) -> f32 {
        if self.kind == ArtKind::Overlay {
            (self.frame_key(stage) as f64 / OVERLAY_FRAMES_A_SECOND) as f32
        } else {
            stage.seconds
        }
    }

    /// What to draw at this moment.
    pub fn frame(&self, stage: Stage) -> ArtFrame {
        let mut frame = ArtFrame {
            seconds: self.frame_seconds(stage),
            ..ArtFrame::REST
        };
        let (coming, going) = (stage.entering, stage.leaving);
        if coming >= 1.0 && going <= 0.0 {
            return frame;
        }
        match self.kind {
            ArtKind::Annotation => frame.drawn = ease_out(coming),
            ArtKind::Shape if self.animate == Animate::Natural => {
                // Going out plays coming in backwards, so it can be worked out the same way.
                let amount = if going > 0.0 { 1.0 - going } else { coming };
                match self.natural {
                    Natural::DrawRound => frame.drawn = ease_out(amount),
                    Natural::DrawAlong => {
                        frame.drawn = ease_out(coming);
                        frame.rubbed = ease_in(going);
                    }
                    Natural::CellsSweep => frame.swept = amount,
                    Natural::Squash => frame.stretch = squash(amount),
                    Natural::Wobble => frame.stretch = wobble(amount),
                    _ => {}
                }
            }
            _ => {}
        }
        frame
    }

    /// How the whole picture differs from where it rests.
    pub fn pose(&self, stage: Stage) -> Pose {
        if self.kind != ArtKind::Shape {
            return Pose::REST;
        }
        let (coming, going) = (stage.entering, stage.leaving);
        if coming >= 1.0 && going <= 0.0 {
            return Pose::REST;
        }
        let amount = if going > 0.0 { 1.0 - going } else { coming };
        let rest = Pose::REST;
        match self.animate {
            Animate::None => rest,
            Animate::Fade => Pose {
                opacity: amount * amount * (3.0 - 2.0 * amount),
                ..rest
            },
            Animate::SmoothSlide => Pose {
                offstage: if going > 0.0 {
                    -ease_in(going)
                } else {
                    1.0 - ease_out(coming)
                },
                ..rest
            },
            Animate::BounceSlide => Pose {
                offstage: if going > 0.0 {
                    -(1.0 - run_past(1.0 - going, BOUNCE_PAST))
                } else {
                    1.0 - run_past(coming, BOUNCE_PAST)
                },
                ..rest
            },
            Animate::SmoothScale => Pose {
                grow: ease_out(amount),
                ..rest
            },
            Animate::BounceScale => Pose {
                grow: bounce(amount, 1.15, 0.95),
                ..rest
            },
            Animate::Natural => self.natural_pose(coming, going, amount),
        }
    }

    fn natural_pose(&self, coming: f32, going: f32, amount: f32) -> Pose {
        let rest = Pose::REST;
        match self.natural {
            Natural::Pop => Pose {
                grow: bounce(amount, 1.05, 1.0),
                ..rest
            },
            Natural::PopFromTail => Pose {
                grow: bounce(amount, 1.05, 1.0),
                pivot: self.tail,
                ..rest
            },
            Natural::Bloom => Pose {
                grow: 0.3 + 0.7 * bounce(amount, 1.05 / 0.7 - 0.3 / 0.7, 1.0),
                turn: -20.0 * (1.0 - ease_out(amount)),
                opacity: (amount * 2.0).min(1.0),
                ..rest
            },
            Natural::SpinIn => Pose {
                grow: ease_out(amount),
                turn: -90.0 * (1.0 - ease_out(amount)),
                ..rest
            },
            Natural::SlidePointing => {
                let travelled = if going > 0.0 {
                    ease_in(going) / 3.0
                } else {
                    -(1.0 - ease_out(coming)) / 3.0
                };
                Pose {
                    shift: [self.points[0] * travelled, self.points[1] * travelled],
                    opacity: if going > 0.0 {
                        1.0 - going
                    } else {
                        (coming * 1.5).min(1.0)
                    },
                    ..rest
                }
            }
            Natural::Squash => Pose {
                down: -0.15 * (1.0 - ease_out((amount / 0.6).min(1.0))),
                opacity: (amount * 3.0).min(1.0),
                ..rest
            },
            Natural::Wobble => Pose {
                grow: ease_out(amount),
                ..rest
            },
            Natural::DrawRound | Natural::DrawAlong | Natural::CellsSweep => rest,
        }
    }
}

/// How much wider and taller a shape that drops in is drawn: stretched tall as it falls, squashed
/// wide as it lands, then settling.
fn squash(amount: f32) -> [f32; 2] {
    if amount >= 1.0 {
        return [1.0, 1.0];
    }
    let amount = amount.max(0.0);
    let (wide, tall) = if amount < 0.6 {
        (0.8, 1.2)
    } else if amount < 0.8 {
        let t = (amount - 0.6) / 0.2;
        (0.8 + 0.4 * t, 1.2 - 0.4 * t)
    } else {
        let t = (amount - 0.8) / 0.2;
        (1.2 - 0.2 * t, 0.8 + 0.2 * t)
    };
    [wide, tall]
}

/// How much wider and taller something wobbling in like jelly is drawn, with width and height
/// overshooting out of step.
fn wobble(amount: f32) -> [f32; 2] {
    let amount = amount.clamp(0.0, 1.0);
    let wave = 0.18 * (amount * std::f32::consts::PI * 3.0).sin() * (1.0 - amount);
    [1.0 + wave, 1.0 - wave]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_item_has_its_own_key_and_a_name() {
        for (index, item) in CATALOGUE.iter().enumerate() {
            assert!(!item.name.is_empty(), "{}", item.key);
            assert!(
                CATALOGUE[index + 1..]
                    .iter()
                    .all(|other| other.key != item.key),
                "{} is listed twice",
                item.key
            );
        }
    }

    #[test]
    fn every_kind_has_something_in_it_and_shapes_are_first() {
        for kind in ArtKind::ALL {
            assert!(!collections(kind).is_empty(), "{kind:?}");
        }
        assert_eq!(CATALOGUE[0].kind, ArtKind::Shape);
        assert_eq!(
            collections(ArtKind::Shape),
            [
                "Basic",
                "Outline",
                "Rounded",
                "Callouts",
                "Geometric",
                "Organic",
                "Hand drawn",
                "Pattern"
            ]
        );
    }

    #[test]
    fn about_a_hundred_items_and_around_sixty_shapes() {
        let shapes = CATALOGUE
            .iter()
            .filter(|item| item.kind == ArtKind::Shape)
            .count();
        assert!((55..=75).contains(&shapes), "{shapes} shapes");
        assert!((90..=130).contains(&CATALOGUE.len()), "{}", CATALOGUE.len());
    }

    #[test]
    fn every_arrow_points_somewhere_and_every_tail_is_on_its_picture() {
        for item in CATALOGUE {
            if item.natural == Natural::SlidePointing {
                assert!(item.points[0].hypot(item.points[1]) > 0.9, "{}", item.key);
            }
            assert!(item.tail.iter().all(|share| (0.0..=1.0).contains(share)));
        }
    }

    #[test]
    fn searching_finds_by_name_collection_or_kind_ignoring_case() {
        let star = item_named("star").expect("it is listed");
        assert!(matches_search(star, "STAR"));
        assert!(matches_search(star, "basic star"));
        assert!(matches_search(star, "shapes"));
        assert!(!matches_search(star, "arrow"));
        assert!(matches_search(star, "   "));
    }

    #[test]
    fn lengths_and_cards_follow_the_kind() {
        assert!(ArtKind::Frame.lasts_whole_project());
        assert!(ArtKind::Overlay.lasts_whole_project());
        assert!(!ArtKind::Shape.lasts_whole_project());
        assert_eq!(ArtKind::Shape.seconds(), 5);
        assert_eq!(ArtKind::Annotation.seconds(), 4);
        assert_eq!(ArtKind::Background.seconds(), 5);
        assert!(!ArtKind::Shape.gets_media_card());
        assert!(ArtKind::Annotation.gets_media_card());
    }

    #[test]
    fn a_hand_edited_item_comes_back_in_range() {
        let art = Art {
            item: "not a thing".to_owned(),
            outline: 200,
            ..Art::default()
        }
        .checked();
        assert_eq!(art.item, CATALOGUE[0].key);
        assert_eq!(art.outline, THICKEST_OUTLINE);
    }

    fn shape_moving(animate: Animate, key: &str) -> ArtMotion {
        let item = item_named(key).expect("it is listed");
        Art {
            animate,
            ..Art::of(item)
        }
        .motion()
    }

    #[test]
    fn a_new_shape_stays_still_and_frames_and_backgrounds_never_move() {
        assert!(shape_moving(Animate::None, "square").is_still());
        for key in ["solid-border", "sunset"] {
            let motion = Art::of(item_named(key).expect("listed")).motion();
            assert!(motion.is_still(), "{key}");
        }
        assert!(
            !Art::of(item_named("snow").expect("listed"))
                .motion()
                .is_still()
        );
    }

    #[test]
    fn every_choice_comes_in_and_goes_out_and_rests_between() {
        for (animate, name) in Animate::ALL.into_iter().skip(1) {
            for item in CATALOGUE.iter().filter(|item| item.kind == ArtKind::Shape) {
                let motion = shape_moving(animate, item.key);
                let middle = motion.stage(2.5, 5.0);
                assert_eq!(motion.pose(middle), Pose::REST, "{name} {}", item.key);
                assert_eq!(motion.frame(middle).drawn, 1.0);
                assert_eq!(motion.frame(middle).stretch, [1.0, 1.0]);
                let start = motion.pose(motion.stage(0.0, 5.0));
                let end = motion.pose(motion.stage(5.0, 5.0));
                let frame_start = motion.frame(motion.stage(0.0, 5.0));
                let hidden = |pose: Pose, frame: ArtFrame| {
                    pose.opacity < 0.05
                        || pose.grow < 0.35
                        || pose.offstage.abs() > 0.99
                        || pose.shift != [0.0, 0.0]
                        || pose.down != 0.0
                        || frame.drawn < 0.05
                        || frame.swept < 0.05
                        || frame.stretch != [1.0, 1.0]
                };
                assert!(
                    hidden(start, frame_start),
                    "{name} {} doesn't start out",
                    item.key
                );
                let frame_end = motion.frame(motion.stage(5.0, 5.0));
                let gone = hidden(end, frame_end) || frame_end.rubbed > 0.95;
                assert!(gone, "{name} {} doesn't go out", item.key);
            }
        }
    }

    #[test]
    fn a_smooth_slide_comes_from_the_left_and_leaves_to_the_right() {
        let motion = shape_moving(Animate::SmoothSlide, "square");
        assert!(motion.pose(motion.stage(0.0, 5.0)).offstage >= 0.99);
        assert!(motion.pose(motion.stage(0.25, 5.0)).offstage > 0.0);
        assert!(motion.pose(motion.stage(4.9, 5.0)).offstage < 0.0);
        assert!(motion.pose(motion.stage(5.0, 5.0)).offstage <= -0.99);
    }

    #[test]
    fn a_bounce_slide_runs_past_its_place_and_settles_back() {
        let motion = shape_moving(Animate::BounceSlide, "square");
        let furthest = (0..=70)
            .map(|step| motion.pose(motion.stage(step as f32 / 100.0, 5.0)).offstage)
            .fold(f32::MAX, f32::min);
        assert!((-0.1..-0.05).contains(&furthest), "it went {furthest} past");
    }

    #[test]
    fn a_bounce_scale_grows_past_its_size_and_dips_before_settling() {
        let motion = shape_moving(Animate::BounceScale, "square");
        let sizes: Vec<f32> = (0..=60)
            .map(|step| motion.pose(motion.stage(step as f32 / 100.0, 5.0)).grow)
            .collect();
        let biggest = sizes.iter().copied().fold(0.0, f32::max);
        assert!((biggest - 1.15).abs() < 0.01, "{biggest}");
        let after_peak = sizes.iter().position(|size| *size == biggest).unwrap_or(0);
        let lowest = sizes[after_peak..].iter().copied().fold(2.0, f32::min);
        assert!((lowest - 0.95).abs() < 0.01, "{lowest}");
    }

    #[test]
    fn natural_suits_the_shape() {
        let natural = |key: &str| shape_moving(Animate::Natural, key);
        let early = |motion: ArtMotion| motion.pose(motion.stage(0.2, 5.0));

        let flower = natural("flower");
        assert!(early(flower).turn < 0.0, "a flower turns as it opens");
        assert!(early(flower).grow > 0.3);

        let star = natural("starburst");
        assert!(early(star).turn < -10.0, "a burst spins in");

        let arrow = natural("arrow");
        assert!(
            early(arrow).shift[0] < 0.0,
            "an arrow comes from behind itself"
        );
        let leaving = arrow.pose(arrow.stage(4.9, 5.0));
        assert!(leaving.shift[0] > 0.0, "and carries on forwards");

        let bubble = natural("speech-bubble");
        assert_eq!(
            early(bubble).pivot,
            [0.25, 1.0],
            "a bubble grows from its tail"
        );

        let outline = natural("outline-circle");
        assert!(outline.draws_frames());
        assert!(outline.frame(outline.stage(0.2, 5.0)).drawn < 1.0);

        let scribble = natural("wavy-line");
        let end = scribble.frame(scribble.stage(4.95, 5.0));
        assert_eq!(end.drawn, 1.0, "it rubs out rather than undrawing");
        assert!(end.rubbed > 0.5);

        let blob = natural("blob");
        let stretch = blob.frame(blob.stage(0.3, 5.0)).stretch;
        assert_ne!(
            stretch[0], stretch[1],
            "width and height wobble out of step"
        );

        let rounded = natural("rounded-square");
        assert!(rounded.frame(rounded.stage(0.1, 5.0)).stretch[1] > 1.0);

        let pattern = natural("checks");
        assert!(pattern.frame(pattern.stage(0.3, 5.0)).swept < 1.0);

        let square = natural("square");
        assert!(
            !square.draws_frames(),
            "a plain shape only moves its picture"
        );
    }

    #[test]
    fn coming_in_and_going_out_squeeze_into_a_short_clip() {
        let motion = shape_moving(Animate::Fade, "square");
        let stage = motion.stage(0.4, 1.0);
        assert_eq!(stage.entering, 1.0, "each part takes 40% of a 1 s clip");
        assert_eq!(stage.leaving, 0.0);
        assert!(motion.stage(0.61, 1.0).leaving > 0.0);
    }

    #[test]
    fn an_annotation_draws_itself_in_and_then_holds() {
        let motion = Art::of(item_named("marker-circle").expect("listed")).motion();
        assert!(motion.frame(motion.stage(0.0, 4.0)).drawn < 0.01);
        assert!(motion.draws_frame_at(motion.stage(0.4, 4.0)));
        assert!(!motion.draws_frame_at(motion.stage(2.0, 4.0)));
        assert_eq!(motion.frame(motion.stage(3.9, 4.0)).drawn, 1.0);
        assert_eq!(motion.pose(motion.stage(0.1, 4.0)), Pose::REST);
    }

    const CANVAS: (u32, u32) = (1920, 1080);
    const PICTURE: (u32, u32) = (400, 200);

    fn rest() -> crate::placement::Placement {
        crate::placement::Placement {
            center_x: WHOLE / 3,
            center_y: WHOLE / 2,
            scale: WHOLE / 4,
            ..Default::default()
        }
    }

    #[test]
    fn sliding_offstage_goes_just_past_either_edge() {
        let off_left = Pose {
            offstage: 1.0,
            ..Pose::REST
        }
        .on(rest(), PICTURE, CANVAS)
        .rect(PICTURE, CANVAS);
        assert!((off_left.x + off_left.width).abs() < 1.0, "{off_left:?}");
        let off_right = Pose {
            offstage: -1.0,
            ..Pose::REST
        }
        .on(rest(), PICTURE, CANVAS)
        .rect(PICTURE, CANVAS);
        assert!((off_right.x - CANVAS.0 as f32).abs() < 1.0, "{off_right:?}");
    }

    #[test]
    fn a_shift_is_in_its_own_size_and_turns_with_it() {
        let before = rest().rect(PICTURE, CANVAS);
        let moved = Pose {
            shift: [1.0, 0.0],
            ..Pose::REST
        }
        .on(rest(), PICTURE, CANVAS)
        .rect(PICTURE, CANVAS);
        assert!((moved.x - before.x - before.width).abs() < 1.0);
        let turned = crate::placement::Placement {
            rotation: 90_000,
            ..rest()
        };
        let down = Pose {
            shift: [1.0, 0.0],
            ..Pose::REST
        }
        .on(turned, PICTURE, CANVAS)
        .rect(PICTURE, CANVAS);
        assert!((down.y - before.y - before.width).abs() < 1.0, "{down:?}");
        assert!((down.x - before.x).abs() < 1.0);
    }

    #[test]
    fn growing_around_a_tail_keeps_the_tail_where_it_was() {
        let before = rest().rect(PICTURE, CANVAS);
        let grown = Pose {
            grow: 0.5,
            pivot: [0.0, 1.0],
            ..Pose::REST
        }
        .on(rest(), PICTURE, CANVAS)
        .rect(PICTURE, CANVAS);
        assert!((grown.x - before.x).abs() < 1.0, "{grown:?}");
        assert!((grown.y + grown.height - before.y - before.height).abs() < 1.0);
        assert!((grown.width - before.width / 2.0).abs() < 1.0);
    }

    #[test]
    fn an_overlay_keeps_moving_for_its_whole_clip() {
        let motion = Art::of(item_named("rain").expect("listed")).motion();
        for seconds in [0.0, 3.0, 9.0] {
            assert!(motion.draws_frame_at(motion.stage(seconds, 10.0)));
        }
        assert_ne!(
            motion.frame_key(motion.stage(1.0, 10.0)),
            motion.frame_key(motion.stage(1.04, 10.0))
        );
        // Thirty pictures a second, each drawn for the start of its moment.
        let (a, b) = (motion.stage(1.001, 10.0), motion.stage(1.03, 10.0));
        assert_eq!(motion.frame_key(a), motion.frame_key(b));
        assert_eq!(motion.frame(a), motion.frame(b));
    }
}
