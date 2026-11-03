//! The export that redraws every frame, at one constant rate. The GPU is only opened if some
//! frame needs it, and each frame takes the cheapest way that gets it right.

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::Instant,
};

use bareclip_core::{
    picture::BlackPicture,
    time::{TICKS_PER_SECOND, Ticks},
    timeline::TimeRange,
};
use bareclip_media::{
    DecodedPicture, ExportPart, ExportRequest, MediaError, PictureReader, same_file,
};
use bareclip_render::{Compositor, Layer, fit_inside, needs_compositor};

use crate::{
    FullPlan, FullReport,
    sink::VideoSink,
    sound::{ClipSound, SoundTracks, sound_frame_at},
};

/// Pictures decoded ahead of the one being drawn.
const PICTURES_AHEAD: usize = 6;
/// How far ahead of the picture the sound is kept, so the file stays well interleaved.
const SOUND_LEAD_SECONDS: f64 = 1.0;
const PROGRESS_EVERY: u64 = 16;

/// A stretch of the finished video that shows one part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Span {
    start: Ticks,
    end: Ticks,
    source: TimeRange,
}

/// Where each part falls in the finished video, leaving room for dead space.
fn spans_of(parts: &[ExportPart]) -> Vec<Span> {
    let mut cursor = Ticks(0);
    parts
        .iter()
        .map(|part| {
            let start = Ticks(cursor.0 + part.gap_before.0);
            let end = Ticks(start.0 + part.range.end.0 - part.range.start.0);
            cursor = end;
            Span {
                start,
                end,
                source: part.range,
            }
        })
        .collect()
}

/// When a frame of the finished video appears. Exact, so that long videos never drift.
fn frame_time(number: u64, rate: (i32, i32)) -> Ticks {
    let ticks =
        i128::from(number) * i128::from(TICKS_PER_SECOND) * i128::from(rate.1) / i128::from(rate.0);
    Ticks(ticks as i64)
}

/// How many frames fill a length of time.
fn frame_count(length: Ticks, rate: (i32, i32)) -> u64 {
    let denominator = i128::from(TICKS_PER_SECOND) * i128::from(rate.1);
    let frames = (i128::from(length.0) * i128::from(rate.0) + denominator / 2) / denominator;
    frames as u64
}

/// Which part a moment falls in, if it isn't dead space. Rounding can put the very last frame
/// just past the end, and that one still belongs to the last part.
fn span_at(spans: &[Span], time: Ticks) -> Option<usize> {
    match spans.iter().position(|span| time < span.end) {
        Some(index) => (time >= spans[index].start).then_some(index),
        None => spans.len().checked_sub(1),
    }
}

/// Up to when a frame of the finished video can take its picture from. Each frame takes the
/// picture nearest its own time, rather than the latest so far: recordings round their times,
/// and a picture landing a hair late would otherwise be skipped while the last one repeats.
/// Nothing from past the end of the part ever shows, because that footage was cut away.
fn pick_pictures_before(wanted: Ticks, half_frame: Ticks, span: Span) -> Ticks {
    Ticks(wanted.0 + half_frame.0).min(span.source.end)
}

/// Decodes a part on its own thread, so the decoder and the encoder can both run at full speed.
struct PictureFeed {
    pictures: Receiver<Result<DecodedPicture, MediaError>>,
    shown: Option<DecodedPicture>,
    coming: Option<DecodedPicture>,
}

impl PictureFeed {
    fn start(path: PathBuf, source: TimeRange) -> Result<Self, MediaError> {
        let (sender, pictures) = mpsc::sync_channel(PICTURES_AHEAD);
        thread::Builder::new()
            .name("bareclip-export-decode".to_owned())
            .spawn(move || {
                let opened = PictureReader::open(&path).and_then(|mut reader| {
                    reader.seek(source.start)?;
                    Ok(reader)
                });
                let mut reader = match opened {
                    Ok(reader) => reader,
                    Err(error) => {
                        let _ = sender.send(Err(error));
                        return;
                    }
                };
                loop {
                    match reader.next_picture() {
                        Ok(Some(picture)) => {
                            let past_the_end = picture.time() >= source.end;
                            if sender.send(Ok(picture)).is_err() || past_the_end {
                                return;
                            }
                        }
                        Ok(None) => return,
                        Err(error) => {
                            let _ = sender.send(Err(error));
                            return;
                        }
                    }
                }
            })?;
        Ok(Self {
            pictures,
            shown: None,
            coming: None,
        })
    }

    fn next(&self) -> Result<Option<DecodedPicture>, MediaError> {
        match self.pictures.recv() {
            Ok(picture) => picture.map(Some),
            Err(_) => Ok(None),
        }
    }

    /// Moves on to the last picture before a time in the video. Returns whether that is a
    /// different picture to the one before.
    fn advance_to(&mut self, before: Ticks) -> Result<bool, MediaError> {
        if self.shown.is_none() && self.coming.is_none() {
            self.coming = self.next()?;
        }
        let mut changed = false;
        // The video can start after the part does, in which case its first picture stands in.
        if self.shown.is_none() && self.coming.is_some() {
            self.shown = self.coming.take();
            self.coming = self.next()?;
            changed = true;
        }
        while self
            .coming
            .as_ref()
            .is_some_and(|next| next.time() < before)
        {
            self.shown = self.coming.take();
            self.coming = self.next()?;
            changed = true;
        }
        Ok(changed)
    }
}

/// What the finished video showed last, so that showing it again needs no work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OnScreen {
    Nothing,
    Black,
    Span(usize),
}

fn render_error(error: impl std::fmt::Display) -> MediaError {
    MediaError::Render(error.to_string())
}

pub fn export_full(
    request: &ExportRequest,
    plan: &FullPlan,
    cancel: &AtomicBool,
    progress: &dyn Fn(f32),
) -> Result<FullReport, MediaError> {
    let started = Instant::now();
    if request.parts.is_empty() {
        return Err(MediaError::NothingToExport);
    }
    if request
        .parts
        .iter()
        .any(|part| same_file(&part.input, &request.output))
    {
        return Err(MediaError::SameInputAndOutput);
    }

    let settings = &plan.settings;
    let canvas = (settings.width, settings.height);
    let rate = settings.frame_rate;
    let spans = spans_of(&request.parts);
    let length = spans.last().map_or(Ticks(0), |span| span.end);
    let frames = frame_count(length, rate);
    if frames == 0 {
        return Err(MediaError::NothingToExport);
    }

    let mut writer = VideoSink::create(&request.output, settings)?;
    let encoder = writer.encoder_name().to_owned();
    let mut compositor: Option<Compositor> = None;
    let black = BlackPicture::new(settings.width, settings.height);
    let mut sound_tracks = SoundTracks::new(settings.sound_tracks.len());

    let mut on_screen = OnScreen::Nothing;
    let mut feed: Option<PictureFeed> = None;
    let mut sound: Option<ClipSound> = None;
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
    let mut current_span = None;
    let half_frame = Ticks(frame_time(1, rate).0 / 2);

    for number in 0..frames {
        if cancel.load(Ordering::Relaxed) {
            return Err(MediaError::Cancelled);
        }
        let time = frame_time(number, rate);
        let span_index = span_at(&spans, time);

        if span_index != current_span {
            if let Some(mut finished) = sound.take() {
                finished.finish(&mut sound_tracks, &mut writer)?;
            }
            feed = None;
            if let Some(index) = span_index {
                let span = spans[index];
                let part = &request.parts[index];
                sound_tracks.pad_to(sound_frame_at(span.start), &mut writer)?;
                feed = Some(PictureFeed::start(part.input.clone(), span.source)?);
                if sound_tracks.count() > 0 {
                    sound = Some(ClipSound::open(
                        &part.input,
                        span.source.start,
                        span.start,
                        span.end,
                    )?);
                }
            }
            current_span = span_index;
        }

        match (span_index, feed.as_mut()) {
            (Some(index), Some(feed)) => {
                let span = spans[index];
                let wanted = Ticks(span.source.start.0 + (time.0 - span.start.0));
                let changed = feed.advance_to(pick_pictures_before(wanted, half_frame, span))?;
                if !changed && on_screen == OnScreen::Span(index) {
                    writer.repeat_picture()?;
                    report.repeated_frames += 1;
                } else if let Some(picture) = feed.shown.as_ref() {
                    let layer = Layer {
                        picture: picture.view(),
                        rect: fit_inside(picture.size(), canvas),
                        opacity: 1.0,
                    };
                    if needs_compositor(&[layer], canvas) {
                        if compositor.is_none() {
                            let opened = Compositor::new(settings.width, settings.height)
                                .map_err(render_error)?;
                            report.gpu = Some(opened.gpu_name().to_owned());
                            compositor = Some(opened);
                        }
                        let drawn = compositor
                            .as_mut()
                            .ok_or_else(|| render_error("The GPU was not opened."))?
                            .render(&[layer])
                            .map_err(render_error)?;
                        writer.write_picture(&drawn)?;
                        report.composited_frames += 1;
                    } else {
                        writer.write_picture(&picture.view())?;
                        report.direct_frames += 1;
                    }
                } else {
                    // The part has no pictures to show at all.
                    writer.write_picture(&black.view())?;
                    report.black_frames += 1;
                }
                on_screen = OnScreen::Span(index);
            }
            _ => {
                if on_screen == OnScreen::Black {
                    writer.repeat_picture()?;
                } else {
                    writer.write_picture(&black.view())?;
                }
                report.black_frames += 1;
                on_screen = OnScreen::Black;
            }
        }

        if let Some(sound) = sound.as_mut() {
            let lead = Ticks(time.0 + Ticks::from_seconds(SOUND_LEAD_SECONDS).0);
            sound.write_until(sound_frame_at(lead), &mut sound_tracks, &mut writer)?;
        }
        if number.is_multiple_of(PROGRESS_EVERY) {
            progress(number as f32 / frames as f32);
        }
    }

    if let Some(mut finished) = sound.take() {
        finished.finish(&mut sound_tracks, &mut writer)?;
    }
    let video_length = frame_time(frames, rate);
    sound_tracks.pad_to(sound_frame_at(video_length), &mut writer)?;
    writer.finish()?;
    progress(1.0);
    report.elapsed = started.elapsed();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use bareclip_core::splice::FrameIndex;

    fn seconds(value: f64) -> Ticks {
        Ticks::from_seconds(value)
    }

    fn part(start: f64, end: f64, gap: f64) -> ExportPart {
        ExportPart {
            input: PathBuf::from("a.mp4"),
            frames: Arc::new(FrameIndex::new(Vec::new())),
            range: TimeRange {
                start: seconds(start),
                end: seconds(end),
            },
            gap_before: seconds(gap),
        }
    }

    #[test]
    fn a_frame_can_take_a_picture_landing_just_after_its_time() {
        let span = spans_of(&[part(0.0, 10.0, 0.0)])[0];

        assert_eq!(
            pick_pictures_before(seconds(1.0), seconds(0.008), span),
            seconds(1.008)
        );
    }

    #[test]
    fn a_frame_never_takes_a_picture_from_past_the_cut() {
        let span = spans_of(&[part(0.0, 1.0, 0.0)])[0];

        assert_eq!(
            pick_pictures_before(seconds(0.995), seconds(0.008), span),
            seconds(1.0)
        );
    }

    #[test]
    fn parts_follow_each_other_leaving_room_for_dead_space() {
        let spans = spans_of(&[
            part(5.0, 7.0, 1.0),
            part(0.0, 1.0, 0.0),
            part(2.0, 3.0, 0.5),
        ]);

        assert_eq!(spans[0].start, seconds(1.0));
        assert_eq!(spans[0].end, seconds(3.0));
        assert_eq!(spans[1].start, seconds(3.0));
        assert_eq!(spans[2].start, seconds(4.5));
        assert_eq!(spans[2].end, seconds(5.5));
    }

    #[test]
    fn frame_times_stay_exact_at_fractional_rates_over_a_long_video() {
        let ntsc = (30000, 1001);
        let an_hour = 30000 * 3600 / 1001;

        let time = frame_time(an_hour * 1001, ntsc);

        assert_eq!(time, seconds(an_hour as f64 * 1001.0 * 1001.0 / 30000.0));
    }

    #[test]
    fn a_length_fills_a_whole_number_of_frames() {
        assert_eq!(frame_count(seconds(2.0), (60, 1)), 120);
        assert_eq!(frame_count(seconds(1.0), (60000, 1001)), 60);
        assert_eq!(frame_count(Ticks(0), (60, 1)), 0);
    }

    #[test]
    fn a_length_rounds_to_the_nearest_frame() {
        assert_eq!(frame_count(seconds(0.024), (30, 1)), 1);
        assert_eq!(frame_count(seconds(0.01), (30, 1)), 0);
    }

    #[test]
    fn a_moment_is_in_its_part_or_in_dead_space() {
        let spans = spans_of(&[part(0.0, 1.0, 1.0), part(0.0, 1.0, 1.0)]);

        assert_eq!(span_at(&spans, seconds(0.5)), None, "dead space first");
        assert_eq!(span_at(&spans, seconds(1.0)), Some(0));
        assert_eq!(span_at(&spans, seconds(2.5)), None, "dead space between");
        assert_eq!(span_at(&spans, seconds(3.5)), Some(1));
    }

    #[test]
    fn the_moment_a_part_ends_belongs_to_what_comes_next() {
        let spans = spans_of(&[part(0.0, 1.0, 0.0), part(0.0, 1.0, 0.0)]);

        assert_eq!(span_at(&spans, seconds(1.0)), Some(1));
    }

    #[test]
    fn a_frame_rounded_past_the_end_stays_with_the_last_part() {
        let spans = spans_of(&[part(0.0, 1.0, 0.0)]);

        assert_eq!(span_at(&spans, seconds(1.01)), Some(0));
    }
}
