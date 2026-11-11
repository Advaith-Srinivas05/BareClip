//! Records pictures of the screen and any number of sound tracks into one video, at a steady
//! frame rate, written as it goes so a crash keeps what was recorded.
//!
//! Windows only sends a picture when something changes, so the last one is repeated until a new
//! one comes. It sends no sound at all while the PC is silent, so each sound track is kept up with
//! the clock by filling the gap with silence.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use bareclip_media::{
    BgraConverter, MediaError, Quality, RateChange, SOUND_CHANNELS, SOUND_RATE, ScreenPicture,
    SoundLabel, VideoFormat, VideoSettings, VideoWriter,
};
use rtrb::Consumer;

use crate::{Picture, ScreenCapture};

/// How far a sound track may fall behind the clock, in seconds, before silence fills it. More
/// than the sound card's own delay, so real sound that is merely late isn't mistaken for silence.
const SILENCE_AFTER: f64 = 0.2;
/// How far behind the clock silence brings a track, leaving room for sound already on its way.
const SILENCE_UP_TO: f64 = 0.1;
/// How often the recording looks for new pictures and sound when it isn't due a frame.
const LOOK_EVERY: Duration = Duration::from_millis(4);

/// Where the pictures come from: the screen, or something standing in for it in tests.
pub trait PictureSource: Send {
    /// The newest picture and how many have arrived so far, if any has.
    fn latest(&self) -> Option<(Picture, u64)>;
    /// The size it started at.
    fn size(&self) -> (u32, u32);
}

impl PictureSource for ScreenCapture {
    fn latest(&self) -> Option<(Picture, u64)> {
        ScreenCapture::latest(self)
    }

    fn size(&self) -> (u32, u32) {
        ScreenCapture::size(self)
    }
}

/// One sound track to record: stereo at `rate` coming out of `queue`.
pub struct SoundFeed {
    pub queue: Consumer<f32>,
    pub rate: u32,
    /// What the track is called in the file, such as "PC sound" or "Microphone".
    pub name: String,
    /// Starts silenced. The track is still made, so it can be heard later on.
    pub muted: bool,
}

/// How the screen is recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenSettings {
    pub frame_rate: u32,
    /// The video's size, or `None` for the size of what is recorded.
    pub size: Option<(u32, u32)>,
    pub quality: Quality,
}

impl Default for ScreenSettings {
    fn default() -> Self {
        Self {
            frame_rate: 30,
            size: None,
            quality: Quality::Balanced,
        }
    }
}

#[derive(Debug, Default)]
struct Control {
    paused: AtomicBool,
    stop: AtomicBool,
    frames: AtomicU64,
    /// One for each sound track, in order.
    muted: Vec<AtomicBool>,
}

/// What a finished recording holds.
#[derive(Clone, Debug, PartialEq)]
pub struct ScreenReport {
    pub path: PathBuf,
    pub seconds: f64,
    pub encoder: String,
}

/// A screen recording under way. Dropping it without stopping finishes the file anyway.
pub struct ScreenRecording {
    control: Arc<Control>,
    frame_rate: u32,
    worker: Option<JoinHandle<Result<ScreenReport, MediaError>>>,
}

/// H.264 needs even sizes.
fn even((width, height): (u32, u32)) -> (u32, u32) {
    ((width & !1).max(2), (height & !1).max(2))
}

/// One sound track on its way into the file.
struct TrackIn {
    feed: SoundFeed,
    change: Option<RateChange>,
    /// Frames written, at `SOUND_RATE`.
    written: u64,
    heard: Vec<f32>,
}

impl ScreenRecording {
    /// Starts recording. Anything already waiting in the sound queues was heard before this
    /// started, so it is left out. Fails if the file can't be made or no encoder works.
    pub fn start(
        path: &Path,
        pictures: Box<dyn PictureSource>,
        sounds: Vec<SoundFeed>,
        settings: ScreenSettings,
    ) -> Result<Self, MediaError> {
        let control = Arc::new(Control {
            muted: sounds
                .iter()
                .map(|feed| AtomicBool::new(feed.muted))
                .collect(),
            ..Control::default()
        });
        let (ready, started) = mpsc::channel();
        let path = path.to_owned();
        let shared = control.clone();
        let worker = thread::spawn(move || {
            let size = even(settings.size.unwrap_or_else(|| pictures.size()));
            let fps = settings.frame_rate.max(1);
            let video = VideoSettings {
                width: size.0,
                height: size.1,
                frame_rate: (fps as i32, 1),
                // Only for encoders that aim at a bit rate: a tenth of a bit for each pixel.
                bit_rate: (f64::from(size.0) * f64::from(size.1) * f64::from(fps) * 0.1) as usize,
                sound_tracks: sounds
                    .iter()
                    .map(|feed| SoundLabel {
                        title: Some(feed.name.clone()),
                        language: None,
                    })
                    .collect(),
                quality: settings.quality,
                format: VideoFormat::H264,
            };
            let opened = VideoWriter::create_growing(&path, &video).and_then(|writer| {
                let tracks = sounds
                    .into_iter()
                    .map(|mut feed| {
                        while feed.queue.pop().is_ok() {}
                        let change = (feed.rate != SOUND_RATE)
                            .then(|| RateChange::new(feed.rate, SOUND_RATE))
                            .transpose()?;
                        Ok(TrackIn {
                            feed,
                            change,
                            written: 0,
                            heard: Vec::new(),
                        })
                    })
                    .collect::<Result<Vec<_>, MediaError>>()?;
                let black = BgraConverter::new(size, size)?.convert(
                    &vec![0; size.0 as usize * size.1 as usize * 4],
                    size.0 as usize * 4,
                )?;
                Ok((writer, tracks, black))
            });
            let (writer, tracks, black) = match opened {
                Ok(opened) => {
                    let _ = ready.send(Ok(()));
                    opened
                }
                Err(error) => {
                    let _ = ready.send(Err(error));
                    return Err(MediaError::Cancelled);
                }
            };
            record(
                Recorder {
                    writer,
                    tracks,
                    black,
                    pictures,
                    size,
                    fps,
                    converter: None,
                    last_count: 0,
                    shown_any: false,
                },
                &shared,
                path,
            )
        });
        match started.recv() {
            Ok(Ok(())) => Ok(Self {
                control,
                frame_rate: settings.frame_rate.max(1),
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

    /// Silences a sound track from now on, or lets it be heard again. Silence is still written,
    /// so the track stays in step with the picture.
    pub fn set_muted(&self, track: usize, muted: bool) {
        if let Some(flag) = self.control.muted.get(track) {
            flag.store(muted, Ordering::Release);
        }
    }

    /// Whether a sound track is silenced. A track that doesn't exist counts as silent.
    pub fn is_muted(&self, track: usize) -> bool {
        self.control
            .muted
            .get(track)
            .is_none_or(|flag| flag.load(Ordering::Acquire))
    }

    /// How long the recording is so far, pauses left out.
    pub fn seconds(&self) -> f64 {
        self.control.frames.load(Ordering::Acquire) as f64 / f64::from(self.frame_rate)
    }

    /// Writes what is still waiting and finishes the file.
    pub fn stop(mut self) -> Result<ScreenReport, MediaError> {
        self.finish()
    }

    fn finish(&mut self) -> Result<ScreenReport, MediaError> {
        self.control.stop.store(true, Ordering::Release);
        match self.worker.take().map(JoinHandle::join) {
            Some(Ok(result)) => result,
            _ => Err(MediaError::Cancelled),
        }
    }
}

impl Drop for ScreenRecording {
    fn drop(&mut self) {
        if self.worker.is_some() {
            let _ = self.finish();
        }
    }
}

/// Everything the recording's thread works with.
struct Recorder {
    writer: VideoWriter,
    tracks: Vec<TrackIn>,
    black: ScreenPicture,
    pictures: Box<dyn PictureSource>,
    size: (u32, u32),
    fps: u32,
    /// Converts pictures of one size, which changes if a window does.
    converter: Option<(BgraConverter, (u32, u32))>,
    last_count: u64,
    shown_any: bool,
}

impl Recorder {
    /// Writes the next frame: a new picture if one came, or the last one again.
    fn write_frame(&mut self) -> Result<(), MediaError> {
        let newest = self
            .pictures
            .latest()
            .filter(|(_, count)| *count != self.last_count);
        let Some((picture, count)) = newest else {
            return if self.shown_any {
                self.writer.repeat_picture()
            } else {
                self.writer.write_picture(&self.black.view())
            };
        };
        self.last_count = count;
        let from = (picture.width, picture.height);
        if self
            .converter
            .as_ref()
            .is_none_or(|(_, size)| *size != from)
        {
            self.converter = Some((BgraConverter::new(from, self.size)?, from));
        }
        let Some((converter, _)) = &mut self.converter else {
            return Ok(());
        };
        let converted = converter.convert(&picture.bgra, picture.width as usize * 4)?;
        self.shown_any = true;
        self.writer.write_picture(&converted.view())
    }

    /// Writes whatever sound has arrived, or throws it away while paused, and fills silence
    /// where a track has fallen behind `recorded` seconds. A muted track gets silence as long as
    /// what was heard.
    fn write_sound(
        &mut self,
        paused: bool,
        recorded: f64,
        muted: &[AtomicBool],
    ) -> Result<(), MediaError> {
        for (index, track) in self.tracks.iter_mut().enumerate() {
            track.heard.clear();
            while let Ok(sample) = track.feed.queue.pop() {
                track.heard.push(sample);
            }
            if paused {
                continue;
            }
            if !track.heard.is_empty() {
                let samples = match &mut track.change {
                    Some(change) => change.run(&track.heard)?,
                    None => track.heard.clone(),
                };
                let whole = samples.len() / SOUND_CHANNELS * SOUND_CHANNELS;
                if muted
                    .get(index)
                    .is_some_and(|flag| flag.load(Ordering::Acquire))
                {
                    self.writer.write_silence(index, whole / SOUND_CHANNELS)?;
                } else {
                    self.writer.write_sound(index, &samples[..whole])?;
                }
                track.written += (whole / SOUND_CHANNELS) as u64;
            }
            let rate = f64::from(SOUND_RATE);
            let behind = recorded * rate - track.written as f64;
            if behind > SILENCE_AFTER * rate {
                let fill = (behind - SILENCE_UP_TO * rate) as usize;
                self.writer.write_silence(index, fill)?;
                track.written += fill as u64;
            }
        }
        Ok(())
    }
}

fn record(
    mut recorder: Recorder,
    control: &Control,
    path: PathBuf,
) -> Result<ScreenReport, MediaError> {
    let started = Instant::now();
    let mut paused_for = Duration::ZERO;
    let mut paused_since: Option<Instant> = None;
    let mut frames: u64 = 0;
    loop {
        let stopping = control.stop.load(Ordering::Acquire);
        let paused = control.paused.load(Ordering::Acquire);
        match (paused, paused_since) {
            (true, None) => paused_since = Some(Instant::now()),
            (false, Some(since)) => {
                paused_for += since.elapsed();
                paused_since = None;
            }
            _ => {}
        }
        let now_paused = paused_since.map_or(Duration::ZERO, |since| since.elapsed());
        let recorded = (started.elapsed() - paused_for - now_paused).as_secs_f64();
        if !paused {
            // Every frame due by now, so a slow moment is caught up with repeats.
            let due = (recorded * f64::from(recorder.fps)).floor() as u64 + 1;
            while frames < due {
                recorder.write_frame()?;
                frames += 1;
                control.frames.store(frames, Ordering::Release);
            }
        }
        recorder.write_sound(paused, recorded, &control.muted)?;
        if stopping {
            break;
        }
        thread::sleep(LOOK_EVERY);
    }
    let encoder = recorder.writer.encoder_name().to_owned();
    recorder.writer.finish()?;
    Ok(ScreenReport {
        path,
        seconds: frames as f64 / f64::from(recorder.fps),
        encoder,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtrb::RingBuffer;
    use std::sync::Mutex;

    /// Stands in for the screen: a picture whose colour changes when told to.
    struct MadeUp {
        latest: Arc<Mutex<Option<(Picture, u64)>>>,
        size: (u32, u32),
    }

    impl PictureSource for MadeUp {
        fn latest(&self) -> Option<(Picture, u64)> {
            self.latest.lock().ok()?.clone()
        }

        fn size(&self) -> (u32, u32) {
            self.size
        }
    }

    fn show(
        latest: &Mutex<Option<(Picture, u64)>>,
        (width, height): (u32, u32),
        [b, g, r]: [u8; 3],
    ) {
        let mut latest = latest.lock().expect("not poisoned");
        let count = latest.as_ref().map_or(0, |(_, count)| count + 1);
        *latest = Some((
            Picture {
                width,
                height,
                bgra: [b, g, r, 255].repeat((width * height) as usize),
                shown_at: 0,
            },
            count,
        ));
    }

    fn folder(name: &str) -> PathBuf {
        let folder = std::env::temp_dir()
            .join("bareclip-screen-recording-tests")
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).expect("the folder can be made");
        folder
    }

    #[test]
    fn a_recording_keeps_its_frame_rate_and_its_sound_in_step() {
        let path = folder("steady").join("screen.mp4");
        let latest = Arc::new(Mutex::new(None));
        show(&latest, (321, 181), [200, 40, 90]);
        let pictures = Box::new(MadeUp {
            latest: latest.clone(),
            size: (321, 181),
        });
        // One track with steady sound, and one that stays silent, as the PC does when nothing
        // plays.
        let (mut talking, heard) = RingBuffer::new(44_100 * 8);
        let (_quiet, silent) = RingBuffer::<f32>::new(16);
        let sounds = vec![
            SoundFeed {
                queue: heard,
                rate: 44_100,
                name: "Microphone".to_owned(),
                muted: false,
            },
            SoundFeed {
                queue: silent,
                rate: 48_000,
                name: "PC sound".to_owned(),
                muted: false,
            },
        ];
        let recording = ScreenRecording::start(&path, pictures, sounds, ScreenSettings::default())
            .expect("it starts");
        let started = Instant::now();
        let mut fed = 0_u64;
        while started.elapsed() < Duration::from_secs(2) {
            let due = (started.elapsed().as_secs_f64() * 44_100.0) as u64;
            while fed < due {
                let _ = talking.push(0.2);
                let _ = talking.push(0.2);
                fed += 1;
            }
            if started.elapsed() > Duration::from_secs(1) {
                show(&latest, (321, 181), [20, 200, 40]);
            }
            thread::sleep(Duration::from_millis(10));
        }
        let report = recording.stop().expect("it finishes");
        assert!((report.seconds - 2.0).abs() < 0.15, "{report:?}");

        let info = bareclip_media::probe(&path).expect("it can be read");
        assert_eq!((info.video.width, info.video.height), (320, 180));
        assert_eq!(info.audio.len(), 2);
        let (frames, picture_seconds, sound_seconds) = streams_of(&path);
        let rate = f64::from(frames) / picture_seconds;
        assert!((rate - 30.0).abs() < 1.5, "{rate} frames a second");
        for seconds in sound_seconds {
            assert!(
                (seconds - picture_seconds).abs() < 0.3,
                "a sound track runs {seconds} s against {picture_seconds} s of picture"
            );
        }
    }

    /// How many pictures a file has, how long its picture runs, and how long each sound track
    /// runs, read straight from its packets.
    fn streams_of(path: &Path) -> (u32, f64, Vec<f64>) {
        use ffmpeg_next::{format, media::Type};
        let mut input = format::input(path).expect("it opens");
        let kinds: Vec<(Type, f64)> = input
            .streams()
            .map(|stream| (stream.parameters().medium(), f64::from(stream.time_base())))
            .collect();
        let mut ends = vec![0.0_f64; kinds.len()];
        let mut frames = 0;
        for (stream, packet) in input.packets() {
            let index = stream.index();
            let (kind, base) = kinds[index];
            if kind == Type::Video {
                frames += 1;
            }
            let end = (packet.pts().unwrap_or(0) + packet.duration()) as f64 * base;
            ends[index] = ends[index].max(end);
        }
        let picture = kinds
            .iter()
            .zip(&ends)
            .find(|((kind, _), _)| *kind == Type::Video)
            .map_or(0.0, |(_, end)| *end);
        let sound = kinds
            .iter()
            .zip(&ends)
            .filter(|((kind, _), _)| *kind == Type::Audio)
            .map(|(_, end)| *end)
            .collect();
        (frames, picture, sound)
    }

    #[test]
    fn a_paused_stretch_is_left_out_of_picture_and_sound() {
        let path = folder("paused").join("screen.mp4");
        let latest = Arc::new(Mutex::new(None));
        show(&latest, (160, 90), [0, 0, 255]);
        let (_feeding, queue) = RingBuffer::<f32>::new(16);
        let recording = ScreenRecording::start(
            &path,
            Box::new(MadeUp {
                latest,
                size: (160, 90),
            }),
            vec![SoundFeed {
                queue,
                rate: 48_000,
                name: "PC sound".to_owned(),
                muted: false,
            }],
            ScreenSettings::default(),
        )
        .expect("it starts");
        thread::sleep(Duration::from_millis(700));
        recording.set_paused(true);
        thread::sleep(Duration::from_millis(1000));
        recording.set_paused(false);
        thread::sleep(Duration::from_millis(700));
        let report = recording.stop().expect("it finishes");
        assert!((report.seconds - 1.4).abs() < 0.2, "{report:?}");
        let info = bareclip_media::probe(&path).expect("it can be read");
        assert!((info.duration.as_seconds() - 1.4).abs() < 0.3);
    }

    /// The loudest sample of a file's first sound track between two times, in seconds.
    fn loudest_between(path: &Path, from: f64, to: f64) -> f32 {
        use ffmpeg_next::{codec, frame, media::Type};
        let mut input = ffmpeg_next::format::input(path).expect("it opens");
        let stream = input.streams().best(Type::Audio).expect("it has sound");
        let index = stream.index();
        let base = f64::from(stream.time_base());
        let mut decoder = codec::context::Context::from_parameters(stream.parameters())
            .and_then(|context| context.decoder().audio())
            .expect("the sound can be decoded");
        let mut peak = 0.0_f32;
        let mut heard = frame::Audio::empty();
        let mut take = |decoder: &mut ffmpeg_next::decoder::Audio, peak: &mut f32| {
            while decoder.receive_frame(&mut heard).is_ok() {
                let at = heard.pts().unwrap_or(0) as f64 * base;
                if at < from || at >= to {
                    continue;
                }
                for plane in 0..heard.planes() {
                    for sample in heard.plane::<f32>(plane) {
                        *peak = peak.max(sample.abs());
                    }
                }
            }
        };
        for (stream, packet) in input.packets() {
            if stream.index() == index {
                decoder.send_packet(&packet).expect("the packet is taken");
                take(&mut decoder, &mut peak);
            }
        }
        decoder.send_eof().expect("the end is taken");
        take(&mut decoder, &mut peak);
        peak
    }

    #[test]
    fn a_muted_track_records_silence_until_it_is_heard_again() {
        let path = folder("muted").join("screen.mp4");
        let latest = Arc::new(Mutex::new(None));
        show(&latest, (160, 90), [0, 0, 255]);
        let (mut talking, queue) = RingBuffer::new(48_000 * 8);
        let recording = ScreenRecording::start(
            &path,
            Box::new(MadeUp {
                latest,
                size: (160, 90),
            }),
            vec![SoundFeed {
                queue,
                rate: 48_000,
                name: "Microphone".to_owned(),
                muted: true,
            }],
            ScreenSettings::default(),
        )
        .expect("it starts");
        assert!(recording.is_muted(0));
        assert!(
            recording.is_muted(1),
            "a track that doesn't exist is silent"
        );
        let started = Instant::now();
        let mut fed = 0_u64;
        let mut unmuted = false;
        while started.elapsed() < Duration::from_millis(2200) {
            let due = (started.elapsed().as_secs_f64() * 48_000.0) as u64;
            while fed < due {
                let tone = (fed as f32 * 440.0 / 48_000.0 * std::f32::consts::TAU).sin() * 0.5;
                let _ = talking.push(tone);
                let _ = talking.push(tone);
                fed += 1;
            }
            if !unmuted && started.elapsed() > Duration::from_millis(1100) {
                recording.set_muted(0, false);
                unmuted = true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!recording.is_muted(0));
        recording.stop().expect("it finishes");
        let muted = loudest_between(&path, 0.1, 0.8);
        let heard = loudest_between(&path, 1.5, 2.0);
        assert!(muted < 0.01, "{muted} while muted");
        assert!(heard > 0.2, "{heard} once heard again");
        let (_, picture_seconds, sound_seconds) = streams_of(&path);
        assert!((sound_seconds[0] - picture_seconds).abs() < 0.3);
    }

    #[test]
    #[ignore = "needs a desktop session, which GitHub's runners may not give"]
    fn a_real_window_is_recorded_in_its_own_colours() {
        use std::sync::atomic::AtomicBool;
        let stop = Arc::new(AtomicBool::new(false));
        let window = crate::screen::tests::coloured_window([200, 40, 90], stop.clone())
            .recv_timeout(Duration::from_secs(10))
            .expect("the window shows");
        let capture =
            ScreenCapture::start(crate::Source::Window(window), false).expect("capture starts");
        let path = folder("real").join("window.mp4");
        let recording = ScreenRecording::start(
            &path,
            Box::new(capture),
            Vec::new(),
            ScreenSettings::default(),
        )
        .expect("it starts");
        thread::sleep(Duration::from_millis(1500));
        let report = recording.stop().expect("it finishes");
        stop.store(true, Ordering::Release);

        // The colour the converter makes of it, which the file should hold, give or take what
        // compression changes.
        let wanted = BgraConverter::new((16, 16), (16, 16))
            .expect("it opens")
            .convert(&[90, 40, 200, 255].repeat(256), 64)
            .expect("it converts");
        let wanted = wanted.view();
        let wanted = [wanted.y[0], wanted.u[0], wanted.v[0]];
        let mut reader = bareclip_media::PictureReader::open(&path).expect("it opens");
        let mut last = None;
        while let Some(picture) = reader.next_picture().expect("it decodes") {
            last = Some(picture);
        }
        let last = last.expect("there are pictures");
        let view = last.view();
        let (width, height) = last.size();
        let middle = (height / 2) as usize;
        let at = middle * view.y_stride + (width / 2) as usize;
        let chroma = (middle / 2) * view.uv_stride + (width / 4) as usize;
        let got = [view.y[at], view.u[chroma], view.v[chroma]];
        for (got, wanted) in got.iter().zip(wanted) {
            assert!(
                i32::from(*got).abs_diff(i32::from(wanted)) <= 6,
                "{got:?} against {wanted:?}"
            );
        }
        assert!(report.seconds > 1.2, "{report:?}");
    }

    #[test]
    fn nothing_shown_yet_records_black_rather_than_failing() {
        let path = folder("black").join("screen.mp4");
        let recording = ScreenRecording::start(
            &path,
            Box::new(MadeUp {
                latest: Arc::new(Mutex::new(None)),
                size: (64, 64),
            }),
            Vec::new(),
            ScreenSettings::default(),
        )
        .expect("it starts");
        thread::sleep(Duration::from_millis(500));
        recording.stop().expect("it finishes");
        assert!(bareclip_media::probe(&path).is_ok());
    }
}
