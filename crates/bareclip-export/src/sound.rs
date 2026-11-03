//! Lines up each video's sound with its place in the finished video, and fills the rest with
//! silence, so every track stays in sync however different the videos are.

use std::path::Path;

use bareclip_core::time::Ticks;
use bareclip_media::{AudioPreview, AudioStep, MediaError, SOUND_CHANNELS, SOUND_RATE};

use crate::sink::VideoSink;

/// Decoded blocks rarely line up to the sample, so a difference this small is not a real hole.
/// Filling it with silence would make a faint click.
const JOIN_TOLERANCE_FRAMES: i64 = 32;

/// An audio frame's number at a moment in the finished video.
pub fn sound_frame_at(time: Ticks) -> i64 {
    (time.as_seconds() * f64::from(SOUND_RATE)).round() as i64
}

/// How a block of sound fits on a track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Placement {
    silence_before: usize,
    skip: usize,
    take: usize,
}

/// Where a block goes, given where the track has got to and where this video's sound ends.
fn place_block(next: i64, start: i64, frames: usize, end: i64) -> Placement {
    let hole = start - next;
    let (silence_before, skip) = if hole.abs() <= JOIN_TOLERANCE_FRAMES {
        (0, 0)
    } else if hole > 0 {
        (hole, 0)
    } else {
        (0, -hole)
    };
    let (silence_before, skip) = (
        silence_before.min((end - next).max(0)),
        skip.min(frames as i64),
    );
    let at = next + silence_before;
    let take = (frames as i64 - skip).min((end - at).max(0));
    Placement {
        silence_before: silence_before as usize,
        skip: skip as usize,
        take: take as usize,
    }
}

/// Where each sound track of the finished video has got to.
pub struct SoundTracks {
    next: Vec<i64>,
}

impl SoundTracks {
    pub fn new(count: usize) -> Self {
        Self {
            next: vec![0; count],
        }
    }

    pub fn count(&self) -> usize {
        self.next.len()
    }

    /// Fills every track with silence up to a place in the finished video.
    pub fn pad_to(&mut self, frame: i64, writer: &mut VideoSink) -> Result<(), MediaError> {
        for (track, next) in self.next.iter_mut().enumerate() {
            let missing = frame - *next;
            if missing > 0 {
                writer.write_silence(track, missing as usize)?;
                *next = frame;
            }
        }
        Ok(())
    }
}

/// One clip's sound on its way into the finished video.
pub struct ClipSound {
    preview: AudioPreview,
    /// Where this clip's sound starts in its own file, as an audio frame.
    source_start: i64,
    /// The same place in the finished video.
    output_start: i64,
    output_end: i64,
    finished: bool,
}

impl ClipSound {
    pub fn open(
        path: &Path,
        source_start: Ticks,
        output_start: Ticks,
        output_end: Ticks,
    ) -> Result<Self, MediaError> {
        let mut preview = AudioPreview::open(path, SOUND_RATE)?;
        preview.seek(source_start.as_seconds())?;
        Ok(Self {
            preview,
            source_start: sound_frame_at(source_start),
            output_start: sound_frame_at(output_start),
            output_end: sound_frame_at(output_end),
            finished: false,
        })
    }

    /// Writes this clip's sound until every track it has reaches a place in the finished video.
    pub fn write_until(
        &mut self,
        frame: i64,
        tracks: &mut SoundTracks,
        writer: &mut VideoSink,
    ) -> Result<(), MediaError> {
        let own_tracks = self.preview.track_count().min(tracks.count());
        let until = frame.min(self.output_end);
        while !self.finished && tracks.next[..own_tracks].iter().any(|next| *next < until) {
            match self.preview.next_step()? {
                AudioStep::Chunk(chunk) => {
                    if chunk.track >= tracks.count() {
                        continue;
                    }
                    let frames = chunk.samples.len() / SOUND_CHANNELS;
                    let start = self.output_start + chunk.start_frame - self.source_start;
                    // Sound from before this clip starts is only there because seeking lands
                    // early, so it must not be mistaken for a late start.
                    let next = tracks.next[chunk.track].max(self.output_start);
                    let placement = place_block(next, start, frames, self.output_end);
                    let before_clip = next - tracks.next[chunk.track];
                    if before_clip > 0 {
                        writer.write_silence(chunk.track, before_clip as usize)?;
                    }
                    if placement.silence_before > 0 {
                        writer.write_silence(chunk.track, placement.silence_before)?;
                    }
                    let first = placement.skip * SOUND_CHANNELS;
                    let last = first + placement.take * SOUND_CHANNELS;
                    if placement.take > 0 {
                        writer.write_sound(chunk.track, &chunk.samples[first..last])?;
                    }
                    tracks.next[chunk.track] =
                        next + (placement.silence_before + placement.take) as i64;
                }
                AudioStep::Pending => {}
                AudioStep::End => self.finished = true,
            }
        }
        Ok(())
    }

    /// Writes the rest of this clip's sound, leaving every track exactly at the clip's end.
    pub fn finish(
        &mut self,
        tracks: &mut SoundTracks,
        writer: &mut VideoSink,
    ) -> Result<(), MediaError> {
        self.write_until(self.output_end, tracks, writer)?;
        tracks.pad_to(self.output_end, writer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const END: i64 = 1_000_000;

    #[test]
    fn a_block_that_follows_on_is_taken_whole() {
        assert_eq!(
            place_block(1000, 1000, 1024, END),
            Placement {
                silence_before: 0,
                skip: 0,
                take: 1024,
            }
        );
    }

    #[test]
    fn a_tiny_mismatch_is_not_treated_as_a_hole() {
        assert_eq!(place_block(1000, 1001, 1024, END).silence_before, 0);
        assert_eq!(place_block(1000, 999, 1024, END).skip, 0);
    }

    #[test]
    fn a_real_hole_in_the_sound_becomes_silence() {
        let placement = place_block(1000, 2000, 1024, END);

        assert_eq!(placement.silence_before, 1000);
        assert_eq!(placement.take, 1024);
    }

    #[test]
    fn sound_from_before_the_clip_starts_is_skipped() {
        let placement = place_block(1000, 500, 1024, END);

        assert_eq!(placement.skip, 500);
        assert_eq!(placement.take, 524);
    }

    #[test]
    fn a_block_entirely_before_the_clip_adds_nothing() {
        let placement = place_block(5000, 0, 1024, END);

        assert_eq!(placement.take, 0);
        assert_eq!(placement.silence_before, 0);
    }

    #[test]
    fn sound_past_the_clip_end_is_cut_off() {
        let placement = place_block(1000, 1000, 1024, 1500);

        assert_eq!(placement.take, 500);
    }

    #[test]
    fn a_hole_past_the_clip_end_stops_at_the_end() {
        let placement = place_block(1000, 9000, 1024, 1500);

        assert_eq!(placement.silence_before, 500);
        assert_eq!(placement.take, 0);
    }

    #[test]
    fn a_moment_becomes_an_audio_frame() {
        assert_eq!(sound_frame_at(Ticks::from_seconds(1.5)), 72_000);
    }
}
