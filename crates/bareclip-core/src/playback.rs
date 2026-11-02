//! Timing for playback: how far playback has moved, and which frame is due.

use std::time::Duration;

use crate::time::{TICKS_PER_SECOND, Ticks};

/// How far playback has moved after the sound card played this many audio frames.
pub fn advance_from_audio(frames: u64, sample_rate: u32) -> Ticks {
    let ticks = i128::from(frames) * i128::from(TICKS_PER_SECOND) / i128::from(sample_rate.max(1));
    Ticks(ticks as i64)
}

/// How far playback has moved after this much real time, for videos with no sound.
pub fn advance_from_time(elapsed: Duration) -> Ticks {
    let ticks = elapsed.as_nanos() * TICKS_PER_SECOND as u128 / 1_000_000_000;
    Ticks(ticks as i64)
}

/// Decoded frames waiting for their moment on screen.
#[derive(Debug)]
pub struct FrameQueue<T> {
    frames: Vec<(Ticks, T)>,
}

impl<T> Default for FrameQueue<T> {
    fn default() -> Self {
        Self { frames: Vec::new() }
    }
}

impl<T> FrameQueue<T> {
    pub fn push(&mut self, time: Ticks, frame: T) {
        let position = self
            .frames
            .partition_point(|(existing, _)| *existing <= time);
        self.frames.insert(position, (time, frame));
    }

    /// Takes the newest frame due by `now`. Older ones are dropped, so a slow
    /// computer skips frames rather than falling behind the sound.
    pub fn take_due(&mut self, now: Ticks) -> Option<T> {
        let due = self.frames.partition_point(|(time, _)| *time <= now);
        if due == 0 {
            return None;
        }
        let newest = self.frames.drain(..due).next_back();
        newest.map(|(_, frame)| frame)
    }

    pub fn clear(&mut self) {
        self.frames.clear();
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_of_audio_is_a_second_of_time() {
        assert_eq!(advance_from_audio(48_000, 48_000), Ticks(TICKS_PER_SECOND));
        assert_eq!(advance_from_audio(44_100, 44_100), Ticks(TICKS_PER_SECOND));
        assert_eq!(
            advance_from_audio(24_000, 48_000),
            Ticks(TICKS_PER_SECOND / 2)
        );
    }

    #[test]
    fn long_playback_from_audio_does_not_drift() {
        let three_hours = 48_000 * 60 * 60 * 3;

        assert_eq!(
            advance_from_audio(three_hours, 48_000),
            Ticks(TICKS_PER_SECOND * 60 * 60 * 3)
        );
    }

    #[test]
    fn real_time_converts_exactly() {
        assert_eq!(
            advance_from_time(Duration::from_secs(2)),
            Ticks(TICKS_PER_SECOND * 2)
        );
        assert_eq!(
            advance_from_time(Duration::from_millis(500)),
            Ticks(TICKS_PER_SECOND / 2)
        );
    }

    #[test]
    fn a_zero_sample_rate_does_not_crash() {
        assert_eq!(advance_from_audio(10, 0), Ticks(10 * TICKS_PER_SECOND));
    }

    #[test]
    fn nothing_is_due_before_its_time() {
        let mut queue = FrameQueue::default();
        queue.push(Ticks(10), "a");

        assert_eq!(queue.take_due(Ticks(9)), None);
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn a_frame_is_due_exactly_at_its_time() {
        let mut queue = FrameQueue::default();
        queue.push(Ticks(10), "a");

        assert_eq!(queue.take_due(Ticks(10)), Some("a"));
        assert!(queue.is_empty());
    }

    #[test]
    fn falling_behind_skips_to_the_newest_due_frame() {
        let mut queue = FrameQueue::default();
        queue.push(Ticks(10), "a");
        queue.push(Ticks(20), "b");
        queue.push(Ticks(30), "c");
        queue.push(Ticks(40), "d");

        assert_eq!(queue.take_due(Ticks(35)), Some("c"));
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.take_due(Ticks(40)), Some("d"));
    }

    #[test]
    fn frames_arriving_out_of_order_are_sorted() {
        let mut queue = FrameQueue::default();
        queue.push(Ticks(30), "c");
        queue.push(Ticks(10), "a");

        assert_eq!(queue.take_due(Ticks(10)), Some("a"));
    }

    #[test]
    fn clearing_removes_everything() {
        let mut queue = FrameQueue::default();
        queue.push(Ticks(10), "a");
        queue.clear();

        assert_eq!(queue.take_due(Ticks(100)), None);
    }
}
