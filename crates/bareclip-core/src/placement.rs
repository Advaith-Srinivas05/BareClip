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
}

/// How a picture is drawn in its place, beyond where that place is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PictureLook {
    /// Clockwise, around the place's middle.
    pub degrees: f32,
    pub flip_x: bool,
    pub flip_y: bool,
}

impl PictureLook {
    pub const PLAIN: Self = Self {
        degrees: 0.0,
        flip_x: false,
        flip_y: false,
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
            ..self
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
        }
    }

    /// The scale at which a picture just fits the canvas when turned this far by quarter turns.
    fn fit_scale(picture: (u32, u32), canvas: (u32, u32), rotation: i32) -> f64 {
        let fitted = fit_inside(picture, canvas);
        let (width, height) = (f64::from(fitted.width), f64::from(fitted.height));
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
            && (scale_now - Self::fit_scale(picture, canvas, self.rotation)).abs() <= CLOSE_TO_FIT;
        let scale = if was_fitting {
            let fit = Self::fit_scale(picture, canvas, rotation);
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
        if self.center_x == WHOLE / 2 && self.center_y == WHOLE / 2 && self.scale == WHOLE {
            return fitted;
        }
        let part = |value: i32| f64::from(value) / f64::from(WHOLE);
        let width = f64::from(fitted.width) * part(self.scale);
        let height = f64::from(fitted.height) * part(self.scale);
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
