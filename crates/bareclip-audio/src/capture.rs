//! Listens to a microphone. What it hears goes into a queue as stereo, at the microphone's own
//! rate, for a recording to take from, and how loud it is goes to a meter.

use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

use cpal::{
    FromSample, Sample, SampleFormat, SizedSample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use rtrb::{Consumer, Producer, RingBuffer};

/// How much sound the queue holds before the oldest is lost, in seconds. Far more than the
/// recording ever lets wait.
const QUEUE_SECONDS: usize = 4;
const CHANNELS: usize = 2;

/// A microphone Windows offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Microphone {
    pub name: String,
    pub is_default: bool,
}

/// Every microphone, with Windows' default first.
pub fn microphones() -> Vec<Microphone> {
    let host = cpal::default_host();
    let default = host.default_input_device().map(|device| device.to_string());
    let mut found: Vec<Microphone> = host
        .input_devices()
        .map(|devices| {
            devices
                .map(|device| {
                    let name = device.to_string();
                    Microphone {
                        is_default: default.as_ref() == Some(&name),
                        name,
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    found.sort_by_key(|microphone| !microphone.is_default);
    found
}

/// How loud the last stretch of sound was, from 0 to 1, shared with the meter. Each reading takes
/// the loudest sample since the one before.
#[derive(Debug, Default)]
pub struct Level(AtomicU32);

impl Level {
    pub fn note(&self, samples: &[f32]) {
        let loudest = samples
            .iter()
            .fold(0.0_f32, |most, sample| most.max(sample.abs()))
            .min(1.0);
        self.0.fetch_max(loudest.to_bits(), Ordering::AcqRel);
    }

    /// The loudest since the last reading, which starts the next one.
    pub fn take(&self) -> f32 {
        f32::from_bits(self.0.swap(0, Ordering::AcqRel))
    }
}

/// Turns a block of a device's frames into interleaved stereo. One channel is heard on both
/// sides, and channels past the second are left out.
pub fn to_stereo<T>(input: &[T], channels: usize, into: &mut Vec<f32>)
where
    T: Sample,
    f32: FromSample<T>,
{
    if channels == 0 {
        return;
    }
    for frame in input.chunks_exact(channels) {
        let left = f32::from_sample(frame[0]);
        let right = frame
            .get(1)
            .map_or(left, |sample| f32::from_sample(*sample));
        into.push(left);
        into.push(right);
    }
}

/// A microphone being listened to. Dropping it stops listening.
pub struct MicrophoneInput {
    _stream: cpal::Stream,
    sample_rate: u32,
    level: Arc<Level>,
    queue: Option<Consumer<f32>>,
}

impl MicrophoneInput {
    /// Starts listening to the microphone with this name, or Windows' default if it's `None` or
    /// gone.
    pub fn open(name: Option<&str>) -> Result<Self, String> {
        let host = cpal::default_host();
        let chosen = name.and_then(|name| {
            host.input_devices()
                .ok()?
                .find(|device| device.to_string() == name)
        });
        let device = chosen
            .or_else(|| host.default_input_device())
            .ok_or_else(|| "No microphone was found.".to_owned())?;
        Self::listen(&device, "microphone", false)
    }

    /// Starts listening to everything the PC plays through its default output, as a recording of
    /// the screen with its sound needs. Windows sends nothing while nothing plays, so whoever takes
    /// the sound has to fill the silence by the clock.
    pub fn open_pc_sound() -> Result<Self, String> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or_else(|| "No sound output was found to record.".to_owned())?;
        Self::listen(&device, "PC sound", true)
    }

    fn listen(device: &cpal::Device, what: &str, output: bool) -> Result<Self, String> {
        let config = if output {
            device.default_output_config()
        } else {
            device.default_input_config()
        }
        .map_err(|error| format!("The {what} could not be read: {error}"))?;
        let sample_rate = config.sample_rate();
        let (producer, consumer) = RingBuffer::new(sample_rate as usize * QUEUE_SECONDS * CHANNELS);
        let level = Arc::new(Level::default());
        let format = config.sample_format();
        let config: cpal::StreamConfig = config.into();
        let stream = match format {
            SampleFormat::F32 => build::<f32>(device, config, producer, level.clone()),
            SampleFormat::I16 => build::<i16>(device, config, producer, level.clone()),
            SampleFormat::U16 => build::<u16>(device, config, producer, level.clone()),
            SampleFormat::I32 => build::<i32>(device, config, producer, level.clone()),
            format => Err(format!("The {what} uses an unsupported format: {format}")),
        }?;
        stream
            .play()
            .map_err(|error| format!("The {what} could not start: {error}"))?;
        Ok(Self {
            _stream: stream,
            sample_rate,
            level,
            queue: Some(consumer),
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn level(&self) -> Arc<Level> {
        self.level.clone()
    }

    /// Hands over the end of the queue the sound comes out of.
    pub fn take_queue(&mut self) -> Option<Consumer<f32>> {
        self.queue.take()
    }
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut producer: Producer<f32>,
    level: Arc<Level>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels);
    let mut stereo = Vec::new();
    device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                stereo.clear();
                to_stereo(data, channels, &mut stereo);
                level.note(&stereo);
                // Whole frames only, so the two sides never swap. A full queue means nobody is
                // taking sound, as while the recording bar only shows the meter.
                let room = producer.slots() / CHANNELS * CHANNELS;
                for sample in stereo.iter().take(room) {
                    let _ = producer.push(*sample);
                }
            },
            |_error| {},
            None,
        )
        .map_err(|error| format!("The sound could not be listened to: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "needs a sound output, which GitHub's runners may not have"]
    fn the_pc_sound_can_be_listened_to() {
        let mut input = MicrophoneInput::open_pc_sound().expect("it opens");
        assert!(input.sample_rate() >= 8_000);
        assert!(input.take_queue().is_some());
    }

    #[test]
    fn one_channel_is_heard_on_both_sides() {
        let mut out = Vec::new();
        to_stereo(&[0.25_f32, -0.5], 1, &mut out);
        assert_eq!(out, [0.25, 0.25, -0.5, -0.5]);
    }

    #[test]
    fn channels_past_the_second_are_left_out() {
        let mut out = Vec::new();
        to_stereo(&[0.1_f32, 0.2, 0.9, 0.9, 0.3, 0.4, 0.9, 0.9], 4, &mut out);
        assert_eq!(out, [0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn whole_numbers_become_the_same_loudness() {
        let mut out = Vec::new();
        to_stereo(&[i16::MAX, i16::MIN], 2, &mut out);
        assert!((out[0] - 1.0).abs() < 0.001);
        assert!((out[1] + 1.0).abs() < 0.001);
    }

    #[test]
    fn the_level_is_the_loudest_since_the_last_reading() {
        let level = Level::default();
        level.note(&[0.1, -0.6, 0.3]);
        level.note(&[0.2]);
        assert!((level.take() - 0.6).abs() < f32::EPSILON);
        assert_eq!(level.take(), 0.0);
        level.note(&[3.0]);
        assert_eq!(level.take(), 1.0, "it never reads past full");
    }
}
