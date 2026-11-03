//! Tracks of clips stacked over each other. Where clips overlap, the highest track's
//! picture and sound win, so the whole stack plays as one sequence of clips.

use crate::{
    time::Ticks,
    timeline::{Clip, ClipEdge, ClipId, MediaId, TimeRange, Timeline},
};

/// A later piece of a clip that another track's clip cuts in two. The first piece keeps
/// the clip's own id, and these stay the same for as long as the arrangement does.
const PIECE_ID_BIT: u64 = 1 << 63;
const PIECE_NUMBER_BITS: u32 = 16;

/// Where something dragged onto the tracks lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackTarget {
    Existing(usize),
    /// A new track, which goes in at this position.
    New(usize),
}

/// The project's tracks, highest first. A track with no clips on it disappears.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tracks {
    tracks: Vec<Timeline>,
    played: Timeline,
}

impl Tracks {
    pub fn tracks(&self) -> &[Timeline] {
        &self.tracks
    }

    /// What actually plays and exports: the visible stretch of every clip, in order.
    pub fn played(&self) -> &Timeline {
        &self.played
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    /// Where the last clip on any track ends.
    pub fn duration(&self) -> Ticks {
        self.played.duration()
    }

    pub fn clip(&self, id: ClipId) -> Option<Clip> {
        self.tracks.iter().find_map(|track| track.clip(id))
    }

    pub fn track_of(&self, id: ClipId) -> Option<usize> {
        self.tracks
            .iter()
            .position(|track| track.index_of(id).is_some())
    }

    pub fn clips_using(&self, media: MediaId) -> usize {
        self.tracks
            .iter()
            .map(|track| track.clips_using(media))
            .sum()
    }

    /// The highest clip at a moment, which is the one on screen.
    pub fn topmost_at(&self, time: Ticks) -> Option<Clip> {
        self.tracks.iter().find_map(|track| {
            track
                .clips()
                .iter()
                .find(|clip| time >= clip.start && time < clip.end())
                .copied()
        })
    }

    /// The clip a split at this moment applies to: the selected one if the moment falls
    /// inside it, or otherwise the one on screen.
    pub fn clip_to_split(&self, selected: Option<ClipId>, time: Ticks) -> Option<Clip> {
        selected
            .and_then(|id| self.clip(id))
            .filter(|clip| time >= clip.start && time < clip.end())
            .or_else(|| self.topmost_at(time))
    }

    /// Dead space: stretches where nothing plays on any track.
    pub fn gaps(&self) -> Vec<TimeRange> {
        self.played.gaps()
    }

    pub fn gap_at(&self, time: Ticks) -> Option<TimeRange> {
        self.played.gap_at(time)
    }

    /// Where something of a given length would end up if dropped on a target.
    pub fn landing(
        &self,
        target: TrackTarget,
        moving: Option<ClipId>,
        wanted: Ticks,
        length: Ticks,
    ) -> Ticks {
        match target {
            TrackTarget::Existing(index) => match self.tracks.get(index) {
                Some(track) => track.landing(moving, wanted, length),
                None => Ticks(wanted.0.max(0)),
            },
            TrackTarget::New(_) => Ticks(wanted.0.max(0)),
        }
    }

    /// Adds a clip after everything else, on the top track, so nothing covers it and
    /// nothing has to move for it.
    pub fn push(&mut self, media: MediaId, source: TimeRange) -> ClipId {
        let end = self.duration();
        self.place(media, source, end, TrackTarget::Existing(0))
    }

    /// Adds a clip as close as it can get to a moment on a target.
    pub fn place(
        &mut self,
        media: MediaId,
        source: TimeRange,
        wanted: Ticks,
        target: TrackTarget,
    ) -> ClipId {
        let index = self.make_room(target);
        let id = self.tracks[index].place(media, source, wanted);
        self.refresh();
        id
    }

    /// Moves a clip as close as it can get to a moment on a target.
    pub fn move_clip(&mut self, id: ClipId, wanted: Ticks, target: TrackTarget) {
        let Some(from) = self.track_of(id) else {
            return;
        };
        if target == TrackTarget::Existing(from) {
            self.tracks[from].move_clip(id, wanted);
            self.refresh();
            return;
        }
        let Some(clip) = self.tracks[from].clip(id) else {
            return;
        };
        let index = self.make_room(target);
        // A new track above pushes the clip's own track one further down.
        let from = if index <= from && matches!(target, TrackTarget::New(_)) {
            from + 1
        } else {
            from
        };
        self.tracks[from].remove(id);
        self.tracks[index].land(clip, wanted);
        self.refresh();
    }

    /// Puts a copy of a clip straight after it on its own track.
    pub fn duplicate(&mut self, id: ClipId) -> Option<ClipId> {
        let track = self.track_of(id)?;
        let copy = self.tracks[track].duplicate(id);
        self.refresh();
        copy
    }

    /// Splits a clip in two at a moment in its source video.
    pub fn split(&mut self, id: ClipId, source_time: Ticks) -> Option<ClipId> {
        let track = self.track_of(id)?;
        let second = self.tracks[track].split(id, source_time);
        self.refresh();
        second
    }

    /// Removes a clip and leaves its stretch of the track empty.
    pub fn remove(&mut self, id: ClipId) -> bool {
        let Some(track) = self.track_of(id) else {
            return false;
        };
        let removed = self.tracks[track].remove(id);
        self.refresh();
        removed
    }

    /// Removes every clip of a source video and returns how many there were.
    pub fn remove_media(&mut self, media: MediaId) -> usize {
        let removed = self
            .tracks
            .iter_mut()
            .map(|track| track.remove_media(media))
            .sum();
        self.refresh();
        removed
    }

    /// Moves one end of a clip. It stops at its neighbours on its own track only.
    pub fn trim(
        &mut self,
        id: ClipId,
        edge: ClipEdge,
        source_time: Ticks,
        source_duration: Ticks,
        min_duration: Ticks,
    ) {
        let Some(track) = self.track_of(id) else {
            return;
        };
        self.tracks[track].trim(id, edge, source_time, source_duration, min_duration);
        self.refresh();
    }

    /// Closes the dead space around a moment. Every track moves together, so clips keep
    /// their places relative to each other.
    pub fn remove_gap(&mut self, time: Ticks) -> bool {
        let Some(gap) = self.gap_at(time) else {
            return false;
        };
        for track in &mut self.tracks {
            track.shift_from(gap.end, gap.duration());
        }
        self.refresh();
        true
    }

    /// Closes all the dead space.
    pub fn remove_all_gaps(&mut self) -> bool {
        let gaps = self.gaps();
        // From the last one back, so each gap is still where it was found.
        for gap in gaps.iter().rev() {
            for track in &mut self.tracks {
                track.shift_from(gap.end, gap.duration());
            }
        }
        self.refresh();
        !gaps.is_empty()
    }

    /// The position of the track a target refers to, making a new one if it needs to.
    fn make_room(&mut self, target: TrackTarget) -> usize {
        let new_at = match target {
            TrackTarget::Existing(index) if index < self.tracks.len() => return index,
            TrackTarget::Existing(_) => self.tracks.len(),
            TrackTarget::New(index) => index.min(self.tracks.len()),
        };
        self.tracks.insert(new_at, Timeline::default());
        new_at
    }

    fn refresh(&mut self) {
        self.tracks.retain(|track| !track.is_empty());
        self.played = flatten(&self.tracks);
    }
}

/// What can be seen of a stack of tracks, highest first.
fn flatten(tracks: &[Timeline]) -> Timeline {
    // The stretches higher tracks already fill, in order and never overlapping.
    let mut covered: Vec<TimeRange> = Vec::new();
    let mut pieces = Vec::new();
    for track in tracks {
        for clip in track.clips() {
            visible_pieces(*clip, &covered, &mut pieces);
        }
        covered = merged(covered, track);
    }
    pieces.sort_by_key(|piece: &Clip| piece.start);
    Timeline::from_ordered(pieces)
}

/// Adds the parts of a clip that nothing covers.
fn visible_pieces(clip: Clip, covered: &[TimeRange], pieces: &mut Vec<Clip>) {
    let mut from = clip.start;
    let mut number = 0;
    let mut add = |start: Ticks, end: Ticks| {
        pieces.push(piece_of(clip, start, end, number));
        number += 1;
    };
    for range in covered {
        if range.end <= from {
            continue;
        }
        if range.start >= clip.end() {
            break;
        }
        if range.start > from {
            add(from, range.start);
        }
        from = range.end;
    }
    if from < clip.end() {
        add(from, clip.end());
    }
}

/// The stretch of a clip between two moments on the timeline.
fn piece_of(clip: Clip, start: Ticks, end: Ticks, number: u64) -> Clip {
    let into_source = clip.source.start.0 - clip.start.0;
    let id = if number == 0 {
        clip.id
    } else {
        ClipId(PIECE_ID_BIT | clip.id.0 << PIECE_NUMBER_BITS | number)
    };
    Clip {
        id,
        start,
        source: TimeRange {
            start: Ticks(start.0 + into_source),
            end: Ticks(end.0 + into_source),
        },
        ..clip
    }
}

/// Adds a track's clips to what is covered.
fn merged(covered: Vec<TimeRange>, track: &Timeline) -> Vec<TimeRange> {
    let mut all = covered;
    all.extend(track.clips().iter().map(|clip| TimeRange {
        start: clip.start,
        end: clip.end(),
    }));
    all.sort_by_key(|range| range.start);
    let mut joined: Vec<TimeRange> = Vec::with_capacity(all.len());
    for range in all {
        match joined.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => joined.push(range),
        }
    }
    joined
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

    /// What plays, as (media, timeline start, source start, source end).
    fn playing(tracks: &Tracks) -> Vec<(MediaId, i64, i64, i64)> {
        tracks
            .played()
            .clips()
            .iter()
            .map(|clip| {
                (
                    clip.media,
                    clip.start.0,
                    clip.source.start.0,
                    clip.source.end.0,
                )
            })
            .collect()
    }

    fn lower_with_upper(lower: (i64, i64), upper: (i64, i64)) -> (Tracks, ClipId, ClipId) {
        let mut tracks = Tracks::default();
        let low = tracks.place(
            VIDEO,
            range(1000, 1000 + lower.1 - lower.0),
            Ticks(lower.0),
            TrackTarget::New(0),
        );
        let high = tracks.place(
            OTHER,
            range(0, upper.1 - upper.0),
            Ticks(upper.0),
            TrackTarget::New(0),
        );
        (tracks, low, high)
    }

    #[test]
    fn one_track_plays_exactly_as_it_is_laid_out() {
        let mut tracks = Tracks::default();
        tracks.push(VIDEO, range(0, 10));
        tracks.push(OTHER, range(5, 25));

        assert_eq!(tracks.played(), &tracks.tracks()[0]);
    }

    #[test]
    fn a_clip_on_top_cuts_the_one_below_in_two_with_the_right_footage() {
        let (tracks, ..) = lower_with_upper((0, 100), (40, 60));

        assert_eq!(
            playing(&tracks),
            vec![
                (VIDEO, 0, 1000, 1040),
                (OTHER, 40, 0, 20),
                (VIDEO, 60, 1060, 1100),
            ]
        );
    }

    #[test]
    fn pieces_of_a_clip_can_be_told_apart() {
        let (tracks, low, _) = lower_with_upper((0, 100), (40, 60));

        let ids: Vec<ClipId> = tracks.played().clips().iter().map(|clip| clip.id).collect();

        assert_eq!(ids[0], low);
        assert_ne!(ids[2], low);
        assert_ne!(ids[0], ids[1]);
        assert_ne!(ids[1], ids[2]);
    }

    #[test]
    fn the_same_arrangement_always_flattens_the_same_way() {
        let (tracks, ..) = lower_with_upper((0, 100), (40, 60));
        let mut copy = tracks.clone();

        copy.refresh();

        assert_eq!(copy, tracks);
    }

    #[test]
    fn a_clip_that_is_completely_covered_plays_nothing() {
        let (tracks, ..) = lower_with_upper((40, 60), (0, 100));

        assert_eq!(playing(&tracks), vec![(OTHER, 0, 0, 100)]);
    }

    #[test]
    fn overlapping_ends_only_lose_the_covered_stretch() {
        let (tracks, ..) = lower_with_upper((0, 50), (30, 80));

        assert_eq!(
            playing(&tracks),
            vec![(VIDEO, 0, 1000, 1030), (OTHER, 30, 0, 50)]
        );
    }

    #[test]
    fn three_tracks_stack_from_the_top_down() {
        let mut tracks = Tracks::default();
        tracks.place(VIDEO, range(0, 100), Ticks(0), TrackTarget::New(0));
        tracks.place(OTHER, range(0, 40), Ticks(30), TrackTarget::New(0));
        tracks.place(MediaId(3), range(0, 20), Ticks(40), TrackTarget::New(0));

        assert_eq!(
            playing(&tracks),
            vec![
                (VIDEO, 0, 0, 30),
                (OTHER, 30, 0, 10),
                (MediaId(3), 40, 0, 20),
                (OTHER, 60, 30, 40),
                (VIDEO, 70, 70, 100),
            ]
        );
    }

    #[test]
    fn played_clips_never_overlap() {
        let mut tracks = Tracks::default();
        for (index, start) in [0, 7, 3, 19, 11].into_iter().enumerate() {
            tracks.place(
                VIDEO,
                range(0, 13 + index as i64),
                Ticks(start),
                TrackTarget::New(index % 2),
            );
        }

        for pair in tracks.played().clips().windows(2) {
            assert!(pair[0].end() <= pair[1].start, "{pair:?}");
        }
    }

    #[test]
    fn dropping_on_a_new_track_keeps_the_clip_where_it_was_dropped() {
        let mut tracks = Tracks::default();
        tracks.place(VIDEO, range(0, 100), Ticks(0), TrackTarget::New(0));

        let id = tracks.place(OTHER, range(0, 10), Ticks(50), TrackTarget::New(1));

        assert_eq!(tracks.tracks().len(), 2);
        assert_eq!(tracks.track_of(id), Some(1));
        assert_eq!(tracks.clip(id).map(|clip| clip.start), Some(Ticks(50)));
    }

    #[test]
    fn a_target_past_the_last_track_makes_one_at_the_end() {
        let mut tracks = Tracks::default();
        tracks.push(VIDEO, range(0, 10));

        let id = tracks.place(OTHER, range(0, 10), Ticks(0), TrackTarget::New(9));

        assert_eq!(tracks.track_of(id), Some(1));
    }

    #[test]
    fn dropping_on_an_existing_track_follows_that_tracks_rules() {
        let mut tracks = Tracks::default();
        tracks.place(VIDEO, range(0, 100), Ticks(0), TrackTarget::New(0));
        tracks.place(OTHER, range(0, 100), Ticks(0), TrackTarget::New(1));

        // Its front lands on the clip there and it is mostly past the middle.
        let id = tracks.place(OTHER, range(0, 10), Ticks(80), TrackTarget::Existing(1));

        assert_eq!(tracks.clip(id).map(|clip| clip.start), Some(Ticks(100)));
        assert_eq!(tracks.track_of(id), Some(1));
    }

    #[test]
    fn landing_says_where_a_drop_will_go_without_changing_anything() {
        let mut tracks = Tracks::default();
        tracks.place(VIDEO, range(0, 100), Ticks(0), TrackTarget::New(0));
        let before = tracks.clone();

        assert_eq!(
            tracks.landing(TrackTarget::Existing(0), None, Ticks(90), Ticks(10)),
            Ticks(100)
        );
        assert_eq!(
            tracks.landing(TrackTarget::New(0), None, Ticks(90), Ticks(10)),
            Ticks(90)
        );
        assert_eq!(tracks, before);
    }

    #[test]
    fn moving_a_clip_to_another_track_keeps_its_id_and_footage() {
        let (mut tracks, low, high) = lower_with_upper((0, 50), (100, 150));
        let before = tracks.clip(high).expect("the clip is there");

        tracks.move_clip(high, Ticks(60), TrackTarget::Existing(1));

        let after = tracks.clip(high).expect("the clip moved");
        assert_eq!(tracks.tracks().len(), 1);
        assert_eq!(tracks.track_of(low), Some(0));
        assert_eq!(after.start, Ticks(60));
        assert_eq!(after.source, before.source);
    }

    #[test]
    fn a_track_left_empty_by_a_move_disappears() {
        let (mut tracks, _, high) = lower_with_upper((0, 50), (100, 150));

        tracks.move_clip(high, Ticks(60), TrackTarget::Existing(1));

        assert_eq!(tracks.tracks().len(), 1);
    }

    #[test]
    fn moving_a_clip_to_a_new_track_above_its_own_works() {
        let mut tracks = Tracks::default();
        let first = tracks.place(VIDEO, range(0, 10), Ticks(0), TrackTarget::New(0));
        let second = tracks.place(VIDEO, range(0, 10), Ticks(20), TrackTarget::Existing(0));
        let other = tracks.place(OTHER, range(0, 10), Ticks(0), TrackTarget::New(1));

        tracks.move_clip(other, Ticks(5), TrackTarget::New(0));

        assert_eq!(tracks.tracks().len(), 2);
        assert_eq!(tracks.track_of(other), Some(0));
        assert_eq!(tracks.track_of(first), Some(1));
        assert_eq!(tracks.track_of(second), Some(1));
    }

    #[test]
    fn moving_a_clip_to_a_new_track_below_its_own_works() {
        let mut tracks = Tracks::default();
        let moved = tracks.place(VIDEO, range(0, 10), Ticks(0), TrackTarget::New(0));
        let stays = tracks.place(VIDEO, range(0, 10), Ticks(20), TrackTarget::Existing(0));

        tracks.move_clip(moved, Ticks(0), TrackTarget::New(1));

        assert_eq!(tracks.track_of(stays), Some(0));
        assert_eq!(tracks.track_of(moved), Some(1));
    }

    #[test]
    fn moving_a_clip_along_its_own_track_keeps_the_tracks() {
        let (mut tracks, low, _) = lower_with_upper((0, 50), (100, 150));

        tracks.move_clip(low, Ticks(70), TrackTarget::Existing(1));

        assert_eq!(tracks.tracks().len(), 2);
        assert_eq!(tracks.clip(low).map(|clip| clip.start), Some(Ticks(70)));
    }

    #[test]
    fn a_new_clip_goes_after_everything_else_on_the_top_track() {
        let (mut tracks, ..) = lower_with_upper((0, 500), (0, 10));

        let id = tracks.push(VIDEO, range(0, 10));

        assert_eq!(tracks.track_of(id), Some(0));
        assert_eq!(tracks.clip(id).map(|clip| clip.start), Some(Ticks(500)));
    }

    #[test]
    fn the_project_lasts_until_the_last_clip_on_any_track_ends() {
        let (tracks, ..) = lower_with_upper((0, 500), (0, 10));

        assert_eq!(tracks.duration(), Ticks(500));
    }

    #[test]
    fn an_empty_stretch_of_one_track_is_not_dead_space_while_another_plays() {
        let (tracks, ..) = lower_with_upper((0, 100), (200, 300));
        let (covered, ..) = lower_with_upper((0, 300), (200, 250));

        assert_eq!(tracks.gaps(), vec![range(100, 200)]);
        assert_eq!(covered.gaps(), vec![]);
    }

    #[test]
    fn removing_a_gap_moves_every_track_together() {
        let mut tracks = Tracks::default();
        let early = tracks.place(VIDEO, range(0, 10), Ticks(0), TrackTarget::New(0));
        let high = tracks.place(VIDEO, range(0, 10), Ticks(50), TrackTarget::New(0));
        let low = tracks.place(OTHER, range(0, 10), Ticks(55), TrackTarget::New(2));

        assert!(tracks.remove_gap(Ticks(20)));

        assert_eq!(tracks.clip(early).map(|clip| clip.start), Some(Ticks(0)));
        assert_eq!(tracks.clip(high).map(|clip| clip.start), Some(Ticks(10)));
        assert_eq!(tracks.clip(low).map(|clip| clip.start), Some(Ticks(15)));
        assert!(tracks.gaps().is_empty());
    }

    #[test]
    fn removing_a_gap_that_is_not_there_changes_nothing() {
        let (mut tracks, ..) = lower_with_upper((0, 100), (0, 10));
        let before = tracks.clone();

        assert!(!tracks.remove_gap(Ticks(50)));
        assert_eq!(tracks, before);
    }

    #[test]
    fn removing_all_gaps_closes_each_one_and_keeps_the_stack_in_step() {
        let mut tracks = Tracks::default();
        tracks.place(VIDEO, range(0, 10), Ticks(20), TrackTarget::New(0));
        let high = tracks.place(VIDEO, range(0, 10), Ticks(50), TrackTarget::New(0));
        let low = tracks.place(OTHER, range(0, 20), Ticks(55), TrackTarget::New(2));

        assert!(tracks.remove_all_gaps());

        assert!(tracks.gaps().is_empty());
        assert_eq!(tracks.clip(high).map(|clip| clip.start), Some(Ticks(10)));
        assert_eq!(tracks.clip(low).map(|clip| clip.start), Some(Ticks(15)));
        assert!(!tracks.remove_all_gaps());
    }

    #[test]
    fn a_trim_stops_at_neighbours_on_its_own_track_only() {
        let mut tracks = Tracks::default();
        let low = tracks.place(VIDEO, range(0, 50), Ticks(0), TrackTarget::New(0));
        tracks.place(OTHER, range(0, 50), Ticks(60), TrackTarget::New(0));

        // The other clip is on another track, so the end can pass it.
        tracks.trim(low, ClipEdge::End, Ticks(90), Ticks(100), Ticks(1));

        assert_eq!(tracks.clip(low).map(|clip| clip.end()), Some(Ticks(90)));
    }

    #[test]
    fn a_split_applies_to_the_selected_clip_when_the_moment_is_inside_it() {
        let (tracks, low, high) = lower_with_upper((0, 100), (40, 60));

        let chosen = |selected, time| {
            tracks
                .clip_to_split(selected, Ticks(time))
                .map(|clip: Clip| clip.id)
        };

        assert_eq!(chosen(Some(low), 50), Some(low));
        assert_eq!(chosen(None, 50), Some(high));
        assert_eq!(chosen(Some(high), 10), Some(low));
        assert_eq!(chosen(None, 500), None);
    }

    #[test]
    fn splitting_a_clip_on_a_lower_track_keeps_it_there() {
        let (mut tracks, low, _) = lower_with_upper((0, 100), (40, 60));

        let second = tracks
            .split(low, Ticks(1050))
            .expect("the moment is inside");

        assert_eq!(tracks.track_of(second), Some(1));
        assert_eq!(tracks.clip(second).map(|clip| clip.start), Some(Ticks(50)));
    }

    #[test]
    fn a_duplicate_stays_on_its_clips_track() {
        let (mut tracks, low, _) = lower_with_upper((0, 100), (40, 60));

        let copy = tracks.duplicate(low).expect("the clip is there");

        assert_eq!(tracks.track_of(copy), Some(1));
        assert_eq!(tracks.clip(copy).map(|clip| clip.start), Some(Ticks(100)));
    }

    #[test]
    fn removing_the_last_clip_of_a_track_removes_the_track() {
        let (mut tracks, _, high) = lower_with_upper((0, 100), (40, 60));

        assert!(tracks.remove(high));

        assert_eq!(tracks.tracks().len(), 1);
        assert_eq!(playing(&tracks), vec![(VIDEO, 0, 1000, 1100)]);
        assert!(!tracks.remove(high));
    }

    #[test]
    fn removing_a_video_takes_its_clips_from_every_track() {
        let mut tracks = Tracks::default();
        tracks.place(VIDEO, range(0, 10), Ticks(0), TrackTarget::New(0));
        tracks.place(VIDEO, range(0, 10), Ticks(0), TrackTarget::New(1));
        tracks.place(OTHER, range(0, 10), Ticks(20), TrackTarget::New(2));

        assert_eq!(tracks.clips_using(VIDEO), 2);
        assert_eq!(tracks.remove_media(VIDEO), 2);

        assert_eq!(tracks.clips_using(VIDEO), 0);
        assert_eq!(tracks.tracks().len(), 1);
    }
}
