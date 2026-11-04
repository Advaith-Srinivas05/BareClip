//! Lays out a title's words and shapes, and paints them into a see-through picture.

use bareclip_core::{
    canvas::Colour,
    picture::StillPicture,
    placement::WHOLE,
    title::{DEFAULT_FONT, Title, TitleShape, scale_for_pixels},
};
use cosmic_text::{Attrs, Buffer, Color, Family, Metrics, Shaping, Weight, Wrap};

use crate::fonts::{Engine, engine, has_font};

/// The largest picture a title draws, so that it always fits on the GPU.
const LARGEST_SIDE: u32 = 4096;
/// A picture can't show smaller than a twentieth of what fits the canvas, so a tiny title gets
/// empty room around it rather than growing.
const SMALLEST_SHARE_OF_CANVAS: u32 = 20;
const LINE_HEIGHT: f32 = 1.2;
/// Sizes of things around the words, as shares of how tall the letters are.
const PADDING: f32 = 0.28;
const OUTLINE: f32 = 0.05;
const SHADOW_OFFSET: f32 = 0.035;
const SHADOW_BLUR: f32 = 0.05;
const LINE_THICKNESS: f32 = 0.07;
const LINE_GAP: f32 = 0.22;
const SIDE_BAR: f32 = 0.11;
const OUTLINE_COLOUR: Colour = [0, 0, 0];
const SHADOW_OPACITY: f32 = 0.7;

/// A title ready to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TitleDrawing {
    pub picture: StillPicture,
    /// The scale that shows it at the size it was asked for.
    pub scale: i32,
    /// The title's font isn't on this PC, so another stood in for it.
    pub font_missing: bool,
}

/// Words laid out, ready to paint.
struct Words {
    buffer: Buffer,
    width: f32,
    height: f32,
}

fn lay_out(engine: &mut Engine, text: &str, font: &str, bold: bool, pixels: f32) -> Words {
    let metrics = Metrics::new(pixels, pixels * LINE_HEIGHT);
    let mut buffer = Buffer::new(&mut engine.system, metrics);
    buffer.set_wrap(Wrap::None);
    buffer.set_size(None, None);
    let attrs = Attrs::new().family(Family::Name(font)).weight(if bold {
        Weight::BOLD
    } else {
        Weight::NORMAL
    });
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
struct Paint {
    width: usize,
    height: usize,
    rgba: Vec<f32>,
}

impl Paint {
    fn new(width: u32, height: u32) -> Self {
        let (width, height) = (width as usize, height as usize);
        Self {
            width,
            height,
            rgba: vec![0.0; width * height * 4],
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
    fn rectangle(&mut self, area: Area, radius: f32, colour: Colour) {
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

    fn into_picture(self) -> StillPicture {
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
struct Area {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

/// How much of each pixel the words cover, and their colour there. A colour font such as an emoji
/// font brings colours of its own.
struct Ink {
    width: usize,
    height: usize,
    covered: Vec<f32>,
    colours: Vec<[f32; 3]>,
}

impl Ink {
    fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            covered: vec![0.0; width * height],
            colours: vec![[0.0; 3]; width * height],
        }
    }

    fn words(
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

/// Where everything goes, working from the words outwards.
struct Layout {
    width: f32,
    height: f32,
    boxes: Vec<(Area, f32, Colour)>,
    first: (f32, f32),
    second: Option<(f32, f32)>,
}

fn arrange(title: &Title, first: &Words, second: Option<&Words>, pixels: f32) -> Layout {
    let look = title.style.look();
    let padding = pixels * PADDING;
    let line = (pixels * LINE_THICKNESS).max(1.0);
    let gap = pixels * LINE_GAP;
    let (second_width, second_height) =
        second.map_or((0.0, 0.0), |words| (words.width, words.height));
    let accent = title.accent;

    let widest = first.width.max(second_width);
    let bar_room = pixels * SIDE_BAR + gap;
    // How wide and tall everything inside the padding is, how far in that starts, where the two
    // lots of words start going down, and how far in each starts when they are left-aligned.
    let (inner_width, inner_height, left, first_y, second_y, first_inset, second_inset) =
        match look.shape {
            TitleShape::Plain | TitleShape::Pill => (
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
            pixels * 0.06
        };
        boxes.push((
            Area {
                x: 0.0,
                y: 0.0,
                width,
                height,
            },
            radius,
            background,
        ));
    }
    let across = |x: f32, y: f32, wide: f32, tall: f32| Area {
        x,
        y,
        width: wide,
        height: tall,
    };
    match look.shape {
        TitleShape::Plain | TitleShape::Pill => {}
        TitleShape::Frame => {
            let (right, bottom) = (width - padding * 0.5, height - padding * 0.5);
            let near = padding * 0.5;
            boxes.push((across(near, near, right - near, line), 0.0, accent));
            boxes.push((across(near, bottom - line, right - near, line), 0.0, accent));
            boxes.push((across(near, near, line, bottom - near), 0.0, accent));
            boxes.push((across(right - line, near, line, bottom - near), 0.0, accent));
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
    }

    Layout {
        width,
        height,
        boxes,
        first: (first_x, first_y),
        second: second.map(|_| (second_x, second_y)),
    }
}

/// The picture's size, with room for anything that spills past the layout, and at least enough
/// that it can show at its own pixels on this canvas.
fn picture_size(layout: &Layout, canvas: (u32, u32), spill: f32) -> ((u32, u32), (f32, f32)) {
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

fn paint_title(
    engine: &mut Engine,
    title: &Title,
    font: &str,
    canvas: (u32, u32),
    pixels: f32,
) -> (StillPicture, (u32, u32)) {
    let look = title.style.look();
    let first = lay_out(engine, &title.text, font, look.bold, pixels);
    let second_pixels = look
        .second_line
        .map(|percent| pixels * percent as f32 / 100.0);
    let second = second_pixels.map(|size| lay_out(engine, &title.second_text, font, false, size));
    let layout = arrange(title, &first, second.as_ref(), pixels);
    let outline = (pixels * OUTLINE).max(1.0);
    let spill = pixels * (SHADOW_OFFSET + SHADOW_BLUR * 3.0) + outline + 2.0;
    let ((width, height), (inset_x, inset_y)) = picture_size(&layout, canvas, spill);
    if width > LARGEST_SIDE || height > LARGEST_SIDE {
        return (StillPicture::new(1, 1, vec![0; 4]), (width, height));
    }

    let mut paint = Paint::new(width, height);
    for (area, radius, colour) in &layout.boxes {
        let moved = Area {
            x: area.x + inset_x,
            y: area.y + inset_y,
            ..*area
        };
        paint.rectangle(moved, *radius, *colour);
    }
    let mut ink = Ink::new(width as usize, height as usize);
    let offset = |place: (f32, f32)| ((place.0 + inset_x).round(), (place.1 + inset_y).round());
    ink.words(
        engine,
        &first,
        offset(layout.first),
        title.colour,
        look.left_aligned,
    );
    if let (Some(words), Some(place)) = (&second, layout.second) {
        ink.words(
            engine,
            words,
            offset(place),
            title.colour,
            look.left_aligned,
        );
    }
    if look.shadow {
        ink.paint_shadow(&mut paint, pixels * SHADOW_OFFSET, pixels * SHADOW_BLUR);
    }
    if look.outline {
        ink.paint_outline(&mut paint, outline, OUTLINE_COLOUR);
    }
    ink.paint_words(&mut paint);
    (paint.into_picture(), (width, height))
}

/// Draws a title for a canvas of this size, at the size it shows at there, so it stays sharp.
pub fn draw_title(title: &Title, canvas: (u32, u32)) -> TitleDrawing {
    let wanted_font = title.font.as_str();
    let font_missing = !has_font(wanted_font);
    let font = if font_missing {
        DEFAULT_FONT
    } else {
        wanted_font
    };
    let shorter = canvas.0.min(canvas.1).max(1) as f32;
    let pixels = (title.size as f32 / WHOLE as f32 * shorter).max(1.0);

    let mut engine = engine();
    let (picture, size) = paint_title(&mut engine, title, font, canvas, pixels);
    let longest = size.0.max(size.1);
    if longest <= LARGEST_SIDE {
        return TitleDrawing {
            scale: scale_for_pixels((picture.width, picture.height), canvas, 1.0),
            picture,
            font_missing,
        };
    }
    // Too big to draw, so it is drawn smaller and shown larger.
    let shrink = f64::from(LARGEST_SIDE - 16) / f64::from(longest);
    let (picture, _) = paint_title(&mut engine, title, font, canvas, pixels * shrink as f32);
    TitleDrawing {
        scale: scale_for_pixels((picture.width, picture.height), canvas, shrink),
        picture,
        font_missing,
    }
}

/// A font's name in that font, for choosing from a list. `None` if it has no way to show it.
pub fn draw_font_name(name: &str, pixels: f32, colour: Colour) -> Option<StillPicture> {
    let mut engine = engine();
    let words = lay_out(&mut engine, name, name, false, pixels);
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
