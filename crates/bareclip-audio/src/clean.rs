//! Noise suppression: takes steady background noise, such as a fan, hum or hiss, out of a voice,
//! with RNNoise through `nnnoiseless`. RNNoise only works on 48 kHz sound in blocks of 10 ms, and
//! gives each block back one block late, which the mix takes back off so the sound stays in time.

use std::sync::OnceLock;

use nnnoiseless::{DenoiseState, RnnModel};

/// The only rate RNNoise works at.
pub const CLEAN_RATE: u32 = 48_000;
/// How much it works on at a time, in frames.
pub const BLOCK_FRAMES: usize = DenoiseState::FRAME_SIZE;
/// How late each cleaned block comes back, in frames.
pub const DELAY_FRAMES: i64 = BLOCK_FRAMES as i64;
/// After a jump, cleaning starts this far before what is heard, so it has settled by then.
pub const LEAD_IN_FRAMES: i64 = CLEAN_RATE as i64 / 2;
/// RNNoise expects sound scaled like 16-bit samples.
const SCALE: f32 = 32_768.0;
const CHANNELS: usize = 2;

/// One copy of the trained model, shared by every cleaner.
fn model() -> &'static RnnModel {
    static MODEL: OnceLock<RnnModel> = OnceLock::new();
    MODEL.get_or_init(RnnModel::default)
}

/// Cleans one stereo stream, each side on its own.
pub struct Cleaner {
    sides: [Box<DenoiseState<'static>>; CHANNELS],
    input: Vec<f32>,
    output: Vec<f32>,
}

impl Default for Cleaner {
    fn default() -> Self {
        Self {
            sides: [
                DenoiseState::with_model(model()),
                DenoiseState::with_model(model()),
            ],
            input: vec![0.0; BLOCK_FRAMES],
            output: vec![0.0; BLOCK_FRAMES],
        }
    }
}

impl Cleaner {
    /// Cleans one block of interleaved stereo, [`BLOCK_FRAMES`] long, and gives back the block
    /// that went in [`DELAY_FRAMES`] before it.
    pub fn clean(&mut self, block: &[f32]) -> Vec<f32> {
        debug_assert_eq!(block.len(), BLOCK_FRAMES * CHANNELS);
        let mut cleaned = vec![0.0; BLOCK_FRAMES * CHANNELS];
        for (side, state) in self.sides.iter_mut().enumerate() {
            for (frame, sample) in self.input.iter_mut().enumerate() {
                *sample = block[frame * CHANNELS + side] * SCALE;
            }
            state.process_frame(&mut self.output, &self.input);
            for (frame, sample) in self.output.iter().enumerate() {
                cleaned[frame * CHANNELS + side] = sample / SCALE;
            }
        }
        cleaned
    }
}

/// Sounds for tests: something voice-like, and steady hiss.
#[cfg(test)]
pub(crate) mod sounds {
    use super::CLEAN_RATE;

    /// A small, repeatable random number generator, so tests don't need a crate for it.
    pub struct Dice(u64);

    impl Dice {
        pub fn new(seed: u64) -> Self {
            Self(seed.max(1))
        }

        /// From -1 to 1.
        pub fn roll(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 40) as f32 / (1u64 << 23) as f32 - 1.0
        }
    }

    /// A vowel sung on a wavering pitch: pulses from the throat, shaped by the mouth's three
    /// strongest resonances, rising and falling four times a second like syllables. Silent
    /// where `talking` says so.
    pub fn voice(seconds: f64, talking: impl Fn(f64) -> bool) -> Vec<f32> {
        let rate = f64::from(CLEAN_RATE);
        let frames = (seconds * rate) as usize;
        let resonances = [
            (700.0, 130.0, 1.0),
            (1220.0, 70.0, 0.5),
            (2600.0, 160.0, 0.25),
        ];
        let mut filters: Vec<[f64; 2]> = vec![[0.0; 2]; resonances.len()];
        let mut phase = 0.0;
        let mut samples = Vec::with_capacity(frames);
        for frame in 0..frames {
            let time = frame as f64 / rate;
            let pitch = 140.0 + 25.0 * (std::f64::consts::TAU * 1.3 * time).sin();
            phase += pitch / rate;
            let pulse = if phase >= 1.0 {
                phase -= 1.0;
                1.0
            } else {
                0.0
            };
            let mut value = 0.0;
            for ((centre, width, weight), state) in resonances.iter().zip(&mut filters) {
                let radius = (-std::f64::consts::PI * width / rate).exp();
                let turn = std::f64::consts::TAU * centre / rate;
                let next =
                    pulse + 2.0 * radius * turn.cos() * state[0] - radius * radius * state[1];
                state[1] = state[0];
                state[0] = next;
                value += weight * next;
            }
            let syllables = (0.5 - 0.5 * (std::f64::consts::TAU * 4.0 * time).cos()).powf(0.7);
            let on = if talking(time) { 1.0 } else { 0.0 };
            samples.push((value * syllables * on * 0.02) as f32);
        }
        samples
    }

    pub fn hiss(frames: usize, level: f32, dice: &mut Dice) -> Vec<f32> {
        (0..frames).map(|_| dice.roll() * level).collect()
    }

    /// The loudness of a stretch, in decibels below full scale.
    pub fn decibels(samples: &[f32]) -> f64 {
        let power: f64 = samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>()
            / samples.len().max(1) as f64;
        10.0 * power.max(1e-20).log10()
    }
}

#[cfg(test)]
mod tests {
    use super::{sounds::*, *};

    fn stereo(mono: &[f32]) -> Vec<f32> {
        mono.iter().flat_map(|s| [*s, *s]).collect()
    }

    fn left(stereo: &[f32]) -> Vec<f32> {
        stereo.iter().step_by(CHANNELS).copied().collect()
    }

    fn cleaned(mono: &[f32]) -> Vec<f32> {
        let mut cleaner = Cleaner::default();
        let input = stereo(mono);
        let mut out = Vec::new();
        for block in input.as_chunks::<{ BLOCK_FRAMES * CHANNELS }>().0 {
            out.extend(cleaner.clean(block));
        }
        left(&out)
    }

    #[test]
    fn the_cleaned_sound_comes_back_one_block_late() {
        let voice = voice(3.0, |_| true);
        let out = cleaned(&voice);

        // Whichever lag lines the two up best is how late it is.
        let score = |lag: usize| -> f64 {
            (CLEAN_RATE as usize..2 * CLEAN_RATE as usize)
                .map(|at| f64::from(voice[at]) * f64::from(out[at + lag]))
                .sum()
        };
        let best = (0..3 * BLOCK_FRAMES)
            .max_by(|a, b| score(*a).total_cmp(&score(*b)))
            .unwrap();
        assert_eq!(best as i64, DELAY_FRAMES);
    }

    #[test]
    fn steady_hiss_on_its_own_goes_almost_silent() {
        let mut dice = Dice::new(7);
        let hiss = hiss(CLEAN_RATE as usize * 3, 0.03, &mut dice);
        let out = cleaned(&hiss);
        let settled = CLEAN_RATE as usize;

        let before = decibels(&hiss[settled..]);
        let after = decibels(&out[settled..]);
        assert!(
            after < before - 20.0,
            "hiss went from {before:.1} dB to {after:.1} dB"
        );
    }

    #[test]
    fn a_voice_keeps_most_of_its_loudness() {
        let voice = voice(3.0, |_| true);
        let out = cleaned(&voice);
        let settled = CLEAN_RATE as usize;
        let delay = DELAY_FRAMES as usize;

        let before = decibels(&voice[settled..voice.len() - delay]);
        let after = decibels(&out[settled + delay..]);
        assert!(
            after > before - 3.0,
            "the voice went from {before:.1} dB to {after:.1} dB"
        );
    }

    #[test]
    fn both_sides_are_cleaned_apart() {
        let voice = voice(3.0, |_| true);
        let mut dice = Dice::new(3);
        let hiss = hiss(voice.len(), 0.03, &mut dice);
        let input: Vec<f32> = voice
            .iter()
            .zip(&hiss)
            .flat_map(|(v, h)| [*v, *h])
            .collect();
        let mut cleaner = Cleaner::default();
        let mut out = Vec::new();
        for block in input.as_chunks::<{ BLOCK_FRAMES * CHANNELS }>().0 {
            out.extend(cleaner.clean(block));
        }
        let settled = CLEAN_RATE as usize * CHANNELS;
        let voice_side: Vec<f32> = out[settled..].iter().step_by(2).copied().collect();
        let hiss_side: Vec<f32> = out[settled + 1..].iter().step_by(2).copied().collect();

        let (voice_level, hiss_level) = (decibels(&voice_side), decibels(&hiss_side));
        assert!(
            voice_level > hiss_level + 20.0,
            "voice {voice_level:.1} dB, hiss {hiss_level:.1} dB"
        );
    }
}
