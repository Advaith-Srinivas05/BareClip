use super::*;

fn folder(name: &str) -> PathBuf {
    let folder = std::env::temp_dir()
        .join("bareclip-file-tests")
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    folder
}

fn file_in(folder: &Path, name: &str, contents: &[u8]) -> PathBuf {
    let path = folder.join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

fn date(text: &str) -> DateTime {
    DateTime::parse(text).unwrap()
}

#[test]
fn a_rename_keeps_the_extension_and_refuses_a_name_already_used() {
    let folder = folder("rename");
    let clip = file_in(&folder, "clip.mp4", b"a");
    file_in(&folder, "taken.mp4", b"b");

    let renamed = rename(&clip, "  Holiday 2025 ").unwrap();
    assert_eq!(renamed, folder.join("Holiday 2025.mp4"));
    assert!(renamed.exists() && !clip.exists());

    let clash = rename(&renamed, "taken").unwrap_err();
    assert!(clash.contains("already a file called taken.mp4"), "{clash}");
    assert!(renamed.exists(), "the old name stays");
    assert_eq!(std::fs::read(folder.join("taken.mp4")).unwrap(), b"b");

    let cased = rename(&renamed, "HOLIDAY 2025").unwrap();
    assert_eq!(
        cased.file_name().unwrap(),
        "HOLIDAY 2025.mp4",
        "case alone can change"
    );
}

#[test]
fn names_windows_wont_take_say_why_and_change_nothing() {
    let folder = folder("bad-names");
    let clip = file_in(&folder, "clip.mp4", b"a");
    for (name, says) in [
        ("a:b", ":"),
        ("   ", "at least one"),
        ("", "at least one"),
        ("what?", "?"),
        ("ends.", "end with a dot"),
        ("con", "keeps the name"),
        ("LPT1", "keeps the name"),
    ] {
        let problem = rename(&clip, name).unwrap_err();
        assert!(problem.contains(says), "{name}: {problem}");
        assert!(clip.exists());
    }
    assert_eq!(
        name_problem("con game"),
        None,
        "only the whole name is kept"
    );
}

#[test]
fn created_and_modified_dates_are_set_and_read_back_to_the_second() {
    let folder = folder("dates");
    let clip = file_in(&folder, "clip.mp4", b"a");
    let created = from_local(date("2021-03-04 05:06:07")).unwrap();
    let modified = from_local(date("2022-08-09 10:11:12")).unwrap();

    set_file_times(&clip, Some(created), Some(modified)).unwrap();
    let (read_created, read_modified) = file_times(&clip).unwrap();
    assert_eq!(local_parts(read_created), date("2021-03-04 05:06:07"));
    assert_eq!(local_parts(read_modified), date("2022-08-09 10:11:12"));

    let later = from_local(date("2023-01-01 00:00:00")).unwrap();
    set_file_times(&clip, None, Some(later)).unwrap();
    let (kept, changed) = file_times(&clip).unwrap();
    assert_eq!(
        local_parts(kept),
        date("2021-03-04 05:06:07"),
        "None leaves it"
    );
    assert_eq!(local_parts(changed), date("2023-01-01 00:00:00"));
}

#[test]
fn local_and_utc_times_go_there_and_back() {
    let moment = from_utc(date("2025-09-14 01:35:00")).unwrap();
    assert_eq!(utc_parts(moment), date("2025-09-14 01:35:00"));
    assert_eq!(from_local(local_parts(moment)), Some(moment));
}

#[test]
fn a_swapped_in_copy_keeps_the_originals_created_date() {
    let folder = folder("swap");
    let original = file_in(&folder, "clip.mp4", b"old");
    let created = from_local(date("2020-05-05 05:05:05")).unwrap();
    set_file_times(&original, Some(created), None).unwrap();
    let copy = copy_path_for(&original);
    assert_eq!(
        copy.extension().unwrap(),
        "mp4",
        "it is written as the same kind"
    );
    std::fs::write(&copy, b"new").unwrap();

    swap_in(&original, &copy).unwrap();
    assert_eq!(std::fs::read(&original).unwrap(), b"new");
    assert!(!copy.exists());
    let (kept, _) = file_times(&original).unwrap();
    assert_eq!(local_parts(kept), date("2020-05-05 05:05:05"));
}

#[test]
fn free_space_is_known_for_a_real_drive() {
    assert!(free_space(&std::env::temp_dir()).is_some_and(|free| free > 0));
}

/// Where the Recycle Bin keeps a file deleted from this path: its record and the file itself.
fn find_in_recycle_bin(original: &Path) -> Option<(PathBuf, PathBuf)> {
    let drive = original.components().next()?;
    let bin = PathBuf::from(format!(
        "{}\\$Recycle.Bin",
        drive.as_os_str().to_string_lossy()
    ));
    for user in std::fs::read_dir(bin).ok()?.filter_map(Result::ok) {
        let Ok(entries) = std::fs::read_dir(user.path()) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(rest) = name.strip_prefix("$I") else {
                continue;
            };
            let Ok(record) = std::fs::read(entry.path()) else {
                continue;
            };
            // Version 2 records keep the name's length at 24 and the name from 28.
            let units: Vec<u16> = record
                .get(28..)
                .unwrap_or_default()
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .take_while(|unit| *unit != 0)
                .collect();
            let path = String::from_utf16_lossy(&units);
            if path.eq_ignore_ascii_case(&original.to_string_lossy()) {
                return Some((entry.path(), user.path().join(format!("$R{rest}"))));
            }
        }
    }
    None
}

#[test]
fn a_deleted_file_goes_to_the_recycle_bin_and_can_come_back() {
    let folder = folder("recycle");
    let clip = file_in(&folder, "deleted clip.mp4", b"keep me");

    move_to_recycle_bin(&clip).unwrap();
    assert!(!clip.exists());
    let (record, kept) = find_in_recycle_bin(&clip).expect("it is in the Recycle Bin");
    assert_eq!(std::fs::read(&kept).unwrap(), b"keep me", "nothing is lost");

    // Put it back, so the test leaves nothing in the Recycle Bin.
    std::fs::rename(&kept, &clip).unwrap();
    std::fs::remove_file(record).unwrap();
    assert_eq!(std::fs::read(&clip).unwrap(), b"keep me");
    assert!(move_to_recycle_bin(&folder.join("gone.mp4")).is_err());
}
