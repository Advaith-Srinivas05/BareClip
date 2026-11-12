//! Listens to one program's sound only, and the programs it started, through Windows' process
//! loopback, which came with Windows 10 version 2004. A game's sound can then be recorded without
//! a voice call or music playing beside it.
//!
//! What it hears goes into a queue as stereo at 48 kHz, like a microphone's, for a recording to
//! take from, and how loud it is goes to a meter.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use bareclip_audio::Level;
use rtrb::{Consumer, Producer, RingBuffer};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, HWND, WAIT_OBJECT_0},
        Media::Audio::{
            AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            AUDCLNT_STREAMFLAGS_LOOPBACK, AUDIOCLIENT_ACTIVATION_PARAMS,
            AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, ActivateAudioInterfaceAsync,
            IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
            IActivateAudioInterfaceCompletionHandler_Impl, IAudioCaptureClient, IAudioClient,
            PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, WAVEFORMATEX,
        },
        System::{
            Com::{
                BLOB, COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize,
                StructuredStorage::{
                    PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
                },
            },
            Threading::{CreateEventW, WaitForSingleObject},
            Variant::VT_BLOB,
        },
        UI::WindowsAndMessaging::GetWindowThreadProcessId,
    },
    core::{Interface, Ref, implement},
};

use crate::Source;

/// The rate the sound comes at. Windows changes the program's own rate to it.
pub const APP_SOUND_RATE: u32 = 48_000;
const CHANNELS: usize = 2;
/// How much sound the queue holds before the oldest is lost, in seconds.
const QUEUE_SECONDS: usize = 4;
/// How long Windows may take to start listening.
const ACTIVATE_WAIT: Duration = Duration::from_secs(5);
/// Windows' buffer, in its units of 100 nanoseconds: 20 ms.
const BUFFER_LENGTH: i64 = 200_000;
/// How long the listening thread waits for sound before looking whether it should stop, in ms.
const WAIT_MS: u32 = 100;
/// Windows' code for sound held as 32-bit floating point numbers.
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;

/// The program that owns a window, by its process id, or `None` for a screen or a window that's
/// gone.
pub fn window_process(source: Source) -> Option<u32> {
    let Source::Window(handle) = source else {
        return None;
    };
    let mut process = 0;
    // SAFETY: Windows only reads the handle, and says 0 for one that's gone.
    let thread = unsafe { GetWindowThreadProcessId(HWND(handle as _), Some(&raw mut process)) };
    (thread != 0 && process != 0).then_some(process)
}

/// One program's sound being listened to. Dropping it stops listening.
pub struct AppSound {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    level: Arc<Level>,
    queue: Option<Consumer<f32>>,
}

impl AppSound {
    /// Starts listening to the program that owns a window.
    pub fn for_window(source: Source) -> Result<Self, String> {
        let process = window_process(source)
            .ok_or_else(|| "The window's program could not be found.".to_owned())?;
        Self::open(process)
    }

    /// Starts listening to a program and the programs it started, by its process id.
    pub fn open(process: u32) -> Result<Self, String> {
        let (producer, consumer) =
            RingBuffer::new(APP_SOUND_RATE as usize * QUEUE_SECONDS * CHANNELS);
        let level = Arc::new(Level::default());
        let stop = Arc::new(AtomicBool::new(false));
        let (started, ready) = mpsc::channel();
        let worker = {
            let level = level.clone();
            let stop = stop.clone();
            thread::spawn(move || listen(process, producer, &level, &stop, &started))
        };
        match ready.recv() {
            Ok(Ok(())) => Ok(Self {
                stop,
                worker: Some(worker),
                level,
                queue: Some(consumer),
            }),
            Ok(Err(problem)) => {
                let _ = worker.join();
                Err(problem)
            }
            Err(_) => {
                let _ = worker.join();
                Err("The program's sound could not be listened to.".to_owned())
            }
        }
    }

    pub fn sample_rate(&self) -> u32 {
        APP_SOUND_RATE
    }

    pub fn level(&self) -> Arc<Level> {
        self.level.clone()
    }

    /// Hands over the end of the queue the sound comes out of.
    pub fn take_queue(&mut self) -> Option<Consumer<f32>> {
        self.queue.take()
    }
}

impl Drop for AppSound {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Told by Windows once listening has been set up, or has failed to be.
#[implement(IActivateAudioInterfaceCompletionHandler)]
struct Activated(Mutex<Option<mpsc::Sender<()>>>);

impl IActivateAudioInterfaceCompletionHandler_Impl for Activated_Impl {
    fn ActivateCompleted(
        &self,
        _operation: Ref<IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        if let Some(done) = self.0.lock().ok().and_then(|mut done| done.take()) {
            let _ = done.send(());
        }
        Ok(())
    }
}

/// Windows' sound client for one program's sound.
fn activate(process: u32) -> Result<IAudioClient, String> {
    let failed = |error: windows::core::Error| {
        format!(
            "Only this app's sound can't be recorded on this PC. {}",
            error.message()
        )
    };
    let params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: process,
                ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
            },
        },
    };
    // The settings travel as a blob pointing at `params`, which outlives the call. The blob is
    // never dropped, because dropping it would free `params` as if Windows had made it.
    let variant = std::mem::ManuallyDrop::new(PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_BLOB,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: PROPVARIANT_0_0_0 {
                    blob: BLOB {
                        cbSize: size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                        pBlobData: (&raw const params).cast_mut().cast(),
                    },
                },
            }),
        },
    });
    let (done, finished) = mpsc::channel();
    let handler: IActivateAudioInterfaceCompletionHandler =
        Activated(Mutex::new(Some(done))).into();
    // SAFETY: everything handed over lives until Windows has answered or the wait gives up, and
    // the handler is a COM object Windows holds its own reference to.
    let operation = unsafe {
        ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&raw const *variant),
            &handler,
        )
    }
    .map_err(failed)?;
    finished
        .recv_timeout(ACTIVATE_WAIT)
        .map_err(|_| "Windows took too long to start listening to the program.".to_owned())?;
    let mut result = windows::core::HRESULT(0);
    let mut client = None;
    // SAFETY: the operation has finished, so its result is ready.
    unsafe { operation.GetActivateResult(&raw mut result, &raw mut client) }.map_err(failed)?;
    result.ok().map_err(failed)?;
    client
        .ok_or_else(|| "Windows gave no sound client for the program.".to_owned())?
        .cast()
        .map_err(failed)
}

/// Listens on this thread until told to stop, saying first whether it could start.
fn listen(
    process: u32,
    mut producer: Producer<f32>,
    level: &Level,
    stop: &AtomicBool,
    started: &mpsc::Sender<Result<(), String>>,
) {
    // SAFETY: COM is set up for this thread and taken down at its end.
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    match open_client(process) {
        Ok((client, capture, event)) => {
            let _ = started.send(Ok(()));
            take_sound(&capture, event, &mut producer, level, stop);
            // SAFETY: the client and event belong to this thread.
            unsafe {
                let _ = client.Stop();
                let _ = CloseHandle(event);
            }
        }
        Err(problem) => {
            let _ = started.send(Err(problem));
        }
    }
    if com {
        // SAFETY: matches the set-up above, after every COM object here is gone.
        unsafe { CoUninitialize() };
    }
}

/// Starts the client listening, as stereo floating point at 48 kHz, with an event Windows sets
/// whenever sound is waiting.
fn open_client(process: u32) -> Result<(IAudioClient, IAudioCaptureClient, HANDLE), String> {
    let client = activate(process)?;
    let block = (CHANNELS * size_of::<f32>()) as u16;
    let format = WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
        nChannels: CHANNELS as u16,
        nSamplesPerSec: APP_SOUND_RATE,
        nAvgBytesPerSec: APP_SOUND_RATE * u32::from(block),
        nBlockAlign: block,
        wBitsPerSample: 32,
        cbSize: 0,
    };
    let failed = |error: windows::core::Error| {
        format!(
            "The program's sound could not be listened to. {}",
            error.message()
        )
    };
    // SAFETY: the format lives through the call, and the event is closed by whoever gets it.
    unsafe {
        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK
                    | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                    | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
                BUFFER_LENGTH,
                0,
                &raw const format,
                None,
            )
            .map_err(failed)?;
        let event = CreateEventW(None, false, false, None).map_err(failed)?;
        let opened = client
            .SetEventHandle(event)
            .and_then(|()| client.GetService::<IAudioCaptureClient>())
            .and_then(|capture| client.Start().map(|()| capture));
        match opened {
            Ok(capture) => Ok((client, capture, event)),
            Err(error) => {
                let _ = CloseHandle(event);
                Err(failed(error))
            }
        }
    }
}

/// Moves each packet of sound into the queue as it comes, until told to stop.
fn take_sound(
    capture: &IAudioCaptureClient,
    event: HANDLE,
    producer: &mut Producer<f32>,
    level: &Level,
    stop: &AtomicBool,
) {
    let mut heard: Vec<f32> = Vec::new();
    while !stop.load(Ordering::Acquire) {
        // SAFETY: the event lives until this loop ends.
        if unsafe { WaitForSingleObject(event, WAIT_MS) } != WAIT_OBJECT_0 {
            continue;
        }
        loop {
            // SAFETY: each buffer is read whole and handed back before the next is asked for.
            let packet = unsafe { capture.GetNextPacketSize() };
            if !matches!(packet, Ok(frames) if frames > 0) {
                break;
            }
            let mut data = std::ptr::null_mut();
            let mut frames = 0;
            let mut flags = 0;
            // SAFETY: as above.
            if unsafe {
                capture.GetBuffer(&raw mut data, &raw mut frames, &raw mut flags, None, None)
            }
            .is_err()
            {
                break;
            }
            let samples = frames as usize * CHANNELS;
            heard.clear();
            if flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 || data.is_null() {
                heard.resize(samples, 0.0);
            } else {
                // SAFETY: Windows gives `frames` whole frames of the format asked for.
                heard.extend_from_slice(unsafe {
                    std::slice::from_raw_parts(data.cast::<f32>(), samples)
                });
            }
            // SAFETY: the whole buffer has been read.
            let _ = unsafe { capture.ReleaseBuffer(frames) };
            level.note(&heard);
            // Whole frames only, so the two sides never swap. A full queue means nobody is
            // taking sound, as while the recording bar only shows the meter.
            let room = producer.slots() / CHANNELS * CHANNELS;
            for sample in heard.iter().take(room) {
                let _ = producer.push(*sample);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_screen_has_no_program() {
        assert_eq!(window_process(Source::Screen(1)), None);
    }

    #[test]
    fn a_window_that_is_gone_has_no_program() {
        assert_eq!(window_process(Source::Window(0)), None);
        assert!(AppSound::for_window(Source::Window(0)).is_err());
    }

    /// Plays a quiet tone from this test program for a while.
    fn play_a_tone(for_how_long: Duration) {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let device = cpal::default_host()
            .default_output_device()
            .expect("there is an output");
        let config = device.default_output_config().expect("it has a format");
        let channels = usize::from(config.channels());
        let rate = config.sample_rate() as f32;
        let mut phase = 0.0_f32;
        let stream = device
            .build_output_stream(
                config.into(),
                move |data: &mut [f32], _| {
                    for frame in data.chunks_mut(channels) {
                        // Quiet, so whoever runs it isn't startled.
                        let value = (phase * std::f32::consts::TAU).sin() * 0.05;
                        phase = (phase + 440.0 / rate).fract();
                        frame.fill(value);
                    }
                },
                |_| {},
                None,
            )
            .expect("the tone can play");
        stream.play().expect("it plays");
        thread::sleep(for_how_long);
    }

    /// How many samples came and the loudest of them.
    fn heard(queue: &mut Consumer<f32>) -> (usize, f32) {
        let mut loudest = 0.0_f32;
        let mut count = 0;
        while let Ok(sample) = queue.pop() {
            loudest = loudest.max(sample.abs());
            count += 1;
        }
        (count, loudest)
    }

    #[test]
    #[ignore = "needs a sound output, which GitHub's runners may not have"]
    fn another_programs_sound_is_left_out() {
        // A program that makes no sound, listened to while this one plays a tone.
        let mut quiet = std::process::Command::new("ping")
            .args(["-n", "4", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("it starts");
        let mut sound = AppSound::open(quiet.id()).expect("it opens");
        let mut queue = sound.take_queue().expect("there is a queue");
        play_a_tone(Duration::from_millis(800));
        let (_, loudest) = heard(&mut queue);
        let _ = quiet.kill();
        let _ = quiet.wait();
        assert!(loudest < 0.001, "heard {loudest} from another program");
    }

    /// Plays a tone from this test program and hears it through its own process loopback.
    #[test]
    #[ignore = "needs a sound output, which GitHub's runners may not have"]
    fn a_programs_own_sound_is_heard() {
        let mut sound = AppSound::open(std::process::id()).expect("it opens");
        let mut queue = sound.take_queue().expect("there is a queue");
        play_a_tone(Duration::from_millis(800));
        let (count, loudest) = heard(&mut queue);
        assert!(count > APP_SOUND_RATE as usize / 4, "{count} samples heard");
        assert!(loudest > 0.02, "loudest {loudest}");
    }
}
