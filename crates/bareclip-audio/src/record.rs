//! Records sound from a queue, such as a microphone's, into an M4A that can be read at any moment
//! while it is written, so a crash keeps what was recorded.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use bareclip_media::{MediaError, RateChange, SOUND_RATE, SoundFile, SoundLabel};
use rtrb::Consumer;

const CHANNELS: usize = 2;
/// How often waiting sound is taken from the queue and written.
const WRITE_EVERY: Duration = Duration::from_millis(20);

/// Shared between a recording and the thread writing it.
#[derive(Debug, Default)]
struct Control {
    paused: AtomicBool,
    stop: AtomicBool,
    /// Frames written to the file, at `SOUND_RATE`.
    written: AtomicU64,
}

/// What a finished recording holds.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordingReport {
    pub path: PathBuf,
    pub seconds: f64,
}

/// A recording in progress. Dropping it without stopping finishes the file anyway.
pub struct SoundRecording {
    control: Arc<Control>,
    worker: Option<JoinHandle<Result<RecordingReport, MediaError>>>,
}

impl SoundRecording {
    /// Starts writing what comes out of `queue`, stereo at `rate`, to `path`. Anything already
    /// waiting in the queue was heard before this started, so it is left out. Fails if the file
    /// can't be made.
    pub fn start(
        path: &Path,
        mut queue: Consumer<f32>,
        rate: u32,
        label: SoundLabel,
    ) -> Result<Self, MediaError> {
        let control = Arc::new(Control::default());
        let (ready, started) = mpsc::channel();
        let path = path.to_owned();
        let shared = control.clone();
        let worker = thread::spawn(move || {
            let made = SoundFile::create_growing(&path, &[label]).and_then(|file| {
                let change = (rate != SOUND_RATE)
                    .then(|| RateChange::new(rate, SOUND_RATE))
                    .transpose()?;
                Ok((file, change))
            });
            let (mut file, mut change) = match made {
                Ok(made) => {
                    while queue.pop().is_ok() {}
                    let _ = ready.send(Ok(()));
                    made
                }
                Err(error) => {
                    let _ = ready.send(Err(error));
                    return Err(MediaError::Cancelled);
                }
            };
            let mut heard = Vec::new();
            loop {
                let stopping = shared.stop.load(Ordering::Acquire);
                heard.clear();
                while let Ok(sample) = queue.pop() {
                    heard.push(sample);
                }
                // A paused stretch is thrown away, so the file has no gap.
                if !shared.paused.load(Ordering::Acquire) && !heard.is_empty() {
                    let samples = match &mut change {
                        Some(change) => change.run(&heard)?,
                        None => heard.clone(),
                    };
                    write(&mut file, &samples, &shared)?;
                }
                if stopping {
                    break;
                }
                thread::sleep(WRITE_EVERY);
            }
            if let Some(change) = &mut change {
                let rest = change.finish()?;
                write(&mut file, &rest, &shared)?;
            }
            file.finish()?;
            Ok(RecordingReport {
                path,
                seconds: shared.written.load(Ordering::Acquire) as f64 / f64::from(SOUND_RATE),
            })
        });
        match started.recv() {
            Ok(Ok(())) => Ok(Self {
                control,
                worker: Some(worker),
            }),
            Ok(Err(error)) => {
                let _ = worker.join();
                Err(error)
            }
            Err(_) => match worker.join() {
                Ok(Err(error)) => Err(error),
                _ => Err(MediaError::Cancelled),
            },
        }
    }

    pub fn set_paused(&self, paused: bool) {
        self.control.paused.store(paused, Ordering::Release);
    }

    pub fn is_paused(&self) -> bool {
        self.control.paused.load(Ordering::Acquire)
    }

    /// How long the recording is so far, pauses left out.
    pub fn seconds(&self) -> f64 {
        self.control.written.load(Ordering::Acquire) as f64 / f64::from(SOUND_RATE)
    }

    /// Writes what is still waiting and finishes the file.
    pub fn stop(mut self) -> Result<RecordingReport, MediaError> {
        self.finish()
    }

    fn finish(&mut self) -> Result<RecordingReport, MediaError> {
        self.control.stop.store(true, Ordering::Release);
        match self.worker.take().map(JoinHandle::join) {
            Some(Ok(result)) => result,
            _ => Err(MediaError::Cancelled),
        }
    }
}

impl Drop for SoundRecording {
    fn drop(&mut self) {
        if self.worker.is_some() {
            let _ = self.finish();
        }
    }
}

fn write(file: &mut SoundFile, samples: &[f32], control: &Control) -> Result<(), MediaError> {
    let whole = samples.len() / CHANNELS * CHANNELS;
    if whole == 0 {
        return Ok(());
    }
    file.write(0, &samples[..whole])?;
    control
        .written
        .fetch_add((whole / CHANNELS) as u64, Ordering::AcqRel);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtrb::{Producer, RingBuffer};
    use std::{
        fs,
        process::{Command, Stdio},
        time::Instant,
    };

    fn folder(name: &str) -> PathBuf {
        let folder = std::env::temp_dir()
            .join("bareclip-record-tests")
            .join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).expect("the folder can be made");
        folder
    }

    fn label() -> SoundLabel {
        SoundLabel {
            title: Some("Microphone".to_owned()),
            language: None,
        }
    }

    /// Pushes `seconds` of a quiet tone at `rate`, then waits until the recording has taken it.
    fn feed(producer: &mut Producer<f32>, rate: u32, seconds: f64, capacity: usize) {
        let frames = (seconds * f64::from(rate)) as usize;
        for frame in 0..frames {
            let sample = (frame as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.3;
            while producer.slots() < CHANNELS {
                thread::sleep(Duration::from_millis(1));
            }
            let _ = producer.push(sample);
            let _ = producer.push(sample);
        }
        let started = Instant::now();
        while producer.slots() < capacity {
            assert!(started.elapsed() < Duration::from_secs(10), "never taken");
            thread::sleep(Duration::from_millis(2));
        }
        // Long enough for the taken sound to be written.
        thread::sleep(WRITE_EVERY * 3);
    }

    fn length_of(path: &Path) -> f64 {
        let info = bareclip_media::probe(path).expect("the file can be read");
        assert_eq!(info.audio.len(), 1);
        assert_eq!(info.audio[0].codec, "aac");
        assert!(info.is_sound_only());
        info.duration.as_seconds()
    }

    #[test]
    fn a_recording_keeps_what_it_heard_at_any_rate() {
        for rate in [44_100, 48_000] {
            let path = folder("rates").join(format!("at-{rate}.m4a"));
            let capacity = rate as usize * CHANNELS;
            let (mut producer, consumer) = RingBuffer::new(capacity);
            // Heard before recording started, so left out.
            for _ in 0..rate {
                let _ = producer.push(0.5);
                let _ = producer.push(0.5);
            }
            let recording =
                SoundRecording::start(&path, consumer, rate, label()).expect("it starts");
            feed(&mut producer, rate, 2.0, capacity);
            let report = recording.stop().expect("it finishes");
            assert!((report.seconds - 2.0).abs() < 0.03, "{report:?}");
            let length = length_of(&path);
            assert!((length - 2.0).abs() < 0.05, "{rate}: {length}");
        }
    }

    #[test]
    fn a_paused_stretch_leaves_no_gap() {
        let path = folder("paused").join("paused.m4a");
        let capacity = 48_000 * CHANNELS;
        let (mut producer, consumer) = RingBuffer::new(capacity);
        let recording = SoundRecording::start(&path, consumer, 48_000, label()).expect("it starts");
        feed(&mut producer, 48_000, 1.0, capacity);
        recording.set_paused(true);
        assert!(recording.is_paused());
        feed(&mut producer, 48_000, 1.0, capacity);
        recording.set_paused(false);
        feed(&mut producer, 48_000, 1.0, capacity);
        assert!((recording.seconds() - 2.0).abs() < 0.03);
        recording.stop().expect("it finishes");
        let length = length_of(&path);
        assert!((length - 2.0).abs() < 0.05, "{length}");
    }

    #[test]
    fn a_file_that_cant_be_made_says_so_at_once() {
        let (_, consumer) = RingBuffer::new(16);
        let path = Path::new(r"Z:\no\such\folder\here.m4a");
        assert!(SoundRecording::start(path, consumer, 48_000, label()).is_err());
    }

    const HELPER_FOLDER: &str = "BARECLIP_RECORDING_HELPER";

    /// Not a test itself: `killing_the_app_mid_recording_keeps_what_was_recorded` runs it as
    /// another process and kills it.
    #[test]
    #[ignore = "only runs as a helper of another test"]
    fn record_until_killed() {
        let Some(folder) = std::env::var_os(HELPER_FOLDER) else {
            return;
        };
        let folder = PathBuf::from(folder);
        let capacity = 48_000 * CHANNELS;
        let (mut producer, consumer) = RingBuffer::new(capacity);
        let recording =
            SoundRecording::start(&folder.join("cut short.m4a"), consumer, 48_000, label())
                .expect("it starts");
        // Sound arrives half a second at a time, each piece written before the next comes.
        for second in 0..60 {
            feed(&mut producer, 48_000, 0.5, capacity);
            if second == 6 {
                fs::write(folder.join("ready"), "").expect("it can be written");
            }
        }
        drop(recording);
    }

    #[test]
    fn killing_the_app_mid_recording_keeps_what_was_recorded() {
        let folder = folder("killed");
        let mut helper = Command::new(std::env::current_exe().expect("the test knows its path"))
            .args(["--ignored", "--exact", "record::tests::record_until_killed"])
            .env(HELPER_FOLDER, &folder)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the helper starts");
        let started = Instant::now();
        while !folder.join("ready").exists() {
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "it never got going"
            );
            thread::sleep(Duration::from_millis(10));
        }
        helper.kill().expect("the helper can be killed");
        helper.wait().expect("the helper ends");

        // About 3.5 seconds were heard. Up to the last piece may be lost.
        let length = length_of(&folder.join("cut short.m4a"));
        assert!(length > 2.0 && length < 4.5, "{length}");
    }
}
