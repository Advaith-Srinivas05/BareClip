//! Lays out a title's words and shapes, and paints them into a see-through picture.

use bareclip_core::{
    canvas::Colour,
    picture::StillPicture,
    placement::WHOLE,
    title::{DEFAULT_FONT, Title, TitleLook, TitleShape, WordStack, scale_for_pixels},
};
use cosmic_text::{Attrs, Buffer, Color, Family, Metrics, Shaping, Style, Weight, Wrap};

use crate::fonts::{Engine, engine, has_font};

/// The largest picture a title draws, so that it always fits on the GPU.
const LARGEST_SIDE: u32 = 4096;
/// A picture can't show smaller than a twentieth of what fits the canvas, so a tiny title gets
/// empty room around it rather than growing.
const SMALLEST_SHARE_OF_CANVAS: u32 = 20;
const LINE_HEIGHT: f32 = 1.2;
/// Sizes of things around the words, as shares of how tall the letters are.
const PADDING: f32 = 0.28;
const SHADOW_OFFSET: f32 = 0.035;
const SHADOW_BLUR: f32 = 0.05;
const LINE_THICKNESS: f32 = 0.07;
const LINE_GAP: f32 = 0.22;
const SIDE_BAR: f32 = 0.11;
const OUTLINE_COLOUR: Colour = [0, 0, 0];
const SHADOW_OPACITY: f32 = 0.7;
/// How far each copy behind the words is stepped, as a share of how tall the letters are.
const STACK_STEP: f32 = 0.018;
const RAINBOW: [Colour; 6] = [
    [228, 3, 3],
    [255, 140, 0],
    [255, 237, 0],
    [0, 128, 38],
    [36, 64, 142],
    [115, 41, 130],
];
const BANDS: [Colour; 4] = [[255, 214, 0], [255, 120, 0], [230, 40, 40], [40, 80, 200]];
/// How thick each of the bands behind the words is, as a share of how tall the letters are.
const BAND_STEP: f32 = 0.05;
const GLOW_BLUR: f32 = 0.09;
/// The dark middle of letters that are drawn as outlines.
const HOLLOW_MIDDLE: Colour = [18, 16, 26];
const STARS: usize = 5;
const STAR_OFF: Colour = [96, 96, 96];
/// How big quote marks are, compared with the words.
const QUOTE_MARK_SIZE: f32 = 1.6;
const OPENING_MARK: &str = "\u{201C}";
const CLOSING_MARK: &str = "\u{201D}";
const QUOTE_MARK_FONT: &str = "Abril Fatface";
/// How much bigger than the canvas a pattern of words is, so it can turn and drift.
const PATTERN_REACH: f32 = 1.7;

/// A title ready to show.
#[derive(Clone, Debug, PartialEq)]
pub struct TitleDrawing {
    pub picture: StillPicture,
    /// The scale that shows it at the size it was asked for.
    pub scale: i32,
    /// The title's font isn't on this PC, so another stood in for it.
    pub font_missing: bool,
    /// Where a logo goes, as left, top, width and height in shares of the picture.
    pub logo_spot: Option<[f32; 4]>,
}

/// Words laid out, ready to paint.
pub(crate) struct Words {
    pub(crate) buffer: Buffer,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

/// How a run of words is set.
#[derive(Clone, Copy)]
struct Setting {
    bold: bool,
    italic: bool,
    spacing: i32,
}

impl Setting {
    const PLAIN: Self = Self {
        bold: false,
        italic: false,
        spacing: 0,
    };
}

fn lay_out(engine: &mut Engine, text: &str, font: &str, setting: Setting, pixels: f32) -> Words {
    let metrics = Metrics::new(pixels, pixels * LINE_HEIGHT);
    let mut buffer = Buffer::new(&mut engine.system, metrics);
    buffer.set_wrap(Wrap::None);
    buffer.set_size(None, None);
    let mut attrs = Attrs::new()
        .family(Family::Name(font))
        .weight(if setting.bold {
            Weight::BOLD
        } else {
            Weight::NORMAL
        })
        .style(if setting.italic {
            Style::Italic
        } else {
            Style::Normal
        });
    if setting.spacing != 0 {
        attrs = attrs.letter_spacing(setting.spacing as f32 / 1000.0);
    }
    buffer.set_text(text, &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(&mut engine.system, false);
    let mut width = 0.0_f32;
    let mut height = 0.0_f32;
    for run in buffer.layout_runs() {
        width = width.max(run.line_w);
        height = height.max(run.line_top + run.line_height);
    }
    // Empty words still take up a line, so a title with nothing typed yet can be seen and grabbed.
    Words {
        buffer,
        width: width.max(pixels * 0.5),
        height: height.max(pixels * LINE_HEIGHT),
    }
}

/// A picture being painted, as colours already scaled by how see-through they are.
pub(crate) struct Paint {
    width: usize,
    height: usize,
    rgba: Vec<f32>,
}

impl Paint {
    pub(crate) fn new(width: u32, height: u32) -> Self {
        let (width, height) = (width as usize, height as usize);
        Self {
            width,
            height,
            rgba: vec![0.0; width * height * 4],
        }
    }

    /// Fills a five-pointed star. Its edges are worked out by sampling each pixel several times,
    /// because a star has no simple distance to its edge.
    pub(crate) fn star(&mut self, (middle_x, middle_y): (f32, f32), reach: f32, colour: Colour) {
        const SAMPLES: usize = 4;
        let colour = float_colour(colour);
        let inner = reach * 0.42;
        let corners: Vec<(f32, f32)> = (0..10)
            .map(|index| {
                let angle = std::f32::consts::PI * (index as f32 / 5.0 - 0.5);
                let distance = if index % 2 == 0 { reach } else { inner };
                (
                    middle_x + distance * angle.cos(),
                    middle_y + distance * angle.sin(),
                )
            })
            .collect();
        let inside = |x: f32, y: f32| {
            let mut crossings = false;
            for (index, corner) in corners.iter().enumerate() {
                let next = corners[(index + 1) % corners.len()];
                if (corner.1 > y) != (next.1 > y)
                    && x < corner.0 + (y - corner.1) / (next.1 - corner.1) * (next.0 - corner.0)
                {
                    crossings = !crossings;
                }
            }
            crossings
        };
        let (left, right) = ((middle_x - reach) as i32 - 1, (middle_x + reach) as i32 + 2);
        let (top, bottom) = ((middle_y - reach) as i32 - 1, (middle_y + reach) as i32 + 2);
        for y in top..bottom {
            for x in left..right {
                let mut hits = 0;
                for down in 0..SAMPLES {
                    for across in 0..SAMPLES {
                        let at_x = x as f32 + (across as f32 + 0.5) / SAMPLES as f32;
                        let at_y = y as f32 + (down as f32 + 0.5) / SAMPLES as f32;
                        if inside(at_x, at_y) {
                            hits += 1;
                        }
                    }
                }
                if hits > 0 {
                    self.lay(x, y, colour, hits as f32 / (SAMPLES * SAMPLES) as f32);
                }
            }
        }
    }

    /// Fits a picture into a square, keeping its shape, and lays it over what is here. The picture
    /// is sampled once per pixel, so a large logo shrinks without shimmering much.
    pub(crate) fn picture_in(&mut self, picture: &StillPicture, area: Area) {
        let (from_width, from_height) = (picture.width as f32, picture.height as f32);
        let scale = (area.width / from_width).min(area.height / from_height);
        let (shown_width, shown_height) = (from_width * scale, from_height * scale);
        let left = area.x + (area.width - shown_width) / 2.0;
        let top = area.y + (area.height - shown_height) / 2.0;
        for y in top as i32..(top + shown_height).ceil() as i32 {
            for x in left as i32..(left + shown_width).ceil() as i32 {
                let from_x = ((x as f32 + 0.5 - left) / scale) as i32;
                let from_y = ((y as f32 + 0.5 - top) / scale) as i32;
                if from_x < 0
                    || from_y < 0
                    || from_x >= picture.width as i32
                    || from_y >= picture.height as i32
                {
                    continue;
                }
                let at = (from_y as usize * picture.width as usize + from_x as usize) * 4;
                let amount = f32::from(picture.rgba[at + 3]) / 255.0;
                if amount <= 0.0 {
                    continue;
                }
                // Stored already scaled by how see-through it is, and `lay` wants it unscaled.
                let colour = [
                    f32::from(picture.rgba[at]) / 255.0 / amount,
                    f32::from(picture.rgba[at + 1]) / 255.0 / amount,
                    f32::from(picture.rgba[at + 2]) / 255.0 / amount,
                ];
                self.lay(x, y, colour, amount);
            }
        }
    }

    /// Lays a colour over one pixel, with this much of it showing.
    fn lay(&mut self, x: i32, y: i32, colour: [f32; 3], amount: f32) {
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            return;
        }
        let at = (y as usize * self.width + x as usize) * 4;
        let pixel = &mut self.rgba[at..at + 4];
        for (value, part) in pixel.iter_mut().zip(colour) {
            *value = part * amount + *value * (1.0 - amount);
        }
        pixel[3] = amount + pixel[3] * (1.0 - amount);
    }

    /// Fills a box, with its corners rounded by `radius` and its edges softened.
    pub(crate) fn rectangle(&mut self, area: Area, radius: f32, colour: Colour) {
        let colour = float_colour(colour);
        let radius = radius.min(area.width / 2.0).min(area.height / 2.0);
        let (middle_x, middle_y) = (area.x + area.width / 2.0, area.y + area.height / 2.0);
        let (half_x, half_y) = (area.width / 2.0 - radius, area.height / 2.0 - radius);
        let (left, top) = ((area.x - 1.0).floor() as i32, (area.y - 1.0).floor() as i32);
        let right = (area.x + area.width + 1.0).ceil() as i32;
        let bottom = (area.y + area.height + 1.0).ceil() as i32;
        for y in top..bottom {
            for x in left..right {
                let across = (x as f32 + 0.5 - middle_x).abs() - half_x;
                let down = (y as f32 + 0.5 - middle_y).abs() - half_y;
                let outside = across.max(0.0).hypot(down.max(0.0));
                let inside = across.max(down).min(0.0);
                let amount = (0.5 - (outside + inside - radius)).clamp(0.0, 1.0);
                if amount > 0.0 {
                    self.lay(x, y, colour, amount);
                }
            }
        }
    }

    pub(crate) fn into_picture(self) -> StillPicture {
        let rgba = self
            .rgba
            .iter()
            .map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8)
            .collect();
        StillPicture::new(self.width as u32, self.height as u32, rgba)
    }
}

fn float_colour(colour: Colour) -> [f32; 3] {
    colour.map(|part| f32::from(part) / 255.0)
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Area {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

/// How much of each pixel the words cover, and their colour there. A colour font such as an emoji
/// font brings colours of its own.
pub(crate) struct Ink {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) covered: Vec<f32>,
    pub(crate) colours: Vec<[f32; 3]>,
}

impl Ink {
    pub(crate) fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            covered: vec![0.0; width * height],
            colours: vec![[0.0; 3]; width * height],
        }
    }

    pub(crate) fn words(
        &mut self,
        engine: &mut Engine,
        words: &Words,
        (x, y): (f32, f32),
        colour: Colour,
        left_aligned: bool,
    ) {
        let base = Color::rgb(colour[0], colour[1], colour[2]);
        let Engine { system, glyphs, .. } = engine;
        for run in words.buffer.layout_runs() {
            let across = if left_aligned {
                0.0
            } else {
                (words.width - run.line_w) / 2.0
            };
            for glyph in run.glyphs {
                let physical = glyph.physical((x + across, y + run.line_y), 1.0);
                glyphs.with_pixels(system, physical.cache_key, base, |gx, gy, pixel| {
                    let (px, py) = (physical.x + gx, physical.y + gy);
                    if px < 0 || py < 0 || px as usize >= self.width || py as usize >= self.height {
                        return;
                    }
                    let at = py as usize * self.width + px as usize;
                    let amount = f32::from(pixel.a()) / 255.0;
                    if amount > self.covered[at] {
                        self.covered[at] = amount;
                        self.colours[at] = float_colour([pixel.r(), pixel.g(), pixel.b()]);
                    }
                });
            }
        }
    }

    /// How far each pixel is from the nearest one with words on it, squared. This is the usual
    /// way of finding exact distances, one pass down every column and then one along every row.
    fn squared_distances(&self) -> Vec<f32> {
        let far = (self.width * self.width + self.height * self.height) as f32;
        let mut distances: Vec<f32> = self
            .covered
            .iter()
            .map(|amount| if *amount >= 0.5 { 0.0 } else { far })
            .collect();
        let longest = self.width.max(self.height);
        let mut line = vec![0.0_f32; longest];
        let mut sites = vec![0_usize; longest];
        let mut bounds = vec![0.0_f32; longest + 1];
        for x in 0..self.width {
            for y in 0..self.height {
                line[y] = distances[y * self.width + x];
            }
            let found = lowest_curves(&line[..self.height], &mut sites, &mut bounds);
            let mut at = 0;
            for y in 0..self.height {
                while at < found && bounds[at + 1] < y as f32 {
                    at += 1;
                }
                let apart = y as f32 - sites[at] as f32;
                distances[y * self.width + x] = apart * apart + line[sites[at]];
            }
        }
        for y in 0..self.height {
            let row = y * self.width;
            line[..self.width].copy_from_slice(&distances[row..row + self.width]);
            let found = lowest_curves(&line[..self.width], &mut sites, &mut bounds);
            let mut at = 0;
            for x in 0..self.width {
                while at < found && bounds[at + 1] < x as f32 {
                    at += 1;
                }
                let apart = x as f32 - sites[at] as f32;
                distances[row + x] = apart * apart + line[sites[at]];
            }
        }
        distances
    }

    /// Paints a soft line around the words, this many pixels wide.
    fn paint_outline(&self, paint: &mut Paint, width: f32, colour: Colour) {
        let colour = float_colour(colour);
        let distances = self.squared_distances();
        for y in 0..self.height {
            for x in 0..self.width {
                let distance = distances[y * self.width + x].sqrt();
                let amount = (width + 0.5 - distance).clamp(0.0, 1.0);
                if amount > 0.0 {
                    paint.lay(x as i32, y as i32, colour, amount);
                }
            }
        }
    }

    /// Paints a soft shadow of the words, moved down and to the right.
    fn paint_shadow(&self, paint: &mut Paint, offset: f32, blur: f32) {
        let mut soft = self.covered.clone();
        let radius = blur.round().max(1.0) as usize;
        // Three box blurs in a row look much like a true blur, and take no longer for a wide one.
        for _ in 0..3 {
            soft = blur_across(&soft, self.width, self.height, radius);
            soft = blur_down(&soft, self.width, self.height, radius);
        }
        let step = offset.round() as i32;
        for y in 0..self.height {
            for x in 0..self.width {
                let amount = soft[y * self.width + x] * SHADOW_OPACITY;
                if amount > 0.003 {
                    paint.lay(x as i32 + step, y as i32 + step, [0.0; 3], amount);
                }
            }
        }
    }

    /// Paints a hard copy of the words, moved by this much.
    fn paint_copy(&self, paint: &mut Paint, (across, down): (i32, i32), colour: Colour) {
        let colour = float_colour(colour);
        for y in 0..self.height {
            for x in 0..self.width {
                let amount = self.covered[y * self.width + x];
                if amount > 0.0 {
                    paint.lay(x as i32 + across, y as i32 + down, colour, amount);
                }
            }
        }
    }

    /// Paints a soft light around the words.
    fn paint_glow(&self, paint: &mut Paint, blur: f32, colour: Colour) {
        let mut soft = self.covered.clone();
        let radius = blur.round().max(1.0) as usize;
        for _ in 0..3 {
            soft = blur_across(&soft, self.width, self.height, radius);
            soft = blur_down(&soft, self.width, self.height, radius);
        }
        let colour = float_colour(colour);
        for y in 0..self.height {
            for x in 0..self.width {
                let amount = (soft[y * self.width + x] * 1.4).min(0.9);
                if amount > 0.003 {
                    paint.lay(x as i32, y as i32, colour, amount);
                }
            }
        }
    }

    /// Paints everything a style does with these words: anything behind them, then them.
    pub(crate) fn paint_styled(&self, paint: &mut Paint, title: &Title, pixels: f32) {
        let look = title.style.look();
        let step = (pixels * STACK_STEP).max(1.0).round() as i32;
        match look.stack {
            WordStack::None => {}
            WordStack::Black(steps) => {
                for index in (1..=steps as i32).rev() {
                    self.paint_copy(paint, (index * step, index * step), [0, 0, 0]);
                }
            }
            WordStack::Accent(steps) => {
                for index in (1..=steps as i32).rev() {
                    self.paint_copy(paint, (index * step, index * step), title.accent);
                }
            }
            WordStack::Rainbow => {
                for (index, colour) in RAINBOW.iter().enumerate().rev() {
                    let reach = (index as i32 + 1) * step;
                    self.paint_copy(paint, (reach, reach), *colour);
                }
            }
            WordStack::Bands => {
                let band = (pixels * BAND_STEP).max(1.0).round() as i32;
                for (index, colour) in BANDS.iter().enumerate().rev() {
                    self.paint_copy(paint, (0, (index as i32 + 1) * band), *colour);
                }
            }
        }
        if look.glow {
            let colour = if look.glow_in_accent {
                title.accent
            } else {
                title.colour
            };
            self.paint_glow(paint, pixels * GLOW_BLUR, colour);
        }
        if look.shadow {
            self.paint_shadow(paint, pixels * SHADOW_OFFSET, pixels * SHADOW_BLUR);
        }
        if look.outline {
            let colour = if look.hollow {
                title.colour
            } else if look.outline_in_accent {
                title.accent
            } else {
                OUTLINE_COLOUR
            };
            let width = (pixels * look.outline_width as f32 / 100.0).max(1.0);
            self.paint_outline(paint, width, colour);
        }
        if look.hollow {
            self.paint_copy(paint, (0, 0), HOLLOW_MIDDLE);
        } else {
            self.paint_words(paint);
        }
    }

    /// Paints only the words, with nothing a style adds around them.
    pub(crate) fn paint_words_on(&self, paint: &mut Paint) {
        self.paint_words(paint);
    }

    fn paint_words(&self, paint: &mut Paint) {
        for y in 0..self.height {
            for x in 0..self.width {
                let at = y * self.width + x;
                if self.covered[at] > 0.0 {
                    paint.lay(x as i32, y as i32, self.colours[at], self.covered[at]);
                }
            }
        }
    }
}

/// Each value along a line is the bottom of a curve rising either side of it. This finds which
/// curves are the lowest anywhere, and where each takes over from the last, and returns the index
/// of the last one.
fn lowest_curves(line: &[f32], sites: &mut [usize], bounds: &mut [f32]) -> usize {
    let mut last = 0_usize;
    sites[0] = 0;
    bounds[0] = f32::NEG_INFINITY;
    bounds[1] = f32::INFINITY;
    for place in 1..line.len() {
        let mut crossing;
        loop {
            let site = sites[last];
            crossing = (line[place] + (place * place) as f32 - line[site] - (site * site) as f32)
                / (2.0 * (place as f32 - site as f32));
            if crossing <= bounds[last] && last > 0 {
                last -= 1;
            } else {
                break;
            }
        }
        last += 1;
        sites[last] = place;
        bounds[last] = crossing;
        bounds[last + 1] = f32::INFINITY;
    }
    last
}

fn blur_across(values: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
    let mut blurred = vec![0.0; values.len()];
    let span = (radius * 2 + 1) as f32;
    for y in 0..height {
        let row = &values[y * width..(y + 1) * width];
        let mut total: f32 = row.iter().take(radius + 1).sum();
        for x in 0..width {
            blurred[y * width + x] = total / span;
            if x + radius + 1 < width {
                total += row[x + radius + 1];
            }
            if x >= radius {
                total -= row[x - radius];
            }
        }
    }
    blurred
}

fn blur_down(values: &[f32], width: usize, height: usize, radius: usize) -> Vec<f32> {
    let mut blurred = vec![0.0; values.len()];
    let span = (radius * 2 + 1) as f32;
    for x in 0..width {
        let mut total = 0.0;
        for y in 0..height.min(radius + 1) {
            total += values[y * width + x];
        }
        for y in 0..height {
            blurred[y * width + x] = total / span;
            if y + radius + 1 < height {
                total += values[(y + radius + 1) * width + x];
            }
            if y >= radius {
                total -= values[(y - radius) * width + x];
            }
        }
    }
    blurred
}

/// A row of stars, with where the first sits, how big each is and how many are lit.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StarRow {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) size: f32,
    pub(crate) lit: usize,
}

/// Where everything goes, working from the words outwards.
pub(crate) struct Layout {
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) boxes: Vec<(Area, f32, Colour)>,
    /// How many of the boxes, from the first, stay where they are while the rest move.
    pub(crate) fixed_boxes: usize,
    pub(crate) first: (f32, f32),
    pub(crate) second: Option<(f32, f32)>,
    pub(crate) stars: Option<StarRow>,
    pub(crate) logo: Option<Area>,
    pub(crate) marks: Option<[(f32, f32); 2]>,
    /// It is exactly as big as it says, with nothing to allow for past its edges.
    pub(crate) exact: bool,
}

/// Where each line of words sits, as left, top, width and height from where the words start.
fn line_boxes(words: &Words) -> Vec<Area> {
    words
        .buffer
        .layout_runs()
        .map(|run| Area {
            x: (words.width - run.line_w) / 2.0,
            y: run.line_top,
            width: run.line_w,
            height: run.line_height,
        })
        .collect()
}

/// Lays out the styles that cover the whole canvas, or a panel of it, whatever size the words are.
fn arrange_on_canvas(
    title: &Title,
    first: &Words,
    second: Option<&Words>,
    pixels: f32,
    canvas: (u32, u32),
) -> Layout {
    let look = title.style.look();
    let (width, height) = (canvas.0 as f32, canvas.1 as f32);
    let shorter = width.min(height);
    let background = title.background.unwrap_or([0, 0, 0]);
    let accent = title.accent;
    let gap = pixels * LINE_GAP;
    let line = (pixels * LINE_THICKNESS).max(1.0);
    let (second_width, second_height) =
        second.map_or((0.0, 0.0), |words| (words.width, words.height));
    let logo_side = title.logo_size as f32 / WHOLE as f32 * shorter;
    let whole_canvas = Area {
        x: 0.0,
        y: 0.0,
        width,
        height,
    };
    let across = |x: f32, y: f32, wide: f32, tall: f32| Area {
        x,
        y,
        width: wide,
        height: tall,
    };
    let middle = |words_width: f32| (width - words_width) / 2.0;
    let logo_above = |x: f32, top_of_words: f32| Area {
        x,
        y: top_of_words - logo_side - gap,
        width: logo_side,
        height: logo_side,
    };
    let mut layout = Layout {
        width,
        height,
        boxes: Vec::new(),
        fixed_boxes: 0,
        first: (0.0, 0.0),
        second: second.map(|_| (0.0, 0.0)),
        stars: None,
        logo: None,
        marks: None,
        exact: true,
    };
    let set_second = |layout: &mut Layout, place: (f32, f32)| {
        if second.is_some() {
            layout.second = Some(place);
        }
    };
    match look.shape {
        TitleShape::MemeBands => {
            let band = first.height + pixels * PADDING * 2.0;
            let lower_band = second_height + pixels * PADDING * 2.0;
            layout
                .boxes
                .push((across(0.0, 0.0, width, band), 0.0, background));
            layout.boxes.push((
                across(0.0, height - lower_band, width, lower_band),
                0.0,
                background,
            ));
            layout.fixed_boxes = 2;
            layout.first = (middle(first.width), pixels * PADDING);
            let place = (middle(second_width), height - lower_band + pixels * PADDING);
            set_second(&mut layout, place);
        }
        TitleShape::IntroLine => {
            layout.boxes.push((whole_canvas, 0.0, background));
            layout.fixed_boxes = 1;
            let line_y = height * 0.65;
            layout
                .boxes
                .push((across(0.0, line_y, width, line * 2.0), 0.0, accent));
            layout.first = (middle(first.width), line_y - first.height - gap);
            let place = (middle(second_width), line_y + line * 2.0 + gap);
            set_second(&mut layout, place);
            layout.logo = Some(logo_above(middle(logo_side), layout.first.1));
        }
        TitleShape::IntroTwoColours => {
            let split = height * 0.7;
            layout
                .boxes
                .push((across(0.0, 0.0, width, split), 0.0, accent));
            layout
                .boxes
                .push((across(0.0, split, width, height - split), 0.0, background));
            layout.fixed_boxes = 2;
            layout.first = (middle(first.width), split * 0.72 - first.height / 2.0);
            let place = (
                middle(second_width),
                split + (height - split - second_height) / 2.0,
            );
            set_second(&mut layout, place);
            layout.logo = Some(logo_above(middle(logo_side), layout.first.1));
        }
        TitleShape::IntroFrame => {
            layout.boxes.push((whole_canvas, 0.0, background));
            layout.fixed_boxes = 1;
            let thick = (shorter * 0.035).max(2.0);
            layout
                .boxes
                .push((across(0.0, 0.0, width, thick), 0.0, accent));
            layout
                .boxes
                .push((across(0.0, height - thick, width, thick), 0.0, accent));
            layout
                .boxes
                .push((across(0.0, 0.0, thick, height), 0.0, accent));
            layout
                .boxes
                .push((across(width - thick, 0.0, thick, height), 0.0, accent));
            let total = first.height + second_height + gap + logo_side + gap;
            let top = (height - total) / 2.0;
            layout.first = (middle(first.width), top);
            let second_y = top + first.height + gap;
            set_second(&mut layout, (middle(second_width), second_y));
            layout.logo = Some(Area {
                x: middle(logo_side),
                y: second_y + second_height + gap,
                width: logo_side,
                height: logo_side,
            });
        }
        TitleShape::FullFill => {
            layout.boxes.push((whole_canvas, 0.0, background));
            layout.fixed_boxes = 1;
            let total = first.height + second_height;
            let top = (height - total) / 2.0;
            layout.first = (middle(first.width), top);
            set_second(&mut layout, (middle(second_width), top + first.height));
        }
        TitleShape::PanelLeft | TitleShape::PanelRight => {
            let panel = width * 0.72;
            let bar = (shorter * 0.008).max(1.0);
            let on_left = look.shape == TitleShape::PanelLeft;
            let left = if on_left { 0.0 } else { width - panel };
            layout
                .boxes
                .push((across(left, 0.0, panel, height), 0.0, background));
            layout.fixed_boxes = 1;
            let bar_x = if on_left { panel - bar } else { left };
            layout
                .boxes
                .push((across(bar_x, 0.0, bar, height), 0.0, accent));
            let words_x = left + width * 0.07;
            let total = first.height + second_height;
            let top = (height - total) / 2.0 + logo_side / 2.0;
            layout.first = (words_x, top);
            set_second(&mut layout, (words_x, top + first.height));
            layout.logo = Some(logo_above(words_x, top));
        }
        _ => {
            // A pattern of words: the words themselves are worked out when it is painted.
            let (wide, tall) = (width * PATTERN_REACH, height * PATTERN_REACH);
            layout.width = wide;
            layout.height = tall;
            layout.boxes.push((
                Area {
                    x: 0.0,
                    y: 0.0,
                    width: wide,
                    height: tall,
                },
                0.0,
                background,
            ));
            layout.fixed_boxes = 1;
        }
    }
    layout
}

fn arrange(
    title: &Title,
    first: &Words,
    second: Option<&Words>,
    pixels: f32,
    canvas: (u32, u32),
) -> Layout {
    let look: TitleLook = title.style.look();
    if look.shape.covers_the_canvas() {
        return arrange_on_canvas(title, first, second, pixels, canvas);
    }
    let padding = pixels * PADDING;
    let line = (pixels * LINE_THICKNESS).max(1.0);
    let gap = pixels * LINE_GAP;
    let (second_width, second_height) =
        second.map_or((0.0, 0.0), |words| (words.width, words.height));
    let accent = title.accent;
    let band_height = pixels * 0.42;
    let mark_room = pixels * QUOTE_MARK_SIZE * 0.55;
    let star_size = pixels * 0.72;
    let stars_width = star_size * STARS as f32 * 1.12;

    let widest = first.width.max(second_width);
    let bar_room = pixels * SIDE_BAR + gap;
    // How wide and tall everything inside the padding is, how far in that starts, where the two
    // lots of words start going down, and how far in each starts when they are left-aligned.
    let (inner_width, inner_height, left, first_y, second_y, first_inset, second_inset) =
        match look.shape {
            TitleShape::Plain | TitleShape::Pill | TitleShape::EachLineInBox => (
                widest,
                first.height + second_height,
                padding,
                padding,
                padding + first.height,
                0.0,
                0.0,
            ),
            TitleShape::Frame => (
                widest,
                first.height + second_height + padding * 2.0,
                padding * 2.0,
                padding * 2.0,
                padding * 2.0 + first.height,
                0.0,
                0.0,
            ),
            TitleShape::TagBand => (
                widest + padding,
                first.height + band_height + padding,
                padding * 1.5,
                padding + band_height + padding * 0.5,
                padding + band_height + padding * 0.5 + first.height,
                0.0,
                0.0,
            ),
            TitleShape::ShortLineUnder => (
                widest,
                first.height + line + gap * 2.0,
                padding,
                padding,
                padding + first.height,
                0.0,
                0.0,
            ),
            TitleShape::LinesAround => (
                widest,
                first.height + second_height + (line + gap) * 2.0,
                padding,
                padding + line + gap,
                padding + line + gap + first.height,
                0.0,
                0.0,
            ),
            TitleShape::LineBetween => (
                widest,
                first.height + second_height + line + gap * 2.0,
                padding,
                padding,
                padding + first.height + line + gap * 2.0,
                0.0,
                0.0,
            ),
            TitleShape::SecondInBox => (
                first.width.max(second_width + padding * 2.0),
                first.height + second_height + padding * 1.5,
                padding,
                padding,
                padding + first.height + padding * 0.75,
                0.0,
                0.0,
            ),
            TitleShape::BothInBoxes => (
                (first.width + padding * 2.0).max(second_width + padding * 2.0),
                first.height + second_height + padding * 1.5,
                padding,
                padding * 1.25,
                padding * 1.25 + first.height + padding * 0.75,
                0.0,
                0.0,
            ),
            TitleShape::FirstInBox => (
                (first.width + padding * 2.0).max(second_width),
                first.height + second_height + padding * 2.0,
                padding,
                padding * 1.5,
                padding * 2.5 + first.height,
                padding,
                0.0,
            ),
            TitleShape::SideBar => (
                widest + bar_room,
                first.height + second_height,
                padding,
                padding,
                padding + first.height,
                bar_room,
                bar_room,
            ),
            TitleShape::QuoteMarks => (
                widest + mark_room * 2.0,
                first.height + second_height + mark_room,
                padding,
                padding + mark_room / 2.0,
                padding + mark_room / 2.0 + first.height,
                0.0,
                0.0,
            ),
            TitleShape::Stars => (
                widest.max(stars_width),
                first.height + star_size * 1.2 + second_height,
                padding,
                padding,
                padding + first.height + star_size * 1.2,
                0.0,
                0.0,
            ),
            TitleShape::Disc => {
                let across = first.width.max(first.height) + padding * 4.0;
                let room = across * 0.12;
                (
                    across + room * 2.0,
                    across + room * 2.0,
                    0.0,
                    (across + room * 2.0 - first.height) / 2.0 - padding,
                    0.0,
                    0.0,
                    0.0,
                )
            }
            TitleShape::Band => (
                widest.max(canvas.0 as f32 - padding * 2.0),
                first.height,
                padding,
                padding,
                0.0,
                0.0,
                0.0,
            ),
            // These go through `arrange_on_canvas`.
            _ => (widest, first.height, padding, padding, 0.0, 0.0, 0.0),
        };
    let mut boxes = Vec::new();
    let width = inner_width + left * 2.0;
    let height = inner_height + padding * 2.0;

    let centred = |words_width: f32| left + (inner_width - words_width) / 2.0;
    let first_x = if look.left_aligned {
        left + first_inset
    } else {
        centred(first.width)
    };
    let second_x = if look.left_aligned {
        left + second_inset
    } else {
        centred(second_width)
    };

    if let Some(background) = title.background {
        let radius = if look.shape == TitleShape::Pill {
            height / 2.0
        } else {
            pixels * look.round as f32 / 100.0
        };
        let whole = Area {
            x: 0.0,
            y: 0.0,
            width,
            height,
        };
        if look.box_shadow {
            let reach = pixels * 0.07;
            boxes.push((
                Area {
                    x: reach,
                    y: reach,
                    ..whole
                },
                radius,
                [0, 0, 0],
            ));
        }
        // In these, the background is part of the shape.
        if !matches!(look.shape, TitleShape::BothInBoxes) {
            boxes.push((whole, radius, background));
        }
    }
    let across = |x: f32, y: f32, wide: f32, tall: f32| Area {
        x,
        y,
        width: wide,
        height: tall,
    };
    match look.shape {
        TitleShape::Frame => {
            let (right, bottom) = (width - padding * 0.5, height - padding * 0.5);
            let near = padding * 0.5;
            boxes.push((across(near, near, right - near, line), 0.0, accent));
            boxes.push((across(near, bottom - line, right - near, line), 0.0, accent));
            boxes.push((across(near, near, line, bottom - near), 0.0, accent));
            boxes.push((across(right - line, near, line, bottom - near), 0.0, accent));
        }
        TitleShape::TagBand => {
            boxes.push((
                across(0.0, 0.0, width, band_height + padding * 0.5),
                0.0,
                accent,
            ));
            boxes.push((across(0.0, height - line, width, line), 0.0, accent));
            boxes.push((across(0.0, 0.0, line, height), 0.0, accent));
            boxes.push((across(width - line, 0.0, line, height), 0.0, accent));
        }
        TitleShape::ShortLineUnder => {
            let wide = (first.width * 0.3).max(pixels);
            boxes.push((
                across(
                    (width - wide) / 2.0,
                    first_y + first.height + gap,
                    wide,
                    line,
                ),
                0.0,
                accent,
            ));
        }
        TitleShape::EachLineInBox => {
            for row in line_boxes(first) {
                let reach = padding * 0.45;
                boxes.push((
                    across(
                        first_x + row.x - reach,
                        first_y + row.y,
                        row.width + reach * 2.0,
                        row.height,
                    ),
                    pixels * look.round as f32 / 100.0,
                    accent,
                ));
            }
        }
        TitleShape::LinesAround => {
            boxes.push((across(left, padding, inner_width, line), 0.0, accent));
            boxes.push((
                across(left, height - padding - line, inner_width, line),
                0.0,
                accent,
            ));
        }
        TitleShape::LineBetween => {
            boxes.push((
                across(left, first_y + first.height + gap, inner_width, line),
                0.0,
                accent,
            ));
        }
        TitleShape::SecondInBox => {
            boxes.push((
                across(
                    second_x - padding,
                    second_y - padding * 0.25,
                    second_width + padding * 2.0,
                    second_height + padding * 0.5,
                ),
                pixels * 0.04,
                accent,
            ));
        }
        TitleShape::BothInBoxes => {
            let background = title.background.unwrap_or(accent);
            boxes.push((
                across(
                    first_x - padding,
                    first_y - padding * 0.25,
                    first.width + padding * 2.0,
                    first.height + padding * 0.5,
                ),
                0.0,
                background,
            ));
            boxes.push((
                across(
                    second_x - padding,
                    second_y - padding * 0.25,
                    second_width + padding * 2.0,
                    second_height + padding * 0.5,
                ),
                0.0,
                accent,
            ));
        }
        TitleShape::FirstInBox => {
            boxes.push((
                across(
                    left,
                    first_y - padding * 0.5,
                    first.width + padding * 2.0,
                    first.height + padding,
                ),
                pixels * 0.04,
                accent,
            ));
        }
        TitleShape::SideBar => {
            boxes.push((
                across(left, padding, pixels * SIDE_BAR, inner_height),
                pixels * 0.03,
                accent,
            ));
        }
        TitleShape::Disc => {
            let side = width.min(height);
            let ring = [accent[0] / 3 * 2, accent[1] / 3 * 2, accent[2] / 3 * 2];
            boxes.push((across(0.0, 0.0, side, side), side / 2.0, ring));
            let inner = side * 0.84;
            let inset = (side - inner) / 2.0;
            boxes.push((across(inset, inset, inner, inner), inner / 2.0, accent));
        }
        TitleShape::Band => {
            boxes.push((across(0.0, 0.0, width, height), 0.0, accent));
        }
        _ => {}
    }

    let stars = (look.shape == TitleShape::Stars).then(|| StarRow {
        x: (width - stars_width) / 2.0,
        y: first_y + first.height + star_size * 0.05,
        size: star_size,
        lit: usize::from(title.stars).min(STARS),
    });
    let marks = (look.shape == TitleShape::QuoteMarks).then_some([
        (padding * 0.3, padding * 0.1),
        (
            width - padding * 0.3 - mark_room,
            height - padding * 0.5 - mark_room * 1.3,
        ),
    ]);

    Layout {
        width,
        height,
        boxes,
        fixed_boxes: 0,
        first: (first_x, first_y),
        second: second.map(|_| (second_x, second_y)),
        stars,
        logo: None,
        marks,
        exact: look.shape == TitleShape::Band,
    }
}

/// The picture's size, with room for anything that spills past the layout, and at least enough
/// that it can show at its own pixels on this canvas.
fn picture_size(layout: &Layout, canvas: (u32, u32), spill: f32) -> ((u32, u32), (f32, f32)) {
    if layout.exact {
        let size = (layout.width.ceil() as u32, layout.height.ceil() as u32);
        return (size, (0.0, 0.0));
    }
    let wanted_width = (layout.width + spill * 2.0).ceil() as u32;
    let wanted_height = (layout.height + spill * 2.0).ceil() as u32;
    let least_height = canvas.1 / SMALLEST_SHARE_OF_CANVAS + 2;
    let least_width = canvas.0 / SMALLEST_SHARE_OF_CANVAS + 2;
    // Either way being big enough is enough, because the picture fits by whichever is tighter.
    let (width, height) = if wanted_width >= least_width || wanted_height >= least_height {
        (wanted_width, wanted_height)
    } else {
        (wanted_width, least_height)
    };
    let inset = (
        (width as f32 - layout.width) / 2.0,
        (height as f32 - layout.height) / 2.0,
    );
    ((width.max(1), height.max(1)), inset)
}

/// How far anything a style draws around its words can reach past them.
pub(crate) fn reach_past_words(look: &TitleLook, pixels: f32) -> f32 {
    let outline = (pixels * look.outline_width as f32 / 100.0).max(1.0);
    let step = (pixels * STACK_STEP).max(1.0);
    let stack = match look.stack {
        WordStack::None => 0.0,
        WordStack::Black(steps) | WordStack::Accent(steps) => steps as f32 * step,
        WordStack::Rainbow => RAINBOW.len() as f32 * step,
        WordStack::Bands => BANDS.len() as f32 * (pixels * BAND_STEP).max(1.0),
    };
    let glow = if look.glow {
        pixels * GLOW_BLUR * 3.0
    } else {
        0.0
    };
    let box_shadow = if look.box_shadow { pixels * 0.07 } else { 0.0 };
    pixels * (SHADOW_OFFSET + SHADOW_BLUR * 3.0) + outline + stack + glow + box_shadow + 2.0
}

/// Everything worked out about a title before anything is painted.
pub(crate) struct Plan {
    pub(crate) first: Words,
    pub(crate) second: Option<Words>,
    pub(crate) marks: Option<[Words; 2]>,
    pub(crate) layout: Layout,
    pub(crate) size: (u32, u32),
    pub(crate) inset: (f32, f32),
}

/// The words a pattern of words repeats, row after row, each row a little further along.
fn pattern_text(
    engine: &mut Engine,
    words: &str,
    font: &str,
    setting: Setting,
    pixels: f32,
    canvas: (u32, u32),
) -> String {
    let piece = format!("{words} \u{2022} ");
    let one = lay_out(engine, &piece, font, setting, pixels);
    let width = canvas.0 as f32 * PATTERN_REACH;
    let height = canvas.1 as f32 * PATTERN_REACH;
    let count = (width / one.width.max(1.0)).ceil() as usize + 2;
    let rows = (height / (pixels * LINE_HEIGHT)).ceil() as usize + 1;
    (0..rows)
        .map(|row| {
            let start = row % piece.chars().count();
            let text: String = piece
                .chars()
                .cycle()
                .skip(start)
                .take(piece.chars().count() * count)
                .collect();
            text
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn plan_title(
    engine: &mut Engine,
    title: &Title,
    font: &str,
    canvas: (u32, u32),
    pixels: f32,
) -> Plan {
    let look = title.style.look();
    let setting = Setting {
        bold: look.bold,
        italic: look.italic,
        spacing: look.spacing,
    };
    let in_capitals = |words: &str| {
        if look.capitals {
            words.to_uppercase()
        } else {
            words.to_owned()
        }
    };
    let text = if look.shape == TitleShape::Pattern {
        let words = in_capitals(&title.text);
        pattern_text(engine, &words, font, setting, pixels, canvas)
    } else {
        in_capitals(&title.text)
    };
    let first = lay_out(engine, &text, font, setting, pixels);
    let second_pixels = look
        .second_line
        .map(|percent| pixels * percent as f32 / 100.0);
    let second = second_pixels.map(|size| {
        let second_setting = Setting {
            bold: false,
            ..setting
        };
        lay_out(
            engine,
            &in_capitals(&title.second_text),
            font,
            second_setting,
            size,
        )
    });
    let layout = arrange(title, &first, second.as_ref(), pixels, canvas);
    let marks = (look.shape == TitleShape::QuoteMarks).then(|| {
        let size = pixels * QUOTE_MARK_SIZE;
        [
            lay_out(engine, OPENING_MARK, QUOTE_MARK_FONT, Setting::PLAIN, size),
            lay_out(engine, CLOSING_MARK, QUOTE_MARK_FONT, Setting::PLAIN, size),
        ]
    });
    let spill = reach_past_words(&look, pixels);
    let (size, inset) = picture_size(&layout, canvas, spill);
    Plan {
        first,
        second,
        marks,
        layout,
        size,
        inset,
    }
}

/// What colour each lot of words is.
pub(crate) fn word_colours(title: &Title) -> (Colour, Colour) {
    let look = title.style.look();
    (
        if look.first_in_accent {
            title.accent
        } else {
            title.colour
        },
        if look.second_in_accent {
            title.accent
        } else {
            title.colour
        },
    )
}

fn paint_title(
    engine: &mut Engine,
    title: &Title,
    font: &str,
    canvas: (u32, u32),
    pixels: f32,
    logo: Option<&StillPicture>,
) -> (StillPicture, (u32, u32), Option<[f32; 4]>) {
    let look = title.style.look();
    let plan = plan_title(engine, title, font, canvas, pixels);
    let (width, height) = plan.size;
    let (inset_x, inset_y) = plan.inset;
    if width > LARGEST_SIDE || height > LARGEST_SIDE {
        return (StillPicture::new(1, 1, vec![0; 4]), (width, height), None);
    }
    let layout = &plan.layout;

    let mut paint = Paint::new(width, height);
    let moved = |area: &Area| Area {
        x: area.x + inset_x,
        y: area.y + inset_y,
        ..*area
    };
    for (area, radius, colour) in &layout.boxes {
        paint.rectangle(moved(area), *radius, *colour);
    }
    let logo_spot = layout.logo.map(|area| {
        let area = moved(&area);
        [
            area.x / width as f32,
            area.y / height as f32,
            area.width / width as f32,
            area.height / height as f32,
        ]
    });
    if let (Some(area), Some(picture)) = (&layout.logo, logo) {
        paint.picture_in(picture, moved(area));
    }
    if let Some(row) = layout.stars {
        for index in 0..STARS {
            let (middle, reach, colour) = star_at(&row, index, title);
            paint.star((middle.0 + inset_x, middle.1 + inset_y), reach, colour);
        }
    }
    let mut ink = Ink::new(width as usize, height as usize);
    let offset = |place: (f32, f32)| ((place.0 + inset_x).round(), (place.1 + inset_y).round());
    let (first_colour, second_colour) = word_colours(title);
    ink.words(
        engine,
        &plan.first,
        offset(layout.first),
        first_colour,
        look.left_aligned || look.shape == TitleShape::Pattern,
    );
    if let (Some(words), Some(place)) = (&plan.second, layout.second) {
        ink.words(
            engine,
            words,
            offset(place),
            second_colour,
            look.left_aligned,
        );
    }
    ink.paint_styled(&mut paint, title, pixels);
    if let (Some(marks), Some(places)) = (&plan.marks, layout.marks) {
        let mut ink = Ink::new(width as usize, height as usize);
        for (words, place) in marks.iter().zip(places) {
            ink.words(engine, words, offset(place), title.accent, true);
        }
        ink.paint_words_on(&mut paint);
    }
    (paint.into_picture(), (width, height), logo_spot)
}

/// Where one of a row of stars goes, how far it reaches, and its colour.
pub(crate) fn star_at(row: &StarRow, index: usize, title: &Title) -> ((f32, f32), f32, Colour) {
    let middle = (
        row.x + row.size * (index as f32 + 0.5) * 1.12,
        row.y + row.size * 0.55,
    );
    let colour = if index < row.lit {
        title.accent
    } else {
        STAR_OFF
    };
    (middle, row.size * 0.5, colour)
}

/// The size in letter pixels a title draws at, on a canvas of this shape.
pub(crate) fn letter_pixels(title: &Title, canvas: (u32, u32)) -> f32 {
    let shorter = canvas.0.min(canvas.1).max(1) as f32;
    (title.size as f32 / WHOLE as f32 * shorter).max(1.0)
}

/// Which font to draw in, and whether the one asked for is missing.
pub(crate) fn font_for(title: &Title) -> (&str, bool) {
    let wanted = title.font.as_str();
    if has_font(wanted) {
        (wanted, false)
    } else {
        (DEFAULT_FONT, true)
    }
}

/// Draws a title for a canvas of this size, at the size it shows at there, so it stays sharp.
pub fn draw_title(title: &Title, canvas: (u32, u32)) -> TitleDrawing {
    draw_title_with_logo(title, canvas, None)
}

/// The same, with the picture for its logo, if it has one and it could be read.
pub fn draw_title_with_logo(
    title: &Title,
    canvas: (u32, u32),
    logo: Option<&StillPicture>,
) -> TitleDrawing {
    if let Some(art) = &title.art {
        return crate::art::draw_art_title(title, art, canvas);
    }
    let (font, font_missing) = font_for(title);
    let pixels = letter_pixels(title, canvas);

    let mut engine = engine();
    let (picture, size, logo_spot) = paint_title(&mut engine, title, font, canvas, pixels, logo);
    let longest = size.0.max(size.1);
    if longest <= LARGEST_SIDE {
        return TitleDrawing {
            scale: scale_for_pixels((picture.width, picture.height), canvas, 1.0),
            picture,
            font_missing,
            logo_spot,
        };
    }
    // Too big to draw, so it is drawn smaller and shown larger.
    let shrink = f64::from(LARGEST_SIDE - 16) / f64::from(longest);
    let (picture, _, logo_spot) = paint_title(
        &mut engine,
        title,
        font,
        canvas,
        pixels * shrink as f32,
        logo,
    );
    TitleDrawing {
        scale: scale_for_pixels((picture.width, picture.height), canvas, shrink),
        picture,
        font_missing,
        logo_spot,
    }
}

/// A font's name in that font, for choosing from a list. `None` if it has no way to show it.
pub fn draw_font_name(name: &str, pixels: f32, colour: Colour) -> Option<StillPicture> {
    let mut engine = engine();
    let words = lay_out(&mut engine, name, name, Setting::PLAIN, pixels);
    let width = words.width.ceil() as u32 + 2;
    let height = words.height.ceil() as u32 + 2;
    if width > LARGEST_SIDE || height > LARGEST_SIDE {
        return None;
    }
    let mut ink = Ink::new(width as usize, height as usize);
    ink.words(&mut engine, &words, (1.0, 1.0), colour, true);
    let mut paint = Paint::new(width, height);
    ink.paint_words(&mut paint);
    Some(paint.into_picture())
}

#[cfg(test)]
mod tests {
    use bareclip_core::{
        placement::{LARGEST_SCALE, Placement, SMALLEST_SCALE},
        title::{LARGEST_SIZE, SMALLEST_SIZE, STYLE_GROUPS, TitleStyle},
    };

    use super::*;

    const CANVAS: (u32, u32) = (1920, 1080);

    fn pixel(picture: &StillPicture, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * picture.width + x) * 4) as usize;
        [
            picture.rgba[at],
            picture.rgba[at + 1],
            picture.rgba[at + 2],
            picture.rgba[at + 3],
        ]
    }

    /// How much of a picture has anything on it.
    fn covered_pixels(picture: &StillPicture) -> usize {
        picture
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[3] > 128)
            .count()
    }

    /// How much of a picture is close to a colour and solid.
    fn pixels_of(picture: &StillPicture, colour: Colour) -> usize {
        picture
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| {
                p[3] > 240
                    && p[..3]
                        .iter()
                        .zip(colour)
                        .all(|(got, want)| got.abs_diff(want) < 16)
            })
            .count()
    }

    #[test]
    fn every_style_draws_something() {
        for (_, styles) in STYLE_GROUPS {
            for style in styles {
                let drawing = draw_title(&Title::in_style(*style), CANVAS);
                assert!(covered_pixels(&drawing.picture) > 100, "{style:?}");
                assert!(!drawing.font_missing, "{style:?}");
            }
        }
    }

    #[test]
    fn drawing_the_same_title_twice_gives_the_same_pixels() {
        let title = Title::in_style(TitleStyle::Outlined);
        let (one, two) = (draw_title(&title, CANVAS), draw_title(&title, CANVAS));
        assert_eq!(one.picture.rgba, two.picture.rgba);
        assert_eq!(one.scale, two.scale);
    }

    #[test]
    fn a_title_shows_at_the_size_it_was_drawn() {
        let drawing = draw_title(&Title::default(), CANVAS);
        let placement = Placement {
            scale: drawing.scale,
            ..Placement::default()
        };
        let size = (drawing.picture.width, drawing.picture.height);
        let rect = placement.rect(size, CANVAS);
        assert!((rect.width - size.0 as f32).abs() < 1.0);
        assert!((rect.height - size.1 as f32).abs() < 1.0);
    }

    #[test]
    fn a_larger_size_gives_larger_letters() {
        let small = draw_title(
            &Title {
                size: WHOLE / 20,
                ..Title::default()
            },
            CANVAS,
        );
        let large = draw_title(
            &Title {
                size: WHOLE / 5,
                ..Title::default()
            },
            CANVAS,
        );
        let ratio = large.picture.width as f32 / small.picture.width as f32;
        assert!((3.0..5.0).contains(&ratio), "{ratio}");
    }

    #[test]
    fn size_goes_by_the_canvass_shorter_side() {
        let wide = draw_title(&Title::default(), (1920, 1080));
        let tall = draw_title(&Title::default(), (1080, 1920));
        assert_eq!(wide.picture.height, tall.picture.height);
    }

    #[test]
    fn more_words_make_a_wider_picture_and_more_lines_a_taller_one() {
        let one = draw_title(&Title::default(), CANVAS).picture;
        let wider = draw_title(
            &Title {
                text: "A good many more words".to_owned(),
                ..Title::default()
            },
            CANVAS,
        )
        .picture;
        let taller = draw_title(
            &Title {
                text: "Text\nText\nText".to_owned(),
                ..Title::default()
            },
            CANVAS,
        )
        .picture;
        assert!(wider.width > one.width * 2);
        assert!(taller.height > one.height * 2);
    }

    #[test]
    fn the_text_colour_is_what_the_letters_come_out_in() {
        let title = Title {
            colour: [255, 0, 0],
            ..Title::default()
        };
        let picture = draw_title(&title, CANVAS).picture;
        assert!(pixels_of(&picture, [255, 0, 0]) > 500);
        assert_eq!(pixels_of(&picture, [255, 255, 255]), 0);
    }

    #[test]
    fn a_background_colour_fills_a_box_behind_the_words() {
        let plain = draw_title(&Title::default(), CANVAS).picture;
        let boxed = draw_title(
            &Title {
                background: Some([0, 0, 255]),
                ..Title::default()
            },
            CANVAS,
        )
        .picture;
        assert_eq!(pixels_of(&plain, [0, 0, 255]), 0);
        assert!(pixels_of(&boxed, [0, 0, 255]) > 5000);
        assert_eq!(pixel(&plain, 0, 0)[3], 0, "the corners are see-through");
    }

    #[test]
    fn colours_are_already_scaled_by_how_see_through_they_are() {
        let picture = draw_title(&Title::in_style(TitleStyle::Outlined), CANVAS).picture;
        for p in picture.rgba.as_chunks::<4>().0 {
            assert!(p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3], "{p:?}");
        }
    }

    #[test]
    fn an_outline_goes_around_the_letters() {
        let without = draw_title(
            &Title {
                style: TitleStyle::Plain,
                ..Title::in_style(TitleStyle::Outlined)
            },
            CANVAS,
        )
        .picture;
        let with = draw_title(&Title::in_style(TitleStyle::Outlined), CANVAS).picture;
        assert!(covered_pixels(&with) > covered_pixels(&without));
        assert!(
            pixels_of(&with, [0, 0, 0]) > 500,
            "there is black around them"
        );
        assert!(
            pixels_of(&with, [255, 255, 255]) > 500,
            "the letters stay white"
        );
    }

    #[test]
    fn a_shadow_falls_to_the_lower_right_and_is_soft() {
        let picture = draw_title(&Title::in_style(TitleStyle::LargeHeading), CANVAS).picture;
        let soft = picture
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| (10..200).contains(&p[3]) && p[0] < 20)
            .count();
        assert!(soft > 1000, "{soft}");
    }

    #[test]
    fn accent_lines_are_drawn_in_the_accent_colour() {
        for style in [
            TitleStyle::Framed,
            TitleStyle::DoubleLines,
            TitleStyle::Underlined,
            TitleStyle::Boxed,
            TitleStyle::LowerThird,
            TitleStyle::SideBar,
        ] {
            let title = Title {
                accent: [0, 200, 0],
                ..Title::in_style(style)
            };
            let picture = draw_title(&title, CANVAS).picture;
            assert!(pixels_of(&picture, [0, 200, 0]) > 200, "{style:?}");
        }
    }

    #[test]
    fn a_pill_has_round_ends() {
        let picture = draw_title(&Title::in_style(TitleStyle::Pill), CANVAS).picture;
        let middle = picture.height / 2;
        let left_of_row = |y: u32| (0..picture.width).find(|x| pixel(&picture, *x, y)[3] > 128);
        let at_middle = left_of_row(middle).expect("it reaches the middle row");
        let near_top = (0..picture.height)
            .find_map(left_of_row)
            .expect("it has a top row");
        assert!(near_top > at_middle + 20, "{near_top} and {at_middle}");
    }

    #[test]
    fn the_second_line_only_shows_on_styles_with_one() {
        let with_words = |style, words: &str| Title {
            second_text: words.to_owned(),
            ..Title::in_style(style)
        };
        let short = draw_title(&with_words(TitleStyle::Underlined, ""), CANVAS).picture;
        let long = draw_title(
            &with_words(TitleStyle::Underlined, "A much longer second line of words"),
            CANVAS,
        )
        .picture;
        assert!(long.width > short.width);
        let ignored = draw_title(
            &with_words(TitleStyle::Plain, "A much longer second line of words"),
            CANVAS,
        )
        .picture;
        assert_eq!(
            ignored.width,
            draw_title(&Title::default(), CANVAS).picture.width
        );
    }

    #[test]
    fn nothing_typed_still_gives_something_to_grab() {
        let empty = Title {
            text: String::new(),
            ..Title::default()
        };
        let picture = draw_title(&empty, CANVAS).picture;
        assert!(picture.width > 10 && picture.height > 10);
        assert_eq!(covered_pixels(&picture), 0);
    }

    #[test]
    fn a_tiny_title_keeps_a_scale_it_can_show_at() {
        let tiny = Title {
            text: "i".to_owned(),
            size: SMALLEST_SIZE,
            ..Title::default()
        };
        for canvas in [CANVAS, (3840, 2160), (1080, 1920)] {
            let drawing = draw_title(&tiny, canvas);
            assert!(drawing.scale > SMALLEST_SCALE, "{}", drawing.scale);
            let placement = Placement {
                scale: drawing.scale,
                ..Placement::default()
            };
            let size = (drawing.picture.width, drawing.picture.height);
            let rect = placement.rect(size, canvas);
            assert!((rect.height - size.1 as f32).abs() < 1.0, "{canvas:?}");
        }
    }

    #[test]
    fn a_huge_title_is_drawn_small_enough_for_the_gpu_and_shown_larger() {
        let huge = Title {
            text: "A very long line of words that goes on and on and on".to_owned(),
            size: LARGEST_SIZE,
            ..Title::default()
        };
        let drawing = draw_title(&huge, CANVAS);
        assert!(drawing.picture.width <= LARGEST_SIDE);
        assert!(drawing.picture.height <= LARGEST_SIDE);
        assert!(covered_pixels(&drawing.picture) > 1000);
        assert!(drawing.scale <= LARGEST_SCALE);
    }

    #[test]
    fn a_font_that_isnt_here_says_so_and_still_draws() {
        let title = Title {
            font: "No Font Is Called This".to_owned(),
            ..Title::default()
        };
        let drawing = draw_title(&title, CANVAS);
        assert!(drawing.font_missing);
        let standing_in = draw_title(&Title::default(), CANVAS);
        assert_eq!(drawing.picture.rgba, standing_in.picture.rgba);
    }

    #[test]
    fn different_fonts_look_different() {
        let in_font = |font: &str| {
            draw_title(
                &Title {
                    font: font.to_owned(),
                    ..Title::default()
                },
                CANVAS,
            )
            .picture
        };
        assert_ne!(in_font("Abril Fatface").rgba, in_font("Space Mono").rgba);
    }

    #[test]
    fn words_in_other_scripts_draw() {
        for words in ["हिन्दी", "日本語", "한국어", "Русский", "العربية"]
        {
            let title = Title {
                text: words.to_owned(),
                ..Title::default()
            };
            let picture = draw_title(&title, CANVAS).picture;
            assert!(covered_pixels(&picture) > 100, "{words}");
        }
    }

    #[test]
    fn a_font_name_can_be_drawn_in_its_own_font() {
        let picture = draw_font_name("Abril Fatface", 20.0, [255, 255, 255]).expect("it draws");
        assert!(picture.width > 20 && picture.height > 10);
        assert!(covered_pixels(&picture) > 20);
    }

    /// Saves every style on a grey ground, to look at. Set `BARECLIP_SCREENS` to a folder.
    #[test]
    #[ignore = "makes a picture to look at, not a check"]
    fn draw_every_style() {
        let folder = std::env::var("BARECLIP_SCREENS").expect("BARECLIP_SCREENS is set");
        let styles: Vec<TitleStyle> = STYLE_GROUPS
            .iter()
            .flat_map(|(_, styles)| styles.iter().copied())
            .collect();
        let (cell_width, cell_height) = (640_usize, 300_usize);
        let columns = 3;
        let rows = styles.len().div_ceil(columns);
        let (width, height) = (cell_width * columns, cell_height * rows);
        let mut sheet = vec![70_u8; width * height * 3];
        for (index, style) in styles.iter().enumerate() {
            let picture = draw_title(&Title::in_style(*style), (1280, 720)).picture;
            let left = (index % columns) * cell_width;
            let top = (index / columns) * cell_height;
            for y in 0..(picture.height as usize).min(cell_height) {
                for x in 0..(picture.width as usize).min(cell_width) {
                    let from = (y * picture.width as usize + x) * 4;
                    let to = ((top + y) * width + left + x) * 3;
                    let amount = 255 - u32::from(picture.rgba[from + 3]);
                    for part in 0..3 {
                        let under = u32::from(sheet[to + part]) * amount / 255;
                        sheet[to + part] = (u32::from(picture.rgba[from + part]) + under) as u8;
                    }
                }
            }
        }
        let mut file = format!("P6 {width} {height} 255\n").into_bytes();
        file.extend(sheet);
        std::fs::write(format!("{folder}/title-styles.ppm"), file).expect("it can be written");
    }

    #[test]
    #[ignore = "a measurement, not a check"]
    fn how_long_titles_take() {
        let started = std::time::Instant::now();
        let list = crate::font_list();
        println!(
            "Finding fonts: {:?}, {} shipped and {} on this PC",
            started.elapsed(),
            list.shipped.len(),
            list.on_this_pc.len()
        );
        for (_, styles) in STYLE_GROUPS {
            for style in styles {
                let started = std::time::Instant::now();
                let drawing = draw_title(&Title::in_style(*style), CANVAS);
                println!(
                    "{style:?}: {:?}, {} x {}",
                    started.elapsed(),
                    drawing.picture.width,
                    drawing.picture.height
                );
            }
        }
    }
}
