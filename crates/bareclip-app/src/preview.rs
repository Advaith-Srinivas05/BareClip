//! Plays the timeline by following its clips, using one player per source video.

use std::time::Instant;

use bareclip_core::{
    time::Ticks,
    timeline::{ClipId, MediaId, Timeline},
};
use bareclip_media::{MediaInfo, PreviewFrame};

use crate::player::{Notify, Player};

/// Open players hold decoders and a sound connection, so only a few stay open.
const MAX_OPEN_PLAYERS: usize = 4;

struct Slot {
    media: MediaId,
    player: Player,
}

pub struct Preview {
    /// Most recently used last.
    slots: Vec<Slot>,
    current: Option<(ClipId, MediaId)>,
    playing: bool,
    position: Ticks,
    /// When playback entered the dead space it is now passing through.
    gap_clock: Option<Instant>,
    use_sound: bool,
    notify: Notify,
    error: Option<String>,
}

impl Preview {
    pub fn new(use_sound: bool, notify: Notify) -> Self {
        Self {
            slots: Vec::new(),
            current: None,
            playing: false,
            position: Ticks(0),
            gap_clock: None,
            use_sound,
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
            .or_else(|| self.error.take())
    }

    /// Whether every open source video that has audio can actually be heard.
    pub fn sound_works(&self) -> bool {
        self.slots
            .iter()
            .all(|slot| slot.player.has_sound() || !slot.player.expects_sound())
    }

    /// Whether the playhead is in dead space, where the picture is black.
    pub fn in_gap(&self, timeline: &Timeline) -> bool {
        self.current.is_none() && !timeline.is_empty()
    }

    /// Where the playhead is on the timeline.
    pub fn position(&self, timeline: &Timeline) -> Ticks {
        if !self.playing {
            return self.position;
        }
        if let Some(entered) = self.gap_clock {
            let passed = Ticks::from_seconds(entered.elapsed().as_secs_f64());
            return Ticks(self.position.0 + passed.0).min(timeline.duration());
        }
        let Some((clip_id, media)) = self.current else {
            return self.position;
        };
        let Some(index) = timeline.index_of(clip_id) else {
            return self.position;
        };
        let Some(slot) = self.slots.iter().find(|slot| slot.media == media) else {
            return self.position;
        };
        let clip = timeline.clips()[index];
        let in_clip = (slot.player.position().0 - clip.source.start.0).clamp(0, clip.duration().0);
        Ticks(clip.start.0 + in_clip)
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
            match Player::open(&info, self.use_sound, self.notify.clone()) {
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

    /// Moves the playhead to a moment on the timeline.
    pub fn seek(
        &mut self,
        timeline: &Timeline,
        lookup: &dyn Fn(MediaId) -> Option<MediaInfo>,
        time: Ticks,
    ) {
        self.position = time.clamp(Ticks(0), timeline.duration());
        self.gap_clock = None;
        let Some(location) = timeline.locate(self.position) else {
            self.pause_all();
            self.current = None;
            if timeline.is_empty() {
                self.playing = false;
            } else if self.playing {
                self.gap_clock = Some(Instant::now());
            }
            return;
        };

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
        if timeline.is_empty() || self.playing {
            return;
        }
        let start = if self.position >= timeline.duration() {
            Ticks(0)
        } else {
            self.position
        };
        self.playing = true;
        self.seek(timeline, lookup, start);
    }

    pub fn pause(&mut self, timeline: &Timeline) {
        if !self.playing {
            return;
        }
        self.position = self.position(timeline);
        self.gap_clock = None;
        self.playing = false;
        self.pause_all();
    }

    pub fn toggle(&mut self, timeline: &Timeline, lookup: &dyn Fn(MediaId) -> Option<MediaInfo>) {
        if self.playing {
            self.pause(timeline);
        } else {
            self.play(timeline, lookup);
        }
    }

    /// Moves between clips as they finish, and returns the picture to show now.
    pub fn update(
        &mut self,
        timeline: &Timeline,
        lookup: &dyn Fn(MediaId) -> Option<MediaInfo>,
    ) -> Option<PreviewFrame> {
        let Some((clip_id, media)) = self.current else {
            if self.playing {
                self.leave_gap_when_due(timeline, lookup);
            }
            return None;
        };
        let Some(index) = timeline.index_of(clip_id) else {
            // The clip was removed, so the playhead finds whatever is there now.
            let position = self.position;
            self.seek(timeline, lookup, position);
            return None;
        };
        let clip = timeline.clips()[index];
        let slot = self.slots.iter_mut().find(|slot| slot.media == media)?;
        let frame = slot.player.update();
        let source_position = slot.player.position();
        let player_stopped = !slot.player.is_playing();

        if self.playing {
            self.position = self.position(timeline);
            if source_position >= clip.source.end || player_stopped {
                match timeline.clips().get(index + 1) {
                    Some(_) => self.seek(timeline, lookup, clip.end()),
                    None => {
                        self.position = timeline.duration();
                        self.playing = false;
                        self.pause_all();
                    }
                }
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
            self.seek(timeline, lookup, next);
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
