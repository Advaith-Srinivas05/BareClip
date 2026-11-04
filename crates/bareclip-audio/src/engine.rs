//! Plays the whole timeline's sound through one connection to the sound card. How much it has
//! played is the clock that everything on screen follows.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use bareclip_core::{
    playback::advance_from_audio,
    time::{TICKS_PER_SECOND, Ticks},
};
use rtrb::Producer;

use crate::{
    mix::{MixSpan, TimelineMix},
    output::{AudioOutput, OutputState},
};

/// How much is mixed at a time, in audio frames.
const MIX_FRAMES: usize = 1024;
const CHANNELS: usize = 2;
/// How far ahead clips about to start get opened, longer than opening one ever takes.
const PREPARE_AHEAD: Ticks = Ticks(TICKS_PER_SECOND);
/// How long to wait for the sound card to throw away what it has queued.
const FLUSH_TIMEOUT: Duration = Duration::from_millis(200);

enum Command {
    Start(Ticks),
    Spans(Vec<MixSpan>),
    Run(bool),
    Quit,
}

struct Worker {
    mix: TimelineMix,
    producer: Producer<f32>,
    state: Arc<OutputState>,
    ready: Arc<AtomicBool>,
    rate: u32,
    buses: Vec<Vec<f32>>,
    buffered_frames: usize,
    /// Sound is wanted, because something has said where to start from.
    started: bool,
    running: bool,
    problem: Arc<std::sync::Mutex<Option<String>>>,
}

impl Worker {
    fn start_at(&mut self, at: Ticks) {
        self.state.request_flush();
        let waiting = Instant::now();
        while self.state.is_flushing() && waiting.elapsed() < FLUSH_TIMEOUT {
            thread::sleep(Duration::from_millis(1));
        }
        self.mix.seek(at);
        self.buses[0].clear();
        self.buffered_frames = 0;
        self.started = true;
    }

    fn apply(&mut self, command: Command) -> bool {
        match command {
            Command::Start(at) => self.start_at(at),
            Command::Spans(spans) => self.mix.set_spans(spans),
            Command::Run(running) => self.running = running,
            Command::Quit => return false,
        }
        true
    }

    /// Does one small piece of work. Returns false when there is nothing to do yet.
    fn step(&mut self) -> bool {
        if !self.started {
            return false;
        }
        if !self.buses[0].is_empty() {
            let room = self.producer.slots() / CHANNELS * CHANNELS;
            if room == 0 {
                return false;
            }
            let count = room.min(self.buses[0].len());
            for sample in self.buses[0].drain(..count) {
                let _ = self.producer.push(sample);
            }
            self.buffered_frames += count / CHANNELS;
            // A tenth of a second of sound is enough to start without a stutter.
            if self.buffered_frames >= self.rate as usize / 10 {
                self.ready.store(true, Ordering::Release);
            }
            return true;
        }
        self.mix.prepare(PREPARE_AHEAD);
        self.mix.fill(MIX_FRAMES, &mut self.buses);
        if let Some(problem) = self.mix.take_problem()
            && let Ok(mut slot) = self.problem.lock()
        {
            *slot = Some(problem);
        }
        true
    }
}

fn work(mut worker: Worker, commands: &Receiver<Command>) {
    loop {
        while let Ok(command) = commands.try_recv() {
            if !worker.apply(command) {
                return;
            }
        }
        if worker.step() {
            continue;
        }
        // Nothing to do. With nothing playing either, this uses no time at all until told more.
        let next = if worker.running {
            match commands.recv_timeout(Duration::from_millis(5)) {
                Ok(command) => Some(command),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match commands.recv() {
                Ok(command) => Some(command),
                Err(_) => return,
            }
        };
        if let Some(command) = next
            && !worker.apply(command)
        {
            return;
        }
    }
}

/// An open connection to the sound card, playing the timeline.
pub struct SoundEngine {
    // Keeps the sound card connection open.
    _output: AudioOutput,
    state: Arc<OutputState>,
    commands: Sender<Command>,
    ready: Arc<AtomicBool>,
    rate: u32,
    problem: Arc<std::sync::Mutex<Option<String>>>,
    worker: Option<JoinHandle<()>>,
}

impl SoundEngine {
    /// Connects to the sound card, paused. Fails on computers with no output device.
    pub fn open() -> Result<Self, String> {
        let mut output = AudioOutput::open()?;
        let rate = output.sample_rate();
        let producer = output
            .take_producer()
            .ok_or_else(|| "The sound output is already in use.".to_owned())?;
        let state = output.state();
        let ready = Arc::new(AtomicBool::new(false));
        let problem = Arc::new(std::sync::Mutex::new(None));
        let (commands, receiver) = mpsc::channel();
        let worker_state = Worker {
            mix: TimelineMix::new(Vec::new(), rate, Ticks(0)),
            producer,
            state: state.clone(),
            ready: ready.clone(),
            rate,
            buses: vec![Vec::new()],
            buffered_frames: 0,
            started: false,
            running: false,
            problem: problem.clone(),
        };
        let worker = thread::Builder::new()
            .name("bareclip-sound".to_owned())
            .spawn(move || work(worker_state, &receiver))
            .map_err(|error| format!("The sound could not start: {error}"))?;
        Ok(Self {
            _output: output,
            state,
            commands,
            ready,
            rate,
            problem,
            worker: Some(worker),
        })
    }

    /// Gets ready to play from a moment, without starting yet.
    pub fn start_at(&self, at: Ticks) {
        self.state.set_paused(true);
        self.ready.store(false, Ordering::Release);
        let _ = self.commands.send(Command::Start(at));
        let _ = self.commands.send(Command::Run(true));
    }

    /// Whether there is enough sound waiting to start without a stutter.
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire) && !self.state.is_flushing()
    }

    pub fn resume(&self) {
        self.state.set_paused(false);
    }

    pub fn pause(&self) {
        self.state.set_paused(true);
        let _ = self.commands.send(Command::Run(false));
    }

    /// Changes what is being played. What has already gone to the sound card still plays, which
    /// is about a quarter of a second at most.
    pub fn set_spans(&self, spans: Vec<MixSpan>) {
        let _ = self.commands.send(Command::Spans(spans));
    }

    /// How much has been played since the last `start_at`.
    pub fn played(&self) -> Ticks {
        advance_from_audio(self.state.played_frames(), self.rate)
    }

    /// The most recent file that couldn't be read, once.
    pub fn take_problem(&self) -> Option<String> {
        self.problem.lock().ok()?.take()
    }
}

impl Drop for SoundEngine {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Quit);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as Process;

    use bareclip_core::{sound::ClipSound, timeline::TimeRange};
    use rtrb::RingBuffer;

    use crate::mix::Bus;

    const RATE: u32 = 48_000;

    fn level_file() -> std::path::PathBuf {
        let folder = std::env::temp_dir().join("bareclip-audio-tests");
        std::fs::create_dir_all(&folder).expect("the folder can be created");
        let path = folder.join(format!("engine-{}.wav", std::process::id()));
        let status = Process::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
            ])
            .arg(format!("aevalsrc=0.25|0.25:d=1:s={RATE}"))
            .args(["-c:a", "pcm_f32le"])
            .arg(&path)
            .status()
            .expect("ffmpeg is on PATH");
        assert!(status.success());
        path
    }

    fn worker_with(spans: Vec<MixSpan>) -> (Worker, rtrb::Consumer<f32>) {
        let (producer, consumer) = RingBuffer::new(RATE as usize * CHANNELS);
        let worker = Worker {
            mix: TimelineMix::new(spans, RATE, Ticks(0)),
            producer,
            state: Arc::new(OutputState::default()),
            ready: Arc::new(AtomicBool::new(false)),
            rate: RATE,
            buses: vec![Vec::new()],
            buffered_frames: 0,
            started: false,
            running: false,
            problem: Arc::new(std::sync::Mutex::new(None)),
        };
        (worker, consumer)
    }

    fn span(path: std::path::PathBuf, start: Ticks) -> MixSpan {
        MixSpan {
            path,
            start,
            source: TimeRange {
                start: Ticks(0),
                end: Ticks(TICKS_PER_SECOND),
            },
            sound: ClipSound::default(),
            bus: Bus::All(0),
        }
    }

    #[test]
    fn nothing_is_mixed_until_something_says_where_to_start() {
        let (mut worker, consumer) = worker_with(Vec::new());

        assert!(!worker.step());
        assert_eq!(consumer.slots(), 0);
    }

    #[test]
    fn it_becomes_ready_after_a_tenth_of_a_second_and_then_stops_when_full() {
        let (mut worker, consumer) = worker_with(vec![span(level_file(), Ticks(0))]);
        worker.started = true;

        let mut steps = 0;
        while worker.step() {
            steps += 1;
            assert!(steps < 10_000, "it never filled up");
        }

        assert!(worker.ready.load(Ordering::Acquire));
        assert_eq!(consumer.slots(), RATE as usize * CHANNELS);
    }

    #[test]
    fn the_sound_that_comes_out_is_the_timelines_in_order() {
        // Half a second of nothing, then the file.
        let (mut worker, mut consumer) =
            worker_with(vec![span(level_file(), Ticks(TICKS_PER_SECOND / 2))]);
        worker.started = true;
        while worker.step() {}

        let samples: Vec<f32> = std::iter::from_fn(|| consumer.pop().ok()).collect();

        assert_eq!(samples[0], 0.0);
        // Stereo, so three quarters of a second in is one and a half rates along.
        assert!((samples[RATE as usize * 3 / 2] - 0.25).abs() < 0.002);
    }

    #[test]
    fn starting_again_begins_from_the_new_moment_with_nothing_left_over() {
        let (mut worker, _consumer) = worker_with(Vec::new());
        worker.started = true;
        worker.step();
        assert!(!worker.buses[0].is_empty());

        // Nothing is running the sound card here, so its flush only times out.
        worker.start_at(Ticks(TICKS_PER_SECOND * 3));

        assert!(worker.buses[0].is_empty());
        assert_eq!(worker.buffered_frames, 0);
        assert_eq!(worker.mix.position(), i64::from(RATE) * 3);
    }

    #[test]
    fn a_new_list_of_clips_takes_over_from_where_it_has_got_to() {
        let (mut worker, _consumer) = worker_with(Vec::new());
        worker.started = true;
        worker.step();
        let position = worker.mix.position();

        assert!(worker.apply(Command::Spans(vec![span(level_file(), Ticks(0))])));

        assert_eq!(worker.mix.position(), position);
    }

    #[test]
    fn being_told_to_quit_stops_the_work() {
        let (mut worker, _consumer) = worker_with(Vec::new());

        assert!(!worker.apply(Command::Quit));
    }
}
