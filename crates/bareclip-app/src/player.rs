//! Plays a video using a thread each for picture and sound, with the picture
//! following the sound card's clock.

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use bareclip_audio::{AudioOutput, OutputState, Producer, TrackMixer};
use bareclip_core::{
    playback::{FrameQueue, advance_from_audio, advance_from_time},
    time::{Ticks, TimeBase},
};
use bareclip_media::{
    AudioPreview, AudioStep, DecodeStep, MediaError, MediaInfo, PreviewFrame, VideoPreview,
};

/// Preview pictures are reduced to this size, which is enough for the stage and
/// keeps each frame cheap to convert and upload.
const PREVIEW_WIDTH: u32 = 1280;
const PREVIEW_HEIGHT: u32 = 720;
/// Decoded frames waiting between the decoder and the screen.
const FRAME_CHANNEL: usize = 6;
const QUEUE_LIMIT: usize = 6;
/// How long playback waits for sound before starting without it.
const START_TIMEOUT: Duration = Duration::from_millis(400);
/// A chunk of mixed sound, in audio frames.
const MIX_FRAMES: usize = 1024;
const FLUSH_TIMEOUT: Duration = Duration::from_millis(200);

enum VideoCommand {
    Seek { pts: i64, generation: u64 },
    Play,
    Pause,
    Quit,
}

enum VideoEvent {
    Frame {
        generation: u64,
        frame: PreviewFrame,
        /// False for the first picture shown while still decoding toward a seek.
        exact: bool,
    },
    End {
        generation: u64,
    },
    Error(String),
}

/// A notification that something is ready to be drawn.
pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// Sends an event while still listening for commands, so a full queue never stops
/// the worker from reacting. Returns false if a command arrived, dropping the event.
fn deliver(
    mut event: VideoEvent,
    events: &SyncSender<VideoEvent>,
    commands: &Receiver<VideoCommand>,
    pending: &mut VecDeque<VideoCommand>,
    notify: &Notify,
) -> bool {
    loop {
        match events.try_send(event) {
            Ok(()) => {
                notify();
                return true;
            }
            Err(TrySendError::Full(returned)) => event = returned,
            Err(TrySendError::Disconnected(_)) => return false,
        }
        if let Ok(command) = commands.try_recv() {
            pending.push_back(command);
            return false;
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn video_worker(
    path: &Path,
    commands: &Receiver<VideoCommand>,
    events: &SyncSender<VideoEvent>,
    notify: &Notify,
) {
    let mut preview = match VideoPreview::open(path, PREVIEW_WIDTH, PREVIEW_HEIGHT) {
        Ok(preview) => preview,
        Err(error) => {
            let _ = events.send(VideoEvent::Error(error.to_string()));
            notify();
            return;
        }
    };

    let mut pending = VecDeque::new();
    let mut generation = 0;
    let mut seek_target: Option<i64> = None;
    let mut first_after_seek = false;
    let mut playing = false;

    loop {
        // Only the newest command of a run of seeks matters, which keeps scrubbing live.
        let idle = seek_target.is_none() && !playing && pending.is_empty();
        if idle {
            match commands.recv() {
                Ok(command) => pending.push_back(command),
                Err(_) => return,
            }
        }
        while let Ok(command) = commands.try_recv() {
            pending.push_back(command);
        }
        let mut newest_seek = None;
        while let Some(command) = pending.pop_front() {
            match command {
                VideoCommand::Seek {
                    pts,
                    generation: new_generation,
                } => newest_seek = Some((pts, new_generation)),
                VideoCommand::Play => playing = true,
                VideoCommand::Pause => playing = false,
                VideoCommand::Quit => return,
            }
        }
        if let Some((pts, new_generation)) = newest_seek {
            generation = new_generation;
            if let Err(error) = preview.seek(pts) {
                let _ = events.send(VideoEvent::Error(error.to_string()));
                notify();
            }
            seek_target = Some(pts);
            first_after_seek = true;
        }

        if seek_target.is_none() && !playing {
            continue;
        }

        // The first picture after a seek shows immediately, however far the exact
        // frame is, so that dragging along the timeline feels responsive.
        let show_from = seek_target.filter(|_| !first_after_seek);
        match preview.next_step(show_from) {
            Ok(DecodeStep::Frame(frame)) => {
                let exact = seek_target.is_none_or(|target| frame.pts >= target);
                first_after_seek = false;
                if exact {
                    seek_target = None;
                }
                deliver(
                    VideoEvent::Frame {
                        generation,
                        frame,
                        exact,
                    },
                    events,
                    commands,
                    &mut pending,
                    notify,
                );
            }
            Ok(DecodeStep::Skipped { .. }) => {}
            Ok(DecodeStep::End) => {
                seek_target = None;
                playing = false;
                deliver(
                    VideoEvent::End { generation },
                    events,
                    commands,
                    &mut pending,
                    notify,
                );
            }
            Err(error) => {
                seek_target = None;
                playing = false;
                let _ = events.send(VideoEvent::Error(error.to_string()));
                notify();
            }
        }
    }
}

enum AudioCommand {
    Seek(f64),
    Quit,
}

struct AudioWorker {
    audio: AudioPreview,
    mixer: TrackMixer,
    producer: Producer<f32>,
    state: Arc<OutputState>,
    ready: Arc<AtomicBool>,
    sample_rate: u32,
    mixed: Vec<f32>,
    buffered_frames: usize,
    decoded_to_end: bool,
    active: bool,
}

impl AudioWorker {
    fn seek(&mut self, seconds: f64) -> Result<(), MediaError> {
        self.state.request_flush();
        let started = Instant::now();
        while self.state.is_flushing() && started.elapsed() < FLUSH_TIMEOUT {
            thread::sleep(Duration::from_millis(1));
        }
        self.audio.seek(seconds)?;
        let position = (seconds * f64::from(self.sample_rate)).round() as i64;
        self.mixer = TrackMixer::new(self.audio.track_count(), position);
        self.mixed.clear();
        self.buffered_frames = 0;
        self.decoded_to_end = false;
        self.active = true;
        Ok(())
    }

    /// Does one small piece of work. Returns false when there is nothing to do yet.
    fn step(&mut self) -> bool {
        if !self.mixed.is_empty() {
            let room = self.producer.slots() / 2 * 2;
            if room == 0 {
                return false;
            }
            let count = room.min(self.mixed.len());
            for sample in self.mixed.drain(..count) {
                let _ = self.producer.push(sample);
            }
            self.buffered_frames += count / 2;
            // A tenth of a second of sound is enough to start without a stutter.
            if self.buffered_frames >= self.sample_rate as usize / 10 {
                self.ready.store(true, Ordering::Release);
            }
            return true;
        }

        if self.mixer.available() > 0 {
            self.mixer.pull(MIX_FRAMES, &mut self.mixed);
            return true;
        }

        if self.mixer.is_finished() {
            self.ready.store(true, Ordering::Release);
            return false;
        }

        if self.decoded_to_end {
            for track in 0..self.audio.track_count() {
                self.mixer.finish(track);
            }
            return true;
        }

        match self.audio.next_step() {
            Ok(AudioStep::Chunk(chunk)) => {
                self.mixer
                    .push(chunk.track, chunk.start_frame, &chunk.samples);
            }
            Ok(AudioStep::Pending) => {}
            Ok(AudioStep::End) | Err(_) => self.decoded_to_end = true,
        }
        true
    }
}

fn audio_worker(mut worker: AudioWorker, commands: &Receiver<AudioCommand>) {
    loop {
        let command = if worker.active {
            match commands.try_recv() {
                Ok(command) => Some(command),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        } else {
            match commands.recv() {
                Ok(command) => Some(command),
                Err(_) => return,
            }
        };

        // Only the newest seek matters.
        let mut newest = command;
        while let Ok(next) = commands.try_recv() {
            newest = Some(next);
        }
        match newest {
            Some(AudioCommand::Quit) => return,
            Some(AudioCommand::Seek(seconds)) if worker.seek(seconds).is_err() => {
                worker.decoded_to_end = true;
            }
            _ => {}
        }

        if worker.active && !worker.step() {
            match commands.recv_timeout(Duration::from_millis(5)) {
                Ok(AudioCommand::Quit) | Err(RecvTimeoutError::Disconnected) => return,
                Ok(AudioCommand::Seek(seconds)) => {
                    if worker.seek(seconds).is_err() {
                        worker.decoded_to_end = true;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }
}

struct AudioLink {
    // Keeps the sound card connection open.
    _output: AudioOutput,
    state: Arc<OutputState>,
    commands: Sender<AudioCommand>,
    ready: Arc<AtomicBool>,
    sample_rate: u32,
    worker: Option<JoinHandle<()>>,
}

fn connect_audio(path: PathBuf) -> Option<AudioLink> {
    let mut output = AudioOutput::open().ok()?;
    let sample_rate = output.sample_rate();
    let audio = AudioPreview::open(&path, sample_rate).ok()?;
    if audio.track_count() == 0 {
        return None;
    }
    let producer = output.take_producer()?;
    let state = output.state();
    let ready = Arc::new(AtomicBool::new(false));
    let (commands, receiver) = mpsc::channel();

    let worker_state = AudioWorker {
        mixer: TrackMixer::new(audio.track_count(), 0),
        audio,
        producer,
        state: state.clone(),
        ready: ready.clone(),
        sample_rate,
        mixed: Vec::new(),
        buffered_frames: 0,
        decoded_to_end: false,
        active: false,
    };
    let worker = thread::Builder::new()
        .name("bareclip-audio".to_owned())
        .spawn(move || audio_worker(worker_state, &receiver))
        .ok()?;

    Some(AudioLink {
        _output: output,
        state,
        commands,
        ready,
        sample_rate,
        worker: Some(worker),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlayState {
    Paused,
    /// Waiting for the first picture and enough sound.
    Starting,
    Playing,
}

pub struct Player {
    commands: Sender<VideoCommand>,
    events: Receiver<VideoEvent>,
    worker: Option<JoinHandle<()>>,
    audio: Option<AudioLink>,
    queue: FrameQueue<PreviewFrame>,
    time_base: TimeBase,
    duration: Ticks,
    generation: u64,
    /// The position when playback last started or was moved.
    base: Ticks,
    state: PlayState,
    exact_frame_ready: bool,
    video_ended: bool,
    start_requested: Instant,
    silent_clock_start: Option<Instant>,
    error: Option<String>,
}

impl Player {
    /// Opens a video for playback. `use_sound` can be off where no sound is wanted.
    pub fn open(info: &MediaInfo, use_sound: bool, notify: Notify) -> Result<Self, String> {
        let path = info.path.clone();
        let (commands, command_receiver) = mpsc::channel();
        let (event_sender, events) = mpsc::sync_channel(FRAME_CHANNEL);
        let worker_path = path.clone();
        let worker = thread::Builder::new()
            .name("bareclip-video".to_owned())
            .spawn(move || video_worker(&worker_path, &command_receiver, &event_sender, &notify))
            .map_err(|error| format!("The video decoder could not start: {error}"))?;

        let audio = if use_sound && !info.audio.is_empty() {
            connect_audio(path)
        } else {
            None
        };

        Ok(Self {
            commands,
            events,
            worker: Some(worker),
            audio,
            queue: FrameQueue::default(),
            time_base: info.video.time_base,
            duration: info.duration,
            generation: 0,
            base: Ticks(0),
            state: PlayState::Paused,
            exact_frame_ready: false,
            video_ended: false,
            start_requested: Instant::now(),
            silent_clock_start: None,
            error: None,
        })
    }

    pub fn is_playing(&self) -> bool {
        self.state != PlayState::Paused
    }

    /// Whether sound is going to the sound card.
    pub fn has_sound(&self) -> bool {
        self.audio.is_some()
    }

    pub fn duration(&self) -> Ticks {
        self.duration
    }

    /// The most recent problem, once. Playback continues as far as it can.
    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    /// Where playback is right now.
    pub fn position(&self) -> Ticks {
        if self.state != PlayState::Playing {
            return self.base;
        }
        let advance = match &self.audio {
            Some(audio) => advance_from_audio(audio.state.played_frames(), audio.sample_rate),
            None => self
                .silent_clock_start
                .map_or(Ticks(0), |start| advance_from_time(start.elapsed())),
        };
        Ticks((self.base.0 + advance.0).min(self.duration.0))
    }

    fn send_seek(&mut self) {
        self.generation += 1;
        self.queue.clear();
        self.exact_frame_ready = false;
        self.video_ended = false;
        let _ = self.commands.send(VideoCommand::Seek {
            pts: self.base.to_pts(self.time_base),
            generation: self.generation,
        });
    }

    fn start_sound_from_base(&mut self) {
        if let Some(audio) = &self.audio {
            audio.state.set_paused(true);
            audio.ready.store(false, Ordering::Release);
            let _ = audio
                .commands
                .send(AudioCommand::Seek(self.base.as_seconds()));
        }
    }

    /// Moves to a time. Playback carries on from there if it was running.
    pub fn seek(&mut self, time: Ticks) {
        self.base = time.clamp(Ticks(0), self.duration);
        self.send_seek();
        if self.is_playing() {
            self.state = PlayState::Starting;
            self.start_requested = Instant::now();
            self.silent_clock_start = None;
            self.start_sound_from_base();
        }
    }

    pub fn play(&mut self) {
        if self.is_playing() {
            return;
        }
        if self.base >= self.duration {
            self.base = Ticks(0);
        }
        self.state = PlayState::Starting;
        self.start_requested = Instant::now();
        self.silent_clock_start = None;
        self.send_seek();
        let _ = self.commands.send(VideoCommand::Play);
        self.start_sound_from_base();
    }

    pub fn pause(&mut self) {
        if !self.is_playing() {
            return;
        }
        self.base = self.position();
        self.state = PlayState::Paused;
        self.queue.clear();
        let _ = self.commands.send(VideoCommand::Pause);
        if let Some(audio) = &self.audio {
            audio.state.set_paused(true);
        }
    }

    pub fn toggle(&mut self) {
        if self.is_playing() {
            self.pause();
        } else {
            self.play();
        }
    }

    fn sound_is_ready(&self) -> bool {
        self.audio
            .as_ref()
            .is_none_or(|audio| audio.ready.load(Ordering::Acquire))
    }

    /// Takes in decoded frames and returns the picture to show now, if it changed.
    pub fn update(&mut self) -> Option<PreviewFrame> {
        let mut newest_paused = None;
        while self.state == PlayState::Paused || self.queue.len() < QUEUE_LIMIT {
            let Ok(event) = self.events.try_recv() else {
                break;
            };
            match event {
                VideoEvent::Frame {
                    generation,
                    frame,
                    exact,
                } if generation == self.generation => {
                    if self.state == PlayState::Paused {
                        newest_paused = Some(frame);
                    } else if exact {
                        self.exact_frame_ready = true;
                        self.queue.push(frame.time, frame);
                    }
                }
                VideoEvent::End { generation } if generation == self.generation => {
                    self.video_ended = true;
                    // A seek past the last frame still has to let playback start.
                    self.exact_frame_ready = true;
                }
                VideoEvent::Error(error) => self.error = Some(error),
                _ => {}
            }
        }

        if self.state == PlayState::Starting
            && self.exact_frame_ready
            && (self.sound_is_ready() || self.start_requested.elapsed() >= START_TIMEOUT)
        {
            self.state = PlayState::Playing;
            match &self.audio {
                Some(audio) => audio.state.set_paused(false),
                None => self.silent_clock_start = Some(Instant::now()),
            }
        }

        if self.state != PlayState::Playing {
            return newest_paused;
        }

        let now = self.position();
        let shown = self.queue.take_due(now);
        let ran_out = self.video_ended && self.queue.is_empty();
        if now >= self.duration || (ran_out && shown.is_none() && self.audio.is_none()) {
            self.base = now;
            self.pause();
        }
        shown
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.commands.send(VideoCommand::Quit);
        if let Some(audio) = &mut self.audio {
            let _ = audio.commands.send(AudioCommand::Quit);
            if let Some(worker) = audio.worker.take() {
                let _ = worker.join();
            }
        }
        // The decoder may be waiting to hand over a frame, so clear the way first.
        while self.events.try_recv().is_ok() {}
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{process::Command, sync::OnceLock};

    use bareclip_media::probe;

    /// A clip whose frames show their own number as blocks, with a little sound.
    fn test_clip() -> &'static Path {
        static CLIP: OnceLock<PathBuf> = OnceLock::new();
        CLIP.get_or_init(|| {
            let folder = std::env::temp_dir().join("bareclip-app-tests");
            std::fs::create_dir_all(&folder).expect("the folder can be created");
            let path = folder.join(format!("player-{}.mp4", std::process::id()));
            let status = Command::new("ffmpeg")
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                ])
                .arg(
                    "nullsrc=s=320x180:r=30:d=4,\
                     geq=lum='255*mod(floor(N/pow(2,floor(X/20))),2)':cb=128:cr=128,\
                     format=yuv420p",
                )
                .args([
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:sample_rate=48000:duration=4",
                ])
                .args([
                    "-c:v",
                    "libopenh264",
                    "-b:v",
                    "4M",
                    "-g",
                    "30",
                    "-c:a",
                    "aac",
                ])
                .arg(&path)
                .status()
                .expect("ffmpeg is on PATH");
            assert!(status.success());
            path
        })
    }

    fn silent_player() -> Player {
        let info = probe(test_clip()).expect("the clip can be read");
        Player::open(&info, false, Arc::new(|| {})).expect("the player opens")
    }

    fn number_of(frame: &PreviewFrame) -> u32 {
        let row = frame.width as usize * 4 * (frame.height as usize / 2);
        (0..16).fold(0, |number, bit| {
            if frame.rgba[row + (bit * 20 + 10) * 4] > 128 {
                number | 1 << bit
            } else {
                number
            }
        })
    }

    /// Runs the player until a frame with this number appears.
    fn wait_for_frame(player: &mut Player, number: u32) {
        let started = Instant::now();
        let mut last = None;
        while started.elapsed() < Duration::from_secs(10) {
            if let Some(frame) = player.update() {
                last = Some(number_of(&frame));
                if last == Some(number) {
                    return;
                }
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!("frame {number} never appeared, last was {last:?}");
    }

    #[test]
    fn seeking_while_paused_shows_the_exact_frame() {
        let mut player = silent_player();

        player.seek(Ticks::from_seconds(1.5));
        wait_for_frame(&mut player, 45);
        assert_eq!(player.position(), Ticks::from_seconds(1.5));

        player.seek(Ticks::from_seconds(0.5));
        wait_for_frame(&mut player, 15);
    }

    #[test]
    fn only_the_newest_of_many_seeks_counts() {
        let mut player = silent_player();

        for number in 1..=40 {
            player.seek(Ticks::from_seconds(f64::from(number) / 30.0));
        }

        wait_for_frame(&mut player, 40);
    }

    #[test]
    fn playback_advances_with_the_clock_and_stops_at_the_end() {
        let mut player = silent_player();

        player.seek(Ticks::from_seconds(3.0));
        wait_for_frame(&mut player, 90);
        player.play();
        assert!(player.is_playing());

        let started = Instant::now();
        let mut highest = 0;
        while player.is_playing() && started.elapsed() < Duration::from_secs(5) {
            if let Some(frame) = player.update() {
                highest = highest.max(number_of(&frame));
            }
            thread::sleep(Duration::from_millis(2));
        }

        assert!(!player.is_playing(), "playback should have stopped");
        assert!(highest >= 115, "only reached frame {highest}");
        let took = started.elapsed().as_secs_f64();
        assert!(
            (0.8..2.5).contains(&took),
            "one second of video took {took}s"
        );
        assert_eq!(player.position(), player.duration());
    }

    #[test]
    fn pausing_holds_the_position() {
        let mut player = silent_player();

        player.play();
        let started = Instant::now();
        while player.position() < Ticks::from_seconds(0.3)
            && started.elapsed() < Duration::from_secs(5)
        {
            player.update();
            thread::sleep(Duration::from_millis(2));
        }
        player.pause();
        let held = player.position();
        assert!(!player.is_playing());

        thread::sleep(Duration::from_millis(150));
        player.update();
        assert_eq!(player.position(), held);
    }

    #[test]
    fn playing_again_from_the_end_starts_over() {
        let mut player = silent_player();
        player.seek(player.duration());

        player.play();

        assert_eq!(player.base, Ticks(0));
    }

    #[test]
    fn seeking_stays_inside_the_video() {
        let mut player = silent_player();

        player.seek(Ticks::from_seconds(99.0));
        assert_eq!(player.position(), player.duration());

        player.seek(Ticks::from_seconds(-5.0));
        assert_eq!(player.position(), Ticks(0));
    }

    #[test]
    fn closing_the_player_stops_its_threads_even_mid_playback() {
        let mut player = silent_player();
        player.play();
        thread::sleep(Duration::from_millis(50));

        let started = Instant::now();
        drop(player);

        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_missing_file_reports_an_error() {
        let mut info = probe(test_clip()).expect("the clip can be read");
        info.path = PathBuf::from("this-file-does-not-exist.mp4");
        let mut player = Player::open(&info, false, Arc::new(|| {})).expect("the player opens");

        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(5) {
            player.update();
            if player.take_error().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("no error was reported");
    }
}
