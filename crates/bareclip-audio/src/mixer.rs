//! Mixes several audio tracks into one stereo stream.
//!
//! Tracks arrive in chunks at uneven times, so each is buffered until every track
//! has sound for the same moment.

use std::collections::VecDeque;

const CHANNELS: usize = 2;

#[derive(Debug, Default)]
struct Track {
    /// Stereo samples, starting at the mixer's current position.
    samples: VecDeque<f32>,
    finished: bool,
}

#[derive(Debug)]
pub struct TrackMixer {
    tracks: Vec<Track>,
    /// The audio frame the next pull starts at.
    position: i64,
}

impl TrackMixer {
    pub fn new(track_count: usize, position: i64) -> Self {
        Self {
            tracks: (0..track_count).map(|_| Track::default()).collect(),
            position,
        }
    }

    pub fn position(&self) -> i64 {
        self.position
    }

    /// Adds stereo samples for a track, starting at audio frame `start`. A gap
    /// becomes silence, and anything before the current position is dropped.
    pub fn push(&mut self, track: usize, start: i64, samples: &[f32]) {
        let Some(buffer) = self.tracks.get_mut(track) else {
            return;
        };
        let buffered_end = self.position + (buffer.samples.len() / CHANNELS) as i64;

        let mut samples = samples;
        if start < buffered_end {
            let overlap = ((buffered_end - start) as usize).saturating_mul(CHANNELS);
            samples = samples.get(overlap..).unwrap_or_default();
        } else if start > buffered_end {
            let gap = (start - buffered_end) as usize * CHANNELS;
            buffer.samples.extend(std::iter::repeat_n(0.0, gap));
        }
        buffer.samples.extend(samples.iter().copied());
    }

    /// Marks a track as having no more sound, so the others are not held back.
    pub fn finish(&mut self, track: usize) {
        if let Some(buffer) = self.tracks.get_mut(track) {
            buffer.finished = true;
        }
    }

    /// How many audio frames can be mixed right now.
    pub fn available(&self) -> usize {
        let waiting_on_live_track = self
            .tracks
            .iter()
            .filter(|track| !track.finished)
            .map(|track| track.samples.len() / CHANNELS)
            .min();
        waiting_on_live_track.unwrap_or_else(|| {
            self.tracks
                .iter()
                .map(|track| track.samples.len() / CHANNELS)
                .max()
                .unwrap_or(0)
        })
    }

    /// Whether every track has finished and been fully played out.
    pub fn is_finished(&self) -> bool {
        self.tracks
            .iter()
            .all(|track| track.finished && track.samples.is_empty())
    }

    /// Mixes up to `max_frames` audio frames into `output` as stereo samples.
    /// Returns how many frames were written.
    pub fn pull(&mut self, max_frames: usize, output: &mut Vec<f32>) -> usize {
        let frames = self.available().min(max_frames);
        let start = output.len();
        output.resize(start + frames * CHANNELS, 0.0);
        for track in &mut self.tracks {
            let take = (frames * CHANNELS).min(track.samples.len());
            for (slot, sample) in output[start..].iter_mut().zip(track.samples.drain(..take)) {
                *slot += sample;
            }
        }
        for sample in &mut output[start..] {
            *sample = sample.clamp(-1.0, 1.0);
        }
        self.position += frames as i64;
        frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pull_all(mixer: &mut TrackMixer) -> Vec<f32> {
        let mut output = Vec::new();
        mixer.pull(usize::MAX, &mut output);
        output
    }

    #[test]
    fn a_single_track_passes_through() {
        let mut mixer = TrackMixer::new(1, 0);
        mixer.push(0, 0, &[0.1, 0.2, 0.3, 0.4]);

        assert_eq!(pull_all(&mut mixer), [0.1, 0.2, 0.3, 0.4]);
        assert_eq!(mixer.position(), 2);
    }

    #[test]
    fn tracks_are_added_together() {
        let mut mixer = TrackMixer::new(2, 0);
        mixer.push(0, 0, &[0.1, 0.2]);
        mixer.push(1, 0, &[0.3, 0.4]);

        let mixed = pull_all(&mut mixer);
        assert!((mixed[0] - 0.4).abs() < 1e-6);
        assert!((mixed[1] - 0.6).abs() < 1e-6);
    }

    #[test]
    fn mixing_waits_for_the_slowest_track() {
        let mut mixer = TrackMixer::new(2, 0);
        mixer.push(0, 0, &[0.5; 8]);

        assert_eq!(mixer.available(), 0);
        mixer.push(1, 0, &[0.5; 4]);
        assert_eq!(mixer.available(), 2);
    }

    #[test]
    fn a_finished_track_stops_holding_the_others_back() {
        let mut mixer = TrackMixer::new(2, 0);
        mixer.push(0, 0, &[0.5; 8]);
        mixer.finish(1);

        assert_eq!(mixer.available(), 4);
        assert_eq!(pull_all(&mut mixer).len(), 8);
    }

    #[test]
    fn loud_mixes_are_limited_rather_than_wrapping() {
        let mut mixer = TrackMixer::new(2, 0);
        mixer.push(0, 0, &[0.9, -0.9]);
        mixer.push(1, 0, &[0.9, -0.9]);

        assert_eq!(pull_all(&mut mixer), [1.0, -1.0]);
    }

    #[test]
    fn a_gap_in_a_track_becomes_silence() {
        let mut mixer = TrackMixer::new(1, 0);
        mixer.push(0, 2, &[0.5, 0.5]);

        assert_eq!(pull_all(&mut mixer), [0.0, 0.0, 0.0, 0.0, 0.5, 0.5]);
    }

    #[test]
    fn audio_from_before_the_position_is_dropped() {
        let mut mixer = TrackMixer::new(1, 10);
        mixer.push(0, 9, &[0.1, 0.1, 0.2, 0.2]);

        assert_eq!(pull_all(&mut mixer), [0.2, 0.2]);
    }

    #[test]
    fn a_chunk_entirely_in_the_past_adds_nothing() {
        let mut mixer = TrackMixer::new(1, 100);
        mixer.push(0, 0, &[0.1, 0.1]);

        assert!(pull_all(&mut mixer).is_empty());
    }

    #[test]
    fn pulling_stops_at_the_limit_and_continues() {
        let mut mixer = TrackMixer::new(1, 0);
        mixer.push(0, 0, &[0.1, 0.1, 0.2, 0.2, 0.3, 0.3]);

        let mut output = Vec::new();
        assert_eq!(mixer.pull(1, &mut output), 1);
        assert_eq!(output, [0.1, 0.1]);
        assert_eq!(mixer.pull(10, &mut output), 2);
        assert_eq!(output, [0.1, 0.1, 0.2, 0.2, 0.3, 0.3]);
    }

    #[test]
    fn finished_means_played_out_as_well() {
        let mut mixer = TrackMixer::new(1, 0);
        mixer.push(0, 0, &[0.1, 0.1]);
        mixer.finish(0);
        assert!(!mixer.is_finished());

        pull_all(&mut mixer);
        assert!(mixer.is_finished());
    }

    #[test]
    fn a_video_with_no_audio_tracks_is_already_finished() {
        let mixer = TrackMixer::new(0, 0);

        assert!(mixer.is_finished());
        assert_eq!(mixer.available(), 0);
    }

    #[test]
    fn an_unknown_track_is_ignored() {
        let mut mixer = TrackMixer::new(1, 0);
        mixer.push(5, 0, &[0.1, 0.1]);
        mixer.finish(5);

        assert_eq!(mixer.available(), 0);
    }
}
