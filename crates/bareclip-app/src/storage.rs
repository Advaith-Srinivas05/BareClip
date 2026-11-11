//! What the app keeps between runs: its settings, and recovery copies of unsaved work.

use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io,
    path::{Path, PathBuf},
    thread::{self, JoinHandle},
    time::{SystemTime, UNIX_EPOCH},
};

use bareclip_core::{
    project::{ProjectFile, write_atomic},
    splice::CutMode,
};
use bareclip_render::GpuId;
use eframe::egui::ThemePreference;
use serde::{Deserialize, Serialize};

const SETTINGS_FILE: &str = "settings.json";
const RECOVERY_FOLDER: &str = "recovery";
const RECOVERY_EXTENSION: &str = "recovery";
const LOCK_EXTENSION: &str = "lock";
const RECORDING_FOLDER: &str = "recording";
const RECORDING_EXTENSION: &str = "recording";

/// Where the app keeps its files, or `None` if Windows doesn't say.
pub fn app_folder() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|folder| PathBuf::from(folder).join("BareClip"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeSetting {
    System,
    Light,
    Dark,
}

impl From<ThemeSetting> for ThemePreference {
    fn from(theme: ThemeSetting) -> Self {
        match theme {
            ThemeSetting::System => Self::System,
            ThemeSetting::Light => Self::Light,
            ThemeSetting::Dark => Self::Dark,
        }
    }
}

impl From<ThemePreference> for ThemeSetting {
    fn from(theme: ThemePreference) -> Self {
        match theme {
            ThemePreference::System => Self::System,
            ThemePreference::Light => Self::Light,
            ThemePreference::Dark => Self::Dark,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportCuts {
    FrameExact,
    Fastest,
}

impl From<ExportCuts> for CutMode {
    fn from(cuts: ExportCuts) -> Self {
        match cuts {
            ExportCuts::FrameExact => Self::FrameExact,
            ExportCuts::Fastest => Self::Keyframe,
        }
    }
}

/// A graphics chip picked in Settings, by its maker's numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuChoice {
    pub vendor: u32,
    pub device: u32,
}

impl From<GpuChoice> for GpuId {
    fn from(chip: GpuChoice) -> Self {
        Self {
            vendor: chip.vendor,
            device: chip.device,
        }
    }
}

impl From<GpuId> for GpuChoice {
    fn from(chip: GpuId) -> Self {
        Self {
            vendor: chip.vendor,
            device: chip.device,
        }
    }
}

/// The user's choices. Anything missing from the file takes its default, so settings written
/// by another version still load.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeSetting,
    /// Whether edits go to the project's own file without being asked.
    pub auto_save: bool,
    pub export_cuts: ExportCuts,
    /// Whether clips on picture rows show the shape of their sound. The older `waveforms` key was
    /// on by default and was written to everyone's file, so it is deliberately not read.
    pub waveforms_on_clips: bool,
    /// The same for sound rows, which holds sound files and sound detached from a video.
    pub waveforms_on_audio: bool,
    /// The graphics chip to run on. `None` is Automatic.
    pub gpu: Option<GpuChoice>,
    /// Where recordings are saved. `None` is `BareClip recordings` in Windows' Videos folder.
    pub recordings_folder: Option<PathBuf>,
    /// The microphone last recorded with, by name. `None` is Windows' default.
    pub microphone: Option<String>,
    /// Whether a recording counts down from 3 before it starts.
    pub countdown: bool,
    /// Whether the timeline plays from the playhead while recording, for a voice-over.
    pub play_while_recording: bool,
    /// What a screen recording takes besides the picture, and how.
    pub screen_pc_sound: bool,
    pub screen_microphone: bool,
    pub screen_frame_rate: u32,
    pub screen_pointer: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeSetting::System,
            auto_save: false,
            export_cuts: ExportCuts::FrameExact,
            waveforms_on_clips: false,
            waveforms_on_audio: false,
            gpu: None,
            recordings_folder: None,
            microphone: None,
            countdown: true,
            play_while_recording: true,
            screen_pc_sound: true,
            screen_microphone: false,
            screen_frame_rate: 30,
            screen_pointer: true,
        }
    }
}

impl Settings {
    /// Whether a clip on this kind of row shows the shape of its sound.
    pub fn shows_waveform(&self, on_sound_row: bool) -> bool {
        if on_sound_row {
            self.waveforms_on_audio
        } else {
            self.waveforms_on_clips
        }
    }

    /// Whether this kind of file's sound shape needs reading. A video needs it for its sound rows
    /// too, because its sound can be detached onto one at any time.
    pub fn reads_sound_shape(&self, sound_only: bool) -> bool {
        self.waveforms_on_audio || (self.waveforms_on_clips && !sound_only)
    }

    /// Reads the settings. A missing or damaged file just means the defaults.
    pub fn load(folder: &Path) -> Self {
        fs::read_to_string(folder.join(SETTINGS_FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, folder: &Path) -> io::Result<()> {
        fs::create_dir_all(folder)?;
        let text = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        write_atomic(&folder.join(SETTINGS_FILE), text.as_bytes())
    }
}

/// A recovery copy: the project, and which file it would be saved to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryRecord {
    pub saved_path: Option<PathBuf>,
    pub project: ProjectFile,
}

impl RecoveryRecord {
    pub fn read(path: &Path) -> Option<Self> {
        serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
    }
}

/// This run's recovery copy. It holds a lock for as long as the app runs, and Windows drops
/// that lock when the process ends however it ends. A copy nobody holds the lock on therefore
/// belongs to a run that never closed properly, while one from another open window is left
/// alone.
pub struct Recovery {
    path: PathBuf,
    /// Held only to keep the lock.
    lock: Option<File>,
    worker: Option<JoinHandle<io::Result<()>>>,
    problem: Option<String>,
}

fn lock_path(recovery: &Path) -> PathBuf {
    recovery.with_extension(LOCK_EXTENSION)
}

fn open_lock(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
}

impl Recovery {
    pub fn start(app_folder: &Path) -> io::Result<Self> {
        let folder = app_folder.join(RECOVERY_FOLDER);
        fs::create_dir_all(&folder)?;
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_millis());
        let name = format!("{}-{started}.{RECOVERY_EXTENSION}", std::process::id());
        let path = folder.join(name);
        let lock = open_lock(&lock_path(&path))?;
        lock.lock()?;
        Ok(Self {
            path,
            lock: Some(lock),
            worker: None,
            problem: None,
        })
    }

    /// Whether a copy is still being written.
    pub fn is_busy(&self) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
    }

    fn finish_worker(&mut self) {
        if let Some(worker) = self.worker.take()
            && let Ok(Err(error)) = worker.join()
        {
            self.problem = Some(format!("Your work could not be autosaved. {error}"));
        }
    }

    /// Writes a copy on another thread, so it can never hold up the interface.
    pub fn write(&mut self, record: &RecoveryRecord) {
        self.finish_worker();
        let path = self.path.clone();
        let Ok(text) = serde_json::to_string(record) else {
            return;
        };
        self.worker = Some(thread::spawn(move || write_atomic(&path, text.as_bytes())));
    }

    /// Removes the copy, because the work is safe or was thrown away on purpose.
    pub fn clear(&mut self) {
        self.finish_worker();
        let _ = fs::remove_file(&self.path);
    }

    /// Why the last copy could not be written, if it couldn't.
    pub fn take_problem(&mut self) -> Option<String> {
        if !self.is_busy() {
            self.finish_worker();
        }
        self.problem.take()
    }

    #[cfg(test)]
    pub fn wait(&mut self) {
        self.finish_worker();
    }

    /// Lets go of the lock without tidying up, as a crash would.
    #[cfg(test)]
    pub fn abandon(mut self) {
        self.finish_worker();
        self.lock = None;
    }
}

impl Drop for Recovery {
    fn drop(&mut self) {
        self.finish_worker();
        self.lock = None;
        let _ = fs::remove_file(lock_path(&self.path));
    }
}

/// Recovery copies left by runs that didn't close properly, newest first.
pub fn orphaned_recoveries(app_folder: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(app_folder.join(RECOVERY_FOLDER)) else {
        return Vec::new();
    };
    let mut found: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == RECOVERY_EXTENSION)
        })
        .filter(|path| {
            open_lock(&lock_path(path))
                .is_ok_and(|lock| !matches!(lock.try_lock(), Err(TryLockError::WouldBlock)))
        })
        .map(|path| {
            let modified = fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .unwrap_or(UNIX_EPOCH);
            (modified, path)
        })
        .collect();
    found.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    found.into_iter().map(|(_, path)| path).collect()
}

/// Removes a recovery copy that has been dealt with.
pub fn discard_recovery(path: &Path) {
    let _ = fs::remove_file(path);
    let _ = fs::remove_file(lock_path(path));
}

/// Says a recording is under way, for as long as this lives, so that if BareClip ends without
/// stopping it the next run can offer to add what was recorded. It holds a lock that Windows lets
/// go of however the process ends, like a recovery copy's.
pub struct RecordingMarker {
    path: PathBuf,
    lock: Option<File>,
}

impl RecordingMarker {
    pub fn start(app_folder: &Path, recording: &Path) -> io::Result<Self> {
        let folder = app_folder.join(RECORDING_FOLDER);
        fs::create_dir_all(&folder)?;
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_millis());
        let path = folder.join(format!(
            "{}-{started}.{RECORDING_EXTENSION}",
            std::process::id()
        ));
        let lock = open_lock(&lock_path(&path))?;
        lock.lock()?;
        write_atomic(&path, recording.to_string_lossy().as_bytes())?;
        Ok(Self {
            path,
            lock: Some(lock),
        })
    }

    /// The recording stopped properly, so there is nothing to offer.
    pub fn finish(mut self) {
        self.lock = None;
        discard_recovery(&self.path);
    }

    /// Lets go of the lock without tidying up, as a crash would.
    #[cfg(test)]
    pub fn abandon(mut self) {
        self.lock = None;
    }
}

impl Drop for RecordingMarker {
    fn drop(&mut self) {
        self.lock = None;
    }
}

/// A recording a run didn't stop, because it crashed or was closed mid-recording: the marker,
/// and the file it was writing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CutShort {
    pub marker: PathBuf,
    pub file: PathBuf,
}

/// Recordings left by runs that ended without stopping them, whose files are still there.
pub fn cut_short_recordings(app_folder: &Path) -> Vec<CutShort> {
    let Ok(entries) = fs::read_dir(app_folder.join(RECORDING_FOLDER)) else {
        return Vec::new();
    };
    let mut found: Vec<CutShort> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == RECORDING_EXTENSION)
        })
        .filter(|path| {
            open_lock(&lock_path(path))
                .is_ok_and(|lock| !matches!(lock.try_lock(), Err(TryLockError::WouldBlock)))
        })
        .filter_map(|marker| {
            let file = PathBuf::from(fs::read_to_string(&marker).ok()?);
            if file.is_file() {
                Some(CutShort { marker, file })
            } else {
                // Nothing was kept, so there is nothing to offer.
                discard_recovery(&marker);
                None
            }
        })
        .collect();
    found.sort_by(|a, b| a.file.cmp(&b.file));
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };

    fn temporary_folder(name: &str) -> PathBuf {
        let folder = std::env::temp_dir()
            .join("bareclip-storage-tests")
            .join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).expect("the folder can be created");
        folder
    }

    fn record(name: &str) -> RecoveryRecord {
        RecoveryRecord {
            saved_path: Some(PathBuf::from(r"D:\work\a.bareclip")),
            project: ProjectFile {
                name: name.to_owned(),
                ..ProjectFile::default()
            },
        }
    }

    #[test]
    fn settings_start_at_their_defaults_when_there_is_no_file() {
        let folder = temporary_folder("settings-none");

        assert_eq!(Settings::load(&folder), Settings::default());
    }

    #[test]
    fn settings_come_back_as_they_were_saved() {
        let folder = temporary_folder("settings-saved");
        let settings = Settings {
            theme: ThemeSetting::Light,
            auto_save: true,
            export_cuts: ExportCuts::Fastest,
            waveforms_on_clips: true,
            waveforms_on_audio: true,
            gpu: Some(GpuChoice {
                vendor: 0x1002,
                device: 0x1638,
            }),
            recordings_folder: Some(PathBuf::from(r"D:\Recordings")),
            microphone: Some("Headset microphone".to_owned()),
            countdown: false,
            play_while_recording: false,
            screen_pc_sound: false,
            screen_microphone: true,
            screen_frame_rate: 60,
            screen_pointer: false,
        };

        settings.save(&folder).expect("they can be saved");

        assert_eq!(Settings::load(&folder), settings);
    }

    #[test]
    fn a_damaged_settings_file_means_the_defaults() {
        let folder = temporary_folder("settings-damaged");
        fs::write(folder.join(SETTINGS_FILE), "not json").expect("it can be written");

        assert_eq!(Settings::load(&folder), Settings::default());
    }

    #[test]
    fn settings_missing_from_the_file_take_their_defaults() {
        let folder = temporary_folder("settings-partial");
        fs::write(folder.join(SETTINGS_FILE), r#"{"theme":"dark"}"#).expect("it can be written");

        let settings = Settings::load(&folder);

        assert_eq!(settings.theme, ThemeSetting::Dark);
        assert!(!settings.auto_save);
        assert_eq!(settings.gpu, None, "the chip starts on Automatic");
    }

    #[test]
    fn settings_from_when_autosave_had_an_interval_still_load() {
        let folder = temporary_folder("settings-old");
        fs::write(
            folder.join(SETTINGS_FILE),
            r#"{"theme":"light","autosave_seconds":30,"waveforms":false}"#,
        )
        .expect("it can be written");

        let settings = Settings::load(&folder);

        assert_eq!(settings.theme, ThemeSetting::Light);
        assert!(!settings.auto_save);
    }

    #[test]
    fn waveforms_start_off_even_for_settings_saved_with_them_on() {
        let folder = temporary_folder("settings-waveforms");
        assert!(!Settings::default().waveforms_on_clips);
        assert!(!Settings::default().waveforms_on_audio);
        fs::write(
            folder.join(SETTINGS_FILE),
            r#"{"theme":"dark","thumbnails":false,"waveforms":true}"#,
        )
        .expect("it can be written");

        let settings = Settings::load(&folder);

        assert_eq!(settings.theme, ThemeSetting::Dark);
        assert!(!settings.waveforms_on_clips);
        assert!(!settings.waveforms_on_audio);
    }

    #[test]
    fn clips_and_audio_each_have_their_own_waveform_setting() {
        let clips_only = Settings {
            waveforms_on_clips: true,
            ..Settings::default()
        };
        assert!(clips_only.shows_waveform(false));
        assert!(!clips_only.shows_waveform(true));
        assert!(clips_only.reads_sound_shape(false));
        assert!(!clips_only.reads_sound_shape(true), "no clip would show it");

        let audio_only = Settings {
            waveforms_on_audio: true,
            ..Settings::default()
        };
        assert!(!audio_only.shows_waveform(false));
        assert!(audio_only.shows_waveform(true));
        assert!(audio_only.reads_sound_shape(true));
        assert!(
            audio_only.reads_sound_shape(false),
            "a video's sound can be detached onto a sound row"
        );

        assert!(!Settings::default().reads_sound_shape(false));
        assert!(!Settings::default().reads_sound_shape(true));
    }

    #[test]
    fn a_copy_from_a_run_still_going_is_left_alone() {
        let folder = temporary_folder("live");
        let mut running = Recovery::start(&folder).expect("it can start");
        running.write(&record("in use"));
        running.wait();

        assert!(orphaned_recoveries(&folder).is_empty());
    }

    #[test]
    fn a_copy_from_a_run_that_stopped_suddenly_is_found_and_can_be_read() {
        let folder = temporary_folder("crashed");
        let mut crashed = Recovery::start(&folder).expect("it can start");
        crashed.write(&record("lost work"));
        crashed.abandon();

        let found = orphaned_recoveries(&folder);

        assert_eq!(found.len(), 1);
        let back = RecoveryRecord::read(&found[0]).expect("it can be read");
        assert_eq!(back, record("lost work"));

        discard_recovery(&found[0]);
        assert!(orphaned_recoveries(&folder).is_empty());
    }

    #[test]
    fn closing_normally_after_clearing_leaves_nothing_to_recover() {
        let folder = temporary_folder("clean");
        let mut recovery = Recovery::start(&folder).expect("it can start");
        recovery.write(&record("safe"));
        recovery.clear();
        drop(recovery);

        assert!(orphaned_recoveries(&folder).is_empty());
        let left = fs::read_dir(folder.join(RECOVERY_FOLDER)).expect("it can be listed");
        assert_eq!(left.count(), 0);
    }

    #[test]
    fn a_run_with_nothing_written_leaves_nothing_to_recover() {
        let folder = temporary_folder("nothing");
        Recovery::start(&folder).expect("it can start").abandon();

        assert!(orphaned_recoveries(&folder).is_empty());
    }

    #[test]
    fn the_newest_copy_comes_first() {
        let folder = temporary_folder("order");
        for name in ["older", "newer"] {
            let mut crashed = Recovery::start(&folder).expect("it can start");
            crashed.write(&record(name));
            crashed.abandon();
            std::thread::sleep(Duration::from_millis(30));
        }

        let found = orphaned_recoveries(&folder);

        let first = RecoveryRecord::read(&found[0]).expect("it can be read");
        assert_eq!(first.project.name, "newer");
    }

    #[test]
    fn a_failed_write_is_reported() {
        let folder = temporary_folder("fails");
        let mut recovery = Recovery::start(&folder).expect("it can start");
        // A folder where the temporary file has to go makes the write fail.
        let mut temporary = recovery.path.clone().into_os_string();
        temporary.push(".tmp");
        fs::create_dir(temporary).expect("the folder can be created");

        recovery.write(&record("x"));
        recovery.wait();

        assert!(recovery.take_problem().is_some());
        assert!(recovery.take_problem().is_none());
    }

    #[test]
    fn a_recording_stopped_properly_is_not_offered() {
        let folder = temporary_folder("recording-stopped");
        let file = folder.join("Microphone.m4a");
        fs::write(&file, "sound").expect("written");
        let marker = RecordingMarker::start(&folder, &file).expect("it starts");
        assert!(cut_short_recordings(&folder).is_empty(), "still recording");
        marker.finish();
        assert!(cut_short_recordings(&folder).is_empty());
        assert!(file.exists(), "the recording itself is never touched");
    }

    #[test]
    fn a_recording_a_run_never_stopped_is_offered_once_its_lock_is_gone() {
        let folder = temporary_folder("recording-cut-short");
        let file = folder.join("Microphone.m4a");
        fs::write(&file, "sound").expect("written");
        RecordingMarker::start(&folder, &file)
            .expect("it starts")
            .abandon();
        let found = cut_short_recordings(&folder);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].file, file);
        discard_recovery(&found[0].marker);
        assert!(cut_short_recordings(&folder).is_empty());
    }

    #[test]
    fn a_cut_short_recording_with_no_file_is_forgotten() {
        let folder = temporary_folder("recording-no-file");
        RecordingMarker::start(&folder, &folder.join("never made.m4a"))
            .expect("it starts")
            .abandon();
        assert!(cut_short_recordings(&folder).is_empty());
        let left = fs::read_dir(folder.join(RECORDING_FOLDER))
            .expect("the folder is there")
            .flatten()
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == RECORDING_EXTENSION)
            })
            .count();
        assert_eq!(left, 0, "the marker was tidied away");
    }

    const HOLDER_FOLDER: &str = "BARECLIP_RECOVERY_HOLDER";

    /// Not a test itself: `killing_the_process_releases_its_copy` runs it as another process.
    #[test]
    #[ignore = "only runs as a helper of another test"]
    fn hold_a_recovery_copy_until_killed() {
        let Some(folder) = std::env::var_os(HOLDER_FOLDER) else {
            return;
        };
        let mut recovery = Recovery::start(Path::new(&folder)).expect("it can start");
        recovery.write(&record("from another process"));
        recovery.wait();
        fs::write(Path::new(&folder).join("ready"), "").expect("it can be written");
        std::thread::sleep(Duration::from_secs(60));
    }

    #[test]
    fn killing_the_process_releases_its_copy() {
        let folder = temporary_folder("killed");
        let mut holder = Command::new(std::env::current_exe().expect("the test knows its path"))
            .args([
                "--ignored",
                "--exact",
                "storage::tests::hold_a_recovery_copy_until_killed",
            ])
            .env(HOLDER_FOLDER, &folder)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the helper starts");
        let started = Instant::now();
        while !folder.join("ready").exists() {
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "it never got ready"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            orphaned_recoveries(&folder).is_empty(),
            "the copy belongs to a process that is still running"
        );

        holder.kill().expect("the helper can be killed");
        holder.wait().expect("the helper ends");

        let found = orphaned_recoveries(&folder);
        assert_eq!(found.len(), 1);
        let back = RecoveryRecord::read(&found[0]).expect("it can be read");
        assert_eq!(back.project.name, "from another process");
    }
}
