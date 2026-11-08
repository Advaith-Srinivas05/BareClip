//! Reads what clips show on the timeline: small pictures along them and the shape of their
//! sound. Both run in the background once, so this favours steady progress over speed.

use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use bareclip_core::time::Ticks;
use ffmpeg_next as ffmpeg;
use ffmpeg_next::{Discard, Packet, codec, format, frame, media::Type, software::scaling};

use crate::{
    error::MediaError,
    preview::{AudioPreview, AudioStep, PREVIEW_CHANNELS, fit_preview_size},
    probe::{discard_other_streams, to_time_base},
    turn::Orientation,
};

/// One small picture from a video. These come from keyframes, the only frames that can be
/// decoded on their own, so `time` is where the picture really is, not where it was asked for.
#[derive(Clone, Debug)]
pub struct StripPicture {
    pub time: Ticks,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// How often to take a picture from a video of this length. The cap keeps very long
/// recordings from holding hundreds of megabytes of pictures.
pub fn picture_spacing(duration: Ticks, shortest: Ticks, most: u32) -> Ticks {
    Ticks((duration.0 / i64::from(most.max(1))).max(shortest.0).max(1))
}

/// Calls `found` with pictures spread along a video, from its start, until it reaches
/// `duration` or `cancel` is set. Pictures can be further apart than asked if the keyframes
/// are. It goes by the length it is told rather than by reading to the end of the file,
/// because seeking past the last keyframe just lands on that keyframe again.
pub fn read_strip_pictures(
    path: &Path,
    duration: Ticks,
    spacing: Ticks,
    (max_width, max_height): (u32, u32),
    cancel: &AtomicBool,
    mut found: impl FnMut(StripPicture),
) -> Result<(), MediaError> {
    crate::init()?;
    let mut input = format::input(path)?;
    let stream = input
        .streams()
        .best(Type::Video)
        .ok_or(MediaError::NoVideoStream)?;
    let index = stream.index();
    let time_base = to_time_base(stream.time_base());
    let orientation = Orientation::of_stream(&stream);
    let context = codec::context::Context::from_parameters(stream.parameters())?;
    let mut decoder = context.decoder().video()?;
    decoder.set_packet_time_base(stream.time_base());
    // Only keyframes get decoded at all, which is what makes this cheap.
    decoder.skip_frame(Discard::NonKey);
    discard_other_streams(&mut input, &[index]);
    // Pictures are made at the stored shape and then turned the right way up.
    let (upright_width, upright_height) =
        orientation.turned_size(decoder.width(), decoder.height());
    let (shown_width, shown_height) =
        fit_preview_size(upright_width, upright_height, max_width, max_height);
    let (width, height) = orientation.turned_size(shown_width, shown_height);
    let mut scaler = None;
    let mut decoded = frame::Video::empty();
    let mut packet = Packet::empty();

    let mut wanted = Ticks(0);
    let mut last_time = None;
    while wanted < duration && !cancel.load(Ordering::Relaxed) {
        let target = (wanted.as_seconds() * f64::from(ffmpeg::ffi::AV_TIME_BASE)) as i64;
        if input.seek(target, ..target).is_err() && wanted > Ticks(0) {
            return Ok(());
        }
        decoder.flush();

        // Reads until a picture comes out or the file ends.
        let picture_time = loop {
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            if decoder.receive_frame(&mut decoded).is_ok() {
                let pts = decoded.timestamp().unwrap_or_default();
                break Some(Ticks::from_pts(pts, time_base));
            }
            match packet.read(&mut input) {
                Ok(()) if packet.stream() == index => {
                    // A damaged packet should only cost one picture.
                    let _ = decoder.send_packet(&packet);
                }
                Ok(()) => {}
                Err(ffmpeg::Error::Eof) => break None,
                // Other read errors are transient or mean the rest of the file is unusable,
                // and neither is worth more than losing the remaining pictures.
                Err(_) => break None,
            }
        };
        let Some(time) = picture_time else {
            return Ok(());
        };
        // A long keyframe interval lands several requests on one picture.
        if last_time.is_none_or(|last| time > last) {
            let pixel = decoded.format();
            let (from_width, from_height) = (decoded.width(), decoded.height());
            let scaler = match &mut scaler {
                Some(scaler) => scaler,
                None => scaler.insert(scaling::Context::get(
                    pixel,
                    from_width,
                    from_height,
                    ffmpeg::format::Pixel::RGBA,
                    width,
                    height,
                    scaling::Flags::BILINEAR,
                )?),
            };
            let mut rgba_frame = frame::Video::empty();
            scaler.run(&decoded, &mut rgba_frame)?;
            found(StripPicture {
                time,
                width: shown_width,
                height: shown_height,
                rgba: orientation.turn(tightly_packed(&rgba_frame, width, height), width, height),
            });
            last_time = Some(time);
        }
        wanted = Ticks(time.0.max(wanted.0) + spacing.0);
    }
    Ok(())
}

/// A frame's pixels with the padding FFmpeg leaves at the end of each row removed.
fn tightly_packed(frame: &frame::Video, width: u32, height: u32) -> Vec<u8> {
    let stride = frame.stride(0);
    let row = width as usize * 4;
    let data = frame.data(0);
    let mut rgba = Vec::with_capacity(row * height as usize);
    for y in 0..height as usize {
        rgba.extend_from_slice(&data[y * stride..y * stride + row]);
    }
    rgba
}

/// How loud each short stretch of a video's sound looks, from 0 to 255. This is only for
/// showing its shape, so it is relative to the video's own loudest moment, not a level.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SoundShape {
    pub peaks_per_second: u32,
    pub peaks: Vec<u8>,
}

impl SoundShape {
    /// The loudest peak between two moments in the video, from 0 to 1.
    pub fn peak_between(&self, start: Ticks, end: Ticks) -> f32 {
        if self.peaks.is_empty() {
            return 0.0;
        }
        let at = |time: Ticks| (time.as_seconds() * f64::from(self.peaks_per_second)) as i64;
        let last = self.peaks.len() as i64 - 1;
        let first = at(start).clamp(0, last) as usize;
        let final_peak = at(end).clamp(first as i64, last) as usize;
        let loudest = self.peaks[first..=final_peak].iter().copied().max();
        f32::from(loudest.unwrap_or(0)) / 255.0
    }
}

/// The sample rate sound is read at. It only has to show a shape, so it doesn't have to be
/// anything like what plays.
const SHAPE_SAMPLE_RATE: u32 = 8000;
const PEAKS_PER_SECOND: u32 = 50;
/// Anything no louder than this is silence with a little noise, not sound worth scaling up.
const SILENCE_PEAK: u8 = 2;

/// Scales peaks so a video's loudest moment fills the height, along a curve that keeps quiet
/// parts visible. On a straight scale, ordinary speech and game sound look nearly flat.
fn shaped_for_showing(peaks: &mut [u8]) {
    let loudest = peaks.iter().copied().max().unwrap_or(0);
    if loudest <= SILENCE_PEAK {
        return;
    }
    for peak in peaks {
        let share = f32::from(*peak) / f32::from(loudest);
        *peak = (share.sqrt() * 255.0) as u8;
    }
}

/// Reads the shape of a video's sound, with every audio track counted together. A video with
/// no sound has an empty shape. Returns `None` if `cancel` was set.
pub fn read_sound_shape(
    path: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(Ticks),
) -> Result<Option<SoundShape>, MediaError> {
    let mut audio = AudioPreview::open(path, SHAPE_SAMPLE_RATE)?;
    let channels_frames_per_peak = i64::from(SHAPE_SAMPLE_RATE / PEAKS_PER_SECOND);
    let mut peaks: Vec<u8> = Vec::new();
    if audio.track_count() == 0 {
        return Ok(Some(SoundShape::default()));
    }
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        match audio.next_step()? {
            AudioStep::Chunk(chunk) => {
                for (position, frame) in chunk.samples.chunks(PREVIEW_CHANNELS).enumerate() {
                    let at = chunk.start_frame + position as i64;
                    let Ok(peak) = usize::try_from(at / channels_frames_per_peak) else {
                        continue;
                    };
                    if peak >= peaks.len() {
                        peaks.resize(peak + 1, 0);
                    }
                    let loudest = frame.iter().fold(0.0_f32, |most, s| most.max(s.abs()));
                    let level = (loudest.min(1.0) * 255.0) as u8;
                    peaks[peak] = peaks[peak].max(level);
                }
                let seconds = peaks.len() as f64 / f64::from(PEAKS_PER_SECOND);
                progress(Ticks::from_seconds(seconds));
            }
            AudioStep::Pending => {}
            AudioStep::End => {
                shaped_for_showing(&mut peaks);
                return Ok(Some(SoundShape {
                    peaks_per_second: PEAKS_PER_SECOND,
                    peaks,
                }));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(peaks: &[u8]) -> SoundShape {
        SoundShape {
            peaks_per_second: 10,
            peaks: peaks.to_vec(),
        }
    }

    #[test]
    fn pictures_are_spaced_by_length_up_to_the_most_allowed() {
        let shortest = Ticks::from_seconds(2.0);

        assert_eq!(
            picture_spacing(Ticks::from_seconds(10.0), shortest, 100),
            shortest
        );
        assert_eq!(
            picture_spacing(Ticks::from_seconds(3600.0), shortest, 100),
            Ticks::from_seconds(36.0)
        );
    }

    #[test]
    fn a_stretch_of_sound_reports_its_loudest_peak() {
        let shape = shape(&[0, 51, 255, 102, 0]);

        assert_eq!(
            shape.peak_between(Ticks::from_seconds(0.0), Ticks::from_seconds(0.1)),
            0.2
        );
        assert_eq!(
            shape.peak_between(Ticks::from_seconds(0.1), Ticks::from_seconds(0.3)),
            1.0
        );
    }

    #[test]
    fn asking_past_either_end_of_the_sound_stays_inside_it() {
        let shape = shape(&[51, 102]);

        assert_eq!(
            shape.peak_between(Ticks::from_seconds(9.0), Ticks::from_seconds(10.0)),
            0.4
        );
        assert_eq!(
            shape.peak_between(Ticks::from_seconds(-1.0), Ticks::from_seconds(0.0)),
            0.2
        );
    }

    #[test]
    fn a_quiet_video_still_fills_the_height_with_quiet_parts_kept_visible() {
        let mut peaks = [32, 8, 0];

        shaped_for_showing(&mut peaks);

        assert_eq!(peaks[0], 255);
        assert_eq!(
            peaks[1], 127,
            "a quarter of the loudest shows at half height"
        );
        assert_eq!(peaks[2], 0);
    }

    #[test]
    fn near_silence_is_not_scaled_up_into_a_shape() {
        let mut peaks = [2, 1, 0];

        shaped_for_showing(&mut peaks);

        assert_eq!(peaks, [2, 1, 0]);
    }

    #[test]
    fn no_sound_has_no_peaks() {
        let silent = SoundShape::default();

        assert_eq!(silent.peak_between(Ticks(0), Ticks::from_seconds(5.0)), 0.0);
    }
}
