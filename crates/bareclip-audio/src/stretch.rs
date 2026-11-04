//! Plays sound faster or slower, either like a tape, where pitch moves with speed, or keeping its
//! pitch. Keeping it works by overlapping short grains of the sound, each read from where the clip
//! has reached by then. Each grain is nudged to line up with the waves of the last, because grains
//! out of step cancel each other out and warble.

use std::collections::VecDeque;

use bareclip_core::speed::Speed;

const CHANNELS: usize = 2;
/// How long a grain is, in frames: about 43 ms at 48 kHz. Long enough to hold a low voice's wave,
/// and short enough not to smear what is said.
const GRAIN: usize = 2048;
/// How far on the output moves each grain. Half a grain, so that two always overlap and their
/// fades add up to full volume.
const HOP: usize = GRAIN / 2;
/// How far either way a grain can be nudged to line up.
const NUDGE: i64 = 512;
/// Only every few frames are compared. Lining up by the larger waves is what matters, and it keeps
/// this cheap enough for several clips at once.
const COMPARE_EVERY: usize = 4;
/// How many frames are averaged at most when a tape plays fast. Without this, high sounds fold back
/// down as a harsh whine.
const MOST_AVERAGED: i64 = 32;

/// Sound to read from, by frame of the clip's footage. Anything it doesn't have is silence.
pub(crate) trait Frames {
    fn frame(&self, at: i64) -> [f32; 2];
}

/// One track of one clip's sound, turned into sound for the timeline.
pub(crate) struct Stretch {
    factor: f64,
    keep_pitch: bool,
    /// Footage past this was cut away, so none of it may be heard.
    footage_frames: i64,
    /// The next timeline frame of the clip to hand over.
    next: Option<i64>,
    /// Finished stereo samples, waiting to be handed over.
    ready: VecDeque<f32>,
    /// Grains overlapping what hasn't finished yet.
    building: Vec<f32>,
    /// Where on the timeline the next grain starts.
    grain_at: i64,
    /// Where in the footage the last grain was read from.
    last_read: Option<i64>,
    window: Vec<f32>,
}

impl Stretch {
    pub(crate) fn new(speed: Speed, footage_frames: i64) -> Self {
        let window = (0..GRAIN)
            .map(|at| {
                let turn = at as f32 / GRAIN as f32;
                0.5 - 0.5 * (std::f32::consts::TAU * turn).cos()
            })
            .collect();
        Self {
            factor: speed.as_factor(),
            keep_pitch: speed.keep_pitch,
            footage_frames,
            next: None,
            ready: VecDeque::new(),
            building: vec![0.0; GRAIN * CHANNELS],
            grain_at: 0,
            last_read: None,
            window,
        }
    }

    fn footage_at(&self, timeline: i64) -> i64 {
        (timeline as f64 * self.factor) as i64
    }

    /// How far into the footage has to be available to hand over everything up to this frame.
    pub(crate) fn needs_until(&self, finish: i64) -> i64 {
        let spare = if self.keep_pitch {
            (GRAIN as i64) * 2
        } else {
            0
        };
        self.footage_at(finish + spare) + GRAIN as i64 + NUDGE + 2
    }

    /// Nothing before this frame of the footage will be read again.
    pub(crate) fn keeps_from(&self, reached: i64) -> i64 {
        if !self.keep_pitch {
            return self.footage_at(reached) - 1;
        }
        let next_grain = self.footage_at(self.grain_at) - NUDGE;
        let template = self.last_read.map_or(next_grain, |last| last + HOP as i64);
        next_grain.min(template)
    }

    fn read(&self, frames: &impl Frames, at: i64) -> [f32; 2] {
        if at < 0 || at >= self.footage_frames {
            [0.0; 2]
        } else {
            frames.frame(at)
        }
    }

    /// Hands over `count` frames of sound starting at this frame of the clip on the timeline.
    pub(crate) fn produce(
        &mut self,
        frames: &impl Frames,
        begin: i64,
        count: usize,
        into: &mut Vec<f32>,
    ) {
        if self.next != Some(begin) {
            self.jump_to(begin);
        }
        if self.keep_pitch {
            while self.ready.len() < count * CHANNELS {
                self.add_grain(frames);
            }
            into.extend(self.ready.drain(..count * CHANNELS));
        } else {
            for frame in begin..begin + count as i64 {
                let sample = self.tape_at(frames, frame);
                into.extend(sample);
            }
        }
        self.next = Some(begin + count as i64);
    }

    fn jump_to(&mut self, at: i64) {
        self.ready.clear();
        self.building.fill(0.0);
        self.grain_at = at;
        self.last_read = None;
    }

    /// The tape's sound at a frame on the timeline: smoothly between two frames when slow, and the
    /// average of what it passes over when fast.
    fn tape_at(&self, frames: &impl Frames, frame: i64) -> [f32; 2] {
        let place = frame as f64 * self.factor;
        let first = place.floor() as i64;
        if self.factor <= 1.0 {
            let along = (place - first as f64) as f32;
            let (a, b) = (self.read(frames, first), self.read(frames, first + 1));
            return [a[0] + (b[0] - a[0]) * along, a[1] + (b[1] - a[1]) * along];
        }
        let span = (self.factor.round() as i64).clamp(1, MOST_AVERAGED);
        let mut sum = [0.0_f32; 2];
        for at in first..first + span {
            let sample = self.read(frames, at);
            sum[0] += sample[0];
            sum[1] += sample[1];
        }
        [sum[0] / span as f32, sum[1] / span as f32]
    }

    /// Where near this place the next grain lines up best with how the last one carries on.
    fn best_place(&self, frames: &impl Frames, ideal: i64) -> i64 {
        let Some(last) = self.last_read else {
            return ideal;
        };
        let carries_on = last + HOP as i64;
        let wave = |at: i64| {
            let [left, right] = self.read(frames, at);
            left + right
        };
        let wanted: Vec<f32> = (0..HOP)
            .step_by(COMPARE_EVERY)
            .map(|at| wave(carries_on + at as i64))
            .collect();
        if wanted.iter().all(|sample| *sample == 0.0) {
            return ideal;
        }
        let mut best = (ideal, f32::MIN);
        for nudge in (-NUDGE..=NUDGE).step_by(COMPARE_EVERY) {
            let place = ideal + nudge;
            let (mut together, mut energy) = (0.0, 0.0);
            for (index, sample) in wanted.iter().enumerate() {
                let here = wave(place + (index * COMPARE_EVERY) as i64);
                together += here * sample;
                energy += here * here;
            }
            let score = together / (energy.sqrt() + 1e-6);
            if score > best.1 {
                best = (place, score);
            }
        }
        best.0
    }

    /// Adds the next grain, which finishes one hop of sound.
    fn add_grain(&mut self, frames: &impl Frames) {
        let ideal = self.footage_at(self.grain_at);
        let place = self.best_place(frames, ideal);
        for at in 0..GRAIN {
            let sample = self.read(frames, place + at as i64);
            let fade = self.window[at];
            self.building[at * CHANNELS] += sample[0] * fade;
            self.building[at * CHANNELS + 1] += sample[1] * fade;
        }
        self.ready.extend(self.building.drain(..HOP * CHANNELS));
        self.building.resize(GRAIN * CHANNELS, 0.0);
        self.grain_at += HOP as i64;
        self.last_read = Some(place);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 48_000.0;

    /// Endless sound worked out from where it is read.
    struct Wave<F: Fn(i64) -> f32>(F);

    impl<F: Fn(i64) -> f32> Frames for Wave<F> {
        fn frame(&self, at: i64) -> [f32; 2] {
            let sample = (self.0)(at);
            [sample, sample]
        }
    }

    fn tone(frequency: f64) -> Wave<impl Fn(i64) -> f32> {
        Wave(move |at| (at as f64 / RATE * frequency * std::f64::consts::TAU).sin() as f32 * 0.5)
    }

    fn speed(factor: f64, keep_pitch: bool) -> Speed {
        Speed {
            keep_pitch,
            ..Speed::default().with_factor(factor)
        }
    }

    /// Runs a second of sound through, in the uneven blocks a mix asks for, as one channel.
    fn stretched(speed: Speed, frames: &impl Frames, seconds: f64) -> Vec<f32> {
        let mut stretch = Stretch::new(speed, i64::MAX);
        let mut out = Vec::new();
        let total = (seconds * RATE) as i64;
        let mut at = 0;
        let mut size = 300;
        while at < total {
            let count = size.min((total - at) as usize);
            stretch.produce(frames, at, count, &mut out);
            at += count as i64;
            size = if size == 300 { 4096 } else { 300 };
        }
        out.iter().step_by(CHANNELS).copied().collect()
    }

    /// How often a sound swings through zero, as a frequency.
    fn frequency_of(samples: &[f32]) -> f64 {
        let settled = &samples[GRAIN..];
        let crossings = settled
            .windows(2)
            .filter(|pair| pair[0] <= 0.0 && pair[1] > 0.0)
            .count();
        crossings as f64 / (settled.len() as f64 / RATE)
    }

    #[track_caller]
    fn assert_close(found: f64, wanted: f64) {
        assert!(
            (found - wanted).abs() / wanted < 0.03,
            "wanted about {wanted}, found {found}"
        );
    }

    #[test]
    fn keeping_pitch_keeps_the_note_at_every_speed() {
        for factor in [0.1, 0.25, 0.5, 0.75, 1.5, 2.0, 4.0, 8.0, 16.0] {
            let out = stretched(speed(factor, true), &tone(440.0), 1.0);
            assert_close(frequency_of(&out), 440.0);
        }
    }

    #[test]
    fn a_tape_moves_the_note_with_the_speed() {
        assert_close(
            frequency_of(&stretched(speed(2.0, false), &tone(220.0), 1.0)),
            440.0,
        );
        assert_close(
            frequency_of(&stretched(speed(0.5, false), &tone(440.0), 1.0)),
            220.0,
        );
    }

    #[test]
    fn it_hands_over_exactly_what_was_asked_for() {
        for keep_pitch in [true, false] {
            let out = stretched(speed(1.7, keep_pitch), &tone(100.0), 0.5);
            assert_eq!(out.len(), 24_000);
        }
    }

    #[test]
    fn steady_sound_stays_at_the_same_level_with_no_dips_where_grains_meet() {
        for factor in [0.3, 0.5, 2.0, 7.0] {
            let out = stretched(speed(factor, true), &Wave(|_| 0.25), 0.5);
            for (at, sample) in out.iter().enumerate().skip(HOP) {
                assert!(
                    (sample - 0.25).abs() < 0.001,
                    "at {factor}x, frame {at} came out as {sample}"
                );
            }
        }
    }

    #[test]
    fn a_tones_level_holds_up_because_grains_are_lined_up() {
        let peak = |samples: &[f32]| {
            samples
                .chunks(1200)
                .skip(2)
                .map(|block| {
                    block
                        .iter()
                        .fold(0.0_f32, |most, sample| most.max(sample.abs()))
                })
                .fold(f32::MAX, f32::min)
        };
        for factor in [0.5, 1.5, 2.0, 4.0] {
            let out = stretched(speed(factor, true), &tone(440.0), 1.0);
            let quietest = peak(&out);
            assert!(quietest > 0.4, "at {factor}x it dipped to {quietest}");
        }
    }

    #[test]
    fn nothing_cut_away_is_heard_at_the_end() {
        let frames = Wave(|_| 0.5);
        for keep_pitch in [true, false] {
            let mut stretch = Stretch::new(speed(2.0, keep_pitch), 48_000);
            let mut out = Vec::new();
            stretch.produce(&frames, 0, 36_000, &mut out);

            let left: Vec<f32> = out.iter().step_by(CHANNELS).copied().collect();
            assert!(left[..23_000].iter().any(|sample| *sample > 0.4));
            assert!(
                left[25_000..].iter().all(|sample| sample.abs() < 1e-6),
                "footage past the clip's end was heard"
            );
        }
    }

    #[test]
    fn jumping_to_a_new_place_starts_clean_from_there() {
        let mut stretch = Stretch::new(speed(2.0, true), i64::MAX);
        let mut out = Vec::new();
        stretch.produce(&Wave(|_| 0.25), 0, 2000, &mut out);

        out.clear();
        stretch.produce(
            &Wave(|at| if at >= 48_000 { 0.25 } else { 0.0 }),
            24_000,
            HOP * 2,
            &mut out,
        );

        let left: Vec<f32> = out.iter().step_by(CHANNELS).copied().collect();
        assert!((left[HOP + 50] - 0.25).abs() < 0.001, "{}", left[HOP + 50]);
    }

    #[test]
    fn what_has_been_passed_can_be_let_go_of_and_what_is_needed_is_ahead() {
        let frames = tone(440.0);
        let mut stretch = Stretch::new(speed(2.0, true), i64::MAX);
        let mut out = Vec::new();
        stretch.produce(&frames, 0, 9600, &mut out);

        let from = stretch.keeps_from(9600);
        assert!(from > 0, "{from}");
        assert!(from <= stretch.footage_at(stretch.grain_at));
        assert!(stretch.needs_until(9600) > 19_200);
    }
}
