//! The project file: links to source videos and the edits made with them. It never holds
//! any video itself.

use std::{
    fmt, fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    time::Ticks,
    timeline::{MediaId, TimeRange},
    tracks::Tracks,
};

pub const EXTENSION: &str = "bareclip";
/// Goes up whenever the format changes in a way older versions can't read.
const CURRENT_VERSION: u32 = 1;
pub const DEFAULT_NAME: &str = "Untitled project";

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectFile {
    pub version: u32,
    pub name: String,
    pub media: Vec<SavedMedia>,
    /// Highest track first, each with its clips in order.
    pub tracks: Vec<Vec<SavedClip>>,
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
            tracks: tracks
                .tracks()
                .iter()
                .map(|track| {
                    track
                        .clips()
                        .iter()
                        .map(|clip| SavedClip {
                            media: clip.media.0,
                            start: clip.start.0,
                            source_start: clip.source.start.0,
                            source_end: clip.source.end.0,
                        })
                        .collect()
                })
                .collect(),
        }
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
    fn a_clip_is_cut_down_to_fit_a_shorter_video_or_dropped_if_nothing_is_left() {
        let clip = SavedClip {
            media: 1,
            start: 0,
            source_start: 100,
            source_end: 300,
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
