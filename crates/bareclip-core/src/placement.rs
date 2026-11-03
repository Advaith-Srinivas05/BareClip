//! Where a clip's picture sits on the finished video, how big it is and how see-through.
//!
//! Values are whole numbers, like time, so that saving, comparing and undoing are exact.

use serde::{Deserialize, Serialize};

use crate::time::Ticks;

/// What one whole means in a placement: the canvas's full width or height, a picture at its
/// normal size, or a picture that hides what is under it.
pub const WHOLE: i32 = 1_000_000;
/// The smallest and largest a picture can be, compared with its normal size.
pub const SMALLEST_SCALE: i32 = WHOLE / 20;
pub const LARGEST_SCALE: i32 = WHOLE * 4;
/// A whole turn, in thousandths of a degree.
pub const FULL_TURN: i32 = 360_000;
const QUARTER_TURN: i32 = FULL_TURN / 4;
/// How far a scale can be from a picture's best fit and still count as being at it, so that turning
/// a picture back round finds its way home exactly.
const CLOSE_TO_FIT: f64 = 0.002;
/// The least of a picture that cropping leaves, each way.
const SMALLEST_KEPT_BY_CROP: i32 = WHOLE / 20;
/// How big a picture-in-picture is, compared with its normal size.
const PICTURE_IN_PICTURE_SCALE: i32 = WHOLE * 3 / 10;
/// How far a picture-in-picture sits from the canvas's edges, as a share of its shorter side.
const PICTURE_IN_PICTURE_MARGIN: f64 = 0.04;
/// How far a saved centre can be from the canvas. A hand-edited file can't lose a clip further
/// away than this.
const FURTHEST_CENTRE: i32 = WHOLE * 5;

/// A place on the canvas, in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// A picture an odd size short of an even canvas leaves less than this uncovered, and nobody
/// could see what shows through.
const COVERS_WITHIN: f32 = 1.5;

impl Rect {
    fn covers(&self, canvas: (u32, u32)) -> bool {
        self.x <= COVERS_WITHIN
            && self.y <= COVERS_WITHIN
            && self.x + self.width >= canvas.0 as f32 - COVERS_WITHIN
            && self.y + self.height >= canvas.1 as f32 - COVERS_WITHIN
    }
}

/// The largest place a picture fits on the canvas without losing any of it, in the middle.
/// Whatever it doesn't cover is left as bars.
pub fn fit_inside(picture: (u32, u32), canvas: (u32, u32)) -> Rect {
    let (width, height) = (picture.0.max(1) as f32, picture.1.max(1) as f32);
    let (canvas_width, canvas_height) = (canvas.0 as f32, canvas.1 as f32);
    let scale = (canvas_width / width).min(canvas_height / height);
    let (shown_width, shown_height) = (width * scale, height * scale);
    Rect {
        x: (canvas_width - shown_width) / 2.0,
        y: (canvas_height - shown_height) / 2.0,
        width: shown_width,
        height: shown_height,
    }
}

/// The size of the finished video: the largest video's, so nothing loses detail. Encoders need
/// even sizes, so an odd one rounds up.
pub fn final_size(videos: &[(u32, u32)]) -> Option<(u32, u32)> {
    let mut largest: Option<(u32, u32)> = None;
    for &(width, height) in videos {
        let area = u64::from(width) * u64::from(height);
        let bigger = largest.is_none_or(|(w, h)| area > u64::from(w) * u64::from(h));
        if bigger {
            largest = Some((width, height));
        }
    }
    largest.map(|(width, height)| (width.next_multiple_of(2), height.next_multiple_of(2)))
}

/// How a clip's picture is laid out. Stored as parts of the canvas, so it means the same thing
/// on the preview's small canvas and on the finished video's full-size one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Placement {
    pub center_x: i32,
    pub center_y: i32,
    /// Compared with the largest size at which the whole picture fits on the canvas.
    pub scale: i32,
    pub opacity: i32,
    /// Clockwise, in thousandths of a degree, from 0 up to a full turn.
    pub rotation: i32,
    pub flip_x: bool,
    pub flip_y: bool,
    /// How much is cut from each side of the source picture, as parts of its width or height.
    /// These go by the source, not by how it looks once flipped.
    pub crop_left: i32,
    pub crop_top: i32,
    pub crop_right: i32,
    pub crop_bottom: i32,
}

/// How much is cut from each side of a picture as it looks on screen, once flipped but not
/// turned, as shares of the whole picture.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CropSides {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

/// A corner of the canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanvasCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl CanvasCorner {
    pub const ALL: [(Self, &'static str); 4] = [
        (Self::TopLeft, "Top left"),
        (Self::TopRight, "Top right"),
        (Self::BottomLeft, "Bottom left"),
        (Self::BottomRight, "Bottom right"),
    ];
}

/// How a picture is drawn in its place, beyond where that place is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PictureLook {
    /// Clockwise, around the place's middle.
    pub degrees: f32,
    pub flip_x: bool,
    pub flip_y: bool,
    /// The part of the source picture that shows: left, top, right and bottom, from 0 to 1.
    pub source: [f32; 4],
}

impl PictureLook {
    pub const PLAIN: Self = Self {
        degrees: 0.0,
        flip_x: false,
        flip_y: false,
        source: [0.0, 0.0, 1.0, 1.0],
    };

    pub fn is_plain(&self) -> bool {
        *self == Self::PLAIN
    }
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            center_x: WHOLE / 2,
            center_y: WHOLE / 2,
            scale: WHOLE,
            opacity: WHOLE,
            rotation: 0,
            flip_x: false,
            flip_y: false,
            crop_left: 0,
            crop_top: 0,
            crop_right: 0,
            crop_bottom: 0,
        }
    }
}

impl Placement {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The same placement with anything out of range brought back in.
    pub fn clamped(self) -> Self {
        Self {
            center_x: self.center_x.clamp(-FURTHEST_CENTRE, FURTHEST_CENTRE),
            center_y: self.center_y.clamp(-FURTHEST_CENTRE, FURTHEST_CENTRE),
            scale: self.scale.clamp(SMALLEST_SCALE, LARGEST_SCALE),
            opacity: self.opacity.clamp(0, WHOLE),
            rotation: self.rotation.rem_euclid(FULL_TURN),
            crop_left: self.crop_left.clamp(0, WHOLE - SMALLEST_KEPT_BY_CROP),
            crop_right: self
                .crop_right
                .clamp(0, (WHOLE - SMALLEST_KEPT_BY_CROP - self.crop_left).max(0)),
            crop_top: self.crop_top.clamp(0, WHOLE - SMALLEST_KEPT_BY_CROP),
            crop_bottom: self
                .crop_bottom
                .clamp(0, (WHOLE - SMALLEST_KEPT_BY_CROP - self.crop_top).max(0)),
            ..self
        }
    }

    /// Whether any of the picture has been cut away.
    pub fn is_cropped(self) -> bool {
        self.crop_left != 0 || self.crop_top != 0 || self.crop_right != 0 || self.crop_bottom != 0
    }

    /// How much of the picture's width and height is left after cropping.
    fn kept(self) -> (f64, f64) {
        let whole = f64::from(WHOLE);
        (
            1.0 - f64::from(self.crop_left + self.crop_right) / whole,
            1.0 - f64::from(self.crop_top + self.crop_bottom) / whole,
        )
    }

    /// How much is cut from each side, as the picture looks on screen.
    pub fn crop_sides(self) -> CropSides {
        let share = |value: i32| f64::from(value) / f64::from(WHOLE);
        let (left, right) = if self.flip_x {
            (self.crop_right, self.crop_left)
        } else {
            (self.crop_left, self.crop_right)
        };
        let (top, bottom) = if self.flip_y {
            (self.crop_bottom, self.crop_top)
        } else {
            (self.crop_top, self.crop_bottom)
        };
        CropSides {
            left: share(left),
            top: share(top),
            right: share(right),
            bottom: share(bottom),
        }
    }

    /// How far the part that is kept sits from the middle of the whole picture, in pixels on the
    /// canvas, after turning.
    fn kept_offset(self, picture: (u32, u32), canvas: (u32, u32)) -> (f64, f64) {
        let fitted = fit_inside(picture, canvas);
        let scale = f64::from(self.scale) / f64::from(WHOLE);
        let sides = self.crop_sides();
        let across = (sides.left - sides.right) / 2.0 * f64::from(fitted.width) * scale;
        let down = (sides.top - sides.bottom) / 2.0 * f64::from(fitted.height) * scale;
        let radians = self.degrees().to_radians();
        (
            across * radians.cos() - down * radians.sin(),
            across * radians.sin() + down * radians.cos(),
        )
    }

    /// The same picture with nothing cut away, with what was kept staying exactly where it is.
    pub fn uncropped(self, picture: (u32, u32), canvas: (u32, u32)) -> Self {
        let (across, down) = self.kept_offset(picture, canvas);
        let whole = f64::from(WHOLE);
        Self {
            center_x: self.center_x - (across / f64::from(canvas.0) * whole).round() as i32,
            center_y: self.center_y - (down / f64::from(canvas.1) * whole).round() as i32,
            crop_left: 0,
            crop_top: 0,
            crop_right: 0,
            crop_bottom: 0,
            ..self
        }
    }

    /// The same picture cropped differently, with what stays staying exactly where it is. Cropping
    /// never makes the rest of the picture move or grow.
    pub fn cropped_to(self, sides: CropSides, picture: (u32, u32), canvas: (u32, u32)) -> Self {
        let full = self.uncropped(picture, canvas);
        let whole = f64::from(WHOLE);
        let part = |share: f64| (share.clamp(0.0, 1.0) * whole).round() as i32;
        let (crop_left, crop_right) = if self.flip_x {
            (part(sides.right), part(sides.left))
        } else {
            (part(sides.left), part(sides.right))
        };
        let (crop_top, crop_bottom) = if self.flip_y {
            (part(sides.bottom), part(sides.top))
        } else {
            (part(sides.top), part(sides.bottom))
        };
        let cropped = Self {
            crop_left,
            crop_top,
            crop_right,
            crop_bottom,
            ..full
        }
        .clamped();
        let (across, down) = cropped.kept_offset(picture, canvas);
        Self {
            center_x: full.center_x + (across / f64::from(canvas.0) * whole).round() as i32,
            center_y: full.center_y + (down / f64::from(canvas.1) * whole).round() as i32,
            ..cropped
        }
    }

    /// The turn, in degrees.
    pub fn degrees(self) -> f64 {
        f64::from(self.rotation) / 1000.0
    }

    /// How the picture is drawn in its place.
    pub fn look(self) -> PictureLook {
        PictureLook {
            degrees: self.degrees() as f32,
            flip_x: self.flip_x,
            flip_y: self.flip_y,
            source: [
                self.crop_left as f32 / WHOLE as f32,
                self.crop_top as f32 / WHOLE as f32,
                1.0 - self.crop_right as f32 / WHOLE as f32,
                1.0 - self.crop_bottom as f32 / WHOLE as f32,
            ],
        }
    }

    /// The scale at which a picture just fits the canvas when turned this far by quarter turns.
    fn fit_scale(picture: (u32, u32), canvas: (u32, u32), rotation: i32, kept: (f64, f64)) -> f64 {
        let fitted = fit_inside(picture, canvas);
        let (kept_width, kept_height) = kept;
        let width = f64::from(fitted.width) * kept_width;
        let height = f64::from(fitted.height) * kept_height;
        let (across, down) = if rotation % (QUARTER_TURN * 2) == 0 {
            (width, height)
        } else {
            (height, width)
        };
        (f64::from(canvas.0) / across).min(f64::from(canvas.1) / down)
    }

    /// Turned on to the next quarter. A picture that was just fitting the canvas goes on fitting it,
    /// so a wide video turned upright shows all of itself, and four turns bring it back exactly.
    pub fn turned_quarter(self, picture: (u32, u32), canvas: (u32, u32)) -> Self {
        let rotation = (self.rotation / QUARTER_TURN + 1) * QUARTER_TURN % FULL_TURN;
        let scale_now = f64::from(self.scale) / f64::from(WHOLE);
        let was_fitting = self.rotation % QUARTER_TURN == 0
            && (scale_now - Self::fit_scale(picture, canvas, self.rotation, self.kept())).abs()
                <= CLOSE_TO_FIT;
        let scale = if was_fitting {
            let fit = Self::fit_scale(picture, canvas, rotation, self.kept());
            ((fit * f64::from(WHOLE)).round() as i32).clamp(SMALLEST_SCALE, LARGEST_SCALE)
        } else {
            self.scale
        };
        Self {
            rotation,
            scale,
            ..self
        }
    }

    /// Small, in a corner of the canvas with a little room around it. However it is turned, all of
    /// it stays on the canvas.
    pub fn in_corner(self, corner: CanvasCorner, picture: (u32, u32), canvas: (u32, u32)) -> Self {
        let small = Self {
            scale: PICTURE_IN_PICTURE_SCALE,
            ..self
        };
        let rect = small.rect(picture, canvas);
        let radians = small.degrees().to_radians();
        let (width, height) = (f64::from(rect.width), f64::from(rect.height));
        let half_across = (width * radians.cos().abs() + height * radians.sin().abs()) / 2.0;
        let half_down = (width * radians.sin().abs() + height * radians.cos().abs()) / 2.0;
        let (canvas_width, canvas_height) = (f64::from(canvas.0), f64::from(canvas.1));
        let margin = canvas_width.min(canvas_height) * PICTURE_IN_PICTURE_MARGIN;
        let (near_x, far_x) = (margin + half_across, canvas_width - margin - half_across);
        let (near_y, far_y) = (margin + half_down, canvas_height - margin - half_down);
        let (x, y) = match corner {
            CanvasCorner::TopLeft => (near_x, near_y),
            CanvasCorner::TopRight => (far_x, near_y),
            CanvasCorner::BottomLeft => (near_x, far_y),
            CanvasCorner::BottomRight => (far_x, far_y),
        };
        let whole = f64::from(WHOLE);
        Self {
            center_x: (x / canvas_width * whole).round() as i32,
            center_y: (y / canvas_height * whole).round() as i32,
            ..small
        }
    }

    /// Mirrored left to right as it looks on screen, however it is turned. A mirror image of a
    /// turned picture is the mirrored picture turned the other way.
    pub fn flipped_across(self) -> Self {
        Self {
            flip_x: !self.flip_x,
            rotation: (FULL_TURN - self.rotation).rem_euclid(FULL_TURN),
            ..self
        }
    }

    /// Mirrored top to bottom as it looks on screen, however it is turned.
    pub fn flipped_down(self) -> Self {
        Self {
            flip_y: !self.flip_y,
            rotation: (FULL_TURN - self.rotation).rem_euclid(FULL_TURN),
            ..self
        }
    }

    /// Where a picture of this size goes on a canvas.
    pub fn rect(self, picture: (u32, u32), canvas: (u32, u32)) -> Rect {
        let fitted = fit_inside(picture, canvas);
        if self.center_x == WHOLE / 2
            && self.center_y == WHOLE / 2
            && self.scale == WHOLE
            && !self.is_cropped()
        {
            return fitted;
        }
        let part = |value: i32| f64::from(value) / f64::from(WHOLE);
        let (kept_width, kept_height) = self.kept();
        let width = f64::from(fitted.width) * part(self.scale) * kept_width;
        let height = f64::from(fitted.height) * part(self.scale) * kept_height;
        Rect {
            x: (part(self.center_x) * f64::from(canvas.0) - width / 2.0) as f32,
            y: (part(self.center_y) * f64::from(canvas.1) - height / 2.0) as f32,
            width: width as f32,
            height: height as f32,
        }
    }

    /// Whether nothing under this picture can be seen. At any angle other than a quarter turn
    /// its corners leave something showing, unless it is far bigger than the canvas, which isn't
    /// worth working out.
    pub fn hides_below(self, picture: (u32, u32), canvas: (u32, u32)) -> bool {
        if self.opacity < WHOLE || self.rotation % QUARTER_TURN != 0 {
            return false;
        }
        let rect = self.rect(picture, canvas);
        if self.rotation % (QUARTER_TURN * 2) == 0 {
            return rect.covers(canvas);
        }
        let (middle_x, middle_y) = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
        Rect {
            x: middle_x - rect.height / 2.0,
            y: middle_y - rect.width / 2.0,
            width: rect.height,
            height: rect.width,
        }
        .covers(canvas)
    }

    /// The placement at a moment in the clip. It doesn't change yet, but animation will arrive
    /// here without anything that reads a placement having to change.
    pub fn at(self, _into_clip: Ticks) -> Self {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANVAS: (u32, u32) = (1920, 1080);

    #[test]
    fn a_normal_placement_is_exactly_the_fitted_place() {
        assert_eq!(
            Placement::default().rect((1280, 720), CANVAS),
            fit_inside((1280, 720), CANVAS)
        );
        assert_eq!(
            Placement::default().rect((1080, 1920), CANVAS),
            fit_inside((1080, 1920), CANVAS)
        );
    }

    #[test]
    fn half_the_scale_is_half_the_size_around_the_same_centre() {
        let placement = Placement {
            scale: WHOLE / 2,
            ..Placement::default()
        };

        assert_eq!(
            placement.rect(CANVAS, CANVAS),
            Rect {
                x: 480.0,
                y: 270.0,
                width: 960.0,
                height: 540.0,
            }
        );
    }

    #[test]
    fn the_centre_is_a_part_of_the_canvas_whatever_its_size() {
        let placement = Placement {
            center_x: WHOLE / 4,
            center_y: WHOLE * 3 / 4,
            scale: WHOLE / 2,
            ..Placement::default()
        };

        let large = placement.rect(CANVAS, CANVAS);
        let small = placement.rect(CANVAS, (960, 540));

        assert_eq!((large.x, large.y), (0.0, 540.0));
        assert_eq!((small.x, small.y), (0.0, 270.0));
        assert_eq!(small.width, large.width / 2.0);
    }

    #[test]
    fn only_an_opaque_picture_covering_the_canvas_hides_what_is_below() {
        let normal = Placement::default();
        assert!(normal.hides_below(CANVAS, CANVAS));
        assert!(!normal.hides_below((1080, 1920), CANVAS), "bars show");

        let faded = Placement {
            opacity: WHOLE - 1,
            ..normal
        };
        assert!(!faded.hides_below(CANVAS, CANVAS));

        let smaller = Placement {
            scale: WHOLE - WHOLE / 100,
            ..normal
        };
        assert!(!smaller.hides_below(CANVAS, CANVAS));

        let moved = Placement {
            center_x: WHOLE / 2 + 1000,
            ..normal
        };
        assert!(!moved.hides_below(CANVAS, CANVAS));
    }

    #[test]
    fn a_picture_an_odd_size_short_of_an_even_canvas_still_hides_what_is_below() {
        assert!(Placement::default().hides_below((853, 481), (854, 482)));
    }

    #[test]
    fn a_picture_made_big_enough_hides_below_even_in_another_shape() {
        let tall_enlarged = Placement {
            scale: WHOLE * 4,
            ..Placement::default()
        };

        assert!(tall_enlarged.hides_below((1080, 1920), CANVAS));
    }

    #[test]
    fn a_picture_turned_part_way_never_hides_what_is_below() {
        let tilted = Placement {
            rotation: 30_000,
            scale: WHOLE * 4,
            ..Placement::default()
        };

        assert!(!tilted.hides_below(CANVAS, CANVAS));
    }

    #[test]
    fn a_wide_picture_turned_upright_shows_all_of_itself() {
        let upright = Placement::default().turned_quarter(CANVAS, CANVAS);

        assert_eq!(upright.rotation, 90_000);
        assert_eq!(upright.scale, 562_500);
        assert!(!upright.hides_below(CANVAS, CANVAS), "it leaves bars");
    }

    #[test]
    fn a_tall_picture_on_a_tall_canvas_turned_twice_fills_it_again() {
        let canvas = (1080, 1920);
        let half_turn = Placement::default()
            .turned_quarter(canvas, canvas)
            .turned_quarter(canvas, canvas);

        assert_eq!(half_turn.rotation, 180_000);
        assert_eq!(half_turn.scale, WHOLE);
        assert!(half_turn.hides_below(canvas, canvas));
    }

    #[test]
    fn four_quarter_turns_come_back_exactly() {
        let mut turning = Placement::default();
        for _ in 0..4 {
            turning = turning.turned_quarter(CANVAS, CANVAS);
        }

        assert!(turning.is_default(), "{turning:?}");
    }

    #[test]
    fn a_resized_picture_keeps_its_size_when_turned() {
        let small = Placement {
            scale: WHOLE / 3,
            ..Placement::default()
        };

        assert_eq!(small.turned_quarter(CANVAS, CANVAS).scale, WHOLE / 3);
    }

    #[test]
    fn a_tilted_picture_goes_on_to_the_next_quarter() {
        let tilted = Placement {
            rotation: 30_000,
            ..Placement::default()
        };
        let nearly_round = Placement {
            rotation: 300_000,
            ..Placement::default()
        };

        assert_eq!(tilted.turned_quarter(CANVAS, CANVAS).rotation, 90_000);
        assert_eq!(nearly_round.turned_quarter(CANVAS, CANVAS).rotation, 0);
    }

    #[test]
    fn a_picture_in_picture_sits_in_its_corner_with_room_around_it() {
        for (corner, _) in CanvasCorner::ALL {
            let placed = Placement::default().in_corner(corner, CANVAS, CANVAS);
            let rect = placed.rect(CANVAS, CANVAS);

            assert_eq!(placed.scale, PICTURE_IN_PICTURE_SCALE);
            assert!(rect.x >= 40.0 && rect.y >= 40.0, "{corner:?} {rect:?}");
            assert!(rect.x + rect.width <= 1880.0, "{corner:?} {rect:?}");
            assert!(rect.y + rect.height <= 1040.0, "{corner:?} {rect:?}");
            let left = matches!(corner, CanvasCorner::TopLeft | CanvasCorner::BottomLeft);
            let top = matches!(corner, CanvasCorner::TopLeft | CanvasCorner::TopRight);
            assert_eq!(rect.x < 960.0, left, "{corner:?} {rect:?}");
            assert_eq!(rect.y < 540.0, top, "{corner:?} {rect:?}");
        }
    }

    #[test]
    fn a_turned_picture_in_picture_keeps_all_of_itself_on_the_canvas() {
        let upright = Placement {
            rotation: 90_000,
            ..Placement::default()
        };

        let placed = upright.in_corner(CanvasCorner::TopLeft, CANVAS, CANVAS);

        let rect = placed.rect(CANVAS, CANVAS);
        let (middle_x, middle_y) = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
        // It stands taller than it is wide, so it is the height that has to fit across.
        assert!(middle_x - rect.height / 2.0 >= 0.0, "{rect:?}");
        assert!(middle_y - rect.width / 2.0 >= 0.0, "{rect:?}");
        assert_eq!(placed.rotation, 90_000);
    }

    #[test]
    fn a_picture_in_picture_keeps_how_see_through_and_flipped_it_was() {
        let look = Placement {
            opacity: WHOLE / 2,
            flip_x: true,
            ..Placement::default()
        };

        let placed = look.in_corner(CanvasCorner::BottomRight, CANVAS, CANVAS);

        assert_eq!(placed.opacity, WHOLE / 2);
        assert!(placed.flip_x);
    }

    /// Half the size somewhere away from the middle, so anything that moves shows up.
    fn off_centre() -> Placement {
        Placement {
            center_x: 400_000,
            center_y: 300_000,
            scale: WHOLE / 2,
            ..Placement::default()
        }
    }

    /// Where a corner of the whole picture is on the canvas, given which way it lies.
    fn full_corner(placement: Placement, across: f64, down: f64) -> (f64, f64) {
        let full = placement.uncropped(CANVAS, CANVAS);
        let rect = full.rect(CANVAS, CANVAS);
        let (half_w, half_h) = (f64::from(rect.width) / 2.0, f64::from(rect.height) / 2.0);
        let (x, y) = (f64::from(rect.x) + half_w, f64::from(rect.y) + half_h);
        let radians = full.degrees().to_radians();
        let (reach_x, reach_y) = (across * half_w, down * half_h);
        (
            x + reach_x * radians.cos() - reach_y * radians.sin(),
            y + reach_x * radians.sin() + reach_y * radians.cos(),
        )
    }

    #[track_caller]
    fn assert_close(found: (f64, f64), wanted: (f64, f64)) {
        assert!(
            (found.0 - wanted.0).abs() < 0.1 && (found.1 - wanted.1).abs() < 0.1,
            "{found:?} is not {wanted:?}"
        );
    }

    #[test]
    fn a_cropped_picture_is_only_as_big_as_what_is_kept() {
        let cropped = Placement {
            crop_left: WHOLE / 4,
            crop_bottom: WHOLE / 2,
            ..Placement::default()
        };

        let rect = cropped.rect(CANVAS, CANVAS);

        assert_eq!(rect.width, 1440.0);
        assert_eq!(rect.height, 540.0);
    }

    #[test]
    fn cropping_leaves_what_is_kept_exactly_where_it_was() {
        let start = off_centre();
        let before = start.rect(CANVAS, CANVAS);

        let cropped = start.cropped_to(
            CropSides {
                left: 0.25,
                ..CropSides::default()
            },
            CANVAS,
            CANVAS,
        );

        let after = cropped.rect(CANVAS, CANVAS);
        // Half size is 960 wide, so a quarter of it cut from the left moves that edge in by 240.
        assert!((after.x - before.x - 240.0).abs() < 0.1, "{after:?}");
        assert!((after.width - 720.0).abs() < 0.1, "{after:?}");
        assert!((after.x + after.width - before.x - before.width).abs() < 0.1);
        assert_eq!(after.y, before.y);
    }

    #[test]
    fn the_whole_picture_stays_put_however_it_is_cropped() {
        for start in [
            off_centre(),
            Placement {
                rotation: 30_000,
                ..off_centre()
            },
            Placement {
                rotation: 90_000,
                flip_x: true,
                ..off_centre()
            },
            Placement {
                rotation: 250_000,
                flip_x: true,
                flip_y: true,
                ..off_centre()
            },
        ] {
            let sides = CropSides {
                left: 0.1,
                top: 0.2,
                right: 0.3,
                bottom: 0.05,
            };

            let cropped = start.cropped_to(sides, CANVAS, CANVAS);

            assert_close(
                full_corner(cropped, -1.0, -1.0),
                full_corner(start, -1.0, -1.0),
            );
            assert_close(full_corner(cropped, 1.0, 1.0), full_corner(start, 1.0, 1.0));
            assert_eq!(cropped.scale, start.scale, "cropping doesn't resize");
        }
    }

    #[test]
    fn uncropping_after_cropping_comes_back_where_it_began() {
        let start = Placement {
            rotation: 45_000,
            ..off_centre()
        };

        let back = start
            .cropped_to(
                CropSides {
                    left: 0.3,
                    bottom: 0.2,
                    ..CropSides::default()
                },
                CANVAS,
                CANVAS,
            )
            .uncropped(CANVAS, CANVAS);

        assert!((back.center_x - start.center_x).abs() <= 2, "{back:?}");
        assert!((back.center_y - start.center_y).abs() <= 2, "{back:?}");
        assert!(!back.is_cropped());
    }

    #[test]
    fn a_flipped_pictures_crop_goes_by_what_is_seen() {
        let flipped = Placement {
            flip_x: true,
            flip_y: true,
            ..Placement::default()
        };

        let cropped = flipped.cropped_to(
            CropSides {
                left: 0.25,
                top: 0.1,
                ..CropSides::default()
            },
            CANVAS,
            CANVAS,
        );

        assert_eq!(cropped.crop_right, WHOLE / 4, "seen on the left");
        assert_eq!(cropped.crop_left, 0);
        assert_eq!(cropped.crop_bottom, WHOLE / 10, "seen on the top");
        assert_eq!(cropped.crop_sides().left, 0.25);
        assert_eq!(cropped.crop_sides().top, 0.1);
    }

    #[test]
    fn crop_shows_the_right_part_of_the_source() {
        let cropped = Placement {
            crop_left: WHOLE / 4,
            crop_top: WHOLE / 2,
            crop_right: WHOLE / 4,
            ..Placement::default()
        };

        assert_eq!(cropped.look().source, [0.25, 0.5, 0.75, 1.0]);
        assert!(Placement::default().look().is_plain());
        assert!(!cropped.look().is_plain());
    }

    #[test]
    fn a_crop_can_never_leave_nothing() {
        let cut_away = Placement {
            crop_left: WHOLE,
            crop_right: WHOLE,
            crop_top: WHOLE * 2,
            crop_bottom: -5,
            ..Placement::default()
        }
        .clamped();

        assert_eq!(cut_away.crop_left, WHOLE - SMALLEST_KEPT_BY_CROP);
        assert_eq!(cut_away.crop_right, 0);
        assert_eq!(cut_away.crop_top, WHOLE - SMALLEST_KEPT_BY_CROP);
        assert_eq!(cut_away.crop_bottom, 0);
        let (width, height) = cut_away.kept();
        assert!(width > 0.0 && height > 0.0);
    }

    #[test]
    fn a_placement_from_before_cropping_reads_as_not_cropped() {
        let text = r#"{"center_x":100,"center_y":200,"scale":300,"opacity":400,"rotation":500}"#;

        let placement: Placement = serde_json::from_str(text).expect("it can be read");

        assert!(!placement.is_cropped());
        assert_eq!(placement.rotation, 500);
    }

    #[test]
    fn a_cropped_picture_fitting_the_canvas_goes_on_fitting_it_when_turned() {
        // What is kept is square, so it should stay the height of the canvas at every turn.
        let square_part = Placement {
            crop_left: 218_750,
            crop_right: 218_750,
            ..Placement::default()
        };

        let turned = square_part.turned_quarter(CANVAS, CANVAS);

        assert!((turned.scale - WHOLE).abs() < 10, "{turned:?}");
    }

    #[test]
    fn flipping_twice_changes_nothing() {
        let turned = Placement {
            rotation: 30_000,
            ..Placement::default()
        };

        assert_eq!(turned.flipped_across().flipped_across(), turned);
        assert_eq!(turned.flipped_down().flipped_down(), turned);
    }

    #[test]
    fn flipping_a_turned_picture_turns_it_the_other_way() {
        let turned = Placement {
            rotation: 30_000,
            ..Placement::default()
        };

        let flipped = turned.flipped_across();

        assert!(flipped.flip_x);
        assert_eq!(flipped.rotation, 330_000);
    }

    #[test]
    fn a_placement_from_before_rotation_reads_as_not_turned() {
        let text = r#"{"center_x":100,"center_y":200,"scale":300,"opacity":400}"#;

        let placement: Placement = serde_json::from_str(text).expect("it can be read");

        assert_eq!(placement.center_x, 100);
        assert_eq!(placement.rotation, 0);
        assert!(!placement.flip_x && !placement.flip_y);
    }

    #[test]
    fn a_rotation_out_of_range_comes_back_in() {
        let wound = Placement {
            rotation: FULL_TURN + 5,
            ..Placement::default()
        };
        let backwards = Placement {
            rotation: -90_000,
            ..Placement::default()
        };

        assert_eq!(wound.clamped().rotation, 5);
        assert_eq!(backwards.clamped().rotation, 270_000);
    }

    #[test]
    fn anything_out_of_range_comes_back_in() {
        let wild = Placement {
            center_x: i32::MAX,
            center_y: i32::MIN,
            scale: 1,
            opacity: WHOLE * 9,
            ..Placement::default()
        }
        .clamped();

        assert_eq!(wild.scale, SMALLEST_SCALE);
        assert_eq!(wild.opacity, WHOLE);
        assert!(wild.center_x < i32::MAX && wild.center_y > i32::MIN);
        assert_eq!(
            Placement {
                scale: WHOLE * 100,
                ..Placement::default()
            }
            .clamped()
            .scale,
            LARGEST_SCALE
        );
    }
}
