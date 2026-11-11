//! Titles whose parts move. Each letter and box is painted once into a small picture of its own, and
//! a frame only has to lay those over each other, which is quick enough to do for every frame played.
//! Titles whose words themselves change, such as Typewriter and Countdown, are drawn again instead.

use bareclip_core::{
    canvas::Colour,
    motion::{BoxMove, Finish, LetterMove, Running, Stage, TitleMotion},
    picture::StillPicture,
    title::{Title, TitleShape, scale_for_pixels},
};
use cosmic_text::Color;

use crate::{
    art::MovingArt,
    draw::{
        Area, Ink, Paint, Plan, draw_title_with_logo, font_for, letter_pixels, plan_title,
        reach_past_words, star_at, word_colours,
    },
    fonts::{Engine, engine},
};

/// Room around the picture for parts to move into, as a share of how tall the letters are.
const ROOM: f32 = 1.6;
/// The largest picture a frame can be, so that it always fits on the GPU.
const LARGEST_SIDE: u32 = 4096;
const SPARK_COLOURS: [Colour; 3] = [[255, 214, 90], [120, 230, 160], [255, 110, 150]];
/// How wide the darker band at a sweep's edge is, as a share of the canvas.
const SWEEP_BAND: f32 = 0.3;
/// How much darker the ring around an opening circle is, and a sweep's leading band.
const DARKER: f32 = 0.75;
const WHITE: Colour = [255, 255, 255];
/// Full colour: nothing changes it.
const UNTINTED: [f32; 3] = [1.0; 3];

/// Which lot of words, or what else, a letter belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Mark,
    First,
    Star,
    Second,
}

/// A small picture, and where it sits in the frame.
#[derive(Clone)]
struct Sprite {
    x: f32,
    y: f32,
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

impl Sprite {
    fn from_paint(paint: Paint, x: f32, y: f32) -> Self {
        let picture = paint.into_picture();
        Self {
            x,
            y,
            width: picture.width as usize,
            height: picture.height as usize,
            rgba: picture.rgba,
        }
    }

    fn middle(&self) -> (f32, f32) {
        (
            self.x + self.width as f32 / 2.0,
            self.y + self.height as f32 / 2.0,
        )
    }

    fn rect(&self) -> [f32; 4] {
        [
            self.x,
            self.y,
            self.x + self.width as f32,
            self.y + self.height as f32,
        ]
    }
}

/// A letter, with the same letter in the accent colour if the style sweeps one along the words.
struct Letter {
    sprite: Sprite,
    swept: Option<Sprite>,
    /// How far it sits from the words' middle, from -1 to 1.
    place: f32,
    part: Part,
    /// Which word it is in, counting through both lots of words.
    word: usize,
}

/// A title ready to show at any moment of its motion.
pub struct MovingTitle {
    title: Title,
    motion: TitleMotion,
    canvas: (u32, u32),
    logo: Option<StillPicture>,
    rest_size: (u32, u32),
    frame_size: (u32, u32),
    /// Shown smaller than it is drawn, if it was too big to draw at its real size.
    shrink: f64,
    pixels: f32,
    /// What stays put, already laid out, so a frame across the whole canvas doesn't lay it again.
    base: Vec<u8>,
    boxes: Vec<Sprite>,
    /// The same boxes in white, for the bars that run across before a box fills in.
    white_boxes: Vec<Sprite>,
    letters: Vec<Letter>,
    words: usize,
    /// The colour across the whole canvas, for finishes that draw it themselves.
    fill: Option<Colour>,
    /// Where the line between the two lots of words is, for words that come out of it.
    line_y: Option<f32>,
    /// The frame's outside edge and how thick its lines are, for one that spins open.
    frame_edge: Option<([f32; 4], f32)>,
    /// Something from the library, which draws itself and has none of the above.
    art: Option<Box<MovingArt>>,
}

/// Lays a picture over the frame, at a whole pixel, with only part of it showing if asked.
fn lay_over(
    frame: &mut [u8],
    (frame_width, frame_height): (usize, usize),
    sprite: &Sprite,
    (at_x, at_y): (i32, i32),
    keep: [f32; 4],
    opacity: f32,
) {
    if opacity <= 0.0 {
        return;
    }
    let amount = (opacity.min(1.0) * 256.0) as u32;
    let first_x = (keep[0].max(0.0) * sprite.width as f32) as i32;
    let end_x = (keep[2].min(1.0) * sprite.width as f32).ceil() as i32;
    let first_y = (keep[1].max(0.0) * sprite.height as f32) as i32;
    let end_y = (keep[3].min(1.0) * sprite.height as f32).ceil() as i32;
    for from_y in first_y..end_y {
        let to_y = at_y + from_y;
        if to_y < 0 || to_y as usize >= frame_height {
            continue;
        }
        for from_x in first_x..end_x {
            let to_x = at_x + from_x;
            if to_x < 0 || to_x as usize >= frame_width {
                continue;
            }
            let from = (from_y as usize * sprite.width + from_x as usize) * 4;
            let to = (to_y as usize * frame_width + to_x as usize) * 4;
            let alpha = (u32::from(sprite.rgba[from + 3]) * amount) >> 8;
            if alpha == 0 {
                continue;
            }
            let under = 255 - alpha;
            for part in 0..3 {
                let top = (u32::from(sprite.rgba[from + part]) * amount) >> 8;
                frame[to + part] = (top + u32::from(frame[to + part]) * under / 255).min(255) as u8;
            }
            frame[to + 3] = (alpha + u32::from(frame[to + 3]) * under / 255).min(255) as u8;
        }
    }
}

/// How a letter is laid over the frame.
#[derive(Clone, Copy)]
struct Laying {
    middle: (f32, f32),
    grow: f32,
    opacity: f32,
    /// Only what falls inside this, in the frame's pixels, shows.
    clip: Option<[f32; 4]>,
    tint: [f32; 3],
}

/// Lays a picture over the frame, grown about its middle and at any position, smoothing it.
fn lay_over_smooth(
    frame: &mut [u8],
    (frame_width, frame_height): (usize, usize),
    sprite: &Sprite,
    laying: Laying,
) {
    let Laying {
        middle: (middle_x, middle_y),
        grow,
        opacity,
        clip,
        tint,
    } = laying;
    if opacity <= 0.0 || grow <= 0.01 {
        return;
    }
    let amount = (opacity.min(1.0) * 256.0) as u32;
    let half_width = sprite.width as f32 * grow / 2.0;
    let half_height = sprite.height as f32 * grow / 2.0;
    let [clip_left, clip_top, clip_right, clip_bottom] =
        clip.unwrap_or([f32::MIN, f32::MIN, f32::MAX, f32::MAX]);
    let left = ((middle_x - half_width).floor().max(clip_left.floor()) as i32).max(0);
    let right =
        ((middle_x + half_width).ceil().min(clip_right.ceil()) as i32).min(frame_width as i32);
    let top = ((middle_y - half_height).floor().max(clip_top.floor()) as i32).max(0);
    let bottom =
        ((middle_y + half_height).ceil().min(clip_bottom.ceil()) as i32).min(frame_height as i32);
    let (last_x, last_y) = (sprite.width as i32 - 1, sprite.height as i32 - 1);
    for y in top..bottom {
        let from_y = (y as f32 + 0.5 - middle_y) / grow + sprite.height as f32 / 2.0 - 0.5;
        if from_y < -0.5 || from_y > last_y as f32 + 0.5 {
            continue;
        }
        let row = from_y.floor() as i32;
        let down = from_y - row as f32;
        for x in left..right {
            let from_x = (x as f32 + 0.5 - middle_x) / grow + sprite.width as f32 / 2.0 - 0.5;
            if from_x < -0.5 || from_x > last_x as f32 + 0.5 {
                continue;
            }
            let column = from_x.floor() as i32;
            let across = from_x - column as f32;
            let read = |read_x: i32, read_y: i32| -> [f32; 4] {
                let inside = read_x >= 0 && read_y >= 0 && read_x <= last_x && read_y <= last_y;
                if !inside {
                    return [0.0; 4];
                }
                let at = (read_y as usize * sprite.width + read_x as usize) * 4;
                [
                    f32::from(sprite.rgba[at]),
                    f32::from(sprite.rgba[at + 1]),
                    f32::from(sprite.rgba[at + 2]),
                    f32::from(sprite.rgba[at + 3]),
                ]
            };
            let (a, b, c, d) = (
                read(column, row),
                read(column + 1, row),
                read(column, row + 1),
                read(column + 1, row + 1),
            );
            let mut mixed = [0.0_f32; 4];
            for part in 0..4 {
                let high = a[part] + (b[part] - a[part]) * across;
                let low = c[part] + (d[part] - c[part]) * across;
                mixed[part] = high + (low - high) * down;
            }
            let alpha = (mixed[3] as u32 * amount) >> 8;
            if alpha == 0 {
                continue;
            }
            let to = (y as usize * frame_width + x as usize) * 4;
            let under = 255 - alpha;
            for part in 0..3 {
                let top = ((mixed[part] * tint[part]) as u32 * amount) >> 8;
                frame[to + part] = (top + u32::from(frame[to + part]) * under / 255).min(255) as u8;
            }
            frame[to + 3] = (alpha + u32::from(frame[to + 3]) * under / 255).min(255) as u8;
        }
    }
}

/// A repeatable number from 0 to 1 for two whole numbers, so motion comes out the same every time,
/// in the preview and the export.
fn scatter(a: i64, b: i64) -> f32 {
    let mut value = (a as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add((b as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F));
    value ^= value >> 29;
    value = value.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value ^= value >> 32;
    (value & 0xFFFF) as f32 / 65535.0
}

/// Smooth noise from 0 to 1, built from `scatter` on a grid.
fn smooth_noise(x: f32, y: f32) -> f32 {
    let (cell_x, cell_y) = (x.floor(), y.floor());
    let (along_x, along_y) = (x - cell_x, y - cell_y);
    let ease = |t: f32| t * t * (3.0 - 2.0 * t);
    let (ease_x, ease_y) = (ease(along_x), ease(along_y));
    let at = |a: f32, b: f32| scatter((cell_x + a) as i64, (cell_y + b) as i64);
    let top = at(0.0, 0.0) + (at(1.0, 0.0) - at(0.0, 0.0)) * ease_x;
    let bottom = at(0.0, 1.0) + (at(1.0, 1.0) - at(0.0, 1.0)) * ease_x;
    top + (bottom - top) * ease_y
}

fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

fn darker(colour: Colour) -> Colour {
    colour.map(|part| (f32::from(part) * DARKER) as u8)
}

/// A soft-edged colour from a hue, for letters that keep changing colour.
fn pastel(hue: f32) -> [f32; 3] {
    let hue = hue.rem_euclid(1.0) * 6.0;
    let part = |offset: f32| {
        let away = ((hue - offset).rem_euclid(6.0) - 3.0).abs();
        (away - 1.0).clamp(0.0, 1.0)
    };
    [part(0.0), part(2.0), part(4.0)].map(|value| 0.55 + value * 0.45)
}

/// Lays one colour over a pixel.
fn blend_pixel(frame: &mut [u8], width: usize, x: usize, y: usize, colour: Colour, amount: f32) {
    let at = (y * width + x) * 4;
    let keep = 1.0 - amount;
    for part in 0..3 {
        let new = f32::from(colour[part]) * amount + f32::from(frame[at + part]) * keep;
        frame[at + part] = new.min(255.0) as u8;
    }
    frame[at + 3] = (255.0 * amount + f32::from(frame[at + 3]) * keep).min(255.0) as u8;
}

/// Fills a box in the frame's pixels with a solid colour.
fn fill_rect(frame: &mut [u8], (width, height): (usize, usize), rect: [f32; 4], colour: Colour) {
    let left = rect[0].round().clamp(0.0, width as f32) as usize;
    let right = rect[2].round().clamp(0.0, width as f32) as usize;
    let top = rect[1].round().clamp(0.0, height as f32) as usize;
    let bottom = rect[3].round().clamp(0.0, height as f32) as usize;
    for y in top..bottom {
        for x in left..right {
            let at = (y * width + x) * 4;
            frame[at..at + 3].copy_from_slice(&colour);
            frame[at + 3] = 255;
        }
    }
}

/// Fills a circle, softening its edge.
fn fill_circle(
    frame: &mut [u8],
    (width, height): (usize, usize),
    (x, y): (f32, f32),
    radius: f32,
    colour: Colour,
) {
    let left = ((x - radius - 1.0) as i32).max(0);
    let right = ((x + radius + 2.0) as i32).min(width as i32);
    let top = ((y - radius - 1.0) as i32).max(0);
    let bottom = ((y + radius + 2.0) as i32).min(height as i32);
    for py in top..bottom {
        for px in left..right {
            let away = (px as f32 + 0.5 - x).hypot(py as f32 + 0.5 - y);
            let amount = (radius + 0.5 - away).clamp(0.0, 1.0);
            if amount > 0.0 {
                blend_pixel(frame, width, px as usize, py as usize, colour, amount);
            }
        }
    }
}

/// Draws a thick line through a point at an angle, softening its edges.
fn fill_line(
    frame: &mut [u8],
    (width, height): (usize, usize),
    (x, y): (f32, f32),
    (angle, length, thick): (f32, f32, f32),
    colour: Colour,
) {
    let (sin, cos) = angle.sin_cos();
    let reach = length / 2.0 + thick;
    let left = ((x - reach) as i32).max(0);
    let right = ((x + reach) as i32 + 1).min(width as i32);
    let top = ((y - reach) as i32).max(0);
    let bottom = ((y + reach) as i32 + 1).min(height as i32);
    for py in top..bottom {
        for px in left..right {
            let (dx, dy) = (px as f32 + 0.5 - x, py as f32 + 0.5 - y);
            let along = (dx * cos + dy * sin).abs() - length / 2.0;
            let beside = (dy * cos - dx * sin).abs() - thick / 2.0;
            let outside = along.max(beside);
            let amount = (0.5 - outside).clamp(0.0, 1.0);
            if amount > 0.0 {
                blend_pixel(frame, width, px as usize, py as usize, colour, amount);
            }
        }
    }
}

/// A pixel a letter covers: where, how much, and in what colour.
type LetterPixel = (i32, i32, f32, [f32; 3]);

/// One letter's picture, with everything its style adds, and where it goes in the frame.
fn letter_sprite(glyph: &[LetterPixel], title: &Title, pixels: f32, reach: f32) -> Option<Sprite> {
    let room = reach.ceil() as i32 + 1;
    let left = glyph.iter().map(|pixel| pixel.0).min()? - room;
    let top = glyph.iter().map(|pixel| pixel.1).min()? - room;
    let right = glyph.iter().map(|pixel| pixel.0).max()? + room + 1;
    let bottom = glyph.iter().map(|pixel| pixel.1).max()? + room + 1;
    let (width, height) = ((right - left) as usize, (bottom - top) as usize);
    let mut ink = Ink::new(width, height);
    for (x, y, amount, colour) in glyph {
        let at = (y - top) as usize * width + (x - left) as usize;
        if *amount > ink.covered[at] {
            ink.covered[at] = *amount;
            ink.colours[at] = *colour;
        }
    }
    let mut paint = Paint::new(width as u32, height as u32);
    ink.paint_styled(&mut paint, title, pixels);
    Some(Sprite::from_paint(paint, left as f32, top as f32))
}

/// A letter's pixels, and which word it is in from the start of its words.
type FoundGlyph = (Vec<LetterPixel>, usize);

/// Each letter of some words, as the pixels it covers, in reading order.
fn glyphs_of(
    engine: &mut Engine,
    words: &crate::draw::Words,
    (x, y): (f32, f32),
    colour: Colour,
    left_aligned: bool,
) -> Vec<FoundGlyph> {
    let base = Color::rgb(colour[0], colour[1], colour[2]);
    let Engine { system, glyphs, .. } = engine;
    let mut found = Vec::new();
    let mut words_before = 0;
    for run in words.buffer.layout_runs() {
        let across = if left_aligned {
            0.0
        } else {
            (words.width - run.line_w) / 2.0
        };
        let text = run.text;
        let mut last_word = None;
        for glyph in run.glyphs {
            let before = &text[..glyph.start.min(text.len())];
            let word = words_before + before.split_whitespace().count()
                - usize::from(before.ends_with(|c: char| !c.is_whitespace()));
            last_word = Some(word);
            let physical = glyph.physical((x + across, y + run.line_y), 1.0);
            let mut pixels = Vec::new();
            glyphs.with_pixels(system, physical.cache_key, base, |gx, gy, pixel| {
                let amount = f32::from(pixel.a()) / 255.0;
                if amount > 0.0 {
                    let colour = [
                        f32::from(pixel.r()) / 255.0,
                        f32::from(pixel.g()) / 255.0,
                        f32::from(pixel.b()) / 255.0,
                    ];
                    pixels.push((physical.x + gx, physical.y + gy, amount, colour));
                }
            });
            if !pixels.is_empty() {
                found.push((pixels, word));
            }
        }
        words_before = last_word.map_or(words_before, |word| word + 1);
    }
    found
}

impl MovingTitle {
    /// Gets a title ready. This takes a moment, so it is only done for titles about to be shown.
    pub fn prepare(title: &Title, canvas: (u32, u32), logo: Option<&StillPicture>) -> Self {
        if let Some(art) = &title.art {
            return Self {
                title: title.clone(),
                motion: title.style.motion(),
                canvas,
                logo: None,
                rest_size: (1, 1),
                frame_size: (1, 1),
                shrink: 1.0,
                pixels: 1.0,
                base: Vec::new(),
                boxes: Vec::new(),
                white_boxes: Vec::new(),
                letters: Vec::new(),
                words: 0,
                fill: None,
                line_y: None,
                frame_edge: None,
                art: Some(Box::new(MovingArt::prepare(title, art, canvas))),
            };
        }
        let mut title = title.clone();
        if title.style.motion().finish == Finish::Karaoke {
            // Each letter is painted in its colour, and the swept copy in the accent's.
            title.second_text.clear();
        }
        let motion = title.style.motion();
        let rest = draw_title_with_logo(&title, canvas, logo);
        let rest_size = (rest.picture.width, rest.picture.height);
        let shrink = Self::shrink_of(canvas, rest_size, rest.scale);
        let pixels = letter_pixels(&title, canvas) * shrink as f32;
        let mut moving = Self {
            title: title.clone(),
            motion,
            canvas,
            logo: logo.cloned(),
            rest_size,
            frame_size: rest_size,
            shrink,
            pixels,
            base: Vec::new(),
            boxes: Vec::new(),
            white_boxes: Vec::new(),
            letters: Vec::new(),
            words: 0,
            fill: None,
            line_y: None,
            frame_edge: None,
            art: None,
        };
        // These are drawn again for each moment, so they need nothing laid out.
        if matches!(motion.running, Running::Countdown | Running::WordByWord)
            || motion.letters == LetterMove::Typewriter
        {
            return moving;
        }
        let (font, _) = font_for(&title);
        let font = font.to_owned();
        let mut engine = engine();
        let plan = plan_title(&mut engine, &title, &font, canvas, pixels);
        moving.lay_out_parts(&mut engine, &plan);
        moving
    }

    /// How much smaller than its real size it was drawn.
    fn shrink_of(canvas: (u32, u32), size: (u32, u32), scale: i32) -> f64 {
        let at_real_size = scale_for_pixels(size, canvas, 1.0);
        if scale == at_real_size {
            return 1.0;
        }
        f64::from(at_real_size) / f64::from(scale.max(1))
    }

    #[allow(clippy::too_many_lines)] // It sets out every part of a title in one place.
    fn lay_out_parts(&mut self, engine: &mut Engine, plan: &Plan) {
        let title = self.title.clone();
        let look = title.style.look();
        let pixels = self.pixels;
        let room = (pixels * ROOM).ceil();
        let (rest_width, rest_height) = self.rest_size;
        // A title across the whole canvas has nothing to move into beyond it.
        let exact = plan.layout.exact;
        let room = if exact { 0.0 } else { room };
        let wanted = (rest_width + room as u32 * 2, rest_height + room as u32 * 2);
        let room = if wanted.0 > LARGEST_SIDE || wanted.1 > LARGEST_SIDE {
            0.0
        } else {
            room
        };
        self.frame_size = (rest_width + room as u32 * 2, rest_height + room as u32 * 2);
        let left = plan.inset.0 + room;
        let top = plan.inset.1 + room;
        let mut fixed = Vec::new();
        let draws_its_own_fill = matches!(self.motion.finish, Finish::Iris | Finish::Sweep);

        for (index, (area, radius, colour)) in plan.layout.boxes.iter().enumerate() {
            if index < plan.layout.fixed_boxes && draws_its_own_fill {
                self.fill = Some(*colour);
                continue;
            }
            let painted = |colour: Colour| {
                let (width, height) = (area.width.ceil() as u32 + 2, area.height.ceil() as u32 + 2);
                let mut paint = Paint::new(width, height);
                paint.rectangle(
                    Area {
                        x: 1.0,
                        y: 1.0,
                        width: area.width,
                        height: area.height,
                    },
                    *radius,
                    colour,
                );
                Sprite::from_paint(paint, area.x + left - 1.0, area.y + top - 1.0)
            };
            // The fill behind an intro whose own boxes move stays put.
            let stays = index < plan.layout.fixed_boxes && self.motion.boxes != BoxMove::Grow
                || index < plan.layout.fixed_boxes
                    && !matches!(look.shape, TitleShape::IntroTwoColours);
            if stays {
                fixed.push(painted(*colour));
            } else {
                if self.motion.boxes == BoxMove::Bars {
                    self.white_boxes.push(painted(WHITE));
                }
                self.boxes.push(painted(*colour));
            }
        }
        if look.shape == TitleShape::LineBetween {
            self.line_y = self
                .boxes
                .first()
                .map(|line| line.y + line.height as f32 / 2.0);
        }
        if look.shape == TitleShape::Frame && self.motion.finish == Finish::SpinOpen {
            // Each box's picture has a pixel of room around it, which isn't part of the line.
            let rects: Vec<[f32; 4]> = self
                .boxes
                .iter()
                .map(|sprite| {
                    let rect = sprite.rect();
                    [rect[0] + 1.0, rect[1] + 1.0, rect[2] - 1.0, rect[3] - 1.0]
                })
                .collect();
            let edge = rects
                .iter()
                .fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |all, rect| {
                    [
                        all[0].min(rect[0]),
                        all[1].min(rect[1]),
                        all[2].max(rect[2]),
                        all[3].max(rect[3]),
                    ]
                });
            let thick = rects
                .iter()
                .map(|rect| (rect[2] - rect[0]).min(rect[3] - rect[1]))
                .fold(f32::MAX, f32::min);
            self.frame_edge = Some((edge, thick));
        }
        if let (Some(area), Some(logo)) = (&plan.layout.logo, &self.logo) {
            let mut paint = Paint::new(area.width.ceil() as u32 + 1, area.height.ceil() as u32 + 1);
            paint.picture_in(
                logo,
                Area {
                    x: 0.0,
                    y: 0.0,
                    width: area.width,
                    height: area.height,
                },
            );
            fixed.push(Sprite::from_paint(paint, area.x + left, area.y + top));
        }
        let size = (self.frame_size.0 as usize, self.frame_size.1 as usize);
        self.base = vec![0; size.0 * size.1 * 4];
        for sprite in &fixed {
            let at = (sprite.x.round() as i32, sprite.y.round() as i32);
            lay_over(&mut self.base, size, sprite, at, [0.0, 0.0, 1.0, 1.0], 1.0);
        }

        let reach = reach_past_words(&look, pixels);
        let (first_colour, second_colour) = word_colours(&title);
        let swept = self.motion.finish == Finish::Karaoke;
        let origin = |place: (f32, f32)| ((place.0 + left).round(), (place.1 + top).round());
        let letter = |sprite, part, word| Letter {
            sprite,
            swept: None,
            place: 0.0,
            part,
            word,
        };
        let mut marks = Vec::new();
        if let (Some(quote_marks), Some(places)) = (&plan.marks, plan.layout.marks) {
            for (words, place) in [(&quote_marks[0], places[0]), (&quote_marks[1], places[1])] {
                let glyphs = glyphs_of(engine, words, origin(place), title.accent, true);
                marks.extend(glyphs.iter().filter_map(|(glyph, _)| {
                    letter_sprite(glyph, &title, pixels, 2.0)
                        .map(|sprite| letter(sprite, Part::Mark, 0))
                }));
            }
        }
        let first = glyphs_of(
            engine,
            &plan.first,
            origin(plan.layout.first),
            first_colour,
            look.left_aligned,
        );
        let accented = swept.then(|| {
            glyphs_of(
                engine,
                &plan.first,
                origin(plan.layout.first),
                title.accent,
                look.left_aligned,
            )
        });
        let mut first_letters = Vec::new();
        for (index, (glyph, word)) in first.iter().enumerate() {
            if let Some(sprite) = letter_sprite(glyph, &title, pixels, reach) {
                let swept = accented
                    .as_ref()
                    .and_then(|all| all.get(index))
                    .and_then(|(glyph, _)| letter_sprite(glyph, &title, pixels, reach));
                first_letters.push(Letter {
                    swept,
                    ..letter(sprite, Part::First, *word)
                });
            }
        }
        let first_words = first.iter().map(|(_, word)| word + 1).max().unwrap_or(0);
        let mut stars = Vec::new();
        if let Some(row) = plan.layout.stars {
            for index in 0..5 {
                let (middle, star_reach, colour) = star_at(&row, index, &title);
                let side = (star_reach * 2.0).ceil() as u32 + 4;
                let mut paint = Paint::new(side, side);
                paint.star((side as f32 / 2.0, side as f32 / 2.0), star_reach, colour);
                let sprite = Sprite::from_paint(
                    paint,
                    middle.0 + left - side as f32 / 2.0,
                    middle.1 + top - side as f32 / 2.0,
                );
                stars.push(letter(sprite, Part::Star, 0));
            }
        }
        let mut second_letters = Vec::new();
        let mut second_words = 0;
        if let (Some(words), Some(place)) = (&plan.second, plan.layout.second) {
            let glyphs = glyphs_of(
                engine,
                words,
                origin(place),
                second_colour,
                look.left_aligned,
            );
            second_words = glyphs.iter().map(|(_, word)| word + 1).max().unwrap_or(0);
            second_letters.extend(glyphs.iter().filter_map(|(glyph, word)| {
                letter_sprite(glyph, &title, pixels, reach)
                    .map(|sprite| letter(sprite, Part::Second, first_words + word))
            }));
        }
        // A rating's stars come in before its words, as in the recording.
        let mut letters = marks;
        if look.shape == TitleShape::Stars {
            letters.extend(stars);
            letters.extend(first_letters);
        } else {
            letters.extend(first_letters);
            letters.extend(stars);
        }
        letters.extend(second_letters);
        self.words = first_words + second_words;
        let middle = self.frame_size.0 as f32 / 2.0;
        let widest = letters
            .iter()
            .map(|letter| (letter.sprite.middle().0 - middle).abs())
            .fold(1.0_f32, f32::max);
        for letter in &mut letters {
            letter.place = (letter.sprite.middle().0 - middle) / widest;
        }
        self.letters = letters;
    }

    /// The scale that shows a picture of this size at the title's real size.
    pub fn scale_for(&self, picture: (u32, u32)) -> i32 {
        if let Some(art) = &self.art {
            return art.scale_for(picture);
        }
        scale_for_pixels(picture, self.canvas, self.shrink)
    }

    /// The picture for this moment. Its size can differ from the resting one, around the same middle.
    pub fn frame(&self, stage: Stage) -> StillPicture {
        if let Some(art) = &self.art {
            return art.frame(stage);
        }
        if self.motion.running == Running::Countdown {
            return self.drawn_with(&TitleMotion::countdown_words(stage));
        }
        if self.motion.running == Running::WordByWord {
            return self.one_word(stage);
        }
        if self.motion.letters == LetterMove::Typewriter {
            return self.typewriter(stage);
        }
        let (width, height) = (self.frame_size.0 as usize, self.frame_size.1 as usize);
        let size = (width, height);
        let mut frame = self.base.clone();
        let clip = match self.motion.finish {
            Finish::Iris => {
                self.iris(&mut frame, size, stage);
                None
            }
            Finish::Sweep => Some(self.sweep(&mut frame, size, stage)),
            Finish::SpinOpen => Some(self.spin_open(&mut frame, size, stage)),
            _ => None,
        };
        let box_clips = self.lay_boxes(&mut frame, size, stage);
        self.lay_letters(&mut frame, size, stage, clip, &box_clips);
        match self.motion.finish {
            Finish::Glitch => self.glitch(&mut frame, size, stage),
            Finish::Smoke => self.smoke(&mut frame, size, stage),
            Finish::Fireworks => self.fireworks(&mut frame, size, stage),
            _ => {}
        }
        StillPicture::new(self.frame_size.0, self.frame_size.1, frame)
    }

    /// The title drawn with other words, for the styles whose words change.
    fn drawn_with(&self, words: &str) -> StillPicture {
        let mut title = self.title.clone();
        title.text = words.to_owned();
        draw_title_with_logo(&title, self.canvas, self.logo.as_ref()).picture
    }

    fn one_word(&self, stage: Stage) -> StillPicture {
        let words: Vec<&str> = self.title.text.split_whitespace().collect();
        if words.is_empty() {
            return self.drawn_with("");
        }
        let showing = TitleMotion::word_showing(stage, words.len());
        self.drawn_with(words[showing])
    }

    /// The words typed so far, kept centred, with the cursor after them when it shows.
    fn typewriter(&self, stage: Stage) -> StillPicture {
        let letters: Vec<char> = self.title.text.chars().collect();
        let (shown, cursor) = TitleMotion::typed(stage, letters.len());
        let typed: String = letters[..shown].iter().collect();
        let mut picture = self.drawn_with(&typed);
        if !cursor {
            return picture;
        }
        let (width, height) = (picture.width as usize, picture.height as usize);
        let inked = |x: usize, y: usize| picture.rgba[(y * width + x) * 4 + 3] > 40;
        let right = (0..width)
            .rev()
            .find(|x| (0..height).any(|y| inked(*x, y)))
            .map_or(width as f32 / 2.0, |x| x as f32 + 1.0);
        let thick = (self.pixels * 0.05).max(2.0);
        let tall = self.pixels * 1.05;
        let x = (right + self.pixels * 0.03).min(width as f32 - thick);
        let y = (height as f32 - tall) / 2.0;
        let colour = self.title.colour;
        for row in (y.max(0.0) as usize)..((y + tall) as usize).min(height) {
            for column in (x as usize)..((x + thick) as usize).min(width) {
                let at = (row * width + column) * 4;
                picture.rgba[at..at + 3].copy_from_slice(&colour);
                picture.rgba[at + 3] = 255;
            }
        }
        picture
    }

    /// How far in the finish is: coming in, or going back out at the end.
    fn finish_amount(&self, stage: Stage) -> f32 {
        if stage.leaving > 0.0 {
            1.0 - stage.leaving
        } else {
            stage.entering
        }
    }

    /// A circle with a darker ring around it, opening out from the middle.
    fn iris(&self, frame: &mut [u8], size: (usize, usize), stage: Stage) {
        let Some(fill) = self.fill else {
            return;
        };
        let amount = ease_out(self.finish_amount(stage));
        let middle = (size.0 as f32 / 2.0, size.1 as f32 / 2.0);
        let corner = middle.0.hypot(middle.1);
        let outer = amount * corner * 1.4;
        let inner = outer * 0.78;
        if inner >= corner {
            fill_rect(frame, size, [0.0, 0.0, size.0 as f32, size.1 as f32], fill);
            return;
        }
        fill_circle(frame, size, middle, outer, darker(fill));
        fill_circle(frame, size, middle, inner, fill);
    }

    /// The colour sweeping across with a darker band at its edge. Returns where the words can show.
    fn sweep(&self, frame: &mut [u8], size: (usize, usize), stage: Stage) -> [f32; 4] {
        let Some(fill) = self.fill else {
            return [0.0, 0.0, size.0 as f32, size.1 as f32];
        };
        let (width, height) = (size.0 as f32, size.1 as f32);
        let band = width * SWEEP_BAND;
        let (light, dark) = if stage.leaving > 0.0 {
            let behind = ease_out(stage.leaving) * (width + band) - band;
            ([behind + band, width], [behind, behind + band])
        } else {
            let lead = ease_out(stage.entering) * (width + band);
            ([0.0, lead - band], [lead - band, lead])
        };
        fill_rect(frame, size, [light[0], 0.0, light[1], height], fill);
        fill_rect(frame, size, [dark[0], 0.0, dark[1], height], darker(fill));
        [light[0], 0.0, light[1].max(light[0]), height]
    }

    /// A line spinning in and opening out into the frame. Returns where the words can show.
    fn spin_open(&self, frame: &mut [u8], size: (usize, usize), stage: Stage) -> [f32; 4] {
        let Some((edge, thick)) = self.frame_edge else {
            return [0.0, 0.0, size.0 as f32, size.1 as f32];
        };
        let amount = self.motion.parts_amount(stage);
        let middle = ((edge[0] + edge[2]) / 2.0, (edge[1] + edge[3]) / 2.0);
        let tall = edge[3] - edge[1];
        let colour = self.title.accent;
        let spinning = 0.45;
        if amount < spinning {
            let along = amount / spinning;
            let angle = std::f32::consts::FRAC_PI_2 + (1.0 - along) * std::f32::consts::PI;
            fill_line(
                frame,
                size,
                middle,
                (angle, tall * ease_out(along), thick),
                colour,
            );
            return [middle.0, middle.1, middle.0, middle.1];
        }
        let open = ease_out((amount - spinning) / (1.0 - spinning));
        let half = (thick + (edge[2] - edge[0] - thick) * open) / 2.0;
        let (left, right) = (middle.0 - half, middle.0 + half);
        if open >= 0.999 {
            // Fully open, it is the frame at rest, drawn the same way.
            for sprite in &self.boxes {
                let at = (sprite.x.round() as i32, sprite.y.round() as i32);
                lay_over(frame, size, sprite, at, [0.0, 0.0, 1.0, 1.0], 1.0);
            }
            return [left + thick, edge[1], right - thick, edge[3]];
        }
        fill_rect(frame, size, [left, edge[1], left + thick, edge[3]], colour);
        fill_rect(
            frame,
            size,
            [right - thick, edge[1], right, edge[3]],
            colour,
        );
        fill_rect(frame, size, [left, edge[1], right, edge[1] + thick], colour);
        fill_rect(frame, size, [left, edge[3] - thick, right, edge[3]], colour);
        [left + thick, edge[1], right - thick, edge[3]]
    }

    /// The part of a box that shows, in the frame's pixels.
    fn box_visible(&self, index: usize, sprite: &Sprite, share: f32) -> [f32; 4] {
        let [left, top, right, bottom] = sprite.rect();
        let (width, height) = (right - left, bottom - top);
        let (middle_x, middle_y) = sprite.middle();
        match self.motion.boxes {
            BoxMove::None => [left, top, right, bottom],
            BoxMove::Wipe | BoxMove::Bars => [left, top, left + width * share, bottom],
            BoxMove::Glide if index % 2 == 1 => [right - width * share, top, right, bottom],
            BoxMove::Glide => [left, top, left + width * share, bottom],
            BoxMove::Grow if width >= height => [
                middle_x - width * share / 2.0,
                top,
                middle_x + width * share / 2.0,
                bottom,
            ],
            BoxMove::Grow => [
                left,
                middle_y - height * share / 2.0,
                right,
                middle_y + height * share / 2.0,
            ],
            BoxMove::Unfold if index == 0 => [
                middle_x - width * share / 2.0,
                middle_y - height * share / 2.0,
                middle_x + width * share / 2.0,
                middle_y + height * share / 2.0,
            ],
            BoxMove::Unfold => [left, top, right, top + height * share],
        }
    }

    /// Lays the boxes as far as they show. Returns where each shows, for the words inside them.
    fn lay_boxes(&self, frame: &mut [u8], size: (usize, usize), stage: Stage) -> Vec<[f32; 4]> {
        let count = self.boxes.len();
        let mut shown = Vec::with_capacity(count);
        // A frame that spins open draws its own lines.
        if self.motion.finish == Finish::SpinOpen {
            return shown;
        }
        for (index, sprite) in self.boxes.iter().enumerate() {
            let share = self.motion.box_share(stage, index, count);
            let at = (sprite.x.round() as i32, sprite.y.round() as i32);
            let keep_of = |rect: [f32; 4]| {
                [
                    (rect[0] - sprite.x) / sprite.width as f32,
                    (rect[1] - sprite.y) / sprite.height as f32,
                    (rect[2] - sprite.x) / sprite.width as f32,
                    (rect[3] - sprite.y) / sprite.height as f32,
                ]
            };
            if self.motion.boxes == BoxMove::Bars {
                // A white bar runs across first, and then the colour fills down over it.
                let rect = sprite.rect();
                let tall = rect[3] - rect[1];
                if share < 0.5 {
                    let across = share / 0.5;
                    let bar = [
                        rect[0],
                        rect[1] + tall * 0.38,
                        rect[0] + (rect[2] - rect[0]) * across,
                        rect[1] + tall * 0.62,
                    ];
                    lay_over(frame, size, &self.white_boxes[index], at, keep_of(bar), 1.0);
                    shown.push([rect[0], rect[1], rect[0], rect[1]]);
                } else {
                    let down = (share - 0.5) / 0.5;
                    let filled = [rect[0], rect[1], rect[2], rect[1] + tall * down];
                    let white = [rect[0], rect[1] + tall * down, rect[2], rect[3]];
                    lay_over(
                        frame,
                        size,
                        &self.white_boxes[index],
                        at,
                        keep_of(white),
                        1.0 - down,
                    );
                    lay_over(frame, size, sprite, at, keep_of(filled), 1.0);
                    shown.push(filled);
                }
                continue;
            }
            let visible = self.box_visible(index, sprite, share);
            lay_over(frame, size, sprite, at, keep_of(visible), 1.0);
            shown.push(visible);
        }
        shown
    }

    /// Where a letter may show: inside the box it sits in, while that box is coming in.
    fn letter_clip(&self, letter: &Letter, box_clips: &[[f32; 4]]) -> Option<[f32; 4]> {
        if matches!(self.motion.boxes, BoxMove::None | BoxMove::Grow) {
            return None;
        }
        let (x, y) = letter.sprite.middle();
        self.boxes
            .iter()
            .zip(box_clips)
            .rev()
            .find(|(sprite, _)| {
                let rect = sprite.rect();
                x >= rect[0] && x <= rect[2] && y >= rect[1] && y <= rect[3]
            })
            .map(|(_, clip)| *clip)
    }

    fn lay_letters(
        &self,
        frame: &mut [u8],
        size: (usize, usize),
        stage: Stage,
        clip: Option<[f32; 4]>,
        box_clips: &[[f32; 4]],
    ) {
        let count = self.letters.len();
        let swept_to = stage.through * count as f32;
        let grow_all = if self.motion.finish == Finish::Iris {
            0.3 + 0.7 * ease_out(self.finish_amount(stage))
        } else {
            1.0
        };
        for (index, letter) in self.letters.iter().enumerate() {
            let pose = self.motion.letter_pose(stage, index, count, letter.place);
            let mut opacity = pose.opacity;
            let mut down = pose.down * self.pixels;
            let mut letter_clip = clip.or_else(|| self.letter_clip(letter, box_clips));
            if self.motion.letters == LetterMove::Flicker {
                opacity *= self.motion.word_flicker(stage, letter.word, self.words);
            }
            if self.motion.letters == LetterMove::FromLine
                && let Some(line) = self.line_y
            {
                // Each lot of words slides out from behind the line, away from it.
                let reach = self.pixels * 1.3;
                if letter.part == Part::Second {
                    down = -pose.down * reach;
                    letter_clip = Some([f32::MIN, line, f32::MAX, f32::MAX]);
                } else {
                    down = pose.down * reach;
                    letter_clip = Some([f32::MIN, f32::MIN, f32::MAX, line]);
                }
            }
            let tint = if self.motion.finish == Finish::ColourCycle {
                pastel(stage.seconds * 0.35 + index as f32 * 0.05)
            } else {
                UNTINTED
            };
            let (middle_x, middle_y) = letter.sprite.middle();
            let centre = (size.0 as f32 / 2.0, size.1 as f32 / 2.0);
            // Growing everything together goes about the middle of the frame.
            let at = (
                centre.0 + (middle_x + pose.across * self.pixels - centre.0) * grow_all,
                centre.1 + (middle_y + down - centre.1) * grow_all,
            );
            let laying = Laying {
                middle: at,
                grow: pose.grow * grow_all,
                opacity,
                clip: letter_clip,
                tint,
            };
            // Laying the swept letter over the plain one would darken its shadow, so a letter the
            // sweep has passed is only drawn the once.
            let swept = letter
                .swept
                .as_ref()
                .map_or(0.0, |_| (swept_to - index as f32).clamp(0.0, 1.0));
            if swept < 1.0 {
                lay_over_smooth(frame, size, &letter.sprite, laying);
            }
            if let (Some(sprite), true) = (&letter.swept, swept > 0.0) {
                let laying = Laying {
                    opacity: opacity * swept,
                    ..laying
                };
                lay_over_smooth(frame, size, sprite, laying);
            }
        }
    }

    /// Slices the picture sideways, and pulls red and blue apart.
    fn glitch(&self, frame: &mut [u8], (width, height): (usize, usize), stage: Stage) {
        if !TitleMotion::glitching(stage) {
            return;
        }
        let beat = (stage.seconds * 24.0) as i64;
        let slice = (self.pixels * 0.2).max(2.0) as usize;
        let split = (self.pixels * 0.05).max(1.0) as usize;
        let reach = self.pixels * 0.5;
        let copy = frame.to_vec();
        for y in 0..height {
            let band = (y / slice) as i64;
            let chance = scatter(band, beat);
            let shift = if chance > 0.55 {
                ((scatter(beat, band) - 0.5) * 2.0 * reach) as isize
            } else {
                0
            };
            let row = y * width * 4;
            for x in 0..width {
                let read = |offset: isize, part: usize| -> u8 {
                    let from = x as isize - shift + offset;
                    if from < 0 || from >= width as isize {
                        0
                    } else {
                        copy[row + from as usize * 4 + part]
                    }
                };
                let to = row + x * 4;
                frame[to] = read(split as isize, 0);
                frame[to + 1] = read(0, 1);
                frame[to + 2] = read(-(split as isize), 2);
                frame[to + 3] = read(split as isize, 3)
                    .max(read(0, 3))
                    .max(read(-(split as isize), 3));
            }
        }
    }

    /// Lets the picture through a cloud of noise that thickens as it comes in and goes out.
    fn smoke(&self, frame: &mut [u8], (width, height): (usize, usize), stage: Stage) {
        let amount = stage.entering.min(1.0 - stage.leaving);
        if amount >= 1.0 {
            return;
        }
        let spread = self.pixels * 0.45;
        let drift = stage.seconds * 1.5;
        for y in 0..height {
            for x in 0..width {
                let at = (y * width + x) * 4;
                if frame[at + 3] == 0 {
                    continue;
                }
                let (cloud_x, cloud_y) = (x as f32 / spread, y as f32 / spread + drift);
                let cloud = smooth_noise(cloud_x, cloud_y) * 0.65
                    + smooth_noise(cloud_x * 2.1, cloud_y * 2.1) * 0.35;
                // The cloud is mostly between 0.2 and 0.8, so that is what `amount` has to cross.
                let edge = amount * 1.2 - 0.1;
                let show = ((edge - cloud) / 0.18 + 0.5).clamp(0.0, 1.0) * amount.sqrt();
                for part in &mut frame[at..at + 4] {
                    *part = (f32::from(*part) * show) as u8;
                }
            }
        }
    }

    /// Rings and sparks burst around the words, one after another.
    fn fireworks(&self, frame: &mut [u8], (width, height): (usize, usize), stage: Stage) {
        const BURSTS: i64 = 3;
        const SPARKS: i64 = 12;
        for burst in 0..BURSTS {
            let starts = burst as f32 * 0.04;
            let age = (stage.through - starts) / 0.2;
            if !(0.0..1.0).contains(&age) {
                continue;
            }
            let centre_x = width as f32 * (0.3 + scatter(burst, 1) * 0.4);
            let centre_y = height as f32 * (0.4 + scatter(burst, 2) * 0.2);
            let reach = self.pixels * (0.5 + scatter(burst, 3) * 0.4);
            let colour = if burst % 2 == 0 {
                self.title.accent
            } else {
                SPARK_COLOURS[(burst as usize / 2) % SPARK_COLOURS.len()]
            };
            let fade = 1.0 - age;
            let radius = reach * (1.0 - (1.0 - age).powi(3));
            for index in 0..SPARKS {
                let angle = index as f32 / SPARKS as f32 * std::f32::consts::TAU;
                let x = centre_x + angle.cos() * radius;
                let y = centre_y + angle.sin() * radius;
                let dot = (self.pixels * 0.03).max(1.5);
                if x >= 0.0 && y >= 0.0 {
                    fill_circle(frame, (width, height), (x, y), dot, colour);
                }
            }
            if age < 0.8 {
                self.ring(
                    frame,
                    (width, height),
                    (centre_x, centre_y),
                    radius,
                    colour,
                    fade * 0.8,
                );
            }
        }
    }

    /// A thin circle of colour.
    #[allow(clippy::unused_self)]
    fn ring(
        &self,
        frame: &mut [u8],
        (width, height): (usize, usize),
        (x, y): (f32, f32),
        radius: f32,
        colour: Colour,
        fade: f32,
    ) {
        let thick = 1.6;
        let reach = radius + thick + 1.0;
        let left = ((x - reach) as i32).max(0);
        let right = ((x + reach) as i32).min(width as i32);
        let top = ((y - reach) as i32).max(0);
        let bottom = ((y + reach) as i32).min(height as i32);
        for py in top..bottom {
            for px in left..right {
                let away = ((px as f32 + 0.5 - x).hypot(py as f32 + 0.5 - y) - radius).abs();
                let amount = (thick + 0.5 - away).clamp(0.0, 1.0) * fade;
                if amount > 0.0 {
                    blend_pixel(frame, width, px as usize, py as usize, colour, amount);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use bareclip_core::{
        placement::{Placement, WHOLE},
        title::{STYLE_GROUPS, TitleStyle},
    };

    use super::*;
    use crate::draw_title;

    const CANVAS: (u32, u32) = (1280, 720);
    const LENGTH: f32 = 5.0;

    fn every_moving_style() -> Vec<TitleStyle> {
        STYLE_GROUPS
            .iter()
            .flat_map(|(_, styles)| styles.iter().copied())
            .filter(|style| style.motion().draws_frames())
            .collect()
    }

    fn solid_pixels(picture: &StillPicture) -> usize {
        picture
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| pixel[3] > 128)
            .count()
    }

    fn frame_at(style: TitleStyle, seconds: f32) -> StillPicture {
        let title = Title::in_style(style);
        let moving = MovingTitle::prepare(&title, CANVAS, None);
        moving.frame(style.motion().stage(seconds, LENGTH))
    }

    #[test]
    fn plenty_of_styles_move_in_parts() {
        assert!(every_moving_style().len() >= 20);
    }

    #[test]
    fn every_frame_of_every_moving_style_can_be_drawn() {
        for style in every_moving_style() {
            let title = Title::in_style(style);
            let moving = MovingTitle::prepare(&title, CANVAS, None);
            let motion = style.motion();
            for tenth in 0..=50 {
                let stage = motion.stage(tenth as f32 / 10.0, LENGTH);
                let frame = moving.frame(stage);
                assert_eq!(
                    frame.rgba.len(),
                    (frame.width * frame.height * 4) as usize,
                    "{style:?}"
                );
            }
        }
    }

    #[test]
    fn a_moving_title_ends_up_covering_as_much_as_its_resting_picture() {
        for style in every_moving_style() {
            let motion = style.motion();
            if matches!(
                motion.finish,
                Finish::Smoke | Finish::Glitch | Finish::Fireworks | Finish::ColourCycle
            ) || motion.running != Running::None
                || motion.letters == LetterMove::Typewriter
            {
                continue;
            }
            let title = Title::in_style(style);
            let moving = MovingTitle::prepare(&title, CANVAS, None);
            let settled = moving.frame(motion.stage(LENGTH / 2.0, LENGTH));
            let rest = draw_title(&title, CANVAS);
            let (got, want) = (solid_pixels(&settled), solid_pixels(&rest.picture));
            let apart = got.abs_diff(want) as f32 / want.max(1) as f32;
            // Outlines of neighbouring letters overlap differently when painted apart.
            assert!(
                apart < 0.08,
                "{style:?} covers {got} at rest against {want}"
            );
        }
    }

    #[test]
    fn parts_start_out_hidden_or_small() {
        for style in [
            TitleStyle::Rating,
            TitleStyle::OutlineShadow,
            TitleStyle::Gliding,
            TitleStyle::MakeItGlow,
            TitleStyle::Circular,
            TitleStyle::PushThrough,
        ] {
            let early = frame_at(style, 0.02);
            let late = frame_at(style, LENGTH / 2.0);
            assert!(
                solid_pixels(&early) * 3 < solid_pixels(&late),
                "{style:?}: {} then {}",
                solid_pixels(&early),
                solid_pixels(&late)
            );
        }
    }

    #[test]
    fn parts_that_came_in_go_back_out_by_the_end() {
        for style in [
            TitleStyle::Gliding,
            TitleStyle::LowerThird,
            TitleStyle::DoubleLines,
        ] {
            let end = frame_at(style, LENGTH - 0.01);
            let middle = frame_at(style, LENGTH / 2.0);
            assert!(solid_pixels(&end) * 3 < solid_pixels(&middle), "{style:?}");
        }
    }

    #[test]
    fn the_same_moment_always_draws_the_same_picture() {
        for style in [
            TitleStyle::Glitch,
            TitleStyle::Fireworks,
            TitleStyle::Smoke,
            TitleStyle::Groovy,
        ] {
            assert_eq!(
                frame_at(style, 0.3).rgba,
                frame_at(style, 0.3).rgba,
                "{style:?}"
            );
        }
    }

    #[test]
    fn a_frame_keeps_the_same_middle_and_pixel_size_as_the_resting_picture() {
        let title = Title::in_style(TitleStyle::Rating);
        let rest = draw_title(&title, CANVAS);
        let moving = MovingTitle::prepare(&title, CANVAS, None);
        let frame = moving.frame(title.style.motion().stage(0.2, LENGTH));
        let placed = |size: (u32, u32), scale: i32| {
            Placement {
                center_x: WHOLE / 3,
                center_y: WHOLE / 2,
                scale,
                ..Placement::default()
            }
            .rect(size, CANVAS)
        };
        let rest_size = (rest.picture.width, rest.picture.height);
        let frame_size = (frame.width, frame.height);
        let (a, b) = (
            placed(rest_size, rest.scale),
            placed(frame_size, moving.scale_for(frame_size)),
        );
        assert!((a.x + a.width / 2.0 - b.x - b.width / 2.0).abs() < 1.0);
        assert!((b.width - frame_size.0 as f32).abs() < 1.0);
    }

    #[test]
    fn typing_grows_from_the_middle_and_the_cursor_shows_while_it_types() {
        let typed_early = frame_at(TitleStyle::Typewriter, 0.06);
        let typed_late = frame_at(TitleStyle::Typewriter, 0.4);
        assert!(
            typed_early.width < typed_late.width,
            "the words are drawn again as they grow"
        );
        let cursor = |picture: &StillPicture| {
            let width = picture.width as usize;
            (0..picture.height as usize).any(|y| {
                let at = (y * width + width - 1) * 4;
                picture.rgba[at + 3] > 0
            }) || solid_pixels(picture) > 0
        };
        assert!(cursor(&typed_early));
        let resting = frame_at(TitleStyle::Typewriter, LENGTH / 2.0);
        let rest = draw_title(&Title::in_style(TitleStyle::Typewriter), CANVAS);
        assert_eq!(resting.width, rest.picture.width);
    }

    #[test]
    fn a_countdown_shows_the_time_left() {
        assert_ne!(
            frame_at(TitleStyle::Countdown, 0.5).rgba,
            frame_at(TitleStyle::Countdown, 0.6).rgba
        );
    }

    #[test]
    fn rapid_shows_one_word_at_a_time() {
        let first = frame_at(TitleStyle::Rapid, 0.1);
        let later = frame_at(TitleStyle::Rapid, 3.0);
        assert_ne!(first.rgba, later.rgba);
        let all = draw_title(&Title::in_style(TitleStyle::Rapid), CANVAS);
        let covered = |picture: &StillPicture| {
            picture
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|pixel| pixel[0] > 200 && pixel[1] > 200 && pixel[2] > 200)
                .count()
        };
        assert!(
            covered(&first) * 3 < covered(&all.picture),
            "only one word is written"
        );
    }

    #[test]
    fn karaoke_turns_more_of_the_words_the_accent_colour_as_it_goes() {
        let title = Title::in_style(TitleStyle::Karaoke);
        let accent_pixels = |picture: &StillPicture| {
            picture
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|pixel| {
                    pixel[3] > 240
                        && pixel[..3]
                            .iter()
                            .zip(title.accent)
                            .all(|(got, want)| got.abs_diff(want) < 16)
                })
                .count()
        };
        let early = accent_pixels(&frame_at(TitleStyle::Karaoke, 0.5));
        let late = accent_pixels(&frame_at(TitleStyle::Karaoke, 4.5));
        assert!(late > early * 2 + 10, "{early} then {late}");
    }

    #[test]
    fn a_rating_lights_the_number_of_stars_it_is_set_to() {
        let count = |stars: u8| {
            let title = Title {
                stars,
                ..Title::in_style(TitleStyle::Rating)
            };
            let picture = draw_title(&title, CANVAS).picture;
            picture
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|pixel| {
                    pixel[3] > 240
                        && pixel[..3]
                            .iter()
                            .zip(title.accent)
                            .all(|(got, want)| got.abs_diff(want) < 16)
                })
                .count()
        };
        assert!(count(5) > count(2));
        assert_eq!(count(0), 0);
    }

    #[test]
    fn a_logo_shows_on_a_title_with_a_place_for_one() {
        let logo = StillPicture::new(2, 2, [0, 255, 0, 255].repeat(4));
        let title = Title::in_style(TitleStyle::IntroMirror);
        let without = draw_title(&title, CANVAS);
        let with = crate::draw_title_with_logo(&title, CANVAS, Some(&logo));
        assert!(without.logo_spot.is_some());
        let green = |picture: &StillPicture| {
            picture
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|pixel| pixel[1] > 200 && pixel[0] < 50 && pixel[2] < 50)
                .count()
        };
        assert_eq!(green(&without.picture), 0);
        assert!(green(&with.picture) > 1000);
    }

    #[test]
    fn words_are_counted_across_both_lines() {
        let title = Title::in_style(TitleStyle::MakeItGlow);
        let moving = MovingTitle::prepare(&title, CANVAS, None);
        assert_eq!(moving.words, 3);
    }

    /// Saves each moving style's frames at a few moments, to compare with a sample recording. Set
    /// `BARECLIP_SCREENS` to a folder.
    #[test]
    #[ignore = "makes a picture to look at, not a check"]
    fn draw_every_motion() {
        let folder = std::env::var("BARECLIP_SCREENS").expect("BARECLIP_SCREENS is set");
        let moments = [0.05, 0.15, 0.3, 0.5, 0.8, 2.5, 4.6, 4.9];
        let styles = every_moving_style();
        let (cell_width, cell_height) = (320_usize, 180_usize);
        let (width, height) = (cell_width * moments.len(), cell_height * styles.len());
        let mut sheet = vec![30_u8; width * height * 3];
        for (row, style) in styles.iter().enumerate() {
            let title = Title::in_style(*style);
            let moving = MovingTitle::prepare(&title, (640, 360), None);
            for (column, seconds) in moments.iter().enumerate() {
                let picture = moving.frame(style.motion().stage(*seconds, LENGTH));
                let shrink = (picture.width as f32 / cell_width as f32)
                    .max(picture.height as f32 / cell_height as f32)
                    .max(1.0);
                for y in 0..cell_height {
                    for x in 0..cell_width {
                        let (from_x, from_y) =
                            ((x as f32 * shrink) as usize, (y as f32 * shrink) as usize);
                        if from_x >= picture.width as usize || from_y >= picture.height as usize {
                            continue;
                        }
                        let from = (from_y * picture.width as usize + from_x) * 4;
                        let to = ((row * cell_height + y) * width + column * cell_width + x) * 3;
                        let keep = 255 - u32::from(picture.rgba[from + 3]);
                        for part in 0..3 {
                            let under = u32::from(sheet[to + part]) * keep / 255;
                            sheet[to + part] =
                                (u32::from(picture.rgba[from + part]) + under).min(255) as u8;
                        }
                    }
                }
            }
        }
        let mut file = format!(
            "P6 {width} {height} 255
"
        )
        .into_bytes();
        file.extend(sheet);
        std::fs::write(format!("{folder}/title-motions.ppm"), file).expect("it can be written");
        let names: Vec<String> = styles.iter().map(|style| format!("{style:?}")).collect();
        std::fs::write(
            format!("{folder}/title-motions.txt"),
            names.join(
                "
",
            ),
        )
        .expect("it can be written");
    }

    #[test]
    #[ignore = "a measurement, not a check"]
    fn how_long_frames_take() {
        for style in every_moving_style() {
            let title = Title::in_style(style);
            let started = std::time::Instant::now();
            let moving = MovingTitle::prepare(&title, (1920, 1080), None);
            let prepared = started.elapsed();
            let started = std::time::Instant::now();
            let motion = style.motion();
            for frame in 0..30 {
                std::hint::black_box(moving.frame(motion.stage(frame as f32 / 60.0, LENGTH)));
            }
            println!(
                "{style:?}: prepared in {prepared:?}, {:?} a frame",
                started.elapsed() / 30
            );
        }
    }
}
