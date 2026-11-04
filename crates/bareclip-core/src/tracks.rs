//! Tracks of clips stacked over each other. Where clips overlap, the highest track's picture wins,
//! so the pictures play as one sequence of clips. Below those are rows that only hold sound.
//! Everything on every row can be heard at once.

use crate::{
    placement::{Placement, final_size},
    project::SavedClip,
    sound::ClipSound,
    speed::Speed,
    time::Ticks,
    timeline::{Clip, ClipEdge, ClipId, MediaId, TimeRange, Timeline, next_clip_id},
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

/// Which way a clip can move among those it overlaps, as seen on the picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderMove {
    Forward,
    Backward,
    ToFront,
    ToBack,
}

/// A stretch of the timeline over which the same clips show. Dead space has none of these.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stretch {
    pub start: Ticks,
    pub end: Ticks,
    /// What to draw, lowest first.
    pub stack: Vec<Clip>,
}

/// What kind of clip a row takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowKind {
    Picture,
    /// Only holds titles. These sit among the picture rows, because they are pictures too.
    Text,
    Sound,
}

/// A stretch of a file to hear. Joins that carry straight on through the same footage are already
/// made into one, so nothing has to reopen the file or risk a click there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoundSpan {
    pub media: MediaId,
    pub start: Ticks,
    pub source: TimeRange,
    pub sound: ClipSound,
    pub speed: Speed,
    /// Which sound row it is on, counted from the top. Picture rows have none.
    pub sound_row: Option<usize>,
}

impl SoundSpan {
    pub fn length(&self) -> Ticks {
        self.speed.on_timeline(self.source.duration())
    }

    pub fn end(&self) -> Ticks {
        Ticks(self.start.0 + self.length().0)
    }

    fn carries_on_from(&self, earlier: &Self) -> bool {
        self.media == earlier.media
            && self.sound_row == earlier.sound_row
            && self.start == earlier.end()
            && self.source.start == earlier.source.end
            && self.speed == earlier.speed
            && self.sound.volume == earlier.sound.volume
            && self.sound.only_track == earlier.sound.only_track
            && self.sound.fade_in == 0
            && earlier.sound.fade_out == 0
    }
}

/// The project's rows from the top down: pictures, highest first, and then sound. A row with no
/// clips on it disappears.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tracks {
    tracks: Vec<Timeline>,
    /// How many of the rows, from the top, hold pictures.
    picture_rows: usize,
    played: Timeline,
    played_videos: Timeline,
    /// Where nothing plays on any row.
    gaps: Vec<TimeRange>,
}

impl Tracks {
    /// Puts saved picture rows back. Clips of a video `media` doesn't recognise are left out.
    pub fn restore(saved: &[Vec<SavedClip>], media: impl Fn(u64) -> Option<MediaId>) -> Self {
        Self::restore_with_sound(saved, &[], &[], media)
    }

    /// Puts saved rows back, picture rows and then sound rows. `muted_rows` says which rows an
    /// older file muted as a whole, and their clips come back muted, so they stay silent.
    pub fn restore_with_sound(
        pictures: &[Vec<SavedClip>],
        sounds: &[Vec<SavedClip>],
        muted_rows: &[bool],
        media: impl Fn(u64) -> Option<MediaId>,
    ) -> Self {
        let tracks: Vec<Timeline> = pictures
            .iter()
            .chain(sounds)
            .enumerate()
            .map(|(row, track)| {
                let row_muted = muted_rows.get(row).copied().unwrap_or(false);
                Timeline::restore(track.iter().filter_map(|clip| {
                    Some((
                        media(clip.media)?,
                        Ticks(clip.start),
                        clip.source(),
                        clip.placement,
                        ClipSound {
                            muted: clip.sound.muted || row_muted,
                            ..clip.sound
                        },
                        clip.speed,
                    ))
                }))
            })
            .collect();
        let mut restored = Self {
            tracks,
            picture_rows: pictures.len(),
            played: Timeline::default(),
            played_videos: Timeline::default(),
            gaps: Vec::new(),
        };
        restored.refresh();
        restored
    }

    /// Every row, from the top down.
    pub fn tracks(&self) -> &[Timeline] {
        &self.tracks
    }

    /// The rows that hold pictures, highest first.
    pub fn picture_tracks(&self) -> &[Timeline] {
        &self.tracks[..self.picture_rows]
    }

    /// The rows that only hold sound, from the top down.
    pub fn audio_tracks(&self) -> &[Timeline] {
        &self.tracks[self.picture_rows..]
    }

    /// How many rows, from the top, hold pictures.
    pub fn picture_rows(&self) -> usize {
        self.picture_rows
    }

    /// Where a drop on this target really lands for a clip of this media.
    pub fn target_for(&self, target: TrackTarget, media: MediaId, sound_only: bool) -> TrackTarget {
        let kind = if sound_only {
            RowKind::Sound
        } else if media.is_title() {
            RowKind::Text
        } else {
            RowKind::Picture
        };
        self.fitted_target(target, kind)
    }

    /// Whether a row only holds titles.
    pub fn is_text_row(&self, row: usize) -> bool {
        self.kind_of_row(row) == RowKind::Text
    }

    /// Whether a row only holds sound.
    pub fn is_sound_row(&self, row: usize) -> bool {
        row >= self.picture_rows
    }

    /// What actually plays and exports: the visible stretch of every clip, in order.
    pub fn played(&self) -> &Timeline {
        &self.played
    }

    /// The same, with images treated as if they weren't there. This is what has to be decoded: a
    /// video under a logo is still the video to play, and an image has nothing to play.
    pub fn played_videos(&self) -> &Timeline {
        &self.played_videos
    }

    /// Whether any row holds an image.
    pub fn has_stills(&self) -> bool {
        self.picture_tracks()
            .iter()
            .flat_map(Timeline::clips)
            .any(|clip| clip.is_still())
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    /// Where the last clip on any row ends.
    pub fn duration(&self) -> Ticks {
        self.tracks
            .iter()
            .map(Timeline::duration)
            .max()
            .unwrap_or(Ticks(0))
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
        self.picture_tracks().iter().find_map(|track| {
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
            .or_else(|| self.heard_only_at(time))
    }

    /// The highest clip on a sound row at a moment.
    fn heard_only_at(&self, time: Ticks) -> Option<Clip> {
        self.audio_tracks().iter().find_map(|track| {
            track
                .clips()
                .iter()
                .find(|clip| time >= clip.start && time < clip.end())
                .copied()
        })
    }

    /// Dead space: stretches where nothing plays on any row. Sound on its own isn't dead space.
    pub fn gaps(&self) -> Vec<TimeRange> {
        self.gaps.clone()
    }

    pub fn gap_at(&self, time: Ticks) -> Option<TimeRange> {
        self.gaps
            .iter()
            .copied()
            .find(|gap| time >= gap.start && time < gap.end)
    }

    /// Stretches with no picture, including those with sound. They show the background.
    pub fn picture_gaps(&self) -> Vec<TimeRange> {
        self.played.gaps()
    }

    /// Whether sound carries on past the last picture.
    pub fn sound_outlasts_pictures(&self) -> bool {
        self.duration() > self.played.duration()
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

    /// Adds a clip after every other picture, on the top row that takes it, so nothing covers it
    /// and nothing has to move for it. Titles don't count, because they go over pictures.
    pub fn push(&mut self, media: MediaId, source: TimeRange) -> ClipId {
        let end = self
            .picture_tracks()
            .iter()
            .flat_map(Timeline::clips)
            .filter(|clip| !clip.is_title())
            .map(|clip| clip.end())
            .max()
            .unwrap_or(Ticks(0));
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
        let kind = Self::kind_of_picture(media);
        let index = self.make_room(target, kind);
        let id = self.tracks[index].place(media, source, wanted);
        self.refresh();
        id
    }

    /// Adds a title exactly at a moment, without moving anything: on the top row if that is a row
    /// of titles with room, or otherwise on a new row above everything.
    pub fn place_title(&mut self, media: MediaId, source: TimeRange, time: Ticks) -> ClipId {
        let (start, end) = (time, Ticks(time.0 + source.duration().0));
        let room_on_top = self.picture_rows > 0
            && self.is_text_row(0)
            && !self.tracks[0]
                .clips()
                .iter()
                .any(|clip| clip.start < end && start < clip.end());
        let target = if room_on_top {
            TrackTarget::Existing(0)
        } else {
            TrackTarget::New(0)
        };
        self.place(media, source, time, target)
    }

    /// Adds a lot of titles at once, each at exactly its own time, on as few new rows above
    /// everything as keeps them from overlapping. `titles` must be in order of time. Returns the
    /// new clips.
    pub fn place_captions(&mut self, titles: &[(MediaId, TimeRange, Ticks)]) -> Vec<ClipId> {
        let mut rows: Vec<Vec<Clip>> = Vec::new();
        let mut ids = Vec::with_capacity(titles.len());
        for (media, source, start) in titles {
            let clip = Clip {
                id: next_clip_id(),
                media: *media,
                start: *start,
                source: *source,
                placement: Placement::default(),
                sound: ClipSound::default(),
                speed: Speed::default(),
            };
            ids.push(clip.id);
            let free = rows
                .iter_mut()
                .find(|row| row.last().is_none_or(|last| last.end() <= clip.start));
            match free {
                Some(row) => row.push(clip),
                None => rows.push(vec![clip]),
            }
        }
        self.picture_rows += rows.len();
        for (place, row) in rows.into_iter().enumerate() {
            self.tracks.insert(place, Timeline::from_ordered(row));
        }
        self.refresh();
        ids
    }

    /// Adds a copy of a clip as close as it can get to a moment, on the row it came from if there
    /// still is one of its kind there, showing `media`.
    pub fn place_copy(
        &mut self,
        clip: Clip,
        media: MediaId,
        wanted: Ticks,
        target: TrackTarget,
        sound_only: bool,
    ) -> ClipId {
        let kind = if sound_only {
            RowKind::Sound
        } else {
            Self::kind_of_picture(media)
        };
        let index = self.make_room(target, kind);
        let id = self.tracks[index].place_copy(clip, media, wanted);
        self.refresh();
        id
    }

    /// Makes a clip show something else. A split or a copy of a title gets a title of its own this
    /// way, so changing one doesn't change the other.
    pub fn set_media(&mut self, id: ClipId, media: MediaId) -> bool {
        let Some(track) = self.track_of(id) else {
            return false;
        };
        let changed = self.tracks[track].set_media(id, media);
        self.refresh();
        changed
    }

    /// Makes a clip last this long on the timeline, as far as its footage allows. It keeps its
    /// start, and later clips on its row move along only as far as they must. Returns whether that
    /// changed anything.
    pub fn set_duration(&mut self, id: ClipId, duration: Ticks, source_length: Ticks) -> bool {
        let Some(track) = self.track_of(id) else {
            return false;
        };
        let changed = self.tracks[track].set_duration(id, duration, source_length);
        self.refresh();
        changed
    }

    /// Adds sound after whatever is on the top sound row.
    pub fn push_sound(&mut self, media: MediaId, source: TimeRange) -> ClipId {
        let end = self
            .audio_tracks()
            .first()
            .map_or(Ticks(0), Timeline::duration);
        self.place_sound(media, source, end, TrackTarget::Existing(self.picture_rows))
    }

    /// Adds sound as close as it can get to a moment on a target. It can only go on a sound row,
    /// so a target among the pictures means the top sound row.
    pub fn place_sound(
        &mut self,
        media: MediaId,
        source: TimeRange,
        wanted: Ticks,
        target: TrackTarget,
    ) -> ClipId {
        let index = self.make_room(target, RowKind::Sound);
        let id = self.tracks[index].place(media, source, wanted);
        self.refresh();
        id
    }

    /// Gives a clip's sound tracks clips of their own, one per track on sound rows, each in time
    /// with the clip. The clip itself goes quiet. Returns the new clips.
    pub fn detach_sound(&mut self, id: ClipId, sound_tracks: u32) -> Vec<ClipId> {
        let Some(clip) = self.clip(id) else {
            return Vec::new();
        };
        if self.track_of(id).is_none_or(|row| self.is_sound_row(row)) || sound_tracks == 0 {
            return Vec::new();
        }
        let tracks = (0..sound_tracks).map(|track| {
            let row = self.row_for_sound_at(clip.start, clip.end());
            let target = row.map_or(TrackTarget::New(self.tracks.len()), TrackTarget::Existing);
            let index = self.make_room(target, RowKind::Sound);
            let detached = next_clip_id();
            self.tracks[index].land(
                Clip {
                    id: detached,
                    placement: Placement::default(),
                    sound: ClipSound {
                        only_track: Some(track),
                        ..clip.sound
                    },
                    ..clip
                },
                clip.start,
            );
            detached
        });
        let detached: Vec<ClipId> = tracks.collect();
        if let Some(row) = self.track_of(id) {
            self.tracks[row].set_sound(
                id,
                ClipSound {
                    muted: true,
                    ..clip.sound
                },
            );
        }
        self.refresh();
        detached
    }

    /// The highest sound row with nothing on it between two moments.
    fn row_for_sound_at(&self, start: Ticks, end: Ticks) -> Option<usize> {
        (self.picture_rows..self.tracks.len()).find(|row| {
            self.tracks[*row]
                .clips()
                .iter()
                .all(|other| other.end() <= start || other.start >= end)
        })
    }

    /// Changes how a clip sounds. Returns whether that changed anything.
    pub fn set_clip_sound(&mut self, id: ClipId, sound: ClipSound) -> bool {
        let Some(track) = self.track_of(id) else {
            return false;
        };
        let changed = self.tracks[track].set_sound(id, sound);
        self.refresh();
        changed
    }

    /// Changes how fast a clip plays. Clips later on its row move along only if they must, and
    /// other rows stay as they are. Returns whether that changed anything.
    pub fn set_speed(&mut self, id: ClipId, speed: Speed) -> bool {
        let Some(track) = self.track_of(id) else {
            return false;
        };
        let changed = self.tracks[track].set_speed(id, speed);
        self.refresh();
        changed
    }

    /// Whether any clip that shows plays at another speed.
    pub fn shows_other_speeds(
        &self,
        canvas: (u32, u32),
        size_of: &dyn Fn(MediaId) -> Option<(u32, u32)>,
    ) -> bool {
        self.stretches(canvas, size_of)
            .iter()
            .any(|stretch| stretch.stack.iter().any(|clip| !clip.speed.is_normal()))
    }

    /// Everything that can be heard, in order of time. Muted clips are left out. `has_sound` says
    /// which files have any.
    pub fn sound_spans(&self, has_sound: &dyn Fn(MediaId) -> bool) -> Vec<SoundSpan> {
        let mut spans: Vec<SoundSpan> = Vec::new();
        for (row, track) in self.tracks.iter().enumerate() {
            let sound_row = self.is_sound_row(row).then(|| row - self.picture_rows);
            let mut last: Option<SoundSpan> = None;
            for clip in track.clips() {
                if clip.sound.muted || clip.sound.volume == 0 || !has_sound(clip.media) {
                    continue;
                }
                let span = SoundSpan {
                    media: clip.media,
                    start: clip.start,
                    source: clip.source,
                    sound: clip.sound,
                    speed: clip.speed,
                    sound_row,
                };
                match &mut last {
                    Some(earlier) if span.carries_on_from(earlier) => {
                        earlier.source.end = span.source.end;
                        earlier.sound.fade_out = span.sound.fade_out;
                    }
                    _ => spans.extend(last.replace(span)),
                }
            }
            spans.extend(last);
        }
        spans.sort_by_key(|span| span.start);
        spans
    }

    /// Whether the sound is just what each video recorded, one at a time, with nothing to mix. Then
    /// it can be copied, every track of it.
    pub fn sound_is_plain(&self, has_sound: &dyn Fn(MediaId) -> bool) -> bool {
        if !self.audio_tracks().is_empty() {
            return false;
        }
        let mut heard: Vec<&Clip> = self
            .tracks
            .iter()
            .flat_map(Timeline::clips)
            .filter(|clip| has_sound(clip.media))
            .collect();
        heard.sort_by_key(|clip| clip.start);
        heard
            .iter()
            .all(|clip| clip.sound.is_default() && clip.speed.is_normal())
            && heard.windows(2).all(|pair| pair[0].end() <= pair[1].start)
    }

    /// Moves a clip as close as it can get to a moment on a target. A clip stays among its own
    /// kind of row, so a target among the other kind means the closest row of its own.
    pub fn move_clip(&mut self, id: ClipId, wanted: Ticks, target: TrackTarget) {
        let Some(from) = self.track_of(id) else {
            return;
        };
        let kind = self.kind_of_row(from);
        let target = self.fitted_target(target, kind);
        if target == TrackTarget::Existing(from) {
            self.tracks[from].move_clip(id, wanted);
            self.refresh();
            return;
        }
        let Some(clip) = self.tracks[from].clip(id) else {
            return;
        };
        let index = self.make_room(target, kind);
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

    /// The picture rows, by position, with a clip overlapping this one in time.
    fn overlapping_tracks(&self, clip: Clip) -> Vec<usize> {
        self.picture_tracks()
            .iter()
            .enumerate()
            .filter(|(_, track)| {
                track.clips().iter().any(|other| {
                    other.id != clip.id && other.start < clip.end() && clip.start < other.end()
                })
            })
            .map(|(index, _)| index)
            .collect()
    }

    /// Whether a clip is in front of or behind anything it could move past. Moving only makes a
    /// difference among clips that overlap in time.
    pub fn can_move_in_order(&self, id: ClipId, order: OrderMove) -> bool {
        let (Some(clip), Some(own)) = (self.clip(id), self.track_of(id)) else {
            return false;
        };
        if self.is_sound_row(own) {
            return false;
        }
        let overlapping = self.overlapping_tracks(clip);
        match order {
            OrderMove::Forward | OrderMove::ToFront => overlapping.iter().any(|index| *index < own),
            OrderMove::Backward | OrderMove::ToBack => overlapping.iter().any(|index| *index > own),
        }
    }

    /// Moves a clip past those it overlaps, onto a track where it fits without moving in time.
    /// It uses a track that has room if it can, and makes a new one if not. Returns whether it
    /// moved.
    pub fn move_in_order(&mut self, id: ClipId, order: OrderMove) -> bool {
        if !self.can_move_in_order(id, order) {
            return false;
        }
        let (Some(clip), Some(own)) = (self.clip(id), self.track_of(id)) else {
            return false;
        };
        let overlapping = self.overlapping_tracks(clip);
        let above = overlapping.iter().copied().filter(|index| *index < own);
        let below = overlapping.iter().copied().filter(|index| *index > own);
        let kind = self.kind_of_row(own);
        let is_free = |index: usize| {
            !overlapping.contains(&index) && index != own && self.kind_of_row(index) == kind
        };
        let target = match order {
            OrderMove::Forward | OrderMove::ToFront => {
                let past = if order == OrderMove::Forward {
                    above.max()
                } else {
                    above.min()
                };
                let past = past.unwrap_or(own);
                if past > 0 && is_free(past - 1) {
                    TrackTarget::Existing(past - 1)
                } else {
                    TrackTarget::New(past)
                }
            }
            OrderMove::Backward | OrderMove::ToBack => {
                let past = if order == OrderMove::Backward {
                    below.min()
                } else {
                    below.max()
                };
                let past = past.unwrap_or(own);
                if past + 1 < self.picture_rows && is_free(past + 1) {
                    TrackTarget::Existing(past + 1)
                } else {
                    TrackTarget::New(past + 1)
                }
            }
        };
        self.move_clip(id, clip.start, target);
        true
    }

    /// Lays a clip's picture out differently. Returns whether that changed anything.
    pub fn set_placement(&mut self, id: ClipId, placement: Placement) -> bool {
        let Some(track) = self.track_of(id) else {
            return false;
        };
        let changed = self.tracks[track].set_placement(id, placement);
        self.refresh();
        changed
    }

    /// What can be seen at a moment, lowest first. Anything completely hidden by what is above
    /// it is left out, so nothing has to be decoded for it. A clip whose video has no known size
    /// can't be drawn, so it hides nothing.
    pub fn stack_at(
        &self,
        time: Ticks,
        canvas: (u32, u32),
        size_of: &dyn Fn(MediaId) -> Option<(u32, u32)>,
    ) -> Vec<Clip> {
        let mut stack = Vec::new();
        for track in self.picture_tracks() {
            // Clips are in order of time, and a project can hold thousands of captions.
            let clips = track.clips();
            let after = clips.partition_point(|clip| clip.start <= time);
            let Some(clip) = after
                .checked_sub(1)
                .map(|index| &clips[index])
                .filter(|clip| time < clip.end())
            else {
                continue;
            };
            stack.push(*clip);
            // Any image might be see-through, so what is under one always shows.
            let hides = !clip.is_still()
                && size_of(clip.media).is_some_and(|size| {
                    clip.placement
                        .at(Ticks(time.0 - clip.start.0))
                        .hides_below(size, canvas)
                });
            if hides {
                break;
            }
        }
        stack.reverse();
        stack
    }

    /// The next moment within reach where what shows changes, and what shows from then on.
    pub fn next_change(
        &self,
        time: Ticks,
        within: Ticks,
        canvas: (u32, u32),
        size_of: &dyn Fn(MediaId) -> Option<(u32, u32)>,
    ) -> Option<(Ticks, Vec<Clip>)> {
        let now = self.stack_at(time, canvas, size_of);
        let mut moments: Vec<Ticks> = self
            .picture_tracks()
            .iter()
            .flat_map(|track| track.clips())
            .flat_map(|clip| [clip.start, clip.end()])
            .filter(|moment| *moment > time && moment.0 - time.0 <= within.0)
            .collect();
        moments.sort();
        moments.dedup();
        moments.into_iter().find_map(|moment| {
            let stack = self.stack_at(moment, canvas, size_of);
            (stack != now).then_some((moment, stack))
        })
    }

    /// Every stretch of the timeline with something showing, in order.
    pub fn stretches(
        &self,
        canvas: (u32, u32),
        size_of: &dyn Fn(MediaId) -> Option<(u32, u32)>,
    ) -> Vec<Stretch> {
        let mut moments: Vec<Ticks> = self
            .picture_tracks()
            .iter()
            .flat_map(|track| track.clips())
            .flat_map(|clip| [clip.start, clip.end()])
            .collect();
        moments.sort();
        moments.dedup();

        let mut stretches: Vec<Stretch> = Vec::new();
        for pair in moments.windows(2) {
            let stack = self.stack_at(pair[0], canvas, size_of);
            if stack.is_empty() {
                continue;
            }
            match stretches.last_mut() {
                Some(last) if last.end == pair[0] && last.stack == stack => last.end = pair[1],
                _ => stretches.push(Stretch {
                    start: pair[0],
                    end: pair[1],
                    stack,
                }),
            }
        }
        stretches
    }

    /// The size of the finished video, which is the largest video that shows. What shows depends
    /// on the size, because a picture only hides what is below it if it covers the canvas, so
    /// this starts from what is on top and grows until nothing new appears.
    pub fn canvas(&self, size_of: &dyn Fn(MediaId) -> Option<(u32, u32)>) -> Option<(u32, u32)> {
        let on_top: Vec<(u32, u32)> = self
            .played
            .clips()
            .iter()
            .filter_map(|clip| size_of(clip.media))
            .collect();
        let mut canvas = final_size(&on_top)?;
        // It only ever grows, so it can't change more times than there are clips.
        let clips: usize = self
            .picture_tracks()
            .iter()
            .map(|track| track.clips().len())
            .sum();
        for _ in 0..=clips {
            let showing: Vec<(u32, u32)> = self
                .stretches(canvas, size_of)
                .iter()
                .flat_map(|stretch| &stretch.stack)
                .filter_map(|clip| size_of(clip.media))
                .collect();
            let grown = final_size(&showing)?;
            let area = |size: (u32, u32)| u64::from(size.0) * u64::from(size.1);
            if area(grown) <= area(canvas) {
                break;
            }
            canvas = grown;
        }
        Some(canvas)
    }

    /// Whether drawing this project takes layers: anywhere two pictures show at once, or one
    /// picture isn't simply in its normal place.
    pub fn needs_layers(
        &self,
        canvas: (u32, u32),
        size_of: &dyn Fn(MediaId) -> Option<(u32, u32)>,
    ) -> bool {
        self.stretches(canvas, size_of).iter().any(|stretch| {
            stretch.stack.len() > 1
                || stretch.stack.iter().any(|clip| {
                    clip.is_still() || !clip.speed.is_normal() || !clip.placement.is_default()
                })
        })
    }

    /// Whether the canvas itself can be seen anywhere: in dead space, or around the lowest picture.
    pub fn shows_canvas(
        &self,
        canvas: (u32, u32),
        size_of: &dyn Fn(MediaId) -> Option<(u32, u32)>,
    ) -> bool {
        if !self.picture_gaps().is_empty() || self.sound_outlasts_pictures() {
            return true;
        }
        self.stretches(canvas, size_of).iter().any(|stretch| {
            stretch.stack.first().is_none_or(|lowest| {
                !size_of(lowest.media).is_some_and(|size| {
                    lowest
                        .placement
                        .at(Ticks(stretch.start.0 - lowest.start.0))
                        .hides_below(size, canvas)
                })
            })
        })
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

    fn kind_of_row(&self, row: usize) -> RowKind {
        if self.is_sound_row(row) {
            RowKind::Sound
        } else if self
            .tracks
            .get(row)
            .and_then(|track| track.clips().first())
            .is_some_and(|clip| clip.is_title())
        {
            RowKind::Text
        } else {
            RowKind::Picture
        }
    }

    fn kind_of_picture(media: MediaId) -> RowKind {
        if media.is_title() {
            RowKind::Text
        } else {
            RowKind::Picture
        }
    }

    /// The nearest target to this one that a kind of clip can go on.
    fn fitted_target(&self, target: TrackTarget, kind: RowKind) -> TrackTarget {
        let pictures = 0..self.picture_rows;
        match (kind, target) {
            (RowKind::Sound, TrackTarget::Existing(row))
                if row >= self.picture_rows && row < self.tracks.len() =>
            {
                target
            }
            // The two kinds meet in the middle, so that is the nearest row of the other kind.
            (RowKind::Sound, TrackTarget::Existing(_)) if self.audio_tracks().is_empty() => {
                TrackTarget::New(self.picture_rows)
            }
            (RowKind::Sound, TrackTarget::Existing(_)) => TrackTarget::Existing(self.picture_rows),
            (RowKind::Sound, TrackTarget::New(row)) => {
                TrackTarget::New(row.clamp(self.picture_rows, self.tracks.len()))
            }
            (_, TrackTarget::New(row)) => TrackTarget::New(row.min(self.picture_rows)),
            (RowKind::Text, TrackTarget::Existing(row)) if self.is_text_row(row) => target,
            // A title dropped on anything else makes a row of its own just above it.
            (RowKind::Text, TrackTarget::Existing(row)) => {
                TrackTarget::New(row.min(self.picture_rows))
            }
            (RowKind::Picture, TrackTarget::Existing(row))
                if pictures.contains(&row) && !self.is_text_row(row) =>
            {
                target
            }
            (RowKind::Picture, TrackTarget::Existing(row)) => {
                let nearest = pictures
                    .filter(|other| !self.is_text_row(*other))
                    .min_by_key(|other| other.abs_diff(row));
                match nearest {
                    Some(other) => TrackTarget::Existing(other),
                    // Only titles so far, and pictures go under them.
                    None => TrackTarget::New(self.picture_rows),
                }
            }
        }
    }

    /// The position of the row a target refers to, making a new one if it needs to.
    fn make_room(&mut self, target: TrackTarget, kind: RowKind) -> usize {
        let new_at = match self.fitted_target(target, kind) {
            TrackTarget::Existing(index) => return index,
            TrackTarget::New(index) => index,
        };
        self.tracks.insert(new_at, Timeline::default());
        if kind != RowKind::Sound {
            self.picture_rows += 1;
        }
        new_at
    }

    fn refresh(&mut self) {
        self.picture_rows = self.tracks[..self.picture_rows]
            .iter()
            .filter(|track| !track.is_empty())
            .count();
        self.tracks.retain(|track| !track.is_empty());
        self.played = flatten(self.picture_tracks());
        self.played_videos = if self.has_stills() {
            let videos: Vec<Timeline> = self
                .picture_tracks()
                .iter()
                .map(|track| {
                    Timeline::from_ordered(
                        track
                            .clips()
                            .iter()
                            .filter(|clip| !clip.is_still())
                            .copied()
                            .collect(),
                    )
                })
                .collect();
            flatten(&videos)
        } else {
            self.played.clone()
        };
        self.gaps = gaps_between(&self.tracks);
    }
}

/// The stretches with nothing on any of these rows, up to the last clip.
fn gaps_between(rows: &[Timeline]) -> Vec<TimeRange> {
    let mut filled: Vec<TimeRange> = rows
        .iter()
        .flat_map(Timeline::clips)
        .map(|clip| TimeRange {
            start: clip.start,
            end: clip.end(),
        })
        .collect();
    filled.sort_by_key(|range| range.start);
    let mut gaps = Vec::new();
    let mut reached = Ticks(0);
    for range in filled {
        if range.start > reached {
            gaps.push(TimeRange {
                start: reached,
                end: range.start,
            });
        }
        reached = reached.max(range.end);
    }
    gaps
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
    let id = if number == 0 {
        clip.id
    } else {
        ClipId(PIECE_ID_BIT | clip.id.0 << PIECE_NUMBER_BITS | number)
    };
    Clip {
        id,
        start,
        source: TimeRange {
            start: clip.source_at(start),
            end: clip.source_at(end),
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
    use crate::time::TICKS_PER_SECOND;

    use crate::placement::WHOLE;

    const VIDEO: MediaId = MediaId(1);
    const OTHER: MediaId = MediaId(2);
    const WIDE: (u32, u32) = (1920, 1080);

    fn all_wide(_: MediaId) -> Option<(u32, u32)> {
        Some(WIDE)
    }

    fn smaller() -> Placement {
        Placement {
            scale: WHOLE / 2,
            ..Placement::default()
        }
    }

    const SONG: MediaId = MediaId(3);

    fn with_sound(_: MediaId) -> bool {
        true
    }

    fn sound_of(tracks: &Tracks, id: ClipId) -> ClipSound {
        tracks.clip(id).expect("the clip is there").sound
    }

    fn seconds(start: i64, end: i64) -> TimeRange {
        range(start * TICKS_PER_SECOND, end * TICKS_PER_SECOND)
    }

    fn a_title() -> MediaId {
        MediaId::for_title(1)
    }

    #[test]
    fn a_new_title_goes_on_a_row_of_its_own_above_everything_and_moves_nothing() {
        let mut tracks = Tracks::default();
        let video = tracks.push(MediaId(1), seconds(0, 10));
        let title = tracks.place_title(a_title(), TimeRange::new_title(), Ticks(TICKS_PER_SECOND));

        assert_eq!(tracks.track_of(title), Some(0));
        assert_eq!(tracks.track_of(video), Some(1));
        assert!(tracks.is_text_row(0));
        assert!(!tracks.is_text_row(1));
        assert_eq!(tracks.clip(video).map(|clip| clip.start), Some(Ticks(0)));
        assert_eq!(
            tracks.clip(title).map(|clip| clip.start),
            Some(Ticks(TICKS_PER_SECOND))
        );
    }

    #[test]
    fn captions_land_at_exactly_their_times_on_one_new_row_above_everything() {
        let mut tracks = Tracks::default();
        let video = tracks.push(MediaId(1), seconds(0, 10));
        let source = TimeRange::new_title();
        let times = [0, 2, 3, 9];
        let titles: Vec<(MediaId, TimeRange, Ticks)> = times
            .iter()
            .enumerate()
            .map(|(number, at)| {
                (
                    MediaId::for_title(number as u64),
                    TimeRange {
                        start: source.start,
                        end: Ticks(source.start.0 + TICKS_PER_SECOND),
                    },
                    Ticks(at * TICKS_PER_SECOND),
                )
            })
            .collect();

        let ids = tracks.place_captions(&titles);

        assert_eq!(tracks.picture_rows(), 2);
        assert!(tracks.is_text_row(0));
        assert_eq!(tracks.track_of(video), Some(1));
        for (id, at) in ids.iter().zip(times) {
            assert_eq!(tracks.track_of(*id), Some(0));
            assert_eq!(
                tracks.clip(*id).map(|clip| clip.start),
                Some(Ticks(at * TICKS_PER_SECOND))
            );
        }
    }

    #[test]
    fn captions_that_overlap_go_on_another_row_and_keep_their_times() {
        let mut tracks = Tracks::default();
        let source = TimeRange::new_title();
        let lasting = |length: i64| TimeRange {
            start: source.start,
            end: Ticks(source.start.0 + length * TICKS_PER_SECOND),
        };
        let ids = tracks.place_captions(&[
            (MediaId::for_title(1), lasting(4), Ticks(0)),
            (MediaId::for_title(2), lasting(1), Ticks(TICKS_PER_SECOND)),
            (
                MediaId::for_title(3),
                lasting(1),
                Ticks(5 * TICKS_PER_SECOND),
            ),
        ]);
        assert_eq!(tracks.tracks().len(), 2);
        assert_eq!(tracks.track_of(ids[0]), Some(0));
        assert_eq!(tracks.track_of(ids[1]), Some(1));
        assert_eq!(
            tracks.track_of(ids[2]),
            Some(0),
            "the first row has room again"
        );
        assert_eq!(
            tracks.clip(ids[1]).map(|clip| clip.start),
            Some(Ticks(TICKS_PER_SECOND))
        );
    }

    #[test]
    fn what_is_at_a_moment_is_found_among_many_clips_and_in_the_gaps_between() {
        let mut tracks = Tracks::default();
        let source = TimeRange::new_title();
        let titles: Vec<(MediaId, TimeRange, Ticks)> = (0..1000)
            .map(|number| {
                (
                    MediaId::for_title(number),
                    TimeRange {
                        start: source.start,
                        end: Ticks(source.start.0 + TICKS_PER_SECOND),
                    },
                    Ticks(number as i64 * 2 * TICKS_PER_SECOND),
                )
            })
            .collect();
        tracks.place_captions(&titles);
        let at = |seconds: f64| tracks.stack_at(Ticks::from_seconds(seconds), WIDE, &all_wide);
        assert_eq!(at(0.0)[0].media, MediaId::for_title(0));
        assert_eq!(at(1000.5)[0].media, MediaId::for_title(500));
        assert!(at(1001.5).is_empty());
        assert!(at(5000.0).is_empty());
    }

    #[test]
    fn a_new_title_lasts_five_seconds() {
        let mut tracks = Tracks::default();
        let title = tracks.place_title(a_title(), TimeRange::new_title(), Ticks(0));
        assert_eq!(
            tracks.clip(title).map(|clip| clip.duration()),
            Some(Ticks(5 * TICKS_PER_SECOND))
        );
    }

    #[test]
    fn titles_share_a_row_if_there_is_room_and_otherwise_open_another_above() {
        let mut tracks = Tracks::default();
        let first = tracks.place_title(a_title(), TimeRange::new_title(), Ticks(0));
        let later = tracks.place_title(
            MediaId::for_title(2),
            TimeRange::new_title(),
            Ticks(6 * TICKS_PER_SECOND),
        );
        assert_eq!(tracks.track_of(later), tracks.track_of(first));

        let over = tracks.place_title(
            MediaId::for_title(3),
            TimeRange::new_title(),
            Ticks(TICKS_PER_SECOND),
        );
        assert_eq!(tracks.track_of(over), Some(0));
        assert_eq!(tracks.track_of(first), Some(1));
        assert_eq!(tracks.clip(first).map(|clip| clip.start), Some(Ticks(0)));
    }

    #[test]
    fn a_video_never_lands_on_a_row_of_titles() {
        let mut tracks = Tracks::default();
        tracks.place_title(a_title(), TimeRange::new_title(), Ticks(0));
        let video = tracks.place(
            MediaId(1),
            seconds(0, 2),
            Ticks(0),
            TrackTarget::Existing(0),
        );
        assert_eq!(tracks.track_of(video), Some(1), "it makes a row below");
        assert!(tracks.is_text_row(0));

        let pushed = tracks.push(MediaId(2), seconds(0, 2));
        assert_eq!(tracks.track_of(pushed), tracks.track_of(video));
    }

    #[test]
    fn a_title_never_lands_on_a_row_of_pictures_or_sound() {
        let mut tracks = Tracks::default();
        let video = tracks.push(MediaId(1), seconds(0, 2));
        tracks.push_sound(MediaId(2), seconds(0, 2));
        let on_picture = tracks.place(
            a_title(),
            TimeRange::new_title(),
            Ticks(0),
            TrackTarget::Existing(0),
        );
        assert_eq!(tracks.track_of(on_picture), Some(0));
        assert_eq!(tracks.track_of(video), Some(1));

        let on_sound = tracks.place(
            MediaId::for_title(2),
            TimeRange::new_title(),
            Ticks(0),
            TrackTarget::Existing(2),
        );
        assert!(tracks.is_text_row(tracks.track_of(on_sound).expect("it landed")));
        assert!(!tracks.is_sound_row(tracks.track_of(on_sound).expect("it landed")));
    }

    #[test]
    fn a_title_can_sit_between_pictures_and_goes_behind_what_is_above_it() {
        let mut tracks = Tracks::default();
        let lower = tracks.push(MediaId(1), seconds(0, 8));
        let upper = tracks.place(MediaId(2), seconds(0, 8), Ticks(0), TrackTarget::New(0));
        let title = tracks.place(
            a_title(),
            TimeRange::new_title(),
            Ticks(0),
            TrackTarget::New(1),
        );

        assert_eq!(tracks.track_of(upper), Some(0));
        assert_eq!(tracks.track_of(title), Some(1));
        assert_eq!(tracks.track_of(lower), Some(2));
        assert!(tracks.is_text_row(1));
    }

    #[test]
    fn dragging_a_title_only_lands_it_on_rows_of_titles() {
        let mut tracks = Tracks::default();
        tracks.push(MediaId(1), seconds(0, 8));
        let title = tracks.place_title(a_title(), TimeRange::new_title(), Ticks(0));
        tracks.move_clip(title, Ticks(0), TrackTarget::Existing(1));
        assert!(tracks.is_text_row(tracks.track_of(title).expect("it is there")));
        assert_eq!(tracks.picture_rows(), 2);
    }

    #[test]
    fn the_order_of_a_video_doesnt_land_it_on_a_row_of_titles() {
        let mut tracks = Tracks::default();
        let lower = tracks.push(MediaId(1), seconds(0, 8));
        let upper = tracks.place(MediaId(2), seconds(0, 8), Ticks(0), TrackTarget::New(0));
        tracks.place_title(
            a_title(),
            TimeRange::new_title(),
            Ticks(20 * TICKS_PER_SECOND),
        );

        assert!(tracks.move_in_order(lower, OrderMove::ToFront));
        for row in 0..tracks.picture_rows() {
            if tracks.is_text_row(row) {
                assert!(
                    tracks.tracks()[row]
                        .clips()
                        .iter()
                        .all(|clip| clip.is_title())
                );
            }
        }
        assert_ne!(tracks.track_of(lower), tracks.track_of(upper));
    }

    #[test]
    fn a_row_of_titles_goes_when_its_last_title_does() {
        let mut tracks = Tracks::default();
        tracks.push(MediaId(1), seconds(0, 8));
        let title = tracks.place_title(a_title(), TimeRange::new_title(), Ticks(0));
        assert!(tracks.remove(title));
        assert_eq!(tracks.picture_rows(), 1);
        assert!(!tracks.is_text_row(0));
    }

    #[test]
    fn a_copy_placed_elsewhere_keeps_how_it_looks_and_takes_its_new_media() {
        let mut tracks = Tracks::default();
        let original = tracks.push(MediaId(1), seconds(0, 4));
        let placed = Placement {
            opacity: 500_000,
            ..Placement::default()
        };
        tracks.set_placement(original, placed);
        let clip = tracks.clip(original).expect("it is there");
        let copy = tracks.place_copy(
            clip,
            MediaId(1),
            Ticks(10 * TICKS_PER_SECOND),
            TrackTarget::Existing(0),
            false,
        );
        let copied = tracks.clip(copy).expect("it landed");
        assert_ne!(copy, original);
        assert_eq!(copied.placement, placed);
        assert_eq!(copied.source, clip.source);
        assert_eq!(copied.start, Ticks(10 * TICKS_PER_SECOND));
        assert_eq!(tracks.clip(original).map(|c| c.start), Some(Ticks(0)));
    }

    #[test]
    fn a_title_can_be_given_a_title_of_its_own() {
        let mut tracks = Tracks::default();
        let title = tracks.place_title(a_title(), TimeRange::new_title(), Ticks(0));
        let copy = tracks.duplicate(title).expect("it copied");
        assert_eq!(tracks.clips_using(a_title()), 2);
        assert!(tracks.set_media(copy, MediaId::for_title(2)));
        assert_eq!(tracks.clips_using(a_title()), 1);
        assert_eq!(tracks.clips_using(MediaId::for_title(2)), 1);
    }

    #[test]
    fn a_duration_shortens_or_lengthens_a_clip_from_its_end() {
        let mut tracks = Tracks::default();
        let clip = tracks.push(MediaId(1), seconds(0, 10));
        let footage = Ticks(20 * TICKS_PER_SECOND);

        assert!(tracks.set_duration(clip, Ticks(4 * TICKS_PER_SECOND), footage));
        let shorter = tracks.clip(clip).expect("it is there");
        assert_eq!(shorter.start, Ticks(0));
        assert_eq!(shorter.duration(), Ticks(4 * TICKS_PER_SECOND));

        assert!(tracks.set_duration(clip, Ticks(12 * TICKS_PER_SECOND), footage));
        let longer = tracks.clip(clip).expect("it is there");
        assert_eq!(longer.duration(), Ticks(12 * TICKS_PER_SECOND));
        assert_eq!(longer.source.start, Ticks(0));
    }

    #[test]
    fn a_duration_cant_run_past_the_end_of_the_footage() {
        let mut tracks = Tracks::default();
        let clip = tracks.push(MediaId(1), seconds(0, 10));
        let footage = Ticks(10 * TICKS_PER_SECOND);
        assert!(!tracks.set_duration(clip, Ticks(30 * TICKS_PER_SECOND), footage));
        assert_eq!(
            tracks.clip(clip).map(|clip| clip.duration()),
            Some(Ticks(10 * TICKS_PER_SECOND))
        );
    }

    #[test]
    fn a_longer_duration_pushes_later_clips_only_as_far_as_they_must() {
        let mut tracks = Tracks::default();
        let first = tracks.push(MediaId(1), seconds(0, 4));
        let second = tracks.place(
            MediaId(2),
            seconds(0, 4),
            Ticks(6 * TICKS_PER_SECOND),
            TrackTarget::Existing(0),
        );
        let footage = Ticks(30 * TICKS_PER_SECOND);

        assert!(tracks.set_duration(first, Ticks(5 * TICKS_PER_SECOND), footage));
        assert_eq!(
            tracks.clip(second).map(|clip| clip.start),
            Some(Ticks(6 * TICKS_PER_SECOND)),
            "it still fits"
        );
        assert!(tracks.set_duration(first, Ticks(8 * TICKS_PER_SECOND), footage));
        assert_eq!(
            tracks.clip(second).map(|clip| clip.start),
            Some(Ticks(8 * TICKS_PER_SECOND)),
            "it moves just far enough"
        );
    }

    #[test]
    fn a_duration_on_a_clip_at_another_speed_goes_by_the_timeline() {
        let mut tracks = Tracks::default();
        let clip = tracks.push(MediaId(1), seconds(0, 20));
        tracks.set_speed(clip, Speed::default().with_factor(2.0));
        assert!(tracks.set_duration(
            clip,
            Ticks(4 * TICKS_PER_SECOND),
            Ticks(20 * TICKS_PER_SECOND)
        ));
        let shorter = tracks.clip(clip).expect("it is there");
        assert_eq!(shorter.duration(), Ticks(4 * TICKS_PER_SECOND));
        assert_eq!(shorter.source.duration(), Ticks(8 * TICKS_PER_SECOND));
    }

    #[test]
    fn sound_goes_on_rows_of_its_own_below_the_pictures() {
        let mut tracks = Tracks::default();
        tracks.push(VIDEO, range(0, 100));
        let song = tracks.push_sound(SONG, range(0, 50));

        assert_eq!(tracks.picture_rows(), 1);
        assert_eq!(tracks.tracks().len(), 2);
        assert_eq!(tracks.track_of(song), Some(1));
        assert!(tracks.is_sound_row(1));
        assert!(!tracks.is_sound_row(0));
    }

    #[test]
    fn a_new_video_goes_above_rows_of_sound_and_after_the_other_pictures() {
        let mut tracks = Tracks::default();
        let song = tracks.push_sound(SONG, range(0, 500));
        let first = tracks.push(VIDEO, range(0, 100));
        let second = tracks.push(VIDEO, range(0, 100));

        assert_eq!(tracks.track_of(first), Some(0));
        assert_eq!(tracks.track_of(song), Some(1));
        assert_eq!(tracks.clip(second).map(|clip| clip.start), Some(Ticks(100)));
    }

    #[test]
    fn more_sound_goes_after_what_is_on_the_top_sound_row() {
        let mut tracks = Tracks::default();
        tracks.push(VIDEO, range(0, 500));
        tracks.push_sound(SONG, range(0, 50));
        let next = tracks.push_sound(SONG, range(0, 50));

        assert_eq!(tracks.clip(next).map(|clip| clip.start), Some(Ticks(50)));
        assert_eq!(tracks.audio_tracks().len(), 1);
    }

    #[test]
    fn a_video_cant_go_on_a_sound_row_nor_sound_on_a_picture_row() {
        let mut tracks = Tracks::default();
        let video = tracks.push(VIDEO, range(0, 100));
        let song = tracks.push_sound(SONG, range(200, 300));

        tracks.move_clip(video, Ticks(0), TrackTarget::Existing(1));
        assert_eq!(tracks.track_of(video), Some(0));

        tracks.move_clip(song, Ticks(200), TrackTarget::Existing(0));
        assert_eq!(tracks.track_of(song), Some(1));

        tracks.move_clip(video, Ticks(0), TrackTarget::New(2));
        assert!(!tracks.is_sound_row(tracks.track_of(video).expect("it is there")));
        tracks.move_clip(song, Ticks(200), TrackTarget::New(0));
        assert!(tracks.is_sound_row(tracks.track_of(song).expect("it is there")));
    }

    #[test]
    fn sound_on_a_picture_target_with_no_sound_rows_makes_one() {
        let mut tracks = Tracks::default();
        tracks.push(VIDEO, range(0, 100));

        let song = tracks.place_sound(SONG, range(0, 50), Ticks(0), TrackTarget::Existing(0));

        assert_eq!(tracks.track_of(song), Some(1));
        assert_eq!(tracks.picture_rows(), 1);
    }

    #[test]
    fn sound_never_changes_what_shows() {
        let mut tracks = Tracks::default();
        let video = tracks.push(VIDEO, range(0, 100));
        tracks.push_sound(SONG, range(0, 50));

        assert_eq!(tracks.played().clips().len(), 1);
        assert_eq!(ids(&tracks.stack_at(Ticks(10), WIDE, &all_wide)), [video]);
        assert!(!tracks.needs_layers(WIDE, &all_wide));
    }

    #[test]
    fn music_that_outlasts_the_pictures_makes_the_project_longer() {
        let mut tracks = Tracks::default();
        tracks.push(VIDEO, range(0, 100));
        assert!(!tracks.sound_outlasts_pictures());

        tracks.push_sound(SONG, range(0, 300));

        assert_eq!(tracks.duration(), Ticks(300));
        assert!(tracks.sound_outlasts_pictures());
        assert!(tracks.shows_canvas(WIDE, &all_wide));
    }

    #[test]
    fn a_stretch_with_only_sound_isnt_dead_space_but_has_no_picture() {
        let mut tracks = Tracks::default();
        tracks.place(VIDEO, range(0, 100), Ticks(200), TrackTarget::New(0));
        tracks.place_sound(SONG, range(0, 100), Ticks(50), TrackTarget::New(1));

        assert_eq!(
            tracks.gaps(),
            [
                TimeRange {
                    start: Ticks(0),
                    end: Ticks(50)
                },
                TimeRange {
                    start: Ticks(150),
                    end: Ticks(200)
                }
            ]
        );
        assert_eq!(tracks.picture_gaps().len(), 1);
    }

    #[test]
    fn removing_dead_space_moves_sound_along_with_the_pictures() {
        let mut tracks = Tracks::default();
        let video = tracks.place(VIDEO, range(0, 100), Ticks(50), TrackTarget::New(0));
        let song = tracks.place_sound(SONG, range(0, 100), Ticks(50), TrackTarget::New(1));

        assert!(tracks.remove_all_gaps());

        assert_eq!(tracks.clip(video).map(|clip| clip.start), Some(Ticks(0)));
        assert_eq!(tracks.clip(song).map(|clip| clip.start), Some(Ticks(0)));
    }

    #[test]
    fn emptied_rows_go_and_the_count_of_picture_rows_follows() {
        let mut tracks = Tracks::default();
        let lower = tracks.place(VIDEO, range(0, 100), Ticks(0), TrackTarget::New(0));
        let song = tracks.push_sound(SONG, range(0, 100));
        let upper = tracks.place(OTHER, range(0, 100), Ticks(0), TrackTarget::New(0));
        assert_eq!(tracks.picture_rows(), 2);

        tracks.remove(upper);
        assert_eq!(tracks.picture_rows(), 1);

        tracks.remove(lower);
        assert_eq!(tracks.picture_rows(), 0);
        assert_eq!(tracks.track_of(song), Some(0));
    }

    #[test]
    fn everything_on_every_row_is_heard_even_what_is_hidden() {
        let (tracks, above, below) = two_tracks();

        let spans = tracks.sound_spans(&with_sound);

        assert_eq!(spans.len(), 2);
        assert_eq!(tracks.played().clips().len(), 1);
        assert_eq!(tracks.played().clips()[0].id, above);
        assert!(
            spans
                .iter()
                .any(|span| span.media == tracks.clip(below).unwrap().media)
        );
    }

    #[test]
    fn a_muted_clip_isnt_heard() {
        let (mut tracks, above, _) = two_tracks();
        tracks.set_clip_sound(
            above,
            ClipSound {
                muted: true,
                ..ClipSound::default()
            },
        );
        assert_eq!(tracks.sound_spans(&with_sound).len(), 1);
    }

    #[test]
    fn files_with_no_sound_arent_listed() {
        let (tracks, _, _) = two_tracks();

        let spans = tracks.sound_spans(&|media| media == VIDEO);

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].media, VIDEO);
    }

    #[test]
    fn a_split_that_carries_straight_on_is_heard_as_one() {
        let mut tracks = Tracks::default();
        let first = tracks.push(VIDEO, range(0, 100));
        tracks.split(first, Ticks(40));

        let spans = tracks.sound_spans(&with_sound);

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].source, range(0, 100));
    }

    #[test]
    fn clips_that_dont_carry_straight_on_stay_apart() {
        let mut tracks = Tracks::default();
        let first = tracks.push(VIDEO, range(0, 100));
        let second = tracks.split(first, Ticks(40)).expect("it splits");

        // Moved in time, leaving a hole.
        tracks.move_clip(second, Ticks(60), TrackTarget::Existing(0));
        assert_eq!(tracks.sound_spans(&with_sound).len(), 2);

        // Back in place, but at another volume.
        tracks.move_clip(second, Ticks(40), TrackTarget::Existing(0));
        assert_eq!(tracks.sound_spans(&with_sound).len(), 1);
        tracks.set_clip_sound(
            second,
            ClipSound {
                volume: 500_000,
                ..ClipSound::default()
            },
        );
        assert_eq!(tracks.sound_spans(&with_sound).len(), 2);
    }

    #[test]
    fn a_split_keeps_the_fades_at_the_ends_they_were_for_and_the_volume_on_both() {
        let mut tracks = Tracks::default();
        let first = tracks.push(VIDEO, range(0, 100));
        let sound = ClipSound {
            volume: 500_000,
            fade_in: 10,
            fade_out: 20,
            ..ClipSound::default()
        };
        tracks.set_clip_sound(first, sound);

        let second = tracks.split(first, Ticks(40)).expect("it splits");

        let (one, two) = (sound_of(&tracks, first), sound_of(&tracks, second));
        assert_eq!((one.volume, two.volume), (500_000, 500_000));
        assert_eq!((one.fade_in, one.fade_out), (10, 0));
        assert_eq!((two.fade_in, two.fade_out), (0, 20));
    }

    #[test]
    fn a_duplicate_sounds_the_same_and_moving_keeps_it() {
        let mut tracks = Tracks::default();
        let first = tracks.push(VIDEO, range(0, 100));
        let sound = ClipSound {
            volume: 1_500_000,
            ..ClipSound::default()
        };
        tracks.set_clip_sound(first, sound);

        let copy = tracks.duplicate(first).expect("it copies");
        tracks.move_clip(copy, Ticks(500), TrackTarget::New(0));

        assert_eq!(sound_of(&tracks, copy), sound);
    }

    #[test]
    fn detaching_gives_each_sound_track_a_row_in_time_with_the_clip_and_quiets_the_clip() {
        let mut tracks = Tracks::default();
        let video = tracks.place(VIDEO, range(30, 130), Ticks(50), TrackTarget::New(0));

        let detached = tracks.detach_sound(video, 2);

        assert_eq!(detached.len(), 2);
        assert_eq!(tracks.audio_tracks().len(), 2);
        for (track, id) in detached.iter().enumerate() {
            let clip = tracks.clip(*id).expect("it is there");
            assert_eq!(clip.start, Ticks(50));
            assert_eq!(clip.source, range(30, 130));
            assert_eq!(clip.sound.only_track, Some(track as u32));
        }
        assert!(sound_of(&tracks, video).muted);
        assert_eq!(tracks.sound_spans(&with_sound).len(), 2);
    }

    #[test]
    fn detached_sound_keeps_the_clips_volume_and_fades() {
        let mut tracks = Tracks::default();
        let video = tracks.push(VIDEO, range(0, 100));
        tracks.set_clip_sound(
            video,
            ClipSound {
                volume: 700_000,
                fade_in: 5,
                ..ClipSound::default()
            },
        );

        let detached = tracks.detach_sound(video, 1)[0];

        let sound = sound_of(&tracks, detached);
        assert_eq!(
            (sound.volume, sound.fade_in, sound.muted),
            (700_000, 5, false)
        );
    }

    #[test]
    fn detached_sound_shares_a_sound_row_that_has_room_and_makes_one_that_doesnt() {
        let mut tracks = Tracks::default();
        let video = tracks.push(VIDEO, range(0, 100));
        tracks.place_sound(SONG, range(0, 50), Ticks(500), TrackTarget::New(1));
        assert_eq!(tracks.detach_sound(video, 1).len(), 1);
        assert_eq!(tracks.audio_tracks().len(), 1);

        let later = tracks.push(OTHER, range(0, 100));
        tracks.move_clip(later, Ticks(0), TrackTarget::New(0));
        tracks.detach_sound(later, 1);
        assert_eq!(tracks.audio_tracks().len(), 2);
    }

    #[test]
    fn sound_clips_and_silent_videos_have_nothing_to_detach() {
        let mut tracks = Tracks::default();
        let video = tracks.push(VIDEO, range(0, 100));
        let song = tracks.push_sound(SONG, range(0, 100));

        assert!(tracks.detach_sound(song, 2).is_empty());
        assert!(tracks.detach_sound(video, 0).is_empty());
        assert!(!sound_of(&tracks, video).muted);
    }

    #[test]
    fn sound_rows_have_no_order_to_change() {
        let mut tracks = Tracks::default();
        let lower = tracks.push_sound(SONG, range(0, 100));
        tracks.place_sound(SONG, range(0, 100), Ticks(0), TrackTarget::New(0));

        assert!(!tracks.can_move_in_order(lower, OrderMove::ToFront));
        assert!(!tracks.can_move_in_order(lower, OrderMove::ToBack));
    }

    #[test]
    fn order_moves_stay_among_the_pictures() {
        let (mut tracks, above, below) = two_tracks();
        tracks.push_sound(SONG, range(0, 100));

        assert!(tracks.move_in_order(above, OrderMove::ToBack));

        assert!(!tracks.is_sound_row(tracks.track_of(above).expect("it is there")));
        assert_eq!(tracks.track_of(below), Some(0));
    }

    #[test]
    fn sound_is_plain_only_when_each_video_plays_as_recorded_one_at_a_time() {
        let mut tracks = Tracks::default();
        let first = tracks.push(VIDEO, range(0, 100));
        tracks.push(OTHER, range(0, 100));
        assert!(tracks.sound_is_plain(&with_sound));

        tracks.set_clip_sound(
            first,
            ClipSound {
                fade_out: 10,
                ..ClipSound::default()
            },
        );
        assert!(!tracks.sound_is_plain(&with_sound));
    }

    #[test]
    fn sound_rows_and_overlaps_stop_sound_being_plain() {
        let (overlapping, _, _) = two_tracks();
        assert!(!overlapping.sound_is_plain(&with_sound));

        let mut with_music = Tracks::default();
        with_music.push(VIDEO, range(0, 100));
        with_music.push_sound(SONG, range(0, 100));
        assert!(!with_music.sound_is_plain(&with_sound));
    }

    #[test]
    fn a_silent_clip_overlapping_another_leaves_the_sound_plain() {
        let (tracks, _, _) = two_tracks();

        assert!(tracks.sound_is_plain(&|media| media == VIDEO));
    }

    #[test]
    fn splitting_with_nothing_showing_finds_the_sound_on_its_own() {
        let mut tracks = Tracks::default();
        let song = tracks.push_sound(SONG, range(0, 100));

        assert_eq!(
            tracks.clip_to_split(None, Ticks(50)).map(|clip| clip.id),
            Some(song)
        );
    }

    /// One clip on a track above another, both from 0 to 100.
    fn two_tracks() -> (Tracks, ClipId, ClipId) {
        let mut tracks = Tracks::default();
        let below = tracks.place(VIDEO, range(0, 100), Ticks(0), TrackTarget::New(0));
        let above = tracks.place(OTHER, range(0, 100), Ticks(0), TrackTarget::New(0));
        (tracks, above, below)
    }

    fn ids(stack: &[Clip]) -> Vec<ClipId> {
        stack.iter().map(|clip| clip.id).collect()
    }

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

    #[test]
    fn a_full_picture_on_top_hides_what_is_below_it() {
        let (tracks, above, _) = two_tracks();

        assert_eq!(ids(&tracks.stack_at(Ticks(5), WIDE, &all_wide)), [above]);
    }

    #[test]
    fn a_smaller_picture_on_top_shows_what_is_below_it_lowest_first() {
        let (mut tracks, above, below) = two_tracks();
        tracks.set_placement(above, smaller());

        assert_eq!(
            ids(&tracks.stack_at(Ticks(5), WIDE, &all_wide)),
            [below, above]
        );
    }

    #[test]
    fn a_see_through_picture_on_top_shows_what_is_below_it() {
        let (mut tracks, above, below) = two_tracks();
        tracks.set_placement(
            above,
            Placement {
                opacity: WHOLE / 2,
                ..Placement::default()
            },
        );

        assert_eq!(
            ids(&tracks.stack_at(Ticks(5), WIDE, &all_wide)),
            [below, above]
        );
    }

    #[test]
    fn bars_around_another_shape_show_the_track_below() {
        let (tracks, above, below) = two_tracks();
        let tall_on_top = |media: MediaId| Some(if media == OTHER { (1080, 1920) } else { WIDE });

        assert_eq!(
            ids(&tracks.stack_at(Ticks(5), WIDE, &tall_on_top)),
            [below, above]
        );
    }

    #[test]
    fn a_full_picture_in_the_middle_hides_only_what_is_below_it() {
        let mut tracks = Tracks::default();
        tracks.place(VIDEO, range(0, 100), Ticks(0), TrackTarget::New(0));
        let middle = tracks.place(VIDEO, range(0, 100), Ticks(0), TrackTarget::New(0));
        let top = tracks.place(VIDEO, range(0, 100), Ticks(0), TrackTarget::New(0));
        tracks.set_placement(top, smaller());

        assert_eq!(
            ids(&tracks.stack_at(Ticks(5), WIDE, &all_wide)),
            [middle, top]
        );
    }

    #[test]
    fn dead_space_shows_nothing() {
        let (tracks, ..) = two_tracks();

        assert!(tracks.stack_at(Ticks(100), WIDE, &all_wide).is_empty());
    }

    #[test]
    fn the_top_of_a_stack_is_the_clip_that_plays() {
        let (mut tracks, above, _) = two_tracks();
        tracks.set_placement(above, smaller());

        let stack = tracks.stack_at(Ticks(5), WIDE, &all_wide);

        assert_eq!(stack.last().map(|clip| clip.id), Some(above));
        assert_eq!(tracks.played().clips()[0].id, above);
    }

    fn faster(factor: f64) -> Speed {
        Speed::default().with_factor(factor)
    }

    #[test]
    fn another_speed_makes_a_project_take_layers_only_if_the_clip_shows() {
        let mut tracks = Tracks::default();
        let video = tracks.push(VIDEO, range(0, 100));
        assert!(!tracks.needs_layers(WIDE, &all_wide));

        tracks.set_speed(video, faster(2.0));

        assert!(tracks.needs_layers(WIDE, &all_wide));
        assert!(tracks.shows_other_speeds(WIDE, &all_wide));
    }

    #[test]
    fn slowing_a_clip_moves_later_clips_on_its_own_row_and_no_others() {
        let mut tracks = Tracks::default();
        let first = tracks.push(VIDEO, range(0, 100));
        let second = tracks.push(OTHER, range(0, 100));
        let other_row = tracks.place(OTHER, range(0, 10), Ticks(150), TrackTarget::New(0));

        tracks.set_speed(first, faster(0.5));

        assert_eq!(tracks.clip(second).map(|clip| clip.start), Some(Ticks(200)));
        assert_eq!(
            tracks.clip(other_row).map(|clip| clip.start),
            Some(Ticks(150))
        );
    }

    #[test]
    fn sound_changes_length_with_its_clip_and_takes_the_speed_with_it() {
        let mut tracks = Tracks::default();
        let video = tracks.push(VIDEO, range(0, 100));
        tracks.set_speed(video, faster(2.0));

        let spans = tracks.sound_spans(&with_sound);

        assert_eq!(spans[0].speed, faster(2.0));
        assert_eq!(spans[0].source, range(0, 100));
        assert_eq!(spans[0].end(), Ticks(50));
    }

    #[test]
    fn another_speed_means_the_sound_is_not_plain() {
        let mut tracks = Tracks::default();
        let video = tracks.push(VIDEO, range(0, 100));
        assert!(tracks.sound_is_plain(&with_sound));

        tracks.set_speed(video, faster(2.0));

        assert!(!tracks.sound_is_plain(&with_sound));
    }

    #[test]
    fn splits_at_the_same_speed_still_read_as_one_run_of_sound() {
        let mut tracks = Tracks::default();
        let video = tracks.push(VIDEO, range(0, 1000));
        tracks.set_speed(video, faster(2.0));
        tracks.split(video, Ticks(400));

        let spans = tracks.sound_spans(&with_sound);

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].source, range(0, 1000));
    }

    #[test]
    fn pieces_of_a_faster_clip_under_another_find_the_right_footage() {
        let mut tracks = Tracks::default();
        tracks.place(VIDEO, range(0, 1000), Ticks(0), TrackTarget::New(0));
        let video = tracks.picture_tracks()[0].clips()[0].id;
        tracks.set_speed(video, faster(2.0));
        tracks.place(OTHER, range(0, 100), Ticks(100), TrackTarget::New(0));

        let played = tracks.played().clips();

        assert_eq!(played[0].source, range(0, 200));
        assert_eq!(played[2].start, Ticks(200));
        assert_eq!(played[2].source, range(400, 1000));
        assert_eq!(played[2].end(), Ticks(500));
    }

    #[test]
    fn detached_sound_keeps_the_speed_and_stays_in_time_with_its_clip() {
        let mut tracks = Tracks::default();
        let video = tracks.push(VIDEO, range(0, 1000));
        tracks.set_speed(video, faster(2.0));

        let detached = tracks.detach_sound(video, 1);

        let sound = tracks.clip(detached[0]).expect("it is there");
        assert_eq!(sound.speed, faster(2.0));
        assert_eq!(sound.start, Ticks(0));
        assert_eq!(sound.end(), Ticks(500));
        assert_eq!(sound.sound.only_track, Some(0));
    }

    #[test]
    fn an_image_makes_a_project_take_layers_even_alone() {
        let mut tracks = Tracks::default();
        tracks.place(VIDEO, TimeRange::new_still(), Ticks(0), TrackTarget::New(0));

        assert!(tracks.needs_layers(WIDE, &all_wide));
        assert!(tracks.has_stills());
    }

    #[test]
    fn what_is_under_a_full_size_image_still_shows() {
        let mut tracks = Tracks::default();
        let video = tracks.place(VIDEO, range(0, 100), Ticks(0), TrackTarget::New(0));
        let still = tracks.place(OTHER, TimeRange::new_still(), Ticks(0), TrackTarget::New(0));

        let stack = tracks.stack_at(Ticks(10), WIDE, &all_wide);

        assert_eq!(ids(&stack), [video, still]);
    }

    #[test]
    fn the_video_under_an_image_is_the_one_to_decode() {
        let mut tracks = Tracks::default();
        let video = tracks.place(VIDEO, range(0, 100), Ticks(0), TrackTarget::New(0));
        let still = tracks.place(OTHER, TimeRange::new_still(), Ticks(0), TrackTarget::New(0));

        assert_eq!(tracks.played().clips()[0].id, still);
        assert_eq!(tracks.played_videos().clips().len(), 1);
        assert_eq!(tracks.played_videos().clips()[0].id, video);
    }

    #[test]
    fn an_image_counts_as_a_picture_so_it_is_not_a_picture_gap() {
        let mut tracks = Tracks::default();
        tracks.push(VIDEO, range(0, 100));
        tracks.push(OTHER, TimeRange::new_still());

        assert!(tracks.picture_gaps().is_empty());
        assert!(!tracks.sound_outlasts_pictures());
    }

    #[test]
    fn with_no_images_videos_and_pictures_are_the_same_thing() {
        let mut tracks = Tracks::default();
        tracks.push(VIDEO, range(0, 100));
        tracks.place(OTHER, range(0, 20), Ticks(10), TrackTarget::New(0));

        assert_eq!(tracks.played(), tracks.played_videos());
    }

    #[test]
    fn stretches_change_where_what_shows_changes_and_skip_dead_space() {
        let mut tracks = Tracks::default();
        let long = tracks.place(VIDEO, range(0, 100), Ticks(0), TrackTarget::New(0));
        let overlay = tracks.place(OTHER, range(0, 20), Ticks(40), TrackTarget::New(0));
        tracks.set_placement(overlay, smaller());
        let later = tracks.place(VIDEO, range(0, 10), Ticks(200), TrackTarget::Existing(1));

        let stretches = tracks.stretches(WIDE, &all_wide);

        let shape: Vec<(i64, i64, Vec<ClipId>)> = stretches
            .iter()
            .map(|stretch| (stretch.start.0, stretch.end.0, ids(&stretch.stack)))
            .collect();
        assert_eq!(
            shape,
            [
                (0, 40, vec![long]),
                (40, 60, vec![long, overlay]),
                (60, 100, vec![long]),
                (200, 210, vec![later]),
            ]
        );
    }

    #[test]
    fn the_next_change_says_when_an_overlay_arrives_and_what_shows_then() {
        let mut tracks = Tracks::default();
        let long = tracks.place(VIDEO, range(0, 100), Ticks(0), TrackTarget::New(0));
        let overlay = tracks.place(OTHER, range(0, 20), Ticks(40), TrackTarget::New(0));
        tracks.set_placement(overlay, smaller());

        let (at, stack) = tracks
            .next_change(Ticks(30), Ticks(20), WIDE, &all_wide)
            .expect("the overlay is in reach");

        assert_eq!(at, Ticks(40));
        assert_eq!(ids(&stack), [long, overlay]);
    }

    #[test]
    fn a_change_out_of_reach_is_not_mentioned() {
        let mut tracks = Tracks::default();
        tracks.place(VIDEO, range(0, 100), Ticks(0), TrackTarget::New(0));
        let overlay = tracks.place(OTHER, range(0, 20), Ticks(40), TrackTarget::New(0));
        tracks.set_placement(overlay, smaller());

        assert_eq!(
            tracks.next_change(Ticks(0), Ticks(20), WIDE, &all_wide),
            None
        );
    }

    #[test]
    fn a_clip_arriving_completely_hidden_is_no_change() {
        let mut tracks = Tracks::default();
        tracks.place(VIDEO, range(0, 100), Ticks(0), TrackTarget::New(0));
        tracks.place(OTHER, range(0, 20), Ticks(40), TrackTarget::New(1));

        assert_eq!(
            tracks.next_change(Ticks(30), Ticks(20), WIDE, &all_wide),
            None
        );
    }

    /// Three clips stacked and overlapping, from the top down.
    fn three_stacked() -> (Tracks, [ClipId; 3]) {
        let mut tracks = Tracks::default();
        let bottom = tracks.place(VIDEO, range(0, 100), Ticks(0), TrackTarget::New(0));
        let middle = tracks.place(VIDEO, range(0, 100), Ticks(0), TrackTarget::New(0));
        let top = tracks.place(VIDEO, range(0, 100), Ticks(0), TrackTarget::New(0));
        (tracks, [top, middle, bottom])
    }

    /// Which clip each track holds, from the top down.
    fn order_of(tracks: &Tracks) -> Vec<ClipId> {
        tracks
            .tracks()
            .iter()
            .map(|track| track.clips()[0].id)
            .collect()
    }

    #[test]
    fn moving_forward_goes_past_one_clip() {
        let (mut tracks, [top, middle, bottom]) = three_stacked();

        assert!(tracks.move_in_order(bottom, OrderMove::Forward));

        assert_eq!(order_of(&tracks), [top, bottom, middle]);
    }

    #[test]
    fn moving_backward_goes_past_one_clip() {
        let (mut tracks, [top, middle, bottom]) = three_stacked();

        assert!(tracks.move_in_order(top, OrderMove::Backward));

        assert_eq!(order_of(&tracks), [middle, top, bottom]);
    }

    #[test]
    fn moving_to_the_front_goes_past_everything() {
        let (mut tracks, [top, middle, bottom]) = three_stacked();

        assert!(tracks.move_in_order(bottom, OrderMove::ToFront));

        assert_eq!(order_of(&tracks), [bottom, top, middle]);
    }

    #[test]
    fn moving_to_the_back_goes_past_everything() {
        let (mut tracks, [top, middle, bottom]) = three_stacked();

        assert!(tracks.move_in_order(top, OrderMove::ToBack));

        assert_eq!(order_of(&tracks), [middle, bottom, top]);
    }

    #[test]
    fn the_clip_on_top_cant_go_further_forward() {
        let (mut tracks, [top, ..]) = three_stacked();

        assert!(!tracks.can_move_in_order(top, OrderMove::Forward));
        assert!(!tracks.can_move_in_order(top, OrderMove::ToFront));
        assert!(!tracks.move_in_order(top, OrderMove::Forward));
    }

    #[test]
    fn the_clip_at_the_bottom_cant_go_further_back() {
        let (tracks, [.., bottom]) = three_stacked();

        assert!(!tracks.can_move_in_order(bottom, OrderMove::Backward));
        assert!(!tracks.can_move_in_order(bottom, OrderMove::ToBack));
    }

    #[test]
    fn a_clip_overlapping_nothing_has_no_order_to_change() {
        let mut tracks = Tracks::default();
        tracks.place(VIDEO, range(0, 100), Ticks(0), TrackTarget::New(0));
        let apart = tracks.place(VIDEO, range(0, 100), Ticks(500), TrackTarget::New(0));

        assert!(!tracks.can_move_in_order(apart, OrderMove::Backward));
        assert!(!tracks.can_move_in_order(apart, OrderMove::ToBack));
    }

    #[test]
    fn moving_in_order_never_moves_a_clip_in_time_or_changes_its_neighbours() {
        let mut tracks = Tracks::default();
        let long = tracks.place(VIDEO, range(0, 1000), Ticks(0), TrackTarget::New(0));
        let short = tracks.place(OTHER, range(0, 100), Ticks(300), TrackTarget::New(0));
        // Above the short one, and later than it, so the track above has room for it to move to.
        let later = tracks.place(OTHER, range(0, 100), Ticks(900), TrackTarget::Existing(0));

        assert!(tracks.move_in_order(long, OrderMove::ToFront));

        assert_eq!(tracks.clip(long).map(|clip| clip.start), Some(Ticks(0)));
        assert_eq!(tracks.clip(short).map(|clip| clip.start), Some(Ticks(300)));
        assert_eq!(tracks.clip(later).map(|clip| clip.start), Some(Ticks(900)));
        assert_eq!(tracks.track_of(long), Some(0));
    }

    #[test]
    fn a_track_with_room_is_used_rather_than_a_new_one() {
        let mut tracks = Tracks::default();
        let bottom = tracks.place(VIDEO, range(0, 100), Ticks(0), TrackTarget::New(0));
        let middle = tracks.place(VIDEO, range(0, 100), Ticks(0), TrackTarget::New(0));
        // The top track has nothing at this time, so there is room there.
        tracks.place(VIDEO, range(0, 100), Ticks(500), TrackTarget::New(0));
        assert_eq!(tracks.tracks().len(), 3);

        assert!(tracks.move_in_order(bottom, OrderMove::Forward));

        assert_eq!(tracks.track_of(bottom), Some(0));
        assert_eq!(tracks.track_of(middle), Some(1));
        assert_eq!(tracks.tracks().len(), 2, "its old track was left empty");
    }

    #[test]
    fn moving_in_order_changes_which_clip_is_seen_on_top() {
        let (mut tracks, [top, _, bottom]) = three_stacked();
        assert_eq!(tracks.topmost_at(Ticks(5)).map(|clip| clip.id), Some(top));

        tracks.move_in_order(bottom, OrderMove::ToFront);

        assert_eq!(
            tracks.topmost_at(Ticks(5)).map(|clip| clip.id),
            Some(bottom)
        );
    }

    #[test]
    fn a_full_picture_with_no_dead_space_shows_no_canvas() {
        let (tracks, ..) = two_tracks();

        assert!(!tracks.shows_canvas(WIDE, &all_wide));
    }

    #[test]
    fn dead_space_shows_the_canvas() {
        let mut tracks = Tracks::default();
        tracks.place(VIDEO, range(0, 100), Ticks(50), TrackTarget::New(0));

        assert!(tracks.shows_canvas(WIDE, &all_wide));
    }

    #[test]
    fn a_smaller_lowest_picture_shows_the_canvas_around_it() {
        let mut tracks = Tracks::default();
        let clip = tracks.push(VIDEO, range(0, 100));
        assert!(!tracks.shows_canvas(WIDE, &all_wide));

        tracks.set_placement(clip, smaller());

        assert!(tracks.shows_canvas(WIDE, &all_wide));
    }

    #[test]
    fn bars_show_the_canvas() {
        let mut tracks = Tracks::default();
        tracks.push(VIDEO, range(0, 100));

        assert!(tracks.shows_canvas((1000, 1000), &all_wide));
    }

    #[test]
    fn a_plain_project_needs_no_layers_even_with_overlapping_tracks() {
        let (tracks, ..) = two_tracks();

        assert!(!tracks.needs_layers(WIDE, &all_wide));
    }

    #[test]
    fn a_resized_clip_that_shows_needs_layers() {
        let mut tracks = Tracks::default();
        let id = tracks.push(VIDEO, range(0, 100));
        assert!(!tracks.needs_layers(WIDE, &all_wide));

        tracks.set_placement(id, smaller());

        assert!(tracks.needs_layers(WIDE, &all_wide));
    }

    #[test]
    fn a_resized_clip_completely_hidden_needs_no_layers() {
        let (mut tracks, _, below) = two_tracks();

        tracks.set_placement(below, smaller());

        assert!(!tracks.needs_layers(WIDE, &all_wide));
    }

    #[test]
    fn the_canvas_is_the_largest_video_on_top() {
        let (tracks, ..) = two_tracks();
        let sizes = |media: MediaId| Some(if media == OTHER { WIDE } else { (3840, 2160) });

        assert_eq!(tracks.canvas(&sizes), Some(WIDE), "the big one is hidden");
    }

    #[test]
    fn a_larger_video_showing_under_a_smaller_one_sets_the_canvas() {
        let (mut tracks, above, _) = two_tracks();
        tracks.set_placement(above, smaller());
        let sizes = |media: MediaId| Some(if media == OTHER { WIDE } else { (3840, 2160) });

        assert_eq!(tracks.canvas(&sizes), Some((3840, 2160)));
    }

    #[test]
    fn an_empty_project_has_no_canvas() {
        assert_eq!(Tracks::default().canvas(&all_wide), None);
    }

    #[test]
    fn splitting_duplicating_and_moving_keep_a_clips_layout() {
        let mut tracks = Tracks::default();
        let id = tracks.place(VIDEO, range(0, 100), Ticks(0), TrackTarget::New(0));
        tracks.set_placement(id, smaller());

        let second = tracks.split(id, Ticks(50)).expect("it splits");
        let copy = tracks.duplicate(second).expect("it duplicates");
        tracks.move_clip(second, Ticks(500), TrackTarget::New(0));
        tracks.move_clip(copy, Ticks(900), TrackTarget::New(1));

        for clip in [id, second, copy] {
            assert_eq!(tracks.clip(clip).map(|c| c.placement), Some(smaller()));
        }
    }

    #[test]
    fn a_placement_change_says_whether_anything_changed() {
        let mut tracks = Tracks::default();
        let id = tracks.push(VIDEO, range(0, 100));

        assert!(tracks.set_placement(id, smaller()));
        assert!(!tracks.set_placement(id, smaller()));
        assert!(!tracks.set_placement(ClipId(u64::MAX), smaller()));
    }
}
