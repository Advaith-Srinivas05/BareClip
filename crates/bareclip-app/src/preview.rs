//! Plays the timeline. Its sound is one mix of everything that can be heard, and how much of that
//! the sound card has played is the clock. Each video's pictures follow that clock, so the sound
//! doesn't stop or restart when one clip gives way to the next, even in dead space.

use std::time::{Duration, Instant};

use bareclip_audio::{MixSpan, SoundEngine};
use bareclip_core::{
    playback::advance_from_time,
    time::Ticks,
    timeline::{Clip, ClipId, MediaId, Timeline},
};
use bareclip_media::{MediaInfo, PreviewFrame};

use crate::player::{Notify, Player};

/// Open players hold decoders, so only a few stay open.
const MAX_OPEN_PLAYERS: usize = 4;
/// How long before it is needed the next clip gets its pictures ready, so moving on to it needs no
/// pause. Longer than the slowest seek.
pub const PREPARE_AHEAD_SECONDS: f64 = 1.0;
/// How long playback waits for its first picture and sound before starting anyway.
const START_TIMEOUT: Duration = Duration::from_millis(400);

struct Slot {
    media: MediaId,
    player: Player,
}

/// A player getting ready for the clip that comes next.
struct Prepared {
    clip: ClipId,
    media: MediaId,
    player: Player,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Clock {
    Stopped,
    /// Waiting for the first picture and enough sound, holding still until then.
    Waiting(Instant),
    Running(Instant),
}

/// Where a moment on the timeline falls in a clip's video.
fn source_time_in(clip: &Clip, now: Ticks) -> Ticks {
    Ticks(clip.source.start.0 + now.0 - clip.start.0).clamp(clip.source.start, clip.source.end)
}

pub struct Preview {
    /// Most recently used last.
    slots: Vec<Slot>,
    prepared: Option<Prepared>,
    current: Option<(ClipId, MediaId)>,
    playing: bool,
    position: Ticks,
    clock: Clock,
    /// Where the clock was when it last started.
    clock_base: Ticks,
    use_sound: bool,
    /// Connected the first time anything plays.
    sound: Option<SoundEngine>,
    sound_failed: bool,
    spans: Vec<MixSpan>,
    /// How long the whole project is, which sound can make longer than its pictures.
    project_length: Ticks,
    /// Whether pictures come out ready for the GPU compositor.
    planes: bool,
    /// The user moved the playhead, or started or stopped, since this was last asked about.
    jumped: bool,
    /// Which piece of the timeline the last picture came from.
    last_picture_from: Option<Clip>,
    notify: Notify,
    error: Option<String>,
}

impl Preview {
    pub fn new(use_sound: bool, notify: Notify) -> Self {
        Self {
            slots: Vec::new(),
            prepared: None,
            current: None,
            playing: false,
            position: Ticks(0),
            clock: Clock::Stopped,
            clock_base: Ticks(0),
            use_sound,
            sound: None,
            sound_failed: false,
            spans: Vec::new(),
            project_length: Ticks(0),
            planes: false,
            jumped: false,
            last_picture_from: None,
            notify,
            error: None,
        }
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    pub fn take_error(&mut self) -> Option<String> {
        self.slots
            .iter_mut()
            .find_map(|slot| slot.player.take_error())
            .or_else(|| self.sound.as_ref().and_then(SoundEngine::take_problem))
            .or_else(|| self.error.take())
    }

    /// Says what can be heard and how long the project is. Anything already playing carries on
    /// with the changes, so a change of volume is heard straight away.
    pub fn set_sound(&mut self, spans: Vec<MixSpan>, project_length: Ticks) {
        self.project_length = project_length;
        if spans != self.spans {
            if let Some(sound) = &self.sound {
                sound.set_spans(spans.clone());
            }
            self.spans = spans;
        }
    }

    /// Says how long the project is, for a change that has to take effect straight away.
    pub fn set_project_length(&mut self, length: Ticks) {
        self.project_length = length;
    }

    /// Whether what should be heard can be.
    pub fn sound_works(&self) -> bool {
        !self.sound_failed || self.spans.is_empty()
    }

    /// Whether the playhead is in dead space, where the picture is black.
    pub fn in_gap(&self, timeline: &Timeline) -> bool {
        self.current.is_none() && !timeline.is_empty()
    }

    fn length(&self, timeline: &Timeline) -> Ticks {
        self.project_length.max(timeline.duration())
    }

    /// Where the playhead is on the timeline.
    pub fn position(&self, timeline: &Timeline) -> Ticks {
        if !self.playing {
            return self.position;
        }
        let Clock::Running(since) = self.clock else {
            return self.clock_base;
        };
        let passed = match &self.sound {
            Some(sound) => sound.played(),
            None => advance_from_time(since.elapsed()),
        };
        Ticks(self.clock_base.0 + passed.0).min(self.length(timeline))
    }

    fn connect_sound(&mut self) {
        if !self.use_sound || self.sound.is_some() || self.sound_failed {
            return;
        }
        match SoundEngine::open() {
            Ok(sound) => {
                sound.set_spans(self.spans.clone());
                self.sound = Some(sound);
            }
            // Playback carries on against the real time, and the top row says why it is silent.
            Err(_) => self.sound_failed = true,
        }
    }

    fn start_clock_from(&mut self, at: Ticks) {
        self.clock_base = at;
        self.clock = Clock::Waiting(Instant::now());
        if let Some(sound) = &self.sound {
            sound.start_at(at);
        }
    }

    /// Lets the clock go once there is a picture to show and sound to play.
    fn start_clock_when_ready(&mut self) {
        let Clock::Waiting(since) = self.clock else {
            return;
        };
        let picture_ready = self.current.is_none_or(|(_, media)| {
            self.slots
                .iter()
                .find(|slot| slot.media == media)
                .is_none_or(|slot| slot.player.has_first_picture())
        });
        let sound_ready = self.sound.as_ref().is_none_or(SoundEngine::is_ready);
        if (picture_ready && sound_ready) || since.elapsed() >= START_TIMEOUT {
            if let Some(sound) = &self.sound {
                sound.resume();
            }
            self.clock = Clock::Running(Instant::now());
        }
    }

    fn stop_clock(&mut self) {
        self.clock = Clock::Stopped;
        if let Some(sound) = &self.sound {
            sound.pause();
        }
    }

    fn player_for(
        &mut self,
        media: MediaId,
        lookup: &dyn Fn(MediaId) -> Option<MediaInfo>,
    ) -> Option<&mut Player> {
        if let Some(position) = self.slots.iter().position(|slot| slot.media == media) {
            let slot = self.slots.remove(position);
            self.slots.push(slot);
        } else {
            let info = lookup(media)?;
            match Player::open_as(&info, self.planes, self.notify.clone()) {
                Ok(player) => {
                    if self.slots.len() >= MAX_OPEN_PLAYERS {
                        self.slots.remove(0);
                    }
                    self.slots.push(Slot { media, player });
                }
                Err(error) => {
                    self.error = Some(error);
                    return None;
                }
            }
        }
        self.slots.last_mut().map(|slot| &mut slot.player)
    }

    fn pause_all(&mut self) {
        for slot in &mut self.slots {
            slot.player.pause();
        }
    }

    /// Whether pictures are coming out ready for the GPU compositor.
    pub fn planes(&self) -> bool {
        self.planes
    }

    /// Switches between plain pictures and ones ready for the GPU compositor. Open players can't
    /// change over, so they close, and playback picks up from the same place.
    pub fn set_planes(
        &mut self,
        planes: bool,
        timeline: &Timeline,
        lookup: &dyn Fn(MediaId) -> Option<MediaInfo>,
    ) {
        if self.planes == planes {
            return;
        }
        let position = self.position(timeline);
        self.planes = planes;
        self.slots.clear();
        self.prepared = None;
        self.current = None;
        self.seek(timeline, lookup, position);
    }

    /// Whether the playhead was moved by hand, or playback started or stopped, since the last time
    /// this was asked. Moving on to the next clip by itself doesn't count.
    pub fn take_jump(&mut self) -> bool {
        std::mem::take(&mut self.jumped)
    }

    /// Which piece of the timeline the picture `update` last returned came from.
    pub fn last_picture_from(&self) -> Option<Clip> {
        self.last_picture_from
    }

    /// Moves the playhead to a moment on the timeline.
    pub fn seek(
        &mut self,
        timeline: &Timeline,
        lookup: &dyn Fn(MediaId) -> Option<MediaInfo>,
        time: Ticks,
    ) {
        self.jumped = true;
        self.prepared = None;
        self.go_to(timeline, lookup, time);
        if self.playing {
            self.start_clock_from(self.position);
        }
    }

    /// Starts getting the clip after this moment ready, if it is about to be needed.
    fn prepare_next(
        &mut self,
        timeline: &Timeline,
        lookup: &dyn Fn(MediaId) -> Option<MediaInfo>,
        now: Ticks,
    ) {
        let ahead = Ticks::from_seconds(PREPARE_AHEAD_SECONDS);
        let next = timeline
            .clips()
            .iter()
            .find(|clip| clip.start > now && clip.start.0 - now.0 <= ahead.0)
            .copied();
        let Some(next) = next else {
            return;
        };
        let known = self
            .prepared
            .as_ref()
            .is_some_and(|prepared| prepared.clip == next.id);
        if known {
            return;
        }
        let Some(info) = lookup(next.media) else {
            return;
        };
        match Player::open_as(&info, self.planes, self.notify.clone()) {
            Ok(mut player) => {
                player.prepare(next.source.start);
                self.prepared = Some(Prepared {
                    clip: next.id,
                    media: next.media,
                    player,
                });
            }
            Err(error) => self.error = Some(error),
        }
    }

    /// Takes in what the prepared player has decoded, so that it keeps going until it is full.
    fn keep_preparing(&mut self) {
        if let Some(prepared) = &mut self.prepared {
            prepared.player.update();
            if let Some(error) = prepared.player.take_error() {
                self.error = Some(error);
            }
        }
    }

    /// Moves on to the prepared player if it is ready for exactly this clip and moment. A
    /// player for the same video isn't needed any more, because this one has taken over.
    fn take_prepared(&mut self, clip: ClipId, source_time: Ticks) -> bool {
        let matches = self.prepared.as_ref().is_some_and(|prepared| {
            prepared.clip == clip && prepared.player.prepared_for() == Some(source_time)
        });
        if !matches {
            return false;
        }
        let Some(mut prepared) = self.prepared.take() else {
            return false;
        };
        self.pause_all();
        self.slots.retain(|slot| slot.media != prepared.media);
        if self.slots.len() >= MAX_OPEN_PLAYERS {
            self.slots.remove(0);
        }
        prepared.player.begin();
        self.current = Some((clip, prepared.media));
        self.slots.push(Slot {
            media: prepared.media,
            player: prepared.player,
        });
        true
    }

    /// Puts the pictures at a moment. This never touches the clock, so moving on from one clip to
    /// the next doesn't disturb the sound.
    fn go_to(
        &mut self,
        timeline: &Timeline,
        lookup: &dyn Fn(MediaId) -> Option<MediaInfo>,
        time: Ticks,
    ) {
        self.position = time.clamp(Ticks(0), self.length(timeline));
        let Some(location) = timeline.locate(self.position) else {
            self.pause_all();
            self.current = None;
            if self.length(timeline) == Ticks(0) {
                self.playing = false;
                self.stop_clock();
            }
            return;
        };

        if self.playing && self.take_prepared(location.clip.id, location.source_time) {
            return;
        }

        let media = location.clip.media;
        let changed_video = self.current.map(|(_, current)| current) != Some(media);
        if changed_video {
            self.pause_all();
        }
        self.current = Some((location.clip.id, media));
        let keep_playing = self.playing;
        if let Some(player) = self.player_for(media, lookup) {
            if changed_video {
                player.forget_pictures();
            }
            player.seek(location.source_time);
            if keep_playing && !player.is_playing() {
                player.play();
            }
        }
    }

    #[cfg(test)]
    pub fn seeks_sent(&self) -> u64 {
        self.slots.iter().map(|slot| slot.player.seeks_sent()).sum()
    }

    pub fn play(&mut self, timeline: &Timeline, lookup: &dyn Fn(MediaId) -> Option<MediaInfo>) {
        let length = self.length(timeline);
        if length == Ticks(0) || self.playing {
            return;
        }
        let start = if self.position >= length {
            Ticks(0)
        } else {
            self.position
        };
        self.connect_sound();
        self.playing = true;
        self.seek(timeline, lookup, start);
    }

    pub fn pause(&mut self, timeline: &Timeline) {
        if !self.playing {
            return;
        }
        self.position = self.position(timeline);
        self.playing = false;
        self.jumped = true;
        self.prepared = None;
        self.stop_clock();
        self.pause_all();
    }

    pub fn toggle(&mut self, timeline: &Timeline, lookup: &dyn Fn(MediaId) -> Option<MediaInfo>) {
        if self.playing {
            self.pause(timeline);
        } else {
            self.play(timeline, lookup);
        }
    }

    /// Moves between clips as the clock reaches them, and returns the picture to show now.
    pub fn update(
        &mut self,
        timeline: &Timeline,
        lookup: &dyn Fn(MediaId) -> Option<MediaInfo>,
    ) -> Option<PreviewFrame> {
        let frame = self.update_pictures(timeline, lookup);
        self.start_clock_when_ready();
        frame
    }

    fn stop_at_the_end(&mut self, timeline: &Timeline) -> bool {
        let length = self.length(timeline);
        if self.position(timeline) < length {
            return false;
        }
        self.position = length;
        self.playing = false;
        self.stop_clock();
        self.pause_all();
        true
    }

    fn update_pictures(
        &mut self,
        timeline: &Timeline,
        lookup: &dyn Fn(MediaId) -> Option<MediaInfo>,
    ) -> Option<PreviewFrame> {
        let Some((clip_id, media)) = self.current else {
            if self.playing && !self.stop_at_the_end(timeline) {
                self.prepare_next(timeline, lookup, self.position(timeline));
                self.keep_preparing();
                self.leave_gap_when_due(timeline, lookup);
            }
            return None;
        };
        let Some(index) = timeline.index_of(clip_id) else {
            // The clip was removed, so the playhead finds whatever is there now.
            let position = self.position(timeline);
            self.go_to(timeline, lookup, position);
            return None;
        };
        let clip = timeline.clips()[index];
        self.last_picture_from = Some(clip);
        let now = self.position(timeline);
        let playing = self.playing;
        let slot = self.slots.iter_mut().find(|slot| slot.media == media)?;
        let frame = if playing {
            slot.player.update_following(source_time_in(&clip, now))
        } else {
            slot.player.update()
        };

        if playing {
            self.position = now;
            self.prepare_next(timeline, lookup, now);
            self.keep_preparing();
            if now >= clip.end() && !self.stop_at_the_end(timeline) {
                self.go_to(timeline, lookup, clip.end());
            }
        }

        // A frame from just past the clip's end belongs to footage cut away.
        frame.filter(|frame| frame.time < clip.source.end)
    }

    /// Starts the next clip once the clock has run through the dead space before it.
    fn leave_gap_when_due(
        &mut self,
        timeline: &Timeline,
        lookup: &dyn Fn(MediaId) -> Option<MediaInfo>,
    ) {
        let now = self.position(timeline);
        if timeline.locate(now).is_some() {
            let next = timeline.gap_at(self.position).map_or(now, |gap| gap.end);
            self.go_to(timeline, lookup, next);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::{Path, PathBuf},
        process::Command,
        sync::{Arc, OnceLock},
        thread,
        time::{Duration, Instant},
    };

    use bareclip_core::{
        timeline::TimeRange,
        tracks::{TrackTarget, Tracks},
    };
    use bareclip_media::probe;

    fn make_clip(name: &str, first_number: u32) -> PathBuf {
        let folder = std::env::temp_dir().join("bareclip-app-tests");
        std::fs::create_dir_all(&folder).expect("the folder can be created");
        let path = folder.join(format!("{name}-{}.mp4", std::process::id()));
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
                "nullsrc=s=320x180:r=30:d=3,\
                 geq=lum='255*mod(floor((N+{first_number})/pow(2,floor(X/20))),2)':cb=128:cr=128,\
                 format=yuv420p"
            ))
            .args(["-c:v", "libopenh264", "-b:v", "4M", "-g", "30"])
            .arg(&path)
            .status()
            .expect("ffmpeg is on PATH");
        assert!(status.success());
        path
    }

    fn no_notify() -> Notify {
        Arc::new(|| {})
    }

    fn first_clip() -> &'static Path {
        static CLIP: OnceLock<PathBuf> = OnceLock::new();
        CLIP.get_or_init(|| make_clip("preview-a", 0))
    }

    fn second_clip() -> &'static Path {
        static CLIP: OnceLock<PathBuf> = OnceLock::new();
        CLIP.get_or_init(|| make_clip("preview-b", 1000))
    }

    fn lookup(media: MediaId) -> Option<MediaInfo> {
        let path = if media == MediaId(1) {
            first_clip()
        } else {
            second_clip()
        };
        probe(path).ok()
    }

    fn seconds(value: f64) -> Ticks {
        Ticks::from_seconds(value)
    }

    fn range(start: f64, end: f64) -> TimeRange {
        TimeRange {
            start: seconds(start),
            end: seconds(end),
        }
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

    fn wait_for_frame(preview: &mut Preview, timeline: &Timeline, number: u32) {
        let started = Instant::now();
        let mut last = None;
        while started.elapsed() < Duration::from_secs(10) {
            if let Some(frame) = preview.update(timeline, &lookup) {
                last = Some(number_of(&frame));
                if last == Some(number) {
                    return;
                }
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!("frame {number} never appeared, last was {last:?}");
    }

    fn two_clip_timeline() -> Timeline {
        let mut timeline = Timeline::default();
        timeline.push(MediaId(1), range(1.0, 1.5));
        timeline.push(MediaId(2), range(0.5, 1.0));
        timeline
    }

    /// A clip of the first video, with half a second of the second one on a track above.
    fn covered_tracks() -> Tracks {
        let mut tracks = Tracks::default();
        tracks.place(
            MediaId(1),
            range(1.0, 2.0),
            seconds(0.0),
            TrackTarget::New(0),
        );
        tracks.place(
            MediaId(2),
            range(0.5, 1.0),
            seconds(0.25),
            TrackTarget::New(0),
        );
        tracks
    }

    #[test]
    fn a_higher_track_shows_over_a_lower_one_and_the_lower_one_carries_on_after_it() {
        let tracks = covered_tracks();
        let played = tracks.played();
        let mut preview = Preview::new(false, no_notify());

        preview.seek(played, &lookup, seconds(0.1));
        wait_for_frame(&mut preview, played, 33);

        // 0.1 s into the upper clip, which starts half a second into its video.
        preview.seek(played, &lookup, seconds(0.35));
        wait_for_frame(&mut preview, played, 1018);

        // 0.8 s into the lower clip, not 0.3 s: the time under the upper clip still passed.
        preview.seek(played, &lookup, seconds(0.8));
        wait_for_frame(&mut preview, played, 54);
    }

    #[test]
    fn playback_runs_from_a_lower_track_to_a_higher_one_and_back() {
        let tracks = covered_tracks();
        let played = tracks.played();
        assert_eq!(played.clips().len(), 3);
        let mut preview = Preview::new(false, no_notify());

        preview.play(played, &lookup);

        wait_for_frame(&mut preview, played, 1016);
        wait_for_frame(&mut preview, played, 54);
    }

    #[test]
    fn seeking_shows_the_right_frame_from_the_right_source() {
        let timeline = two_clip_timeline();
        let mut preview = Preview::new(false, no_notify());

        preview.seek(&timeline, &lookup, seconds(0.1));
        wait_for_frame(&mut preview, &timeline, 33);

        preview.seek(&timeline, &lookup, seconds(0.6));
        wait_for_frame(&mut preview, &timeline, 1018);

        preview.seek(&timeline, &lookup, seconds(0.2));
        wait_for_frame(&mut preview, &timeline, 36);
    }

    #[test]
    fn playback_carries_on_into_the_next_clip_and_stops_at_the_end() {
        let timeline = two_clip_timeline();
        let mut preview = Preview::new(false, no_notify());

        preview.play(&timeline, &lookup);
        let started = Instant::now();
        let mut shown = Vec::new();
        while preview.is_playing() && started.elapsed() < Duration::from_secs(10) {
            if let Some(frame) = preview.update(&timeline, &lookup) {
                shown.push(number_of(&frame));
            }
            thread::sleep(Duration::from_millis(2));
        }

        assert!(!preview.is_playing(), "playback should have stopped");
        assert_eq!(preview.position(&timeline), timeline.duration());
        assert!(
            shown.iter().any(|number| (30..45).contains(number)),
            "{shown:?}"
        );
        assert!(
            shown.iter().any(|number| (1015..1030).contains(number)),
            "{shown:?}"
        );
        assert!(
            shown
                .iter()
                .all(|number| (30..45).contains(number) || (1015..1030).contains(number)),
            "footage that was cut away appeared: {shown:?}"
        );
    }

    #[test]
    fn playback_can_jump_between_parts_of_one_source() {
        let mut timeline = Timeline::default();
        timeline.push(MediaId(1), range(2.0, 2.3));
        timeline.push(MediaId(1), range(0.5, 0.8));
        let mut preview = Preview::new(false, no_notify());

        preview.play(&timeline, &lookup);
        let started = Instant::now();
        let mut shown = Vec::new();
        while preview.is_playing() && started.elapsed() < Duration::from_secs(10) {
            if let Some(frame) = preview.update(&timeline, &lookup) {
                shown.push(number_of(&frame));
            }
            thread::sleep(Duration::from_millis(2));
        }

        assert!(
            shown.iter().any(|number| (60..69).contains(number)),
            "{shown:?}"
        );
        assert!(
            shown.iter().any(|number| (15..24).contains(number)),
            "{shown:?}"
        );
    }

    #[test]
    fn pausing_holds_the_playhead() {
        let timeline = two_clip_timeline();
        let mut preview = Preview::new(false, no_notify());

        preview.play(&timeline, &lookup);
        let started = Instant::now();
        while preview.position(&timeline) < seconds(0.15)
            && started.elapsed() < Duration::from_secs(5)
        {
            preview.update(&timeline, &lookup);
            thread::sleep(Duration::from_millis(2));
        }
        preview.pause(&timeline);
        let held = preview.position(&timeline);

        thread::sleep(Duration::from_millis(100));
        preview.update(&timeline, &lookup);

        assert_eq!(preview.position(&timeline), held);
        assert!(!preview.is_playing());
    }

    #[test]
    fn playing_again_from_the_end_starts_over() {
        let timeline = two_clip_timeline();
        let mut preview = Preview::new(false, no_notify());
        preview.seek(&timeline, &lookup, timeline.duration());

        preview.play(&timeline, &lookup);
        preview.update(&timeline, &lookup);

        assert!(preview.position(&timeline) < seconds(0.5));
        preview.pause(&timeline);
    }

    #[test]
    fn an_empty_timeline_has_nothing_to_play() {
        let timeline = Timeline::default();
        let mut preview = Preview::new(false, no_notify());

        preview.play(&timeline, &lookup);
        preview.seek(&timeline, &lookup, seconds(1.0));

        assert!(!preview.is_playing());
        assert_eq!(preview.position(&timeline), Ticks(0));
        assert!(preview.update(&timeline, &lookup).is_none());
    }

    #[test]
    fn removing_the_current_clip_leaves_dead_space_under_the_playhead() {
        let mut timeline = two_clip_timeline();
        let mut preview = Preview::new(false, no_notify());
        preview.seek(&timeline, &lookup, seconds(0.1));
        wait_for_frame(&mut preview, &timeline, 33);

        let first = timeline.clips()[0].id;
        timeline.remove(first);
        preview.update(&timeline, &lookup);
        assert!(preview.in_gap(&timeline));

        preview.seek(&timeline, &lookup, seconds(0.6));
        wait_for_frame(&mut preview, &timeline, 1018);
    }

    /// A clip, half a second of dead space, then another clip.
    fn timeline_with_gap() -> Timeline {
        let mut timeline = Timeline::default();
        timeline.push(MediaId(1), range(1.0, 1.3));
        timeline.place(MediaId(2), range(0.5, 0.8), seconds(0.8));
        timeline
    }

    #[test]
    fn coming_back_to_a_video_shows_nothing_left_over_from_before() {
        let timeline = two_clip_timeline();
        let mut preview = Preview::new(false, no_notify());
        preview.seek(&timeline, &lookup, seconds(0.1));
        // The first video's picture arrives while the second is being shown, so nobody takes it.
        preview.seek(&timeline, &lookup, seconds(0.6));
        thread::sleep(Duration::from_millis(300));

        preview.seek(&timeline, &lookup, seconds(0.2));

        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(5) {
            if let Some(frame) = preview.update(&timeline, &lookup) {
                assert_ne!(number_of(&frame), 33, "the picture from the first seek");
                if number_of(&frame) == 36 {
                    return;
                }
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!("the picture never arrived");
    }

    #[test]
    fn seeking_into_dead_space_shows_no_picture_from_either_clip() {
        let timeline = timeline_with_gap();
        let mut preview = Preview::new(false, no_notify());
        preview.seek(&timeline, &lookup, seconds(0.1));
        wait_for_frame(&mut preview, &timeline, 33);

        preview.seek(&timeline, &lookup, seconds(0.5));

        assert!(preview.in_gap(&timeline));
        assert_eq!(preview.position(&timeline), seconds(0.5));
        thread::sleep(Duration::from_millis(50));
        assert!(preview.update(&timeline, &lookup).is_none());
    }

    #[test]
    fn leaving_dead_space_shows_the_clip_again() {
        let timeline = timeline_with_gap();
        let mut preview = Preview::new(false, no_notify());
        preview.seek(&timeline, &lookup, seconds(0.5));

        preview.seek(&timeline, &lookup, seconds(0.9));

        assert!(!preview.in_gap(&timeline));
        wait_for_frame(&mut preview, &timeline, 1018);
    }

    #[test]
    fn playback_runs_through_dead_space_at_normal_speed() {
        let timeline = timeline_with_gap();
        let mut preview = Preview::new(false, no_notify());
        preview.seek(&timeline, &lookup, seconds(0.3));

        preview.play(&timeline, &lookup);
        let started = Instant::now();
        assert!(preview.in_gap(&timeline));
        let mut last = seconds(0.3);
        while preview.in_gap(&timeline) && started.elapsed() < Duration::from_secs(5) {
            preview.update(&timeline, &lookup);
            let now = preview.position(&timeline);
            assert!(now >= last, "the playhead went back");
            last = now;
            thread::sleep(Duration::from_millis(2));
        }

        let took = started.elapsed().as_secs_f64();
        assert!(!preview.in_gap(&timeline));
        assert!((0.45..0.9).contains(&took), "the gap took {took}s");
        wait_for_frame(&mut preview, &timeline, 1015);
    }

    #[test]
    fn playback_goes_from_a_clip_through_dead_space_and_on_to_the_end() {
        let timeline = timeline_with_gap();
        let mut preview = Preview::new(false, no_notify());

        preview.play(&timeline, &lookup);
        let started = Instant::now();
        let mut shown = Vec::new();
        let mut was_in_gap = false;
        while preview.is_playing() && started.elapsed() < Duration::from_secs(10) {
            if let Some(frame) = preview.update(&timeline, &lookup) {
                shown.push(number_of(&frame));
            }
            was_in_gap |= preview.in_gap(&timeline);
            thread::sleep(Duration::from_millis(2));
        }

        assert!(was_in_gap);
        assert!(!preview.is_playing());
        assert_eq!(preview.position(&timeline), timeline.duration());
        assert!(shown.iter().any(|n| (30..39).contains(n)), "{shown:?}");
        assert!(shown.iter().any(|n| (1015..1024).contains(n)), "{shown:?}");
    }

    #[test]
    fn dead_space_can_come_first() {
        let mut timeline = Timeline::default();
        timeline.place(MediaId(1), range(1.0, 1.3), seconds(0.2));
        let mut preview = Preview::new(false, no_notify());

        preview.play(&timeline, &lookup);
        assert!(preview.in_gap(&timeline));

        wait_for_frame(&mut preview, &timeline, 30);
        assert!(!preview.in_gap(&timeline));
    }

    #[test]
    fn pausing_in_dead_space_holds_the_playhead() {
        let timeline = timeline_with_gap();
        let mut preview = Preview::new(false, no_notify());
        preview.seek(&timeline, &lookup, seconds(0.35));
        preview.play(&timeline, &lookup);
        preview.update(&timeline, &lookup);
        thread::sleep(Duration::from_millis(50));

        preview.pause(&timeline);
        let held = preview.position(&timeline);
        thread::sleep(Duration::from_millis(50));
        preview.update(&timeline, &lookup);

        assert_eq!(preview.position(&timeline), held);
        assert!(held > seconds(0.35));
        assert!(preview.in_gap(&timeline));
    }

    #[test]
    fn a_project_of_nothing_but_sound_plays_on_the_clock_and_stops_at_its_end() {
        let timeline = Timeline::default();
        let mut preview = Preview::new(false, no_notify());
        preview.set_sound(Vec::new(), seconds(0.2));

        preview.play(&timeline, &lookup);
        assert!(preview.is_playing());

        let started = Instant::now();
        while preview.is_playing() {
            assert!(started.elapsed() < Duration::from_secs(5), "it never ended");
            assert!(preview.update(&timeline, &lookup).is_none());
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(preview.position(&timeline), seconds(0.2));
    }

    #[test]
    fn sound_that_outlasts_the_pictures_keeps_the_playhead_going_after_them() {
        let mut timeline = Timeline::default();
        timeline.push(MediaId(1), range(1.0, 1.2));
        let mut preview = Preview::new(false, no_notify());
        preview.set_sound(Vec::new(), seconds(0.5));

        preview.play(&timeline, &lookup);
        let started = Instant::now();
        let mut furthest = Ticks(0);
        while preview.is_playing() {
            assert!(started.elapsed() < Duration::from_secs(5), "it never ended");
            preview.update(&timeline, &lookup);
            furthest = furthest.max(preview.position(&timeline));
            thread::sleep(Duration::from_millis(2));
        }

        assert!(furthest > seconds(0.2), "it stopped at the last picture");
        assert_eq!(preview.position(&timeline), seconds(0.5));
    }

    #[test]
    fn the_playhead_never_jumps_back_when_one_clip_gives_way_to_the_next() {
        let timeline = two_clip_timeline();
        let mut preview = Preview::new(false, no_notify());

        preview.play(&timeline, &lookup);
        let started = Instant::now();
        let mut last = Ticks(0);
        while preview.is_playing() {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "it never ended"
            );
            preview.update(&timeline, &lookup);
            let now = preview.position(&timeline);
            assert!(now >= last, "it went from {last:?} back to {now:?}");
            last = now;
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn the_clock_waits_for_the_first_picture_before_it_moves() {
        let timeline = two_clip_timeline();
        let mut preview = Preview::new(false, no_notify());

        preview.play(&timeline, &lookup);

        assert_eq!(preview.position(&timeline), Ticks(0));
        thread::sleep(Duration::from_millis(20));
        assert_eq!(preview.position(&timeline), Ticks(0), "nothing has run yet");
    }

    #[test]
    fn saying_what_can_be_heard_with_nothing_connected_is_harmless() {
        let mut preview = Preview::new(true, no_notify());
        let span = MixSpan {
            path: PathBuf::from("a.wav"),
            start: Ticks(0),
            source: range(0.0, 1.0),
            sound: Default::default(),
            bus: bareclip_audio::Bus::All(0),
        };

        preview.set_sound(vec![span], seconds(1.0));

        assert!(preview.sound_works(), "nothing has tried to play yet");
    }

    #[test]
    fn only_a_few_videos_stay_open() {
        let mut timeline = Timeline::default();
        for media in 1..=6 {
            timeline.push(MediaId(media), range(0.0, 0.2));
        }
        let mut preview = Preview::new(false, no_notify());

        for step in 0..6 {
            preview.seek(&timeline, &lookup, seconds(f64::from(step) * 0.2 + 0.05));
        }

        assert!(preview.slots.len() <= MAX_OPEN_PLAYERS);
    }
}
