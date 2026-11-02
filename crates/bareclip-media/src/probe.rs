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

/// Why two videos cannot be joined by copying, in words for the user. They must share
/// a picture format and matching audio tracks, or a player could not read the result.
pub fn join_problem(first: &MediaInfo, other: &MediaInfo) -> Option<String> {
    let name = |info: &MediaInfo| {
        info.path.file_name().map_or_else(
            || "A video".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        )
    };
    let reason = if first.video.codec != other.video.codec {
        format!(
            "uses {} video instead of {}",
            other.video.codec, first.video.codec
        )
    } else if (first.video.width, first.video.height) != (other.video.width, other.video.height) {
        format!(
            "is {} x {} instead of {} x {}",
            other.video.width, other.video.height, first.video.width, first.video.height
        )
    } else if first.video.pixel_format != other.video.pixel_format {
        "uses a different colour format".to_owned()
    } else if first.audio.len() != other.audio.len() {
        format!(
            "has {} audio tracks instead of {}",
            other.audio.len(),
            first.audio.len()
        )
    } else if first.audio.iter().zip(&other.audio).any(|(a, b)| {
        a.codec != b.codec || a.channels != b.channels || a.sample_rate != b.sample_rate
    }) {
        "has audio in a different format".to_owned()
    } else {
        return None;
    };
    Some(format!(
        "{} {reason}, so it can't be exported together with {} yet.",
        name(other),
        name(first)
    ))
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
        if frames.len().is_multiple_of(256) {
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

    fn info(name: &str) -> MediaInfo {
        MediaInfo {
            path: PathBuf::from(name),
            container: "matroska,webm".to_owned(),
            size_bytes: 1,
            duration: Ticks(1),
            video: VideoInfo {
                stream_index: 0,
                codec: "h264".to_owned(),
                width: 2560,
                height: 1440,
                pixel_format: "YUV420P".to_owned(),
                time_base: TimeBase { num: 1, den: 1000 },
                frame_rate: 60.0,
                is_hdr: false,
            },
            audio: vec![AudioInfo {
                stream_index: 1,
                codec: "aac".to_owned(),
                channels: 2,
                sample_rate: 48_000,
                title: None,
                language: None,
            }],
        }
    }

    #[test]
    fn videos_in_the_same_format_can_join() {
        let mut other = info("b.mkv");
        other.video.frame_rate = 30.0;
        other.video.time_base = TimeBase {
            num: 1,
            den: 90_000,
        };

        assert_eq!(join_problem(&info("a.mkv"), &other), None);
    }

    #[test]
    fn differences_that_break_a_join_are_explained() {
        let first = info("a.mkv");
        let check = |change: fn(&mut MediaInfo), wanted: &str| {
            let mut other = info("b.mkv");
            change(&mut other);
            let problem = join_problem(&first, &other).expect("these cannot join");
            assert!(problem.starts_with("b.mkv"), "{problem}");
            assert!(problem.contains(wanted), "{problem}");
        };

        check(|o| o.video.codec = "hevc".to_owned(), "hevc");
        check(|o| o.video.width = 1920, "1920 x 1440");
        check(
            |o| o.video.pixel_format = "YUV420P10LE".to_owned(),
            "colour format",
        );
        check(|o| o.audio.clear(), "0 audio tracks");
        check(
            |o| o.audio[0].sample_rate = 44_100,
            "audio in a different format",
        );
        check(
            |o| o.audio[0].codec = "opus".to_owned(),
            "audio in a different format",
        );
        check(|o| o.audio[0].channels = 1, "audio in a different format");
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
