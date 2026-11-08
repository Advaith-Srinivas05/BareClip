//! Writes GIFs. A GIF can only hold 256 colours, so every picture is kept until the end, then one
//! palette is chosen from all of them and used for the whole file. One palette throughout also
//! lets FFmpeg's GIF encoder store only what changed from the frame before, which keeps files far
//! smaller than a new palette per picture does.

use std::{
    fs::{self, File},
    io::{BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
};

use bareclip_core::picture::{ColorFormat, Yuv420};
use ffmpeg_next as ffmpeg;
use ffmpeg_next::{Rational, codec, encoder, ffi, format, format::Pixel, frame, software::scaling};

use crate::{
    error::MediaError,
    export::temporary_path,
    full::{VideoSettings, coefficients_of},
};

/// How finely colours are counted, per channel of red, green and blue.
const BITS: usize = 6;
const LEVELS: usize = 1 << BITS;
const CELLS: usize = LEVELS * LEVELS * LEVELS;
/// How finely the nearest palette colour is looked up. Finer than the counting, because a dark
/// picture's palette colours can sit closer together than the counting's steps.
const TABLE_BITS: usize = 7;
const TABLE_LEVELS: usize = 1 << TABLE_BITS;
const TABLE_CELLS: usize = TABLE_LEVELS * TABLE_LEVELS * TABLE_LEVELS;
/// One slot is left see-through, so the encoder can store only what changed between frames.
const COLOURS: usize = 255;
const SEE_THROUGH: u8 = 255;
/// 8 by 8 ordered dithering: the same pixel always gets the same nudge, so a still part of the
/// picture doesn't flicker and stays cheap to store.
const BAYER: [[i32; 8]; 8] = [
    [0, 32, 8, 40, 2, 34, 10, 42],
    [48, 16, 56, 24, 50, 18, 58, 26],
    [12, 44, 4, 36, 14, 46, 6, 38],
    [60, 28, 52, 20, 62, 30, 54, 22],
    [3, 35, 11, 43, 1, 33, 9, 41],
    [51, 19, 59, 27, 49, 17, 57, 25],
    [15, 47, 7, 39, 13, 45, 5, 37],
    [63, 31, 55, 23, 61, 29, 53, 21],
];
/// How far a colour is nudged, out of 255, each way. Measured on real footage: enough to break up
/// bands, and 8 made the pattern itself show up on dark surfaces.
const DITHER_SPREAD: i32 = 3;

fn cell_of(r: u8, g: u8, b: u8) -> usize {
    (usize::from(r) >> (8 - BITS)) << (2 * BITS)
        | (usize::from(g) >> (8 - BITS)) << BITS
        | (usize::from(b) >> (8 - BITS))
}

/// The middle of a counting cell, as a colour.
fn cell_colour(index: usize) -> [u8; 3] {
    let centre = |level: usize| ((level << (8 - BITS)) + (1 << (7 - BITS))) as u8;
    [
        centre(index >> (2 * BITS)),
        centre((index >> BITS) & (LEVELS - 1)),
        centre(index & (LEVELS - 1)),
    ]
}

fn table_cell_of(r: u8, g: u8, b: u8) -> usize {
    (usize::from(r) >> (8 - TABLE_BITS)) << (2 * TABLE_BITS)
        | (usize::from(g) >> (8 - TABLE_BITS)) << TABLE_BITS
        | (usize::from(b) >> (8 - TABLE_BITS))
}

fn table_cell_colour(index: usize) -> [u8; 3] {
    let centre = |level: usize| ((level << (8 - TABLE_BITS)) + (1 << (7 - TABLE_BITS))) as u8;
    [
        centre(index >> (2 * TABLE_BITS)),
        centre((index >> TABLE_BITS) & (TABLE_LEVELS - 1)),
        centre(index & (TABLE_LEVELS - 1)),
    ]
}

/// A cell in the 65 by 65 by 65 table of running totals.
fn at(r: usize, g: usize, b: usize) -> usize {
    (r * (LEVELS + 1) + g) * (LEVELS + 1) + b
}

/// Running totals over every colour counted, so a box's total comes from eight lookups.
struct Totals {
    pixels: Vec<i64>,
    red: Vec<i64>,
    green: Vec<i64>,
    blue: Vec<i64>,
    squares: Vec<f64>,
}

impl Totals {
    fn of(histogram: &[u32]) -> Self {
        let size = (LEVELS + 1) * (LEVELS + 1) * (LEVELS + 1);
        let mut totals = Self {
            pixels: vec![0; size],
            red: vec![0; size],
            green: vec![0; size],
            blue: vec![0; size],
            squares: vec![0.0; size],
        };
        for r in 0..LEVELS {
            for g in 0..LEVELS {
                for b in 0..LEVELS {
                    let index = (r << (2 * BITS)) | (g << BITS) | b;
                    let count = i64::from(histogram[index]);
                    if count == 0 {
                        continue;
                    }
                    let [cr, cg, cb] = cell_colour(index).map(i64::from);
                    let cell = at(r + 1, g + 1, b + 1);
                    totals.pixels[cell] += count;
                    totals.red[cell] += count * cr;
                    totals.green[cell] += count * cg;
                    totals.blue[cell] += count * cb;
                    totals.squares[cell] += count as f64 * (cr * cr + cg * cg + cb * cb) as f64;
                }
            }
        }
        // Each cell becomes the total of everything at or below it in all three directions.
        for r in 1..=LEVELS {
            for g in 1..=LEVELS {
                for b in 1..=LEVELS {
                    let cell = at(r, g, b);
                    macro_rules! accumulate {
                        ($table:expr) => {{
                            let table = &mut $table;
                            table[cell] = table[cell]
                                + table[at(r - 1, g, b)]
                                + table[at(r, g - 1, b)]
                                + table[at(r, g, b - 1)]
                                - table[at(r - 1, g - 1, b)]
                                - table[at(r - 1, g, b - 1)]
                                - table[at(r, g - 1, b - 1)]
                                + table[at(r - 1, g - 1, b - 1)];
                        }};
                    }
                    accumulate!(totals.pixels);
                    accumulate!(totals.red);
                    accumulate!(totals.green);
                    accumulate!(totals.blue);
                    accumulate!(totals.squares);
                }
            }
        }
        totals
    }
}

/// A box of colours: above the low corner, up to and including the high one.
#[derive(Clone, Copy, Debug)]
struct ColourBox {
    low: [usize; 3],
    high: [usize; 3],
}

impl ColourBox {
    /// What the eight corners of this box add up to in a table of running totals.
    fn sum<T>(&self, table: &[T]) -> T
    where
        T: Copy + std::ops::Add<Output = T> + std::ops::Sub<Output = T>,
    {
        let ([r0, g0, b0], [r1, g1, b1]) = (self.low, self.high);
        table[at(r1, g1, b1)] - table[at(r1, g1, b0)] - table[at(r1, g0, b1)]
            + table[at(r1, g0, b0)]
            - table[at(r0, g1, b1)]
            + table[at(r0, g1, b0)]
            + table[at(r0, g0, b1)]
            - table[at(r0, g0, b0)]
    }

    /// How spread out the colours inside are. Zero means there's nothing to gain by splitting it.
    fn spread(&self, totals: &Totals) -> f64 {
        let pixels = self.sum(&totals.pixels);
        if pixels == 0 {
            return 0.0;
        }
        let colour_sum = |table: &[i64]| self.sum(table) as f64;
        let squared = colour_sum(&totals.red).powi(2)
            + colour_sum(&totals.green).powi(2)
            + colour_sum(&totals.blue).powi(2);
        self.sum(&totals.squares) - squared / pixels as f64
    }

    fn split_at(&self, axis: usize, place: usize) -> (Self, Self) {
        let mut lower = *self;
        let mut upper = *self;
        lower.high[axis] = place;
        upper.low[axis] = place;
        (lower, upper)
    }

    /// The best split of the box, if there's any point: the one that leaves the two halves with
    /// their colours the least spread out.
    fn best_split(&self, totals: &Totals) -> Option<(Self, Self)> {
        let mut best: Option<(f64, Self, Self)> = None;
        for axis in 0..3 {
            for place in self.low[axis] + 1..self.high[axis] {
                let (lower, upper) = self.split_at(axis, place);
                if lower.sum(&totals.pixels) == 0 || upper.sum(&totals.pixels) == 0 {
                    continue;
                }
                let spread = lower.spread(totals) + upper.spread(totals);
                if best.is_none_or(|(smallest, ..)| spread < smallest) {
                    best = Some((spread, lower, upper));
                }
            }
        }
        best.map(|(_, lower, upper)| (lower, upper))
    }

    fn average(&self, totals: &Totals) -> [u8; 3] {
        let pixels = self.sum(&totals.pixels).max(1) as f64;
        let of = |table: &[i64]| (self.sum(table) as f64 / pixels).round().clamp(0.0, 255.0) as u8;
        [of(&totals.red), of(&totals.green), of(&totals.blue)]
    }
}

/// Chooses up to `COLOURS` colours from a count of how often each one was seen, by repeatedly
/// splitting the box of colours whose insides are the most spread out.
fn choose_palette(histogram: &[u32]) -> Vec<[u8; 3]> {
    let totals = Totals::of(histogram);
    let everything = ColourBox {
        low: [0, 0, 0],
        high: [LEVELS, LEVELS, LEVELS],
    };
    let mut boxes = vec![(everything, everything.spread(&totals))];
    while boxes.len() < COLOURS {
        let Some((place, _)) = boxes
            .iter()
            .enumerate()
            .filter(|(_, (_, spread))| *spread > 0.0)
            .max_by(|a, b| a.1.1.total_cmp(&b.1.1))
        else {
            break;
        };
        let Some((lower, upper)) = boxes[place].0.best_split(&totals) else {
            boxes[place].1 = 0.0;
            continue;
        };
        boxes[place] = (lower, lower.spread(&totals));
        boxes.push((upper, upper.spread(&totals)));
    }
    boxes
        .iter()
        .map(|(found, _)| found.average(&totals))
        .collect()
}

fn distance(a: [u8; 3], b: [u8; 3]) -> u32 {
    a.iter()
        .zip(b)
        .map(|(a, b)| {
            let difference = i32::from(*a) - i32::from(b);
            (difference * difference) as u32
        })
        .sum()
}

/// Finds the closest palette entry for a colour, working out each lookup cell only the first time
/// it's needed, because a video only ever uses a small part of all possible colours.
struct NearestTable<'a> {
    palette: &'a [[u8; 3]],
    cells: Vec<u16>,
}

impl<'a> NearestTable<'a> {
    const UNKNOWN: u16 = u16::MAX;

    fn new(palette: &'a [[u8; 3]]) -> Self {
        Self {
            palette,
            cells: vec![Self::UNKNOWN; TABLE_CELLS],
        }
    }

    fn nearest(&mut self, r: u8, g: u8, b: u8) -> u8 {
        let cell = table_cell_of(r, g, b);
        let known = self.cells[cell];
        if known != Self::UNKNOWN {
            return known as u8;
        }
        let colour = table_cell_colour(cell);
        let found = self
            .palette
            .iter()
            .enumerate()
            .min_by_key(|(_, entry)| distance(**entry, colour))
            .map_or(0, |(place, _)| place as u16);
        self.cells[cell] = found;
        found as u8
    }
}

/// A palette entry for each pixel of a picture, nudging colours so that gradients break up into
/// a fine pattern rather than bands.
fn map_picture(
    rgb: &[u8],
    width: usize,
    height: usize,
    table: &mut NearestTable,
    into: &mut [u8],
    stride: usize,
) {
    for (y, row) in rgb.chunks_exact(width * 3).take(height).enumerate() {
        let target = &mut into[y * stride..][..width];
        for (x, [r, g, b]) in row.as_chunks::<3>().0.iter().enumerate() {
            let nudge = BAYER[y & 7][x & 7] * 2 * DITHER_SPREAD / 64 - DITHER_SPREAD;
            let channel = |value: u8| (i32::from(value) + nudge).clamp(0, 255) as u8;
            target[x] = table.nearest(channel(*r), channel(*g), channel(*b));
        }
    }
}

/// A GIF being built up, picture by picture. Nothing appears at the real path until it is
/// finished, and giving up leaves nothing behind.
pub struct GifWriter {
    target: PathBuf,
    pictures_path: PathBuf,
    pictures: Option<BufWriter<File>>,
    width: u32,
    height: u32,
    frame_rate: (i32, i32),
    histogram: Vec<u32>,
    /// How many frames each picture kept shows for.
    shown_for: Vec<u32>,
    converter: Option<Converter>,
    source: frame::Video,
    rgb: frame::Video,
}

/// Turns pictures into red, green and blue for one colour standard.
struct Converter {
    context: scaling::Context,
    color: ColorFormat,
}

// SAFETY: a scaling context is only ever used by one thread at a time, and moving it to another
// thread between uses doesn't change that.
unsafe impl Send for Converter {}

impl GifWriter {
    pub fn create(path: &Path, settings: &VideoSettings) -> Result<Self, MediaError> {
        crate::init()?;
        let pictures_path = temporary_path(path).with_extension("pictures");
        let pictures = BufWriter::new(File::create(&pictures_path)?);
        Ok(Self {
            target: path.to_owned(),
            pictures_path,
            pictures: Some(pictures),
            width: settings.width,
            height: settings.height,
            frame_rate: settings.frame_rate,
            histogram: vec![0; CELLS],
            shown_for: Vec::new(),
            converter: None,
            source: frame::Video::new(Pixel::YUV420P, settings.width, settings.height),
            rgb: frame::Video::empty(),
        })
    }

    pub fn encoder_name(&self) -> &str {
        "gif"
    }

    fn pictures_file(&mut self) -> Result<&mut BufWriter<File>, MediaError> {
        self.pictures
            .as_mut()
            .ok_or(MediaError::Ffmpeg(ffmpeg::Error::InvalidData))
    }

    fn convert_to_rgb(&mut self, picture: &Yuv420) -> Result<(), MediaError> {
        let (width, height) = (picture.width as usize, picture.height as usize);
        let chroma = picture.chroma_size();
        let (chroma_width, chroma_height) = (chroma.0 as usize, chroma.1 as usize);
        let planes = [
            (picture.y, picture.y_stride, width, height, 0),
            (picture.u, picture.uv_stride, chroma_width, chroma_height, 1),
            (picture.v, picture.uv_stride, chroma_width, chroma_height, 2),
        ];
        for (from, from_stride, row_width, rows, plane) in planes {
            let stride = self.source.stride(plane);
            let into = self.source.data_mut(plane);
            for row in 0..rows {
                into[row * stride..][..row_width]
                    .copy_from_slice(&from[row * from_stride..][..row_width]);
            }
        }
        let stale = self
            .converter
            .as_ref()
            .is_none_or(|converter| converter.color != picture.color);
        if stale {
            let mut context = scaling::Context::get(
                Pixel::YUV420P,
                picture.width,
                picture.height,
                Pixel::RGB24,
                picture.width,
                picture.height,
                // Without these it repeats each colour value across four pixels, and edges show it.
                scaling::Flags::BILINEAR
                    | scaling::Flags::FULL_CHR_H_INT
                    | scaling::Flags::ACCURATE_RND,
            )?;
            // SAFETY: the context is live, and these calls only read the constant coefficient
            // tables and copy their values into it.
            unsafe {
                let table = ffi::sws_getCoefficients(coefficients_of(picture.color.matrix));
                let output = ffi::sws_getCoefficients(ffi::SWS_CS_DEFAULT);
                ffi::sws_setColorspaceDetails(
                    context.as_mut_ptr(),
                    table,
                    i32::from(picture.color.full_range),
                    output,
                    1,
                    0,
                    1 << 16,
                    1 << 16,
                );
            }
            self.converter = Some(Converter {
                context,
                color: picture.color,
            });
        }
        let Self {
            converter,
            source,
            rgb,
            ..
        } = self;
        if let Some(converter) = converter.as_mut() {
            converter.context.run(source, rgb)?;
        }
        Ok(())
    }

    pub fn write_picture(&mut self, picture: &Yuv420) -> Result<(), MediaError> {
        self.convert_to_rgb(picture)?;
        let (width, height) = (self.width as usize, self.height as usize);
        let stride = self.rgb.stride(0);
        for row in 0..height {
            for [r, g, b] in self.rgb.data(0)[row * stride..][..width * 3]
                .as_chunks::<3>()
                .0
            {
                self.histogram[cell_of(*r, *g, *b)] += 1;
            }
        }
        let tight: Vec<u8> = (0..height)
            .flat_map(|row| {
                self.rgb.data(0)[row * stride..][..width * 3]
                    .iter()
                    .copied()
            })
            .collect();
        self.pictures_file()?.write_all(&tight)?;
        self.shown_for.push(1);
        Ok(())
    }

    /// Shows the last picture for one more frame, which takes no room to keep.
    pub fn repeat_picture(&mut self) -> Result<(), MediaError> {
        if let Some(last) = self.shown_for.last_mut() {
            *last += 1;
        }
        Ok(())
    }

    /// Chooses the palette, encodes every picture with it and puts the file in place.
    pub fn finish(mut self) -> Result<(), MediaError> {
        if let Some(mut pictures) = self.pictures.take() {
            pictures.flush()?;
        }
        let palette = choose_palette(&self.histogram);
        let mut table = NearestTable::new(&palette);
        let temporary = temporary_path(&self.target);
        let written = self.encode(&temporary, &palette, &mut table);
        let _ = fs::remove_file(&self.pictures_path);
        if written.is_ok() {
            fs::rename(&temporary, &self.target)?;
        } else {
            let _ = fs::remove_file(&temporary);
        }
        written
    }

    fn encode(
        &self,
        path: &Path,
        palette: &[[u8; 3]],
        table: &mut NearestTable,
    ) -> Result<(), MediaError> {
        let found = encoder::find_by_name("gif").ok_or(ffmpeg::Error::EncoderNotFound)?;
        let time_base = Rational(self.frame_rate.1, self.frame_rate.0);
        let mut video = codec::context::Context::new_with_codec(found)
            .encoder()
            .video()?;
        video.set_width(self.width);
        video.set_height(self.height);
        video.set_format(Pixel::PAL8);
        video.set_time_base(time_base);
        video.set_frame_rate(Some(Rational(self.frame_rate.0, self.frame_rate.1)));
        let mut encoder = video.open()?;

        let mut output = format::output_as(path, "gif")?;
        output
            .add_stream(encoder::find(codec::Id::GIF))?
            .set_parameters(&*encoder);
        output.write_header()?;
        let stream_time_base = output
            .stream(0)
            .ok_or(MediaError::NoVideoStream)?
            .time_base();

        let (width, height) = (self.width as usize, self.height as usize);
        let mut picture = vec![0; width * height * 3];
        let mut reader = BufReader::new(File::open(&self.pictures_path)?);
        let mut paletted = frame::Video::new(Pixel::PAL8, self.width, self.height);
        let mut pts = 0;
        for shown_for in &self.shown_for {
            reader.read_exact(&mut picture)?;
            for _ in 0..*shown_for {
                // SAFETY: the frame is live. The encoder may still hold the last picture, and this
                // gives the frame new memory if so.
                let writable = unsafe { ffi::av_frame_make_writable(paletted.as_mut_ptr()) };
                if writable < 0 {
                    return Err(ffmpeg::Error::from(writable).into());
                }
                // Asked for after making it writable, because new memory can have longer rows.
                let stride = paletted.stride(0);
                map_picture(&picture, width, height, table, paletted.data_mut(0), stride);
                write_palette(&mut paletted, palette)?;
                paletted.set_pts(Some(pts));
                pts += 1;
                encoder.send_frame(&paletted)?;
                drain(&mut encoder, &mut output, time_base, stream_time_base)?;
            }
        }
        encoder.send_eof()?;
        drain(&mut encoder, &mut output, time_base, stream_time_base)?;
        output.write_trailer()?;
        Ok(())
    }
}

/// Puts the colours where a paletted frame keeps them, with the last slot see-through.
fn write_palette(frame: &mut frame::Video, palette: &[[u8; 3]]) -> Result<(), MediaError> {
    // SAFETY: a paletted frame's second plane is a buffer of 256 four-byte colours, and nothing
    // else reads it until the frame is sent to the encoder.
    let entries = unsafe {
        let data = (*frame.as_mut_ptr()).data[1];
        if data.is_null() {
            return Err(ffmpeg::Error::InvalidData.into());
        }
        std::slice::from_raw_parts_mut(data.cast::<u32>(), 256)
    };
    entries.fill(0);
    for (entry, [r, g, b]) in entries.iter_mut().zip(palette) {
        *entry = 0xff00_0000 | u32::from(*r) << 16 | u32::from(*g) << 8 | u32::from(*b);
    }
    entries[usize::from(SEE_THROUGH)] = 0;
    Ok(())
}

fn drain(
    encoder: &mut encoder::Video,
    output: &mut format::context::Output,
    from: Rational,
    to: Rational,
) -> Result<(), MediaError> {
    let mut packet = ffmpeg::Packet::empty();
    while encoder.receive_packet(&mut packet).is_ok() {
        packet.set_stream(0);
        packet.rescale_ts(from, to);
        packet.write_interleaved(output)?;
    }
    Ok(())
}

impl Drop for GifWriter {
    fn drop(&mut self) {
        if self.pictures.take().is_some() {
            let _ = fs::remove_file(&self.pictures_path);
            let _ = fs::remove_file(temporary_path(&self.target));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn histogram_of(colours: &[([u8; 3], u32)]) -> Vec<u32> {
        let mut histogram = vec![0; CELLS];
        for ([r, g, b], count) in colours {
            histogram[cell_of(*r, *g, *b)] += count;
        }
        histogram
    }

    #[test]
    fn a_picture_with_a_few_colours_keeps_each_one() {
        let colours = [
            ([200, 40, 40], 100),
            ([40, 200, 40], 100),
            ([40, 40, 200], 100),
            ([250, 250, 250], 50),
        ];

        let palette = choose_palette(&histogram_of(&colours));

        assert_eq!(palette.len(), colours.len());
        for (wanted, _) in colours {
            let closest = palette
                .iter()
                .map(|entry| distance(*entry, wanted))
                .min()
                .unwrap();
            assert!(closest <= 3 * 2 * 2, "{wanted:?} came out {closest} away");
        }
    }

    #[test]
    fn a_full_rainbow_stops_at_what_a_gif_can_hold() {
        let histogram = vec![1; CELLS];

        let palette = choose_palette(&histogram);

        assert!(palette.len() <= COLOURS);
        assert!(palette.len() > 200, "only {} colours", palette.len());
    }

    #[test]
    fn colours_seen_more_get_more_of_the_palette() {
        // Far more colours than the palette can hold, so it has to choose.
        let mut histogram = vec![0; CELLS];
        for first in 0..LEVELS {
            for second in 0..LEVELS {
                histogram[cell_of((first * 4) as u8, (second * 4) as u8, 0)] = 1000;
                histogram[cell_of(0, (first * 4) as u8, (second * 4) as u8)] = 1;
            }
        }

        let palette = choose_palette(&histogram);
        let common = palette.iter().filter(|c| c[0] > c[2]).count();
        let rare = palette.iter().filter(|c| c[2] > c[0]).count();

        assert!(common > rare * 2, "{common} common and {rare} rare");
    }

    #[test]
    fn every_colour_finds_its_nearest_palette_entry() {
        let palette = [[0, 0, 0], [255, 255, 255], [255, 0, 0]];

        let mut table = NearestTable::new(&palette);

        assert_eq!(table.nearest(10, 10, 10), 0);
        assert_eq!(table.nearest(240, 240, 240), 1);
        assert_eq!(table.nearest(230, 20, 20), 2);
    }

    #[test]
    fn a_flat_colour_stays_on_the_nearest_entries_only() {
        let palette = [[100, 100, 100], [108, 108, 108], [0, 0, 0], [255, 255, 255]];
        let mut table = NearestTable::new(&palette);
        let rgb = [103_u8; 16 * 16 * 3];
        let mut mapped = [SEE_THROUGH; 16 * 16];

        map_picture(&rgb, 16, 16, &mut table, &mut mapped, 16);

        assert!(mapped.iter().all(|index| *index < 2), "{mapped:?}");
        assert!(
            mapped.contains(&0) && mapped.contains(&1),
            "it should dither"
        );
    }

    #[test]
    fn the_same_picture_dithers_the_same_every_time() {
        let palette = [[50, 50, 50], [60, 60, 60]];
        let mut table = NearestTable::new(&palette);
        let rgb: Vec<u8> = (0..16 * 16 * 3).map(|i| (50 + i % 10) as u8).collect();
        let (mut first, mut second) = ([0_u8; 256], [0_u8; 256]);

        map_picture(&rgb, 16, 16, &mut table, &mut first, 16);
        map_picture(&rgb, 16, 16, &mut table, &mut second, 16);

        assert_eq!(first, second);
    }
}
