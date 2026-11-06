//! Changes stereo sound from one rate to another as it streams past. Sound that has to be worked
//! on at one rate, such as noise suppression at 48 kHz, can then be heard at the sound card's.
//! The filter makes it about a third of a millisecond late, far too little to hear.

use ffmpeg_next::{
    ChannelLayout, ffi, format::Sample, format::sample::Type as SampleType, software::resampling,
};

use crate::MediaError;

const CHANNELS: usize = 2;
/// More than FFmpeg's resampling filter keeps back between blocks.
const HELD_BACK: i32 = 256;

pub struct RateChange {
    context: resampling::Context,
}

impl RateChange {
    pub fn new(from: u32, to: u32) -> Result<Self, MediaError> {
        crate::init()?;
        let format = Sample::F32(SampleType::Packed);
        let context = resampling::Context::get(
            format,
            ChannelLayout::STEREO,
            from,
            format,
            ChannelLayout::STEREO,
            to,
        )?;
        Ok(Self { context })
    }

    /// Converts a block of interleaved stereo samples. A little is held back to smooth the join
    /// with the next block, so blocks don't come out exactly in proportion, but the total does.
    pub fn run(&mut self, samples: &[f32]) -> Result<Vec<f32>, MediaError> {
        let frames = samples.len() / CHANNELS;
        if frames == 0 {
            return Ok(Vec::new());
        }
        self.convert(samples.as_ptr().cast::<u8>(), frames)
    }

    /// Whatever is still held back, at the end of the sound.
    pub fn finish(&mut self) -> Result<Vec<f32>, MediaError> {
        self.convert(std::ptr::null(), 0)
    }

    fn convert(&mut self, input: *const u8, frames: usize) -> Result<Vec<f32>, MediaError> {
        // SAFETY: the context is live and set up for packed stereo floats both ways, `input` holds
        // `frames` of them or is null to drain, and the output has room for as many frames as
        // FFmpeg says this call can give.
        unsafe {
            let context = self.context.as_mut_ptr();
            let room = ffi::swr_get_out_samples(context, frames as i32);
            if room < 0 {
                return Err(ffmpeg_next::Error::from(room).into());
            }
            // What the filter holds back isn't counted when draining, so leave room for it.
            let room = room + HELD_BACK;
            let mut output = vec![0.0f32; room as usize * CHANNELS];
            let output_plane = output.as_mut_ptr().cast::<u8>();
            // Draining is asked for with no list of input planes at all.
            let planes = if input.is_null() {
                std::ptr::null()
            } else {
                &input as *const *const u8
            };
            let made = ffi::swr_convert(context, &output_plane, room, planes, frames as i32);
            if made < 0 {
                return Err(ffmpeg_next::Error::from(made).into());
            }
            output.truncate(made as usize * CHANNELS);
            Ok(output)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(frequency: f64, rate: u32, seconds: f64) -> Vec<f32> {
        let frames = (seconds * f64::from(rate)) as usize;
        (0..frames)
            .flat_map(|frame| {
                let value = (std::f64::consts::TAU * frequency * frame as f64 / f64::from(rate))
                    .sin() as f32
                    * 0.5;
                [value, value]
            })
            .collect()
    }

    /// Where a tone crosses zero going up, in seconds, which shows its pitch and its timing.
    fn rising_crossings(samples: &[f32], rate: u32) -> Vec<f64> {
        samples
            .iter()
            .step_by(CHANNELS)
            .copied()
            .collect::<Vec<_>>()
            .windows(2)
            .enumerate()
            .filter(|(_, pair)| pair[0] < 0.0 && pair[1] >= 0.0)
            .map(|(place, pair)| {
                let fraction = f64::from(-pair[0] / (pair[1] - pair[0]));
                (place as f64 + fraction) / f64::from(rate)
            })
            .collect()
    }

    #[test]
    fn sound_changes_rate_in_blocks_keeping_its_pitch_length_and_timing() {
        let input = tone(441.0, 48_000, 2.0);
        let mut change = RateChange::new(48_000, 44_100).unwrap();

        let mut output = Vec::new();
        for block in input.chunks(480 * CHANNELS) {
            output.extend(change.run(block).unwrap());
        }
        output.extend(change.finish().unwrap());

        let frames = output.len() / CHANNELS;
        assert!((frames as i64 - 88_200).abs() <= 2, "{frames} frames");
        let before = rising_crossings(&input, 48_000);
        let after = rising_crossings(&output, 44_100);
        // Away from the very start, every cycle lands where it was, within half a millisecond.
        for (wanted, found) in before.iter().zip(&after).skip(10).take(800) {
            assert!(
                (wanted - found).abs() < 0.000_5,
                "{wanted} came out at {found}"
            );
        }
    }

    #[test]
    fn nothing_in_gives_nothing_out() {
        let mut change = RateChange::new(48_000, 44_100).unwrap();
        assert!(change.run(&[]).unwrap().is_empty());
    }
}
