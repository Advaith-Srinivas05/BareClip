//! Exact time values. Floating point is avoided because its rounding errors
//! add up over a long recording and cause audio and video to drift apart.

/// Ticks in one second. This is divisible by the common video frame rates and
/// audio sample rates, including 23.976, 29.97, 59.94, 44100 and 48000.
pub const TICKS_PER_SECOND: i64 = 705_600_000;

/// A fraction of a second, used to interpret timestamps stored in media files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeBase {
    pub num: i32,
    pub den: i32,
}

/// A position or duration on the timeline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ticks(pub i64);

impl Ticks {
    /// Converts a timestamp from a media file, rounding to the nearest tick.
    pub fn from_pts(pts: i64, base: TimeBase) -> Self {
        let numerator = i128::from(pts) * i128::from(base.num) * i128::from(TICKS_PER_SECOND);
        Self(round_div(numerator, i128::from(base.den)) as i64)
    }

    /// Converts to a timestamp in a media file's time base, rounding to the nearest value.
    pub fn to_pts(self, base: TimeBase) -> i64 {
        let numerator = i128::from(self.0) * i128::from(base.den);
        let denominator = i128::from(base.num) * i128::from(TICKS_PER_SECOND);
        round_div(numerator, denominator) as i64
    }
}

fn round_div(numerator: i128, denominator: i128) -> i128 {
    let half = denominator / 2;
    if numerator >= 0 {
        (numerator + half) / denominator
    } else {
        (numerator - half) / denominator
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MILLISECONDS: TimeBase = TimeBase { num: 1, den: 1000 };
    const MPEG: TimeBase = TimeBase {
        num: 1,
        den: 90_000,
    };

    fn frame_rate(num: i32, den: i32) -> TimeBase {
        TimeBase { num: den, den: num }
    }

    #[test]
    fn converts_one_second_exactly() {
        assert_eq!(Ticks::from_pts(1000, MILLISECONDS).0, TICKS_PER_SECOND);
        assert_eq!(Ticks::from_pts(90_000, MPEG).0, TICKS_PER_SECOND);
    }

    #[test]
    fn common_rates_divide_into_whole_ticks() {
        for rate in [
            frame_rate(24_000, 1001),
            frame_rate(24, 1),
            frame_rate(25, 1),
            frame_rate(30_000, 1001),
            frame_rate(30, 1),
            frame_rate(50, 1),
            frame_rate(60_000, 1001),
            frame_rate(60, 1),
            frame_rate(120, 1),
            frame_rate(144, 1),
            frame_rate(240, 1),
            TimeBase {
                num: 1,
                den: 44_100,
            },
            TimeBase {
                num: 1,
                den: 48_000,
            },
        ] {
            let exact = i128::from(TICKS_PER_SECOND) * i128::from(rate.num);
            assert_eq!(exact % i128::from(rate.den), 0, "{rate:?} is not exact");
        }
    }

    #[test]
    fn long_recording_at_59_94_fps_does_not_drift() {
        let rate = frame_rate(60_000, 1001);
        let one_frame = Ticks::from_pts(1, rate).0;
        let frames = 60 * 60 * 24 * 60;

        assert_eq!(Ticks::from_pts(frames, rate).0, one_frame * frames);
    }

    #[test]
    fn round_trips_through_file_time_bases() {
        for base in [MILLISECONDS, MPEG, frame_rate(60_000, 1001)] {
            for pts in [-90_000, -1, 0, 1, 12_345, 987_654_321] {
                assert_eq!(Ticks::from_pts(pts, base).to_pts(base), pts);
            }
        }
    }

    #[test]
    fn rounds_to_the_nearest_value() {
        let tick_base = TimeBase { num: 1, den: 1 };
        assert_eq!(Ticks(TICKS_PER_SECOND / 2 - 1).to_pts(tick_base), 0);
        assert_eq!(Ticks(TICKS_PER_SECOND / 2).to_pts(tick_base), 1);
        assert_eq!(Ticks(-TICKS_PER_SECOND / 2).to_pts(tick_base), -1);
    }
}
