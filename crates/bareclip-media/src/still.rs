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

/// Reads an image. For a file with several pictures in it, such as an animated GIF, this is the
/// first one.
pub fn read_still(path: &Path) -> Result<StillPicture, MediaError> {
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
    let mut found = false;
    for (stream, packet) in input.packets() {
        if stream.index() != index {
            continue;
        }
        decoder.send_packet(&packet)?;
        if decoder.receive_frame(&mut decoded).is_ok() {
            found = true;
            break;
        }
    }
    if !found {
        decoder.send_eof()?;
        if decoder.receive_frame(&mut decoded).is_err() {
            return Err(MediaError::NoFrames);
        }
    }

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
