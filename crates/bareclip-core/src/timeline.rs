//! The project's timeline: clips of source videos, each at its own place in time.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::time::Ticks;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeRange {
    pub start: Ticks,
    pub end: Ticks,
}

impl TimeRange {
    pub fn duration(self) -> Ticks {
        Ticks(self.end.0 - self.start.0)
    }
}

/// A video added to the project.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MediaId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClipId(pub u64);

/// Ids stay unique across undo and redo because they never come from a snapshot.
fn next_clip_id() -> ClipId {
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
}

impl Clip {
    pub fn duration(self) -> Ticks {
        self.source.duration()
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
        let offset = time.0.min(clip.end().0) - clip.start.0;
        Some(Location {
            index,
            clip,
            source_time: Ticks(clip.source.start.0 + offset),
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
        });
        id
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
        let second = next_clip_id();
        self.clips.insert(
            index + 1,
            Clip {
                id: second,
                media: clip.media,
                start: Ticks(clip.start.0 + source_time.0 - clip.source.start.0),
                source: TimeRange {
                    start: source_time,
                    end: clip.source.end,
                },
            },
        );
        Some(second)
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
        match edge {
            ClipEdge::Start => {
                let earliest = (clip.source.start.0 - room_before).max(0);
                let latest = (clip.source.end.0 - min_duration.0).max(earliest);
                let new_start = source_time.0.clamp(earliest, latest);
                clip.start = Ticks(clip.start.0 + new_start - clip.source.start.0);
                clip.source.start = Ticks(new_start);
            }
            ClipEdge::End => {
                let earliest = clip.source.start.0 + min_duration.0;
                let limit = match room_after {
                    Some(room) => source_duration.0.min(clip.source.end.0 + room),
                    None => source_duration.0,
                };
                let latest = limit.max(earliest);
                clip.source.end = Ticks(source_time.0.clamp(earliest, latest));
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
