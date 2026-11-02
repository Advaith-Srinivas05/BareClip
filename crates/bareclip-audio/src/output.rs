//! Sends mixed audio to the sound card, and counts what has actually been played.
//! That count is the clock the picture follows during playback.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use cpal::{
    FromSample, Sample, SampleFormat, SizedSample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use rtrb::{Consumer, Producer, RingBuffer};

/// Audio frames held between the decoder and the sound card, in seconds.
const BUFFER_SECONDS: f64 = 0.25;
const CHANNELS: usize = 2;

/// State shared between the audio callback and everything else.
#[derive(Debug, Default)]
pub struct OutputState {
    played_frames: AtomicU64,
    paused: AtomicBool,
    flush: AtomicBool,
}

impl OutputState {
    /// Audio frames played since the last flush.
    pub fn played_frames(&self) -> u64 {
        self.played_frames.load(Ordering::Acquire)
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Release);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    /// Throws away queued audio and restarts the played count.
    pub fn request_flush(&self) {
        self.flush.store(true, Ordering::Release);
    }

    /// Whether a requested flush is still waiting for the audio callback.
    pub fn is_flushing(&self) -> bool {
        self.flush.load(Ordering::Acquire)
    }
}

/// Fills a sound card buffer from the queue. Silence covers anything missing, and
/// silence does not count as played, so the clock holds still until sound returns.
pub fn fill_buffer<T>(
    buffer: &mut [T],
    device_channels: usize,
    queue: &mut Consumer<f32>,
    state: &OutputState,
) where
    T: Sample + FromSample<f32>,
{
    if state.flush.load(Ordering::Acquire) {
        while queue.pop().is_ok() {}
        state.played_frames.store(0, Ordering::Release);
        state.flush.store(false, Ordering::Release);
    }

    let silence = T::from_sample(0.0_f32);
    if state.paused.load(Ordering::Acquire) || device_channels == 0 {
        buffer.fill(silence);
        return;
    }

    let mut played = 0;
    for frame in buffer.chunks_mut(device_channels) {
        if queue.slots() < CHANNELS {
            frame.fill(silence);
            continue;
        }
        let left = queue.pop().unwrap_or(0.0);
        let right = queue.pop().unwrap_or(0.0);
        write_frame(frame, left, right);
        played += 1;
    }
    state.played_frames.fetch_add(played, Ordering::AcqRel);
}

/// Places a stereo frame into however many channels the sound card has.
fn write_frame<T>(frame: &mut [T], left: f32, right: f32)
where
    T: Sample + FromSample<f32>,
{
    match frame {
        [only] => *only = T::from_sample((left + right) * 0.5),
        [first, second, rest @ ..] => {
            *first = T::from_sample(left);
            *second = T::from_sample(right);
            rest.fill(T::from_sample(0.0_f32));
        }
        [] => {}
    }
}

/// An open connection to the sound card.
pub struct AudioOutput {
    // Dropping this stops the sound.
    _stream: cpal::Stream,
    sample_rate: u32,
    state: Arc<OutputState>,
    producer: Option<Producer<f32>>,
}

impl AudioOutput {
    /// Opens the default sound output, starting paused. Fails on computers with no
    /// output device, in which case playback falls back to a silent clock.
    pub fn open() -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "No sound output was found.".to_owned())?;
        let config = device
            .default_output_config()
            .map_err(|error| format!("The sound output could not be read: {error}"))?;
        let sample_rate = config.sample_rate();
        let capacity = (f64::from(sample_rate) * BUFFER_SECONDS) as usize * CHANNELS;
        let (producer, consumer) = RingBuffer::new(capacity);
        let state = Arc::new(OutputState {
            paused: AtomicBool::new(true),
            ..Default::default()
        });

        let stream = match config.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, config.into(), consumer, state.clone()),
            SampleFormat::I16 => build::<i16>(&device, config.into(), consumer, state.clone()),
            SampleFormat::U16 => build::<u16>(&device, config.into(), consumer, state.clone()),
            SampleFormat::I32 => build::<i32>(&device, config.into(), consumer, state.clone()),
            format => Err(format!(
                "The sound output uses an unsupported format: {format}"
            )),
        }?;
        stream
            .play()
            .map_err(|error| format!("The sound output could not start: {error}"))?;

        Ok(Self {
            _stream: stream,
            sample_rate,
            state,
            producer: Some(producer),
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn state(&self) -> Arc<OutputState> {
        self.state.clone()
    }

    /// Hands over the end of the queue that audio is written into.
    pub fn take_producer(&mut self) -> Option<Producer<f32>> {
        self.producer.take()
    }
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut consumer: Consumer<f32>,
    state: Arc<OutputState>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let channels = usize::from(config.channels);
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                fill_buffer(data, channels, &mut consumer, &state);
            },
            |_error| {},
            None,
        )
        .map_err(|error| format!("The sound output could not open: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue_with(samples: &[f32]) -> Consumer<f32> {
        let (mut producer, consumer) = RingBuffer::new(64);
        for sample in samples {
            producer.push(*sample).expect("the queue has room");
        }
        consumer
    }

    #[test]
    fn stereo_passes_through_and_is_counted() {
        let state = OutputState::default();
        let mut queue = queue_with(&[0.1, 0.2, 0.3, 0.4]);
        let mut buffer = [9.0_f32; 4];

        fill_buffer(&mut buffer, 2, &mut queue, &state);

        assert_eq!(buffer, [0.1, 0.2, 0.3, 0.4]);
        assert_eq!(state.played_frames(), 2);
    }

    #[test]
    fn running_out_gives_silence_that_does_not_move_the_clock() {
        let state = OutputState::default();
        let mut queue = queue_with(&[0.1, 0.2]);
        let mut buffer = [9.0_f32; 6];

        fill_buffer(&mut buffer, 2, &mut queue, &state);

        assert_eq!(buffer, [0.1, 0.2, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(state.played_frames(), 1);
    }

    #[test]
    fn pausing_gives_silence_and_keeps_the_audio() {
        let state = OutputState::default();
        state.set_paused(true);
        let mut queue = queue_with(&[0.1, 0.2]);
        let mut buffer = [9.0_f32; 2];

        fill_buffer(&mut buffer, 2, &mut queue, &state);

        assert_eq!(buffer, [0.0, 0.0]);
        assert_eq!(state.played_frames(), 0);
        assert_eq!(queue.slots(), 2);
    }

    #[test]
    fn flushing_drops_queued_audio_and_restarts_the_clock() {
        let state = OutputState::default();
        state.played_frames.store(500, Ordering::Release);
        state.set_paused(true);
        state.request_flush();
        let mut queue = queue_with(&[0.1, 0.2, 0.3, 0.4]);
        let mut buffer = [9.0_f32; 2];

        fill_buffer(&mut buffer, 2, &mut queue, &state);

        assert!(queue.is_empty());
        assert_eq!(state.played_frames(), 0);
        assert!(!state.is_flushing());
    }

    #[test]
    fn one_channel_outputs_get_both_sides_mixed() {
        let state = OutputState::default();
        let mut queue = queue_with(&[0.2, 0.4]);
        let mut buffer = [9.0_f32; 1];

        fill_buffer(&mut buffer, 1, &mut queue, &state);

        assert!((buffer[0] - 0.3).abs() < 1e-6);
    }

    #[test]
    fn surround_outputs_use_the_front_speakers_only() {
        let state = OutputState::default();
        let mut queue = queue_with(&[0.2, 0.4]);
        let mut buffer = [9.0_f32; 6];

        fill_buffer(&mut buffer, 6, &mut queue, &state);

        assert_eq!(buffer, [0.2, 0.4, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(state.played_frames(), 1);
    }

    #[test]
    fn integer_sound_cards_receive_scaled_samples() {
        let state = OutputState::default();
        let mut queue = queue_with(&[1.0, -1.0]);
        let mut buffer = [0_i16; 2];

        fill_buffer(&mut buffer, 2, &mut queue, &state);

        assert_eq!(buffer[0], i16::MAX);
        assert!(buffer[1] <= -i16::MAX);
    }
}
