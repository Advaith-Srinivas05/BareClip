//! How a clip sounds: its volume, fades and whether it is muted.

use serde::{Deserialize, Serialize};

use crate::time::Ticks;

/// A clip's volume as it was recorded, in millionths so saving and undo stay exact.
pub const FULL_VOLUME: u32 = 1_000_000;
/// As loud as a clip can be made.
pub const LOUDEST: u32 = 2 * FULL_VOLUME;
/// How far a fade can reach is half the clip, so both can be used at once.
const LONGEST_FADE_SHARE: i64 = 2;

/// How a clip sounds. It belongs to the whole clip, like its place on the picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipSound {
    pub volume: u32,
    pub muted: bool,
    /// How long it takes to get up to volume, in ticks.
    pub fade_in: i64,
    /// How long it takes to go quiet at the end, in ticks.
    pub fade_out: i64,
    /// Which of its file's sound tracks it plays, once detached from its video. All of them if
    /// not.
    pub only_track: Option<u32>,
}

impl Default for ClipSound {
    fn default() -> Self {
        Self {
            volume: FULL_VOLUME,
            muted: false,
            fade_in: 0,
            fade_out: 0,
            only_track: None,
        }
    }
}

impl ClipSound {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The same with anything out of range brought back, for what comes from a file.
    pub fn clamped(self) -> Self {
        Self {
            volume: self.volume.min(LOUDEST),
            fade_in: self.fade_in.max(0),
            fade_out: self.fade_out.max(0),
            ..self
        }
    }

    /// How long a fade in really is on a clip this long.
    pub fn fade_in_on(&self, length: Ticks) -> Ticks {
        Ticks(self.fade_in.clamp(0, length.0 / LONGEST_FADE_SHARE))
    }

    /// How long a fade out really is on a clip this long.
    pub fn fade_out_on(&self, length: Ticks) -> Ticks {
        Ticks(self.fade_out.clamp(0, length.0 / LONGEST_FADE_SHARE))
    }

    /// What to multiply the sound by this far into a clip of this length.
    pub fn gain_at(&self, into: Ticks, length: Ticks) -> f32 {
        if self.muted {
            return 0.0;
        }
        let volume = self.volume as f32 / FULL_VOLUME as f32;
        let fade_in = self.fade_in_on(length).0;
        let fade_out = self.fade_out_on(length).0;
        let left = length.0 - into.0;
        let mut gain = volume;
        if fade_in > 0 && into.0 < fade_in {
            gain *= fade_curve(into.0 as f64 / fade_in as f64);
        }
        if fade_out > 0 && left < fade_out {
            gain *= fade_curve(left as f64 / fade_out as f64);
        }
        gain
    }
}

/// A quarter of a sine wave, which sounds even where a straight line seems to drop away suddenly.
fn fade_curve(progress: f64) -> f32 {
    (progress.clamp(0.0, 1.0) * std::f64::consts::FRAC_PI_2).sin() as f32
}

/// How a whole row of the timeline sounds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct RowSound {
    pub muted: bool,
    pub solo: bool,
}

impl RowSound {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seconds(value: f64) -> Ticks {
        Ticks::from_seconds(value)
    }

    fn fading(fade_in: f64, fade_out: f64) -> ClipSound {
        ClipSound {
            fade_in: seconds(fade_in).0,
            fade_out: seconds(fade_out).0,
            ..ClipSound::default()
        }
    }

    #[test]
    fn a_normal_clip_plays_as_recorded_all_the_way_through() {
        let sound = ClipSound::default();

        for into in [0.0, 1.0, 9.999] {
            assert_eq!(sound.gain_at(seconds(into), seconds(10.0)), 1.0);
        }
    }

    #[test]
    fn volume_scales_the_sound_and_can_go_past_as_recorded() {
        let half = ClipSound {
            volume: FULL_VOLUME / 2,
            ..ClipSound::default()
        };
        let double = ClipSound {
            volume: LOUDEST,
            ..ClipSound::default()
        };

        assert_eq!(half.gain_at(seconds(1.0), seconds(10.0)), 0.5);
        assert_eq!(double.gain_at(seconds(1.0), seconds(10.0)), 2.0);
    }

    #[test]
    fn a_muted_clip_is_silent_whatever_its_volume() {
        let sound = ClipSound {
            muted: true,
            ..ClipSound::default()
        };

        assert_eq!(sound.gain_at(seconds(1.0), seconds(10.0)), 0.0);
    }

    #[test]
    fn a_fade_in_rises_from_silence_to_full_only_during_the_fade() {
        let sound = fading(2.0, 0.0);
        let length = seconds(10.0);

        assert_eq!(sound.gain_at(Ticks(0), length), 0.0);
        let halfway = sound.gain_at(seconds(1.0), length);
        assert!(halfway > 0.5 && halfway < 1.0, "{halfway}");
        assert_eq!(sound.gain_at(seconds(2.0), length), 1.0);
        assert_eq!(sound.gain_at(seconds(5.0), length), 1.0);
    }

    #[test]
    fn a_fade_out_falls_to_silence_at_the_very_end() {
        let sound = fading(0.0, 2.0);
        let length = seconds(10.0);

        assert_eq!(sound.gain_at(seconds(5.0), length), 1.0);
        let halfway = sound.gain_at(seconds(9.0), length);
        assert!(halfway > 0.5 && halfway < 1.0, "{halfway}");
        assert_eq!(sound.gain_at(length, length), 0.0);
    }

    #[test]
    fn a_fade_never_reaches_past_half_the_clip() {
        let sound = fading(100.0, 100.0);
        let length = seconds(4.0);

        assert_eq!(sound.fade_in_on(length), seconds(2.0));
        assert_eq!(sound.fade_out_on(length), seconds(2.0));
        assert_eq!(sound.gain_at(seconds(2.0), length), 1.0);
    }

    #[test]
    fn a_trimmed_clip_goes_back_to_its_full_fade_when_it_grows_again() {
        let sound = fading(3.0, 0.0);

        assert_eq!(sound.fade_in_on(seconds(2.0)), seconds(1.0));
        assert_eq!(sound.fade_in_on(seconds(10.0)), seconds(3.0));
    }

    #[test]
    fn values_from_a_file_are_brought_back_into_range() {
        let sound = ClipSound {
            volume: 99 * FULL_VOLUME,
            fade_in: -5,
            fade_out: -5,
            ..ClipSound::default()
        }
        .clamped();

        assert_eq!(sound.volume, LOUDEST);
        assert_eq!((sound.fade_in, sound.fade_out), (0, 0));
    }
}
