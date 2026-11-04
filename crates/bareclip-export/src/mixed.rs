//! Exports where the sound has changed. The picture can often still be copied, so only the sound is
//! worked out again.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use bareclip_audio::{Bus, MixSpan, TimelineMix};
use bareclip_core::{sound::ClipSound, splice::CutMode, time::Ticks};
use bareclip_media::{
    ExportPart, ExportReport, ExportRequest, MediaError, MediaInfo, SOUND_CHANNELS, SOUND_RATE,
    SoundFile, SoundLabel, export_picture_only, join_picture_and_sound, same_file,
};

use crate::{plan::sound_labels, sink::VideoSink};

/// How much sound is worked out at a time, in audio frames.
const BLOCK_FRAMES: usize = 4096;
/// How much of the time of an export with new sound goes on the picture, for the progress bar.
const PICTURE_SHARE: f32 = 0.8;

/// What a finished video's sound comes from, and the tracks it goes on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SoundPlan {
    pub spans: Vec<MixSpan>,
    pub tracks: Vec<SoundLabel>,
}

impl SoundPlan {
    /// The sound of each part just as it was recorded, one after another, with each of its tracks
    /// kept apart.
    pub fn as_recorded(parts: &[ExportPart], videos: &[MediaInfo]) -> Self {
        let mut reached = Ticks(0);
        let spans = parts
            .iter()
            .map(|part| {
                let start = Ticks(reached.0 + part.gap_before.0);
                reached = Ticks(start.0 + part.range.duration().0);
                MixSpan {
                    path: part.input.clone(),
                    start,
                    source: part.range,
                    sound: ClipSound::default(),
                    bus: Bus::ByTrack(0),
                }
            })
            .collect();
        Self {
            spans,
            tracks: sound_labels(videos),
        }
    }

    /// Where the last of the sound ends.
    pub fn end(&self) -> Ticks {
        self.spans
            .iter()
            .map(|span| Ticks(span.start.0 + span.source.duration().0))
            .max()
            .unwrap_or(Ticks(0))
    }

    fn buses(&self) -> Vec<Vec<f32>> {
        vec![Vec::new(); self.tracks.len()]
    }
}

/// Adds the plan's sound to a video being written, up to a moment. Called as pictures go in, so the
/// two stay woven together.
pub struct SoundFeed {
    mix: TimelineMix,
    buses: Vec<Vec<f32>>,
    written: i64,
}

impl SoundFeed {
    pub fn new(plan: &SoundPlan) -> Self {
        Self {
            mix: TimelineMix::new(plan.spans.clone(), SOUND_RATE, Ticks(0)),
            buses: plan.buses(),
            written: 0,
        }
    }

    pub fn write_until(&mut self, frame: i64, writer: &mut VideoSink) -> Result<(), MediaError> {
        while self.written < frame {
            let now = BLOCK_FRAMES.min((frame - self.written) as usize);
            self.mix.fill(now, &mut self.buses);
            for (track, bus) in self.buses.iter_mut().enumerate() {
                writer.write_sound(track, bus)?;
                bus.clear();
            }
            self.written += now as i64;
        }
        Ok(())
    }
}

/// A temporary file next to the finished one. It stays there so it can be moved rather than copied.
fn working_file(output: &Path, tail: &str) -> PathBuf {
    let name = output
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    output.with_file_name(format!(".{name}.{tail}"))
}

/// Removes a working file however the export ends.
struct Cleanup(Vec<PathBuf>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn write_sound_file(
    plan: &SoundPlan,
    path: &Path,
    length: Ticks,
    cancel: &AtomicBool,
    progress: &dyn Fn(f32),
) -> Result<(), MediaError> {
    let mut file = SoundFile::create(path, &plan.tracks)?;
    let mut mix = TimelineMix::new(plan.spans.clone(), SOUND_RATE, Ticks(0));
    let mut buses = plan.buses();
    let total = (length.as_seconds() * f64::from(SOUND_RATE)).round() as i64;
    let mut written = 0;
    while written < total {
        if cancel.load(Ordering::Relaxed) {
            return Err(MediaError::Cancelled);
        }
        let now = BLOCK_FRAMES.min((total - written) as usize);
        mix.fill(now, &mut buses);
        for (track, bus) in buses.iter_mut().enumerate() {
            debug_assert_eq!(bus.len(), now * SOUND_CHANNELS);
            file.write(track, bus)?;
            bus.clear();
        }
        written += now as i64;
        progress(written as f32 / total as f32);
    }
    file.finish()
}

/// Copies the picture and adds newly worked out sound, which takes far less time than redrawing.
/// Cuts are always exact, because the sound follows the timeline exactly, and a picture that moved
/// its cuts out to keyframes would run longer than it and drift out of sync.
pub fn export_with_new_sound(
    request: &ExportRequest,
    plan: &SoundPlan,
    cancel: &AtomicBool,
    progress: &dyn Fn(f32),
) -> Result<ExportReport, MediaError> {
    bareclip_media::init()?;
    let overwrites = plan
        .spans
        .iter()
        .map(|span| &span.path)
        .chain(request.parts.iter().map(|part| &part.input))
        .any(|input| same_file(input, &request.output));
    if overwrites {
        return Err(MediaError::SameInputAndOutput);
    }
    // Keeping the finished file's extension makes the picture's working file the right format.
    let extension = request.output.extension().map_or_else(
        || "mp4".to_owned(),
        |ext| ext.to_string_lossy().into_owned(),
    );
    let picture = working_file(&request.output, &format!("picture.{extension}"));
    let sound = working_file(&request.output, "sound.m4a");
    let _cleanup = Cleanup(vec![picture.clone(), sound.clone()]);

    let picture_request = ExportRequest {
        output: picture.clone(),
        mode: CutMode::FrameExact,
        ..request.clone()
    };
    let mut report = export_picture_only(&picture_request, cancel, &|fraction| {
        progress(fraction * PICTURE_SHARE);
    })?;
    if report.mode_used != CutMode::FrameExact {
        return Err(MediaError::GapNeedsEncoder);
    }

    if plan.tracks.is_empty() {
        std::fs::rename(&picture, &request.output)?;
    } else {
        let length = picture_length(&request.parts);
        write_sound_file(plan, &sound, length, cancel, &|fraction| {
            progress(PICTURE_SHARE + fraction * (1.0 - PICTURE_SHARE));
        })?;
        join_picture_and_sound(&picture, &sound, &request.output)?;
    }
    progress(1.0);
    report.audio_tracks = plan.tracks.len();
    Ok(report)
}

/// How long the finished video's picture runs, dead space and all.
fn picture_length(parts: &[ExportPart]) -> Ticks {
    Ticks(
        parts
            .iter()
            .map(|part| part.gap_before.0 + part.range.duration().0)
            .sum(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use bareclip_core::{splice::FrameIndex, timeline::TimeRange};

    fn part(gap: f64, start: f64, end: f64) -> ExportPart {
        ExportPart {
            input: PathBuf::from("a.mp4"),
            frames: Arc::new(FrameIndex::new(Vec::new())),
            range: TimeRange {
                start: Ticks::from_seconds(start),
                end: Ticks::from_seconds(end),
            },
            gap_before: Ticks::from_seconds(gap),
        }
    }

    #[test]
    fn recorded_sound_keeps_its_place_including_dead_space() {
        let plan = SoundPlan::as_recorded(&[part(1.0, 5.0, 7.0), part(0.5, 0.0, 1.0)], &[]);

        assert_eq!(plan.spans[0].start, Ticks::from_seconds(1.0));
        assert_eq!(plan.spans[1].start, Ticks::from_seconds(3.5));
        assert_eq!(plan.spans[1].bus, Bus::ByTrack(0));
        assert_eq!(plan.end(), Ticks::from_seconds(4.5));
    }

    #[test]
    fn the_picture_runs_for_its_clips_and_the_dead_space_before_them() {
        let length = picture_length(&[part(1.0, 5.0, 7.0), part(0.5, 0.0, 1.0)]);

        assert_eq!(length, Ticks::from_seconds(4.5));
    }

    #[test]
    fn working_files_sit_beside_the_finished_one_and_are_hidden_from_its_name() {
        let path = working_file(Path::new("out/my video.mp4"), "sound.m4a");

        assert_eq!(path, Path::new("out/.my video.mp4.sound.m4a"));
    }

    #[test]
    fn overwriting_a_source_is_refused() {
        let request = ExportRequest {
            output: PathBuf::from("a.mp4"),
            parts: vec![part(0.0, 0.0, 1.0)],
            mode: CutMode::Keyframe,
        };

        assert!(matches!(
            export_with_new_sound(
                &request,
                &SoundPlan::default(),
                &AtomicBool::new(false),
                &|_| {}
            ),
            Err(MediaError::SameInputAndOutput) | Err(MediaError::Io(_))
        ));
    }
}
