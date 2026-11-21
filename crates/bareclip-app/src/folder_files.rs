//! The files in a folder the player can open, in the order File Explorer shows them by name,
//! for moving to the next and previous file.

use std::path::{Path, PathBuf};

use crate::text;

/// Every file in a folder the player opens, sorted by name with numbers compared by value, as
/// the media panel sorts. A folder that can't be read lists nothing.
pub fn list(folder: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.path())
        .filter(|path| text::is_playable(path))
        .collect();
    files.sort_by(|a, b| text::natural_order(&name_of(a), &name_of(b)));
    files
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The file after this one that is still there, or `None` at the end. It doesn't go round to
/// the start. The file itself needn't be in the list, as when it was renamed or deleted since
/// the list was read, and then the next is whatever sorts after its name.
pub fn next_after(files: &[PathBuf], current: &Path) -> Option<PathBuf> {
    let name = name_of(current);
    files
        .iter()
        .filter(|file| text::natural_order(&name_of(file), &name).is_gt())
        .find(|file| file.exists())
        .cloned()
}

/// The file before this one that is still there, or `None` at the start.
pub fn previous_before(files: &[PathBuf], current: &Path) -> Option<PathBuf> {
    let name = name_of(current);
    files
        .iter()
        .rev()
        .filter(|file| text::natural_order(&name_of(file), &name).is_lt())
        .find(|file| file.exists())
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder_with(name: &str, files: &[&str]) -> PathBuf {
        let folder = std::env::temp_dir()
            .join("bareclip-folder-tests")
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        for file in files {
            std::fs::write(folder.join(file), b"").unwrap();
        }
        folder
    }

    fn names(files: &[PathBuf]) -> Vec<String> {
        files.iter().map(|file| name_of(file)).collect()
    }

    #[test]
    fn files_come_in_name_order_with_numbers_by_value_and_any_case() {
        let folder = folder_with(
            "order",
            &[
                "clip 10.mp4",
                "Clip 2.MKV",
                "clip 1.mp4",
                "b song.mp3",
                "A talk.wav",
                "notes.txt",
                "photo.png",
                "code.ts",
                "project.bareclip",
            ],
        );
        std::fs::create_dir(folder.join("folder.mp4")).unwrap();

        let files = list(&folder);
        assert_eq!(
            names(&files),
            [
                "A talk.wav",
                "b song.mp3",
                "clip 1.mp4",
                "Clip 2.MKV",
                "clip 10.mp4"
            ],
            "only what the player opens, never folders"
        );
    }

    #[test]
    fn next_and_previous_stop_at_the_ends_and_skip_files_that_have_gone() {
        let folder = folder_with("next", &["a.mp4", "b.mp4", "c.mp4", "d.mp4"]);
        let files = list(&folder);
        let at = |name: &str| folder.join(name);

        assert_eq!(next_after(&files, &at("a.mp4")), Some(at("b.mp4")));
        assert_eq!(previous_before(&files, &at("b.mp4")), Some(at("a.mp4")));
        assert_eq!(next_after(&files, &at("d.mp4")), None, "no going round");
        assert_eq!(previous_before(&files, &at("a.mp4")), None);

        std::fs::remove_file(at("b.mp4")).unwrap();
        assert_eq!(next_after(&files, &at("a.mp4")), Some(at("c.mp4")));
        assert_eq!(previous_before(&files, &at("c.mp4")), Some(at("a.mp4")));

        // The open file was renamed since the list was read: it goes by its old name's place.
        assert_eq!(next_after(&files, &at("bb.mp4")), Some(at("c.mp4")));
        assert_eq!(previous_before(&files, &at("bb.mp4")), Some(at("a.mp4")));
    }

    #[test]
    fn a_folder_that_cant_be_read_lists_nothing() {
        assert!(list(Path::new(r"Z:\no such folder\anywhere")).is_empty());
    }
}
