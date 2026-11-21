//! One picture of a video at its full size, for saving as a PNG.

use std::path::Path;

use bareclip_core::time::Ticks;
use ffmpeg_next as ffmpeg;
use ffmpeg_next::{Packet, Rational, codec, encoder, ffi, format::Pixel, frame, software::scaling};

use crate::{
    error::MediaError,
    full::{DecodedPicture, PictureReader, coefficients_of},
};

/// A picture as rows of red, green and blue, with nothing between rows.
pub struct FullPicture {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// The picture a video shows at a moment, the latest that starts at or before it, as the
/// player shows it, at the video's full size and the right way up.
pub fn picture_at(path: &Path, time: Ticks) -> Result<FullPicture, MediaError> {
    let mut reader = PictureReader::open(path)?;
    reader.seek(time)?;
    let mut chosen: Option<DecodedPicture> = None;
    while let Some(picture) = reader.next_picture()? {
        let past = picture.time() > time;
        if past && chosen.is_some() {
            break;
        }
        chosen = Some(picture);
        if past {
            break;
        }
    }
    to_rgb(&chosen.ok_or(MediaError::NoFrames)?)
}

/// Turns a decoded picture into red, green and blue, by the colours its video says it uses.
fn to_rgb(picture: &DecodedPicture) -> Result<FullPicture, MediaError> {
    let decoded = picture.frame();
    let (width, height) = (decoded.width(), decoded.height());
    let mut context = scaling::Context::get(
        decoded.format(),
        width,
        height,
        Pixel::RGB24,
        width,
        height,
        scaling::Flags::BICUBIC | scaling::Flags::ACCURATE_RND | scaling::Flags::FULL_CHR_H_INT,
    )?;
    let color = picture.color();
    let table = unsafe { ffi::sws_getCoefficients(coefficients_of(color.matrix)) };
    // SAFETY: the context is live, and this only copies the constant tables' values into it.
    unsafe {
        ffi::sws_setColorspaceDetails(
            context.as_mut_ptr(),
            table,
            i32::from(color.full_range),
            table,
            1,
            0,
            1 << 16,
            1 << 16,
        );
    }
    let mut converted = frame::Video::empty();
    context.run(decoded, &mut converted)?;
    let row = width as usize * 3;
    let stride = converted.stride(0);
    let mut rgb = Vec::with_capacity(row * height as usize);
    for line in converted.data(0).chunks(stride).take(height as usize) {
        rgb.extend_from_slice(&line[..row]);
    }
    Ok(FullPicture { width, height, rgb })
}

/// Writes a picture as a PNG, which keeps every pixel as it is.
pub fn write_png(path: &Path, picture: &FullPicture) -> Result<(), MediaError> {
    crate::init()?;
    let codec = encoder::find(codec::Id::PNG).ok_or(ffmpeg::Error::EncoderNotFound)?;
    let mut context = codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()?;
    context.set_width(picture.width);
    context.set_height(picture.height);
    context.set_format(Pixel::RGB24);
    context.set_time_base(Rational(1, 1));
    let mut encoder = context.open_as(codec)?;

    let mut frame = frame::Video::new(Pixel::RGB24, picture.width, picture.height);
    let row = picture.width as usize * 3;
    let stride = frame.stride(0);
    for (to, from) in frame
        .data_mut(0)
        .chunks_mut(stride)
        .zip(picture.rgb.chunks(row))
    {
        to[..row].copy_from_slice(from);
    }
    frame.set_pts(Some(0));
    encoder.send_frame(&frame)?;
    encoder.send_eof()?;
    let mut packet = Packet::empty();
    encoder.receive_packet(&mut packet)?;
    let data = packet.data().ok_or(MediaError::NoFrames)?;
    std::fs::write(path, data)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// A 2 second 320x180 video at 30 fps whose frames count up in binary across it.
    fn numbered_video() -> std::path::PathBuf {
        let folder = std::env::temp_dir().join("bareclip-picture-tests");
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join(format!("numbered-{}.mp4", std::process::id()));
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
            ])
            .arg(
                "nullsrc=s=320x180:r=30:d=2,\
                 geq=lum='255*mod(floor(N/pow(2,floor(X/20))),2)':cb=128:cr=128,format=yuv420p",
            )
            .args(["-c:v", "libopenh264", "-b:v", "4M", "-g", "15"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        path
    }

    /// Grey levels of one frame of a file, decoded by FFmpeg itself.
    fn ffmpeg_grey(path: &Path, select: &str) -> Vec<u8> {
        let output = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-i"])
            .arg(path)
            .args(["-vf", &format!("{select}format=gray"), "-frames:v", "1"])
            .args(["-f", "rawvideo", "-"])
            .output()
            .unwrap();
        assert!(output.status.success());
        output.stdout
    }

    #[test]
    fn the_saved_picture_is_the_frame_at_that_moment_at_full_size() {
        let video = numbered_video();
        let folder = video.parent().unwrap().to_owned();
        for frame_number in [0_u32, 17, 44] {
            // Partway through the frame, which still shows it.
            let time = Ticks::from_seconds((f64::from(frame_number) + 0.5) / 30.0);
            let picture = picture_at(&video, time).unwrap();
            assert_eq!((picture.width, picture.height), (320, 180));
            let png = folder.join(format!("frame-{frame_number}-{}.png", std::process::id()));
            write_png(&png, &picture).unwrap();

            let saved = ffmpeg_grey(&png, "");
            let wanted = ffmpeg_grey(&video, &format!("select=eq(n\\,{frame_number}),"));
            assert_eq!(saved.len(), wanted.len());
            let worst = saved
                .iter()
                .zip(&wanted)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            // Bars only change every 20 pixels, so any other frame differs by far more.
            assert!(worst <= 24, "frame {frame_number} differs by {worst}");
            let _ = std::fs::remove_file(png);
        }
    }
}
