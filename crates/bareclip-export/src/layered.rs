//! The full export for projects where clips are resized, moved or see-through, or show together.
//! Pictures come from every clip that shows, and sound from everything that can be heard.

use std::{
    collections::{HashMap, hash_map::Entry},
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

use bareclip_core::{
    canvas::{BLACK, Colour},
    picture::{BlackPicture, StillPicture},
    placement::{Placement, WHOLE},
    time::Ticks,
    timeline::TimeRange,
};
use bareclip_media::{MediaError, MediaInfo, Quality, probe, read_still, same_file};
use bareclip_render::{Compositor, Layer, LayerPicture, needs_compositor};

use crate::{
    ExportRate, FullPlan, FullReport,
    full::{PictureFeed, Span, frame_count, frame_time, pick_pictures_before},
    mixed::{SoundFeed, SoundPlan},
    sink::VideoSink,
    sound::sound_frame_at,
};

/// One clip showing during a stretch of the finished video.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayeredClip {
    /// Tells clips apart, so one showing across several stretches keeps its place in its video.
    pub key: u64,
    pub input: PathBuf,
    /// When the clip starts on the timeline, which can be before this stretch does.
    pub start: Ticks,
    pub source: TimeRange,
    pub placement: Placement,
}

impl LayeredClip {
    fn source_time(&self, time: Ticks) -> Ticks {
        Ticks(self.source.start.0 + time.0 - self.start.0)
    }
}

/// A stretch of the finished video over which the same clips show, lowest first. Whatever falls
/// between stretches is dead space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayeredStretch {
    pub start: Ticks,
    pub end: Ticks,
    pub stack: Vec<LayeredClip>,
}

#[derive(Clone, Debug)]
pub struct LayeredRequest {
    pub output: PathBuf,
    pub stretches: Vec<LayeredStretch>,
    /// What can be heard, which can carry on past the last picture.
    pub sound: SoundPlan,
    /// The finished video's size, or the largest video's if the project hasn't chosen one.
    pub canvas: Option<(u32, u32)>,
    /// What shows wherever no picture covers the canvas.
    pub background: Colour,
    pub quality: Quality,
    pub rate: ExportRate,
}

/// Which stretch a moment falls in. Rounding can put the very last frame just past the end by
/// up to `slack`, and that one still belongs to the last stretch.
fn stretch_at(stretches: &[LayeredStretch], time: Ticks, slack: Ticks) -> Option<usize> {
    match stretches.iter().position(|stretch| time < stretch.end) {
        Some(index) => (time >= stretches[index].start).then_some(index),
        None => {
            let last = stretches.len().checked_sub(1)?;
            (time.0 - stretches[last].end.0 < slack.0).then_some(last)
        }
    }
}

fn render_error(error: impl std::fmt::Display) -> MediaError {
    MediaError::Render(error.to_string())
}

/// The GPU, opened the first time a frame needs it.
fn opened<'a>(
    compositor: &'a mut Option<Compositor>,
    size: (u32, u32),
    background: Colour,
    report: &mut FullReport,
) -> Result<&'a mut Compositor, MediaError> {
    if compositor.is_none() {
        let mut new = Compositor::new(size.0, size.1).map_err(render_error)?;
        new.set_background(background);
        report.gpu = Some(new.gpu_name().to_owned());
        *compositor = Some(new);
    }
    compositor
        .as_mut()
        .ok_or_else(|| render_error("The GPU was not opened."))
}

/// Reads each different video that shows, in the order they first do.
fn videos_of(stretches: &[LayeredStretch]) -> Result<Vec<MediaInfo>, MediaError> {
    let mut videos: Vec<MediaInfo> = Vec::new();
    for clip in stretches.iter().flat_map(|stretch| &stretch.stack) {
        if !videos.iter().any(|info| info.path == clip.input) {
            videos.push(probe(&clip.input)?);
        }
    }
    Ok(videos)
}

/// Exports a project that takes layers.
pub fn export_layered(
    request: &LayeredRequest,
    cancel: &AtomicBool,
    progress: &dyn Fn(f32),
) -> Result<FullReport, MediaError> {
    bareclip_media::init()?;
    let started = Instant::now();
    let stretches = &request.stretches;
    if stretches.is_empty() && request.sound.spans.is_empty() {
        return Err(MediaError::NothingToExport);
    }
    let writes_over_a_source = stretches
        .iter()
        .flat_map(|stretch| &stretch.stack)
        .map(|clip| &clip.input)
        .chain(request.sound.spans.iter().map(|span| &span.path))
        .any(|input| same_file(input, &request.output));
    if writes_over_a_source {
        return Err(MediaError::SameInputAndOutput);
    }

    let videos = videos_of(stretches)?;
    let sizes: HashMap<PathBuf, (u32, u32)> = videos
        .iter()
        .map(|info| (info.path.clone(), (info.video.width, info.video.height)))
        .collect();
    let mut plan = match request.canvas {
        Some(canvas) => FullPlan::on_canvas(&videos, canvas),
        None => FullPlan::for_videos(&videos),
    };
    plan = plan.with_choices(request.quality, request.rate);
    plan.settings.sound_tracks = request.sound.tracks.clone();
    let settings = &plan.settings;
    let canvas = (settings.width, settings.height);
    let rate = settings.frame_rate;
    let length = stretches
        .last()
        .map_or(Ticks(0), |stretch| stretch.end)
        .max(request.sound.end());
    let frames = frame_count(length, rate);
    if frames == 0 {
        return Err(MediaError::NothingToExport);
    }

    let mut writer = VideoSink::create(&request.output, settings)?;
    let encoder = writer.encoder_name().to_owned();
    let mut compositor: Option<Compositor> = None;
    let black = BlackPicture::new(settings.width, settings.height);
    let mut sound = SoundFeed::new(&request.sound);

    let mut stills: HashMap<PathBuf, StillPicture> = HashMap::new();
    for clip in stretches.iter().flat_map(|stretch| &stretch.stack) {
        if clip.source.is_still()
            && let Entry::Vacant(place) = stills.entry(clip.input.clone())
        {
            place.insert(read_still(&clip.input)?);
        }
    }
    let mut feeds: HashMap<u64, PictureFeed> = HashMap::new();
    let mut current_stretch = None;
    // Whether the last frame written was dead space, or else came from the current stretch.
    let mut wrote_black = false;
    let mut wrote_anything = false;
    let mut report = FullReport {
        width: settings.width,
        height: settings.height,
        frame_rate: plan.frame_rate(),
        frames,
        composited_frames: 0,
        direct_frames: 0,
        repeated_frames: 0,
        black_frames: 0,
        encoder,
        gpu: None,
        audio_tracks: settings.sound_tracks.len(),
        elapsed: started.elapsed(),
    };
    let half_frame = Ticks(frame_time(1, rate).0 / 2);

    for number in 0..frames {
        if cancel.load(Ordering::Relaxed) {
            return Err(MediaError::Cancelled);
        }
        let time = frame_time(number, rate);

        let stretch_index = stretch_at(stretches, time, frame_time(1, rate));
        let mut stack_changed = stretch_index != current_stretch;
        if stack_changed {
            // A clip that carries on keeps its decoder, so it doesn't have to find its place again.
            let stack = stretch_index.map_or(&[][..], |index| &stretches[index].stack[..]);
            feeds.retain(|key, _| stack.iter().any(|clip| clip.key == *key));
            for clip in stack.iter().filter(|clip| !clip.source.is_still()) {
                if let Entry::Vacant(place) = feeds.entry(clip.key) {
                    let from = TimeRange {
                        start: clip.source_time(time).max(clip.source.start),
                        end: clip.source.end,
                    };
                    place.insert(PictureFeed::start(clip.input.clone(), from)?);
                }
            }
            current_stretch = stretch_index;
        }

        match stretch_index {
            Some(index) => {
                let stack = &stretches[index].stack;
                for clip in stack {
                    let Some(feed) = feeds.get_mut(&clip.key) else {
                        continue;
                    };
                    let span = Span {
                        start: clip.start,
                        end: Ticks(clip.start.0 + clip.source.duration().0),
                        source: clip.source,
                    };
                    let before = pick_pictures_before(clip.source_time(time), half_frame, span);
                    stack_changed |= feed.advance_to(before)?;
                }

                if !stack_changed && wrote_anything && !wrote_black {
                    writer.repeat_picture()?;
                    report.repeated_frames += 1;
                } else {
                    let layers: Vec<Layer> = stack
                        .iter()
                        .filter_map(|clip| {
                            let picture: LayerPicture = if clip.source.is_still() {
                                stills.get(&clip.input)?.into()
                            } else {
                                feeds.get(&clip.key)?.shown.as_ref()?.view().into()
                            };
                            let placement = clip.placement.at(Ticks(time.0 - clip.start.0));
                            let size = sizes.get(&clip.input).copied()?;
                            Some(Layer {
                                picture,
                                rect: placement.rect(size, canvas),
                                opacity: placement.opacity as f32 / WHOLE as f32,
                                look: placement.look(),
                            })
                        })
                        .collect();
                    if layers.is_empty() && request.background == BLACK {
                        writer.write_picture(&black.view())?;
                        report.black_frames += 1;
                    } else if layers.is_empty() || needs_compositor(&layers, canvas) {
                        let drawn =
                            opened(&mut compositor, canvas, request.background, &mut report)?
                                .render(&layers)
                                .map_err(render_error)?;
                        writer.write_picture(&drawn)?;
                        report.composited_frames += 1;
                    } else if let LayerPicture::Video(picture) = &layers[0].picture {
                        writer.write_picture(picture)?;
                        report.direct_frames += 1;
                    }
                }
                wrote_black = false;
            }
            None => {
                if wrote_black {
                    writer.repeat_picture()?;
                } else if request.background == BLACK {
                    writer.write_picture(&black.view())?;
                } else {
                    let drawn = opened(&mut compositor, canvas, request.background, &mut report)?
                        .render(&[])
                        .map_err(render_error)?;
                    writer.write_picture(&drawn)?;
                }
                report.black_frames += 1;
                wrote_black = true;
            }
        }
        wrote_anything = true;

        let lead = Ticks(time.0 + Ticks::from_seconds(crate::full::SOUND_LEAD_SECONDS).0);
        sound.write_until(sound_frame_at(lead.min(length)), &mut writer)?;
        if number.is_multiple_of(crate::full::PROGRESS_EVERY) {
            progress(number as f32 / frames as f32);
        }
    }

    let video_length = frame_time(frames, rate);
    sound.write_until(sound_frame_at(video_length), &mut writer)?;
    writer.finish()?;
    progress(1.0);
    report.elapsed = started.elapsed();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seconds(value: f64) -> Ticks {
        Ticks::from_seconds(value)
    }

    fn stretch(start: f64, end: f64) -> LayeredStretch {
        LayeredStretch {
            start: seconds(start),
            end: seconds(end),
            stack: Vec::new(),
        }
    }

    #[test]
    fn a_moment_is_in_its_stretch_or_in_dead_space() {
        let stretches = [stretch(1.0, 2.0), stretch(3.0, 4.0)];

        assert_eq!(stretch_at(&stretches, seconds(0.5), seconds(0.1)), None);
        assert_eq!(stretch_at(&stretches, seconds(1.0), seconds(0.1)), Some(0));
        assert_eq!(stretch_at(&stretches, seconds(2.0), seconds(0.1)), None);
        assert_eq!(stretch_at(&stretches, seconds(3.5), seconds(0.1)), Some(1));
    }

    #[test]
    fn a_frame_rounded_past_the_end_stays_with_the_last_stretch() {
        assert_eq!(
            stretch_at(&[stretch(0.0, 1.0)], seconds(1.01), seconds(0.1)),
            Some(0)
        );
    }

    #[test]
    fn a_moment_well_past_the_last_stretch_shows_nothing_because_only_sound_goes_on_there() {
        assert_eq!(
            stretch_at(&[stretch(0.0, 1.0)], seconds(2.0), seconds(0.1)),
            None
        );
        assert_eq!(stretch_at(&[], seconds(2.0), seconds(0.1)), None);
    }

    #[test]
    fn a_clip_finds_its_footage_from_anywhere_on_the_timeline() {
        let clip = LayeredClip {
            key: 1,
            input: PathBuf::from("a.mp4"),
            start: seconds(10.0),
            source: TimeRange {
                start: seconds(3.0),
                end: seconds(8.0),
            },
            placement: Placement::default(),
        };

        assert_eq!(clip.source_time(seconds(12.0)), seconds(5.0));
    }

    #[test]
    fn exporting_nothing_is_refused() {
        let request = LayeredRequest {
            output: PathBuf::from("out.mp4"),
            stretches: Vec::new(),
            sound: SoundPlan::default(),
            canvas: None,
            background: BLACK,
            quality: Quality::default(),
            rate: ExportRate::default(),
        };

        assert!(matches!(
            export_layered(&request, &AtomicBool::new(false), &|_| {}),
            Err(MediaError::NothingToExport)
        ));
    }
}
