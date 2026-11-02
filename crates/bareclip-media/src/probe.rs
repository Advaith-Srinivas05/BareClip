//! Reads a media file's details and its table of video frames.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use bareclip_core::{
    splice::{FrameEntry, FrameIndex},
    time::{Ticks, TimeBase},
};
use ffmpeg_next as ffmpeg;
use ffmpeg_next::{
    color::TransferCharacteristic, ffi, format::context::Input, media::Type,
    util::rational::Rational,
};

use crate::error::MediaError;

#[derive(Clone, Debug, PartialEq)]
pub struct VideoInfo {
    pub stream_index: usize,
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub pixel_format: String,
    pub time_base: TimeBase,
    /// Average frames per second.
    pub frame_rate: f64,
    pub is_hdr: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioInfo {
    pub stream_index: usize,
    pub codec: String,
    pub channels: u16,
    pub sample_rate: u32,
    pub title: Option<String>,
    pub language: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MediaInfo {
    pub path: PathBuf,
    pub container: String,
    pub size_bytes: u64,
    pub duration: Ticks,
    pub video: VideoInfo,
    pub audio: Vec<AudioInfo>,
}

pub(crate) fn to_time_base(rational: Rational) -> TimeBase {
    TimeBase {
        num: rational.numerator(),
        den: rational.denominator(),
    }
}

pub fn probe(path: &Path) -> Result<MediaInfo, MediaError> {
    crate::init()?;
    let size_bytes = std::fs::metadata(path)?.len();
    let input = ffmpeg::format::input(path)?;

    let video_stream = input
        .streams()
        .best(Type::Video)
        .ok_or(MediaError::NoVideoStream)?;
    let context = ffmpeg::codec::context::Context::from_parameters(video_stream.parameters())?;
    let codec = context.id();
    let decoder = context.decoder().video()?;
    let rate = video_stream.avg_frame_rate();
    let video = VideoInfo {
        stream_index: video_stream.index(),
        codec: codec.name().to_owned(),
        width: decoder.width(),
        height: decoder.height(),
        pixel_format: format!("{:?}", decoder.format()),
        time_base: to_time_base(video_stream.time_base()),
        frame_rate: if rate.denominator() == 0 {
            0.0
        } else {
            f64::from(rate.numerator()) / f64::from(rate.denominator())
        },
        is_hdr: matches!(
            decoder.color_transfer_characteristic(),
            TransferCharacteristic::SMPTE2084 | TransferCharacteristic::ARIB_STD_B67
        ),
    };

    let mut audio = Vec::new();
    for stream in input.streams() {
        if stream.parameters().medium() != Type::Audio {
            continue;
        }
        let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
        let codec = context.id();
        let decoder = context.decoder().audio()?;
        let metadata = stream.metadata();
        audio.push(AudioInfo {
            stream_index: stream.index(),
            codec: codec.name().to_owned(),
            channels: decoder.channels(),
            sample_rate: decoder.rate(),
            title: metadata.get("title").map(str::to_owned),
            language: metadata.get("language").map(str::to_owned),
        });
    }

    Ok(MediaInfo {
        path: path.to_owned(),
        container: input.format().name().to_owned(),
        size_bytes,
        duration: Ticks::from_pts(
            input.duration().max(0),
            TimeBase {
                num: 1,
                den: ffi::AV_TIME_BASE,
            },
        ),
        video,
        audio,
    })
}

/// Tells the demuxer to skip every stream except the ones to keep.
pub(crate) fn discard_other_streams(input: &mut Input, keep: &[usize]) {
    // SAFETY: the pointers come from a live input context, and the loop stays
    // within its stream count. `discard` is a plain field read by the demuxer.
    unsafe {
        let context = input.as_mut_ptr();
        for index in 0..(*context).nb_streams as usize {
            let stream = *(*context).streams.add(index);
            (*stream).discard = if keep.contains(&index) {
                ffi::AVDiscard::AVDISCARD_DEFAULT
            } else {
                ffi::AVDiscard::AVDISCARD_ALL
            };
        }
    }
}

/// Lists every frame in the video stream without decoding any of them.
/// `progress` receives the fraction of the file read, from 0 to 1.
pub fn scan_frames(
    path: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(f32),
) -> Result<FrameIndex, MediaError> {
    crate::init()?;
    let mut input = ffmpeg::format::input(path)?;
    let video_index = input
        .streams()
        .best(Type::Video)
        .ok_or(MediaError::NoVideoStream)?
        .index();
    let duration = input.duration().max(1);
    let time_base = input
        .stream(video_index)
        .ok_or(MediaError::NoVideoStream)?
        .time_base();
    discard_other_streams(&mut input, &[video_index]);

    let mut frames: Vec<FrameEntry> = Vec::new();
    for (stream, packet) in input.packets() {
        if stream.index() != video_index {
            continue;
        }
        let Some(pts) = packet.pts() else {
            continue;
        };
        frames.push(FrameEntry {
            pts,
            dts: packet.dts().unwrap_or(pts),
            duration: packet.duration(),
            keyframe: packet.is_key(),
        });
        if frames.len() % 256 == 0 {
            if cancel.load(Ordering::Relaxed) {
                return Err(MediaError::Cancelled);
            }
            let seconds = f64::from(time_base) * pts as f64;
            progress(
                (seconds * f64::from(ffi::AV_TIME_BASE) / duration as f64).clamp(0.0, 1.0) as f32,
            );
        }
    }
    if frames.is_empty() {
        return Err(MediaError::NoFrames);
    }

    fill_missing_durations(&mut frames);
    Ok(FrameIndex::new(frames))
}

/// Some containers leave frame durations out. The gap to the next shown frame is
/// what actually matters, so it replaces any missing value.
fn fill_missing_durations(frames: &mut [FrameEntry]) {
    let mut order: Vec<usize> = (0..frames.len()).collect();
    order.sort_by_key(|&index| frames[index].pts);

    let mut previous_gap = 1;
    for position in 0..order.len() {
        let current = order[position];
        let gap = order
            .get(position + 1)
            .map(|&next| frames[next].pts - frames[current].pts);
        if frames[current].duration <= 0 {
            frames[current].duration = gap.filter(|gap| *gap > 0).unwrap_or(previous_gap);
        }
        previous_gap = gap.filter(|gap| *gap > 0).unwrap_or(previous_gap);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(pts: i64, duration: i64) -> FrameEntry {
        FrameEntry {
            pts,
            dts: pts,
            duration,
            keyframe: false,
        }
    }

    #[test]
    fn missing_durations_come_from_the_next_shown_frame() {
        let mut frames = [entry(0, 0), entry(40, 0), entry(10, 0), entry(90, 0)];

        fill_missing_durations(&mut frames);

        assert_eq!(frames[0].duration, 10);
        assert_eq!(frames[2].duration, 30);
        assert_eq!(frames[1].duration, 50);
        assert_eq!(frames[3].duration, 50);
    }

    #[test]
    fn durations_already_present_are_kept() {
        let mut frames = [entry(0, 7), entry(10, 3)];

        fill_missing_durations(&mut frames);

        assert_eq!(frames[0].duration, 7);
        assert_eq!(frames[1].duration, 3);
    }
}
