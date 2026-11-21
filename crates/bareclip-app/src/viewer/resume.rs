//! Where each file was stopped, so it picks up there next time.
//!
//! A file is known by its path, size and Modified date, so a file changed or replaced since
//! starts again from the beginning. Only a spot more than 10 seconds in and more than 10
//! seconds before the end is kept, and only the 200 most recently stopped files.

use std::{
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use bareclip_core::{project::write_atomic, time::Ticks};
use serde::{Deserialize, Serialize};

const RESUME_FILE: &str = "viewer-resume.json";
/// How far in a spot must be, and how far from the end, to be kept.
pub const RESUME_MARGIN_SECONDS: f64 = 10.0;
/// The most files remembered.
pub const MOST_REMEMBERED: usize = 200;

/// One file's spot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Spot {
    path: PathBuf,
    size: u64,
    /// When Windows says the file last changed, in nanoseconds since 1970.
    modified: u128,
    /// Where it was stopped, in the file's own time.
    at: i64,
}

/// What identifies a file's version: its size and Modified date.
fn identity(path: &Path) -> Option<(u64, u128)> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((metadata.len(), modified))
}

/// Every remembered spot, the most recently stopped last.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeSpots {
    spots: Vec<Spot>,
}

impl ResumeSpots {
    /// Reads the spots. A missing or damaged file means none.
    pub fn load(folder: &Path) -> Self {
        std::fs::read_to_string(folder.join(RESUME_FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, folder: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(folder)?;
        let text = serde_json::to_string(self).map_err(std::io::Error::other)?;
        write_atomic(&folder.join(RESUME_FILE), text.as_bytes())
    }

    /// Where to pick this file up, if it was stopped partway and hasn't changed since.
    pub fn spot_for(&self, path: &Path) -> Option<Ticks> {
        let (size, modified) = identity(path)?;
        self.spots
            .iter()
            .find(|spot| spot.path == path && spot.size == size && spot.modified == modified)
            .map(|spot| Ticks(spot.at))
    }

    /// Remembers where a file was stopped, as the newest, or forgets it if that was near either
    /// end.
    pub fn remember(&mut self, path: &Path, at: Ticks, duration: Ticks) {
        self.spots.retain(|spot| spot.path != path);
        let margin = Ticks::from_seconds(RESUME_MARGIN_SECONDS);
        let partway = at.0 > margin.0 && at.0 < duration.0 - margin.0;
        let Some((size, modified)) = identity(path).filter(|_| partway) else {
            return;
        };
        self.spots.push(Spot {
            path: path.to_owned(),
            size,
            modified,
            at: at.0,
        });
        if self.spots.len() > MOST_REMEMBERED {
            let extra = self.spots.len() - MOST_REMEMBERED;
            self.spots.drain(..extra);
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.spots.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let folder = std::env::temp_dir()
            .join("bareclip-resume-tests")
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    fn file_in(folder: &Path, name: &str) -> PathBuf {
        let path = folder.join(name);
        std::fs::write(&path, b"some video").unwrap();
        path
    }

    fn seconds(value: f64) -> Ticks {
        Ticks::from_seconds(value)
    }

    #[test]
    fn only_a_spot_more_than_10_seconds_from_either_end_is_kept() {
        let folder = folder("limits");
        let video = file_in(&folder, "a.mp4");
        let mut spots = ResumeSpots::default();
        let length = seconds(60.0);

        spots.remember(&video, seconds(9.9), length);
        assert_eq!(spots.spot_for(&video), None, "too near the start");
        spots.remember(&video, seconds(50.1), length);
        assert_eq!(spots.spot_for(&video), None, "too near the end");
        spots.remember(&video, seconds(10.1), length);
        assert_eq!(spots.spot_for(&video), Some(seconds(10.1)));
        spots.remember(&video, seconds(49.9), length);
        assert_eq!(spots.spot_for(&video), Some(seconds(49.9)));

        spots.remember(&video, seconds(59.0), length);
        assert_eq!(
            spots.spot_for(&video),
            None,
            "watching to the end forgets it"
        );
        assert_eq!(spots.len(), 0);
    }

    #[test]
    fn a_changed_file_starts_again() {
        let folder = folder("changed");
        let video = file_in(&folder, "a.mp4");
        let mut spots = ResumeSpots::default();
        spots.remember(&video, seconds(30.0), seconds(60.0));
        assert!(spots.spot_for(&video).is_some());

        std::fs::write(&video, b"a different, longer video").unwrap();
        assert_eq!(spots.spot_for(&video), None);
        assert_eq!(spots.spot_for(&folder.join("gone.mp4")), None);
    }

    #[test]
    fn only_the_200_most_recently_stopped_files_are_kept() {
        let folder = folder("many");
        let mut spots = ResumeSpots::default();
        let files: Vec<PathBuf> = (0..205)
            .map(|index| file_in(&folder, &format!("{index}.mp4")))
            .collect();
        for file in &files {
            spots.remember(file, seconds(20.0), seconds(60.0));
        }
        assert_eq!(spots.len(), MOST_REMEMBERED);
        assert_eq!(spots.spot_for(&files[4]), None, "the oldest went");
        assert!(spots.spot_for(&files[5]).is_some());

        // Stopping an old one again makes it the newest.
        spots.remember(&files[5], seconds(25.0), seconds(60.0));
        let more = file_in(&folder, "more.mp4");
        spots.remember(&more, seconds(20.0), seconds(60.0));
        assert_eq!(spots.spot_for(&files[5]), Some(seconds(25.0)));
        assert_eq!(spots.spot_for(&files[6]), None);
    }

    #[test]
    fn spots_are_saved_and_a_broken_file_is_ignored() {
        let folder = folder("saved");
        let video = file_in(&folder, "a.mp4");
        let mut spots = ResumeSpots::default();
        spots.remember(&video, seconds(30.0), seconds(60.0));
        spots.save(&folder).unwrap();
        assert_eq!(
            ResumeSpots::load(&folder).spot_for(&video),
            Some(seconds(30.0))
        );

        std::fs::write(folder.join(RESUME_FILE), b"{ not json").unwrap();
        assert_eq!(ResumeSpots::load(&folder), ResumeSpots::default());
    }
}
