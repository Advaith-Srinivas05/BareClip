//! Adds up everything that can be heard on the timeline, a block at a time. The preview and the
//! export both use this, so what is heard is what gets saved.

use std::{collections::VecDeque, path::PathBuf};

use bareclip_core::{
    sound::ClipSound,
    speed::Speed,
    time::{TICKS_PER_SECOND, Ticks},
    timeline::TimeRange,
};
use bareclip_media::{AudioPreview, AudioStep, MediaError, RateChange};

use crate::{
    clean::{BLOCK_FRAMES, CLEAN_RATE, Cleaner, DELAY_FRAMES, LEAD_IN_FRAMES},
    stretch::{Frames, Stretch},
};

const CHANNELS: usize = 2;
/// Decoded blocks rarely line up to the sample, so a difference this small is not a real hole.
/// Filling it with silence would make a faint click.
const JOIN_TOLERANCE_FRAMES: i64 = 32;
/// How far ahead of a track with nothing to give the others can decode before it counts as silent
/// for now, so a track that starts late can't pull a whole file into memory.
const MOST_SECONDS_AHEAD: i64 = 2;
/// Below this, sound passes through untouched. Above it, it is eased in under full scale.
const LIMIT_KNEE: f32 = 0.8;

/// Keeps a mix from going over full scale, which would crackle. Anything quieter than the knee
/// doesn't change at all.
pub fn soft_limit(sample: f32) -> f32 {
    let size = sample.abs();
    if size <= LIMIT_KNEE {
        return sample;
    }
    let room = 1.0 - LIMIT_KNEE;
    (LIMIT_KNEE + room * ((size - LIMIT_KNEE) / room).tanh()).copysign(sample)
}

/// Which finished track a file's sound goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bus {
    /// Every sound track of the file, added together, goes to this one.
    All(usize),
    /// Each sound track of the file goes to its own, counting up from this one.
    ByTrack(usize),
}

impl Bus {
    fn of(self, track: usize) -> usize {
        match self {
            Self::All(bus) => bus,
            Self::ByTrack(first) => first + track,
        }
    }
}

/// A stretch of a file to hear, at its place on the timeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MixSpan {
    pub path: PathBuf,
    pub start: Ticks,
    pub source: TimeRange,
    pub sound: ClipSound,
    pub speed: Speed,
    pub bus: Bus,
}

impl MixSpan {
    /// How long this lasts on the timeline.
    fn length(&self) -> Ticks {
        self.speed.on_timeline(self.source.duration())
    }

    /// Where this ends on the timeline.
    pub fn end(&self) -> Ticks {
        Ticks(self.start.0 + self.length().0)
    }

    fn wants(&self, track: usize) -> bool {
        self.sound
            .only_track
            .is_none_or(|only| only as usize == track)
    }

    /// Whether a reader for one of these could go on being used for the other.
    fn reads_same_as(&self, other: &Self) -> bool {
        self.path == other.path
            && self.start == other.start
            && self.source == other.source
            && self.speed == other.speed
            && self.sound.only_track == other.sound.only_track
            && self.sound.noise_suppression == other.sound.noise_suppression
    }
}

fn frame_at(time: Ticks, rate: u32) -> i64 {
    (i128::from(time.0) * i128::from(rate) / i128::from(TICKS_PER_SECOND)) as i64
}

fn time_of_frames(frames: i64, rate: u32) -> Ticks {
    Ticks((i128::from(frames) * i128::from(TICKS_PER_SECOND) / i128::from(rate)) as i64)
}

/// Sound decoded from one track of a file, waiting to be used.
#[derive(Debug, Default)]
struct TrackBuffer {
    /// Stereo samples.
    samples: VecDeque<f32>,
    /// Which frame of the span the first of them is.
    first: i64,
}

impl TrackBuffer {
    fn end(&self) -> i64 {
        self.first + (self.samples.len() / CHANNELS) as i64
    }

    /// Adds a decoded block that starts at this frame of the span.
    fn add(&mut self, start: i64, samples: &[f32]) {
        let hole = start - self.end();
        if hole.abs() <= JOIN_TOLERANCE_FRAMES {
            self.samples.extend(samples.iter().copied());
        } else if hole > 0 {
            let silence = hole as usize * CHANNELS;
            self.samples.extend(std::iter::repeat_n(0.0, silence));
            self.samples.extend(samples.iter().copied());
        } else {
            let skip = ((-hole) as usize * CHANNELS).min(samples.len());
            self.samples.extend(samples[skip..].iter().copied());
        }
    }

    /// Throws away everything before this frame of the span.
    fn skip_to(&mut self, frame: i64) {
        let drop = ((frame - self.first).max(0) as usize * CHANNELS).min(self.samples.len());
        self.samples.drain(..drop);
        self.first = frame;
    }
}

impl Frames for TrackBuffer {
    fn frame(&self, at: i64) -> [f32; 2] {
        let place = (at - self.first) as usize * CHANNELS;
        if at < self.first || place + 1 >= self.samples.len() {
            return [0.0; 2];
        }
        [self.samples[place], self.samples[place + 1]]
    }
}

/// One open file, positioned for one span.
struct SpanReader {
    preview: AudioPreview,
    /// What is heard from each track, in frames of the footage at the mix's rate.
    tracks: Vec<TrackBuffer>,
    /// For a clip at another speed, what turns each track's footage into sound for the timeline.
    stretches: Vec<Stretch>,
    /// With noise suppression on, each track's sound on its way to be cleaned.
    cleaning: Vec<Option<TrackCleaning>>,
    /// Where the span's footage starts, in frames at the rate the file is decoded at.
    source_start: i64,
    ended: bool,
}

impl SpanReader {
    fn open(span: &MixSpan, rate: u32, from: i64) -> Result<Self, MediaError> {
        let cleans = span.sound.noise_suppression;
        let decode_rate = if cleans { CLEAN_RATE } else { rate };
        let mut preview = AudioPreview::open(&span.path, decode_rate)?;
        // `from` is a frame on the timeline, and everything here is in frames of the footage.
        let from_footage = frame_at(span.speed.in_footage(time_of_frames(from, rate)), rate);
        // Cleaning starts a little earlier, so it has settled by the time anything is heard.
        let from_decoded = if cleans {
            rescale(from_footage, rate, CLEAN_RATE) - LEAD_IN_FRAMES
        } else {
            from_footage
        };
        let seconds = span.source.start.as_seconds() + from_decoded as f64 / f64::from(decode_rate);
        preview.seek(seconds.max(0.0))?;
        let tracks: Vec<TrackBuffer> = (0..preview.track_count())
            .map(|_| TrackBuffer {
                samples: VecDeque::new(),
                first: from_footage,
            })
            .collect();
        let footage_frames = frame_at(span.source.duration(), rate);
        let stretches = tracks
            .iter()
            .map(|_| Stretch::new(span.speed, footage_frames))
            .collect();
        let cleaning = tracks
            .iter()
            .map(|_| {
                cleans
                    .then(|| TrackCleaning::new(from_decoded, rate))
                    .transpose()
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            preview,
            tracks,
            stretches,
            cleaning,
            source_start: frame_at(span.source.start, decode_rate),
            ended: false,
        })
    }

    /// How far a track has been decoded, in frames of the footage at the mix's rate.
    fn reach(&self, track: usize, rate: u32) -> i64 {
        match &self.cleaning[track] {
            Some(cleaning) => rescale(cleaning.raw.end(), CLEAN_RATE, rate),
            None => self.tracks[track].end(),
        }
    }

    /// Decodes until every track the span wants has sound up to this frame of the span.
    fn fill_to(&mut self, span: &MixSpan, until: i64, rate: u32) {
        let furthest = until + MOST_SECONDS_AHEAD * i64::from(rate);
        loop {
            self.clean_what_is_ready(rate);
            if self.ended {
                return;
            }
            let lacking = self
                .tracks
                .iter()
                .enumerate()
                .any(|(track, buffer)| span.wants(track) && buffer.end() < until);
            let room = (0..self.tracks.len()).all(|track| self.reach(track, rate) < furthest);
            if !lacking || !room {
                return;
            }
            match self.preview.next_step() {
                Ok(AudioStep::Chunk(chunk)) => {
                    if span.wants(chunk.track) {
                        let start = chunk.start_frame - self.source_start;
                        let buffer = match self.cleaning.get_mut(chunk.track) {
                            Some(Some(cleaning)) => Some(&mut cleaning.raw),
                            _ => self.tracks.get_mut(chunk.track),
                        };
                        if let Some(buffer) = buffer {
                            buffer.add(start, &chunk.samples);
                        }
                    }
                }
                Ok(AudioStep::Pending) => {}
                // A damaged file should cost a moment of sound, not the whole mix.
                Ok(AudioStep::End) | Err(_) => self.ended = true,
            }
        }
    }

    fn clean_what_is_ready(&mut self, rate: u32) {
        for (cleaning, heard) in self.cleaning.iter_mut().zip(&mut self.tracks) {
            if let Some(cleaning) = cleaning {
                cleaning.run(heard, rate, self.ended);
            }
        }
    }
}

/// Adds cleaned sound that is exactly in place. What overlaps what is already there, which is the
/// lead-in at the start, is cut off to the frame, where `TrackBuffer::add` would let a small
/// overlap through and leave the sound up to 32 frames late.
fn put(heard: &mut TrackBuffer, at: i64, samples: &[f32]) {
    let skip = ((heard.end() - at).max(0) as usize * CHANNELS).min(samples.len());
    heard.add(at + (skip / CHANNELS) as i64, &samples[skip..]);
}

/// A frame count at one rate, at another.
fn rescale(frames: i64, from: u32, to: u32) -> i64 {
    if from == to {
        return frames;
    }
    (i128::from(frames) * i128::from(to)).div_euclid(i128::from(from)) as i64
}

/// One track's sound being cleaned at 48 kHz, on its way to be heard at the mix's rate.
struct TrackCleaning {
    /// Decoded sound waiting to be cleaned, in frames of the footage at 48 kHz.
    raw: TrackBuffer,
    cleaner: Cleaner,
    /// Takes cleaned sound to the mix's rate, if that isn't 48 kHz.
    rate_change: Option<RateChange>,
    /// Where the next cleaned frame goes, in frames of the footage at the mix's rate.
    next: Option<i64>,
    /// The end of the file has been pushed through.
    finished: bool,
}

impl TrackCleaning {
    fn new(from: i64, rate: u32) -> Result<Self, MediaError> {
        let rate_change = (rate != CLEAN_RATE)
            .then(|| RateChange::new(CLEAN_RATE, rate))
            .transpose()?;
        Ok(Self {
            raw: TrackBuffer {
                samples: VecDeque::new(),
                first: from,
            },
            cleaner: Cleaner::default(),
            rate_change,
            next: None,
            finished: false,
        })
    }

    /// Cleans every whole block waiting, or everything once the file has ended, and puts it
    /// where it belongs in what is heard, taking off the cleaner's delay. What was only there to
    /// let the cleaner settle lands before what is heard begins, so it is dropped.
    fn run(&mut self, heard: &mut TrackBuffer, rate: u32, ended: bool) {
        if self.finished {
            return;
        }
        if ended {
            // Silence pushes the last of the sound out of the cleaner, a block late.
            let waiting = self.raw.samples.len() / CHANNELS;
            let pad =
                (BLOCK_FRAMES - waiting % BLOCK_FRAMES) % BLOCK_FRAMES + DELAY_FRAMES as usize;
            self.raw
                .samples
                .extend(std::iter::repeat_n(0.0, pad * CHANNELS));
        }
        while self.raw.samples.len() >= BLOCK_FRAMES * CHANNELS {
            let at = self.raw.first;
            let block: Vec<f32> = self.raw.samples.drain(..BLOCK_FRAMES * CHANNELS).collect();
            self.raw.first += BLOCK_FRAMES as i64;
            let cleaned = self.cleaner.clean(&block);
            let next = *self
                .next
                .get_or_insert_with(|| rescale(at - DELAY_FRAMES, CLEAN_RATE, rate));
            let samples = match &mut self.rate_change {
                Some(change) => change.run(&cleaned).unwrap_or_default(),
                None => cleaned,
            };
            put(heard, next, &samples);
            self.next = Some(next + (samples.len() / CHANNELS) as i64);
        }
        if ended {
            if let (Some(change), Some(next)) = (&mut self.rate_change, self.next) {
                let rest = change.finish().unwrap_or_default();
                put(heard, next, &rest);
            }
            self.finished = true;
        }
    }
}

/// Everything that can be heard on the timeline, read a block at a time from a moving position.
pub struct TimelineMix {
    spans: Vec<MixSpan>,
    /// One for each span, open only around the time it plays.
    readers: Vec<Option<SpanReader>>,
    rate: u32,
    position: i64,
    problem: Option<String>,
}

impl TimelineMix {
    pub fn new(spans: Vec<MixSpan>, rate: u32, at: Ticks) -> Self {
        let readers = spans.iter().map(|_| None).collect();
        Self {
            spans,
            readers,
            rate,
            position: frame_at(at, rate),
            problem: None,
        }
    }

    /// The next frame to be mixed, on the timeline.
    pub fn position(&self) -> i64 {
        self.position
    }

    /// The most recent file that couldn't be read, once.
    pub fn take_problem(&mut self) -> Option<String> {
        self.problem.take()
    }

    /// Moves to a moment on the timeline.
    pub fn seek(&mut self, at: Ticks) {
        self.position = frame_at(at, self.rate);
        self.readers.iter_mut().for_each(|reader| *reader = None);
    }

    /// Changes what is being mixed from here on. Anything still read the same way carries on, so a
    /// change of volume doesn't interrupt the sound.
    pub fn set_spans(&mut self, spans: Vec<MixSpan>) {
        let mut old: Vec<(MixSpan, Option<SpanReader>)> =
            self.spans.drain(..).zip(self.readers.drain(..)).collect();
        self.readers = spans
            .iter()
            .map(|span| {
                old.iter_mut()
                    .find(|(earlier, _)| earlier.reads_same_as(span))
                    .and_then(|(_, reader)| reader.take())
            })
            .collect();
        self.spans = spans;
    }

    /// Opens what starts within this long, so that it is ready when its time comes.
    pub fn prepare(&mut self, ahead: Ticks) {
        let window = frame_at(ahead, self.rate);
        for index in 0..self.spans.len() {
            let from = frame_at(self.spans[index].start, self.rate);
            let to = frame_at(self.spans[index].end(), self.rate);
            let due = from >= self.position && from - self.position <= window;
            if due && to > self.position && self.readers[index].is_none() {
                self.open_reader(index, 0);
            }
        }
    }

    fn open_reader(&mut self, index: usize, from: i64) {
        match SpanReader::open(&self.spans[index], self.rate, from) {
            Ok(reader) => self.readers[index] = Some(reader),
            Err(error) => {
                let name = self.spans[index].path.display();
                self.problem = Some(format!("The sound of {name} could not be read. {error}"));
            }
        }
    }

    /// Adds this many frames of stereo sound to the end of each bus, and moves on past them.
    pub fn fill(&mut self, frames: usize, buses: &mut [Vec<f32>]) {
        let starts: Vec<usize> = buses.iter().map(Vec::len).collect();
        for bus in buses.iter_mut() {
            bus.resize(bus.len() + frames * CHANNELS, 0.0);
        }
        let start = self.position;
        let end = start + frames as i64;
        for index in 0..self.spans.len() {
            self.add_span(index, start, end, buses, &starts);
        }
        for (bus, first) in buses.iter_mut().zip(&starts) {
            for sample in &mut bus[*first..] {
                *sample = soft_limit(*sample);
            }
        }
        self.position = end;
    }

    fn add_span(
        &mut self,
        index: usize,
        start: i64,
        end: i64,
        buses: &mut [Vec<f32>],
        starts: &[usize],
    ) {
        let rate = self.rate;
        let from = frame_at(self.spans[index].start, rate);
        let to = frame_at(self.spans[index].end(), rate);
        if to <= start {
            self.readers[index] = None;
            return;
        }
        if from >= end {
            return;
        }
        let first = start.max(from);
        let last = end.min(to);
        if self.readers[index].is_none() {
            self.open_reader(index, first - from);
        }
        let span = &self.spans[index];
        let Some(reader) = self.readers[index].as_mut() else {
            return;
        };
        let (begin, finish) = (first - from, last - from);
        let count = (finish - begin) as usize;
        let normal = span.speed.is_normal();
        let until = if normal {
            finish
        } else {
            reader
                .stretches
                .first()
                .map_or(finish, |stretch| stretch.needs_until(finish))
        };
        reader.fill_to(span, until, rate);

        let length = span.length();
        let mut stretched = Vec::new();
        for track in 0..reader.tracks.len() {
            if !span.wants(track) {
                continue;
            }
            let buffer = &mut reader.tracks[track];
            if normal {
                buffer.skip_to(begin);
            } else {
                stretched.clear();
                reader.stretches[track].produce(buffer, begin, count, &mut stretched);
            }
            let bus = span.bus.of(track);
            if let Some(out) = buses.get_mut(bus) {
                let offset = starts[bus] + (first - start) as usize * CHANNELS;
                let gain_of = |frame: usize| {
                    let into = time_of_frames(begin + frame as i64, rate);
                    span.sound.gain_at(into, length)
                };
                let fixed = fixed_gain(&span.sound, begin, finish, rate, length);
                let mut add = |place: usize, sample: f32| {
                    let gain = fixed.unwrap_or_else(|| gain_of(place / CHANNELS));
                    out[offset + place] += sample * gain;
                };
                if normal {
                    let samples = buffer.samples.iter().take(count * CHANNELS);
                    for (place, sample) in samples.enumerate() {
                        add(place, *sample);
                    }
                } else {
                    for (place, sample) in stretched.iter().enumerate() {
                        add(place, *sample);
                    }
                }
            }
            if normal {
                buffer.skip_to(finish);
            } else {
                let from = reader.stretches[track].keeps_from(finish);
                if from > buffer.first {
                    buffer.skip_to(from);
                }
            }
        }
    }
}

/// The gain over a stretch of a span if it doesn't change, which is nearly always.
fn fixed_gain(sound: &ClipSound, begin: i64, finish: i64, rate: u32, length: Ticks) -> Option<f32> {
    let fade_in = sound.fade_in_on(length);
    let fade_out = sound.fade_out_on(length);
    let after_fade_in = time_of_frames(begin, rate) >= fade_in;
    let before_fade_out = time_of_frames(finish, rate).0 <= length.0 - fade_out.0;
    (after_fade_in && before_fade_out).then(|| sound.gain_at(fade_in, length))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{path::Path, process::Command, sync::OnceLock};

    use bareclip_core::sound::{FULL_VOLUME, LOUDEST};

    const RATE: u32 = 48_000;

    /// A two-second file holding a constant value, written exactly, so sums can be checked exactly.
    fn level_file(name: &str, tracks: &[f32]) -> PathBuf {
        let folder = std::env::temp_dir().join("bareclip-audio-tests");
        std::fs::create_dir_all(&folder).expect("the folder can be created");
        let path = folder.join(format!("{name}-{}.mkv", std::process::id()));
        let mut command = Command::new("ffmpeg");
        command.args(["-hide_banner", "-loglevel", "error", "-y"]);
        for level in tracks {
            command.args(["-f", "lavfi", "-i"]);
            command.arg(format!("aevalsrc={level}|{level}:d=2:s={RATE}"));
        }
        for track in 0..tracks.len() {
            command.args(["-map", &track.to_string()]);
        }
        command.args(["-c:a", "pcm_f32le"]).arg(&path);
        assert!(command.status().expect("ffmpeg is on PATH").success());
        path
    }

    fn quarter() -> &'static Path {
        static FILE: OnceLock<PathBuf> = OnceLock::new();
        FILE.get_or_init(|| level_file("quarter", &[0.25]))
    }

    fn tenth() -> &'static Path {
        static FILE: OnceLock<PathBuf> = OnceLock::new();
        FILE.get_or_init(|| level_file("tenth", &[0.1]))
    }

    /// One file with game sound on its first track and mic on its second.
    fn game_and_mic() -> &'static Path {
        static FILE: OnceLock<PathBuf> = OnceLock::new();
        FILE.get_or_init(|| level_file("game-and-mic", &[0.2, 0.05]))
    }

    fn seconds(value: f64) -> Ticks {
        Ticks::from_seconds(value)
    }

    fn span(path: &Path, start: f64, source_start: f64, length: f64) -> MixSpan {
        MixSpan {
            path: path.to_owned(),
            start: seconds(start),
            source: TimeRange {
                start: seconds(source_start),
                end: seconds(source_start + length),
            },
            sound: ClipSound::default(),
            speed: Speed::default(),
            bus: Bus::All(0),
        }
    }

    fn at_speed(span: MixSpan, factor: f64) -> MixSpan {
        MixSpan {
            speed: Speed::default().with_factor(factor),
            ..span
        }
    }

    fn frames(time: f64) -> usize {
        (time * f64::from(RATE)) as usize
    }

    /// Mixes a stretch of the timeline into one bus.
    fn mixed(spans: Vec<MixSpan>, from: f64, length: f64) -> Vec<f32> {
        let mut mix = TimelineMix::new(spans, RATE, seconds(from));
        let mut buses = vec![Vec::new()];
        mix.fill(frames(length), &mut buses);
        assert_eq!(mix.take_problem(), None);
        buses.remove(0)
    }

    /// The level around a moment, measured from `from`.
    fn level_at(samples: &[f32], time: f64) -> f32 {
        samples[frames(time) * CHANNELS]
    }

    fn near(actual: f32, wanted: f32) {
        assert!((actual - wanted).abs() < 0.002, "{actual} is not {wanted}");
    }

    #[test]
    fn quiet_sound_passes_through_the_limit_unchanged() {
        for sample in [0.0, 0.5, -0.5, 0.8, -0.8] {
            assert_eq!(soft_limit(sample), sample);
        }
    }

    #[test]
    fn loud_sound_is_eased_in_under_full_scale_keeping_its_order() {
        let mut last = 0.8;
        for sample in [0.85, 1.0, 1.5, 2.0, 4.0, 100.0] {
            let limited = soft_limit(sample);
            assert!(
                limited >= last && limited <= 1.0,
                "{sample} became {limited}"
            );
            assert_eq!(soft_limit(-sample), -limited);
            last = limited;
        }
    }

    #[test]
    fn a_clip_is_heard_only_during_its_stretch_of_the_timeline() {
        let sound = mixed(vec![span(quarter(), 0.5, 0.0, 0.5)], 0.0, 1.5);

        near(level_at(&sound, 0.25), 0.0);
        near(level_at(&sound, 0.75), 0.25);
        near(level_at(&sound, 1.25), 0.0);
    }

    #[test]
    fn clips_playing_together_are_added_up() {
        let sound = mixed(
            vec![span(quarter(), 0.0, 0.0, 1.0), span(tenth(), 0.5, 0.0, 1.0)],
            0.0,
            1.5,
        );

        near(level_at(&sound, 0.25), 0.25);
        near(level_at(&sound, 0.75), 0.35);
        near(level_at(&sound, 1.25), 0.1);
    }

    #[test]
    fn volume_and_mute_change_what_is_heard() {
        let mut quiet = span(quarter(), 0.0, 0.0, 1.0);
        quiet.sound.volume = FULL_VOLUME / 2;
        near(level_at(&mixed(vec![quiet], 0.0, 1.0), 0.5), 0.125);

        let mut loud = span(tenth(), 0.0, 0.0, 1.0);
        loud.sound.volume = LOUDEST;
        near(level_at(&mixed(vec![loud], 0.0, 1.0), 0.5), 0.2);

        let mut muted = span(quarter(), 0.0, 0.0, 1.0);
        muted.sound.muted = true;
        near(level_at(&mixed(vec![muted], 0.0, 1.0), 0.5), 0.0);
    }

    #[test]
    fn fades_rise_from_and_fall_to_silence_on_the_timeline() {
        let mut fading = span(quarter(), 0.25, 0.0, 1.0);
        fading.sound.fade_in = seconds(0.2).0;
        fading.sound.fade_out = seconds(0.2).0;

        let sound = mixed(vec![fading], 0.0, 1.5);

        near(level_at(&sound, 0.25), 0.0);
        let rising = level_at(&sound, 0.35);
        assert!(rising > 0.1 && rising < 0.25, "{rising}");
        near(level_at(&sound, 0.75), 0.25);
        let falling = level_at(&sound, 1.15);
        assert!(falling > 0.1 && falling < 0.25, "{falling}");
        near(level_at(&sound, 1.26), 0.0);
    }

    #[test]
    fn a_fade_is_the_same_however_the_blocks_fall() {
        let mut fading = span(quarter(), 0.0, 0.0, 1.0);
        fading.sound.fade_in = seconds(0.3).0;
        let whole = mixed(vec![fading.clone()], 0.0, 1.0);

        let mut mix = TimelineMix::new(vec![fading], RATE, Ticks(0));
        let mut buses = vec![Vec::new()];
        while buses[0].len() < whole.len() {
            mix.fill(
                777.min((whole.len() - buses[0].len()) / CHANNELS),
                &mut buses,
            );
        }

        assert_eq!(buses[0], whole);
    }

    #[test]
    fn too_many_loud_clips_stay_under_full_scale() {
        let mut one = span(quarter(), 0.0, 0.0, 1.0);
        one.sound.volume = LOUDEST;
        let sound = mixed(vec![one.clone(), one.clone(), one], 0.0, 1.0);

        let highest = sound.iter().copied().fold(0.0, f32::max);
        assert!(highest > 0.9 && highest <= 1.0, "{highest}");
    }

    #[test]
    fn a_file_with_two_sound_tracks_is_heard_as_both_together() {
        let sound = mixed(vec![span(game_and_mic(), 0.0, 0.0, 1.0)], 0.0, 1.0);

        near(level_at(&sound, 0.5), 0.25);
    }

    #[test]
    fn detached_sound_plays_only_its_own_track() {
        let mut mic = span(game_and_mic(), 0.0, 0.0, 1.0);
        mic.sound.only_track = Some(1);

        near(level_at(&mixed(vec![mic], 0.0, 1.0), 0.5), 0.05);
    }

    #[test]
    fn tracks_can_be_kept_apart_on_buses_of_their_own() {
        let mut video = span(game_and_mic(), 0.0, 0.0, 1.0);
        video.bus = Bus::ByTrack(0);
        let mut song = span(quarter(), 0.0, 0.0, 1.0);
        song.bus = Bus::All(2);

        let mut mix = TimelineMix::new(vec![video, song], RATE, Ticks(0));
        let mut buses = vec![Vec::new(), Vec::new(), Vec::new()];
        mix.fill(frames(1.0), &mut buses);

        near(level_at(&buses[0], 0.5), 0.2);
        near(level_at(&buses[1], 0.5), 0.05);
        near(level_at(&buses[2], 0.5), 0.25);
    }

    #[test]
    fn starting_partway_through_a_clip_picks_up_at_the_right_place() {
        // The fade makes where in the clip it has got to easy to tell.
        let mut fading = span(quarter(), 0.0, 0.0, 2.0);
        fading.sound.fade_in = seconds(1.0).0;
        let from_start = mixed(vec![fading.clone()], 0.0, 1.0);
        let from_middle = mixed(vec![fading], 0.5, 0.25);

        near(level_at(&from_middle, 0.1), level_at(&from_start, 0.6));
    }

    #[test]
    fn seeking_moves_the_mix_and_it_carries_on_from_there() {
        let spans = vec![span(quarter(), 0.0, 0.0, 0.5), span(tenth(), 1.0, 0.0, 0.5)];
        let mut mix = TimelineMix::new(spans, RATE, Ticks(0));
        let mut buses = vec![Vec::new()];
        mix.fill(100, &mut buses);
        near(buses[0][0], 0.25);

        mix.seek(seconds(1.25));
        buses[0].clear();
        mix.fill(100, &mut buses);

        near(buses[0][0], 0.1);
        assert_eq!(mix.position(), frames(1.25) as i64 + 100);
    }

    #[test]
    fn nothing_on_the_timeline_is_silence_of_the_right_length() {
        let sound = mixed(Vec::new(), 0.0, 0.5);

        assert_eq!(sound.len(), frames(0.5) * CHANNELS);
        assert!(sound.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn a_change_of_volume_applies_without_reopening_the_file() {
        let first = span(quarter(), 0.0, 0.0, 1.0);
        let mut mix = TimelineMix::new(vec![first.clone()], RATE, Ticks(0));
        let mut buses = vec![Vec::new()];
        mix.fill(100, &mut buses);
        let open = mix.readers[0].as_ref().map(|reader| reader.tracks[0].first);

        let mut quieter = first;
        quieter.sound.volume = FULL_VOLUME / 2;
        mix.set_spans(vec![quieter]);
        assert_eq!(
            mix.readers[0].as_ref().map(|reader| reader.tracks[0].first),
            open,
            "the same reader carries on"
        );

        buses[0].clear();
        mix.fill(100, &mut buses);
        near(buses[0][0], 0.125);
    }

    #[test]
    fn a_change_of_where_a_clip_is_starts_it_over() {
        let first = span(quarter(), 0.0, 0.0, 1.0);
        let mut mix = TimelineMix::new(vec![first.clone()], RATE, Ticks(0));
        let mut buses = vec![Vec::new()];
        mix.fill(100, &mut buses);

        let mut moved = first;
        moved.start = seconds(0.5);
        mix.set_spans(vec![moved]);

        assert!(mix.readers[0].is_none());
    }

    #[test]
    fn what_starts_soon_is_opened_ahead_of_time_and_what_is_far_off_isnt() {
        let spans = vec![span(quarter(), 0.5, 0.0, 0.5), span(tenth(), 5.0, 0.0, 0.5)];
        let mut mix = TimelineMix::new(spans, RATE, Ticks(0));

        mix.prepare(seconds(1.0));

        assert!(mix.readers[0].is_some());
        assert!(mix.readers[1].is_none());
    }

    #[test]
    fn a_reader_closes_once_its_clip_has_gone_by() {
        let mut mix = TimelineMix::new(vec![span(quarter(), 0.0, 0.0, 0.25)], RATE, Ticks(0));
        let mut buses = vec![Vec::new()];
        mix.fill(frames(0.1), &mut buses);
        assert!(mix.readers[0].is_some());

        mix.fill(frames(0.5), &mut buses);
        mix.fill(frames(0.1), &mut buses);

        assert!(mix.readers[0].is_none());
    }

    #[test]
    fn sound_at_another_rate_comes_out_at_the_mix_rate_and_the_right_length() {
        let folder = std::env::temp_dir().join("bareclip-audio-tests");
        std::fs::create_dir_all(&folder).expect("the folder can be created");
        let path = folder.join(format!("low-rate-{}.wav", std::process::id()));
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
            .arg("aevalsrc=0.25|0.25:d=2:s=22050")
            .args(["-c:a", "pcm_f32le"])
            .arg(&path)
            .status()
            .expect("ffmpeg is on PATH");
        assert!(status.success());

        let sound = mixed(vec![span(&path, 0.0, 0.0, 1.0)], 0.0, 1.25);

        near(level_at(&sound, 0.5), 0.25);
        near(level_at(&sound, 0.95), 0.25);
        near(level_at(&sound, 1.1), 0.0);
    }

    #[test]
    fn a_file_that_cant_be_read_is_silence_and_says_so_once() {
        let missing = Path::new("nothing-is-here.wav");
        let mut mix = TimelineMix::new(vec![span(missing, 0.0, 0.0, 1.0)], RATE, Ticks(0));
        let mut buses = vec![Vec::new()];

        mix.fill(100, &mut buses);

        assert!(buses[0].iter().all(|sample| *sample == 0.0));
        assert!(mix.take_problem().is_some());
        assert!(mix.take_problem().is_none());
    }

    #[test]
    fn a_small_mismatch_between_blocks_joins_without_a_hole() {
        let mut buffer = TrackBuffer::default();
        buffer.add(0, &[0.5; 200]);

        buffer.add(103, &[0.5; 200]);

        assert_eq!(buffer.samples.len(), 400);
    }

    #[test]
    fn a_real_hole_between_blocks_becomes_silence() {
        let mut buffer = TrackBuffer::default();
        buffer.add(0, &[0.5; 200]);

        buffer.add(200, &[0.5; 200]);

        assert_eq!(buffer.end(), 300);
        assert_eq!(buffer.samples[200], 0.0);
        assert_eq!(buffer.samples[399], 0.0);
        assert_eq!(buffer.samples[400], 0.5);
    }

    #[test]
    fn sound_from_before_where_a_buffer_has_got_to_is_left_out() {
        let mut buffer = TrackBuffer {
            samples: VecDeque::new(),
            first: 1000,
        };

        buffer.add(900, &[0.5; 400]);

        assert_eq!(buffer.end(), 1100);
    }

    /// A file with a steady note, to measure what speed does to it.
    fn note_file() -> &'static Path {
        static FILE: OnceLock<PathBuf> = OnceLock::new();
        FILE.get_or_init(|| {
            let folder = std::env::temp_dir().join("bareclip-audio-tests");
            std::fs::create_dir_all(&folder).expect("the folder can be created");
            let path = folder.join(format!("note-{}.mkv", std::process::id()));
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
                .arg(format!("sine=f=440:r={RATE}:d=8"))
                .args(["-c:a", "pcm_f32le"])
                .arg(&path)
                .status()
                .expect("ffmpeg is on PATH");
            assert!(status.success());
            path
        })
    }

    fn frequency_of(left_and_right: &[f32]) -> f64 {
        let left: Vec<f32> = left_and_right.iter().step_by(CHANNELS).copied().collect();
        let crossings = left
            .windows(2)
            .filter(|pair| pair[0] <= 0.0 && pair[1] > 0.0)
            .count();
        crossings as f64 / (left.len() as f64 / f64::from(RATE))
    }

    /// How much faster than it plays sound at other speeds can be worked out. Run with
    /// `./scripts/cargo.ps1 test --release -p bareclip-audio stretching_keeps_ahead_of_real_time '--' --ignored --nocapture`
    #[test]
    #[ignore = "reports numbers rather than checking them, and needs an optimised build to mean anything"]
    fn stretching_keeps_ahead_of_real_time() {
        for (factor, keep_pitch) in [
            (1.0, true),
            (0.5, true),
            (2.0, true),
            (4.0, true),
            (16.0, true),
            (2.0, false),
            (16.0, false),
        ] {
            let quick = MixSpan {
                speed: Speed {
                    keep_pitch,
                    ..Speed::default().with_factor(factor)
                },
                ..span(note_file(), 0.0, 0.0, 8.0)
            };
            let length = quick.length().as_seconds();
            let started = std::time::Instant::now();
            let mut mix = TimelineMix::new(vec![quick], RATE, seconds(0.0));
            let mut buses = vec![Vec::new()];
            mix.fill(frames(length), &mut buses);
            let taken = started.elapsed().as_secs_f64();
            println!(
                "{factor}x, keep pitch {keep_pitch}: {length:.1} s of sound in {:.0} ms, {:.0} times real time",
                taken * 1000.0,
                length / taken
            );
        }
    }

    #[test]
    fn a_span_at_twice_the_speed_lasts_half_as_long_and_is_silent_after() {
        let quick = at_speed(span(quarter(), 0.0, 0.0, 2.0), 2.0);

        let out = mixed(vec![quick], 0.0, 1.5);

        // Stereo, past where the first grain fades in.
        let middle = out[frames(0.5) * CHANNELS];
        assert!(
            (middle - 0.25).abs() < 0.01,
            "the middle came out as {middle}"
        );
        let after = out[frames(1.2) * CHANNELS];
        assert!(
            after.abs() < 1e-6,
            "it should have ended at 1 s, got {after}"
        );
    }

    #[test]
    fn a_slow_span_lasts_longer() {
        let slow = at_speed(span(quarter(), 0.0, 0.0, 1.0), 0.5);

        let out = mixed(vec![slow], 0.0, 2.5);

        let late = out[frames(1.8) * CHANNELS];
        assert!((late - 0.25).abs() < 0.01, "at 1.8 s it came out as {late}");
        let after = out[frames(2.2) * CHANNELS];
        assert!(
            after.abs() < 1e-6,
            "it should have ended at 2 s, got {after}"
        );
    }

    #[test]
    fn a_fast_clip_keeps_its_note_when_asked_and_moves_it_when_not() {
        let quick = at_speed(span(note_file(), 0.0, 0.0, 4.0), 2.0);
        let kept = mixed(vec![quick.clone()], 0.2, 1.0);
        let tape = mixed(
            vec![MixSpan {
                speed: Speed {
                    keep_pitch: false,
                    ..quick.speed
                },
                ..quick
            }],
            0.2,
            1.0,
        );

        let (kept, tape) = (frequency_of(&kept), frequency_of(&tape));
        assert!((kept - 440.0).abs() < 15.0, "kept came out at {kept}");
        assert!((tape - 880.0).abs() < 30.0, "tape came out at {tape}");
    }

    #[test]
    fn starting_partway_through_a_fast_clip_reads_from_the_right_place_in_the_footage() {
        // Quarter level for its first second, then tenth level, by a file made of two.
        let folder = std::env::temp_dir().join("bareclip-audio-tests");
        let path = folder.join(format!("steps-{}.mkv", std::process::id()));
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
            .arg(format!(
                "aevalsrc='if(lt(t,2),0.25,0.1)|if(lt(t,2),0.25,0.1)':d=4:s={RATE}"
            ))
            .args(["-c:a", "pcm_f32le"])
            .arg(&path)
            .status()
            .expect("ffmpeg is on PATH");
        assert!(status.success());

        // At 2x, a second in on the timeline is two seconds into the footage.
        let quick = at_speed(span(&path, 0.0, 0.0, 4.0), 2.0);
        let before = mixed(vec![quick.clone()], 0.5, 0.2);
        let after = mixed(vec![quick], 1.2, 0.2);

        assert!((before[frames(0.1) * CHANNELS] - 0.25).abs() < 0.01);
        assert!((after[frames(0.1) * CHANNELS] - 0.1).abs() < 0.01);
    }

    #[test]
    fn fades_are_measured_on_the_timeline_at_any_speed() {
        let quick = MixSpan {
            sound: ClipSound {
                fade_out: seconds(0.5).0,
                ..ClipSound::default()
            },
            ..at_speed(span(quarter(), 0.0, 0.0, 2.0), 2.0)
        };

        let out = mixed(vec![quick], 0.0, 1.0);

        let halfway_down = out[frames(0.75) * CHANNELS];
        assert!(
            halfway_down > 0.05 && halfway_down < 0.24,
            "the fade should be partway at 0.75 s, got {halfway_down}"
        );
    }

    #[test]
    fn changing_speed_while_playing_opens_the_sound_again_in_the_right_place() {
        let mut mix = TimelineMix::new(vec![span(quarter(), 0.0, 0.0, 2.0)], RATE, seconds(0.0));
        let mut buses = vec![Vec::new()];
        mix.fill(frames(0.2), &mut buses);

        mix.set_spans(vec![at_speed(span(quarter(), 0.0, 0.0, 2.0), 2.0)]);
        buses[0].clear();
        mix.fill(frames(0.2), &mut buses);

        assert_eq!(mix.take_problem(), None);
        let late = buses[0][frames(0.15) * CHANNELS];
        assert!((late - 0.25).abs() < 0.01, "{late}");
    }

    #[test]
    fn a_normal_speed_span_with_tape_pitch_still_passes_straight_through() {
        let plain = MixSpan {
            speed: Speed {
                keep_pitch: false,
                ..Speed::default()
            },
            ..span(quarter(), 0.0, 0.0, 1.0)
        };

        let out = mixed(vec![plain], 0.0, 0.5);

        assert!(out.iter().all(|sample| (*sample - 0.25).abs() < 1e-6));
    }

    /// Writes mono sound as a 48 kHz stereo WAV of 32-bit floats.
    fn wav_file(name: &str, mono: &[f32]) -> PathBuf {
        let folder = std::env::temp_dir().join("bareclip-audio-tests");
        std::fs::create_dir_all(&folder).expect("the folder can be created");
        let path = folder.join(format!("{name}-{}.wav", std::process::id()));
        let data_bytes = (mono.len() * CHANNELS * 4) as u32;
        let mut bytes = Vec::with_capacity(44 + data_bytes as usize);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&3u16.to_le_bytes());
        bytes.extend_from_slice(&(CHANNELS as u16).to_le_bytes());
        bytes.extend_from_slice(&RATE.to_le_bytes());
        bytes.extend_from_slice(&(RATE * CHANNELS as u32 * 4).to_le_bytes());
        bytes.extend_from_slice(&((CHANNELS * 4) as u16).to_le_bytes());
        bytes.extend_from_slice(&32u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_bytes.to_le_bytes());
        for sample in mono {
            for _ in 0..CHANNELS {
                bytes.extend_from_slice(&sample.to_le_bytes());
            }
        }
        std::fs::write(&path, bytes).expect("the file can be written");
        path
    }

    /// Talking from 0.5 to 1.5 seconds and from 2.5 to 3.5, in a four-second file.
    fn talking(time: f64) -> bool {
        (0.5..1.5).contains(&time) || (2.5..3.5).contains(&time)
    }

    fn voice_only() -> &'static Path {
        static FILE: OnceLock<PathBuf> = OnceLock::new();
        FILE.get_or_init(|| wav_file("voice", &crate::clean::sounds::voice(4.0, talking)))
    }

    /// The same voice with a fan's steady hiss under it the whole time.
    fn voice_and_fan() -> &'static Path {
        static FILE: OnceLock<PathBuf> = OnceLock::new();
        FILE.get_or_init(|| {
            use crate::clean::sounds::{Dice, hiss, voice};
            let voice = voice(4.0, talking);
            let fan = hiss(voice.len(), 0.02, &mut Dice::new(11));
            let both: Vec<f32> = voice.iter().zip(&fan).map(|(v, f)| v + f).collect();
            wav_file("voice-and-fan", &both)
        })
    }

    fn cleaned(span: MixSpan) -> MixSpan {
        MixSpan {
            sound: ClipSound {
                noise_suppression: true,
                ..span.sound
            },
            ..span
        }
    }

    fn mixed_at(spans: Vec<MixSpan>, rate: u32, from: f64, length: f64) -> Vec<f32> {
        let mut mix = TimelineMix::new(spans, rate, seconds(from));
        let mut buses = vec![Vec::new()];
        mix.fill((length * f64::from(rate)) as usize, &mut buses);
        assert_eq!(mix.take_problem(), None);
        buses.remove(0)
    }

    /// The left side of a stretch of a mix that started at 0.
    fn stretch_of(samples: &[f32], rate: u32, from: f64, to: f64) -> Vec<f32> {
        let at = |time: f64| (time * f64::from(rate)) as usize * CHANNELS;
        samples[at(from)..at(to)]
            .iter()
            .step_by(CHANNELS)
            .copied()
            .collect()
    }

    /// How many frames later `late` is than `early`, judged by where they line up best.
    fn lag_between(early: &[f32], late: &[f32], most: i64) -> i64 {
        let score = |lag: i64| -> f64 {
            (most as usize..early.len() - most as usize)
                .map(|at| f64::from(early[at]) * f64::from(late[(at as i64 + lag) as usize]))
                .sum()
        };
        (-most..=most)
            .max_by(|a, b| score(*a).total_cmp(&score(*b)))
            .unwrap()
    }

    #[test]
    fn noise_suppression_quietens_a_fan_between_words_and_keeps_the_words() {
        use crate::clean::sounds::decibels;
        let plain = mixed(vec![span(voice_and_fan(), 0.0, 0.0, 4.0)], 0.0, 4.0);
        let cleaned = mixed(
            vec![cleaned(span(voice_and_fan(), 0.0, 0.0, 4.0))],
            0.0,
            4.0,
        );
        let voice = mixed(vec![span(voice_only(), 0.0, 0.0, 4.0)], 0.0, 4.0);

        let fan_before = decibels(&stretch_of(&plain, RATE, 1.7, 2.3));
        let fan_after = decibels(&stretch_of(&cleaned, RATE, 1.7, 2.3));
        assert!(
            fan_after < fan_before - 15.0,
            "between words the fan went from {fan_before:.1} dB to {fan_after:.1} dB"
        );
        let words = decibels(&stretch_of(&voice, RATE, 0.7, 1.3));
        let words_after = decibels(&stretch_of(&cleaned, RATE, 0.7, 1.3));
        assert!(
            (words_after - words).abs() < 3.0,
            "the words were {words:.1} dB and came out {words_after:.1} dB"
        );
    }

    #[test]
    fn cleaned_sound_stays_in_time() {
        let plain = mixed(vec![span(voice_only(), 0.0, 0.0, 4.0)], 0.0, 4.0);
        let cleaned = mixed(vec![cleaned(span(voice_only(), 0.0, 0.0, 4.0))], 0.0, 4.0);

        let lag = lag_between(
            &stretch_of(&plain, RATE, 0.6, 1.4),
            &stretch_of(&cleaned, RATE, 0.6, 1.4),
            600,
        );
        assert!(lag.abs() <= 2, "it came out {lag} frames late");
    }

    #[test]
    fn jumping_into_the_middle_of_a_word_sounds_as_it_does_playing_through() {
        use crate::clean::sounds::decibels;
        let clip = cleaned(span(voice_and_fan(), 0.0, 0.0, 4.0));
        let through = mixed(vec![clip.clone()], 0.0, 4.0);
        let jumped = mixed(vec![clip], 2.8, 0.5);

        let expected = stretch_of(&through, RATE, 2.8, 3.3);
        let got = stretch_of(&jumped, RATE, 0.0, 0.5);
        let difference: Vec<f32> = expected.iter().zip(&got).map(|(a, b)| a - b).collect();
        let (level, off) = (decibels(&expected), decibels(&difference));
        assert!(
            off < level - 20.0,
            "the sound was {level:.1} dB and off by {off:.1} dB"
        );
    }

    #[test]
    fn a_sound_card_at_another_rate_hears_the_same_cleaning_in_time() {
        use crate::clean::sounds::decibels;
        const CARD: u32 = 44_100;
        let plain = mixed_at(vec![span(voice_and_fan(), 0.0, 0.0, 4.0)], CARD, 0.0, 4.0);
        let cleaned_fan = mixed_at(
            vec![cleaned(span(voice_and_fan(), 0.0, 0.0, 4.0))],
            CARD,
            0.0,
            4.0,
        );
        assert_eq!(cleaned_fan.len(), plain.len());
        let fan_before = decibels(&stretch_of(&plain, CARD, 1.7, 2.3));
        let fan_after = decibels(&stretch_of(&cleaned_fan, CARD, 1.7, 2.3));
        assert!(
            fan_after < fan_before - 15.0,
            "{fan_before:.1} dB to {fan_after:.1} dB"
        );

        let voice = mixed_at(vec![span(voice_only(), 0.0, 0.0, 4.0)], CARD, 0.0, 4.0);
        let cleaned_voice = mixed_at(
            vec![cleaned(span(voice_only(), 0.0, 0.0, 4.0))],
            CARD,
            0.0,
            4.0,
        );
        let lag = lag_between(
            &stretch_of(&voice, CARD, 0.6, 1.4),
            &stretch_of(&cleaned_voice, CARD, 0.6, 1.4),
            600,
        );
        assert!(lag.abs() <= 2, "it came out {lag} frames late");
    }

    #[test]
    fn cleaned_sound_at_twice_the_speed_lasts_half_as_long() {
        use crate::clean::sounds::decibels;
        let fast = at_speed(cleaned(span(voice_and_fan(), 0.0, 0.0, 4.0)), 2.0);
        let out = mixed(vec![fast], 0.0, 2.5);

        assert!(
            decibels(&stretch_of(&out, RATE, 0.3, 0.7)) > -60.0,
            "the words are heard"
        );
        assert!(stretch_of(&out, RATE, 2.05, 2.5).iter().all(|s| *s == 0.0));
    }

    #[test]
    fn turning_noise_suppression_on_reads_the_clip_again() {
        let plain = span(voice_and_fan(), 0.0, 0.0, 4.0);
        assert!(!plain.reads_same_as(&cleaned(plain.clone())));
        let quieter = MixSpan {
            sound: ClipSound {
                volume: FULL_VOLUME / 2,
                ..plain.sound
            },
            ..plain.clone()
        };
        assert!(
            plain.reads_same_as(&quieter),
            "a volume change still carries on"
        );
    }

    /// How much faster than real time cleaning runs, at the export's rate and a common sound
    /// card's, and how long a jump waits for its lead-in.
    #[test]
    #[ignore]
    fn cleaning_keeps_ahead_of_real_time() {
        for rate in [RATE, 44_100] {
            let clip = cleaned(span(voice_and_fan(), 0.0, 0.0, 4.0));
            let started = std::time::Instant::now();
            let out = mixed_at(vec![clip.clone()], rate, 0.0, 4.0);
            let taken = started.elapsed().as_secs_f64();
            assert!(!out.is_empty());
            let started = std::time::Instant::now();
            mixed_at(vec![clip], rate, 2.8, 0.01);
            let jump = started.elapsed().as_secs_f64();
            println!(
                "{rate} Hz: 4 s cleaned in {:.0} ms, {:.0} times real time. A jump waits {:.1} ms",
                taken * 1000.0,
                4.0 / taken,
                jump * 1000.0
            );
        }
    }
}
