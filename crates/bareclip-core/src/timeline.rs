//! The project's timeline: clips of source videos, each at its own place in time.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::{
    placement::Placement,
    sound::ClipSound,
    speed::Speed,
    time::{TICKS_PER_SECOND, Ticks},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeRange {
    pub start: Ticks,
    pub end: Ticks,
}

impl TimeRange {
    pub fn duration(self) -> Ticks {
        Ticks(self.end.0 - self.start.0)
    }

    /// Whether this is a stretch of an image rather than of real footage.
    pub fn is_still(self) -> bool {
        self.start.0 >= STILL_FLOOR
    }

    /// The stretch a new clip of an image starts on.
    pub fn new_still() -> Self {
        Self {
            start: Ticks(STILL_START),
            end: Ticks(STILL_START + STILL_CLIP_SECONDS * TICKS_PER_SECOND),
        }
    }

    /// The stretch a new title starts on.
    pub fn new_title() -> Self {
        Self {
            start: Ticks(STILL_START),
            end: Ticks(STILL_START + TITLE_CLIP_SECONDS * TICKS_PER_SECOND),
        }
    }
}

const SECONDS_PER_HOUR: i64 = 3600;
/// An image has no footage, so it counts as very long footage of one picture. Every rule for
/// trimming, splitting and moving clips then applies to it as it stands. Its clips start this far
/// in, so that they can stretch either way.
const STILL_START: i64 = 1000 * SECONDS_PER_HOUR * TICKS_PER_SECOND;
/// Nothing this far into a file is real footage, even with a clip stretched a long way left.
const STILL_FLOOR: i64 = 100 * SECONDS_PER_HOUR * TICKS_PER_SECOND;
/// How long a new clip of an image lasts.
const STILL_CLIP_SECONDS: i64 = 4;
/// How long a new title lasts.
const TITLE_CLIP_SECONDS: i64 = 5;
/// How long an image counts as being.
pub const STILL_LENGTH: Ticks = Ticks(2 * STILL_START);

/// A video added to the project.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MediaId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClipId(pub u64);

/// Ids stay unique across undo and redo because they never come from a snapshot.
pub(crate) fn next_clip_id() -> ClipId {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    ClipId(NEXT.fetch_add(1, Ordering::Relaxed))
}

/// A stretch of a source video placed on the timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clip {
    pub id: ClipId,
    pub media: MediaId,
    /// When this clip starts on the timeline.
    pub start: Ticks,
    pub source: TimeRange,
    /// How its picture is laid out. It belongs to the whole clip.
    pub placement: Placement,
    /// How it sounds. This belongs to the whole clip too.
    pub sound: ClipSound,
    /// How fast it plays, and so how long it lasts. This too.
    pub speed: Speed,
}

impl Clip {
    /// How long this clip lasts on the timeline.
    pub fn duration(self) -> Ticks {
        Ticks(self.speed.on_timeline(self.source.duration()).0.max(1))
    }

    /// The moment in the footage that plays at a moment on the timeline.
    pub fn source_at(self, time: Ticks) -> Ticks {
        if time >= self.end() {
            return self.source.end;
        }
        Ticks(self.source.start.0 + self.speed.in_footage(Ticks(time.0 - self.start.0)).0)
    }

    /// The moment on the timeline that a moment in the footage plays at.
    pub fn time_of(self, source_time: Ticks) -> Ticks {
        Ticks(
            self.start.0
                + self
                    .speed
                    .on_timeline(Ticks(source_time.0 - self.source.start.0))
                    .0,
        )
    }

    /// Whether this clip shows an image.
    pub fn is_still(self) -> bool {
        self.source.is_still()
    }

    /// Whether this clip shows a title.
    pub fn is_title(self) -> bool {
        self.media.is_title()
    }

    /// When this clip ends on the timeline.
    pub fn end(self) -> Ticks {
        Ticks(self.start.0 + self.duration().0)
    }
}

/// Which end of a clip is being moved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipEdge {
    Start,
    End,
}

/// Where a moment on the timeline falls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Location {
    pub index: usize,
    pub clip: Clip,
    /// The matching moment in the source video.
    pub source_time: Ticks,
}

/// Clips in playing order. They never overlap, and nothing moves a clip unless asked
/// to, so dead space can sit between them until it is removed by hand.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Timeline {
    clips: Vec<Clip>,
}

impl Timeline {
    pub fn clips(&self) -> &[Clip] {
        &self.clips
    }

    pub fn is_empty(&self) -> bool {
        self.clips.is_empty()
    }

    /// Where the last clip ends. Dead space before it counts, and nothing after it does.
    pub fn duration(&self) -> Ticks {
        self.clips.last().map_or(Ticks(0), |clip| clip.end())
    }

    pub fn index_of(&self, id: ClipId) -> Option<usize> {
        self.clips.iter().position(|clip| clip.id == id)
    }

    pub fn clip(&self, id: ClipId) -> Option<Clip> {
        self.clips.iter().find(|clip| clip.id == id).copied()
    }

    /// Finds the clip at a moment, or `None` in dead space. A moment at or past the end
    /// falls at the end of the last clip, so the playhead has a last frame to show.
    pub fn locate(&self, time: Ticks) -> Option<Location> {
        let last = self.clips.len().checked_sub(1)?;
        let index = if time >= self.clips[last].end() {
            last
        } else {
            self.clips
                .iter()
                .position(|clip| time >= clip.start && time < clip.end())?
        };
        let clip = self.clips[index];
        Some(Location {
            index,
            clip,
            source_time: clip.source_at(time),
        })
    }

    /// The dead space on the timeline, in order.
    pub fn gaps(&self) -> Vec<TimeRange> {
        let mut gaps = Vec::new();
        let mut previous_end = Ticks(0);
        for clip in &self.clips {
            if clip.start > previous_end {
                gaps.push(TimeRange {
                    start: previous_end,
                    end: clip.start,
                });
            }
            previous_end = clip.end();
        }
        gaps
    }

    /// The dead space around a moment, if the moment falls in some.
    pub fn gap_at(&self, time: Ticks) -> Option<TimeRange> {
        self.gaps()
            .into_iter()
            .find(|gap| time >= gap.start && time < gap.end)
    }

    /// Puts saved clips back, each with a new id. Going through the usual rules means a
    /// file edited by hand can't produce an arrangement the editor couldn't.
    pub(crate) fn restore(
        saved: impl IntoIterator<Item = (MediaId, Ticks, TimeRange, Placement, ClipSound, Speed)>,
    ) -> Self {
        let mut clips: Vec<(MediaId, Ticks, TimeRange, Placement, ClipSound, Speed)> = saved
            .into_iter()
            .filter(|(_, _, source, ..)| source.start < source.end)
            .collect();
        clips.sort_by_key(|(_, start, ..)| *start);
        let mut timeline = Self::default();
        for (media, start, source, placement, sound, speed) in clips {
            let id = next_clip_id();
            let wanted = start.max(Ticks(0));
            // Past whatever is already there, since these arrive in order.
            let start = wanted.max(timeline.duration());
            timeline.clips.push(Clip {
                id,
                media,
                start,
                source,
                placement: placement.clamped(),
                sound: sound.clamped(),
                speed: speed.clamped(),
            });
        }
        timeline
    }

    /// Clips already in order and not overlapping, such as what a stack of tracks plays.
    pub(crate) fn from_ordered(clips: Vec<Clip>) -> Self {
        Self { clips }
    }

    /// Moves every clip starting at or after a moment back by a stretch of time.
    pub(crate) fn shift_from(&mut self, time: Ticks, by: Ticks) {
        for clip in &mut self.clips {
            if clip.start >= time {
                clip.start = Ticks(clip.start.0 - by.0);
            }
        }
    }

    /// Puts an existing clip as close as it can get to a moment.
    pub(crate) fn land(&mut self, mut clip: Clip, wanted: Ticks) {
        clip.start = self.landing(None, wanted, clip.duration());
        self.settle(clip);
    }

    /// Closes the dead space around a moment by moving every later clip back.
    pub fn remove_gap(&mut self, time: Ticks) -> bool {
        let Some(gap) = self.gap_at(time) else {
            return false;
        };
        for clip in &mut self.clips {
            if clip.start >= gap.end {
                clip.start = Ticks(clip.start.0 - gap.duration().0);
            }
        }
        true
    }

    /// Closes all the dead space, so the clips play straight after one another.
    pub fn remove_all_gaps(&mut self) -> bool {
        let mut free = Ticks(0);
        let mut changed = false;
        for clip in &mut self.clips {
            if clip.start != free {
                clip.start = free;
                changed = true;
            }
            free = clip.end();
        }
        changed
    }

    /// Where something of a given length ends up if dropped at a moment. If its front
    /// lands on a clip it goes to whichever side of that clip it mostly covers.
    /// Otherwise it stays where it was dropped, and later clips make room.
    pub fn landing(&self, moving: Option<ClipId>, wanted: Ticks, length: Ticks) -> Ticks {
        let wanted = wanted.0.max(0);
        let end = wanted + length.0;
        let under = self
            .clips
            .iter()
            .filter(|clip| Some(clip.id) != moving)
            .find(|clip| clip.end().0 > wanted && clip.start.0 < end);
        match under {
            Some(clip) if clip.start.0 <= wanted => {
                let middle = wanted + length.0 / 2;
                if middle < clip.start.0 + clip.duration().0 / 2 {
                    clip.start
                } else {
                    clip.end()
                }
            }
            _ => Ticks(wanted),
        }
    }

    /// Puts a clip in its place in the order. Later clips move along only as far as
    /// they need to, so dead space absorbs the push where it can.
    fn settle(&mut self, clip: Clip) {
        let index = self
            .clips
            .iter()
            .position(|other| other.start >= clip.start)
            .unwrap_or(self.clips.len());
        self.clips.insert(index, clip);
        let mut free = clip.end();
        for later in &mut self.clips[index + 1..] {
            if later.start >= free {
                break;
            }
            later.start = free;
            free = later.end();
        }
    }

    /// Adds a clip to the end of the timeline.
    pub fn push(&mut self, media: MediaId, source: TimeRange) -> ClipId {
        let id = next_clip_id();
        self.clips.push(Clip {
            id,
            media,
            start: self.duration(),
            source,
            placement: Placement::default(),
            sound: ClipSound::default(),
            speed: Speed::default(),
        });
        id
    }

    /// Adds a clip as close as it can get to a moment.
    pub fn place(&mut self, media: MediaId, source: TimeRange, wanted: Ticks) -> ClipId {
        let id = next_clip_id();
        let start = self.landing(None, wanted, source.duration());
        self.settle(Clip {
            id,
            media,
            start,
            source,
            placement: Placement::default(),
            sound: ClipSound::default(),
            speed: Speed::default(),
        });
        id
    }

    /// Adds a copy of a clip as close as it can get to a moment, as a new clip showing `media`.
    pub(crate) fn place_copy(&mut self, clip: Clip, media: MediaId, wanted: Ticks) -> ClipId {
        let id = next_clip_id();
        let start = self.landing(None, wanted, clip.duration());
        self.settle(Clip {
            id,
            media,
            start,
            ..clip
        });
        id
    }

    /// Makes a clip show something else, leaving everything else about it alone.
    pub(crate) fn set_media(&mut self, id: ClipId, media: MediaId) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };
        self.clips[index].media = media;
        true
    }

    /// Makes a clip last this long, as `set_speed` does: it keeps its start, and later clips move
    /// along only as far as they must. It can't run past the end of its footage. Returns whether
    /// that changed anything.
    pub(crate) fn set_duration(
        &mut self,
        id: ClipId,
        duration: Ticks,
        source_length: Ticks,
    ) -> bool {
        let Some(clip) = self.clip(id) else {
            return false;
        };
        let wanted_end = clip.source.start.0 + clip.speed.in_footage(duration).0;
        let end = wanted_end.min(source_length.0).max(clip.source.start.0 + 1);
        if end == clip.source.end.0 {
            return false;
        }
        self.remove(id);
        self.settle(Clip {
            source: TimeRange {
                start: clip.source.start,
                end: Ticks(end),
            },
            ..clip
        });
        true
    }

    /// Puts a copy of a clip straight after it, moving later clips along if they need room.
    pub fn duplicate(&mut self, id: ClipId) -> Option<ClipId> {
        let clip = self.clip(id)?;
        let copy = next_clip_id();
        self.settle(Clip {
            id: copy,
            start: clip.end(),
            ..clip
        });
        Some(copy)
    }

    /// Moves a clip as close as it can get to a moment.
    pub fn move_clip(&mut self, id: ClipId, wanted: Ticks) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        let mut clip = self.clips[index];
        clip.start = self.landing(Some(id), wanted, clip.duration());
        self.clips.remove(index);
        self.settle(clip);
    }

    /// Splits a clip in two at a moment in its source video. The first part keeps the
    /// clip's id. Returns the new second part, or `None` if that moment is not inside.
    pub fn split(&mut self, id: ClipId, source_time: Ticks) -> Option<ClipId> {
        let index = self.index_of(id)?;
        let clip = self.clips[index];
        if source_time <= clip.source.start || source_time >= clip.source.end {
            return None;
        }
        self.clips[index].source.end = source_time;
        // The fades stay at the ends they were made for.
        self.clips[index].sound.fade_out = 0;
        let second = next_clip_id();
        let start = self.clips[index].end();
        self.clips.insert(
            index + 1,
            Clip {
                id: second,
                media: clip.media,
                start,
                source: TimeRange {
                    start: source_time,
                    end: clip.source.end,
                },
                placement: clip.placement,
                sound: ClipSound {
                    fade_in: 0,
                    ..clip.sound
                },
                speed: clip.speed,
            },
        );
        Some(second)
    }

    /// Lays a clip's picture out differently. Returns whether that changed anything.
    pub(crate) fn set_placement(&mut self, id: ClipId, placement: Placement) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };
        let placement = placement.clamped();
        let changed = self.clips[index].placement != placement;
        self.clips[index].placement = placement;
        changed
    }

    /// Changes how fast a clip plays. It keeps its start, so playing faster leaves dead space
    /// after it, and playing slower moves later clips along only as far as they must.
    pub(crate) fn set_speed(&mut self, id: ClipId, speed: Speed) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };
        let speed = speed.clamped();
        let changed = self.clips[index].speed != speed;
        self.clips[index].speed = speed;
        let mut free = self.clips[index].end();
        for later in &mut self.clips[index + 1..] {
            if later.start >= free {
                break;
            }
            later.start = free;
            free = later.end();
        }
        changed
    }

    /// Changes how a clip sounds. Returns whether that changed anything.
    pub(crate) fn set_sound(&mut self, id: ClipId, sound: ClipSound) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };
        let sound = sound.clamped();
        let changed = self.clips[index].sound != sound;
        self.clips[index].sound = sound;
        changed
    }

    /// Removes a clip and leaves dead space where it was.
    pub fn remove(&mut self, id: ClipId) -> bool {
        let before = self.clips.len();
        self.clips.retain(|clip| clip.id != id);
        self.clips.len() != before
    }

    /// Removes every clip of a source video and returns how many there were.
    pub fn remove_media(&mut self, media: MediaId) -> usize {
        let before = self.clips.len();
        self.clips.retain(|clip| clip.media != media);
        before - self.clips.len()
    }

    pub fn clips_using(&self, media: MediaId) -> usize {
        self.clips.iter().filter(|clip| clip.media == media).count()
    }

    /// Moves one end of a clip to a moment in its source video, leaving the footage it
    /// keeps where it is on the timeline. It stops at the bounds of the source and at
    /// its neighbours, and always leaves at least `min_duration` of clip.
    pub fn trim(
        &mut self,
        id: ClipId,
        edge: ClipEdge,
        source_time: Ticks,
        source_duration: Ticks,
        min_duration: Ticks,
    ) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        let room_before = match index.checked_sub(1) {
            Some(previous) => self.clips[index].start.0 - self.clips[previous].end().0,
            None => self.clips[index].start.0,
        };
        let room_after = self
            .clips
            .get(index + 1)
            .map(|next| next.start.0 - self.clips[index].end().0);
        let clip = &mut self.clips[index];
        let speed = clip.speed;
        let shortest = speed.in_footage(min_duration).0;
        match edge {
            ClipEdge::Start => {
                let end = clip.end();
                let reach = speed.in_footage(Ticks(room_before)).0;
                let earliest = (clip.source.start.0 - reach).max(0);
                let latest = (clip.source.end.0 - shortest).max(earliest);
                clip.source.start = Ticks(source_time.0.clamp(earliest, latest));
                // The other end stays exactly where it is, whatever rounding does to the length.
                clip.start = Ticks((end.0 - clip.duration().0).max(0));
            }
            ClipEdge::End => {
                let earliest = clip.source.start.0 + shortest;
                let limit = match room_after {
                    Some(room) => source_duration
                        .0
                        .min(clip.source.end.0 + speed.in_footage(Ticks(room)).0),
                    None => source_duration.0,
                };
                let latest = limit.max(earliest);
                let allowed = room_after.map(|room| clip.end().0 + room);
                clip.source.end = Ticks(source_time.0.clamp(earliest, latest));
                // Rounding mustn't let it touch the next clip.
                if let Some(allowed) = allowed {
                    while clip.end().0 > allowed && clip.source.end.0 > earliest {
                        clip.source.end.0 -= 1;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIDEO: MediaId = MediaId(1);
    const OTHER: MediaId = MediaId(2);

    fn range(start: i64, end: i64) -> TimeRange {
        TimeRange {
            start: Ticks(start),
            end: Ticks(end),
        }
    }

    /// Three touching clips of 10, 20 and 30 ticks.
    fn three_clips() -> (Timeline, [ClipId; 3]) {
        let mut timeline = Timeline::default();
        let first = timeline.push(VIDEO, range(0, 10));
        let second = timeline.push(OTHER, range(5, 25));
        let third = timeline.push(VIDEO, range(40, 70));
        (timeline, [first, second, third])
    }

    fn order(timeline: &Timeline) -> Vec<ClipId> {
        timeline.clips().iter().map(|clip| clip.id).collect()
    }

    fn starts(timeline: &Timeline) -> Vec<i64> {
        timeline.clips().iter().map(|clip| clip.start.0).collect()
    }

    fn assert_no_overlaps(timeline: &Timeline) {
        for pair in timeline.clips().windows(2) {
            assert!(pair[0].end() <= pair[1].start, "{pair:?}");
        }
    }

    fn at(factor: f64) -> Speed {
        Speed::default().with_factor(factor)
    }

    #[test]
    fn a_faster_clip_is_shorter_and_finds_its_footage_from_anywhere() {
        let mut timeline = Timeline::default();
        let id = timeline.place(VIDEO, range(1000, 3000), Ticks(100));
        timeline.set_speed(id, at(2.0));

        let clip = timeline.clip(id).expect("it is there");
        assert_eq!(clip.duration(), Ticks(1000));
        assert_eq!(clip.end(), Ticks(1100));
        assert_eq!(clip.source_at(Ticks(600)), Ticks(2000));
        assert_eq!(clip.time_of(Ticks(2000)), Ticks(600));
        assert_eq!(
            timeline.locate(Ticks(600)).map(|at| at.source_time),
            Some(Ticks(2000))
        );
    }

    #[test]
    fn the_very_end_of_a_clip_is_the_end_of_its_footage_whatever_rounding_says() {
        let mut timeline = Timeline::default();
        let id = timeline.place(VIDEO, range(0, 1000), Ticks(0));
        timeline.set_speed(id, at(3.0));

        let clip = timeline.clip(id).expect("it is there");
        assert_eq!(clip.source_at(clip.end()), Ticks(1000));
    }

    #[test]
    fn speeding_up_leaves_dead_space_and_moves_nothing() {
        let (mut timeline, [first, second, third]) = three_clips();
        let starts_before = starts(&timeline);

        timeline.set_speed(first, at(2.0));

        assert_eq!(starts(&timeline), starts_before);
        assert_eq!(timeline.clip(first).map(Clip::end), Some(Ticks(5)));
        assert_eq!(timeline.gaps().len(), 1);
        assert_eq!(order(&timeline), [first, second, third]);
    }

    #[test]
    fn slowing_down_moves_later_clips_only_as_far_as_they_must() {
        let mut timeline = Timeline::default();
        let first = timeline.place(VIDEO, range(0, 100), Ticks(0));
        let near = timeline.place(OTHER, range(0, 10), Ticks(110));
        let far = timeline.place(OTHER, range(0, 10), Ticks(500));

        timeline.set_speed(first, at(0.5));

        assert_eq!(timeline.clip(first).map(Clip::end), Some(Ticks(200)));
        assert_eq!(timeline.clip(near).map(|clip| clip.start), Some(Ticks(200)));
        assert_eq!(timeline.clip(far).map(|clip| clip.start), Some(Ticks(500)));
        assert_no_overlaps(&timeline);
    }

    #[test]
    fn splitting_a_faster_clip_keeps_both_halves_touching_and_at_the_same_speed() {
        let mut timeline = Timeline::default();
        let id = timeline.place(VIDEO, range(0, 1000), Ticks(100));
        timeline.set_speed(id, at(2.0));

        let second = timeline.split(id, Ticks(400)).expect("it splits");

        let (a, b) = (
            timeline.clip(id).expect("it is there"),
            timeline.clip(second).expect("it is there"),
        );
        assert_eq!(a.end(), Ticks(300));
        assert_eq!(b.start, a.end());
        assert_eq!(b.end(), Ticks(600));
        assert_eq!(b.speed, a.speed);
        assert_eq!(b.source_at(b.start), Ticks(400));
    }

    #[test]
    fn splitting_at_an_awkward_speed_never_leaves_a_gap_or_an_overlap() {
        for factor in [0.1, 0.3, 0.7, 1.3, 3.0, 7.0, 16.0] {
            let mut timeline = Timeline::default();
            let id = timeline.place(VIDEO, range(0, 1_000_003), Ticks(7));
            timeline.set_speed(id, at(factor));
            let second = timeline.split(id, Ticks(333_331)).expect("it splits");
            let third = timeline.split(second, Ticks(777_773)).expect("it splits");

            let clips = timeline.clips();
            assert_eq!(clips[0].end(), clips[1].start, "{factor}");
            assert_eq!(clips[1].end(), clips[2].start, "{factor}");
            assert_eq!(
                timeline.clip(third).map(|clip| clip.source.end),
                Some(Ticks(1_000_003))
            );
            assert_no_overlaps(&timeline);
        }
    }

    #[test]
    fn trimming_a_faster_clips_start_keeps_its_end_and_the_footage_in_place() {
        let mut timeline = Timeline::default();
        let id = timeline.place(VIDEO, range(1000, 3000), Ticks(100));
        timeline.set_speed(id, at(2.0));

        timeline.trim(id, ClipEdge::Start, Ticks(1400), Ticks(5000), Ticks(1));

        let clip = timeline.clip(id).expect("it is there");
        assert_eq!(clip.end(), Ticks(1100));
        assert_eq!(clip.start, Ticks(300));
        assert_eq!(
            clip.source_at(Ticks(600)),
            Ticks(2000),
            "footage stays where it was"
        );
    }

    #[test]
    fn a_faster_clip_can_only_stretch_into_the_room_it_has_on_the_timeline() {
        let mut timeline = Timeline::default();
        let id = timeline.place(VIDEO, range(1000, 2000), Ticks(0));
        timeline.set_speed(id, at(2.0));
        let next = timeline.place(OTHER, range(0, 100), Ticks(600));
        assert_eq!(timeline.clip(id).map(Clip::end), Some(Ticks(500)));

        timeline.trim(id, ClipEdge::End, Ticks(4000), Ticks(5000), Ticks(1));

        assert_eq!(timeline.clip(id).map(Clip::end), Some(Ticks(600)));
        assert_eq!(
            timeline.clip(id).map(|clip| clip.source.end),
            Some(Ticks(2200))
        );
        assert_eq!(timeline.clip(next).map(|clip| clip.start), Some(Ticks(600)));
    }

    #[test]
    fn the_shortest_a_trim_can_leave_is_measured_on_the_timeline() {
        let mut timeline = Timeline::default();
        let id = timeline.place(VIDEO, range(0, 1000), Ticks(0));
        timeline.set_speed(id, at(4.0));

        timeline.trim(id, ClipEdge::End, Ticks(0), Ticks(5000), Ticks(100));

        assert_eq!(timeline.clip(id).map(Clip::duration), Some(Ticks(100)));
    }

    #[test]
    fn a_duplicate_keeps_its_speed() {
        let mut timeline = Timeline::default();
        let id = timeline.place(VIDEO, range(0, 1000), Ticks(0));
        timeline.set_speed(id, at(2.0));

        let copy = timeline.duplicate(id).expect("it copies");

        assert_eq!(timeline.clip(copy).map(|clip| clip.start), Some(Ticks(500)));
        assert_eq!(timeline.clip(copy).map(Clip::duration), Some(Ticks(500)));
    }

    #[test]
    fn a_new_image_clip_lasts_four_seconds_and_counts_as_an_image() {
        let source = TimeRange::new_still();

        assert_eq!(source.duration(), Ticks::from_seconds(4.0));
        assert!(source.is_still());
        assert!(!range(0, 100).is_still());
    }

    #[test]
    fn an_image_clip_stretches_either_way_and_is_still_an_image() {
        let mut timeline = Timeline::default();
        let source = TimeRange::new_still();
        let id = timeline.place(VIDEO, source, Ticks::from_seconds(100.0));

        timeline.trim(
            id,
            ClipEdge::Start,
            Ticks(source.start.0 - Ticks::from_seconds(60.0).0),
            STILL_LENGTH,
            Ticks(1),
        );
        timeline.trim(
            id,
            ClipEdge::End,
            Ticks(source.end.0 + Ticks::from_seconds(3600.0).0),
            STILL_LENGTH,
            Ticks(1),
        );

        let clip = timeline.clip(id).expect("it is still there");
        assert_eq!(clip.start, Ticks::from_seconds(40.0));
        assert_eq!(clip.duration(), Ticks::from_seconds(3664.0));
        assert!(clip.is_still());
    }

    #[test]
    fn removing_all_gaps_closes_every_one_and_keeps_the_order() {
        let (mut timeline, ids) = three_clips();
        timeline.move_clip(ids[2], Ticks(100));
        timeline.move_clip(ids[1], Ticks(50));
        timeline.move_clip(ids[0], Ticks(5));
        assert_eq!(timeline.gaps().len(), 3);

        assert!(timeline.remove_all_gaps());

        assert!(timeline.gaps().is_empty());
        assert_eq!(order(&timeline), ids);
        assert_eq!(starts(&timeline), [0, 10, 30]);
    }

    #[test]
    fn removing_all_gaps_with_none_changes_nothing() {
        let (mut timeline, _) = three_clips();

        assert!(!timeline.remove_all_gaps());
        assert_eq!(starts(&timeline), [0, 10, 30]);
    }

    #[test]
    fn a_duplicate_lands_straight_after_its_clip_with_the_same_footage() {
        let (mut timeline, ids) = three_clips();

        let copy = timeline.duplicate(ids[0]).expect("the clip exists");

        assert_eq!(order(&timeline), [ids[0], copy, ids[1], ids[2]]);
        assert_eq!(starts(&timeline), [0, 10, 20, 40]);
        let (original, duplicate) = (
            timeline.clip(ids[0]).expect("the clip stays"),
            timeline.clip(copy).expect("the copy is there"),
        );
        assert_eq!(duplicate.media, original.media);
        assert_eq!(duplicate.source, original.source);
        assert_no_overlaps(&timeline);
    }

    #[test]
    fn a_duplicate_uses_up_dead_space_before_moving_anything() {
        let (mut timeline, ids) = three_clips();
        timeline.move_clip(ids[1], Ticks(50));
        let before = timeline.clip(ids[1]).expect("the clip exists").start;

        timeline.duplicate(ids[0]);

        assert_eq!(
            timeline.clip(ids[1]).expect("the clip exists").start,
            before
        );
        assert_no_overlaps(&timeline);
    }

    #[test]
    fn a_clip_that_is_gone_cannot_be_duplicated() {
        let (mut timeline, ids) = three_clips();
        timeline.remove(ids[0]);

        assert_eq!(timeline.duplicate(ids[0]), None);
    }

    #[test]
    fn an_empty_timeline_has_nothing_to_find() {
        let timeline = Timeline::default();

        assert!(timeline.is_empty());
        assert_eq!(timeline.duration(), Ticks(0));
        assert_eq!(timeline.locate(Ticks(0)), None);
        assert!(timeline.gaps().is_empty());
    }

    #[test]
    fn duration_runs_to_the_end_of_the_last_clip() {
        assert_eq!(three_clips().0.duration(), Ticks(60));
    }

    #[test]
    fn a_moment_maps_to_its_clip_and_source_time() {
        let (timeline, ids) = three_clips();

        let location = timeline.locate(Ticks(15)).expect("a clip is there");

        assert_eq!(location.index, 1);
        assert_eq!(location.clip.id, ids[1]);
        assert_eq!(location.source_time, Ticks(10));
    }

    #[test]
    fn a_clip_boundary_belongs_to_the_later_clip() {
        let (timeline, ids) = three_clips();

        let location = timeline.locate(Ticks(10)).expect("a clip is there");

        assert_eq!(location.clip.id, ids[1]);
        assert_eq!(location.source_time, Ticks(5));
    }

    #[test]
    fn the_end_of_the_timeline_belongs_to_the_last_clip() {
        let (timeline, ids) = three_clips();

        for time in [60, 500] {
            let location = timeline.locate(Ticks(time)).expect("a clip is there");
            assert_eq!(location.clip.id, ids[2]);
            assert_eq!(location.source_time, Ticks(70));
        }
    }

    #[test]
    fn dead_space_has_no_clip() {
        let (mut timeline, ids) = three_clips();
        timeline.remove(ids[1]);

        assert_eq!(
            timeline.locate(Ticks(9)).expect("first clip").clip.id,
            ids[0]
        );
        assert_eq!(timeline.locate(Ticks(10)), None);
        assert_eq!(timeline.locate(Ticks(29)), None);
        assert_eq!(
            timeline.locate(Ticks(30)).expect("last clip").clip.id,
            ids[2]
        );
    }

    #[test]
    fn splitting_makes_two_clips_covering_the_same_footage() {
        let (mut timeline, ids) = three_clips();

        let second = timeline
            .split(ids[1], Ticks(12))
            .expect("the split is inside");

        assert_eq!(timeline.clips().len(), 4);
        assert_eq!(timeline.clips()[1].id, ids[1]);
        assert_eq!(timeline.clips()[1].source, range(5, 12));
        assert_eq!(timeline.clips()[2].id, second);
        assert_eq!(timeline.clips()[2].source, range(12, 25));
        assert_eq!(timeline.clips()[2].media, OTHER);
        assert_eq!(starts(&timeline), [0, 10, 17, 30]);
        assert_eq!(timeline.duration(), Ticks(60));
    }

    #[test]
    fn splitting_at_or_outside_a_clips_ends_does_nothing() {
        let (mut timeline, ids) = three_clips();
        let before = timeline.clone();

        for time in [0, 5, 25, 90] {
            assert_eq!(timeline.split(ids[1], Ticks(time)), None);
        }

        assert_eq!(timeline, before);
    }

    #[test]
    fn deleting_a_clip_leaves_dead_space() {
        let (mut timeline, ids) = three_clips();

        assert!(timeline.remove(ids[1]));

        assert_eq!(order(&timeline), [ids[0], ids[2]]);
        assert_eq!(starts(&timeline), [0, 30]);
        assert_eq!(timeline.duration(), Ticks(60));
        assert_eq!(timeline.gaps(), [range(10, 30)]);
    }

    #[test]
    fn deleting_the_last_clip_shortens_the_timeline() {
        let (mut timeline, ids) = three_clips();

        timeline.remove(ids[2]);

        assert_eq!(timeline.duration(), Ticks(30));
        assert!(timeline.gaps().is_empty());
    }

    #[test]
    fn deleting_an_unknown_clip_changes_nothing() {
        let (mut timeline, _) = three_clips();

        assert!(!timeline.remove(ClipId(u64::MAX)));
        assert_eq!(timeline.clips().len(), 3);
    }

    #[test]
    fn removing_a_video_removes_all_its_clips() {
        let (mut timeline, ids) = three_clips();

        assert_eq!(timeline.clips_using(VIDEO), 2);
        assert_eq!(timeline.remove_media(VIDEO), 2);

        assert_eq!(order(&timeline), [ids[1]]);
    }

    #[test]
    fn dead_space_can_be_removed_by_hand() {
        let (mut timeline, ids) = three_clips();
        timeline.remove(ids[1]);

        assert!(timeline.remove_gap(Ticks(20)));

        assert_eq!(starts(&timeline), [0, 10]);
        assert!(timeline.gaps().is_empty());
        assert_eq!(timeline.duration(), Ticks(40));
    }

    #[test]
    fn removing_one_gap_leaves_the_others() {
        let (mut timeline, ids) = three_clips();
        timeline.trim(ids[0], ClipEdge::Start, Ticks(4), Ticks(100), Ticks(1));
        timeline.remove(ids[1]);

        assert_eq!(timeline.gaps(), [range(0, 4), range(10, 30)]);
        timeline.remove_gap(Ticks(2));

        assert_eq!(starts(&timeline), [0, 26]);
        assert_eq!(timeline.gaps(), [range(6, 26)]);
    }

    #[test]
    fn removing_a_gap_where_there_is_none_changes_nothing() {
        let (mut timeline, _) = three_clips();
        let before = timeline.clone();

        assert!(!timeline.remove_gap(Ticks(15)));
        assert!(!timeline.remove_gap(Ticks(500)));

        assert_eq!(timeline, before);
    }

    #[test]
    fn trimming_the_start_leaves_dead_space_and_the_footage_in_place() {
        let (mut timeline, ids) = three_clips();

        timeline.trim(ids[2], ClipEdge::Start, Ticks(50), Ticks(100), Ticks(1));

        let clip = timeline.clips()[2];
        assert_eq!(clip.source, range(50, 70));
        assert_eq!(clip.start, Ticks(40));
        assert_eq!(clip.end(), Ticks(60));
        assert_eq!(timeline.gaps(), [range(30, 40)]);
        assert_eq!(timeline.duration(), Ticks(60));
    }

    #[test]
    fn trimming_the_first_clips_start_keeps_the_total_length() {
        let mut timeline = Timeline::default();
        let id = timeline.push(VIDEO, range(0, 100));

        timeline.trim(id, ClipEdge::Start, Ticks(30), Ticks(100), Ticks(1));

        assert_eq!(timeline.clips()[0].start, Ticks(30));
        assert_eq!(timeline.duration(), Ticks(100));
        assert_eq!(timeline.gaps(), [range(0, 30)]);
    }

    #[test]
    fn trimming_the_end_leaves_dead_space_before_the_next_clip() {
        let (mut timeline, ids) = three_clips();

        timeline.trim(ids[1], ClipEdge::End, Ticks(15), Ticks(100), Ticks(1));

        assert_eq!(timeline.clips()[1].source, range(5, 15));
        assert_eq!(starts(&timeline), [0, 10, 30]);
        assert_eq!(timeline.gaps(), [range(20, 30)]);
    }

    #[test]
    fn trimming_the_last_clips_end_shortens_the_timeline() {
        let (mut timeline, ids) = three_clips();

        timeline.trim(ids[2], ClipEdge::End, Ticks(60), Ticks(100), Ticks(1));

        assert_eq!(timeline.clips()[2].source, range(40, 60));
        assert_eq!(timeline.duration(), Ticks(50));
    }

    #[test]
    fn a_trim_can_bring_back_footage_up_to_the_source_ends() {
        let mut timeline = Timeline::default();
        let id = timeline.place(VIDEO, range(40, 70), Ticks(40));

        timeline.trim(id, ClipEdge::Start, Ticks(-30), Ticks(100), Ticks(1));
        timeline.trim(id, ClipEdge::End, Ticks(500), Ticks(100), Ticks(1));

        assert_eq!(timeline.clips()[0].source, range(0, 100));
        assert_eq!(timeline.clips()[0].start, Ticks(0));
    }

    #[test]
    fn a_trim_stops_at_its_neighbours() {
        let (mut timeline, ids) = three_clips();
        let before = timeline.clone();

        timeline.trim(ids[1], ClipEdge::Start, Ticks(0), Ticks(100), Ticks(1));
        timeline.trim(ids[1], ClipEdge::End, Ticks(90), Ticks(100), Ticks(1));

        assert_eq!(timeline, before);
    }

    #[test]
    fn a_trim_can_only_grow_into_the_dead_space_there_is() {
        let (mut timeline, ids) = three_clips();
        timeline.trim(ids[1], ClipEdge::Start, Ticks(15), Ticks(100), Ticks(1));
        timeline.trim(ids[1], ClipEdge::Start, Ticks(0), Ticks(100), Ticks(1));

        assert_eq!(timeline.clips()[1].source, range(5, 25));
        assert_eq!(timeline.clips()[1].start, Ticks(10));
    }

    #[test]
    fn a_trim_always_leaves_some_clip() {
        let (mut timeline, ids) = three_clips();

        timeline.trim(ids[2], ClipEdge::Start, Ticks(90), Ticks(100), Ticks(5));
        assert_eq!(timeline.clips()[2].source, range(65, 70));

        timeline.trim(ids[2], ClipEdge::End, Ticks(0), Ticks(100), Ticks(5));
        assert_eq!(timeline.clips()[2].source, range(65, 70));
    }

    #[test]
    fn a_trim_of_an_unknown_clip_changes_nothing() {
        let (mut timeline, _) = three_clips();
        let before = timeline.clone();

        timeline.trim(
            ClipId(u64::MAX),
            ClipEdge::End,
            Ticks(1),
            Ticks(100),
            Ticks(1),
        );

        assert_eq!(timeline, before);
    }

    #[test]
    fn a_clip_moves_into_empty_space_without_touching_the_others() {
        let (mut timeline, ids) = three_clips();

        timeline.move_clip(ids[0], Ticks(200));

        assert_eq!(order(&timeline), [ids[1], ids[2], ids[0]]);
        assert_eq!(starts(&timeline), [10, 30, 200]);
        assert_eq!(timeline.gaps(), [range(0, 10), range(60, 200)]);
    }

    #[test]
    fn a_clip_cannot_move_before_the_start() {
        let (mut timeline, ids) = three_clips();
        timeline.remove(ids[0]);

        timeline.move_clip(ids[1], Ticks(-50));

        assert_eq!(timeline.clip(ids[1]).expect("still there").start, Ticks(0));
    }

    #[test]
    fn dropping_on_a_clips_front_half_goes_before_it_and_pushes_later_clips() {
        let (mut timeline, ids) = three_clips();

        timeline.move_clip(ids[0], Ticks(12));

        assert_eq!(order(&timeline), [ids[0], ids[1], ids[2]]);
        assert_eq!(starts(&timeline), [10, 20, 40]);
        assert_no_overlaps(&timeline);
    }

    #[test]
    fn dropping_on_a_clips_back_half_goes_after_it() {
        let (mut timeline, ids) = three_clips();

        timeline.move_clip(ids[0], Ticks(24));

        assert_eq!(order(&timeline), [ids[1], ids[0], ids[2]]);
        assert_eq!(starts(&timeline), [10, 30, 40]);
        assert_no_overlaps(&timeline);
    }

    #[test]
    fn dead_space_absorbs_a_push() {
        let mut timeline = Timeline::default();
        let first = timeline.place(VIDEO, range(0, 10), Ticks(0));
        let second = timeline.place(VIDEO, range(0, 10), Ticks(30));
        let third = timeline.place(VIDEO, range(0, 10), Ticks(60));
        let moving = timeline.place(VIDEO, range(0, 25), Ticks(100));

        timeline.move_clip(moving, Ticks(12));

        assert_eq!(order(&timeline), [first, moving, second, third]);
        // Only the second clip has to move. The gap after it takes the rest.
        assert_eq!(starts(&timeline), [0, 12, 37, 60]);
    }

    #[test]
    fn a_clips_tail_landing_on_another_pushes_it_along() {
        let mut timeline = Timeline::default();
        let moving = timeline.place(VIDEO, range(0, 20), Ticks(100));
        let other = timeline.place(VIDEO, range(0, 10), Ticks(30));

        timeline.move_clip(moving, Ticks(15));

        assert_eq!(order(&timeline), [moving, other]);
        assert_eq!(starts(&timeline), [15, 35]);
    }

    #[test]
    fn dropping_a_clip_where_it_already_is_changes_nothing() {
        let (mut timeline, ids) = three_clips();
        let before = timeline.clone();

        timeline.move_clip(ids[1], Ticks(10));

        assert_eq!(timeline, before);
    }

    #[test]
    fn moving_an_unknown_clip_changes_nothing() {
        let (mut timeline, _) = three_clips();
        let before = timeline.clone();

        timeline.move_clip(ClipId(u64::MAX), Ticks(5));

        assert_eq!(timeline, before);
    }

    #[test]
    fn a_new_clip_lands_where_it_was_dropped() {
        let (mut timeline, ids) = three_clips();

        let new = timeline.place(OTHER, range(0, 5), Ticks(80));

        assert_eq!(order(&timeline), [ids[0], ids[1], ids[2], new]);
        assert_eq!(timeline.clip(new).expect("placed").start, Ticks(80));
        assert_eq!(timeline.gaps(), [range(60, 80)]);
    }

    #[test]
    fn a_new_clip_makes_room_for_itself() {
        let (mut timeline, ids) = three_clips();

        let new = timeline.place(OTHER, range(0, 5), Ticks(11));

        assert_eq!(order(&timeline), [ids[0], new, ids[1], ids[2]]);
        assert_eq!(starts(&timeline), [0, 10, 15, 35]);
        assert_no_overlaps(&timeline);
    }

    #[test]
    fn pushing_adds_to_the_end_after_any_dead_space() {
        let (mut timeline, ids) = three_clips();
        timeline.remove(ids[1]);

        let new = timeline.push(OTHER, range(0, 5));

        assert_eq!(timeline.clip(new).expect("added").start, Ticks(60));
    }

    #[test]
    fn new_clips_never_reuse_an_id_after_going_back_in_time() {
        let (mut timeline, _) = three_clips();
        let snapshot = timeline.clone();
        let later = timeline.push(VIDEO, range(0, 1));

        timeline = snapshot;
        let again = timeline.push(VIDEO, range(0, 1));

        assert_ne!(later, again);
    }
}
