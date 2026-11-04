//! How fast a clip plays. This is the one place that turns time on the timeline into time in a
//! clip's footage and back.

use serde::{Deserialize, Serialize};

use crate::time::Ticks;

/// Speeds are millionths, so saving and undo stay exact. This is as recorded.
pub const NORMAL: u32 = 1_000_000;
pub const SLOWEST: u32 = 100_000;
pub const FASTEST: u32 = 16_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Speed {
    pub rate: u32,
    /// Keeps voices at their own pitch, rather than rising and falling like a tape.
    pub keep_pitch: bool,
}

impl Default for Speed {
    fn default() -> Self {
        Self {
            rate: NORMAL,
            keep_pitch: true,
        }
    }
}

/// Divides, rounding to the nearest whole number either way, because `/` alone would round towards
/// zero and treat moves left and right differently.
fn divided(number: i128, by: i128) -> i64 {
    let half = by / 2;
    let rounded = if number >= 0 {
        (number + half) / by
    } else {
        (number - half) / by
    };
    rounded as i64
}

impl Speed {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Whether this plays as recorded, whatever its pitch setting would do otherwise.
    pub fn is_normal(&self) -> bool {
        self.rate == NORMAL
    }

    /// The same with anything out of range brought back, for what comes from a file.
    pub fn clamped(self) -> Self {
        Self {
            rate: self.rate.clamp(SLOWEST, FASTEST),
            ..self
        }
    }

    /// How much footage plays in a stretch of the timeline.
    pub fn in_footage(&self, on_timeline: Ticks) -> Ticks {
        Ticks(divided(
            i128::from(on_timeline.0) * i128::from(self.rate),
            i128::from(NORMAL),
        ))
    }

    /// How long a stretch of footage lasts on the timeline.
    pub fn on_timeline(&self, footage: Ticks) -> Ticks {
        Ticks(divided(
            i128::from(footage.0) * i128::from(NORMAL),
            i128::from(self.rate),
        ))
    }

    /// As a plain number, where 2 is twice as fast. This is only for sound and showing to people,
    /// because a float's rounding must not reach where clips are.
    pub fn as_factor(&self) -> f64 {
        f64::from(self.rate) / f64::from(NORMAL)
    }

    /// The speed for a plain number, kept in range.
    pub fn with_factor(self, factor: f64) -> Self {
        let rate = (factor * f64::from(NORMAL)).round();
        Self {
            rate: rate.clamp(f64::from(SLOWEST), f64::from(FASTEST)) as u32,
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(rate: f64) -> Speed {
        Speed::default().with_factor(rate)
    }

    #[test]
    fn at_normal_speed_nothing_changes_at_all() {
        let speed = Speed::default();
        for ticks in [0, 1, 7, 705_600_000, -705_600_001, i64::MAX / 32] {
            assert_eq!(speed.in_footage(Ticks(ticks)), Ticks(ticks));
            assert_eq!(speed.on_timeline(Ticks(ticks)), Ticks(ticks));
        }
    }

    #[test]
    fn twice_as_fast_takes_half_as_long_and_covers_twice_the_footage() {
        let speed = at(2.0);

        assert_eq!(speed.on_timeline(Ticks(1000)), Ticks(500));
        assert_eq!(speed.in_footage(Ticks(500)), Ticks(1000));
    }

    #[test]
    fn a_tenth_of_the_speed_takes_ten_times_as_long() {
        assert_eq!(at(0.1).on_timeline(Ticks(1000)), Ticks(10_000));
    }

    #[test]
    fn moves_left_and_right_round_the_same_way() {
        let speed = at(3.0);

        assert_eq!(speed.on_timeline(Ticks(100)), Ticks(33));
        assert_eq!(speed.on_timeline(Ticks(-100)), Ticks(-33));
    }

    #[test]
    fn a_number_out_of_range_is_brought_back() {
        assert_eq!(at(0.001).rate, SLOWEST);
        assert_eq!(at(1000.0).rate, FASTEST);
        let from_a_file = Speed {
            rate: 1,
            keep_pitch: false,
        };
        assert_eq!(from_a_file.clamped().rate, SLOWEST);
        assert!(!from_a_file.clamped().keep_pitch);
    }

    #[test]
    fn a_speed_of_one_with_tape_pitch_is_still_normal_but_not_the_default() {
        let speed = Speed {
            rate: NORMAL,
            keep_pitch: false,
        };

        assert!(speed.is_normal());
        assert!(!speed.is_default());
    }
}
