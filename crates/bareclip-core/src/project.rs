//! The project file: links to source videos and the edits made with them. It never holds
//! any video itself.

use std::{
    fmt, fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    canvas::Canvas,
    placement::Placement,
    sound::ClipSound,
    speed::Speed,
    time::Ticks,
    timeline::{MediaId, TimeRange, Timeline},
    tracks::Tracks,
};

pub const EXTENSION: &str = "bareclip";
/// Goes up whenever the format changes in a way older versions can't read.
const CURRENT_VERSION: u32 = 5;
/// Files older than this had their size worked out from the largest video.
const FIRST_VERSION_WITH_CANVAS: u32 = 3;
pub const DEFAULT_NAME: &str = "untitled";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedMedia {
    pub id: u64,
    pub path: PathBuf,
}

/// A clip as it is written down. Clips get fresh ids when loaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedClip {
    pub media: u64,
    pub start: i64,
    pub source_start: i64,
    pub source_end: i64,
    /// Left out for a clip that hasn't been moved or resized. Files from before layers have
    /// none, which means the same thing.
    #[serde(default, skip_serializing_if = "Placement::is_default")]
    pub placement: Placement,
    /// Left out for a clip that plays as recorded. Files from before audio have none, which
    /// means the same thing.
    #[serde(default, skip_serializing_if = "ClipSound::is_default")]
    pub sound: ClipSound,
    /// Left out for a clip that plays as recorded. Files from before speed have none, which means
    /// the same thing.
    #[serde(default, skip_serializing_if = "Speed::is_default")]
    pub speed: Speed,
}

impl SavedClip {
    pub fn source(self) -> TimeRange {
        TimeRange {
            start: Ticks(self.source_start),
            end: Ticks(self.source_end),
        }
    }

    /// The clip with its footage cut down to what a source video has, in case the file
    /// was replaced by a shorter one. `None` if that leaves nothing of it.
    pub fn fitted_to(self, duration: Ticks) -> Option<Self> {
        let source_end = self.source_end.min(duration.0);
        (self.source_start < source_end).then_some(Self { source_end, ..self })
    }
}

/// What an older file says about a whole row. Solo is gone, so a file with it on opens with
/// everything heard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct OldRowSound {
    pub muted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectFile {
    pub version: u32,
    pub name: String,
    pub media: Vec<SavedMedia>,
    /// Highest track first, each with its clips in order.
    pub tracks: Vec<Vec<SavedClip>>,
    /// The rows of sound below those, from the top down.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audio_tracks: Vec<Vec<SavedClip>>,
    /// How older files said each row sounded, picture rows first. Only ever read, because rows
    /// no longer have sound settings of their own. Use `muted_rows`.
    #[serde(default, skip_serializing)]
    pub row_sound: Vec<OldRowSound>,
    #[serde(default)]
    pub canvas: Canvas,
}

impl Default for ProjectFile {
    fn default() -> Self {
        Self::capture(DEFAULT_NAME, Vec::new(), &Tracks::default())
    }
}

/// What can go wrong reading a project.
#[derive(Debug)]
pub enum ProjectError {
    Io(io::Error),
    /// It isn't a project file, or part of it is missing.
    Damaged(String),
    /// It was saved by a later version of BareClip than this one.
    TooNew(u32),
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "It could not be read: {error}"),
            Self::Damaged(_) => write!(f, "It is damaged, or isn't a BareClip project."),
            Self::TooNew(_) => write!(f, "It was saved by a newer version of BareClip."),
        }
    }
}

impl ProjectFile {
    pub fn capture(name: &str, media: Vec<(MediaId, PathBuf)>, tracks: &Tracks) -> Self {
        Self {
            version: CURRENT_VERSION,
            name: name.to_owned(),
            media: media
                .into_iter()
                .map(|(id, path)| SavedMedia { id: id.0, path })
                .collect(),
            tracks: saved_rows(tracks.picture_tracks()),
            audio_tracks: saved_rows(tracks.audio_tracks()),
            row_sound: Vec::new(),
            canvas: Canvas::default(),
        }
    }

    /// Which rows this file muted as a whole, if it is old enough to have done that.
    pub fn muted_rows(&self) -> Vec<bool> {
        self.row_sound.iter().map(|row| row.muted).collect()
    }

    /// Whether this file was saved before projects kept their own size.
    pub fn predates_canvas(&self) -> bool {
        self.version < FIRST_VERSION_WITH_CANVAS
    }

    pub fn to_text(&self) -> String {
        // Nothing here can fail to serialise: it has no maps or non-string keys.
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn from_text(text: &str) -> Result<Self, ProjectError> {
        // The version comes first, so a newer format gets a clear message rather than
        // being called damaged.
        #[derive(Deserialize)]
        struct Version {
            version: u32,
        }
        let damaged = |error: serde_json::Error| ProjectError::Damaged(error.to_string());
        let Version { version } = serde_json::from_str(text).map_err(damaged)?;
        if version > CURRENT_VERSION {
            return Err(ProjectError::TooNew(version));
        }
        serde_json::from_str(text).map_err(damaged)
    }

    pub fn read(path: &Path) -> Result<Self, ProjectError> {
        Self::from_text(&fs::read_to_string(path).map_err(ProjectError::Io)?)
    }
}

fn saved_rows(rows: &[Timeline]) -> Vec<Vec<SavedClip>> {
    rows.iter()
        .map(|track| {
            track
                .clips()
                .iter()
                .map(|clip| SavedClip {
                    media: clip.media.0,
                    start: clip.start.0,
                    source_start: clip.source.start.0,
                    source_end: clip.source.end.0,
                    placement: clip.placement,
                    sound: clip.sound,
                    speed: clip.speed,
                })
                .collect()
        })
        .collect()
}

/// Replaces a file in one step: the new contents go in a temporary file beside it, which is
/// then swapped in. A crash at any moment leaves either the old file or the new one whole.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    let written = fs::File::create(&temporary).and_then(|mut file| {
        file.write_all(contents)?;
        file.sync_all()
    });
    let result = written.and_then(|()| fs::rename(&temporary, path));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracks::TrackTarget;

    const VIDEO: MediaId = MediaId(7);
    const OTHER: MediaId = MediaId(9);

    fn range(start: i64, end: i64) -> TimeRange {
        TimeRange {
            start: Ticks(start),
            end: Ticks(end),
        }
    }

    fn stacked() -> Tracks {
        let mut tracks = Tracks::default();
        tracks.place(VIDEO, range(10, 110), Ticks(0), TrackTarget::New(0));
        tracks.place(VIDEO, range(200, 250), Ticks(150), TrackTarget::Existing(0));
        tracks.place(OTHER, range(0, 30), Ticks(40), TrackTarget::New(0));
        tracks
    }

    fn file() -> ProjectFile {
        ProjectFile::capture(
            "My project",
            vec![
                (VIDEO, PathBuf::from(r"D:\videos\game.mkv")),
                (OTHER, PathBuf::from(r"D:\videos\मेरा वीडियो.mp4")),
            ],
            &stacked(),
        )
    }

    fn temporary_folder(name: &str) -> PathBuf {
        let folder = std::env::temp_dir()
            .join("bareclip-core-tests")
            .join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).expect("the folder can be created");
        folder
    }

    #[test]
    fn a_project_survives_being_written_and_read_back() {
        let file = file();

        let back = ProjectFile::from_text(&file.to_text()).expect("it can be read");

        assert_eq!(back, file);
    }

    #[test]
    fn restoring_tracks_gives_the_same_arrangement_with_new_clip_ids() {
        let original = stacked();
        let file = ProjectFile::capture("x", Vec::new(), &original);

        let restored = Tracks::restore(&file.tracks, |id| Some(MediaId(id)));

        assert_eq!(
            ProjectFile::capture("x", Vec::new(), &restored).tracks,
            file.tracks
        );
        assert_eq!(
            restored.played().clips().len(),
            original.played().clips().len()
        );
        let first = original.played().clips()[0].id;
        assert!(restored.clip(first).is_none());
    }

    #[test]
    fn restoring_leaves_out_clips_of_videos_that_are_gone_and_their_empty_tracks() {
        let file = file();

        let restored = Tracks::restore(&file.tracks, |id| (id == VIDEO.0).then_some(MediaId(id)));

        assert_eq!(restored.tracks().len(), 1);
        assert_eq!(restored.clips_using(VIDEO), 2);
        assert_eq!(restored.clips_using(OTHER), 0);
    }

    #[test]
    fn restoring_a_hand_edited_file_still_follows_the_timeline_rules() {
        let clip = |start, source_end| SavedClip {
            media: 1,
            start,
            source_start: 0,
            source_end,
            placement: Placement::default(),
            sound: Default::default(),
            speed: Default::default(),
        };
        // Out of order, overlapping, empty and starting before zero.
        let tracks = vec![vec![clip(50, 100), clip(0, 100), clip(5, 0), clip(-20, 10)]];

        let restored = Tracks::restore(&tracks, |id| Some(MediaId(id)));

        let clips = restored.tracks()[0].clips();
        assert_eq!(clips.len(), 3);
        for pair in clips.windows(2) {
            assert!(pair[0].end() <= pair[1].start, "{pair:?}");
        }
        assert!(clips[0].start >= Ticks(0));
    }

    #[test]
    fn a_projects_size_survives_being_written_and_read_back() {
        let mut project = file();
        project.canvas = Canvas {
            original: Some((1920, 992)),
            ratio: Some((9, 16)),
            background: [10, 20, 30],
        };

        let back = ProjectFile::from_text(&project.to_text()).expect("it can be read");

        assert_eq!(back.canvas, project.canvas);
        assert!(!back.predates_canvas());
    }

    #[test]
    fn a_project_from_before_sizes_says_so_and_has_none() {
        let text = r#"{"version": 2, "name": "old", "media": [], "tracks": []}"#;

        let file = ProjectFile::from_text(text).expect("it can be read");

        assert!(file.predates_canvas());
        assert_eq!(file.canvas, Canvas::default());
    }

    #[test]
    fn a_project_from_before_layers_still_opens_with_nothing_moved() {
        let text = r#"{
            "version": 1,
            "name": "old",
            "media": [{"id": 1, "path": "a.mp4"}],
            "tracks": [[{"media": 1, "start": 0, "source_start": 0, "source_end": 100}]]
        }"#;

        let file = ProjectFile::from_text(text).expect("it can be read");
        let tracks = Tracks::restore(&file.tracks, |id| Some(MediaId(id)));

        assert!(tracks.tracks()[0].clips()[0].placement.is_default());
    }

    #[test]
    fn a_layout_survives_being_written_and_read_back() {
        let mut tracks = stacked();
        let id = tracks.tracks()[0].clips()[0].id;
        let placement = Placement {
            center_x: 250_000,
            center_y: 750_000,
            scale: 400_000,
            opacity: 600_000,
            ..Placement::default()
        };
        tracks.set_placement(id, placement);
        let file = ProjectFile::capture("x", Vec::new(), &tracks);

        let back = ProjectFile::from_text(&file.to_text()).expect("it can be read");
        let restored = Tracks::restore(&back.tracks, |id| Some(MediaId(id)));

        assert_eq!(restored.tracks()[0].clips()[0].placement, placement);
    }

    #[test]
    fn sound_rows_and_clip_sound_all_come_back() {
        let mut tracks = Tracks::default();
        let video = tracks.push(VIDEO, range(0, 100));
        tracks.set_clip_sound(
            video,
            ClipSound {
                volume: 750_000,
                fade_in: 10,
                fade_out: 20,
                ..ClipSound::default()
            },
        );
        let detached = tracks.detach_sound(video, 1)[0];
        let file = ProjectFile::capture("x", Vec::new(), &tracks);

        let back = ProjectFile::from_text(&file.to_text()).expect("it can be read");
        let restored = Tracks::restore_with_sound(
            &back.tracks,
            &back.audio_tracks,
            &back.muted_rows(),
            |id| Some(MediaId(id)),
        );

        assert_eq!(restored.picture_rows(), 1);
        assert_eq!(restored.audio_tracks().len(), 1);
        let saved = restored.tracks()[1].clips()[0].sound;
        assert_eq!(saved, tracks.clip(detached).expect("it is there").sound);
        assert_eq!(saved.only_track, Some(0));
        assert_eq!(saved.volume, 750_000);
    }

    #[test]
    fn a_project_from_before_audio_opens_and_plays_as_it_did() {
        let text = r#"{"version":3,"name":"old","media":[{"id":1,"path":"a.mp4"}],
            "tracks":[[{"media":1,"start":0,"source_start":0,"source_end":100}]]}"#;

        let file = ProjectFile::from_text(text).expect("it can be read");
        let tracks = Tracks::restore_with_sound(
            &file.tracks,
            &file.audio_tracks,
            &file.muted_rows(),
            |id| Some(MediaId(id)),
        );

        assert!(tracks.audio_tracks().is_empty());
        assert!(tracks.tracks()[0].clips()[0].sound.is_default());
    }

    #[test]
    fn a_row_an_older_file_muted_comes_back_as_muted_clips_and_solo_is_forgotten() {
        let text = r#"{"version":5,"name":"old","media":[{"id":1,"path":"a.mp4"}],
            "tracks":[
                [{"media":1,"start":0,"source_start":0,"source_end":100}],
                [{"media":1,"start":0,"source_start":0,"source_end":100}]
            ],
            "row_sound":[{"muted":true,"solo":false},{"muted":false,"solo":true}]}"#;

        let file = ProjectFile::from_text(text).expect("it can be read");
        let tracks = Tracks::restore_with_sound(
            &file.tracks,
            &file.audio_tracks,
            &file.muted_rows(),
            |id| Some(MediaId(id)),
        );

        assert!(tracks.tracks()[0].clips()[0].sound.muted);
        assert!(!tracks.tracks()[1].clips()[0].sound.muted);
        assert_eq!(tracks.sound_spans(&|_| true).len(), 1);
        let saved_again = ProjectFile::capture("old", Vec::new(), &tracks).to_text();
        assert!(!saved_again.contains("row_sound"));
    }

    #[test]
    fn a_project_with_no_sound_changes_writes_nothing_about_sound() {
        let text = file().to_text();

        for word in ["sound", "audio_tracks", "row_sound"] {
            assert!(!text.contains(word), "{word} is in {text}");
        }
    }

    #[test]
    fn a_clip_that_was_not_moved_writes_nothing_about_its_layout() {
        assert!(!file().to_text().contains("placement"));
    }

    #[test]
    fn a_hand_edited_layout_comes_back_in_range() {
        let clip = SavedClip {
            media: 1,
            start: 0,
            source_start: 0,
            source_end: 100,
            placement: Placement {
                scale: 0,
                opacity: -5,
                ..Placement::default()
            },
            sound: Default::default(),
            speed: Default::default(),
        };

        let tracks = Tracks::restore(&[vec![clip]], |id| Some(MediaId(id)));

        let placement = tracks.tracks()[0].clips()[0].placement;
        assert!(placement.scale > 0);
        assert_eq!(placement.opacity, 0);
    }

    #[test]
    fn a_clip_is_cut_down_to_fit_a_shorter_video_or_dropped_if_nothing_is_left() {
        let clip = SavedClip {
            media: 1,
            start: 0,
            source_start: 100,
            source_end: 300,
            placement: Placement::default(),
            sound: Default::default(),
            speed: Default::default(),
        };

        assert_eq!(clip.fitted_to(Ticks(500)), Some(clip));
        assert_eq!(clip.fitted_to(Ticks(200)).map(|c| c.source_end), Some(200));
        assert_eq!(clip.fitted_to(Ticks(100)), None);
    }

    #[test]
    fn something_that_is_not_a_project_is_called_damaged() {
        for text in ["", "not json", "{}", r#"{"version":1}"#, "[1,2,3]"] {
            assert!(
                matches!(ProjectFile::from_text(text), Err(ProjectError::Damaged(_))),
                "{text}"
            );
        }
    }

    #[test]
    fn a_project_from_a_newer_version_says_so() {
        let text = r#"{"version":999,"something":"new"}"#;

        assert!(matches!(
            ProjectFile::from_text(text),
            Err(ProjectError::TooNew(999))
        ));
    }

    #[test]
    fn writing_replaces_the_whole_file_and_leaves_nothing_else_behind() {
        let folder = temporary_folder("atomic");
        let path = folder.join("a.bareclip");

        write_atomic(&path, b"a much longer first version").expect("it can be written");
        write_atomic(&path, b"second").expect("it can be replaced");

        assert_eq!(fs::read(&path).expect("it can be read"), b"second");
        assert_eq!(fs::read_dir(&folder).expect("it can be listed").count(), 1);
    }

    #[test]
    fn a_failed_write_leaves_the_old_file_alone() {
        let folder = temporary_folder("atomic-fail");
        let path = folder.join("a.bareclip");
        write_atomic(&path, b"old").expect("it can be written");
        // A folder where the temporary file has to go makes the write fail.
        fs::create_dir(folder.join("a.bareclip.tmp")).expect("the folder can be created");

        assert!(write_atomic(&path, b"new").is_err());

        assert_eq!(fs::read(&path).expect("it can be read"), b"old");
    }

    #[test]
    fn reading_a_missing_file_says_it_could_not_be_read() {
        let folder = temporary_folder("missing");

        let result = ProjectFile::read(&folder.join("nothing.bareclip"));

        assert!(matches!(result, Err(ProjectError::Io(_))));
    }
}
