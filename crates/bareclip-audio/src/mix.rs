//! Adds up everything that can be heard on the timeline, a block at a time. The preview and the
//! export both use this, so what is heard is what gets saved.

use std::{collections::VecDeque, path::PathBuf};

use bareclip_core::{
    sound::ClipSound,
    time::{TICKS_PER_SECOND, Ticks},
    timeline::TimeRange,
};
use bareclip_media::{AudioPreview, AudioStep, MediaError};

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
    pub bus: Bus,
}

impl MixSpan {
    fn length(&self) -> Ticks {
        self.source.duration()
    }

    fn end(&self) -> Ticks {
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
            && self.sound.only_track == other.sound.only_track
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

/// One open file, positioned for one span.
struct SpanReader {
    preview: AudioPreview,
    tracks: Vec<TrackBuffer>,
    source_start: i64,
    ended: bool,
}

impl SpanReader {
    fn open(span: &MixSpan, rate: u32, from: i64) -> Result<Self, MediaError> {
        let mut preview = AudioPreview::open(&span.path, rate)?;
        let seconds = span.source.start.as_seconds() + from as f64 / f64::from(rate);
        preview.seek(seconds)?;
        let tracks = (0..preview.track_count())
            .map(|_| TrackBuffer {
                samples: VecDeque::new(),
                first: from,
            })
            .collect();
        Ok(Self {
            preview,
            tracks,
            source_start: frame_at(span.source.start, rate),
            ended: false,
        })
    }

    /// Decodes until every track the span wants has sound up to this frame of the span.
    fn fill_to(&mut self, span: &MixSpan, until: i64, rate: u32) {
        let furthest = until + MOST_SECONDS_AHEAD * i64::from(rate);
        while !self.ended {
            let lacking = self
                .tracks
                .iter()
                .enumerate()
                .any(|(track, buffer)| span.wants(track) && buffer.end() < until);
            let room = self.tracks.iter().all(|buffer| buffer.end() < furthest);
            if !lacking || !room {
                return;
            }
            match self.preview.next_step() {
                Ok(AudioStep::Chunk(chunk)) => {
                    if span.wants(chunk.track)
                        && let Some(buffer) = self.tracks.get_mut(chunk.track)
                    {
                        buffer.add(chunk.start_frame - self.source_start, &chunk.samples);
                    }
                }
                Ok(AudioStep::Pending) => {}
                // A damaged file should cost a moment of sound, not the whole mix.
                Ok(AudioStep::End) | Err(_) => self.ended = true,
            }
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
        reader.fill_to(span, finish, rate);

        let length = span.length();
        for (track, buffer) in reader.tracks.iter_mut().enumerate() {
            if !span.wants(track) {
                continue;
            }
            buffer.skip_to(begin);
            let bus = span.bus.of(track);
            if let Some(out) = buses.get_mut(bus) {
                let offset = starts[bus] + (first - start) as usize * CHANNELS;
                let count = (finish - begin) as usize;
                let samples = buffer.samples.iter().take(count * CHANNELS);
                let gain_of = |frame: usize| {
                    let into = time_of_frames(begin + frame as i64, rate);
                    span.sound.gain_at(into, length)
                };
                let fixed = fixed_gain(&span.sound, begin, finish, rate, length);
                for (place, sample) in samples.enumerate() {
                    let gain = fixed.unwrap_or_else(|| gain_of(place / CHANNELS));
                    out[offset + place] += sample * gain;
                }
            }
            buffer.skip_to(finish);
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
            bus: Bus::All(0),
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
}
