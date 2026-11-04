//! Reads an image file into memory, ready to draw.

use std::path::Path;

use bareclip_core::picture::StillPicture;
use ffmpeg_next as ffmpeg;
use ffmpeg_next::{
    format::Pixel,
    frame,
    media::Type,
    software::scaling::{Context as Scaler, Flags},
};

use crate::error::MediaError;

/// The longest side kept in memory. It is larger than any finished video can be, so nothing is
/// lost, and a huge photo doesn't take up gigabytes on the GPU.
const LONGEST_SIDE: u32 = 4096;

/// The size to read a picture at: its own, or smaller with the same shape if it is huge.
fn size_to_keep(width: u32, height: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest <= LONGEST_SIDE {
        return (width, height);
    }
    let shrink = |side: u32| {
        let kept = u64::from(side) * u64::from(LONGEST_SIDE) / u64::from(longest);
        (kept as u32).max(1)
    };
    (shrink(width), shrink(height))
}

/// Makes each pixel's colour already take its see-through amount into account. Smoothing colours
/// that haven't been treated this way bleeds the colour of invisible pixels into the edges of a
/// logo, as a dark or bright fringe.
fn premultiply(rgba: &mut [u8]) {
    let (pixels, _) = rgba.as_chunks_mut::<4>();
    for pixel in pixels {
        let alpha = u32::from(pixel[3]);
        if alpha < 255 {
            for value in &mut pixel[..3] {
                *value = ((u32::from(*value) * alpha + 127) / 255) as u8;
            }
        }
    }
}

/// How a photo has to be turned to show the right way up. Cameras and phones usually save the
/// sensor's picture as it came, and note which way up the camera was held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Orientation {
    Upright,
    Mirrored,
    HalfTurn,
    Flipped,
    /// Mirrored across the line from the top left to the bottom right.
    Transposed,
    QuarterClockwise,
    /// Mirrored across the line from the top right to the bottom left.
    Transversed,
    QuarterAnticlockwise,
}

impl Orientation {
    /// Reads FFmpeg's matrix for a picture, of nine numbers row by row. Only the top left four say
    /// anything about a turn. Anything but a whole quarter turn or mirror is treated as upright,
    /// because no photo format can store one.
    fn from_matrix(matrix: &[i32; 9]) -> Self {
        match [matrix[0], matrix[1], matrix[3], matrix[4]].map(i32::signum) {
            [-1, 0, 0, 1] => Self::Mirrored,
            [-1, 0, 0, -1] => Self::HalfTurn,
            [1, 0, 0, -1] => Self::Flipped,
            [0, 1, 1, 0] => Self::Transposed,
            [0, 1, -1, 0] => Self::QuarterClockwise,
            [0, -1, -1, 0] => Self::Transversed,
            [0, -1, 1, 0] => Self::QuarterAnticlockwise,
            _ => Self::Upright,
        }
    }

    fn of(picture: &frame::Video) -> Self {
        let Some(side_data) = picture.side_data(frame::side_data::Type::DisplayMatrix) else {
            return Self::Upright;
        };
        let (numbers, _) = side_data.data().as_chunks::<4>();
        if numbers.len() < 9 {
            return Self::Upright;
        }
        let mut matrix = [0; 9];
        for (number, bytes) in matrix.iter_mut().zip(numbers) {
            *number = i32::from_ne_bytes(*bytes);
        }
        Self::from_matrix(&matrix)
    }

    /// The size of a picture once it is turned.
    fn turned_size(self, width: u32, height: u32) -> (u32, u32) {
        match self {
            Self::Upright | Self::Mirrored | Self::HalfTurn | Self::Flipped => (width, height),
            Self::Transposed
            | Self::QuarterClockwise
            | Self::Transversed
            | Self::QuarterAnticlockwise => (height, width),
        }
    }

    /// Where the pixel at `x`, `y` lands, in a picture of this size before it is turned.
    fn place(self, x: usize, y: usize, width: usize, height: usize) -> (usize, usize) {
        let across = width - 1 - x;
        let down = height - 1 - y;
        match self {
            Self::Upright => (x, y),
            Self::Mirrored => (across, y),
            Self::HalfTurn => (across, down),
            Self::Flipped => (x, down),
            Self::Transposed => (y, x),
            Self::QuarterClockwise => (down, x),
            Self::Transversed => (down, across),
            Self::QuarterAnticlockwise => (y, across),
        }
    }

    /// The pixels of a picture of this size after it is turned.
    fn turn(self, rgba: Vec<u8>, width: u32, height: u32) -> Vec<u8> {
        if self == Self::Upright {
            return rgba;
        }
        let (turned_width, _) = self.turned_size(width, height);
        let (width, height, turned_width) =
            (width as usize, height as usize, turned_width as usize);
        let mut turned = vec![0; rgba.len()];
        let (pixels, _) = rgba.as_chunks::<4>();
        for (index, pixel) in pixels.iter().enumerate() {
            let (x, y) = self.place(index % width, index / width, width, height);
            turned[(y * turned_width + x) * 4..][..4].copy_from_slice(pixel);
        }
        turned
    }
}

/// Decodes the first picture in a file.
fn first_picture(path: &Path) -> Result<frame::Video, MediaError> {
    crate::init()?;
    let mut input = ffmpeg::format::input(path)?;
    let stream = input
        .streams()
        .best(Type::Video)
        .ok_or(MediaError::NoVideoStream)?;
    let index = stream.index();
    let mut decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?
        .decoder()
        .video()?;

    let mut decoded = frame::Video::empty();
    for (stream, packet) in input.packets() {
        if stream.index() != index {
            continue;
        }
        decoder.send_packet(&packet)?;
        if decoder.receive_frame(&mut decoded).is_ok() {
            return Ok(decoded);
        }
    }
    decoder.send_eof()?;
    if decoder.receive_frame(&mut decoded).is_err() {
        return Err(MediaError::NoFrames);
    }
    Ok(decoded)
}

/// An image's size once it is the right way up. The way up is only known once the picture is
/// decoded, and this has to agree with `read_still`, because pictures are placed by this size.
pub(crate) fn upright_size(path: &Path) -> Result<(u32, u32), MediaError> {
    let picture = first_picture(path)?;
    Ok(Orientation::of(&picture).turned_size(picture.width(), picture.height()))
}

/// Reads an image, the right way up. For a file with several pictures in it, such as an animated
/// GIF, this is the first one.
pub fn read_still(path: &Path) -> Result<StillPicture, MediaError> {
    let decoded = first_picture(path)?;

    let (width, height) = size_to_keep(decoded.width(), decoded.height());
    let mut scaler = Scaler::get(
        decoded.format(),
        decoded.width(),
        decoded.height(),
        Pixel::RGBA,
        width,
        height,
        Flags::BICUBIC,
    )?;
    let mut converted = frame::Video::empty();
    scaler.run(&decoded, &mut converted)?;

    let row = width as usize * 4;
    let stride = converted.stride(0);
    let mut rgba = Vec::with_capacity(row * height as usize);
    for line in converted.data(0).chunks(stride).take(height as usize) {
        rgba.extend_from_slice(&line[..row]);
    }
    premultiply(&mut rgba);

    let orientation = Orientation::of(&decoded);
    let rgba = orientation.turn(rgba, width, height);
    let (width, height) = orientation.turned_size(width, height);
    Ok(StillPicture::new(width, height, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_that_fits_keeps_its_size() {
        assert_eq!(size_to_keep(4032, 3024), (4032, 3024));
        assert_eq!(size_to_keep(16, 16), (16, 16));
    }

    #[test]
    fn a_huge_picture_shrinks_and_keeps_its_shape() {
        assert_eq!(size_to_keep(8192, 4096), (4096, 2048));
        assert_eq!(size_to_keep(1000, 9000), (455, 4096));
    }

    #[test]
    fn each_way_up_is_read_from_the_matrix_ffmpeg_gives() {
        const ONE: i32 = 65536;
        const LAST: i32 = 1 << 30;
        for (a, b, c, d, expected) in [
            (ONE, 0, 0, ONE, Orientation::Upright),
            (-ONE, 0, 0, ONE, Orientation::Mirrored),
            (-ONE, 0, 0, -ONE, Orientation::HalfTurn),
            (ONE, 0, 0, -ONE, Orientation::Flipped),
            (0, ONE, ONE, 0, Orientation::Transposed),
            (0, ONE, -ONE, 0, Orientation::QuarterClockwise),
            (0, -ONE, -ONE, 0, Orientation::Transversed),
            (0, -ONE, ONE, 0, Orientation::QuarterAnticlockwise),
        ] {
            let matrix = [a, b, 0, c, d, 0, 0, 0, LAST];
            assert_eq!(Orientation::from_matrix(&matrix), expected);
        }
    }

    #[test]
    fn a_matrix_that_is_no_whole_turn_counts_as_upright() {
        let matrix = [46341, 46341, 0, -46341, 46341, 0, 0, 0, 1 << 30];
        assert_eq!(Orientation::from_matrix(&matrix), Orientation::Upright);
    }

    #[test]
    fn turning_moves_every_pixel_and_only_quarter_turns_swap_the_sides() {
        // Two across and one down, in pixels of 1 and 2.
        let pixels = vec![1, 1, 1, 1, 2, 2, 2, 2];
        for (orientation, size, expected) in [
            (Orientation::Upright, (2, 1), [1, 2]),
            (Orientation::Mirrored, (2, 1), [2, 1]),
            (Orientation::HalfTurn, (2, 1), [2, 1]),
            (Orientation::Flipped, (2, 1), [1, 2]),
            (Orientation::Transposed, (1, 2), [1, 2]),
            (Orientation::QuarterClockwise, (1, 2), [1, 2]),
            (Orientation::Transversed, (1, 2), [2, 1]),
            (Orientation::QuarterAnticlockwise, (1, 2), [2, 1]),
        ] {
            assert_eq!(orientation.turned_size(2, 1), size, "{orientation:?}");
            let turned = orientation.turn(pixels.clone(), 2, 1);
            assert_eq!([turned[0], turned[4]], expected, "{orientation:?}");
        }
    }

    #[test]
    fn colours_are_scaled_by_how_see_through_they_are() {
        let mut pixels = [200, 100, 50, 255, 200, 100, 50, 128, 200, 100, 50, 0];

        premultiply(&mut pixels);

        assert_eq!(pixels[..4], [200, 100, 50, 255], "solid is left alone");
        assert_eq!(pixels[4..8], [100, 50, 25, 128]);
        assert_eq!(
            pixels[8..],
            [0, 0, 0, 0],
            "invisible has no colour to bleed"
        );
    }
}
