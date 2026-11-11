//! Draws the library's items into see-through pictures, on the processor. Shapes and annotations
//! are drawn from their outlines at the size they show, so they stay sharp however big they are.
//! Frames, overlays and backgrounds cover the canvas, and soft ones are drawn smaller and shown
//! larger, because softness hides it and it saves a lot of time.

use std::f32::consts::{PI, TAU};

use bareclip_core::{
    art::{Art, ArtFrame, ArtItem, ArtKind, Natural},
    canvas::Colour,
    picture::StillPicture,
    placement::WHOLE,
    title::{Title, scale_for_pixels},
};
use zeno::{Cap, Command, Fill, Join, Mask, Scratch, Stroke};

use crate::draw::TitleDrawing;

/// The largest picture drawn, so that it always fits on the GPU.
const LARGEST_SIDE: f32 = 4000.0;
/// How many points a whole turn of a curve is drawn with.
const TURN_STEPS: usize = 96;
/// How much bigger than at rest a frame is drawn for a shape that stretches as it comes in, so it
/// has room to.
const STRETCH_ROOM: f32 = 1.3;
/// Lines are thicknesses in two-hundredths of the longer side of what they belong to.
const LINE_PARTS: f32 = 200.0;
/// How far a sharp outline's corner may reach, in half lines, before it is cut flat, which is the
/// usual limit for drawing programs.
const SHARPEST_CORNER: f32 = 4.0;
/// How long one sweep of an overlay's light takes, in seconds.
const LIGHT_CYCLE: f32 = 6.0;

type Point = (f32, f32);

/// One run of joined points.
#[derive(Clone, Debug, PartialEq)]
struct Run {
    points: Vec<Point>,
    closed: bool,
}

impl Run {
    fn closed(points: Vec<Point>) -> Self {
        Self {
            points,
            closed: true,
        }
    }

    fn open(points: Vec<Point>) -> Self {
        Self {
            points,
            closed: false,
        }
    }

    /// The points in order, with the first again at the end if it goes round.
    fn path(&self) -> Vec<Point> {
        let mut points = self.points.clone();
        if self.closed
            && let Some(first) = self.points.first()
        {
            points.push(*first);
        }
        points
    }

    fn length(&self) -> f32 {
        self.path()
            .windows(2)
            .map(|pair| (pair[1].0 - pair[0].0).hypot(pair[1].1 - pair[0].1))
            .sum()
    }
}

/// An outline in its own box, whose longer side is 1.
#[derive(Clone, Debug)]
struct Outline {
    width: f32,
    height: f32,
    runs: Vec<Run>,
}

/// Scales runs drawn anywhere so the longer side of all of them is 1, starting at 0.
fn fitted(runs: Vec<Run>) -> Outline {
    let (mut left, mut top, mut right, mut bottom) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for point in runs.iter().flat_map(|run| &run.points) {
        left = left.min(point.0);
        top = top.min(point.1);
        right = right.max(point.0);
        bottom = bottom.max(point.1);
    }
    let longer = (right - left).max(bottom - top).max(1e-6);
    let runs = runs
        .into_iter()
        .map(|run| Run {
            points: run
                .points
                .iter()
                .map(|(x, y)| ((x - left) / longer, (y - top) / longer))
                .collect(),
            closed: run.closed,
        })
        .collect();
    Outline {
        width: ((right - left) / longer).max(0.02),
        height: ((bottom - top) / longer).max(0.02),
        runs,
    }
}

/// Points round an ellipse, clockwise on screen from `start` degrees, where -90 is the top.
fn ellipse(middle: Point, radii: Point, start: f32, turns: f32) -> Vec<Point> {
    let steps = ((TURN_STEPS as f32 * turns.abs()).ceil() as usize).max(8);
    (0..steps)
        .map(|step| {
            let angle = (start + 360.0 * turns * step as f32 / steps as f32).to_radians();
            (
                middle.0 + radii.0 * angle.cos(),
                middle.1 + radii.1 * angle.sin(),
            )
        })
        .collect()
}

fn circle(middle: Point, radius: f32) -> Vec<Point> {
    ellipse(middle, (radius, radius), -90.0, 1.0)
}

/// The corners of a regular shape, the first at the top, going clockwise.
fn polygon(sides: usize) -> Vec<Point> {
    (0..sides)
        .map(|side| {
            let angle = -PI / 2.0 + TAU * side as f32 / sides as f32;
            (angle.cos(), angle.sin())
        })
        .collect()
}

/// A star's points and the corners between them, the first point at the top.
fn star(points: usize, inner: f32) -> Vec<Point> {
    (0..points * 2)
        .map(|index| {
            let angle = -PI / 2.0 + PI * index as f32 / points as f32;
            let reach = if index % 2 == 0 { 1.0 } else { inner };
            (reach * angle.cos(), reach * angle.sin())
        })
        .collect()
}

/// A box's corners, going clockwise from the middle of its top.
fn rectangle(left: f32, top: f32, width: f32, height: f32) -> Vec<Point> {
    vec![
        (left + width / 2.0, top),
        (left + width, top),
        (left + width, top + height),
        (left, top + height),
        (left, top),
    ]
}

/// Rounds every corner of a shape that goes round, by `radius` in its own units.
fn rounded(corners: &[Point], radius: f32) -> Vec<Point> {
    let count = corners.len();
    let mut points = Vec::with_capacity(count * 12);
    for index in 0..count {
        let before = corners[(index + count - 1) % count];
        let corner = corners[index];
        let after = corners[(index + 1) % count];
        let towards = |to: Point| {
            let (x, y) = (to.0 - corner.0, to.1 - corner.1);
            let length = x.hypot(y).max(1e-6);
            let reach = radius.min(length / 2.0);
            (corner.0 + x / length * reach, corner.1 + y / length * reach)
        };
        let (from, to) = (towards(before), towards(after));
        if (from.0 - to.0).hypot(from.1 - to.1) < 1e-5 {
            points.push(corner);
            continue;
        }
        // A curve through the corner's neighbourhood, drawn as points.
        for step in 0..=10 {
            let t = step as f32 / 10.0;
            let (a, b, c) = ((1.0 - t) * (1.0 - t), 2.0 * (1.0 - t) * t, t * t);
            points.push((
                a * from.0 + b * corner.0 + c * to.0,
                a * from.1 + b * corner.1 + c * to.1,
            ));
        }
    }
    points
}

/// A rectangle with its corners rounded by `radius`.
fn rounded_box(width: f32, height: f32, radius: f32) -> Vec<Point> {
    rounded(&rectangle(0.0, 0.0, width, height)[1..], radius).rotated_to_top()
}

/// An ellipse's points with a speech bubble's tail taking the place of those along its lower edge
/// between `from` and `to` across.
fn with_tail(points: Vec<Point>, middle_y: f32, from: f32, to: f32, tip: Point) -> Vec<Point> {
    let lower = |point: &Point| point.1 > middle_y;
    let start = points.iter().position(|p| lower(p) && p.0 < from);
    let end = points.iter().position(|p| lower(p) && p.0 < to);
    match (start, end) {
        (Some(start), Some(end)) if end > start => {
            let mut points = points;
            points.splice(start..end, [tip]);
            points
        }
        _ => points,
    }
}

trait FromTheTop {
    /// The same points, starting from the one nearest the middle of the top, so something drawn
    /// round starts there.
    fn rotated_to_top(self) -> Self;
}

impl FromTheTop for Vec<Point> {
    fn rotated_to_top(mut self) -> Self {
        let Some(top) = self.iter().map(|point| point.1).reduce(f32::min) else {
            return self;
        };
        let left = self.iter().map(|point| point.0).fold(f32::MAX, f32::min);
        let right = self.iter().map(|point| point.0).fold(f32::MIN, f32::max);
        let middle = (left + right) / 2.0;
        let start = self
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let score = |p: &Point| (p.0 - middle).abs() + (p.1 - top).abs() * 4.0;
                score(a).total_cmp(&score(b))
            })
            .map_or(0, |(index, _)| index);
        self.rotate_left(start);
        self
    }
}

/// A shape round its middle whose reach changes with the angle.
fn polar(reach: impl Fn(f32) -> f32) -> Vec<Point> {
    (0..TURN_STEPS * 2)
        .map(|step| {
            let angle = -PI / 2.0 + TAU * step as f32 / (TURN_STEPS * 2) as f32;
            let r = reach(angle);
            (r * angle.cos(), r * angle.sin())
        })
        .collect()
}

/// A small, smooth, repeatable wobble, so lines look drawn by hand.
fn shaky(points: &[Point], amount: f32, seed: u32) -> Vec<Point> {
    let phase = |n: u32| noise(seed.wrapping_mul(7).wrapping_add(n)) * TAU;
    let count = points.len().max(1) as f32;
    points
        .iter()
        .enumerate()
        .map(|(index, (x, y))| {
            let t = index as f32 / count * TAU;
            let dx = (t * 3.0 + phase(1)).sin() * 0.6 + (t * 7.0 + phase(2)).sin() * 0.4;
            let dy = (t * 4.0 + phase(3)).sin() * 0.6 + (t * 9.0 + phase(4)).sin() * 0.4;
            (x + dx * amount, y + dy * amount)
        })
        .collect()
}

/// A number from 0 to 1 that always comes out the same for the same seed.
fn noise(seed: u32) -> f32 {
    let mut state = seed.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    state = ((state >> ((state >> 28) + 4)) ^ state).wrapping_mul(277_803_737);
    state = (state >> 22) ^ state;
    state as f32 / u32::MAX as f32
}

/// The points of a curve through control points, drawn as a smooth line.
fn bezier(points: [Point; 4], steps: usize) -> Vec<Point> {
    (0..=steps)
        .map(|step| {
            let t = step as f32 / steps as f32;
            let u = 1.0 - t;
            let weights = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
            let mut point = (0.0, 0.0);
            for (weight, control) in weights.iter().zip(points) {
                point.0 += weight * control.0;
                point.1 += weight * control.1;
            }
            point
        })
        .collect()
}

/// An arrowhead's two strokes at the end of a line that arrives going `angle` degrees.
fn arrowhead(tip: Point, angle: f32, length: f32) -> Vec<Run> {
    [150.0_f32, -150.0]
        .iter()
        .map(|turn| {
            let back = (angle + turn).to_radians();
            Run::open(vec![
                tip,
                (tip.0 + length * back.cos(), tip.1 + length * back.sin()),
            ])
        })
        .collect()
}

/// The outline of a shape or annotation, in its own box.
fn outline_of(key: &str) -> Outline {
    let closed = |points: Vec<Point>| fitted(vec![Run::closed(points)]);
    let base = key.strip_prefix("outline-").unwrap_or(key);
    match base {
        "square" => closed(rectangle(0.0, 0.0, 1.0, 1.0)),
        "rectangle" => closed(rectangle(0.0, 0.0, 1.0, 0.6)),
        "circle" => closed(circle((0.0, 0.0), 1.0)),
        "oval" => closed(ellipse((0.0, 0.0), (1.0, 0.65), -90.0, 1.0)),
        "triangle" => closed(vec![(0.5, 0.0), (1.0, 0.866), (0.0, 0.866)]),
        "right-triangle" => closed(vec![(1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]),
        "diamond" => closed(vec![(0.5, 0.0), (1.0, 0.5), (0.5, 1.0), (0.0, 0.5)]),
        "pentagon" => closed(polygon(5)),
        "hexagon" => closed(polygon(6)),
        "octagon" => closed(
            polygon(8)
                .into_iter()
                .map(|(x, y)| rotate((x, y), 22.5))
                .collect::<Vec<_>>()
                .rotated_to_top(),
        ),
        "pill" => closed(rounded_box(1.0, 0.45, 0.225)),
        "arch" => {
            let mut points = ellipse((0.35, 0.35), (0.35, 0.35), -90.0, 0.25);
            points.extend([(0.7, 0.35), (0.7, 1.0), (0.0, 1.0), (0.0, 0.35)]);
            points.extend(ellipse((0.35, 0.35), (0.35, 0.35), 180.0, 0.25));
            closed(points)
        }
        "line" => closed(rectangle(0.0, 0.0, 1.0, 0.03)),
        "parallelogram" => closed(vec![(0.25, 0.0), (1.0, 0.0), (0.75, 0.5), (0.0, 0.5)]),
        "star" => closed(star(5, 0.42)),
        "sparkle" => closed(star(4, 0.22)),
        "six-star" => closed(star(6, 0.55)),
        "burst" => closed(star(12, 0.78)),
        "semicircle" => {
            let mut points = ellipse((0.0, 0.0), (1.0, 1.0), 180.0, 0.5);
            points.push((1.0, 0.0));
            closed(points.rotated_to_top())
        }
        "quarter-circle" => {
            let mut points = ellipse((0.0, 1.0), (1.0, 1.0), -90.0, 0.25);
            points.extend([(1.0, 1.0), (0.0, 1.0)]);
            closed(points)
        }
        "heart" => {
            let mut points = bezier([(0.5, 0.25), (0.5, -0.05), (1.05, -0.05), (1.0, 0.35)], 24);
            points.extend(bezier(
                [(1.0, 0.35), (0.95, 0.6), (0.6, 0.75), (0.5, 0.92)],
                24,
            ));
            points.extend(bezier(
                [(0.5, 0.92), (0.4, 0.75), (0.05, 0.6), (0.0, 0.35)],
                24,
            ));
            points.extend(bezier(
                [(0.0, 0.35), (-0.05, -0.05), (0.5, -0.05), (0.5, 0.25)],
                24,
            ));
            closed(points)
        }
        "plus" => closed(vec![
            (0.33, 0.0),
            (0.67, 0.0),
            (0.67, 0.33),
            (1.0, 0.33),
            (1.0, 0.67),
            (0.67, 0.67),
            (0.67, 1.0),
            (0.33, 1.0),
            (0.33, 0.67),
            (0.0, 0.67),
            (0.0, 0.33),
            (0.33, 0.33),
        ]),
        "rounded-square" => closed(rounded_box(1.0, 1.0, 0.2)),
        "rounded-rectangle" => closed(rounded_box(1.0, 0.6, 0.15)),
        "squircle" => closed(polar(|angle| {
            let (c, s) = (angle.cos().abs(), angle.sin().abs());
            1.0 / (c.powi(4) + s.powi(4)).powf(0.25)
        })),
        "rounded-triangle" => closed(rounded(&[(0.5, 0.0), (1.0, 0.866), (0.0, 0.866)], 0.15)),
        "rounded-diamond" => closed(rounded(
            &[(0.5, 0.0), (1.0, 0.5), (0.5, 1.0), (0.0, 0.5)],
            0.12,
        )),
        "rounded-pentagon" => closed(rounded(&polygon(5), 0.25)),
        "arrow" => closed(vec![
            (0.0, 0.18),
            (0.6, 0.18),
            (0.6, 0.0),
            (1.0, 0.3),
            (0.6, 0.6),
            (0.6, 0.42),
            (0.0, 0.42),
        ]),
        "thin-arrow" => closed(vec![
            (0.0, 0.135),
            (0.82, 0.135),
            (0.78, 0.04),
            (1.0, 0.15),
            (0.78, 0.26),
            (0.82, 0.165),
            (0.0, 0.165),
        ]),
        "double-arrow" => closed(vec![
            (0.0, 0.25),
            (0.28, 0.0),
            (0.28, 0.15),
            (0.72, 0.15),
            (0.72, 0.0),
            (1.0, 0.25),
            (0.72, 0.5),
            (0.72, 0.35),
            (0.28, 0.35),
            (0.28, 0.5),
        ]),
        "chevron" => closed(vec![
            (0.0, 0.0),
            (0.6, 0.0),
            (1.0, 0.3),
            (0.6, 0.6),
            (0.0, 0.6),
            (0.4, 0.3),
        ]),
        "curved-arrow" => {
            // A thick band round the top of a circle, with a head at its right-hand end.
            let at = |degrees: f32, reach: f32| {
                let angle = degrees.to_radians();
                (reach * angle.cos(), reach * angle.sin())
            };
            let mut points = ellipse((0.0, 0.0), (0.5, 0.5), 200.0, 1.0 / 3.0);
            points.extend([at(320.0, 0.58), at(352.0, 0.41), at(320.0, 0.24)]);
            points.extend(
                ellipse((0.0, 0.0), (0.32, 0.32), 200.0, 1.0 / 3.0)
                    .into_iter()
                    .rev(),
            );
            closed(points)
        }
        "tag" => closed(vec![
            (0.0, 0.0),
            (0.75, 0.0),
            (1.0, 0.25),
            (0.75, 0.5),
            (0.0, 0.5),
        ]),
        "speech-box" => {
            let mut points = rounded(&rectangle(0.0, 0.0, 1.0, 0.65)[1..], 0.1);
            // The tail goes in along the bottom edge, between the bottom corners.
            let bottom = points
                .iter()
                .position(|point| point.1 > 0.64 && point.0 < 0.5)
                .unwrap_or(0);
            points.splice(bottom..bottom, [(0.35, 0.65), (0.22, 0.85), (0.24, 0.65)]);
            closed(points.rotated_to_top())
        }
        "speech-bubble" => closed(with_tail(
            ellipse((0.5, 0.4), (0.5, 0.36), -90.0, 1.0),
            0.4,
            0.42,
            0.26,
            (0.25, 0.87),
        )),
        "banner" => closed(vec![
            (0.0, 0.0),
            (1.0, 0.0),
            (0.92, 0.2),
            (1.0, 0.4),
            (0.0, 0.4),
            (0.08, 0.2),
        ]),
        "flower" => petals(6, 0.27, 0.22, 0.25),
        "eight-petals" => petals(8, 0.3, 0.17, 0.22),
        "daisy" => long_petals(12, 0.5, 0.09, 0.2),
        "scallop" => closed(polar(|angle| 0.44 + 0.06 * (angle * 6.0).cos().abs())),
        "clover" => petals(4, 0.25, 0.25, 0.15),
        "asterisk" => fitted(
            (0..3)
                .map(|bar| {
                    let corners = rectangle(-0.5, -0.09, 1.0, 0.18);
                    Run::closed(
                        rounded(&corners[1..], 0.09)
                            .into_iter()
                            .map(|point| rotate(point, 60.0 * bar as f32 + 90.0))
                            .collect(),
                    )
                })
                .collect(),
        ),
        "starburst" => closed(star(16, 0.72)),
        "cog" => {
            let teeth = polar(|angle| {
                let tooth = ((angle * 8.0 / TAU + 0.25).rem_euclid(1.0) - 0.5).abs();
                if tooth < 0.25 { 0.5 } else { 0.4 }
            });
            let hole = circle((0.0, 0.0), 0.16).into_iter().rev().collect();
            fitted(vec![Run::closed(teeth), Run::closed(hole)])
        }
        "blob" => closed(polar(|a| {
            0.5 + 0.08 * (2.0 * a + 1.0).sin() + 0.05 * (3.0 * a).cos()
        })),
        "puddle" => closed(
            polar(|a| 0.5 + 0.06 * (3.0 * a + 0.4).sin() + 0.04 * (5.0 * a).cos())
                .into_iter()
                .map(|(x, y)| (x * 1.4, y * 0.8))
                .collect(),
        ),
        "cloud" => fitted(
            [
                ((0.3, 0.55), 0.22),
                ((0.55, 0.4), 0.28),
                ((0.8, 0.55), 0.2),
                ((0.55, 0.62), 0.22),
            ]
            .iter()
            .map(|(middle, radius)| Run::closed(circle(*middle, *radius)))
            .collect(),
        ),
        "pebble" => closed(
            polar(|a| 0.5 + 0.03 * (2.0 * a).sin() + 0.02 * (3.0 * a + 1.0).cos())
                .into_iter()
                .map(|(x, y)| (x * 1.25, y))
                .collect(),
        ),
        "leaf" => {
            let mut points = bezier([(0.0, 1.0), (0.0, 0.3), (0.4, 0.0), (1.0, 0.0)], 32);
            points.extend(bezier([(1.0, 0.0), (1.0, 0.6), (0.7, 1.0), (0.0, 1.0)], 32));
            closed(points.rotated_to_top())
        }
        "drop" => {
            let mut points = bezier([(0.5, 0.0), (0.6, 0.25), (1.0, 0.5), (1.0, 0.75)], 24);
            points.extend(
                ellipse((0.5, 0.75), (0.5, 0.5), 0.0, 0.5)
                    .into_iter()
                    .skip(1),
            );
            points.extend(bezier(
                [(0.0, 0.75), (0.0, 0.5), (0.4, 0.25), (0.5, 0.0)],
                24,
            ));
            closed(points)
        }
        "rough-oval" => fitted(vec![Run::open(shaky(
            &ellipse((0.0, 0.0), (1.0, 0.6), -120.0, 1.12),
            0.03,
            3,
        ))]),
        "double-oval" => fitted(vec![Run::open(shaky(
            &polar_turns(2.0, |turn| (1.0 - 0.07 * turn, 0.6 - 0.05 * turn)),
            0.025,
            5,
        ))]),
        "wavy-line" => fitted(vec![Run::open(
            (0..=96)
                .map(|step| {
                    let x = step as f32 / 96.0;
                    (x, 0.1 * (x * TAU * 2.0).sin())
                })
                .collect(),
        )]),
        "zigzag" => fitted(vec![Run::open(
            (0..=8)
                .map(|step| (step as f32 / 8.0, if step % 2 == 0 { 0.0 } else { 0.2 }))
                .collect(),
        )]),
        "swoosh" => fitted(vec![Run::open(shaky(
            &bezier([(0.0, 0.3), (0.3, 0.32), (0.6, 0.25), (1.0, 0.0)], 48),
            0.006,
            9,
        ))]),
        "hatching" => fitted(
            (0..7)
                .map(|line| {
                    let x = line as f32 * 0.16;
                    Run::open(shaky(&[(x, 1.0), (x + 0.5, 0.0)], 0.01, line))
                })
                .collect(),
        ),
        "checks" | "dots" | "grid" => fitted(pattern_cells(base, 0.0..1.0)),
        // Annotations, drawn by hand.
        "marker-circle" => fitted(vec![Run::open(shaky(
            &polar_turns(1.15, |turn| (1.0 - 0.08 * turn, 0.75 - 0.1 * turn)),
            0.02,
            11,
        ))]),
        "marker-oval" => fitted(vec![Run::open(shaky(
            &polar_turns(1.1, |turn| (1.0 - 0.05 * turn, 0.42 - 0.04 * turn)),
            0.015,
            12,
        ))]),
        "double-circle" => fitted(vec![Run::open(shaky(
            &polar_turns(2.05, |turn| (1.0 - 0.06 * turn, 0.85 - 0.06 * turn)),
            0.02,
            13,
        ))]),
        "bubble-round" => {
            let mut points = ellipse((0.5, 0.4), (0.5, 0.36), 120.0, 0.93);
            points.extend([(0.18, 0.92), (0.33, 0.73)]);
            fitted(vec![Run::open(shaky(&points, 0.008, 14))])
        }
        "bubble-box" => {
            let mut points = rounded(&rectangle(0.0, 0.0, 1.0, 0.65)[1..], 0.1);
            let bottom = points
                .iter()
                .position(|point| point.1 > 0.64 && point.0 < 0.5)
                .unwrap_or(0);
            points.splice(bottom..bottom, [(0.35, 0.65), (0.2, 0.88), (0.22, 0.65)]);
            fitted(vec![Run::closed(shaky(&points, 0.006, 15))])
        }
        "marker-arrow" => {
            let curve = bezier([(0.0, 0.6), (0.2, 0.1), (0.7, 0.0), (1.0, 0.35)], 48);
            let mut runs = vec![Run::open(shaky(&curve, 0.006, 16))];
            runs.extend(arrowhead((1.0, 0.35), 50.0, 0.2));
            fitted(runs)
        }
        "straight-arrow" => {
            let mut runs = vec![Run::open(shaky(&[(0.0, 0.2), (1.0, 0.18)], 0.006, 17))];
            runs.extend(arrowhead((1.0, 0.18), 0.0, 0.2));
            fitted(runs)
        }
        "loop-arrow" => {
            let curve = ellipse((0.5, 0.5), (0.45, 0.45), -60.0, 0.82);
            let end = *curve.last().unwrap_or(&(0.0, 0.0));
            let mut runs = vec![Run::open(shaky(&curve, 0.006, 18))];
            runs.extend(arrowhead(end, 175.0, 0.18));
            fitted(runs)
        }
        "tick" => fitted(vec![Run::open(shaky(
            &[(0.0, 0.5), (0.35, 0.85), (1.0, 0.0)],
            0.008,
            19,
        ))]),
        "cross" => fitted(vec![
            Run::open(shaky(&[(0.0, 0.0), (1.0, 1.0)], 0.01, 20)),
            Run::open(shaky(&[(1.0, 0.0), (0.0, 1.0)], 0.01, 21)),
        ]),
        "ticked-box" => fitted(vec![
            Run::closed(shaky(&rectangle(0.0, 0.2, 0.8, 0.8), 0.006, 22)),
            Run::open(shaky(&[(0.15, 0.55), (0.38, 0.8), (1.0, 0.0)], 0.008, 23)),
        ]),
        "burst-lines" => fitted(
            (0..10)
                .map(|line| {
                    let angle = TAU * line as f32 / 10.0 - PI / 2.0;
                    let (c, s) = (angle.cos(), angle.sin());
                    Run::open(vec![(c * 0.55, s * 0.55), (c, s)])
                })
                .collect(),
        ),
        "drawn-star" => {
            let corners = polygon(5);
            let order = [0, 2, 4, 1, 3, 0];
            let points: Vec<Point> = order.iter().map(|index| corners[*index]).collect();
            fitted(vec![Run::open(shaky(&points, 0.01, 24))])
        }
        "underline" => fitted(vec![Run::open(shaky(
            &bezier([(0.0, 0.06), (0.3, 0.0), (0.7, 0.08), (1.0, 0.02)], 32),
            0.004,
            25,
        ))]),
        "double-underline" => fitted(vec![
            Run::open(shaky(&[(0.0, 0.05), (1.0, 0.0)], 0.004, 26)),
            Run::open(shaky(&[(0.08, 0.2), (0.95, 0.17)], 0.004, 27)),
        ]),
        "squiggle" => fitted(vec![Run::open(
            (0..=160)
                .map(|step| {
                    let t = step as f32 / 160.0;
                    let angle = t * TAU * 5.0;
                    (t + 0.05 * angle.cos(), 0.12 * angle.sin())
                })
                .collect(),
        )]),
        "drawn-box" => fitted(vec![Run::open(shaky(
            &[
                (0.02, 0.0),
                (1.0, 0.03),
                (0.98, 0.7),
                (0.0, 0.68),
                (0.03, -0.04),
            ],
            0.006,
            28,
        ))]),
        "brackets" => fitted(vec![
            Run::open(shaky(
                &[(0.15, 0.0), (0.0, 0.0), (0.0, 0.6), (0.15, 0.6)],
                0.005,
                29,
            )),
            Run::open(shaky(
                &[(0.85, 0.0), (1.0, 0.0), (1.0, 0.6), (0.85, 0.6)],
                0.005,
                30,
            )),
        ]),
        _ => closed(rectangle(0.0, 0.0, 1.0, 1.0)),
    }
}

/// A loop that goes round `turns` times from the upper left, its two reaches changing as it goes.
fn polar_turns(turns: f32, reach: impl Fn(f32) -> Point) -> Vec<Point> {
    let steps = (TURN_STEPS as f32 * turns) as usize;
    (0..=steps)
        .map(|step| {
            let turn = turns * step as f32 / steps as f32;
            let angle = (-130.0 + 360.0 * turn).to_radians();
            let (x, y) = reach(turn);
            (x * angle.cos(), y * angle.sin())
        })
        .collect()
}

fn rotate((x, y): Point, degrees: f32) -> Point {
    let (s, c) = degrees.to_radians().sin_cos();
    (x * c - y * s, x * s + y * c)
}

/// Round petals around a middle, all going the same way round so they join into one.
fn petals(count: usize, reach: f32, radius: f32, middle: f32) -> Outline {
    let mut runs: Vec<Run> = (0..count)
        .map(|petal| {
            let angle = -PI / 2.0 + TAU * petal as f32 / count as f32;
            Run::closed(circle((reach * angle.cos(), reach * angle.sin()), radius))
        })
        .collect();
    runs.push(Run::closed(circle((0.0, 0.0), middle)));
    fitted(runs)
}

/// Long thin petals, for a daisy.
fn long_petals(count: usize, reach: f32, width: f32, middle: f32) -> Outline {
    let mut runs: Vec<Run> = (0..count)
        .map(|petal| {
            let degrees = 360.0 * petal as f32 / count as f32;
            Run::closed(
                ellipse((reach / 2.0, 0.0), (reach / 2.0, width), -90.0, 1.0)
                    .into_iter()
                    .map(|point| rotate(point, degrees - 90.0))
                    .collect(),
            )
        })
        .collect();
    runs.push(Run::closed(circle((0.0, 0.0), middle)));
    fitted(runs)
}

/// The cells of a pattern whose middles fall in a stretch of its width.
fn pattern_cells(key: &str, across: std::ops::Range<f32>) -> Vec<Run> {
    const CELLS: usize = 8;
    let size = 1.0 / CELLS as f32;
    let mut runs = Vec::new();
    for row in 0..CELLS {
        for column in 0..CELLS {
            let (x, y) = (column as f32 * size, row as f32 * size);
            let middle = x + size / 2.0;
            if !across.contains(&middle) {
                continue;
            }
            match key {
                "checks" if (row + column) % 2 == 0 => {
                    runs.push(Run::closed(rectangle(x, y, size, size)));
                }
                "dots" => runs.push(Run::closed(circle((middle, y + size / 2.0), size * 0.32))),
                "grid" => {
                    let line = size * 0.12;
                    runs.push(Run::closed(rectangle(x, y, size, line)));
                    runs.push(Run::closed(rectangle(x, y, line, size)));
                    if column == CELLS - 1 {
                        runs.push(Run::closed(rectangle(x + size - line, y, line, size)));
                    }
                    if row == CELLS - 1 {
                        runs.push(Run::closed(rectangle(x, y + size - line, size, line)));
                    }
                }
                _ => {}
            }
        }
    }
    // Every pattern has the same box, whichever cells are showing.
    runs.push(Run::open(vec![(0.0, 0.0)]));
    runs.push(Run::open(vec![(1.0, 1.0)]));
    runs
}

/// The part of a set of runs from `from` to `to`, as shares of their whole length, taking each
/// run in turn.
fn part_of(runs: &[Run], from: f32, to: f32) -> Vec<Run> {
    if from <= 0.0 && to >= 1.0 {
        return runs.to_vec();
    }
    let total: f32 = runs.iter().map(Run::length).sum();
    let (start, end) = (from.max(0.0) * total, to.min(1.0) * total);
    let mut gone = 0.0;
    let mut kept = Vec::new();
    for run in runs {
        let path = run.path();
        let mut points = Vec::new();
        for pair in path.windows(2) {
            let length = (pair[1].0 - pair[0].0).hypot(pair[1].1 - pair[0].1);
            let (a, b) = (gone, gone + length);
            gone = b;
            if b <= start || a >= end || length <= 0.0 {
                continue;
            }
            let at = |distance: f32| {
                let t = ((distance - a) / length).clamp(0.0, 1.0);
                (
                    pair[0].0 + (pair[1].0 - pair[0].0) * t,
                    pair[0].1 + (pair[1].1 - pair[0].1) * t,
                )
            };
            if points.is_empty() {
                points.push(at(start.max(a)));
            }
            points.push(at(end.min(b)));
        }
        if points.len() >= 2 {
            kept.push(Run::open(points));
        }
    }
    kept
}

/// A picture being painted, as colours already scaled by how see-through they are.
struct Canvas {
    width: usize,
    height: usize,
    rgba: Vec<f32>,
    /// Kept between shapes, because setting it up again for each of an overlay's hundreds of tiny
    /// shapes took most of the time.
    scratch: Scratch,
}

impl Canvas {
    fn new(width: u32, height: u32) -> Self {
        let (width, height) = (width.max(1) as usize, height.max(1) as usize);
        Self {
            width,
            height,
            rgba: vec![0.0; width * height * 4],
            scratch: Scratch::new(),
        }
    }

    /// Lays a colour over one pixel, with this much of it showing.
    fn lay(&mut self, x: usize, y: usize, colour: [f32; 3], amount: f32) {
        if x >= self.width || y >= self.height || amount <= 0.0 {
            return;
        }
        let at = (y * self.width + x) * 4;
        let pixel = &mut self.rgba[at..at + 4];
        let amount = amount.min(1.0);
        for (value, part) in pixel.iter_mut().zip(colour) {
            *value = part * amount + *value * (1.0 - amount);
        }
        pixel[3] = amount + pixel[3] * (1.0 - amount);
    }

    /// Lays a colour through a mask from `zeno`, with `amount` of it showing at most.
    fn lay_mask(&mut self, mask: &[u8], place: zeno::Placement, colour: Colour, amount: f32) {
        let colour = float(colour);
        for row in 0..place.height as i32 {
            let y = place.top + row;
            if y < 0 || y as usize >= self.height {
                continue;
            }
            for column in 0..place.width as i32 {
                let x = place.left + column;
                if x < 0 || x as usize >= self.width {
                    continue;
                }
                let cover = mask[(row * place.width as i32 + column) as usize];
                if cover > 0 {
                    self.lay(
                        x as usize,
                        y as usize,
                        colour,
                        f32::from(cover) / 255.0 * amount,
                    );
                }
            }
        }
    }

    /// Fills the inside of runs, joined into one shape.
    fn fill(&mut self, runs: &[Run], at: &dyn Fn(Point) -> Point, colour: Colour, amount: f32) {
        let commands = commands(runs, at, true);
        if commands.is_empty() {
            return;
        }
        let (mask, place) = Mask::with_scratch(&commands, &mut self.scratch)
            .style(Fill::NonZero)
            .render();
        self.lay_mask(&mask, place, colour, amount);
    }

    /// Draws along runs with a round-ended pen `width` pixels wide.
    fn stroke(
        &mut self,
        runs: &[Run],
        at: &dyn Fn(Point) -> Point,
        width: f32,
        colour: Colour,
        amount: f32,
    ) {
        let commands = commands(runs, at, false);
        if commands.is_empty() || width <= 0.0 {
            return;
        }
        let mut pen = Stroke::new(width);
        pen.cap(Cap::Round).join(Join::Round);
        let (mask, place) = Mask::with_scratch(&commands, &mut self.scratch)
            .style(pen)
            .render();
        self.lay_mask(&mask, place, colour, amount);
    }

    /// Draws along runs keeping their corners sharp, as a shape's outline should, while ends
    /// stay round for a line still being drawn.
    fn stroke_sharp(
        &mut self,
        runs: &[Run],
        at: &dyn Fn(Point) -> Point,
        width: f32,
        colour: Colour,
        amount: f32,
    ) {
        let commands = commands(runs, at, false);
        if commands.is_empty() || width <= 0.0 {
            return;
        }
        let mut pen = Stroke::new(width);
        pen.cap(Cap::Round)
            .join(Join::Miter)
            .miter_limit(SHARPEST_CORNER);
        let (mask, place) = Mask::with_scratch(&commands, &mut self.scratch)
            .style(pen)
            .render();
        self.lay_mask(&mask, place, colour, amount);
    }

    fn into_picture(self) -> StillPicture {
        // Adding a half and cutting off the rest rounds the same as `round`, far faster.
        let rgba = self
            .rgba
            .iter()
            .map(|value| (value.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
            .collect();
        StillPicture::new(self.width as u32, self.height as u32, rgba)
    }
}

fn float(colour: Colour) -> [f32; 3] {
    colour.map(|part| f32::from(part) / 255.0)
}

fn mix(a: Colour, b: Colour, share_of_b: f32) -> Colour {
    let share = share_of_b.clamp(0.0, 1.0);
    [0, 1, 2].map(|index| {
        (f32::from(a[index]) * (1.0 - share) + f32::from(b[index]) * share).round() as u8
    })
}

/// Runs as path commands for `zeno`, through `at`. Runs that go round are closed, and when filling
/// every run is.
fn commands(runs: &[Run], at: &dyn Fn(Point) -> Point, filling: bool) -> Vec<Command> {
    let mut commands = Vec::new();
    for run in runs.iter().filter(|run| run.points.len() >= 2) {
        let mut points = run.points.iter().map(|point| at(*point));
        if let Some(first) = points.next() {
            commands.push(Command::MoveTo(first.into()));
        }
        commands.extend(points.map(|point| Command::LineTo(point.into())));
        if run.closed || filling {
            commands.push(Command::Close);
        }
    }
    commands
}

/// How a drawn item is laid out for one picture.
struct Layout {
    /// How many pixels its longer side covers, at rest.
    pixels: f32,
    /// How many pixels thick its lines are.
    line: f32,
    size: (u32, u32),
}

/// How far past the middle of its line a sharp outline's pointiest corner reaches, in half
/// lines: 1 for no corners, about 1.4 for a square's, and never past the pen's limit.
fn sharpest_corner(outline: &Outline) -> f32 {
    let mut reach: f32 = 1.0;
    for run in &outline.runs {
        let points = &run.points;
        let count = points.len();
        if count < 3 {
            continue;
        }
        let corners = if run.closed { 0..count } else { 1..count - 1 };
        for index in corners {
            let before = points[(index + count - 1) % count];
            let here = points[index];
            let after = points[(index + 1) % count];
            let a = (before.0 - here.0, before.1 - here.1);
            let b = (after.0 - here.0, after.1 - here.1);
            let lengths = (a.0.hypot(a.1), b.0.hypot(b.1));
            if lengths.0 <= f32::EPSILON || lengths.1 <= f32::EPSILON {
                continue;
            }
            let cosine = ((a.0 * b.0 + a.1 * b.1) / (lengths.0 * lengths.1)).clamp(-1.0, 1.0);
            // The corner's own angle. Its miter reaches 1 / sin(half of it) half lines out.
            let half = cosine.acos() / 2.0;
            let miter = 1.0 / half.sin().max(f32::EPSILON);
            if miter <= SHARPEST_CORNER {
                reach = reach.max(miter);
            }
        }
    }
    reach
}

/// Lays out a shape or annotation `pixels` across its longer side, with room around it for its
/// lines, and for stretching if `room` is more than 1.
fn layout(outline: &Outline, art: &Art, pixels: f32, room: f32, sharp: bool) -> Layout {
    let line = f32::from(art.outline) / LINE_PARTS * pixels;
    // A sharp corner reaches further out than half the line.
    let reach = if sharp { sharpest_corner(outline) } else { 1.0 };
    let margin = line / 2.0 * reach + 2.0;
    let size = (
        (outline.width * pixels * room + margin * 2.0).ceil() as u32,
        (outline.height * pixels * room + margin * 2.0).ceil() as u32,
    );
    Layout { pixels, line, size }
}

/// Draws a shape or annotation, as it is at one moment of its motion.
fn draw_placed(
    art: &Art,
    entry: &ArtItem,
    pixels: f32,
    frame: ArtFrame,
    room: f32,
) -> StillPicture {
    let outline = outline_of(entry.key);
    // Shapes keep their corners. Annotations and hand-drawn shapes look drawn with a pen.
    let sharp = entry.kind == ArtKind::Shape && entry.collection != "Hand drawn";
    let layout = layout(&outline, art, pixels, room, sharp);
    let mut canvas = Canvas::new(layout.size.0, layout.size.1);
    let middle = (layout.size.0 as f32 / 2.0, layout.size.1 as f32 / 2.0);
    let stretch = frame.stretch;
    let half = (outline.width / 2.0, outline.height / 2.0);
    let at = |(x, y): Point| {
        (
            middle.0 + (x - half.0) * layout.pixels * stretch[0],
            middle.1 + (y - half.1) * layout.pixels * stretch[1],
        )
    };

    if entry.collection == "Pattern" && entry.kind == ArtKind::Shape {
        if let Some(fill) = art.fill {
            // Each column of cells fades in as the sweep reaches it.
            const COLUMNS: usize = 8;
            for column in 0..COLUMNS {
                let from = column as f32 / COLUMNS as f32;
                let to = from + 1.0 / COLUMNS as f32;
                let shown = ((frame.swept * 1.4 - from) / 0.4).clamp(0.0, 1.0);
                if shown <= 0.0 {
                    continue;
                }
                let cells = fitted(pattern_cells(entry.key, from..to)).runs;
                canvas.fill(&cells, &at, fill, shown);
            }
        }
        return canvas.into_picture();
    }

    let partly = frame.drawn < 1.0 || frame.rubbed > 0.0;
    let drawn = part_of(&outline.runs, frame.rubbed, frame.drawn);
    if let Some(fill) = art.fill.filter(|_| entry.kind == ArtKind::Shape) {
        // A shape that draws its outline fills in as the outline finishes, and one rubbed out
        // fades as it goes.
        let amount = if partly {
            (frame.drawn * frame.drawn - frame.rubbed).max(0.0)
        } else {
            1.0
        };
        canvas.fill(&outline.runs, &at, fill, amount);
    }
    if layout.line > 0.0 {
        let runs = if partly { &drawn } else { &outline.runs };
        if sharp {
            canvas.stroke_sharp(runs, &at, layout.line, art.line_colour(), 1.0);
        } else {
            canvas.stroke(runs, &at, layout.line, art.line_colour(), 1.0);
        }
    }
    canvas.into_picture()
}

/// How many pixels the longer side of a shape or annotation covers at the size it shows at.
fn shown_pixels(title: &Title, canvas: (u32, u32)) -> f32 {
    let shorter = canvas.0.min(canvas.1).max(1) as f32;
    (title.size as f32 / WHOLE as f32 * shorter).max(2.0)
}

/// How much smaller than the canvas something covering it is drawn.
fn canvas_shrink(entry: &ArtItem) -> f32 {
    match entry.key {
        "bokeh" => 0.125,
        "light-leak" | "sunset" | "ocean" | "spotlight" | "soft-shapes" => 0.25,
        "neon-glow" | "cool-glow" | "dust" | "snow" | "rain" | "sparkles" => 0.5,
        _ => 1.0,
    }
}

/// Draws a frame, overlay or background at the canvas's size, or smaller by `shrink`.
fn draw_covering(art: &Art, entry: &ArtItem, canvas: (u32, u32), seconds: f32) -> StillPicture {
    let shrink = canvas_shrink(entry);
    let width = ((canvas.0 as f32 * shrink).round() as u32).max(1);
    let height = ((canvas.1 as f32 * shrink).round() as u32).max(1);
    let mut paint = Canvas::new(width, height);
    let (w, h) = (width as f32, height as f32);
    let shorter = w.min(h);
    let colour = art.fill.unwrap_or(art.second);
    let second = art.second;
    let identity = |point: Point| point;
    match entry.key {
        "solid-border" => {
            let edge = shorter * 0.03;
            ring(&mut paint, (w, h), edge, 0.0, colour);
        }
        "double-border" => {
            let edge = shorter * 0.012;
            ring_at(&mut paint, (w, h), shorter * 0.02, edge, colour);
            ring_at(&mut paint, (w, h), shorter * 0.045, edge, colour);
        }
        "rounded-border" => ring(&mut paint, (w, h), shorter * 0.04, shorter * 0.08, colour),
        "dashed-border" => {
            let inset = shorter * 0.04;
            let dash = shorter * 0.05;
            let edges = [
                ((inset, inset), (w - inset, inset)),
                ((w - inset, inset), (w - inset, h - inset)),
                ((w - inset, h - inset), (inset, h - inset)),
                ((inset, h - inset), (inset, inset)),
            ];
            for (from, to) in edges {
                let length = (to.0 - from.0).hypot(to.1 - from.1);
                let count = (length / (dash * 1.8)).floor().max(1.0) as usize;
                let step = length / count as f32;
                for index in 0..count {
                    let t0 = index as f32 * step / length;
                    let t1 = (index as f32 * step + step * 0.55) / length;
                    let point =
                        |t: f32| (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
                    let run = Run::open(vec![point(t0), point(t1)]);
                    paint.stroke(&[run], &identity, shorter * 0.012, colour, 1.0);
                }
            }
        }
        "corners" | "round-corners" | "viewfinder" => {
            let inset = shorter * 0.05;
            let reach = shorter * 0.14;
            let radius = if entry.key == "round-corners" {
                shorter * 0.05
            } else {
                0.0
            };
            for (cx, cy, sx, sy) in [
                (inset, inset, 1.0, 1.0),
                (w - inset, inset, -1.0, 1.0),
                (w - inset, h - inset, -1.0, -1.0),
                (inset, h - inset, 1.0, -1.0),
            ] {
                let mut points = vec![(cx, cy + sy * reach)];
                if radius > 0.0 {
                    // A curve through the corner, from one side to the other.
                    for step in 0..=10 {
                        let t = step as f32 / 10.0;
                        let (a, b, c) = ((1.0 - t) * (1.0 - t), 2.0 * (1.0 - t) * t, t * t);
                        points.push((
                            a * cx + b * cx + c * (cx + sx * radius),
                            a * (cy + sy * radius) + b * cy + c * cy,
                        ));
                    }
                } else {
                    points.push((cx, cy));
                }
                points.push((cx + sx * reach, cy));
                paint.stroke(
                    &[Run::open(points)],
                    &identity,
                    shorter * 0.012,
                    colour,
                    1.0,
                );
            }
            if entry.key == "viewfinder" {
                let dot = Run::closed(circle(
                    (inset + reach * 0.6, inset + reach * 0.6),
                    shorter * 0.018,
                ));
                paint.fill(&[dot], &identity, [230, 32, 32], 1.0);
                let cross = shorter * 0.03;
                let (mx, my) = (w / 2.0, h / 2.0);
                let lines = [
                    Run::open(vec![(mx - cross, my), (mx + cross, my)]),
                    Run::open(vec![(mx, my - cross), (mx, my + cross)]),
                ];
                paint.stroke(&lines, &identity, shorter * 0.005, colour, 1.0);
                let battery = rectangle(
                    w - inset - reach * 0.9,
                    inset + reach * 0.4,
                    reach * 0.5,
                    reach * 0.25,
                );
                paint.stroke(
                    &[Run::closed(battery)],
                    &identity,
                    shorter * 0.005,
                    colour,
                    1.0,
                );
            }
        }
        "film-strip" => {
            let band = h * 0.1;
            let holes = |top: f32| -> Vec<Run> {
                let hole = band * 0.45;
                let count = (w / (hole * 1.9)).floor().max(1.0) as usize;
                let gap = w / count as f32;
                let mut runs = vec![Run::closed(rectangle(0.0, top, w, band))];
                for index in 0..count {
                    let x = gap * (index as f32 + 0.5) - hole * 0.6;
                    let corners = rectangle(x, top + (band - hole) / 2.0, hole * 1.2, hole);
                    let mut hole_points = rounded(&corners[1..], hole * 0.2);
                    hole_points.reverse();
                    runs.push(Run::closed(hole_points));
                }
                runs
            };
            paint.fill(&holes(0.0), &identity, colour, 1.0);
            paint.fill(&holes(h - band), &identity, colour, 1.0);
        }
        "instant-photo" => {
            let side = shorter * 0.05;
            let mut window = rectangle(side, side, w - side * 2.0, h - side * 2.0 - shorter * 0.13);
            window.reverse();
            let runs = [Run::closed(rectangle(0.0, 0.0, w, h)), Run::closed(window)];
            paint.fill(&runs, &identity, colour, 1.0);
        }
        "neon-glow" | "cool-glow" => {
            let inset = shorter * 0.05;
            let corners = rectangle(inset, inset, w - inset * 2.0, h - inset * 2.0);
            let edge = Run::closed(rounded(&corners[1..], shorter * 0.05));
            let mut glow = Canvas::new(width, height);
            glow.stroke(
                std::slice::from_ref(&edge),
                &identity,
                shorter * 0.04,
                colour,
                1.0,
            );
            soften(&mut glow, (shorter * 0.03) as usize);
            for (into, from) in paint.rgba.iter_mut().zip(&glow.rgba) {
                *into = *from;
            }
            paint.stroke(
                &[edge],
                &identity,
                shorter * 0.008,
                mix(colour, [255, 255, 255], 0.65),
                1.0,
            );
        }
        "phone" | "laptop" | "monitor" => device(&mut paint, entry.key, (w, h), colour),
        "light-leak" => light_leak(&mut paint, colour, seconds),
        "bokeh" => bokeh(&mut paint, colour, seconds),
        "dust" => dust(&mut paint, colour, seconds),
        "sparkles" => sparkles(&mut paint, colour, seconds),
        "snow" => snow(&mut paint, colour, seconds),
        "rain" => rain(&mut paint, colour, seconds),
        "sunset" => gradient(&mut paint, colour, second, |_, y| y),
        "ocean" => gradient(&mut paint, colour, second, |x, y| (x + y) / 2.0),
        "spotlight" => gradient(&mut paint, colour, second, |x, y| {
            ((x - 0.5).hypot((y - 0.5) * h / w) * 1.6).min(1.0)
        }),
        "split" => {
            paint.fill(
                &[Run::closed(rectangle(0.0, 0.0, w, h))],
                &identity,
                colour,
                1.0,
            );
            let half = Run::closed(vec![(w * 0.62, 0.0), (w, 0.0), (w, h), (w * 0.38, h)]);
            paint.fill(&[half], &identity, second, 1.0);
        }
        "halves" => {
            paint.fill(
                &[Run::closed(rectangle(0.0, 0.0, w / 2.0, h))],
                &identity,
                colour,
                1.0,
            );
            paint.fill(
                &[Run::closed(rectangle(w / 2.0, 0.0, w / 2.0, h))],
                &identity,
                second,
                1.0,
            );
        }
        "soft-shapes" => {
            paint.fill(
                &[Run::closed(rectangle(0.0, 0.0, w, h))],
                &identity,
                colour,
                1.0,
            );
            let mut shapes = Canvas::new(width, height);
            for (index, (x, y, r)) in [(0.2, 0.25, 0.3), (0.8, 0.3, 0.25), (0.55, 0.85, 0.35)]
                .iter()
                .enumerate()
            {
                let tint = mix(second, colour, index as f32 * 0.25);
                let blob = Run::closed(circle((x * w, y * h), r * shorter));
                shapes.fill(&[blob], &identity, tint, 0.85);
            }
            soften(&mut shapes, (shorter * 0.12) as usize);
            for pixel in 0..width as usize * height as usize {
                let at = pixel * 4;
                let amount = shapes.rgba[at + 3];
                for part in 0..3 {
                    paint.rgba[at + part] =
                        shapes.rgba[at + part] + paint.rgba[at + part] * (1.0 - amount);
                }
            }
        }
        "dot-grid" | "stripes" | "graph-paper" => {
            paint.fill(
                &[Run::closed(rectangle(0.0, 0.0, w, h))],
                &identity,
                colour,
                1.0,
            );
            let step = shorter / 12.0;
            match entry.key {
                "dot-grid" => {
                    let dots: Vec<Run> = grid_points((w, h), step)
                        .map(|point| Run::closed(circle(point, step * 0.09)))
                        .collect();
                    paint.fill(&dots, &identity, second, 1.0);
                }
                "stripes" => {
                    let count = ((w + h) / step).ceil() as usize;
                    let stripes: Vec<Run> = (0..count)
                        .map(|index| {
                            let x = index as f32 * step - h;
                            Run::closed(vec![
                                (x, h),
                                (x + step * 0.45, h),
                                (x + step * 0.45 + h, 0.0),
                                (x + h, 0.0),
                            ])
                        })
                        .collect();
                    paint.fill(&stripes, &identity, second, 1.0);
                }
                _ => {
                    let small = step / 2.0;
                    let mut lines = Vec::new();
                    let mut x = 0.0;
                    let mut index = 0;
                    while x < w {
                        lines.push((Run::open(vec![(x, 0.0), (x, h)]), index % 4 == 0));
                        x += small;
                        index += 1;
                    }
                    let (mut y, mut index) = (0.0, 0);
                    while y < h {
                        lines.push((Run::open(vec![(0.0, y), (w, y)]), index % 4 == 0));
                        y += small;
                        index += 1;
                    }
                    for (line, thick) in lines {
                        let width = if thick {
                            shorter * 0.003
                        } else {
                            shorter * 0.0012
                        };
                        paint.stroke(&[line], &identity, width.max(1.0), second, 1.0);
                    }
                }
            }
        }
        _ => {}
    }
    paint.into_picture()
}

/// The middles of a grid of points `step` apart, covering a picture.
fn grid_points((w, h): Point, step: f32) -> impl Iterator<Item = Point> {
    let columns = (w / step).ceil() as usize + 1;
    let rows = (h / step).ceil() as usize + 1;
    (0..rows).flat_map(move |row| {
        (0..columns).map(move |column| (column as f32 * step, row as f32 * step))
    })
}

/// A border `edge` thick all round, whose inside corners are rounded by `radius`.
fn ring(paint: &mut Canvas, (w, h): Point, edge: f32, radius: f32, colour: Colour) {
    let inside = rectangle(edge, edge, w - edge * 2.0, h - edge * 2.0);
    let mut window = if radius > 0.0 {
        rounded(&inside[1..], radius)
    } else {
        inside
    };
    window.reverse();
    let runs = [Run::closed(rectangle(0.0, 0.0, w, h)), Run::closed(window)];
    paint.fill(&runs, &|point| point, colour, 1.0);
}

/// A thin border `edge` thick, `inset` in from the canvas's edges.
fn ring_at(paint: &mut Canvas, (w, h): Point, inset: f32, edge: f32, colour: Colour) {
    let line = Run::closed(rectangle(inset, inset, w - inset * 2.0, h - inset * 2.0));
    let mut pen = Stroke::new(edge);
    pen.join(Join::Miter);
    let commands = commands(&[line], &|point| point, false);
    let (mask, place) = Mask::with_scratch(&commands, &mut paint.scratch)
        .style(pen)
        .render();
    paint.lay_mask(&mask, place, colour, 1.0);
}

/// A phone, laptop or monitor's outline in the middle of the canvas, with its screen see-through.
fn device(paint: &mut Canvas, key: &str, (w, h): Point, colour: Colour) {
    let identity = |point: Point| point;
    let (screen_w, screen_h) = match key {
        "phone" => (h * 0.8 * 9.0 / 19.5, h * 0.8),
        "laptop" => (
            w.min(h * 16.0 / 9.0) * 0.62,
            w.min(h * 16.0 / 9.0) * 0.62 * 10.0 / 16.0,
        ),
        _ => (
            w.min(h * 16.0 / 9.0) * 0.66,
            w.min(h * 16.0 / 9.0) * 0.66 * 9.0 / 16.0,
        ),
    };
    let bezel = screen_w.min(screen_h) * if key == "phone" { 0.05 } else { 0.035 };
    let top = match key {
        "phone" => (h - screen_h) / 2.0,
        "laptop" => h * 0.12,
        _ => h * 0.1,
    };
    let left = (w - screen_w) / 2.0;
    let outer = rectangle(
        left - bezel,
        top - bezel,
        screen_w + bezel * 2.0,
        screen_h + bezel * 2.0,
    );
    let radius = if key == "phone" {
        bezel * 3.0
    } else {
        bezel * 0.6
    };
    let mut screen = rectangle(left, top, screen_w, screen_h);
    screen = if key == "phone" {
        rounded(&screen[1..], bezel * 2.0)
    } else {
        screen
    };
    screen.reverse();
    let body = [
        Run::closed(rounded(&outer[1..], radius)),
        Run::closed(screen),
    ];
    paint.fill(&body, &identity, colour, 1.0);
    let bottom = top + screen_h + bezel;
    match key {
        "laptop" => {
            let base = Run::closed(vec![
                (left - bezel * 3.0, bottom),
                (left + screen_w + bezel * 3.0, bottom),
                (left + screen_w + bezel * 5.0, bottom + bezel * 1.6),
                (left - bezel * 5.0, bottom + bezel * 1.6),
            ]);
            paint.fill(&[base], &identity, colour, 1.0);
        }
        "monitor" => {
            let neck = rectangle(w / 2.0 - bezel * 1.5, bottom, bezel * 3.0, bezel * 4.0);
            let foot = rectangle(
                w / 2.0 - bezel * 6.0,
                bottom + bezel * 4.0,
                bezel * 12.0,
                bezel,
            );
            paint.fill(
                &[Run::closed(neck), Run::closed(foot)],
                &identity,
                colour,
                1.0,
            );
        }
        _ => {}
    }
}

/// Blurs a picture, as two passes of a box `radius` pixels each way, three times over, which comes
/// close to a smooth blur.
fn soften(paint: &mut Canvas, radius: usize) {
    if radius == 0 {
        return;
    }
    let (width, height) = (paint.width, paint.height);
    let mut scratch = vec![0.0f32; paint.rgba.len()];
    for _ in 0..3 {
        for y in 0..height {
            for channel in 0..4 {
                let row = |x: usize| (y * width + x) * 4 + channel;
                let mut total = 0.0;
                let mut count = 0.0;
                for x in 0..=radius.min(width - 1) {
                    total += paint.rgba[row(x)];
                    count += 1.0;
                }
                for x in 0..width {
                    scratch[row(x)] = total / count;
                    if x + radius + 1 < width {
                        total += paint.rgba[row(x + radius + 1)];
                        count += 1.0;
                    }
                    if x >= radius {
                        total -= paint.rgba[row(x - radius)];
                        count -= 1.0;
                    }
                }
            }
        }
        for x in 0..width {
            for channel in 0..4 {
                let column = |y: usize| (y * width + x) * 4 + channel;
                let mut total = 0.0;
                let mut count = 0.0;
                for y in 0..=radius.min(height - 1) {
                    total += scratch[column(y)];
                    count += 1.0;
                }
                for y in 0..height {
                    paint.rgba[column(y)] = total / count;
                    if y + radius + 1 < height {
                        total += scratch[column(y + radius + 1)];
                        count += 1.0;
                    }
                    if y >= radius {
                        total -= scratch[column(y - radius)];
                        count -= 1.0;
                    }
                }
            }
        }
    }
}

/// Fills a picture with two colours, the second's share at each point coming from `share`, which
/// takes the point across and down as shares of the picture.
fn gradient(paint: &mut Canvas, first: Colour, second: Colour, share: impl Fn(f32, f32) -> f32) {
    let (a, b) = (float(first), float(second));
    for y in 0..paint.height {
        for x in 0..paint.width {
            let t = share(
                (x as f32 + 0.5) / paint.width as f32,
                (y as f32 + 0.5) / paint.height as f32,
            )
            .clamp(0.0, 1.0);
            let colour = [0, 1, 2].map(|index| a[index] * (1.0 - t) + b[index] * t);
            paint.lay(x, y, colour, 1.0);
        }
    }
}

/// Warm light drifting across the picture and swelling.
fn light_leak(paint: &mut Canvas, colour: Colour, seconds: f32) {
    let tint = float(colour);
    let (w, h) = (paint.width as f32, paint.height as f32);
    let phase = seconds / LIGHT_CYCLE * TAU;
    let spots = [
        (0.1 + 0.25 * phase.sin().abs(), 0.2, 0.55),
        (0.9 - 0.2 * (phase * 0.7).cos().abs(), 0.75, 0.45),
    ];
    let swell = 0.75 + 0.25 * (phase * 1.3).sin();
    for y in 0..paint.height {
        for x in 0..paint.width {
            let (px, py) = (x as f32 / w, y as f32 / h);
            let mut amount: f32 = 0.0;
            for (sx, sy, reach) in spots {
                let distance = (px - sx).hypot((py - sy) * h / w) / reach;
                amount += (1.0 - distance).max(0.0).powi(2);
            }
            let amount = (amount * swell * 0.85).min(0.9);
            if amount > 0.003 {
                paint.lay(x, y, tint, amount);
            }
        }
    }
}

/// Soft circles of light drifting slowly upwards.
fn bokeh(paint: &mut Canvas, colour: Colour, seconds: f32) {
    let (w, h) = (paint.width as f32, paint.height as f32);
    let shorter = w.min(h);
    for index in 0..18_u32 {
        let x = noise(index * 3 + 1) * w + (seconds * 0.3 + index as f32).sin() * shorter * 0.02;
        let rise = (noise(index * 3 + 2) + seconds * 0.02 * (0.5 + noise(index))).fract();
        let y = h * (1.1 - rise * 1.2);
        let radius = shorter * (0.04 + 0.07 * noise(index * 3 + 3));
        let tint = mix(colour, [255, 255, 255], noise(index * 5) * 0.4);
        let glow = 0.25 + 0.2 * (seconds * 0.8 + index as f32).sin().abs();
        let spot = Run::closed(circle((x, y), radius));
        paint.fill(&[spot], &|point| point, tint, glow);
    }
}

/// Specks and thin scratches that change twelve times a second, like old film.
fn dust(paint: &mut Canvas, colour: Colour, seconds: f32) {
    let (w, h) = (paint.width as f32, paint.height as f32);
    let shorter = w.min(h);
    let beat = (seconds * 12.0) as u32;
    for index in 0..40_u32 {
        let seed = beat.wrapping_mul(131).wrapping_add(index * 17);
        if noise(seed) < 0.5 {
            continue;
        }
        let (x, y) = (noise(seed + 1) * w, noise(seed + 2) * h);
        let radius = shorter * (0.0015 + 0.004 * noise(seed + 3));
        let shade = if noise(seed + 4) < 0.5 {
            colour
        } else {
            [20, 18, 16]
        };
        let speck = Run::closed(shaky(&circle((x, y), radius), radius * 0.4, seed));
        paint.fill(&[speck], &|point| point, shade, 0.8);
    }
    for index in 0..2_u32 {
        let seed = (beat / 3).wrapping_mul(71).wrapping_add(index * 29);
        if noise(seed) < 0.4 {
            continue;
        }
        let x = noise(seed + 1) * w;
        let line = Run::open(vec![
            (x, 0.0),
            (x + shorter * 0.01 * (noise(seed + 2) - 0.5), h),
        ]);
        paint.stroke(&[line], &|point| point, shorter * 0.0015, colour, 0.5);
    }
}

/// Four-pointed stars that twinkle in and out.
fn sparkles(paint: &mut Canvas, colour: Colour, seconds: f32) {
    let (w, h) = (paint.width as f32, paint.height as f32);
    let shorter = w.min(h);
    for index in 0..24_u32 {
        let (x, y) = (noise(index * 7 + 1) * w, noise(index * 7 + 2) * h);
        let speed = 0.6 + noise(index * 7 + 3) * 0.9;
        let twinkle = ((seconds * speed + noise(index * 7 + 4)) * TAU).sin();
        if twinkle <= 0.0 {
            continue;
        }
        let reach = shorter * (0.015 + 0.025 * noise(index * 7 + 5)) * twinkle;
        let turn = seconds * 40.0 * (noise(index) - 0.5);
        let points: Vec<Point> = star(4, 0.2)
            .into_iter()
            .map(|point| {
                let (px, py) = rotate(point, turn);
                (x + px * reach, y + py * reach)
            })
            .collect();
        paint.fill(&[Run::closed(points)], &|point| point, colour, twinkle);
    }
}

/// Flakes falling and drifting from side to side.
fn snow(paint: &mut Canvas, colour: Colour, seconds: f32) {
    let (w, h) = (paint.width as f32, paint.height as f32);
    let shorter = w.min(h);
    for index in 0..140_u32 {
        let depth = 0.4 + 0.6 * noise(index * 5 + 1);
        let fall = (noise(index * 5 + 2) + seconds * 0.09 * depth).fract();
        let sway = (seconds * (0.5 + depth) + index as f32).sin() * shorter * 0.015;
        let (x, y) = (
            noise(index * 5 + 3) * w + sway,
            fall * (h * 1.05) - h * 0.025,
        );
        let radius = shorter * 0.004 * (0.5 + depth);
        let flake = Run::closed(circle((x, y), radius));
        paint.fill(&[flake], &|point| point, colour, 0.5 + 0.45 * depth);
    }
}

/// Thin streaks falling at a slant.
fn rain(paint: &mut Canvas, colour: Colour, seconds: f32) {
    let (w, h) = (paint.width as f32, paint.height as f32);
    let shorter = w.min(h);
    let streaks: Vec<(Run, f32)> = (0..160_u32)
        .map(|index| {
            let depth = 0.4 + 0.6 * noise(index * 5 + 1);
            let fall = (noise(index * 5 + 2) + seconds * 0.9 * depth).fract();
            let length = shorter * 0.05 * depth;
            let x = noise(index * 5 + 3) * (w + h * 0.2) - fall * h * 0.2;
            let y = fall * (h + length) - length;
            (
                Run::open(vec![(x, y), (x - length * 0.2, y + length)]),
                0.25 + 0.4 * depth,
            )
        })
        .collect();
    for (streak, amount) in streaks {
        paint.stroke(
            &[streak],
            &|point| point,
            (shorter * 0.0018).max(1.0),
            colour,
            amount,
        );
    }
}

/// Draws a title that is something from the library, at the size it shows at so it stays sharp.
pub(crate) fn draw_art_title(title: &Title, art: &Art, canvas: (u32, u32)) -> TitleDrawing {
    let entry = art.entry();
    let (picture, shrink) = picture_at(title, art, canvas, rest_frame(entry), 1.0);
    TitleDrawing {
        scale: scale_for_pixels((picture.width, picture.height), canvas, shrink),
        picture,
        font_missing: false,
        logo_spot: None,
    }
}

/// What something looks like at rest: whole, and for an overlay, a moment in.
fn rest_frame(entry: &ArtItem) -> ArtFrame {
    ArtFrame {
        seconds: if entry.kind == ArtKind::Overlay {
            1.0
        } else {
            0.0
        },
        ..ArtFrame::REST
    }
}

/// The picture for one moment, and how much smaller than it shows it was drawn.
fn picture_at(
    title: &Title,
    art: &Art,
    canvas: (u32, u32),
    frame: ArtFrame,
    room: f32,
) -> (StillPicture, f64) {
    let entry = art.entry();
    if entry.kind.fills_canvas() {
        let picture = draw_covering(art, entry, canvas, frame.seconds);
        return (picture, f64::from(canvas_shrink(entry)));
    }
    let mut pixels = shown_pixels(title, canvas);
    let mut shrink = 1.0;
    // Too big to draw, so it is drawn smaller and shown larger.
    if pixels * room > LARGEST_SIDE {
        shrink = LARGEST_SIDE / (pixels * room);
        pixels *= shrink;
    }
    (
        draw_placed(art, entry, pixels, frame, room),
        f64::from(shrink),
    )
}

/// Something from the library whose picture changes as it moves, ready to draw any moment.
pub(crate) struct MovingArt {
    title: Title,
    art: Art,
    canvas: (u32, u32),
    room: f32,
    shrink: f64,
}

impl MovingArt {
    pub(crate) fn prepare(title: &Title, art: &Art, canvas: (u32, u32)) -> Self {
        let motion = art.motion();
        let stretches = motion.kind == ArtKind::Shape
            && matches!(motion.natural, Natural::Squash | Natural::Wobble);
        let room = if stretches { STRETCH_ROOM } else { 1.0 };
        let (_, shrink) = picture_at(title, art, canvas, rest_frame(art.entry()), room);
        Self {
            title: title.clone(),
            art: art.clone(),
            canvas,
            room,
            shrink,
        }
    }

    pub(crate) fn scale_for(&self, picture: (u32, u32)) -> i32 {
        scale_for_pixels(picture, self.canvas, self.shrink)
    }

    pub(crate) fn frame(&self, stage: bareclip_core::motion::Stage) -> StillPicture {
        let frame = self.art.motion().frame(stage);
        picture_at(&self.title, &self.art, self.canvas, frame, self.room).0
    }
}

#[cfg(test)]
mod tests {
    use bareclip_core::{
        art::{Animate, CATALOGUE},
        motion::Motion,
    };

    use super::*;

    const CANVAS: (u32, u32) = (1280, 720);

    fn title_of(key: &str) -> Title {
        let item = bareclip_core::art::item_named(key).expect("it is listed");
        Title::of_art(Art::of(item))
    }

    /// How much of a picture is covered, from 0 to 1.
    fn coverage(picture: &StillPicture) -> f32 {
        let total: f32 = picture
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|pixel| f32::from(pixel[3]) / 255.0)
            .sum();
        total / (picture.width * picture.height) as f32
    }

    #[test]
    fn every_item_draws_something_and_leaves_room_around_it() {
        for item in CATALOGUE {
            let title = title_of(item.key);
            let drawing = bareclip_text_draw(&title);
            let covered = coverage(&drawing.picture);
            assert!(covered > 0.0002, "{} is empty", item.key);
            if !item.kind.fills_canvas() {
                assert!(covered < 0.995, "{} fills its whole box", item.key);
            }
        }
    }

    fn bareclip_text_draw(title: &Title) -> TitleDrawing {
        crate::draw_title(title, CANVAS)
    }

    #[test]
    fn a_shape_is_drawn_at_the_size_it_shows_so_it_stays_sharp() {
        let mut title = title_of("circle");
        let small = bareclip_text_draw(&title);
        title.size *= 4;
        let big = bareclip_text_draw(&title);
        assert!(big.picture.width >= small.picture.width * 3);
        // A picture four times as many pixels across shows four times as big: one for one.
        let grown = f64::from(big.scale) / f64::from(small.scale);
        let wider = f64::from(big.picture.width) / f64::from(small.picture.width);
        assert!((grown - wider).abs() < 0.05, "{grown} {wider}");
        // Its edge is as soft at four times the size as at its own: one or two pixels.
        let edge_pixels = |picture: &StillPicture| {
            let row = picture.height as usize / 2;
            (0..picture.width as usize)
                .filter(|x| {
                    let alpha = picture.rgba[(row * picture.width as usize + x) * 4 + 3];
                    alpha > 10 && alpha < 245
                })
                .count()
        };
        assert!(
            edge_pixels(&big.picture) <= 4,
            "{}",
            edge_pixels(&big.picture)
        );
    }

    #[test]
    fn a_shapes_fill_and_outline_are_its_own_colours() {
        let mut title = title_of("square");
        if let Some(art) = &mut title.art {
            art.fill = Some([10, 200, 30]);
            art.second = [250, 0, 0];
            art.outline = 12;
        }
        let picture = bareclip_text_draw(&title).picture;
        let (w, h) = (picture.width as usize, picture.height as usize);
        let at = |x: usize, y: usize| {
            let index = (y * w + x) * 4;
            [
                picture.rgba[index],
                picture.rgba[index + 1],
                picture.rgba[index + 2],
            ]
        };
        assert_eq!(at(w / 2, h / 2), [10, 200, 30]);
        let edge = (f32::from(12_u8) / LINE_PARTS * shown_pixels(&title, CANVAS) / 2.0) as usize;
        assert_eq!(at(w / 2, edge + 1), [250, 0, 0]);
    }

    /// The box around every pixel that shows, as left, top, right and bottom.
    fn shown_box(picture: &StillPicture) -> (usize, usize, usize, usize) {
        let (w, h) = (picture.width as usize, picture.height as usize);
        let shows = |x: usize, y: usize| picture.rgba[(y * w + x) * 4 + 3] > 128;
        let columns: Vec<usize> = (0..w).filter(|&x| (0..h).any(|y| shows(x, y))).collect();
        let rows: Vec<usize> = (0..h).filter(|&y| (0..w).any(|x| shows(x, y))).collect();
        (
            columns[0],
            rows[0],
            columns[columns.len() - 1],
            rows[rows.len() - 1],
        )
    }

    #[test]
    fn a_shapes_outline_keeps_its_corners_sharp() {
        let mut title = title_of("outline-square");
        if let Some(art) = &mut title.art {
            art.outline = 12;
        }
        let picture = bareclip_text_draw(&title).picture;
        let w = picture.width as usize;
        let (left, top, right, bottom) = shown_box(&picture);
        // A round join would leave the outer corner see-through.
        for (x, y) in [
            (left + 1, top + 1),
            (right - 1, top + 1),
            (left + 1, bottom - 1),
            (right - 1, bottom - 1),
        ] {
            assert!(picture.rgba[(y * w + x) * 4 + 3] > 200, "({x}, {y})");
        }
        // And nothing is cut off at the picture's edges.
        assert!(left > 0 && top > 0);
        assert!(right < w - 1 && bottom < picture.height as usize - 1);
    }

    #[test]
    fn a_stars_sharp_points_fit_inside_its_picture() {
        let mut title = title_of("outline-star");
        if let Some(art) = &mut title.art {
            art.outline = 12;
        }
        let picture = bareclip_text_draw(&title).picture;
        let (left, top, right, bottom) = shown_box(&picture);
        assert!(left > 0 && top > 0);
        assert!(right < picture.width as usize - 1 && bottom < picture.height as usize - 1);
    }

    #[test]
    fn an_annotation_keeps_its_round_pen() {
        let mut title = title_of("drawn-box");
        if let Some(art) = &mut title.art {
            art.outline = 12;
        }
        let picture = bareclip_text_draw(&title).picture;
        let w = picture.width as usize;
        let (left, top, ..) = shown_box(&picture);
        assert!(picture.rgba[((top + 1) * w + left + 1) * 4 + 3] < 128);
    }

    #[test]
    fn an_empty_shape_is_only_its_outline() {
        let picture = bareclip_text_draw(&title_of("outline-circle")).picture;
        let middle = ((picture.height / 2 * picture.width + picture.width / 2) * 4 + 3) as usize;
        assert_eq!(picture.rgba[middle], 0);
        assert!(coverage(&picture) > 0.02);
    }

    fn moving(key: &str, animate: Animate) -> (Title, MovingArt) {
        let mut title = title_of(key);
        if let Some(art) = &mut title.art {
            art.animate = animate;
        }
        let art = title.art.clone().expect("it is art");
        let moving = MovingArt::prepare(&title, &art, CANVAS);
        (title, moving)
    }

    fn stage_of(title: &Title, seconds: f32) -> bareclip_core::motion::Stage {
        title.motion().stage(seconds, 5.0)
    }

    #[test]
    fn drawing_in_shows_more_of_the_line_as_it_goes() {
        let (title, moving) = moving("marker-circle", Animate::None);
        let amounts: Vec<f32> = [0.05, 0.3, 0.6, 2.0]
            .iter()
            .map(|seconds| coverage(&moving.frame(stage_of(&title, *seconds))))
            .collect();
        for pair in amounts.windows(2) {
            assert!(pair[0] < pair[1], "{amounts:?}");
        }
        let rest = bareclip_text_draw(&title).picture;
        assert!((amounts[3] - coverage(&rest)).abs() < 0.001);
    }

    #[test]
    fn a_scribble_rubs_out_from_where_it_started() {
        let (title, moving) = moving("wavy-line", Animate::Natural);
        let whole = coverage(&moving.frame(stage_of(&title, 2.5)));
        let rubbing = moving.frame(stage_of(&title, 4.8));
        assert!(coverage(&rubbing) < whole * 0.8);
        // The start, on the left, goes first.
        let left_half: u32 = rubbing
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(index, _)| (*index as u32 % rubbing.width) < rubbing.width / 2)
            .map(|(_, pixel)| u32::from(pixel[3]))
            .sum();
        let all: u32 = rubbing
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|pixel| u32::from(pixel[3]))
            .sum();
        assert!(left_half * 3 < all, "{left_half} of {all}");
    }

    #[test]
    fn a_stretching_shape_keeps_the_same_size_of_picture_and_density() {
        let (title, moving) = moving("blob", Animate::Natural);
        let first = moving.frame(stage_of(&title, 0.2));
        let later = moving.frame(stage_of(&title, 0.5));
        assert_eq!((first.width, first.height), (later.width, later.height));
        let rest = bareclip_text_draw(&title);
        // Drawn with room, so the frame is bigger, at the same pixels for each pixel shown.
        assert!(first.width > rest.picture.width);
        let shown_rest = f64::from(rest.scale) / f64::from(rest.picture.width);
        let shown_frame =
            f64::from(moving.scale_for((first.width, first.height))) / f64::from(first.width);
        assert!((shown_rest - shown_frame).abs() / shown_rest < 0.02);
    }

    #[test]
    fn a_pattern_sweeps_in_from_the_left() {
        let (title, moving) = moving("checks", Animate::Natural);
        let part = moving.frame(stage_of(&title, 0.25));
        let right: u32 = part
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(index, _)| (*index as u32 % part.width) > part.width * 3 / 4)
            .map(|(_, pixel)| u32::from(pixel[3]))
            .sum();
        assert_eq!(right, 0);
        assert!(coverage(&part) > 0.0);
    }

    #[test]
    fn overlays_change_from_moment_to_moment_and_cover_the_canvas() {
        for key in ["snow", "rain", "sparkles", "dust", "bokeh", "light-leak"] {
            let (title, moving) = moving(key, Animate::None);
            let a = moving.frame(stage_of(&title, 1.0));
            let b = moving.frame(stage_of(&title, 1.5));
            assert_ne!(a.rgba, b.rgba, "{key}");
            assert_eq!(
                bareclip_text_draw(&title).scale,
                WHOLE,
                "{key} covers the canvas"
            );
        }
    }

    #[test]
    fn frames_and_backgrounds_cover_the_canvas() {
        for key in [
            "solid-border",
            "film-strip",
            "neon-glow",
            "sunset",
            "graph-paper",
        ] {
            let drawing = bareclip_text_draw(&title_of(key));
            assert_eq!(drawing.scale, WHOLE, "{key}");
            let fitted = bareclip_core::placement::fit_inside(
                (drawing.picture.width, drawing.picture.height),
                CANVAS,
            );
            assert_eq!((fitted.width as u32, fitted.height as u32), CANVAS, "{key}");
        }
        let background = bareclip_text_draw(&title_of("sunset")).picture;
        assert!(coverage(&background) > 0.999);
        let frame = bareclip_text_draw(&title_of("solid-border")).picture;
        assert!(coverage(&frame) < 0.2, "a frame leaves the middle clear");
    }

    #[test]
    fn a_shape_the_canvas_cant_hold_is_drawn_smaller_and_shown_larger() {
        let mut title = title_of("square");
        title.size = bareclip_core::title::LARGEST_ART_SIZE;
        let drawing = draw_art_title(&title, title.art.as_ref().expect("art"), (3840, 2160));
        assert!(drawing.picture.width as f32 <= LARGEST_SIDE + 8.0);
    }

    #[test]
    fn its_motion_is_arts() {
        assert!(matches!(title_of("star").motion(), Motion::Art(_)));
    }

    /// `./scripts/cargo.ps1 test --release -p bareclip-text how_long_library_frames_take '--' --ignored --nocapture`
    #[test]
    #[ignore = "a measurement, which only means something in a release build"]
    fn how_long_library_frames_take() {
        let canvas = (1920, 1080);
        for (key, animate) in [
            ("light-leak", Animate::None),
            ("bokeh", Animate::None),
            ("dust", Animate::None),
            ("sparkles", Animate::None),
            ("snow", Animate::None),
            ("rain", Animate::None),
            ("marker-circle", Animate::None),
            ("outline-star", Animate::Natural),
            ("blob", Animate::Natural),
            ("checks", Animate::Natural),
        ] {
            let mut title = title_of(key);
            if let Some(art) = &mut title.art {
                art.animate = animate;
            }
            let art = title.art.clone().expect("it is art");
            let moving = MovingArt::prepare(&title, &art, canvas);
            let frames = 60;
            let started = std::time::Instant::now();
            let mut slowest = std::time::Duration::ZERO;
            for frame in 0..frames {
                let at = std::time::Instant::now();
                let seconds = 0.05 + frame as f32 * 0.01;
                std::hint::black_box(moving.frame(title.motion().stage(seconds, 5.0)));
                slowest = slowest.max(at.elapsed());
            }
            let each = started.elapsed() / frames;
            println!(
                "{key:>14}: {:5.2} ms a frame, slowest {:5.2} ms",
                each.as_secs_f64() * 1000.0,
                slowest.as_secs_f64() * 1000.0
            );
        }
    }

    #[test]
    #[ignore = "writes a sheet of every item to look at; run on demand"]
    fn draw_every_item() {
        let Ok(folder) = std::env::var("BARECLIP_SCREENS") else {
            return;
        };
        let cell = 160_usize;
        let columns = 10;
        let rows = CATALOGUE.len().div_ceil(columns);
        let (width, height) = (cell * columns, cell * rows);
        let mut sheet = vec![40_u8; width * height * 3];
        for (index, item) in CATALOGUE.iter().enumerate() {
            let mut title = title_of(item.key);
            if item.kind.fills_canvas() {
                title.size = WHOLE;
            }
            let picture = crate::draw_title(&title, (cell as u32 * 16 / 9, cell as u32)).picture;
            let (left, top) = ((index % columns) * cell, (index / columns) * cell);
            let scale = (picture.width.max(picture.height) as f32 / (cell - 8) as f32).max(1.0);
            for y in 0..cell - 8 {
                for x in 0..cell - 8 {
                    let (from_x, from_y) = ((x as f32 * scale) as u32, (y as f32 * scale) as u32);
                    if from_x >= picture.width || from_y >= picture.height {
                        continue;
                    }
                    let at = ((from_y * picture.width + from_x) * 4) as usize;
                    let alpha = f32::from(picture.rgba[at + 3]) / 255.0;
                    let to = ((top + y + 4) * width + left + x + 4) * 3;
                    for part in 0..3 {
                        sheet[to + part] = (f32::from(picture.rgba[at + part])
                            + 40.0 * (1.0 - alpha))
                            .min(255.0) as u8;
                    }
                }
            }
        }
        let mut file = format!("P6 {width} {height} 255\n").into_bytes();
        file.extend(sheet);
        std::fs::write(
            std::path::Path::new(&folder).join("library-items.ppm"),
            file,
        )
        .expect("the sheet can be written");
    }
}
