//! The editing screen: a media panel, the preview, and the timeline below them.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, Receiver},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime},
};

use bareclip_core::{
    history::History,
    project::{self, ProjectFile, write_atomic},
    splice::{CutMode, FrameIndex},
    time::{Ticks, format_timecode},
    timeline::{ClipEdge, ClipId, MediaId, TimeRange},
    tracks::{TrackTarget, Tracks},
};
use bareclip_media::{
    DecodeStep, ExportPart, ExportReport, ExportRequest, MediaError, MediaInfo, PreviewFrame,
    SoundShape, StripPicture, VideoPreview, can_write_gaps, export, join_problem, picture_spacing,
    plan_export, probe, read_sound_shape, read_strip_pictures, scan_frames,
};
use eframe::egui::{
    self, Align, Color32, CornerRadius, FontFamily, FontId, Frame, Key, Layout, Margin, Modifiers,
    RichText, Sense, Stroke, TextureHandle, TextureOptions, UiBuilder, pos2, vec2,
};

use crate::{
    components::{self, ActionButton, CheckBox, IconButton, Kind, MenuItem},
    fonts, icons,
    preview::Preview,
    storage::{self, ExportCuts, Recovery, RecoveryRecord, Settings},
    text,
    theme::{self, Palette},
    timeline::{self, ClipFace, ClipView, MediaDrag, MenuAction, TimelineState, TimelineView},
};

const VIDEO_EXTENSIONS: [&str; 6] = ["mp4", "mkv", "mov", "webm", "avi", "ts"];
const LOGO_DARK: &[u8] = include_bytes!("../../../assets/brand/png/mark-dark-256.png");
const LOGO_LIGHT: &[u8] = include_bytes!("../../../assets/brand/png/mark-light-256.png");
// The brand rules call for the simplified mark at this size.
const SMALL_LOGO_DARK: &[u8] = include_bytes!("../../../assets/brand/png/mark-small-dark-256.png");
const SMALL_LOGO_LIGHT: &[u8] =
    include_bytes!("../../../assets/brand/png/mark-small-light-256.png");
/// How far the arrow keys and the buttons beside play move the playhead.
const SKIP_SECONDS: f64 = 5.0;
const BACKGROUND_TICK: Duration = Duration::from_millis(50);
/// How often recovery copies are written, and how long a change waits before Auto Save writes
/// it to the project's file.
const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(5);
const AUTO_SAVE_HELD: &str =
    "Auto Save is paused because parts of this project couldn't be opened. Save it to resume.";
const MIN_CLIP_SECONDS: f64 = 0.05;
const THUMBNAIL_WIDTH: u32 = 160;
const THUMBNAIL_HEIGHT: u32 = 90;
/// The pictures along clips: at most one this often, and at most this many per video, so a
/// long recording costs no more than about 8 MB and 10 seconds in the background.
const STRIP_PICTURE_SIZE: (u32, u32) = (128, 72);
const STRIP_SHORTEST_SPACING_SECONDS: f64 = 2.0;
const STRIP_MOST_PICTURES: u32 = 240;
/// How many pictures get handed to the GPU per frame, so a burst of them can't cause a stutter.
const STRIP_UPLOADS_PER_FRAME: usize = 4;
/// A reader holds this many pictures ready before it waits for the interface to catch up.
const STRIP_CHANNEL: usize = 8;
const RAIL_WIDTH: f32 = 78.0;
const MEDIA_PANEL_WIDTH: f32 = 290.0;
const MEDIA_PANEL_MIN_WIDTH: f32 = 240.0;
const MEDIA_PANEL_MAX_WIDTH: f32 = 720.0;
/// Each card is at least this wide, so a wider panel fits more of them across.
const MEDIA_CARD_MIN_WIDTH: f32 = 220.0;
const MEDIA_CARD_SPACING: f32 = 10.0;
const CARD_BUTTON_SIZE: f32 = 28.0;
/// Cards squeezed side by side have room for smaller buttons, and only name their video.
const COMPACT_CARD_BUTTON_SIZE: f32 = 20.0;
const CARD_BUTTON_MARGIN: f32 = 6.0;
const COMPACT_CARD_BUTTON_MARGIN: f32 = 3.0;
const CARD_INNER_MARGIN: i8 = 8;
/// The list's own row of controls needs this much room for Sort and Filter as well as the
/// buttons for ticked videos.
const MEDIA_ROW_ROOM_FOR_ALL: f32 = 250.0;
const CARD_RADIUS: u8 = 8;
const CARD_NAME_HEIGHT: f32 = 18.0;
const CARD_ROW_SPACING: f32 = 2.0;
const HDR_WARNING: &str = "HDR video: the preview shows it without HDR.";
const CARD_TICK_SIZE: f32 = 14.0;
const CARD_TICK_ROOM: f32 = 20.0;
const SELECT_BOX_MARGIN: f32 = 6.0;
const PANEL_CLOSE_SIZE: f32 = 26.0;
/// The panels are rounded cards, with this much of the background showing between them.
const PANEL_GAP: i8 = 8;
const PANEL_RADIUS: u8 = 10;
const STAGE_MARGIN: i8 = 8;
/// The strip across the top of the window, standing in for the Windows title bar.
const TITLE_STRIP_HEIGHT: f32 = 40.0;
const WINDOW_BUTTON_SIZE: f32 = 32.0;
const PROJECT_NAME_WIDTH: f32 = 220.0;
const SETTINGS_ROW_GAP: f32 = 14.0;
const STRIP_LOGO_HEIGHT: f32 = 22.0;
/// How close to the window's edge the pointer has to be to resize it, and how far along
/// from a corner still counts as that corner.
const RESIZE_EDGE: f32 = 5.0;
const RESIZE_CORNER: f32 = 14.0;
const TRANSPORT_HEIGHT: f32 = 52.0;
/// How far above and below the line over the transport row the timeline's height can be
/// dragged from.
const TIMELINE_RESIZE_GRAB: f32 = 4.0;
const TIMELINE_SLIDE_SECONDS: f32 = 0.2;
/// The room a timeline tool takes: its button and the space after it. A tool with nothing
/// to act on leaves this empty, so the others don't shift under the pointer.
const TOOL_SLOT: f32 = 36.0;
const TIME_READOUT_WIDTH: f32 = 170.0;
const EMPTY_HINT_MAX_HEIGHT: f32 = 200.0;
const DIALOG_RADIUS: u8 = 10;
/// A dialog's own buttons are never narrower than this, so short names still feel solid.
const DIALOG_BUTTON_WIDTH: f32 = 84.0;
/// The playback buttons: four of 32, the play button, and the spaces between them.
const TRANSPORT_WIDTH: f32 = 180.0;
const CONTROL_SPACING: f32 = 4.0;
const CARD_TOP_ROW_HEIGHT: f32 = 48.0;
/// These include the transport row.
const TIMELINE_HEIGHT: f32 = 330.0;
const TIMELINE_MIN_HEIGHT: f32 = 230.0;
/// Whatever the timeline's height, the preview keeps this share of the room.
const TIMELINE_MAX_SHARE: f32 = 0.65;
/// How far the arrow keys step in dead space, which has no frames of its own.
const GAP_STEP_SECONDS: f64 = 1.0 / 30.0;

/// What the reader of a video's pictures and sound sends back.
enum StripMessage {
    Picture(StripPicture),
    PicturesDone,
    Sound(SoundShape),
}

/// Reads a video's pictures and sound shape in the background, for showing on its clips.
struct StripReader {
    messages: Receiver<StripMessage>,
    cancel: Arc<AtomicBool>,
}

impl Drop for StripReader {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn start_strip_reader(
    ctx: &egui::Context,
    info: &MediaInfo,
    want_pictures: bool,
    want_sound: bool,
) -> StripReader {
    let (sender, messages) = mpsc::sync_channel(STRIP_CHANNEL);
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    let path = info.path.clone();
    let duration = info.duration;
    let spacing = picture_spacing(
        duration,
        Ticks::from_seconds(STRIP_SHORTEST_SPACING_SECONDS),
        STRIP_MOST_PICTURES,
    );
    let repaint = ctx.clone();
    thread::spawn(move || {
        // Sound first, since it takes a fraction of the time. Anything that goes wrong only
        // costs the look of the clip, so there is nothing to report.
        if want_sound && let Ok(Some(shape)) = read_sound_shape(&path, &worker_cancel, |_| {}) {
            let _ = sender.send(StripMessage::Sound(shape));
            repaint.request_repaint();
        }
        if want_pictures {
            let read = read_strip_pictures(
                &path,
                duration,
                spacing,
                STRIP_PICTURE_SIZE,
                &worker_cancel,
                |picture| {
                    if sender.send(StripMessage::Picture(picture)).is_ok() {
                        repaint.request_repaint();
                    } else {
                        worker_cancel.store(true, Ordering::Relaxed);
                    }
                },
            );
            if read.is_ok() && !worker_cancel.load(Ordering::Relaxed) {
                let _ = sender.send(StripMessage::PicturesDone);
                repaint.request_repaint();
            }
        }
    });
    StripReader { messages, cancel }
}

/// What a video's background reading sends back.
enum ScanMessage {
    Thumbnail(PreviewFrame),
    Frames(Result<FrameIndex, MediaError>),
}

/// What the videos in the panel are ordered by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MediaSort {
    Name,
    DateAdded,
    DateCreated,
    UsedInVideo,
}

impl MediaSort {
    const ALL: [(Self, &'static str); 4] = [
        (Self::Name, "Name"),
        (Self::DateAdded, "Date added"),
        (Self::DateCreated, "Date created"),
        (Self::UsedInVideo, "Used in video"),
    ];
}

/// Which videos the panel lists. Videos are the only kind of media so far, so when images
/// or audio arrive their types belong here too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MediaFilter {
    All,
    OnTimeline,
    NotOnTimeline,
}

impl MediaFilter {
    const ALL: [(Self, &'static str); 3] = [
        (Self::All, "All videos"),
        (Self::OnTimeline, "On the timeline"),
        (Self::NotOnTimeline, "Not on the timeline"),
    ];

    fn keeps(self, used: bool) -> bool {
        match self {
            Self::All => true,
            Self::OnTimeline => used,
            Self::NotOnTimeline => !used,
        }
    }
}

/// A video in the project.
struct MediaEntry {
    id: MediaId,
    info: MediaInfo,
    name: String,
    /// When the file was created, on file systems that record it.
    created: Option<SystemTime>,
    index: Option<Arc<FrameIndex>>,
    scan: Option<Receiver<ScanMessage>>,
    cancel_scan: Arc<AtomicBool>,
    progress: Arc<AtomicU32>,
    thumbnail: Option<TextureHandle>,
    /// What its clips show, as far as it has been read, with pictures in order of time.
    strip_pictures: Vec<(Ticks, TextureHandle)>,
    strip_pictures_done: bool,
    sound_shape: Option<SoundShape>,
    strip_reader: Option<StripReader>,
}

impl Drop for MediaEntry {
    fn drop(&mut self) {
        self.cancel_scan.store(true, Ordering::Relaxed);
    }
}

/// The parts of the project that undo and redo bring back.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Snapshot {
    tracks: Tracks,
    /// Videos removed from the panel. They stay loaded so undo can restore them.
    hidden: Vec<MediaId>,
}

/// A trim in progress: where the clip's edge and the playhead were when the drag started.
#[derive(Clone, Copy, Debug)]
struct TrimOrigin {
    source: Ticks,
    pointer: Ticks,
    playhead: Ticks,
}

struct ExportTask {
    result: Receiver<Result<ExportReport, MediaError>>,
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    output: PathBuf,
}

enum ExportState {
    Closed,
    /// The timeline has dead space on it, which the user may not have meant to export.
    GapWarning,
    Choosing,
    Running(ExportTask),
    Done {
        report: ExportReport,
        output: PathBuf,
    },
    Failed(String),
}

pub struct Editor {
    media: Vec<MediaEntry>,
    next_media: u64,
    project: Snapshot,
    history: History<Snapshot>,
    preview: Preview,
    picture: Option<TextureHandle>,
    selected: Option<ClipId>,
    /// A moment in the dead space that was clicked on.
    selected_gap: Option<Ticks>,
    timeline_state: TimelineState,
    trim_origin: Option<TrimOrigin>,
    notice: Option<String>,
    export_state: ExportState,
    mode: CutMode,
    confirm_remove: Vec<MediaId>,
    project_name: String,
    /// The file this project saves to, once it has one.
    project_path: Option<PathBuf>,
    /// The project as it last was on disk, which unsaved changes are measured against.
    baseline: ProjectFile,
    leave_for: Option<LeaveFor>,
    missing_videos: Option<MissingVideos>,
    /// The name being typed, while the project is being renamed.
    renaming: Option<String>,
    window_title: String,
    closing: bool,
    settings: Settings,
    settings_open: bool,
    /// Where settings and recovery copies live. Without it, the app keeps nothing.
    app_folder: Option<PathBuf>,
    recovery: Option<Recovery>,
    last_autosaved: Option<ProjectFile>,
    last_autosave_at: Instant,
    /// When the project first differed from what its file holds.
    unsaved_since: Option<Instant>,
    /// Auto Save's write to the project's file, while it is under way.
    auto_save_write: Option<AutoSaveWrite>,
    /// What Auto Save last failed to write, so it only tries again once something changes.
    auto_save_failed: Option<ProjectFile>,
    /// Parts of the file couldn't be opened, so writing now would drop them from it. Auto Save
    /// waits until the user saves on purpose.
    auto_save_held: bool,
    /// Recovery copies from runs that never closed properly, waiting to be offered.
    recovery_offers: Vec<PathBuf>,
    /// Videos ticked in the panel, in the order they were ticked.
    selected_media: Vec<MediaId>,
    media_panel_open: bool,
    media_sort: MediaSort,
    media_sort_descending: bool,
    media_filter: MediaFilter,
    /// The timeline's height when open, including the transport row.
    timeline_height: f32,
    timeline_open: bool,
    /// The logo on the empty stage, and which theme it was loaded for.
    logo: Option<(bool, TextureHandle)>,
    /// The logo in the top strip, and the theme and shrinking it was loaded with.
    strip_logo: Option<(bool, usize, TextureHandle)>,
    loaded_fallbacks: Vec<&'static str>,
    /// Where things were drawn in the last frame.
    media_panel_rect: egui::Rect,
    timeline_panel_rect: egui::Rect,
    card_thumbnails: Vec<(MediaId, egui::Rect)>,
}

/// Something that would lose unsaved work, waiting on the user's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
enum LeaveFor {
    NewProject,
    OpenPicked,
    Open(PathBuf),
    CloseWindow,
}

/// Auto Save writing the project to its file on another thread.
struct AutoSaveWrite {
    file: ProjectFile,
    worker: JoinHandle<std::io::Result<()>>,
}

/// A project that can't finish opening until the user deals with videos that have moved.
struct MissingVideos {
    project: ProjectFile,
    path: Option<PathBuf>,
    /// The recovery copy this came from. It stays on disk until the work is back in the
    /// editor, so backing out of this dialog can't lose it.
    restored_from: Option<PathBuf>,
    missing: Vec<usize>,
}

/// How a media card relates to the timeline and to what is ticked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CardState {
    /// A clip of this video is on the timeline.
    used: bool,
    /// The selected clip came from this video.
    linked: bool,
    /// Where this video comes in the ticked ones, counting from 1.
    order: Option<usize>,
}

/// What was pressed on the media cards this frame.
#[derive(Default)]
struct CardActions {
    add: Option<MediaId>,
    remove: Option<MediaId>,
    toggle: Option<MediaId>,
}

fn media_info(media: &[MediaEntry], id: MediaId) -> Option<MediaInfo> {
    media
        .iter()
        .find(|entry| entry.id == id)
        .map(|entry| entry.info.clone())
}

impl Editor {
    pub fn new(ctx: &egui::Context, use_sound: bool) -> Self {
        let repaint = ctx.clone();
        Self {
            media: Vec::new(),
            next_media: 1,
            project: Snapshot {
                tracks: Tracks::default(),
                hidden: Vec::new(),
            },
            history: History::default(),
            preview: Preview::new(use_sound, Arc::new(move || repaint.request_repaint())),
            picture: None,
            selected: None,
            selected_gap: None,
            timeline_state: TimelineState::default(),
            trim_origin: None,
            notice: None,
            export_state: ExportState::Closed,
            mode: CutMode::FrameExact,
            confirm_remove: Vec::new(),
            project_name: project::DEFAULT_NAME.to_owned(),
            project_path: None,
            baseline: ProjectFile::default(),
            leave_for: None,
            missing_videos: None,
            renaming: None,
            window_title: String::new(),
            closing: false,
            settings: Settings::default(),
            settings_open: false,
            app_folder: None,
            recovery: None,
            last_autosaved: None,
            last_autosave_at: Instant::now(),
            unsaved_since: None,
            auto_save_write: None,
            auto_save_failed: None,
            auto_save_held: false,
            recovery_offers: Vec::new(),
            selected_media: Vec::new(),
            media_panel_open: true,
            media_sort: MediaSort::Name,
            media_sort_descending: false,
            media_filter: MediaFilter::All,
            timeline_height: TIMELINE_HEIGHT,
            timeline_open: true,
            logo: None,
            strip_logo: None,
            loaded_fallbacks: Vec::new(),
            media_panel_rect: egui::Rect::NOTHING,
            timeline_panel_rect: egui::Rect::NOTHING,
            card_thumbnails: Vec::new(),
        }
    }

    /// Starts keeping settings and recovery copies in a folder, and looks for work left
    /// behind by a run that never closed properly.
    pub fn use_storage(&mut self, ctx: &egui::Context, folder: PathBuf) {
        self.settings = Settings::load(&folder);
        ctx.set_theme(egui::ThemePreference::from(self.settings.theme));
        self.mode = self.settings.export_cuts.into();
        self.recovery_offers = storage::orphaned_recoveries(&folder);
        match Recovery::start(&folder) {
            Ok(recovery) => self.recovery = Some(recovery),
            Err(error) => {
                self.notice = Some(format!("Your work can't be autosaved. {error}"));
            }
        }
        self.app_folder = Some(folder);
    }

    fn save_settings(&mut self) {
        if let Some(folder) = &self.app_folder
            && let Err(error) = self.settings.save(folder)
        {
            self.notice = Some(format!("Your settings could not be saved. {error}"));
        }
    }

    /// The project as it would be written down right now.
    fn current_file(&self) -> ProjectFile {
        let media = self
            .visible_media()
            .map(|entry| {
                let path = &entry.info.path;
                let absolute = std::path::absolute(path).unwrap_or_else(|_| path.clone());
                (entry.id, absolute)
            })
            .collect();
        ProjectFile::capture(&self.project_name, media, &self.project.tracks)
    }

    fn has_unsaved_changes(&self) -> bool {
        self.current_file() != self.baseline
    }

    /// Does something that would replace the project, asking first if that would lose work.
    fn leave_project(&mut self, ctx: &egui::Context, reason: LeaveFor) {
        self.flush_auto_save();
        if self.has_unsaved_changes() {
            self.leave_for = Some(reason);
        } else {
            self.carry_out(ctx, reason);
        }
    }

    fn carry_out(&mut self, ctx: &egui::Context, reason: LeaveFor) {
        self.leave_for = None;
        match reason {
            LeaveFor::NewProject => self.start_new_project(),
            LeaveFor::OpenPicked => {
                let picked = rfd::FileDialog::new()
                    .set_title("Open project")
                    .add_filter("BareClip project", &[project::EXTENSION])
                    .pick_file();
                if let Some(path) = picked {
                    self.open_project(ctx, &path);
                }
            }
            LeaveFor::Open(path) => self.open_project(ctx, &path),
            LeaveFor::CloseWindow => {
                if let Some(recovery) = &mut self.recovery {
                    recovery.clear();
                }
                self.closing = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    /// Clears everything down to an empty project.
    fn start_new_project(&mut self) {
        self.wait_for_auto_save();
        self.preview.pause(self.project.tracks.played());
        self.media.clear();
        self.project = Snapshot {
            tracks: Tracks::default(),
            hidden: Vec::new(),
        };
        self.history = History::default();
        self.selected = None;
        self.selected_gap = None;
        self.selected_media.clear();
        self.confirm_remove.clear();
        self.picture = None;
        self.card_thumbnails.clear();
        self.notice = None;
        self.renaming = None;
        let (height_kept, keep) = (self.timeline_height, self.timeline_open);
        self.timeline_state = TimelineState::default();
        self.timeline_height = height_kept;
        self.timeline_open = keep;
        self.seek(Ticks(0));
        self.project_name = project::DEFAULT_NAME.to_owned();
        self.project_path = None;
        self.baseline = self.current_file();
        self.last_autosaved = None;
        self.unsaved_since = None;
        self.auto_save_failed = None;
        self.auto_save_held = false;
        if let Some(recovery) = &mut self.recovery {
            recovery.clear();
        }
    }

    /// Saves to the project's file, asking where if it doesn't have one yet.
    fn save(&mut self) -> bool {
        match self.project_path.clone() {
            Some(path) => self.save_to(&path),
            None => self.save_as(),
        }
    }

    fn save_as(&mut self) -> bool {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Save project")
            .set_file_name(format!("{}.{}", self.project_name, project::EXTENSION))
            .add_filter("BareClip project", &[project::EXTENSION]);
        if let Some(folder) = self.project_path.as_deref().and_then(Path::parent) {
            dialog = dialog.set_directory(folder);
        }
        dialog.save_file().is_some_and(|path| self.save_to(&path))
    }

    fn save_to(&mut self, path: &Path) -> bool {
        self.wait_for_auto_save();
        let mut path = path.to_owned();
        if path.extension().is_none_or(|e| e != project::EXTENSION) {
            let mut named = path.into_os_string();
            named.push(".");
            named.push(project::EXTENSION);
            path = PathBuf::from(named);
        }
        // An unnamed project takes the name the user just gave its file.
        if self.project_name == project::DEFAULT_NAME
            && let Some(stem) = path.file_stem()
        {
            self.project_name = stem.to_string_lossy().into_owned();
        }
        let file = self.current_file();
        match write_atomic(&path, file.to_text().as_bytes()) {
            Ok(()) => {
                self.baseline = file;
                self.project_path = Some(path);
                self.last_autosaved = None;
                self.unsaved_since = None;
                self.auto_save_failed = None;
                self.auto_save_held = false;
                self.notice = None;
                if let Some(recovery) = &mut self.recovery {
                    recovery.clear();
                }
                true
            }
            Err(error) => {
                self.notice = Some(format!("The project could not be saved. {error}"));
                false
            }
        }
    }

    /// Opens a project file. What is open now stays open if this one can't be read.
    fn open_project(&mut self, ctx: &egui::Context, path: &Path) {
        match ProjectFile::read(path) {
            Ok(file) => self.begin_loading(ctx, file, Some(path.to_owned()), None),
            Err(error) => {
                let name = path.file_name().map_or_else(
                    || "That project".to_owned(),
                    |name| name.to_string_lossy().into_owned(),
                );
                self.notice = Some(format!("{name} could not be opened. {error}"));
            }
        }
    }

    /// Starts opening a project, stopping to ask about any video that has moved. A video
    /// that is simply beside the project file is found without asking, so a folder holding
    /// both can be moved as a whole.
    fn begin_loading(
        &mut self,
        ctx: &egui::Context,
        mut file: ProjectFile,
        path: Option<PathBuf>,
        restored_from: Option<PathBuf>,
    ) {
        let folder = path.as_deref().and_then(Path::parent).map(Path::to_owned);
        let mut missing = Vec::new();
        for (index, media) in file.media.iter_mut().enumerate() {
            if media.path.exists() {
                continue;
            }
            let beside = folder.as_deref().zip(media.path.file_name());
            match beside.map(|(folder, name)| folder.join(name)) {
                Some(found) if found.exists() => media.path = found,
                _ => missing.push(index),
            }
        }
        let waiting = MissingVideos {
            project: file,
            path,
            restored_from,
            missing,
        };
        if waiting.missing.is_empty() {
            self.finish_loading(ctx, waiting);
        } else {
            self.missing_videos = Some(waiting);
        }
    }

    /// Replaces the project with a loaded one, leaving out videos still missing.
    fn finish_loading(&mut self, ctx: &egui::Context, waiting: MissingVideos) {
        let MissingVideos {
            project: file,
            path,
            restored_from,
            missing,
        } = waiting;
        self.start_new_project();
        let mut new_ids = std::collections::HashMap::new();
        let mut durations = std::collections::HashMap::new();
        let mut unreadable = !missing.is_empty();
        for (index, media) in file.media.iter().enumerate() {
            if missing.contains(&index) {
                continue;
            }
            match self.load(ctx, &media.path) {
                Ok(entry) => {
                    new_ids.insert(media.id, entry.id);
                    durations.insert(media.id, entry.info.duration);
                    self.media.push(entry);
                }
                Err(error) => {
                    unreadable = true;
                    let name = media.path.display();
                    self.notice = Some(format!("{name} could not be opened. {error}"));
                }
            }
        }
        let fitted: Vec<Vec<project::SavedClip>> = file
            .tracks
            .iter()
            .map(|track| {
                track
                    .iter()
                    .filter_map(|clip| clip.fitted_to(*durations.get(&clip.media)?))
                    .collect()
            })
            .collect();
        self.project.tracks = Tracks::restore(&fitted, |id| new_ids.get(&id).copied());
        self.project_name = file.name;
        self.project_path = path;
        self.media_panel_open = true;
        self.auto_save_held = unreadable;
        if unreadable && self.settings.auto_save {
            self.notice = Some(match self.notice.take() {
                Some(problem) => format!("{problem} {AUTO_SAVE_HELD}"),
                None => AUTO_SAVE_HELD.to_owned(),
            });
        }
        // Work brought back from a crash, or missing parts of the file, is not what is on
        // disk, so it counts as unsaved until the user saves it.
        if restored_from.is_some() || unreadable {
            self.baseline = ProjectFile::default();
        } else {
            self.baseline = self.current_file();
        }
        self.timeline_state
            .content_added(self.project.tracks.duration());
        self.seek(Ticks(0));
        if let Some(old_copy) = restored_from {
            // This run's own copy takes over from the old one.
            let record = RecoveryRecord {
                saved_path: self.project_path.clone(),
                project: self.current_file(),
            };
            if let Some(recovery) = &mut self.recovery {
                recovery.write(&record);
            }
            self.last_autosaved = Some(record.project);
            self.last_autosave_at = Instant::now();
            storage::discard_recovery(&old_copy);
        }
    }

    /// Keeps unsaved work safe: Auto Save takes it to the project's file if that is on and the
    /// project has one, and a recovery copy holds it until then. The first change after a quiet
    /// spell goes to the copy straight away, and waits a while before touching the file, so a
    /// mistake can be undone before it gets there.
    fn autosave(&mut self, ctx: &egui::Context) {
        self.collect_auto_save(ctx);
        if self.recovery.is_none() && !self.settings.auto_save {
            return;
        }
        let file = self.current_file();
        let unsaved = file != self.baseline;
        if unsaved {
            self.auto_save_to_file(ctx, &file);
        } else {
            self.unsaved_since = None;
        }
        let saved_path = self.project_path.clone();
        let since = self.last_autosave_at.elapsed();
        let Some(recovery) = &mut self.recovery else {
            return;
        };
        if let Some(problem) = recovery.take_problem() {
            self.notice = Some(problem);
        }
        if !unsaved {
            if self.last_autosaved.take().is_some() {
                recovery.clear();
            }
            return;
        }
        if recovery.is_busy() || self.last_autosaved.as_ref() == Some(&file) {
            return;
        }
        if since < AUTOSAVE_INTERVAL {
            ctx.request_repaint_after(AUTOSAVE_INTERVAL - since);
            return;
        }
        recovery.write(&RecoveryRecord {
            saved_path,
            project: file.clone(),
        });
        self.last_autosaved = Some(file);
        self.last_autosave_at = Instant::now();
    }

    /// The file Auto Save would write to right now, if it would.
    fn auto_save_path(&self) -> Option<&Path> {
        if self.settings.auto_save && !self.auto_save_held {
            self.project_path.as_deref()
        } else {
            None
        }
    }

    /// Starts writing the project to its file on another thread once a change has waited long
    /// enough, so a slow drive can't hold up the interface.
    fn auto_save_to_file(&mut self, ctx: &egui::Context, file: &ProjectFile) {
        let unsaved_for = self
            .unsaved_since
            .get_or_insert_with(Instant::now)
            .elapsed();
        let Some(path) = self.auto_save_path().map(Path::to_owned) else {
            return;
        };
        if self.auto_save_write.is_some() || self.auto_save_failed.as_ref() == Some(file) {
            return;
        }
        if unsaved_for < AUTOSAVE_INTERVAL {
            ctx.request_repaint_after(AUTOSAVE_INTERVAL - unsaved_for);
            return;
        }
        let text = file.to_text();
        self.auto_save_write = Some(AutoSaveWrite {
            file: file.clone(),
            worker: thread::spawn(move || write_atomic(&path, text.as_bytes())),
        });
    }

    /// Takes in the result of Auto Save's write if it has finished.
    fn collect_auto_save(&mut self, ctx: &egui::Context) {
        if self
            .auto_save_write
            .as_ref()
            .is_some_and(|write| !write.worker.is_finished())
        {
            ctx.request_repaint_after(BACKGROUND_TICK);
        } else {
            self.wait_for_auto_save();
        }
    }

    /// Lets Auto Save's write finish, which has to happen before anything else writes the file
    /// or the project is replaced.
    fn wait_for_auto_save(&mut self) {
        let Some(write) = self.auto_save_write.take() else {
            return;
        };
        self.unsaved_since = None;
        match write.worker.join() {
            Ok(Ok(())) => {
                self.baseline = write.file;
                self.auto_save_failed = None;
            }
            Ok(Err(error)) => {
                self.notice = Some(format!("Auto Save could not write the project. {error}"));
                self.auto_save_failed = Some(write.file);
            }
            Err(_) => {
                self.notice = Some("Auto Save stopped unexpectedly.".to_owned());
                self.auto_save_failed = Some(write.file);
            }
        }
    }

    /// With Auto Save on, leaving a project saves what hasn't reached the file yet instead of
    /// asking about it. If that fails, the work still counts as unsaved.
    fn flush_auto_save(&mut self) {
        self.wait_for_auto_save();
        if let Some(path) = self.auto_save_path().map(Path::to_owned)
            && self.has_unsaved_changes()
        {
            self.save_to(&path);
        }
    }

    fn toggle_auto_save(&mut self) {
        self.settings.auto_save = !self.settings.auto_save;
        self.auto_save_failed = None;
        if self.settings.auto_save && self.auto_save_held {
            self.notice = Some(AUTO_SAVE_HELD.to_owned());
        }
        self.save_settings();
    }

    /// The window's close button, Alt+F4 and the like all arrive here.
    fn handle_close_request(&mut self, ctx: &egui::Context) {
        if self.closing {
            return;
        }
        self.flush_auto_save();
        if self.has_unsaved_changes() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.leave_for = Some(LeaveFor::CloseWindow);
        } else if let Some(recovery) = &mut self.recovery {
            recovery.clear();
        }
    }

    fn position(&self) -> Ticks {
        self.preview.position(self.project.tracks.played())
    }

    fn entry(&self, id: MediaId) -> Option<&MediaEntry> {
        self.media.iter().find(|entry| entry.id == id)
    }

    fn visible_media(&self) -> impl Iterator<Item = &MediaEntry> {
        self.media
            .iter()
            .filter(|entry| !self.project.hidden.contains(&entry.id))
    }

    /// The videos the panel lists, filtered and in order.
    fn listed_media(&self) -> Vec<&MediaEntry> {
        let tracks = &self.project.tracks;
        let used = |entry: &MediaEntry| tracks.clips_using(entry.id) > 0;
        let mut listed: Vec<&MediaEntry> = self
            .visible_media()
            .filter(|entry| self.media_filter.keeps(used(entry)))
            .collect();
        listed.sort_by(|a, b| {
            let by_name = || text::natural_order(&a.name, &b.name);
            let order = match self.media_sort {
                MediaSort::Name => by_name(),
                MediaSort::DateAdded => a.id.cmp(&b.id),
                MediaSort::DateCreated => a.created.cmp(&b.created),
                MediaSort::UsedInVideo => used(a).cmp(&used(b)),
            };
            let order = if self.media_sort_descending {
                order.reverse()
            } else {
                order
            };
            order.then_with(by_name).then(a.id.cmp(&b.id))
        });
        listed
    }

    fn seek(&mut self, time: Ticks) {
        let Self {
            preview,
            project,
            media,
            ..
        } = self;
        preview.seek(project.tracks.played(), &|id| media_info(media, id), time);
        if self.project.tracks.is_empty() {
            self.picture = None;
        }
    }

    fn toggle_playback(&mut self) {
        let Self {
            preview,
            project,
            media,
            ..
        } = self;
        preview.toggle(project.tracks.played(), &|id| media_info(media, id));
    }

    /// Remembers the project as it is now, ready for a change that can be undone.
    fn record(&mut self) {
        self.history.record(self.project.clone());
    }

    /// Adds videos to the project, where they wait in the panel until put on the timeline.
    /// A project file among them is opened instead.
    pub fn import(&mut self, ctx: &egui::Context, paths: &[PathBuf]) {
        for path in paths {
            if path.extension().is_some_and(|e| e == project::EXTENSION) {
                self.leave_project(ctx, LeaveFor::Open(path.clone()));
                return;
            }
            match self.load(ctx, path) {
                Ok(entry) => {
                    self.media.push(entry);
                    self.media_panel_open = true;
                }
                Err(error) => {
                    let name = path.file_name().map_or_else(
                        || "That file".to_owned(),
                        |name| name.to_string_lossy().into_owned(),
                    );
                    self.notice = Some(format!("{name} could not be opened. {error}"));
                }
            }
        }
    }

    fn load(&mut self, ctx: &egui::Context, path: &Path) -> Result<MediaEntry, MediaError> {
        let info = probe(path)?;
        let name = path.file_name().map_or_else(
            || "Untitled".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        fonts::load_fallbacks_for(ctx, &name, &mut self.loaded_fallbacks);
        let created = std::fs::metadata(path)
            .and_then(|metadata| metadata.created())
            .ok();

        let (sender, scan) = mpsc::channel();
        let progress = Arc::new(AtomicU32::new(0));
        let cancel_scan = Arc::new(AtomicBool::new(false));
        let scan_path = path.to_owned();
        let scan_progress = progress.clone();
        let scan_cancel = cancel_scan.clone();
        let repaint = ctx.clone();
        thread::spawn(move || {
            if let Ok(mut preview) =
                VideoPreview::open(&scan_path, THUMBNAIL_WIDTH, THUMBNAIL_HEIGHT)
                && let Ok(DecodeStep::Frame(frame)) = preview.next_step(None)
            {
                let _ = sender.send(ScanMessage::Thumbnail(frame));
                repaint.request_repaint();
            }
            let outcome = scan_frames(&scan_path, &scan_cancel, |fraction| {
                scan_progress.store(fraction.to_bits(), Ordering::Relaxed);
                repaint.request_repaint();
            });
            let _ = sender.send(ScanMessage::Frames(outcome));
            repaint.request_repaint();
        });

        let strip_reader = (self.settings.thumbnails || self.settings.waveforms).then(|| {
            start_strip_reader(
                ctx,
                &info,
                self.settings.thumbnails,
                self.settings.waveforms,
            )
        });
        let id = MediaId(self.next_media);
        self.next_media += 1;
        Ok(MediaEntry {
            id,
            info,
            name,
            created,
            index: None,
            scan: Some(scan),
            cancel_scan,
            progress,
            thumbnail: None,
            strip_pictures: Vec::new(),
            strip_pictures_done: false,
            sound_shape: None,
            strip_reader,
        })
    }

    fn full_range(&self, id: MediaId) -> Option<TimeRange> {
        let entry = self.entry(id)?;
        Some(TimeRange {
            start: Ticks(0),
            end: entry.info.duration,
        })
    }

    /// Adds a whole video to the end of the timeline.
    fn add_to_timeline(&mut self, id: MediaId) {
        let Some(range) = self.full_range(id) else {
            return;
        };
        self.record();
        self.selected = Some(self.project.tracks.push(id, range));
        self.after_clip_added();
    }

    /// Adds the ticked videos to the end of the timeline in the order they were ticked,
    /// as one step to undo.
    fn add_selected_to_timeline(&mut self) {
        let chosen = std::mem::take(&mut self.selected_media);
        let ranges: Vec<(MediaId, TimeRange)> = chosen
            .into_iter()
            .filter_map(|id| Some((id, self.full_range(id)?)))
            .collect();
        if ranges.is_empty() {
            return;
        }
        self.record();
        for (id, range) in ranges {
            self.selected = Some(self.project.tracks.push(id, range));
        }
        self.after_clip_added();
    }

    /// Ticks a video, or unticks it if it already is.
    fn toggle_media(&mut self, id: MediaId) {
        if self.selected_media.contains(&id) {
            self.selected_media.retain(|chosen| *chosen != id);
        } else {
            self.selected_media.push(id);
        }
    }

    fn card_state(&self, id: MediaId) -> CardState {
        let tracks = &self.project.tracks;
        CardState {
            used: tracks.clips_using(id) > 0,
            linked: self
                .selected
                .and_then(|selected| tracks.clip(selected))
                .is_some_and(|clip| clip.media == id),
            order: self
                .selected_media
                .iter()
                .position(|chosen| *chosen == id)
                .map(|position| position + 1),
        }
    }

    /// Adds a whole video where it was dropped.
    fn drop_on_timeline(&mut self, id: MediaId, time: Ticks, target: TrackTarget) {
        let Some(range) = self.full_range(id) else {
            return;
        };
        self.record();
        self.selected = Some(self.project.tracks.place(id, range, time, target));
        self.after_clip_added();
    }

    fn after_clip_added(&mut self) {
        self.selected_gap = None;
        self.timeline_state
            .content_added(self.project.tracks.duration());
        let position = self.position();
        self.seek(position);
    }

    /// Asks before removing a video that clips are using, since they go with it.
    fn request_remove(&mut self, id: MediaId) {
        self.request_remove_many(vec![id]);
    }

    fn request_remove_many(&mut self, ids: Vec<MediaId>) {
        let in_use = ids
            .iter()
            .any(|id| self.project.tracks.clips_using(*id) > 0);
        if in_use {
            self.confirm_remove = ids;
        } else {
            self.remove_many(&ids);
        }
    }

    #[cfg(test)]
    fn remove_media(&mut self, id: MediaId) {
        self.remove_many(&[id]);
    }

    /// Removes videos from the project, as one step to undo.
    fn remove_many(&mut self, ids: &[MediaId]) {
        self.record();
        for id in ids {
            self.project.tracks.remove_media(*id);
            self.project.hidden.push(*id);
        }
        self.after_timeline_change();
    }

    /// Tidies up after clips changed, so nothing points at a clip that has gone.
    fn after_timeline_change(&mut self) {
        if let Some(selected) = self.selected
            && self.project.tracks.clip(selected).is_none()
        {
            self.selected = None;
        }
        if let Some(gap) = self.selected_gap
            && self.project.tracks.gap_at(gap).is_none()
        {
            self.selected_gap = None;
        }
        let hidden = &self.project.hidden;
        self.selected_media.retain(|id| !hidden.contains(id));
        let position = self.position();
        self.seek(position);
    }

    /// The frame boundary on or before a moment in a source video.
    fn snap_to_frame(&self, media: MediaId, source_time: Ticks) -> Ticks {
        let Some(entry) = self.entry(media) else {
            return source_time;
        };
        if source_time >= entry.info.duration {
            return entry.info.duration;
        }
        let time_base = entry.info.video.time_base;
        entry
            .index
            .as_ref()
            .and_then(|index| index.frame_at(source_time.to_pts(time_base)))
            .map_or(source_time, |pts| Ticks::from_pts(pts, time_base))
    }

    /// Splits the selected clip if the playhead is inside it, or otherwise the clip on
    /// screen. The frame at the playhead starts the second part.
    fn split_at_playhead(&mut self) {
        if self.project.tracks.is_empty() {
            self.notice = Some("Add a video to the timeline first.".to_owned());
            return;
        }
        let position = self.position();
        let Some(clip) = self.project.tracks.clip_to_split(self.selected, position) else {
            self.notice = Some("Move the playhead inside a clip to split it there.".to_owned());
            return;
        };
        self.preview.pause(self.project.tracks.played());
        let source_time = Ticks(clip.source.start.0 + position.0 - clip.start.0);
        let split_at = self.snap_to_frame(clip.media, source_time);
        let before = self.project.clone();
        if let Some(second) = self.project.tracks.split(clip.id, split_at) {
            self.history.record(before);
            self.selected = Some(second);
            self.notice = None;
        } else {
            self.notice = Some("Move the playhead inside a clip to split it there.".to_owned());
        }
    }

    /// Deletes the selected clip, leaving dead space, or closes the selected dead space.
    fn delete_selected(&mut self) {
        if let Some(selected) = self.selected {
            self.record();
            self.project.tracks.remove(selected);
            self.after_timeline_change();
        } else if let Some(gap) = self.selected_gap {
            self.remove_gap(gap);
        }
    }

    /// Puts a copy of a clip straight after it, and selects the copy.
    fn duplicate_clip(&mut self, id: ClipId) {
        let before = self.project.clone();
        if let Some(copy) = self.project.tracks.duplicate(id) {
            self.history.record(before);
            self.selected = Some(copy);
            self.after_clip_added();
        }
    }

    fn remove_all_gaps(&mut self) {
        let before = self.project.clone();
        if self.project.tracks.remove_all_gaps() {
            self.history.record(before);
            self.after_timeline_change();
        }
    }

    /// Starts exporting, first pointing out any dead space on the timeline.
    fn open_export(&mut self) {
        self.export_state = if self.project.tracks.gaps().is_empty() {
            ExportState::Choosing
        } else {
            ExportState::GapWarning
        };
    }

    fn remove_gap(&mut self, time: Ticks) {
        let before = self.project.clone();
        if self.project.tracks.remove_gap(time) {
            self.history.record(before);
            self.after_timeline_change();
        }
    }

    fn undo(&mut self) {
        let current = self.project.clone();
        if let Ok(previous) = self.history.undo(current) {
            self.project = previous;
            self.after_timeline_change();
        }
    }

    fn redo(&mut self) {
        let current = self.project.clone();
        if let Ok(next) = self.history.redo(current) {
            self.project = next;
            self.after_timeline_change();
        }
    }

    /// Moves the playhead a few seconds, without changing whether it is playing.
    fn skip(&mut self, forward: bool) {
        let seconds = if forward { SKIP_SECONDS } else { -SKIP_SECONDS };
        let target = Ticks(self.position().0 + Ticks::from_seconds(seconds).0);
        self.seek(target);
    }

    fn step_frame(&mut self, forward: bool) {
        self.preview.pause(self.project.tracks.played());
        let position = self.position();
        let gap_step = Ticks::from_seconds(if forward {
            GAP_STEP_SECONDS
        } else {
            -GAP_STEP_SECONDS
        });
        let Some(location) = self.project.tracks.played().locate(position) else {
            self.seek(Ticks(position.0 + gap_step.0));
            return;
        };
        let Some(entry) = self.entry(location.clip.media) else {
            return;
        };
        let time_base = entry.info.video.time_base;
        let Some(index) = entry.index.as_ref() else {
            return;
        };
        let shown = index
            .frame_at(location.source_time.to_pts(time_base))
            .unwrap_or_default();
        let neighbour = if forward {
            index.next_frame_after(shown)
        } else {
            index.previous_frame_before(shown)
        };
        let shown_time = Ticks::from_pts(shown, time_base);
        // The source's first and last frames have no neighbour, but the timeline may
        // carry on past them.
        let target = neighbour.map_or(Ticks(position.0 + gap_step.0), |pts| {
            let gap = Ticks::from_pts(pts, time_base).0 - shown_time.0;
            Ticks(location.clip.start.0 + shown_time.0 - location.clip.source.start.0 + gap)
        });
        self.seek(target);
    }

    fn poll_background(&mut self, ctx: &egui::Context) {
        let mut problem = None;
        for entry in &mut self.media {
            let Some(scan) = &entry.scan else {
                continue;
            };
            let mut finished = false;
            while let Ok(message) = scan.try_recv() {
                match message {
                    ScanMessage::Thumbnail(frame) => {
                        let image = egui::ColorImage::from_rgba_unmultiplied(
                            [frame.width as usize, frame.height as usize],
                            &frame.rgba,
                        );
                        entry.thumbnail = Some(ctx.load_texture(
                            format!("thumbnail-{}", entry.id.0),
                            image,
                            TextureOptions::LINEAR,
                        ));
                    }
                    ScanMessage::Frames(Ok(index)) => {
                        entry.index = Some(Arc::new(index));
                        finished = true;
                    }
                    ScanMessage::Frames(Err(error)) => {
                        problem = Some(format!("{} can't be edited. {error}", entry.name));
                        finished = true;
                    }
                }
            }
            if finished {
                entry.scan = None;
            } else {
                ctx.request_repaint_after(BACKGROUND_TICK);
            }
        }
        if problem.is_some() {
            self.notice = problem;
        }
        self.poll_strip_readers(ctx);

        let Self {
            preview,
            project,
            media,
            picture,
            notice,
            timeline_state,
            ..
        } = self;
        let frame = preview.update(project.tracks.played(), &|id| media_info(media, id));
        if let Some(frame) = frame {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [frame.width as usize, frame.height as usize],
                &frame.rgba,
            );
            match picture {
                Some(picture) => picture.set(image, TextureOptions::LINEAR),
                None => {
                    *picture = Some(ctx.load_texture("video", image, TextureOptions::LINEAR));
                }
            }
        }
        if let Some(error) = preview.take_error() {
            *notice = Some(error);
        }
        if preview.is_playing() {
            let position = preview.position(project.tracks.played());
            timeline_state.follow(position, project.tracks.duration());
            ctx.request_repaint();
        }

        let finished = match &self.export_state {
            ExportState::Running(task) => match task.result.try_recv() {
                Ok(outcome) => Some((outcome, task.output.clone())),
                Err(_) => {
                    ctx.request_repaint_after(BACKGROUND_TICK);
                    None
                }
            },
            _ => None,
        };
        if let Some((outcome, output)) = finished {
            self.export_state = match outcome {
                Ok(report) => ExportState::Done { report, output },
                Err(MediaError::Cancelled) => ExportState::Choosing,
                Err(error) => ExportState::Failed(error.to_string()),
            };
        }
    }

    /// Takes in what the background readers have found for the clips.
    fn poll_strip_readers(&mut self, ctx: &egui::Context) {
        let mut uploads_left = STRIP_UPLOADS_PER_FRAME;
        for entry in &mut self.media {
            let Some(reader) = &entry.strip_reader else {
                continue;
            };
            while uploads_left > 0 {
                let message = match reader.messages.try_recv() {
                    Ok(message) => message,
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        entry.strip_reader = None;
                        break;
                    }
                };
                match message {
                    StripMessage::Picture(picture) => {
                        let image = egui::ColorImage::from_rgba_unmultiplied(
                            [picture.width as usize, picture.height as usize],
                            &picture.rgba,
                        );
                        let name = format!("strip-{}-{}", entry.id.0, entry.strip_pictures.len());
                        let texture = ctx.load_texture(name, image, TextureOptions::LINEAR);
                        entry.strip_pictures.push((picture.time, texture));
                        uploads_left -= 1;
                    }
                    StripMessage::PicturesDone => entry.strip_pictures_done = true,
                    StripMessage::Sound(shape) => entry.sound_shape = Some(shape),
                }
            }
            if uploads_left == 0 {
                ctx.request_repaint();
                break;
            }
        }
    }

    /// Brings what is being read for the clips into line with the settings. Turning something
    /// off stops reading the file for it and forgets what was read.
    fn apply_strip_settings(&mut self, ctx: &egui::Context) {
        let (thumbnails, waveforms) = (self.settings.thumbnails, self.settings.waveforms);
        for entry in &mut self.media {
            entry.strip_reader = None;
            if !thumbnails {
                entry.strip_pictures.clear();
                entry.strip_pictures_done = false;
            }
            if !waveforms {
                entry.sound_shape = None;
            }
            let want_pictures = thumbnails && !entry.strip_pictures_done;
            let want_sound = waveforms && entry.sound_shape.is_none();
            if want_pictures {
                entry.strip_pictures.clear();
            }
            if want_pictures || want_sound {
                entry.strip_reader = Some(start_strip_reader(
                    ctx,
                    &entry.info,
                    want_pictures,
                    want_sound,
                ));
            }
        }
    }

    /// The timeline as parts to export, once every video on it has been read.
    fn export_parts(&self) -> Option<Vec<ExportPart>> {
        let mut previous_end = Ticks(0);
        self.project
            .tracks
            .played()
            .clips()
            .iter()
            .map(|clip| {
                let entry = self.entry(clip.media)?;
                let gap_before = Ticks(clip.start.0 - previous_end.0);
                previous_end = clip.end();
                Some(ExportPart {
                    input: entry.info.path.clone(),
                    frames: entry.index.clone()?,
                    range: clip.source,
                    gap_before,
                })
            })
            .collect()
    }

    /// Why the videos on the timeline can't be exported together, if they can't.
    fn export_problem(&self) -> Option<String> {
        let mut infos = self
            .project
            .tracks
            .played()
            .clips()
            .iter()
            .filter_map(|clip| self.entry(clip.media).map(|entry| &entry.info));
        let first = infos.next()?;
        infos.find_map(|other| join_problem(first, other))
    }

    /// Why dead space can't be exported for the videos on the timeline, if it can't.
    fn gap_problem(&self) -> Option<String> {
        if self.project.tracks.gaps().is_empty() {
            return None;
        }
        let first = self
            .project
            .tracks
            .played()
            .clips()
            .first()
            .and_then(|clip| self.entry(clip.media))?;
        (!can_write_gaps(&first.info.video.codec)).then(|| {
            format!(
                "Dead space can't be written for {} video yet. Remove the gaps to export it.",
                text::codec_name(&first.info.video.codec)
            )
        })
    }

    fn start_export(&mut self, ctx: &egui::Context) {
        let Some(first) = self
            .project
            .tracks
            .played()
            .clips()
            .first()
            .and_then(|clip| self.entry(clip.media))
        else {
            return;
        };
        let suggested = text::default_output_path(&first.info.path);
        let mut dialog = rfd::FileDialog::new().set_title("Export video as");
        if let Some(folder) = suggested.parent() {
            dialog = dialog.set_directory(folder);
        }
        if let Some(name) = suggested.file_name() {
            dialog = dialog.set_file_name(name.to_string_lossy());
        }
        if let Some(extension) = suggested.extension() {
            let extension = extension.to_string_lossy().into_owned();
            dialog = dialog.add_filter(format!("{} video", extension.to_uppercase()), &[extension]);
        }
        if let Some(output) = dialog.save_file() {
            self.start_export_to(ctx, output);
        }
    }

    fn start_export_to(&mut self, ctx: &egui::Context, output: PathBuf) {
        let Some(parts) = self.export_parts() else {
            return;
        };
        let request = ExportRequest {
            output: output.clone(),
            parts,
            mode: self.mode,
        };
        let (sender, result) = mpsc::channel();
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_progress = progress.clone();
        let worker_cancel = cancel.clone();
        let repaint = ctx.clone();
        thread::spawn(move || {
            let outcome = export(&request, &worker_cancel, &|fraction| {
                worker_progress.store(fraction.to_bits(), Ordering::Relaxed);
            });
            let _ = sender.send(outcome);
            repaint.request_repaint();
        });
        self.export_state = ExportState::Running(ExportTask {
            result,
            progress,
            cancel,
            output,
        });
    }

    fn pick_files(&mut self, ctx: &egui::Context) {
        let picked = rfd::FileDialog::new()
            .set_title("Add videos")
            .add_filter("Video files", &VIDEO_EXTENSIONS)
            .add_filter("All files", &["*"])
            .pick_files();
        if let Some(paths) = picked {
            self.import(ctx, &paths);
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let blocked = !matches!(self.export_state, ExportState::Closed)
            || !self.confirm_remove.is_empty()
            || self.leave_for.is_some()
            || self.missing_videos.is_some()
            || !self.recovery_offers.is_empty()
            || self.settings_open
            // Letters typed into a name are not commands.
            || ctx.egui_wants_keyboard_input();
        if blocked {
            return;
        }
        let consume = |modifiers, key| ctx.input_mut(|input| input.consume_key(modifiers, key));

        if consume(Modifiers::COMMAND | Modifiers::SHIFT, Key::S) {
            self.save_as();
        }
        if consume(Modifiers::COMMAND, Key::S) {
            self.save();
        }
        if consume(Modifiers::COMMAND, Key::N) {
            self.leave_project(ctx, LeaveFor::NewProject);
        }
        if consume(Modifiers::COMMAND | Modifiers::SHIFT, Key::O) {
            self.leave_project(ctx, LeaveFor::OpenPicked);
        }
        if consume(Modifiers::COMMAND, Key::O) {
            self.pick_files(ctx);
        }
        if consume(Modifiers::COMMAND, Key::E) && !self.project.tracks.is_empty() {
            self.open_export();
        }
        if consume(Modifiers::COMMAND, Key::D)
            && let Some(selected) = self.selected
        {
            self.duplicate_clip(selected);
        }
        if consume(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)
            || consume(Modifiers::COMMAND, Key::Y)
        {
            self.redo();
        }
        if consume(Modifiers::COMMAND, Key::Z) {
            self.undo();
        }
        if consume(Modifiers::NONE, Key::Space) {
            self.toggle_playback();
        }
        if consume(Modifiers::NONE, Key::S) {
            self.split_at_playhead();
        }
        if consume(Modifiers::NONE, Key::Delete) || consume(Modifiers::NONE, Key::Backspace) {
            self.delete_selected();
        }
        if consume(Modifiers::SHIFT, Key::ArrowLeft) {
            self.step_frame(false);
        }
        if consume(Modifiers::SHIFT, Key::ArrowRight) {
            self.step_frame(true);
        }
        if consume(Modifiers::NONE, Key::ArrowLeft) {
            self.skip(false);
        }
        if consume(Modifiers::NONE, Key::ArrowRight) {
            self.skip(true);
        }
        if consume(Modifiers::COMMAND | Modifiers::ALT, Key::Num3) {
            self.timeline_open = !self.timeline_open;
        }
        if consume(Modifiers::NONE, Key::Home) {
            self.seek(Ticks(0));
        }
        if consume(Modifiers::NONE, Key::End) {
            let end = self.project.tracks.duration();
            self.seek(end);
        }
    }

    fn take_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_owned())
                .collect()
        });
        if !dropped.is_empty() {
            self.import(ctx, &dropped);
        }
    }

    fn logo(&mut self, ctx: &egui::Context) -> Option<TextureHandle> {
        let dark = ctx.global_style().visuals.dark_mode;
        if let Some((loaded_for, logo)) = &self.logo
            && *loaded_for == dark
        {
            return Some(logo.clone());
        }
        let bytes = if dark { LOGO_DARK } else { LOGO_LIGHT };
        let icon = eframe::icon_data::from_png_bytes(bytes).ok()?;
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [icon.width as usize, icon.height as usize],
            &icon.rgba,
        );
        let logo = ctx.load_texture("logo", image, TextureOptions::LINEAR);
        self.logo = Some((dark, logo.clone()));
        Some(logo)
    }

    /// The simplified logo for the top strip. Textures have no smaller copies to draw from,
    /// so it is shrunk to about its size on screen, or its thin lines would break up.
    fn strip_logo(&mut self, ctx: &egui::Context) -> Option<TextureHandle> {
        let dark = ctx.global_style().visuals.dark_mode;
        let bytes = if dark {
            SMALL_LOGO_DARK
        } else {
            SMALL_LOGO_LIGHT
        };
        let icon = eframe::icon_data::from_png_bytes(bytes).ok()?;
        let wanted = STRIP_LOGO_HEIGHT * ctx.pixels_per_point();
        let factor = ((icon.height as f32 / wanted).floor() as usize).max(1);
        if let Some((loaded_for, loaded_factor, logo)) = &self.strip_logo
            && *loaded_for == dark
            && *loaded_factor == factor
        {
            return Some(logo.clone());
        }
        let size = [icon.width as usize, icon.height as usize];
        let image = shrunk_image(&icon.rgba, size, factor);
        let logo = ctx.load_texture("strip-logo", image, TextureOptions::LINEAR);
        self.strip_logo = Some((dark, factor, logo.clone()));
        Some(logo)
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let palette = theme::palette(&ctx);
        // The background shows through the gaps between the panels.
        ui.painter().rect_filled(ui.max_rect(), 0.0, palette.bg_app);

        if ctx.input(|input| input.viewport().close_requested()) {
            self.handle_close_request(&ctx);
        }
        self.poll_background(&ctx);
        self.handle_shortcuts(&ctx);
        self.take_dropped_files(&ctx);
        self.autosave(&ctx);
        self.update_window_title(&ctx);

        self.title_strip(ui, palette);
        self.rail(ui, palette);
        self.media_panel(ui, palette);
        // The video and the timeline share one card, from here down to the stage.
        self.card_top_row(ui, palette);
        self.timeline_panel(ui, palette);
        self.stage(ui, palette);
        self.remove_dialog(&ctx, palette);
        self.export_dialog(&ctx, palette);
        self.settings_dialog(&ctx, palette);
        self.missing_videos_dialog(&ctx, palette);
        self.recovery_dialog(&ctx, palette);
        self.unsaved_changes_dialog(&ctx, palette);
        drop_overlay(&ctx, palette);
        window_edges(&ctx);
    }

    /// The strip across the top, which replaces the Windows title bar and does its jobs.
    fn title_strip(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let frame = Frame::new().fill(palette.bg_app).inner_margin(Margin {
            left: 14,
            right: 8,
            top: 0,
            bottom: 0,
        });
        egui::Panel::top("title_strip")
            .exact_size(TITLE_STRIP_HEIGHT)
            .show_separator_line(false)
            .frame(frame)
            .show(ui, |ui| {
                let ctx = ui.ctx().clone();
                let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
                let strip = ui.interact(
                    ui.max_rect(),
                    egui::Id::unique("title_strip_drag"),
                    Sense::click_and_drag(),
                );
                if strip.double_clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                } else if strip.drag_started_by(egui::PointerButton::Primary) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                let logo = self.strip_logo(&ctx);
                ui.horizontal_centered(|ui| {
                    if let Some(logo) = logo {
                        let size = logo.size_vec2();
                        let width = STRIP_LOGO_HEIGHT * size.x / size.y;
                        ui.add(
                            egui::Image::new(&logo)
                                .fit_to_exact_size(vec2(width, STRIP_LOGO_HEIGHT)),
                        );
                    }
                    ui.label(
                        RichText::new("BareClip")
                            .font(fonts::strong(15.0))
                            .color(palette.accent),
                    );
                    ui.add_space(10.0);
                    self.project_name_controls(ui, palette);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = CONTROL_SPACING;
                        let button = |icon, name| {
                            IconButton::new(palette, Kind::Ghost, icon, name)
                                .size(WINDOW_BUTTON_SIZE, 16.0)
                        };
                        if ui
                            .add(button(icons::X, "Close window"))
                            .on_hover_text("Close")
                            .clicked()
                        {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        let (icon, name, hint) = if maximized {
                            (icons::COPY, "Restore window", "Restore")
                        } else {
                            (icons::SQUARE, "Maximise window", "Maximise")
                        };
                        if ui.add(button(icon, name)).on_hover_text(hint).clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                        }
                        if ui
                            .add(button(icons::MINUS, "Minimise window"))
                            .on_hover_text("Minimise")
                            .clicked()
                        {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                        }
                        ui.add_space(8.0);
                        if ui
                            .add(IconButton::new(
                                palette,
                                Kind::Ghost,
                                icons::SETTINGS,
                                "Settings",
                            ))
                            .on_hover_text("Settings")
                            .clicked()
                        {
                            self.settings_open = true;
                        }
                    });
                });
            });
    }

    /// The project's name, which a click renames, and the menu of things to do with files.
    fn project_name_controls(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let ctx = ui.ctx().clone();
        if let Some(mut typed) = self.renaming.take() {
            let edit = ui.add(
                egui::TextEdit::singleline(&mut typed)
                    .id(egui::Id::unique("project_name_edit"))
                    .desired_width(PROJECT_NAME_WIDTH),
            );
            let cancelled = ui.input(|input| input.key_pressed(Key::Escape));
            if edit.lost_focus() {
                let name = typed.trim();
                if !cancelled && !name.is_empty() {
                    self.project_name = name.to_owned();
                }
            } else {
                if !edit.has_focus() {
                    edit.request_focus();
                }
                self.renaming = Some(typed);
            }
        } else {
            let unsaved = self.has_unsaved_changes();
            let label = text::project_title(&self.project_name, unsaved);
            let name = ActionButton::new(palette, Kind::Ghost, &label).small();
            if ui.add(name).on_hover_text("Rename this project").clicked() {
                self.renaming = Some(self.project_name.clone());
            }
        }
        let menu = ui
            .add(
                IconButton::new(palette, Kind::Ghost, icons::CHEVRON_DOWN, "Project menu")
                    .size(26.0, 14.0),
            )
            .on_hover_text("Project");
        egui::Popup::menu(&menu).show(|ui| {
            let line = |icon, name, keys| MenuItem::new(palette, icon, name, keys);
            if ui
                .add(line(icons::FILE_PLUS, "New project", "Ctrl+N"))
                .clicked()
            {
                self.leave_project(&ctx, LeaveFor::NewProject);
            }
            if ui
                .add(line(icons::FOLDER_OPEN, "Open project", "Ctrl+Shift+O"))
                .clicked()
            {
                self.leave_project(&ctx, LeaveFor::OpenPicked);
            }
            ui.separator();
            if ui.add(line(icons::SAVE, "Save", "Ctrl+S")).clicked() {
                self.save();
            }
            if ui
                .add(line(icons::SAVE, "Save as", "Ctrl+Shift+S"))
                .clicked()
            {
                self.save_as();
            }
            ui.separator();
            let auto_save = MenuItem::choice(palette, "Auto Save", self.settings.auto_save);
            if ui.add(auto_save).clicked() {
                self.toggle_auto_save();
            }
        });
    }

    /// Keeps the name Windows shows on the taskbar up to date.
    fn update_window_title(&mut self, ctx: &egui::Context) {
        let title = format!("{} - BareClip", self.project_name);
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    fn settings_dialog(&mut self, ctx: &egui::Context, palette: &Palette) {
        if !self.settings_open {
            return;
        }
        let mut changed = false;
        let mut strip_changed = false;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("settings_dialog"))
            .frame(dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width(440.0);
                components::dialog_title(ui, &icons::SETTINGS, palette.accent, "Settings");
                ui.add_space(SETTINGS_ROW_GAP);

                ui.label(RichText::new("Theme").font(fonts::strong(13.0)));
                ui.add_space(4.0);
                theme::theme_picker(ui, palette);
                let theme = ui.ctx().options(|options| options.theme_preference).into();
                if theme != self.settings.theme {
                    self.settings.theme = theme;
                    changed = true;
                }
                ui.add_space(SETTINGS_ROW_GAP);

                ui.label(RichText::new("Export cuts").font(fonts::strong(13.0)));
                ui.colored_label(
                    palette.text_muted,
                    "Which choice the export dialog starts on.",
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = CONTROL_SPACING;
                    for (cuts, name) in [
                        (ExportCuts::FrameExact, "Frame-exact"),
                        (ExportCuts::Fastest, "Fastest"),
                    ] {
                        let chosen = self.settings.export_cuts == cuts;
                        let button = ActionButton::new(palette, Kind::Ghost, name)
                            .small()
                            .selected(chosen);
                        if ui.add(button).clicked() && !chosen {
                            self.settings.export_cuts = cuts;
                            self.mode = cuts.into();
                            changed = true;
                        }
                    }
                });
                ui.add_space(SETTINGS_ROW_GAP);

                ui.label(RichText::new("On clips").font(fonts::strong(13.0)));
                ui.colored_label(
                    palette.text_muted,
                    "Reading these takes a little while in the background for each video.",
                );
                ui.add_space(6.0);
                for (name, setting) in [
                    ("Thumbnails on clips", &mut self.settings.thumbnails),
                    ("Waveforms on clips", &mut self.settings.waveforms),
                ] {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        if ui.add(CheckBox::new(palette, *setting, name)).clicked() {
                            *setting = !*setting;
                            strip_changed = true;
                        }
                        ui.label(name);
                    });
                    ui.add_space(2.0);
                }
                ui.add_space(18.0);
                components::dialog_buttons(ui, |ui| {
                    close = ui.add(dialog_button(palette, "Done")).clicked();
                });
            });
        if strip_changed {
            self.apply_strip_settings(ctx);
        }
        if changed || strip_changed {
            self.save_settings();
        }
        if close || modal.should_close() {
            self.settings_open = false;
        }
    }

    /// Asks what to do with unsaved work before something would throw it away.
    fn unsaved_changes_dialog(&mut self, ctx: &egui::Context, palette: &Palette) {
        let Some(reason) = self.leave_for.clone() else {
            return;
        };
        let mut save = false;
        let mut discard = false;
        let mut cancel = false;
        let title = format!("Save changes to {}?", self.project_name);
        egui::Modal::new(egui::Id::unique("unsaved_changes_dialog"))
            .frame(dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width(400.0);
                components::dialog_title(ui, &icons::CIRCLE_ALERT, palette.accent, &title);
                ui.add_space(12.0);
                ui.label("Your changes will be lost if you don't save them.");
                ui.add_space(18.0);
                components::dialog_buttons(ui, |ui| {
                    let primary = ActionButton::new(palette, Kind::Primary, "Save")
                        .icon(icons::SAVE)
                        .width(DIALOG_BUTTON_WIDTH);
                    save = ui.add(primary).clicked();
                    discard = ui.add(dialog_button(palette, "Don't save")).clicked();
                    cancel = ui.add(dialog_button(palette, "Cancel")).clicked();
                });
            });
        if cancel {
            self.leave_for = None;
        } else if discard || (save && self.save()) {
            self.carry_out(ctx, reason);
        }
    }

    /// Lists videos a project can't find, so they can be pointed at or left out.
    fn missing_videos_dialog(&mut self, ctx: &egui::Context, palette: &Palette) {
        let Some(waiting) = &self.missing_videos else {
            return;
        };
        let names: Vec<(usize, String)> = waiting
            .missing
            .iter()
            .map(|&index| {
                let path = &waiting.project.media[index].path;
                (index, path.display().to_string())
            })
            .collect();
        let mut find = None;
        let mut without = false;
        let mut cancel = false;
        egui::Modal::new(egui::Id::unique("missing_videos_dialog"))
            .frame(dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width(480.0);
                components::dialog_title(
                    ui,
                    &icons::CIRCLE_ALERT,
                    palette.danger,
                    "Some videos can't be found",
                );
                ui.add_space(12.0);
                for (index, name) in &names {
                    ui.horizontal(|ui| {
                        let room = ui.available_width() - DIALOG_BUTTON_WIDTH - 8.0;
                        ui.allocate_ui_with_layout(
                            vec2(room, 20.0),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                ui.add(egui::Label::new(RichText::new(name).size(12.0)).truncate());
                            },
                        );
                        let button = ActionButton::new(palette, Kind::Secondary, "Find")
                            .icon(icons::FOLDER_OPEN)
                            .small()
                            .width(DIALOG_BUTTON_WIDTH);
                        if ui.add(button).clicked() {
                            find = Some(*index);
                        }
                    });
                }
                ui.add_space(8.0);
                ui.colored_label(
                    palette.text_muted,
                    "Opening without them leaves out their clips. The file on disk only changes if you save.",
                );
                ui.add_space(18.0);
                components::dialog_buttons(ui, |ui| {
                    without = ui.add(dialog_button(palette, "Open without them")).clicked();
                    cancel = ui.add(dialog_button(palette, "Cancel")).clicked();
                });
            });
        if let Some(index) = find
            && let Some(path) = rfd::FileDialog::new()
                .set_title("Find video")
                .add_filter("Video files", &VIDEO_EXTENSIONS)
                .add_filter("All files", &["*"])
                .pick_file()
        {
            self.point_video_at(ctx, index, path);
        } else if without {
            if let Some(waiting) = self.missing_videos.take() {
                self.finish_loading(ctx, waiting);
            }
        } else if cancel {
            self.missing_videos = None;
        }
    }

    /// Records where a missing video is now, and carries on opening once none are left.
    fn point_video_at(&mut self, ctx: &egui::Context, index: usize, path: PathBuf) {
        let Some(waiting) = &mut self.missing_videos else {
            return;
        };
        waiting.project.media[index].path = path;
        waiting.missing.retain(|missing| *missing != index);
        if waiting.missing.is_empty()
            && let Some(waiting) = self.missing_videos.take()
        {
            self.finish_loading(ctx, waiting);
        }
    }

    /// Offers back work from a run that never closed properly, one copy at a time.
    fn recovery_dialog(&mut self, ctx: &egui::Context, palette: &Palette) {
        // Another dialog goes first, and a copy that can't be read has nothing to offer.
        if self.leave_for.is_some() || self.missing_videos.is_some() {
            return;
        }
        let Some(path) = self.recovery_offers.first().cloned() else {
            return;
        };
        let Some(record) = RecoveryRecord::read(&path) else {
            storage::discard_recovery(&path);
            self.recovery_offers.remove(0);
            return;
        };
        let mut restore = false;
        let mut discard = false;
        egui::Modal::new(egui::Id::unique("recovery_dialog"))
            .frame(dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width(400.0);
                components::dialog_title(
                    ui,
                    &icons::CIRCLE_ALERT,
                    palette.accent,
                    "Restore unsaved work?",
                );
                ui.add_space(12.0);
                ui.label(format!(
                    "BareClip didn't close properly last time. Your changes to {} were kept.",
                    record.project.name
                ));
                ui.add_space(18.0);
                components::dialog_buttons(ui, |ui| {
                    let primary = ActionButton::new(palette, Kind::Primary, "Restore")
                        .width(DIALOG_BUTTON_WIDTH);
                    restore = ui.add(primary).clicked();
                    discard = ui.add(dialog_button(palette, "Discard")).clicked();
                });
            });
        if restore {
            self.recovery_offers.remove(0);
            self.begin_loading(ctx, record.project, record.saved_path, Some(path));
        } else if discard {
            self.recovery_offers.remove(0);
            storage::discard_recovery(&path);
        }
    }

    /// The top of the card: undo and redo, anything that needs saying, and Export.
    fn card_top_row(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let frame = Frame::new()
            .fill(palette.surface)
            .inner_margin(Margin::symmetric(14, 8))
            .outer_margin(Margin {
                left: 0,
                right: PANEL_GAP,
                top: 0,
                bottom: 0,
            })
            .corner_radius(CornerRadius {
                nw: PANEL_RADIUS,
                ne: PANEL_RADIUS,
                sw: 0,
                se: 0,
            });
        egui::Panel::top("card_top_row")
            .exact_size(CARD_TOP_ROW_HEIGHT)
            .show_separator_line(false)
            .frame(frame)
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = CONTROL_SPACING;
                    let can_undo = self.history.can_undo();
                    let can_redo = self.history.can_redo();
                    if ui
                        .add_enabled(
                            can_undo,
                            IconButton::new(palette, Kind::Ghost, icons::UNDO_2, "Undo"),
                        )
                        .on_hover_text("Undo (Ctrl+Z)")
                        .clicked()
                    {
                        self.undo();
                    }
                    if ui
                        .add_enabled(
                            can_redo,
                            IconButton::new(palette, Kind::Ghost, icons::REDO_2, "Redo"),
                        )
                        .on_hover_text("Redo (Ctrl+Y)")
                        .clicked()
                    {
                        self.redo();
                    }
                    if let Some(notice) = self.notice.clone() {
                        ui.add_space(8.0);
                        components::symbol(ui, &icons::CIRCLE_ALERT, 16.0, palette.danger);
                        ui.colored_label(palette.danger, notice);
                    } else if !self.preview.sound_works() {
                        ui.add_space(8.0);
                        ui.colored_label(palette.text_muted, "Preview is silent on this computer");
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let can_export = !self.project.tracks.is_empty();
                        let export = ui.add_enabled(
                            can_export,
                            ActionButton::new(palette, Kind::Primary, "Export").icon(icons::UPLOAD),
                        );
                        let export = if can_export {
                            export.on_hover_text("Export the timeline (Ctrl+E)")
                        } else {
                            export.on_disabled_hover_text("Add a video to the timeline first")
                        };
                        if export.clicked() {
                            self.open_export();
                        }
                    });
                });
            });
    }

    /// The list of tools down the left. It only ever shows what works today.
    fn rail(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let frame = Frame::new().fill(palette.bg_app).inner_margin(Margin {
            left: 8,
            right: 8,
            top: 0,
            bottom: 8,
        });
        egui::Panel::left("rail")
            .exact_size(RAIL_WIDTH)
            .show_separator_line(false)
            .frame(frame)
            .show(ui, |ui| {
                let open = self.media_panel_open;
                let hint = if open {
                    "Close My media"
                } else {
                    "Open My media"
                };
                if components::rail_tab(ui, palette, &icons::FILM, "My media", open)
                    .on_hover_text(hint)
                    .clicked()
                {
                    self.media_panel_open = !open;
                }
            });
    }

    fn media_panel(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let frame = panel_card(palette.surface, Margin::same(14))
            .corner_radius(CornerRadius::same(PANEL_RADIUS));
        // Only the close button and the rail close the panel. Its edge stops at the smallest
        // size, so whatever the drag asks for here is left out.
        let mut expanded = self.media_panel_open;
        let mut close = false;
        let panel = egui::Panel::left("media_panel")
            .resizable(true)
            .drag_to_open(false)
            .show_separator_line(false)
            .default_size(MEDIA_PANEL_WIDTH)
            .size_range(MEDIA_PANEL_MIN_WIDTH..=MEDIA_PANEL_MAX_WIDTH)
            .frame(frame)
            .show_collapsible(ui, &mut expanded, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("My media").font(fonts::strong(15.0)));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        close = ui
                            .add(
                                IconButton::new(
                                    palette,
                                    Kind::Ghost,
                                    icons::PANEL_LEFT_CLOSE,
                                    "Close media panel",
                                )
                                .size(PANEL_CLOSE_SIZE, 16.0),
                            )
                            .on_hover_text("Close this panel. My media on the left opens it again.")
                            .clicked();
                    });
                });
                ui.add_space(10.0);
                let width = ui.available_width();
                // Once there are videos, what comes next is on the cards and the timeline.
                let kind = if self.visible_media().next().is_none() {
                    Kind::Primary
                } else {
                    Kind::Secondary
                };
                let clicked = ui
                    .add(
                        ActionButton::new(palette, kind, "Add videos")
                            .icon(icons::PLUS)
                            .width(width),
                    )
                    .on_hover_text("Add videos to this project (Ctrl+O)")
                    .clicked();
                if clicked {
                    let ctx = ui.ctx().clone();
                    self.pick_files(&ctx);
                }
                ui.add_space(12.0);

                if self.visible_media().next().is_none() {
                    empty_media_hint(ui, palette);
                    return;
                }

                self.media_selection_row(ui, palette);
                ui.add_space(10.0);

                let mut actions = CardActions::default();
                let mut thumbnails = Vec::new();
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        let entries = self.listed_media();
                        if entries.is_empty() {
                            ui.colored_label(palette.text_muted, "No videos match this filter");
                        }
                        let width = ui.available_width();
                        let stacked: f32 = entries
                            .iter()
                            .map(|entry| stacked_card_height(ui, entry, width))
                            .sum();
                        let columns = media_columns(width, stacked, ui.available_height());
                        let compact = cards_are_compact(width, columns);
                        for row in entries.chunks(columns) {
                            ui.columns(columns, |cells| {
                                for (cell, entry) in cells.iter_mut().zip(row) {
                                    let state = self.card_state(entry.id);
                                    let thumbnail = media_card(
                                        cell,
                                        palette,
                                        entry,
                                        state,
                                        compact,
                                        &mut actions,
                                    );
                                    thumbnails.push((entry.id, thumbnail));
                                }
                            });
                            ui.add_space(MEDIA_CARD_SPACING);
                        }
                    });
                self.card_thumbnails = thumbnails;
                if let Some(id) = actions.add {
                    self.add_to_timeline(id);
                }
                if let Some(id) = actions.remove {
                    self.request_remove(id);
                }
                if let Some(id) = actions.toggle {
                    self.toggle_media(id);
                }
            });
        if close {
            self.media_panel_open = false;
        }
        self.media_panel_rect = panel.map_or(egui::Rect::NOTHING, |panel| panel.response.rect);
    }

    /// The row above the cards, for working on several videos at once.
    fn media_selection_row(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let visible: Vec<MediaId> = self.listed_media().iter().map(|entry| entry.id).collect();
        // A video the filter hides can't be acted on by surprise.
        self.selected_media.retain(|id| visible.contains(id));
        let chosen = self.selected_media.len();
        let all = !visible.is_empty() && chosen == visible.len();
        let show_list_controls = chosen == 0 || ui.available_width() >= MEDIA_ROW_ROOM_FOR_ALL;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let name = if all { "Deselect all" } else { "Select all" };
            if ui
                .add(CheckBox::new(palette, all, name))
                .on_hover_text(name)
                .clicked()
            {
                if all {
                    self.selected_media.clear();
                } else {
                    // Videos already ticked keep their numbers.
                    for id in visible {
                        if !self.selected_media.contains(&id) {
                            self.selected_media.push(id);
                        }
                    }
                }
            }
            if chosen == 0 {
                ui.colored_label(palette.text_muted, "Select all");
            } else {
                ui.colored_label(palette.text_muted, format!("{chosen} selected"));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = CONTROL_SPACING;
                if show_list_controls {
                    self.media_sort_button(ui, palette);
                    self.media_filter_button(ui, palette);
                }
                if chosen == 0 {
                    return;
                }
                if ui
                    .add(
                        IconButton::new(
                            palette,
                            Kind::Primary,
                            icons::PLUS,
                            "Add selected to timeline",
                        )
                        .size(CARD_BUTTON_SIZE, 16.0),
                    )
                    .on_hover_text(format!("Add to timeline ({chosen})"))
                    .clicked()
                {
                    self.add_selected_to_timeline();
                }
                if ui
                    .add(
                        IconButton::new(palette, Kind::Ghost, icons::TRASH_2, "Remove selected")
                            .size(CARD_BUTTON_SIZE, 16.0),
                    )
                    .on_hover_text(format!("Remove from this project ({chosen})"))
                    .clicked()
                {
                    let ids = std::mem::take(&mut self.selected_media);
                    self.request_remove_many(ids);
                }
            });
        });
    }

    fn media_sort_button(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let button = ui
            .add(
                IconButton::new(palette, Kind::Ghost, icons::ARROW_UP_DOWN, "Sort")
                    .size(CARD_BUTTON_SIZE, 16.0),
            )
            .on_hover_text("Sort");
        egui::Popup::menu(&button).show(|ui| {
            for (sort, name) in MediaSort::ALL {
                if ui
                    .add(MenuItem::choice(palette, name, self.media_sort == sort))
                    .clicked()
                {
                    self.media_sort = sort;
                }
            }
            ui.separator();
            for (descending, name) in [(false, "Ascending"), (true, "Descending")] {
                let chosen = self.media_sort_descending == descending;
                if ui.add(MenuItem::choice(palette, name, chosen)).clicked() {
                    self.media_sort_descending = descending;
                }
            }
        });
    }

    fn media_filter_button(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        // The button stands out while something is hidden, so a short list isn't a mystery.
        let kind = if self.media_filter == MediaFilter::All {
            Kind::Ghost
        } else {
            Kind::Secondary
        };
        let button = ui
            .add(
                IconButton::new(palette, kind, icons::LIST_FILTER, "Filter")
                    .size(CARD_BUTTON_SIZE, 16.0),
            )
            .on_hover_text("Filter");
        egui::Popup::menu(&button).show(|ui| {
            for (filter, name) in MediaFilter::ALL {
                if ui
                    .add(MenuItem::choice(palette, name, self.media_filter == filter))
                    .clicked()
                {
                    self.media_filter = filter;
                }
            }
        });
    }

    fn timeline_panel(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let frame = Frame::new()
            .fill(palette.surface)
            .inner_margin(Margin::symmetric(8, 0))
            .outer_margin(Margin {
                left: 0,
                right: PANEL_GAP,
                top: 0,
                bottom: PANEL_GAP,
            })
            .corner_radius(CornerRadius {
                nw: 0,
                ne: 0,
                sw: PANEL_RADIUS,
                se: PANEL_RADIUS,
            });
        let tallest = (ui.available_height() * TIMELINE_MAX_SHARE).max(TIMELINE_MIN_HEIGHT);
        self.timeline_height = self.timeline_height.clamp(TIMELINE_MIN_HEIGHT, tallest);
        let openness = ui.ctx().animate_bool_with_time(
            egui::Id::unique("timeline_open"),
            self.timeline_open,
            TIMELINE_SLIDE_SECONDS,
        );
        let closed_height = TRANSPORT_HEIGHT + f32::from(PANEL_GAP);
        let height = closed_height + (self.timeline_height - closed_height) * openness;
        let panel = egui::Panel::bottom("timeline")
            .exact_size(height)
            .show_separator_line(false)
            .frame(frame)
            .show(ui, |ui| {
                self.transport_row(ui, palette);
                if openness > 0.0 {
                    self.timeline_track(ui, palette);
                }
            });
        self.timeline_panel_rect = panel.response.rect;
        if self.timeline_open {
            self.timeline_resize_edge(ui, panel.response.rect);
        }
    }

    /// Dragging the line above the transport row changes the timeline's height.
    fn timeline_resize_edge(&mut self, ui: &mut egui::Ui, panel: egui::Rect) {
        let edge = egui::Rect::from_min_max(
            pos2(panel.left(), panel.top() - TIMELINE_RESIZE_GRAB),
            pos2(
                panel.right() - f32::from(PANEL_GAP),
                panel.top() + TIMELINE_RESIZE_GRAB,
            ),
        );
        let response = ui.interact(edge, egui::Id::unique("timeline_resize"), Sense::drag());
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        if response.dragged() {
            self.timeline_height -= response.drag_delta().y;
        }
    }

    /// The row between the video and the timeline. When the timeline is collapsed it is
    /// all that is left of it, so it takes over the job of moving the playhead.
    fn transport_row(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let (row, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), TRANSPORT_HEIGHT), Sense::hover());
        let line = Stroke::new(1.0, palette.border);
        ui.painter().hline(row.x_range(), row.top(), line);
        let left_to_right = Layout::left_to_right(Align::Center);
        if self.timeline_open {
            let tools = ui
                .scope_builder(UiBuilder::new().max_rect(row).layout(left_to_right), |ui| {
                    self.timeline_tools(ui, palette);
                })
                .response
                .rect;
            let wanted = TRANSPORT_WIDTH + TIME_READOUT_WIDTH;
            let left = (row.center().x - wanted / 2.0).max(tools.right() + 8.0);
            let rest = row.with_min_x(left);
            ui.scope_builder(
                UiBuilder::new().max_rect(rest).layout(left_to_right),
                |ui| {
                    ui.spacing_mut().item_spacing.x = CONTROL_SPACING;
                    self.transport_buttons(ui, palette);
                    self.time_readout(ui, palette);
                },
            );
        } else {
            let inner = row.shrink2(vec2(6.0, 0.0));
            ui.scope_builder(
                UiBuilder::new().max_rect(inner).layout(left_to_right),
                |ui| {
                    ui.spacing_mut().item_spacing.x = CONTROL_SPACING;
                    self.transport_buttons(ui, palette);
                    self.time_readout(ui, palette);
                    ui.add_space(8.0);
                    let room = ui.available_width() - WINDOW_BUTTON_SIZE - CONTROL_SPACING - 8.0;
                    let position = self.position();
                    let total = self.project.tracks.duration();
                    if let Some(time) =
                        timeline::seek_bar(ui, palette, room.max(40.0), position, total)
                    {
                        self.seek(time);
                    }
                    ui.add_space(8.0);
                    if ui
                        .add(IconButton::new(
                            palette,
                            Kind::Ghost,
                            icons::PANEL_BOTTOM_OPEN,
                            "Expand timeline",
                        ))
                        .on_hover_text("Expand timeline (Ctrl+Alt+3)")
                        .clicked()
                    {
                        self.timeline_open = true;
                    }
                },
            );
        }
    }

    fn transport_buttons(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let has_clips = !self.project.tracks.is_empty();
        let total = self.project.tracks.duration();
        let playing = self.preview.is_playing();
        let quiet = |icon, name| IconButton::new(palette, Kind::Ghost, icon, name);
        if ui
            .add_enabled(has_clips, quiet(icons::SKIP_BACK, "Go to start"))
            .on_hover_text("Go to the start (Home)")
            .clicked()
        {
            self.seek(Ticks(0));
        }
        if skip_button(
            ui,
            palette,
            has_clips,
            icons::ROTATE_CCW,
            "5 seconds back",
            "Back 5 seconds (Left arrow). One frame: Shift+Left",
        )
        .clicked()
        {
            self.skip(false);
        }
        let (icon, name) = if playing {
            (icons::PAUSE, "Pause")
        } else {
            (icons::PLAY, "Play")
        };
        let play = IconButton::new(palette, Kind::Primary, icon, name)
            .size(36.0, 16.0)
            .round()
            .solid();
        if ui
            .add_enabled(has_clips, play)
            .on_hover_text("Play or pause (Space)")
            .clicked()
        {
            self.toggle_playback();
        }
        if skip_button(
            ui,
            palette,
            has_clips,
            icons::ROTATE_CW,
            "5 seconds forward",
            "Forward 5 seconds (Right arrow). One frame: Shift+Right",
        )
        .clicked()
        {
            self.skip(true);
        }
        if ui
            .add_enabled(has_clips, quiet(icons::SKIP_FORWARD, "Go to end"))
            .on_hover_text("Go to the end (End)")
            .clicked()
        {
            self.seek(total);
        }
    }

    fn time_readout(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        ui.add_space(8.0);
        ui.label(RichText::new(format_timecode(self.position())).font(fonts::strong(15.0)));
        ui.colored_label(
            palette.text_muted,
            format!("/ {}", format_timecode(self.project.tracks.duration())),
        );
    }

    fn timeline_tools(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let has_clips = !self.project.tracks.is_empty();
        let can_delete = self.selected.is_some() || self.selected_gap.is_some();
        let has_gaps = !self.project.tracks.gaps().is_empty();
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = CONTROL_SPACING;
            ui.add_space(6.0);
            let tool = |icon, name| IconButton::new(palette, Kind::Ghost, icon, name);
            if ui
                .add_enabled(has_clips, tool(icons::SCISSORS, "Split"))
                .on_hover_text("Split the clip at the playhead (S)")
                .clicked()
            {
                self.split_at_playhead();
            }
            if !can_delete {
                ui.add_space(TOOL_SLOT);
            } else if ui
                .add(tool(icons::TRASH_2, "Delete"))
                .on_hover_text("Delete the selected clip or dead space (Delete)")
                .clicked()
            {
                self.delete_selected();
            }
            if !has_gaps {
                ui.add_space(TOOL_SLOT);
            } else if ui
                .add(tool(icons::FOLD_HORIZONTAL, "Remove all gaps"))
                .on_hover_text("Remove all gaps")
                .clicked()
            {
                self.remove_all_gaps();
            }
        });
    }

    fn timeline_track(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let clips: Vec<ClipView> = self
            .project
            .tracks
            .tracks()
            .iter()
            .enumerate()
            .flat_map(|(track, clips)| clips.clips().iter().map(move |clip| (track, clip)))
            .map(|(track, clip)| ClipView {
                id: clip.id,
                track,
                start: clip.start,
                end: clip.end(),
                label: self
                    .entry(clip.media)
                    .map_or_else(String::new, |entry| entry.name.clone()),
                media: clip.media,
                source_start: clip.source.start,
            })
            .collect();
        let faces = self
            .media
            .iter()
            .map(|entry| {
                let face = ClipFace {
                    pictures: &entry.strip_pictures,
                    sound: entry.sound_shape.as_ref(),
                };
                (entry.id, face)
            })
            .collect();
        let view = TimelineView {
            tracks: &self.project.tracks,
            clips: &clips,
            faces: &faces,
            position: self.position(),
            selected: self.selected,
            selected_gap: self.selected_gap,
        };
        let events = timeline::timeline(ui, palette, &mut self.timeline_state, &view);
        self.apply_timeline_events(events);
    }

    fn apply_timeline_events(&mut self, events: timeline::TimelineEvents) {
        if let Some(selection) = events.select {
            self.selected = selection;
            self.selected_gap = events.select_gap;
        }
        if let Some((id, edge, pointer)) = events.trim_started
            && let Some(clip) = self.project.tracks.clip(id)
        {
            self.preview.pause(self.project.tracks.played());
            self.record();
            let source = match edge {
                ClipEdge::Start => clip.source.start,
                ClipEdge::End => clip.source.end,
            };
            self.trim_origin = Some(TrimOrigin {
                source,
                pointer,
                playhead: self.position(),
            });
        }
        if let (Some((id, edge, pointer)), Some(origin)) = (events.trim_to, self.trim_origin) {
            self.trim_clip(id, edge, origin, pointer, events.trim_snap);
        }
        if events.trim_finished {
            self.trim_origin = None;
        }
        if events.collapse {
            self.timeline_open = false;
        }
        if let Some((id, time, target)) = events.move_clip {
            let before = self.project.clone();
            self.project.tracks.move_clip(id, time, target);
            if self.project != before {
                self.history.record(before);
                self.after_timeline_change();
            }
        }
        if let Some(gap) = events.remove_gap {
            self.remove_gap(gap);
        }
        if let Some((media, time, target)) = events.drop_media {
            self.drop_on_timeline(media, time, target);
        }
        match events.menu {
            Some(MenuAction::Split) => self.split_at_playhead(),
            Some(MenuAction::Duplicate(id)) => self.duplicate_clip(id),
            Some(MenuAction::Delete(id)) => {
                self.selected = Some(id);
                self.delete_selected();
            }
            Some(MenuAction::RemoveGap(time)) => self.remove_gap(time),
            Some(MenuAction::RemoveAllGaps) => self.remove_all_gaps(),
            None => {}
        }
        if let Some(seek) = events.seek {
            self.seek(seek);
        }
    }

    /// Moves a clip's edge by however far the pointer has travelled. The playhead stays
    /// where it was, and the picture only changes if the trim changed what is under it.
    /// An edge snapping to something lands exactly on it even if that falls partway through
    /// one of its own frames, because missing by a fraction of a frame could export as a
    /// flash of black between clips on different tracks.
    fn trim_clip(
        &mut self,
        id: ClipId,
        edge: ClipEdge,
        origin: TrimOrigin,
        pointer: Ticks,
        snap: Option<Ticks>,
    ) {
        let Some(clip) = self.project.tracks.clip(id) else {
            return;
        };
        let Some(duration) = self.entry(clip.media).map(|entry| entry.info.duration) else {
            return;
        };
        let wanted = Ticks(origin.source.0 + pointer.0 - origin.pointer.0);
        let source_time = if snap.is_some() {
            wanted
        } else {
            self.snap_to_frame(clip.media, wanted)
        };
        let under_playhead = self.project.tracks.played().locate(origin.playhead);
        self.project.tracks.trim(
            id,
            edge,
            source_time,
            duration,
            Ticks::from_seconds(MIN_CLIP_SECONDS),
        );
        let trimmed = self.project.tracks.clip(id);
        self.timeline_state.trim_aligned = snap.filter(|moment| {
            trimmed.is_some_and(|clip| match edge {
                ClipEdge::Start => clip.start == *moment,
                ClipEdge::End => clip.end() == *moment,
            })
        });
        if self.project.tracks.played().locate(origin.playhead) != under_playhead {
            self.seek(origin.playhead);
        }
    }

    fn stage(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        // Around the picture is simply more of the card. Dead space stays black below,
        // because black is what it exports as.
        let frame = Frame::new()
            .fill(palette.surface)
            .inner_margin(Margin::same(STAGE_MARGIN))
            .outer_margin(Margin {
                left: 0,
                right: PANEL_GAP,
                top: 0,
                bottom: 0,
            });
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            let rect = ui.available_rect_before_wrap();
            if self.preview.in_gap(self.project.tracks.played()) {
                if let Some(size) = self.picture_size() {
                    let scale = (rect.width() / size.x).min(rect.height() / size.y);
                    let shown = egui::Rect::from_center_size(rect.center(), size * scale);
                    ui.painter().rect_filled(shown, 0.0, Color32::BLACK);
                }
                return;
            }
            if let Some(picture) = &self.picture {
                let size = picture.size_vec2();
                let scale = (rect.width() / size.x).min(rect.height() / size.y);
                let shown = egui::Rect::from_center_size(rect.center(), size * scale);
                ui.painter().image(
                    picture.id(),
                    shown,
                    egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                return;
            }
            if !self.project.tracks.is_empty() {
                return;
            }

            let ctx = ui.ctx().clone();
            let logo = self.logo(&ctx);
            let hint = if self.visible_media().next().is_none() {
                "Add a video to get started"
            } else {
                "Press + on a video to put it on the timeline"
            };
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space((rect.height() - 200.0).max(0.0) / 2.0);
                    if let Some(logo) = logo {
                        ui.add(egui::Image::new(&logo).fit_to_exact_size(vec2(96.0, 111.0)));
                        ui.add_space(18.0);
                    }
                    ui.label(
                        RichText::new(hint)
                            .font(fonts::strong(18.0))
                            .color(palette.text),
                    );
                });
            });
        });
    }

    /// The shape of the picture, even before any frame of it has been shown.
    fn picture_size(&self) -> Option<egui::Vec2> {
        self.picture
            .as_ref()
            .map(TextureHandle::size_vec2)
            .or_else(|| {
                let first = self.project.tracks.played().clips().first()?;
                let video = &self.entry(first.media)?.info.video;
                Some(vec2(video.width as f32, video.height as f32))
            })
    }

    fn remove_dialog(&mut self, ctx: &egui::Context, palette: &Palette) {
        if self.confirm_remove.is_empty() {
            return;
        }
        let count: usize = self
            .confirm_remove
            .iter()
            .map(|id| self.project.tracks.clips_using(*id))
            .sum();
        let (title, warning) = match self.confirm_remove.as_slice() {
            [id] => {
                let name = self
                    .entry(*id)
                    .map_or_else(String::new, |entry| entry.name.clone());
                ("Remove this video?", text::remove_warning(&name, count))
            }
            ids => (
                "Remove these videos?",
                text::remove_many_warning(ids.len(), count),
            ),
        };
        let mut confirmed = false;
        let mut cancelled = false;
        egui::Modal::new(egui::Id::unique("remove_dialog"))
            .frame(dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width(400.0);
                components::dialog_title(ui, &icons::CIRCLE_ALERT, palette.danger, title);
                ui.add_space(12.0);
                ui.label(warning);
                ui.add_space(4.0);
                ui.colored_label(
                    palette.text_muted,
                    "The file on your computer is not touched.",
                );
                ui.add_space(18.0);
                components::dialog_buttons(ui, |ui| {
                    confirmed = ui
                        .add(
                            ActionButton::new(palette, Kind::Danger, "Remove")
                                .icon(icons::X)
                                .width(DIALOG_BUTTON_WIDTH),
                        )
                        .clicked();
                    cancelled = ui.add(dialog_button(palette, "Cancel")).clicked();
                });
            });
        if confirmed {
            let ids = std::mem::take(&mut self.confirm_remove);
            self.remove_many(&ids);
        } else if cancelled {
            self.confirm_remove.clear();
        }
    }

    fn export_dialog(&mut self, ctx: &egui::Context, palette: &Palette) {
        if matches!(self.export_state, ExportState::Closed) {
            return;
        }
        let mut next_state = None;
        let mut begin_export = false;
        let mut remove_gaps = false;
        let gaps = self.project.tracks.gaps();
        let gap_length = Ticks(gaps.iter().map(|gap| gap.duration().0).sum());
        let problem = self.export_problem();
        let gap_problem = self.gap_problem();
        let ready = self.export_parts().is_some();
        let summary = self.plan_description();

        egui::Modal::new(egui::Id::unique("export_dialog")).frame(dialog_frame(ctx)).show(ctx, |ui| {
            ui.set_width(440.0);
            match &self.export_state {
                ExportState::GapWarning => {
                    components::dialog_title(
                        ui,
                        &icons::CIRCLE_ALERT,
                        palette.accent,
                        "This video has gaps",
                    );
                    ui.add_space(12.0);
                    ui.label(text::gap_warning(gaps.len(), gap_length));
                    ui.add_space(4.0);
                    ui.colored_label(
                        palette.text_muted,
                        "They show as hatching on the ruler. Removing them can be undone.",
                    );
                    ui.add_space(18.0);
                    components::dialog_buttons(ui, |ui| {
                        let remove = ActionButton::new(palette, Kind::Primary, "Remove all gaps")
                            .icon(icons::X);
                        if ui.add(remove).clicked() {
                            remove_gaps = true;
                        }
                        if ui.add(dialog_button(palette, "Keep gaps")).clicked() {
                            next_state = Some(ExportState::Choosing);
                        }
                        if ui.add(dialog_button(palette, "Cancel")).clicked() {
                            next_state = Some(ExportState::Closed);
                        }
                    });
                }
                ExportState::Choosing => {
                    components::dialog_title(ui, &icons::UPLOAD, palette.accent, "Export");
                    ui.add_space(14.0);
                    if components::choice_card(
                        ui,
                        palette,
                        &icons::TARGET,
                        "Frame-exact",
                        "Cuts land exactly where they are on the timeline. Short parts around each cut are re-encoded.",
                        self.mode == CutMode::FrameExact,
                    )
                    .clicked()
                    {
                        self.mode = CutMode::FrameExact;
                    }
                    ui.add_space(8.0);
                    if components::choice_card(
                        ui,
                        palette,
                        &icons::ZAP,
                        "Fastest",
                        "Nothing is re-encoded, but each cut moves outward to the nearest keyframe, keeping a little extra.",
                        self.mode == CutMode::Keyframe,
                    )
                    .clicked()
                    {
                        self.mode = CutMode::Keyframe;
                    }
                    ui.add_space(14.0);

                    if let Some(problem) = &problem {
                        ui.colored_label(palette.danger, problem);
                        ui.add_space(4.0);
                        ui.colored_label(
                            palette.text_muted,
                            "Videos need the same format to export together for now. Mixing formats comes later.",
                        );
                    } else if let Some(gap_problem) = &gap_problem {
                        ui.colored_label(palette.danger, gap_problem);
                    } else if let Some(summary) = &summary {
                        ui.label(summary);
                    } else {
                        ui.colored_label(palette.text_muted, "Still reading the videos' frames.");
                    }
                    ui.add_space(18.0);
                    components::dialog_buttons(ui, |ui| {
                        let can_start = ready && problem.is_none() && gap_problem.is_none();
                        let start = ActionButton::new(palette, Kind::Primary, "Choose file and export")
                            .icon(icons::UPLOAD);
                        if ui.add_enabled(can_start, start).clicked() {
                            begin_export = true;
                        }
                        if ui.add(dialog_button(palette, "Cancel")).clicked() {
                            next_state = Some(ExportState::Closed);
                        }
                    });
                }
                ExportState::Running(task) => {
                    components::dialog_title(ui, &icons::UPLOAD, palette.accent, "Exporting");
                    ui.add_space(14.0);
                    let fraction = f32::from_bits(task.progress.load(Ordering::Relaxed));
                    components::progress_bar(ui, palette, fraction);
                    ui.add_space(18.0);
                    components::dialog_buttons(ui, |ui| {
                        if ui.add(dialog_button(palette, "Cancel")).clicked() {
                            task.cancel.store(true, Ordering::Relaxed);
                        }
                    });
                }
                ExportState::Done { report, output } => {
                    components::dialog_title(
                        ui,
                        &icons::CIRCLE_CHECK,
                        palette.success,
                        "Export finished",
                    );
                    ui.add_space(12.0);
                    for line in text::export_summary(report) {
                        ui.label(line);
                    }
                    ui.add_space(6.0);
                    ui.colored_label(palette.text_muted, output.display().to_string());
                    ui.add_space(18.0);
                    components::dialog_buttons(ui, |ui| {
                        let done = ActionButton::new(palette, Kind::Primary, "Done")
                            .icon(icons::CHECK)
                            .width(DIALOG_BUTTON_WIDTH);
                        if ui.add(done).clicked() {
                            next_state = Some(ExportState::Closed);
                        }
                        let show = dialog_button(palette, "Show in folder").icon(icons::FOLDER_OPEN);
                        if ui.add(show).clicked() {
                            show_in_folder(output);
                        }
                    });
                }
                ExportState::Failed(error) => {
                    components::dialog_title(
                        ui,
                        &icons::CIRCLE_ALERT,
                        palette.danger,
                        "Export failed",
                    );
                    ui.add_space(12.0);
                    ui.colored_label(palette.danger, error);
                    ui.add_space(18.0);
                    components::dialog_buttons(ui, |ui| {
                        if ui.add(dialog_button(palette, "Close")).clicked() {
                            next_state = Some(ExportState::Choosing);
                        }
                    });
                }
                ExportState::Closed => {}
            }
        });

        if remove_gaps {
            self.remove_all_gaps();
            next_state = Some(ExportState::Choosing);
        }
        if begin_export {
            self.start_export(ctx);
        }
        if let Some(state) = next_state {
            self.export_state = state;
        }
    }

    fn plan_description(&self) -> Option<String> {
        let mut copied = 0;
        let mut encoded = 0;
        for clip in self.project.tracks.played().clips() {
            let entry = self.entry(clip.media)?;
            let index = entry.index.as_ref()?;
            let plan = plan_export(index, &[clip.source], entry.info.video.time_base, self.mode);
            copied += plan.copied_frames;
            encoded += plan.encoded_frames;
        }
        Some(text::plan_summary(copied, encoded))
    }
}

/// A rounded panel. The gap on its right and below it keeps it clear of its neighbours,
/// and the rail or panel before it keeps it clear on the left.
fn panel_card(fill: Color32, inner: Margin) -> Frame {
    Frame::new()
        .fill(fill)
        .inner_margin(inner)
        .outer_margin(Margin {
            left: 0,
            right: PANEL_GAP,
            top: 0,
            bottom: PANEL_GAP,
        })
}

fn dialog_frame(ctx: &egui::Context) -> Frame {
    Frame::popup(&ctx.global_style())
        .inner_margin(Margin::same(20))
        .corner_radius(CornerRadius::same(DIALOG_RADIUS))
}

/// The ordinary, secondary button of a dialog.
fn dialog_button<'a>(palette: &'a Palette, label: &'a str) -> ActionButton<'a> {
    ActionButton::new(palette, Kind::Secondary, label).width(DIALOG_BUTTON_WIDTH)
}

fn empty_media_hint(ui: &mut egui::Ui, palette: &Palette) {
    let rect = ui.available_rect_before_wrap();
    let height = rect.height().clamp(120.0, EMPTY_HINT_MAX_HEIGHT);
    let area = egui::Rect::from_min_size(rect.min, vec2(rect.width(), height));
    let painter = ui.painter_at(area.expand(2.0));
    let stroke = Stroke::new(1.0, palette.border_strong);
    let corners = [
        area.left_top(),
        area.right_top(),
        area.right_bottom(),
        area.left_bottom(),
        area.left_top(),
    ];
    painter.extend(egui::Shape::dashed_line(&corners, stroke, 6.0, 5.0));
    icons::paint(
        &painter,
        icons::square_at(area.center() - vec2(0.0, 28.0), 26.0),
        &icons::FILM,
        palette.text_muted,
    );
    painter.text(
        area.center() + vec2(0.0, 4.0),
        egui::Align2::CENTER_CENTER,
        "Drop videos here",
        fonts::strong(14.0),
        palette.text,
    );
    painter.text(
        area.center() + vec2(0.0, 25.0),
        egui::Align2::CENTER_CENTER,
        "MP4, MKV, MOV and more",
        FontId::new(12.0, FontFamily::Proportional),
        palette.text_muted,
    );
}

/// How many media cards go across the panel. One column is the most readable, so a second
/// only comes when the list would otherwise have to scroll, even if that squeezes the
/// cards. A panel wide enough for roomy cards side by side always gets them.
fn media_columns(width: f32, stacked_height: f32, room: f32) -> usize {
    let per_card = MEDIA_CARD_MIN_WIDTH + MEDIA_CARD_SPACING;
    let roomy = ((width + MEDIA_CARD_SPACING) / per_card).floor() as usize;
    if roomy >= 2 {
        roomy
    } else if stacked_height > room {
        2
    } else {
        1
    }
}

/// Whether cards are too narrow for anything but their thumbnail and name.
fn cards_are_compact(width: f32, columns: usize) -> bool {
    let spaces = columns.saturating_sub(1) as f32 * MEDIA_CARD_SPACING;
    columns > 1 && (width - spaces) / (columns as f32) < MEDIA_CARD_MIN_WIDTH
}

/// The lines of detail under a card's name, and whether each one is a warning.
fn card_details(entry: &MediaEntry) -> Vec<(String, bool)> {
    let first = if entry.scan.is_some() {
        let fraction = f32::from_bits(entry.progress.load(Ordering::Relaxed)) * 100.0;
        format!("Reading frames {fraction:.0}%")
    } else {
        text::file_size(entry.info.size_bytes)
    };
    let mut details = vec![(first, false), (text::video_summary(&entry.info), false)];
    for (position, track) in entry.info.audio.iter().enumerate() {
        details.push((text::audio_track_summary(position, track), false));
    }
    if entry.info.video.is_hdr {
        details.push((HDR_WARNING.to_owned(), true));
    }
    details
}

/// How much height a card and the space after it take with the panel in one column.
fn stacked_card_height(ui: &egui::Ui, entry: &MediaEntry, width: f32) -> f32 {
    let margins = f32::from(CARD_INNER_MARGIN) * 2.0;
    let inner = width - margins;
    let thumbnail = inner * 9.0 / 16.0;
    let details: f32 = card_details(entry)
        .into_iter()
        .map(|(line, _)| {
            let font = FontId::proportional(12.0);
            let galley = ui.painter().layout(line, font, Color32::WHITE, inner);
            galley.size().y + CARD_ROW_SPACING
        })
        .sum();
    margins + thumbnail + CARD_ROW_SPACING + 6.0 + CARD_NAME_HEIGHT + details + MEDIA_CARD_SPACING
}

/// A button beside play that moves the playhead, with how far written inside its symbol.
fn skip_button(
    ui: &mut egui::Ui,
    palette: &Palette,
    enabled: bool,
    icon: icons::Icon,
    name: &str,
    hint: &str,
) -> egui::Response {
    let button = ui
        .add_enabled(enabled, IconButton::new(palette, Kind::Ghost, icon, name))
        .on_hover_text(hint);
    let color = if enabled {
        palette.text
    } else {
        palette.text_muted
    };
    ui.painter().text(
        button.rect.center() + vec2(0.0, 0.5),
        egui::Align2::CENTER_CENTER,
        "5",
        fonts::strong(8.0),
        color,
    );
    button
}

/// A smaller copy of a picture, averaging each square of `factor` pixels into one. It
/// averages with colours weighted by their opacity, or edges would pick up a dark fringe from
/// the invisible pixels next to them.
fn shrunk_image(rgba: &[u8], [width, height]: [usize; 2], factor: usize) -> egui::ColorImage {
    let size = [width.div_ceil(factor), height.div_ceil(factor)];
    let mut shrunk = Vec::with_capacity(size[0] * size[1] * 4);
    for y in 0..size[1] {
        for x in 0..size[0] {
            let mut sum = [0_u32; 4];
            let mut count = 0;
            for source_y in (y * factor)..((y + 1) * factor).min(height) {
                for source_x in (x * factor)..((x + 1) * factor).min(width) {
                    let pixel = &rgba[(source_y * width + source_x) * 4..][..4];
                    let alpha = u32::from(pixel[3]);
                    for channel in 0..3 {
                        sum[channel] += u32::from(pixel[channel]) * alpha / 255;
                    }
                    sum[3] += alpha;
                    count += 1;
                }
            }
            shrunk.extend(sum.map(|total| (total / count) as u8));
        }
    }
    egui::ColorImage::from_rgba_premultiplied(size, &shrunk)
}

/// Which way the window resizes from a point, if that point is on its edge.
fn resize_direction(window: egui::Rect, at: egui::Pos2) -> Option<egui::ResizeDirection> {
    use egui::ResizeDirection as Direction;
    let west = at.x - window.left() <= RESIZE_EDGE;
    let east = window.right() - at.x <= RESIZE_EDGE;
    let north = at.y - window.top() <= RESIZE_EDGE;
    let south = window.bottom() - at.y <= RESIZE_EDGE;
    let near_west = at.x - window.left() <= RESIZE_CORNER;
    let near_east = window.right() - at.x <= RESIZE_CORNER;
    let near_north = at.y - window.top() <= RESIZE_CORNER;
    let near_south = window.bottom() - at.y <= RESIZE_CORNER;
    if (north && near_west) || (west && near_north) {
        Some(Direction::NorthWest)
    } else if (north && near_east) || (east && near_north) {
        Some(Direction::NorthEast)
    } else if (south && near_west) || (west && near_south) {
        Some(Direction::SouthWest)
    } else if (south && near_east) || (east && near_south) {
        Some(Direction::SouthEast)
    } else if north {
        Some(Direction::North)
    } else if south {
        Some(Direction::South)
    } else if west {
        Some(Direction::West)
    } else if east {
        Some(Direction::East)
    } else {
        None
    }
}

fn resize_cursor(direction: egui::ResizeDirection) -> egui::CursorIcon {
    use egui::{CursorIcon as Cursor, ResizeDirection as Direction};
    match direction {
        Direction::North => Cursor::ResizeNorth,
        Direction::South => Cursor::ResizeSouth,
        Direction::West => Cursor::ResizeWest,
        Direction::East => Cursor::ResizeEast,
        Direction::NorthEast => Cursor::ResizeNorthEast,
        Direction::SouthEast => Cursor::ResizeSouthEast,
        Direction::NorthWest => Cursor::ResizeNorthWest,
        Direction::SouthWest => Cursor::ResizeSouthWest,
    }
}

/// With no Windows title bar the app resizes its own window from its edges.
fn window_edges(ctx: &egui::Context) {
    let (maximized, pointer, pressed, busy) = ctx.input(|input| {
        (
            input.viewport().maximized.unwrap_or(false),
            input.pointer.latest_pos(),
            input.pointer.primary_pressed(),
            input.pointer.any_down(),
        )
    });
    let Some(pointer) = pointer.filter(|_| !maximized) else {
        return;
    };
    let Some(direction) = resize_direction(ctx.content_rect(), pointer) else {
        return;
    };
    if pressed {
        ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
    }
    if !busy || pressed {
        ctx.set_cursor_icon(resize_cursor(direction));
    }
}

/// Draws one video in the panel, and returns where its thumbnail is.
fn media_card(
    ui: &mut egui::Ui,
    palette: &Palette,
    entry: &MediaEntry,
    state: CardState,
    compact: bool,
    actions: &mut CardActions,
) -> egui::Rect {
    let mut thumbnail_rect = egui::Rect::NOTHING;
    let outline = if state.order.is_some() {
        Stroke::new(2.0, palette.accent)
    } else {
        Stroke::new(1.0, palette.border)
    };
    let card = Frame::new()
        .fill(palette.raised)
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .inner_margin(Margin::same(CARD_INNER_MARGIN))
        .stroke(outline)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = CARD_ROW_SPACING;
            let drag_id = egui::Id::unique(("media-card", entry.id.0));
            let payload = MediaDrag {
                media: entry.id,
                length: entry.info.duration,
            };
            ui.dnd_drag_source(drag_id, payload, |ui| {
                let width = ui.available_width();
                let size = vec2(width, width * 9.0 / 16.0);
                let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                thumbnail_rect = rect;
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 4.0, palette.stage);
                if let Some(thumbnail) = &entry.thumbnail {
                    let image = thumbnail.size_vec2();
                    let scale = (rect.width() / image.x).min(rect.height() / image.y);
                    painter.image(
                        thumbnail.id(),
                        egui::Rect::from_center_size(rect.center(), image * scale),
                        egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                // The stage colour stays dark in both themes, so this label reads the same.
                let length = painter.layout_no_wrap(
                    text::short_duration(entry.info.duration),
                    FontId::new(11.0, FontFamily::Proportional),
                    palette.clip_video_text,
                );
                let badge = egui::Rect::from_min_size(
                    rect.left_bottom() + vec2(6.0, -6.0 - length.size().y - 4.0),
                    length.size() + vec2(10.0, 4.0),
                );
                painter.rect_filled(badge, 4.0, palette.stage.gamma_multiply(0.75));
                painter.galley(badge.min + vec2(5.0, 2.0), length, palette.clip_video_text);
            })
            .response
            .on_hover_text(card_hint(entry, compact));
            ui.add_space(6.0);
            card_name(ui, palette, entry, state);
            let details = card_details(entry);
            for (position, (line, warning)) in details.into_iter().enumerate() {
                // A squeezed card still says if its video isn't ready, and the rest moves
                // to its hint.
                let reading = position == 0 && entry.scan.is_some();
                if compact && !reading && !warning {
                    continue;
                }
                let color = if warning {
                    palette.danger
                } else {
                    palette.text_muted
                };
                ui.add(egui::Label::new(RichText::new(line).size(12.0).color(color)).truncate());
            }
        });

    let hovered = ui.rect_contains_pointer(card.response.rect);
    if hovered && state.order.is_none() {
        ui.painter().rect_stroke(
            card.response.rect,
            CARD_RADIUS,
            Stroke::new(1.0, palette.border_strong),
            egui::StrokeKind::Inside,
        );
    }
    let (button_size, margin) = if compact {
        (COMPACT_CARD_BUTTON_SIZE, COMPACT_CARD_BUTTON_MARGIN)
    } else {
        (CARD_BUTTON_SIZE, CARD_BUTTON_MARGIN)
    };
    let icon_size = button_size * 16.0 / CARD_BUTTON_SIZE;
    let size = vec2(button_size, button_size);
    let add_rect = egui::Rect::from_min_size(
        thumbnail_rect.right_bottom() - size - vec2(margin, margin),
        size,
    );
    let remove_rect = egui::Rect::from_min_size(
        thumbnail_rect.right_top() + vec2(-button_size - margin, margin),
        size,
    );
    let added = IconButton::new(palette, Kind::Primary, icons::PLUS, "Add to timeline")
        .size(button_size, icon_size)
        .hidden(!hovered)
        .show_at(
            ui,
            egui::Id::unique(("media-card-add", entry.id.0)),
            add_rect,
        )
        .on_hover_text("Add to timeline")
        .on_hover_cursor(egui::CursorIcon::Default);
    if added.clicked() {
        actions.add = Some(entry.id);
    }
    let removed = IconButton::new(
        palette,
        Kind::Secondary,
        icons::X,
        "Remove from this project",
    )
    .size(button_size, icon_size)
    .hidden(!hovered)
    .show_at(
        ui,
        egui::Id::unique(("media-card-remove", entry.id.0)),
        remove_rect,
    )
    .on_hover_text("Remove from this project")
    .on_hover_cursor(egui::CursorIcon::Default);
    if removed.clicked() {
        actions.remove = Some(entry.id);
    }
    let select_name = format!("Select {}", entry.name);
    let ticked = state.order.is_some();
    let select = CheckBox::new(palette, ticked, &select_name)
        .number(state.order)
        .hidden(!hovered && !ticked)
        .show_in_corner(
            ui,
            egui::Id::unique(("media-card-select", entry.id.0)),
            thumbnail_rect,
            SELECT_BOX_MARGIN,
        )
        .on_hover_text(if ticked { "Deselect" } else { "Select" })
        .on_hover_cursor(egui::CursorIcon::Default);
    if select.clicked() {
        actions.toggle = Some(entry.id);
    }
    thumbnail_rect
}

/// What hovering a thumbnail says. Squeezed cards keep their details here instead.
fn card_hint(entry: &MediaEntry, compact: bool) -> String {
    let drag = "Drag onto the timeline";
    if !compact {
        return drag.to_owned();
    }
    let mut lines: Vec<String> = card_details(entry)
        .into_iter()
        .map(|(line, _)| line)
        .collect();
    lines.insert(0, entry.name.clone());
    lines.push(drag.to_owned());
    lines.join("\n")
}

/// A video's name, and a tick if it is on the timeline. Both stand out while one of its
/// clips is selected.
fn card_name(ui: &mut egui::Ui, palette: &Palette, entry: &MediaEntry, state: CardState) {
    let (row, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), CARD_NAME_HEIGHT), Sense::hover());
    let room = if state.used { CARD_TICK_ROOM } else { 0.0 };
    let name = if state.linked {
        RichText::new(&entry.name).font(fonts::strong(13.0))
    } else {
        RichText::new(&entry.name).size(13.0)
    };
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(row.with_max_x(row.right() - room))
            .layout(Layout::left_to_right(Align::Center)),
        |ui| ui.add(egui::Label::new(name).truncate()),
    );
    if !state.used {
        return;
    }
    let tick = icons::square_at(
        egui::pos2(row.right() - CARD_TICK_SIZE / 2.0, row.center().y),
        CARD_TICK_SIZE,
    );
    let color = if state.linked {
        palette.accent
    } else {
        palette.text_muted
    };
    icons::paint(ui.painter(), tick, &icons::CIRCLE_CHECK, color);
    let label = "On the timeline";
    let response = ui.interact(
        tick,
        egui::Id::unique(("media-card-used", entry.id.0)),
        Sense::hover(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, label));
    response.on_hover_text(label);
}

fn drop_overlay(ctx: &egui::Context, palette: &Palette) {
    if ctx.input(|input| input.raw.hovered_files.is_empty()) {
        return;
    }
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("drop_overlay"),
    ));
    let screen = ctx.content_rect();
    painter.rect_filled(screen, 0.0, palette.stage.gamma_multiply(0.85));
    painter.text(
        screen.center(),
        egui::Align2::CENTER_CENTER,
        "Drop to add to My media",
        FontId::new(24.0, FontFamily::Proportional),
        Color32::from_rgb(0xF1, 0xEE, 0xE9),
    );
}

fn show_in_folder(path: &Path) {
    let _ = std::process::Command::new("explorer")
        .arg("/select,")
        .arg(path)
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::{egui_wgpu, wgpu};
    use std::{process::Command, sync::OnceLock, time::Instant};

    fn make_clip(name: &str, size: &str) -> PathBuf {
        let folder = std::env::temp_dir().join("bareclip-app-tests");
        std::fs::create_dir_all(&folder).expect("the folder can be created");
        let path = folder.join(format!("{name}-{}.mp4", std::process::id()));
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
            ])
            .arg(format!("testsrc2=s={size}:r=30:d=4,format=yuv420p"))
            .args(["-c:v", "libopenh264", "-b:v", "2M", "-g", "30"])
            .arg(&path)
            .status()
            .expect("ffmpeg is on PATH");
        assert!(status.success());
        path
    }

    fn test_clip() -> &'static Path {
        static CLIP: OnceLock<PathBuf> = OnceLock::new();
        CLIP.get_or_init(|| make_clip("editor", "320x180"))
    }

    fn second_clip() -> &'static Path {
        static CLIP: OnceLock<PathBuf> = OnceLock::new();
        CLIP.get_or_init(|| make_clip("editor-second", "320x180"))
    }

    fn small_clip() -> &'static Path {
        static CLIP: OnceLock<PathBuf> = OnceLock::new();
        CLIP.get_or_init(|| make_clip("editor-small", "160x90"))
    }

    /// A clip whose sound has a regular beat, so its shape is easy to judge by eye.
    fn clip_with_sound() -> PathBuf {
        static CLIP: OnceLock<PathBuf> = OnceLock::new();
        CLIP.get_or_init(|| {
            let folder = std::env::temp_dir().join("bareclip-app-tests");
            std::fs::create_dir_all(&folder).expect("the folder can be created");
            let path = folder.join(format!("editor-sound-{}.mp4", std::process::id()));
            let status = Command::new("ffmpeg")
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                ])
                .arg("testsrc2=s=320x180:r=30:d=8,format=yuv420p")
                .args(["-f", "lavfi", "-i"])
                .arg("sine=f=440:r=48000:d=8,volume='0.2+0.8*abs(sin(t*2))':eval=frame")
                .args([
                    "-c:v",
                    "libopenh264",
                    "-b:v",
                    "2M",
                    "-g",
                    "30",
                    "-c:a",
                    "aac",
                ])
                .arg(&path)
                .status()
                .expect("ffmpeg is on PATH");
            assert!(status.success());
            path
        })
        .clone()
    }

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        fonts::install(&ctx);
        theme::install(&ctx);
        ctx
    }

    fn run_frame(
        ctx: &egui::Context,
        editor: &mut Editor,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                pos2(0.0, 0.0),
                vec2(1280.0, 800.0),
            )),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| editor.show(ui));
        let mut textures = std::mem::take(&mut output.textures_delta);
        KEPT_TEXTURES.with_borrow_mut(|kept| match kept {
            Some(kept) => kept.push(textures),
            None => textures.clear(),
        });
        output
    }

    thread_local! {
        /// Texture changes saved for `Camera`, which draws frames some time later.
        static KEPT_TEXTURES: std::cell::RefCell<Option<Vec<egui::TexturesDelta>>> =
            const { std::cell::RefCell::new(None) };
    }

    /// Draws the screen with the real renderer into a picture, with no window involved.
    struct Camera {
        device: wgpu::Device,
        queue: wgpu::Queue,
        renderer: egui_wgpu::Renderer,
        folder: PathBuf,
    }

    const CAMERA_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
    const CAMERA_SIZE: [u32; 2] = [1280, 800];

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        loop {
            if let std::task::Poll::Ready(value) = future.as_mut().poll(&mut context) {
                return value;
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    impl Camera {
        fn new(folder: PathBuf) -> Self {
            std::fs::create_dir_all(&folder).expect("the folder can be created");
            let instance = wgpu::Instance::default();
            let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            }))
            .expect("a GPU is available");
            let (device, queue) = block_on(adapter.request_device(&Default::default()))
                .expect("the GPU can be opened");
            let renderer = egui_wgpu::Renderer::new(&device, CAMERA_FORMAT, Default::default());
            KEPT_TEXTURES.with_borrow_mut(|kept| *kept = Some(Vec::new()));
            Self {
                device,
                queue,
                renderer,
                folder,
            }
        }

        /// Runs one frame with these events and saves it as a PPM picture.
        fn shoot(
            &mut self,
            ctx: &egui::Context,
            editor: &mut Editor,
            events: Vec<egui::Event>,
            name: &str,
        ) {
            let output = run_frame(ctx, editor, events);
            let mut textures = KEPT_TEXTURES
                .with_borrow_mut(|kept| std::mem::take(kept.as_mut().expect("the camera is on")));
            for delta in &textures {
                for (id, images) in &delta.set {
                    for image in images {
                        self.renderer
                            .update_texture(&self.device, &self.queue, *id, image);
                    }
                }
            }
            let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
            let screen = egui_wgpu::ScreenDescriptor {
                size_in_pixels: CAMERA_SIZE,
                pixels_per_point: output.pixels_per_point,
            };
            let size = wgpu::Extent3d {
                width: CAMERA_SIZE[0],
                height: CAMERA_SIZE[1],
                depth_or_array_layers: 1,
            };
            let target = self.device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: CAMERA_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = target.create_view(&Default::default());
            let row_bytes =
                (CAMERA_SIZE[0] * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
            let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: u64::from(row_bytes * CAMERA_SIZE[1]),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });

            let mut encoder = self.device.create_command_encoder(&Default::default());
            let extra = self.renderer.update_buffers(
                &self.device,
                &self.queue,
                &mut encoder,
                &jobs,
                &screen,
            );
            {
                let mut pass = encoder
                    .begin_render_pass(&wgpu::RenderPassDescriptor {
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        ..Default::default()
                    })
                    .forget_lifetime();
                self.renderer.render(&mut pass, &jobs, &screen);
            }
            encoder.copy_texture_to_buffer(
                target.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row_bytes),
                        rows_per_image: None,
                    },
                },
                size,
            );
            self.queue
                .submit(extra.into_iter().chain([encoder.finish()]));
            readback.map_async(wgpu::MapMode::Read, .., |result| {
                result.expect("the picture can be read back");
            });
            self.device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("the GPU finishes");

            let data = readback
                .get_mapped_range(..)
                .expect("the picture was mapped");
            let mut picture =
                format!("P6 {} {} 255\n", CAMERA_SIZE[0], CAMERA_SIZE[1]).into_bytes();
            for row in data.chunks(row_bytes as usize) {
                for pixel in row[..CAMERA_SIZE[0] as usize * 4].chunks(4) {
                    picture.extend_from_slice(&pixel[..3]);
                }
            }
            std::fs::write(self.folder.join(format!("{name}.ppm")), picture)
                .expect("the picture can be saved");
            drop(data);
            for delta in &mut textures {
                for id in &delta.free {
                    self.renderer.free_texture(id);
                }
                delta.clear();
            }
        }
    }

    fn moved(at: egui::Pos2) -> Vec<egui::Event> {
        vec![egui::Event::PointerMoved(at)]
    }

    /// Saves pictures of the screen to look at, in the folder named by `BARECLIP_SCREENS`.
    /// Needs a GPU, and a person or tool to judge the results.
    #[test]
    #[ignore = "needs a GPU, and writes pictures to look at"]
    fn draw_the_timeline_screens() {
        let folder = std::env::var_os("BARECLIP_SCREENS").map_or_else(
            || std::env::temp_dir().join("bareclip-screens"),
            PathBuf::from,
        );
        let ctx = context();
        let mut camera = Camera::new(folder);
        let mut editor = in_panel_only(&ctx, &[test_clip(), second_clip()]);
        camera.shoot(&ctx, &mut editor, vec![], "01-in-panel-only");

        let thumbnail = editor.card_thumbnails[0].1;
        camera.shoot(
            &ctx,
            &mut editor,
            moved(thumbnail.center()),
            "02-card-hover",
        );

        click(
            &ctx,
            &mut editor,
            thumbnail.right_bottom() - vec2(20.0, 20.0),
        );
        let second = editor.card_thumbnails[1].1;
        click(&ctx, &mut editor, second.right_bottom() - vec2(20.0, 20.0));
        let scale = editor.timeline_state.scale;
        let track = editor.timeline_state.track;
        let y = track.center().y;
        let x_of = |seconds: f64| scale.x_of(Ticks::from_seconds(seconds));
        let wait_for_picture = |ctx: &egui::Context, editor: &mut Editor| {
            for _ in 0..50 {
                run_frame(ctx, editor, vec![]);
                thread::sleep(Duration::from_millis(10));
            }
        };
        wait_for_picture(&ctx, &mut editor);
        camera.shoot(
            &ctx,
            &mut editor,
            moved(pos2(700.0, 150.0)),
            "03-both-added",
        );
        camera.shoot(
            &ctx,
            &mut editor,
            moved(pos2(x_of(2.5), track.top() - 12.0)),
            "04-hover-ruler",
        );
        camera.shoot(
            &ctx,
            &mut editor,
            moved(pos2(x_of(9.5), y)),
            "05-hover-past-end",
        );

        // Trim the first clip's start, which leaves dead space in front of it.
        pointer_drag(
            &ctx,
            &mut editor,
            pos2(x_of(0.0) + 4.0, y),
            pos2(x_of(1.5) + 4.0, y),
        );
        camera.shoot(
            &ctx,
            &mut editor,
            moved(pos2(700.0, 150.0)),
            "06-trimmed-start",
        );
        camera.shoot(&ctx, &mut editor, moved(pos2(x_of(0.2), y)), "07-hover-gap");
        editor.seek(Ticks::from_seconds(0.5));
        wait_for_picture(&ctx, &mut editor);
        camera.shoot(
            &ctx,
            &mut editor,
            moved(pos2(700.0, 150.0)),
            "08-playhead-in-gap",
        );
        click(&ctx, &mut editor, pos2(x_of(0.1), y));
        camera.shoot(
            &ctx,
            &mut editor,
            moved(pos2(700.0, 150.0)),
            "09-gap-selected",
        );

        // Halfway through dragging a clip over the other one.
        let middle = pos2(x_of(6.0), y);
        run_frame(&ctx, &mut editor, moved(middle));
        let press = |pressed, pos| {
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            }]
        };
        run_frame(&ctx, &mut editor, press(true, middle));
        let target = pos2(x_of(3.0), y);
        for step in 1..=6 {
            let point = middle + (target - middle) * (step as f32 / 6.0);
            run_frame(&ctx, &mut editor, moved(point));
        }
        camera.shoot(&ctx, &mut editor, moved(target), "10-dragging-a-clip");
        run_frame(&ctx, &mut editor, press(false, target));
        editor.undo();
        wait_for_picture(&ctx, &mut editor);

        let edge = editor.media_panel_rect.right();
        pointer_drag(
            &ctx,
            &mut editor,
            pos2(edge, 300.0),
            pos2(edge + 250.0, 300.0),
        );
        wait_for_picture(&ctx, &mut editor);
        camera.shoot(
            &ctx,
            &mut editor,
            moved(pos2(1000.0, 150.0)),
            "11-wide-media-panel",
        );
        let edge = editor.media_panel_rect.right();
        pointer_drag(
            &ctx,
            &mut editor,
            pos2(edge, 300.0),
            pos2(edge - 250.0, 300.0),
        );

        let top = editor.timeline_panel_rect.top();
        pointer_drag(
            &ctx,
            &mut editor,
            pos2(800.0, top),
            pos2(800.0, top - 120.0),
        );
        camera.shoot(
            &ctx,
            &mut editor,
            moved(pos2(1000.0, 150.0)),
            "12-tall-timeline",
        );

        let [zoom_out, zoom_in, _] = editor.timeline_state.zoom_buttons;
        for _ in 0..4 {
            click(&ctx, &mut editor, zoom_in.center());
        }
        camera.shoot(
            &ctx,
            &mut editor,
            moved(pos2(1000.0, 150.0)),
            "13-zoomed-in",
        );
        for _ in 0..10 {
            click(&ctx, &mut editor, zoom_out.center());
        }
        camera.shoot(
            &ctx,
            &mut editor,
            moved(pos2(1000.0, 150.0)),
            "14-zoomed-out",
        );

        let away = pos2(700.0, 150.0);
        let [_, _, fit] = editor.timeline_state.zoom_buttons;
        click(&ctx, &mut editor, fit.center());
        let theme_button = named_rect(&ctx, &mut editor, "Settings");
        click(&ctx, &mut editor, theme_button.center());
        for _ in 0..12 {
            run_frame(&ctx, &mut editor, moved(theme_button.center()));
        }
        camera.shoot(
            &ctx,
            &mut editor,
            moved(theme_button.center()),
            "16-settings",
        );
        click_named(&ctx, &mut editor, "Done");
        run_frame(&ctx, &mut editor, vec![]);
        let export_button = named_rect(&ctx, &mut editor, "Export");
        camera.shoot(
            &ctx,
            &mut editor,
            moved(export_button.center()),
            "17-export-button-hover",
        );

        // A dialog takes a few frames to measure itself and fade in.
        let shoot_dialog = |camera: &mut Camera, editor: &mut Editor, name: String| {
            for _ in 0..12 {
                run_frame(&ctx, editor, moved(away));
            }
            camera.shoot(&ctx, editor, moved(away), &name);
        };
        let show_dialogs = |camera: &mut Camera, editor: &mut Editor, prefix: &str| {
            editor.export_state = ExportState::Choosing;
            shoot_dialog(camera, editor, format!("{prefix}-export-choosing"));
            let (_sender, result) = mpsc::channel();
            editor.export_state = ExportState::Running(ExportTask {
                result,
                progress: Arc::new(AtomicU32::new(0.42_f32.to_bits())),
                cancel: Arc::new(AtomicBool::new(false)),
                output: PathBuf::from("export.mp4"),
            });
            shoot_dialog(camera, editor, format!("{prefix}-export-running"));
            editor.export_state = ExportState::Done {
                report: ExportReport {
                    mode_requested: CutMode::FrameExact,
                    mode_used: CutMode::FrameExact,
                    sections: 2,
                    copied_frames: 200,
                    encoded_frames: 12,
                    black_frames: 0,
                    encoder: Some("h264_nvenc".to_owned()),
                    audio_tracks: 2,
                    elapsed: Duration::from_millis(2100),
                },
                output: PathBuf::from("C:\\Videos\\export.mp4"),
            };
            shoot_dialog(camera, editor, format!("{prefix}-export-done"));
            editor.export_state = ExportState::Failed("The disk is full.".to_owned());
            shoot_dialog(camera, editor, format!("{prefix}-export-failed"));
            editor.export_state = ExportState::Closed;
            editor.confirm_remove = editor.media.iter().take(1).map(|entry| entry.id).collect();
            shoot_dialog(camera, editor, format!("{prefix}-remove-dialog"));
            editor.confirm_remove.clear();
            for _ in 0..12 {
                run_frame(&ctx, editor, vec![]);
            }
        };
        show_dialogs(&mut camera, &mut editor, "18-dark");

        ctx.set_theme(egui::Theme::Light);
        theme::install(&ctx);
        let [_, _, fit] = editor.timeline_state.zoom_buttons;
        click(&ctx, &mut editor, fit.center());
        let scale = editor.timeline_state.scale;
        run_frame(&ctx, &mut editor, vec![]);
        let hover = pos2(
            scale.x_of(Ticks::from_seconds(0.5)),
            editor.timeline_state.track.center().y,
        );
        camera.shoot(&ctx, &mut editor, moved(hover), "15-light-theme");
        show_dialogs(&mut camera, &mut editor, "19-light");
        let mut empty = Editor::new(&ctx, false);
        run_frame(&ctx, &mut empty, vec![]);
        camera.shoot(&ctx, &mut empty, vec![], "20-light-empty");
        ctx.set_theme(egui::Theme::Dark);
        run_frame(&ctx, &mut empty, vec![]);
        camera.shoot(&ctx, &mut empty, vec![], "21-dark-empty");

        // The working states of the timeline, the cards and the panel.
        let mut study = with_videos(&ctx, &[test_clip(), second_clip()]);
        let away = pos2(700.0, 150.0);
        wait_for_picture(&ctx, &mut study);
        camera.shoot(&ctx, &mut study, moved(away), "22-selected-clip");
        let scale = study.timeline_state.scale;
        let track = study.timeline_state.track;
        let y = track.center().y;
        let x_of = |seconds: f64| scale.x_of(Ticks::from_seconds(seconds));
        camera.shoot(
            &ctx,
            &mut study,
            moved(pos2(x_of(2.0), y)),
            "23-hovered-clip",
        );

        // Halfway through a trim, and a move that has snapped.
        let trim_from = pos2(x_of(8.0) - 4.0, y);
        let trim_to = pos2(x_of(6.5), y);
        begin_drag(&ctx, &mut study, trim_from, trim_to);
        camera.shoot(&ctx, &mut study, moved(trim_to), "24-trim-readout");
        end_drag(&ctx, &mut study, trim_to);
        study.undo();
        let move_from = pos2(x_of(6.0), y);
        let move_to = move_from + vec2(7.0, 0.0);
        begin_drag(&ctx, &mut study, move_from, move_to);
        camera.shoot(&ctx, &mut study, moved(move_to), "25-snapped");
        end_drag(&ctx, &mut study, move_to);

        // The playhead's time, while it is dragged.
        let ruler_y = track.top() - 12.0;
        let scrub_to = pos2(x_of(3.0), ruler_y);
        begin_drag(&ctx, &mut study, pos2(x_of(5.0), ruler_y), scrub_to);
        camera.shoot(&ctx, &mut study, moved(scrub_to), "26-playhead-bubble");
        end_drag(&ctx, &mut study, scrub_to);

        // A gap, and the right-click menu.
        pointer_drag(
            &ctx,
            &mut study,
            pos2(x_of(0.0) + 4.0, y),
            pos2(x_of(1.2) + 4.0, y),
        );
        camera.shoot(
            &ctx,
            &mut study,
            moved(pos2(x_of(0.3), track.top() - 12.0)),
            "27-gap-hover",
        );
        let on_clip = pos2(x_of(3.0), y);
        secondary_click(&ctx, &mut study, on_clip);
        for _ in 0..12 {
            run_frame(&ctx, &mut study, moved(on_clip));
        }
        camera.shoot(&ctx, &mut study, moved(on_clip), "28-clip-menu");
        run_frame(&ctx, &mut study, key_event(Key::Escape, Modifiers::NONE));
        for _ in 0..12 {
            run_frame(&ctx, &mut study, vec![]);
        }
        study.open_export();
        for _ in 0..12 {
            run_frame(&ctx, &mut study, moved(away));
        }
        camera.shoot(&ctx, &mut study, moved(away), "29-gap-warning");
        study.export_state = ExportState::Closed;

        // Several videos ticked, then the panel sliding and closed.
        click_named(&ctx, &mut study, "Select all");
        camera.shoot(&ctx, &mut study, moved(away), "30-videos-selected");
        click_named(&ctx, &mut study, "Deselect all");
        click_named(&ctx, &mut study, "Close media panel");
        camera.shoot(&ctx, &mut study, moved(away), "31-panel-sliding");
        for _ in 0..30 {
            run_frame(&ctx, &mut study, moved(away));
        }
        camera.shoot(&ctx, &mut study, moved(away), "32-panel-closed");
        study.media_panel_open = true;
        for _ in 0..30 {
            run_frame(&ctx, &mut study, moved(away));
        }
        ctx.set_theme(egui::Theme::Light);
        camera.shoot(
            &ctx,
            &mut study,
            moved(pos2(x_of(2.0), y)),
            "33-light-cards",
        );
        ctx.set_theme(egui::Theme::Dark);

        // Tracks, the collapsed timeline and the two-column panel.
        let mut tracks = with_videos(&ctx, &[test_clip(), second_clip()]);
        wait_for_strip(&ctx, &mut tracks);
        let upper = tracks.project.tracks.played().clips()[1].id;
        tracks.project.tracks.trim(
            upper,
            ClipEdge::End,
            Ticks::from_seconds(1.5),
            Ticks::from_seconds(4.0),
            Ticks::from_seconds(0.05),
        );
        tracks
            .project
            .tracks
            .move_clip(upper, Ticks::from_seconds(1.0), TrackTarget::New(0));
        tracks.after_timeline_change();
        run_frame(&ctx, &mut tracks, vec![]);
        let away = pos2(640.0, 90.0);
        camera.shoot(&ctx, &mut tracks, moved(away), "34-two-tracks");

        let layout = tracks.timeline_state.layout;
        let scale = tracks.timeline_state.scale;
        let lower_y = layout.track(1).center().y;
        let from = pos2(scale.x_of(Ticks::from_seconds(3.0)), lower_y);
        let between = pos2(
            from.x,
            (layout.track(0).bottom() + layout.track(1).top()) / 2.0,
        );
        begin_drag(&ctx, &mut tracks, from, between);
        camera.shoot(&ctx, &mut tracks, moved(between), "35-drop-between-tracks");
        let below = pos2(from.x, layout.new_lane_below().center().y);
        camera.shoot(&ctx, &mut tracks, moved(below), "36-drop-on-new-lane");
        run_frame(
            &ctx,
            &mut tracks,
            pointer_button(from, egui::PointerButton::Primary, false),
        );

        click_named(&ctx, &mut tracks, "Fit");
        camera.shoot(&ctx, &mut tracks, moved(away), "37-fit");

        tracks.timeline_height = 560.0;
        let_timeline_finish(&ctx, &mut tracks);
        camera.shoot(&ctx, &mut tracks, moved(away), "38-tall-timeline-tracks");
        tracks.timeline_height = TIMELINE_HEIGHT;
        let_timeline_finish(&ctx, &mut tracks);

        tracks.timeline_open = false;
        let_timeline_finish(&ctx, &mut tracks);
        camera.shoot(&ctx, &mut tracks, moved(away), "39-timeline-collapsed");
        tracks.timeline_open = true;
        let_timeline_finish(&ctx, &mut tracks);

        let many: Vec<PathBuf> = (0..7).map(|_| test_clip().to_path_buf()).collect();
        tracks.import(&ctx, &many);
        settle(&ctx, &mut tracks);
        for _ in 0..3 {
            run_frame(&ctx, &mut tracks, vec![]);
        }
        camera.shoot(&ctx, &mut tracks, moved(away), "40-two-columns");
        click_named(&ctx, &mut tracks, "Sort");
        for _ in 0..12 {
            run_frame(&ctx, &mut tracks, vec![]);
        }
        camera.shoot(&ctx, &mut tracks, moved(away), "41-sort-menu");
        run_frame(&ctx, &mut tracks, key_event(Key::Escape, Modifiers::NONE));
        click_named(&ctx, &mut tracks, "Filter");
        for _ in 0..12 {
            run_frame(&ctx, &mut tracks, vec![]);
        }
        camera.shoot(&ctx, &mut tracks, moved(away), "42-filter-menu");
        run_frame(&ctx, &mut tracks, key_event(Key::Escape, Modifiers::NONE));
        ctx.set_theme(egui::Theme::Light);
        for _ in 0..3 {
            run_frame(&ctx, &mut tracks, vec![]);
        }
        camera.shoot(&ctx, &mut tracks, moved(away), "43-light-two-tracks");
        ctx.set_theme(egui::Theme::Dark);

        let (mut aligned, _, _) = clips_on_two_tracks(&ctx, 1.0);
        let y = aligned.timeline_state.layout.track(1).center().y;
        let from = pos2(time_x(&aligned, 5.0) - 4.0, y);
        let to = pos2(time_x(&aligned, 4.0) - 1.0, y);
        begin_drag(&ctx, &mut aligned, from, to);
        run_frame(&ctx, &mut aligned, moved(to));
        camera.shoot(&ctx, &mut aligned, moved(to), "44-trim-aligned");

        // Projects: the name, its menu and the dialogs that protect unsaved work.
        // A clip with both pictures and sound, zoomed in on.
        let mut sounding = with_videos(&ctx, &[&clip_with_sound()]);
        wait_for_strip(&ctx, &mut sounding);
        camera.shoot(
            &ctx,
            &mut sounding,
            moved(pos2(640.0, 90.0)),
            "50-pictures-and-sound",
        );
        ctx.set_theme(egui::Theme::Light);
        for _ in 0..3 {
            run_frame(&ctx, &mut sounding, vec![]);
        }
        camera.shoot(
            &ctx,
            &mut sounding,
            moved(pos2(640.0, 90.0)),
            "51-light-pictures-and-sound",
        );
        ctx.set_theme(egui::Theme::Dark);

        let mut named = with_videos(&ctx, &[test_clip()]);
        named.project_name = "Final round".to_owned();
        let settled = |editor: &mut Editor| {
            for _ in 0..12 {
                run_frame(&ctx, editor, moved(pos2(640.0, 300.0)));
            }
        };
        click_named(&ctx, &mut named, "Project menu");
        settled(&mut named);
        camera.shoot(
            &ctx,
            &mut named,
            moved(pos2(640.0, 300.0)),
            "45-project-menu",
        );
        run_frame(&ctx, &mut named, key_event(Key::Escape, Modifiers::NONE));
        settled(&mut named);
        named.leave_for = Some(LeaveFor::NewProject);
        settled(&mut named);
        camera.shoot(
            &ctx,
            &mut named,
            moved(pos2(640.0, 300.0)),
            "46-unsaved-changes",
        );
        named.leave_for = None;
        named.missing_videos = Some(MissingVideos {
            project: ProjectFile {
                media: vec![project::SavedMedia {
                    id: 1,
                    path: PathBuf::from(r"D:\Recordings\2026\ranked-match-final.mkv"),
                }],
                ..ProjectFile::default()
            },
            path: None,
            restored_from: None,
            missing: vec![0],
        });
        settled(&mut named);
        camera.shoot(
            &ctx,
            &mut named,
            moved(pos2(640.0, 300.0)),
            "47-missing-video",
        );
        named.missing_videos = None;
        settled(&mut named);
        let recovered = project_folder("screens");
        let mut lost = with_storage(&ctx, &recovered);
        lost.import(&ctx, &[test_clip().to_path_buf()]);
        settle(&ctx, &mut lost);
        autosave_now(&ctx, &mut lost);
        crash(lost);
        let mut offered = with_storage(&ctx, &recovered);
        settled(&mut offered);
        camera.shoot(
            &ctx,
            &mut offered,
            moved(pos2(640.0, 300.0)),
            "48-restore-offer",
        );
        run_frame(&ctx, &mut offered, vec![]);
        offered.recovery_offers.clear();
        offered.settings_open = true;
        settled(&mut offered);
        camera.shoot(&ctx, &mut offered, moved(pos2(640.0, 300.0)), "49-settings");
        KEPT_TEXTURES.with_borrow_mut(|kept| *kept = None);
    }

    fn key_event(key: Key, modifiers: Modifiers) -> Vec<egui::Event> {
        vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }]
    }

    fn press(key: Key) -> Vec<egui::Event> {
        key_event(key, Modifiers::NONE)
    }

    /// Runs a frame and reads back what screen readers are told is on screen.
    fn named_nodes(ctx: &egui::Context, editor: &mut Editor) -> Vec<(String, egui::Rect, bool)> {
        ctx.enable_accesskit();
        let output = run_frame(ctx, editor, vec![]);
        let update = output
            .platform_output
            .accesskit_update
            .expect("the tree is reported");
        update
            .nodes
            .iter()
            .filter_map(|(_, node)| {
                let bounds = node.bounds()?;
                let rect = egui::Rect::from_min_max(
                    pos2(bounds.x0 as f32, bounds.y0 as f32),
                    pos2(bounds.x1 as f32, bounds.y1 as f32),
                );
                Some((node.label()?.to_owned(), rect, node.is_disabled()))
            })
            .collect()
    }

    /// Where the control with this name is, so a test presses it the way a person would.
    fn named_rect(ctx: &egui::Context, editor: &mut Editor, name: &str) -> egui::Rect {
        named_nodes(ctx, editor)
            .into_iter()
            .find(|(label, ..)| label == name)
            .unwrap_or_else(|| panic!("nothing is named {name}"))
            .1
    }

    fn click_named(ctx: &egui::Context, editor: &mut Editor, name: &str) {
        let rect = named_rect(ctx, editor, name);
        click(ctx, editor, rect.center());
    }

    /// Runs frames until every video has been read.
    fn settle(ctx: &egui::Context, editor: &mut Editor) {
        let started = Instant::now();
        while editor.media.iter().any(|entry| entry.scan.is_some()) {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "reading frames took too long"
            );
            run_frame(ctx, editor, vec![]);
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn with_videos(ctx: &egui::Context, paths: &[&Path]) -> Editor {
        let mut editor = Editor::new(ctx, false);
        run_frame(ctx, &mut editor, vec![]);
        let paths: Vec<PathBuf> = paths.iter().map(|path| path.to_path_buf()).collect();
        editor.import(ctx, &paths);
        settle(ctx, &mut editor);
        let added: Vec<MediaId> = editor.media.iter().map(|entry| entry.id).collect();
        for id in added {
            editor.add_to_timeline(id);
        }
        run_frame(ctx, &mut editor, vec![]);
        editor
    }

    /// Videos added to the project, but not yet put on the timeline.
    fn in_panel_only(ctx: &egui::Context, paths: &[&Path]) -> Editor {
        let mut editor = Editor::new(ctx, false);
        run_frame(ctx, &mut editor, vec![]);
        let paths: Vec<PathBuf> = paths.iter().map(|path| path.to_path_buf()).collect();
        editor.import(ctx, &paths);
        settle(ctx, &mut editor);
        run_frame(ctx, &mut editor, vec![]);
        editor
    }

    fn pointer_button(
        at: egui::Pos2,
        button: egui::PointerButton,
        pressed: bool,
    ) -> Vec<egui::Event> {
        vec![egui::Event::PointerButton {
            pos: at,
            button,
            pressed,
            modifiers: Modifiers::NONE,
        }]
    }

    /// Presses at one place and moves to another, leaving the drag in progress.
    fn begin_drag(ctx: &egui::Context, editor: &mut Editor, from: egui::Pos2, to: egui::Pos2) {
        run_frame(ctx, editor, moved(from));
        run_frame(
            ctx,
            editor,
            pointer_button(from, egui::PointerButton::Primary, true),
        );
        for step in 1..=6 {
            run_frame(ctx, editor, moved(from + (to - from) * (step as f32 / 6.0)));
        }
    }

    fn end_drag(ctx: &egui::Context, editor: &mut Editor, at: egui::Pos2) {
        run_frame(
            ctx,
            editor,
            pointer_button(at, egui::PointerButton::Primary, false),
        );
        run_frame(ctx, editor, vec![]);
    }

    fn secondary_click(ctx: &egui::Context, editor: &mut Editor, at: egui::Pos2) {
        run_frame(ctx, editor, moved(at));
        run_frame(
            ctx,
            editor,
            pointer_button(at, egui::PointerButton::Secondary, true),
        );
        run_frame(
            ctx,
            editor,
            pointer_button(at, egui::PointerButton::Secondary, false),
        );
        run_frame(ctx, editor, vec![]);
    }

    /// Finds a menu line by name, telling it apart from a toolbar button with the same one
    /// by its width.
    fn menu_item(
        ctx: &egui::Context,
        editor: &mut Editor,
        name: &str,
    ) -> Option<(egui::Rect, bool)> {
        for _ in 0..3 {
            run_frame(ctx, editor, vec![]);
        }
        named_nodes(ctx, editor)
            .into_iter()
            .find(|(label, rect, _)| label == name && rect.width() > 180.0)
            .map(|(_, rect, disabled)| (rect, disabled))
    }

    fn click_menu_item(ctx: &egui::Context, editor: &mut Editor, name: &str) {
        let (rect, disabled) =
            menu_item(ctx, editor, name).unwrap_or_else(|| panic!("the menu has no {name}"));
        assert!(!disabled, "{name} is disabled");
        click(ctx, editor, rect.center());
    }

    /// How wide one second is on the timeline right now.
    fn pixels_per_second(editor: &Editor) -> f32 {
        let scale = editor.timeline_state.scale;
        scale.x_of(Ticks::from_seconds(1.0)) - scale.x_of(Ticks(0))
    }

    /// The tab on the rail, which shares its name with the panel's title.
    fn click_rail_tab(ctx: &egui::Context, editor: &mut Editor) {
        let rect = named_nodes(ctx, editor)
            .into_iter()
            .find(|(label, rect, _)| label == "My media" && rect.right() <= RAIL_WIDTH)
            .expect("the rail has its tab")
            .1;
        click(ctx, editor, rect.center());
    }

    fn let_panel_finish(ctx: &egui::Context, editor: &mut Editor) {
        for _ in 0..30 {
            run_frame(ctx, editor, vec![]);
        }
    }

    #[test]
    fn the_gaps_between_panels_are_painted() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);

        let output = run_frame(&ctx, &mut editor, vec![]);

        let screen = ctx.content_rect();
        let painted = output.shapes.iter().any(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect) => {
                rect.rect == screen && rect.fill == theme::palette(&ctx).bg_app
            }
            _ => false,
        });
        assert!(painted);
    }

    #[test]
    fn panels_sit_apart_from_each_other_and_the_window_edge() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        let screen = ctx.content_rect();
        let track = editor.timeline_state.track;

        // The timeline's own margin sits inside the card, and the card stops short of the edge.
        assert!(track.right() < screen.right() - f32::from(PANEL_GAP));
        assert!(track.bottom() < screen.bottom() - f32::from(PANEL_GAP));
        assert!(editor.media_panel_rect.left() >= RAIL_WIDTH);
    }

    #[test]
    fn the_close_button_hides_the_panel_and_gives_the_room_to_the_rest() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let wide_before = editor.timeline_state.track.width();
        let position = editor.position();
        let zoom = pixels_per_second(&editor);

        click_named(&ctx, &mut editor, "Close media panel");
        let_panel_finish(&ctx, &mut editor);

        assert!(!editor.media_panel_open);
        assert_eq!(editor.media_panel_rect, egui::Rect::NOTHING);
        let names = named_nodes(&ctx, &mut editor);
        assert!(!names.iter().any(|(label, ..)| label == "Add videos"));
        assert!(editor.timeline_state.track.width() > wide_before + 200.0);
        assert!((pixels_per_second(&editor) - zoom).abs() < 0.05, "{zoom}");
        assert_eq!(editor.position(), position);
    }

    #[test]
    fn the_rail_reopens_the_panel_at_the_width_it_had() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip()]);
        let edge = editor.media_panel_rect.right();
        pointer_drag(
            &ctx,
            &mut editor,
            pos2(edge, 300.0),
            pos2(edge + 100.0, 300.0),
        );
        let width = editor.media_panel_rect.width();

        click_rail_tab(&ctx, &mut editor);
        let_panel_finish(&ctx, &mut editor);
        assert!(!editor.media_panel_open);

        click_rail_tab(&ctx, &mut editor);
        let_panel_finish(&ctx, &mut editor);
        assert!(editor.media_panel_open);
        assert!((editor.media_panel_rect.width() - width).abs() < 1.0);
    }

    #[test]
    fn the_panel_slides_quickly_rather_than_jumping() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip()]);
        editor.media_panel_open = false;
        let frame_seconds = 1.0 / 60.0;
        let mut frames = 0;
        let mut seen_partway = false;
        let timeline_left = |editor: &Editor| editor.timeline_panel_rect.left();
        let open_left = timeline_left(&editor);
        loop {
            run_frame(&ctx, &mut editor, vec![]);
            frames += 1;
            let left = timeline_left(&editor);
            if editor.media_panel_rect == egui::Rect::NOTHING {
                break;
            }
            seen_partway |= left < open_left - 1.0;
            assert!(frames < 60, "the panel never finished closing");
        }

        assert!(seen_partway, "it should slide");
        let seconds = frames as f32 * frame_seconds;
        assert!(seconds < 0.25, "it took {seconds} seconds");
    }

    #[test]
    fn dragging_the_panels_edge_never_closes_it() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip()]);
        let edge = editor.media_panel_rect.right();

        pointer_drag(&ctx, &mut editor, pos2(edge, 300.0), pos2(0.0, 300.0));
        let_panel_finish(&ctx, &mut editor);

        assert!(editor.media_panel_open);
        assert!(editor.media_panel_rect.width() >= MEDIA_PANEL_MIN_WIDTH - 1.0);
    }

    #[test]
    fn adding_a_video_opens_a_closed_panel() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        editor.media_panel_open = false;

        editor.import(&ctx, &[test_clip().to_path_buf()]);

        assert!(editor.media_panel_open);
    }

    #[test]
    fn select_all_ticks_every_video_in_order_and_deselect_clears_them() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip(), second_clip()]);
        let ids: Vec<MediaId> = editor.media.iter().map(|entry| entry.id).collect();

        click_named(&ctx, &mut editor, "Select all");
        assert_eq!(editor.selected_media, ids);
        assert_eq!(editor.card_state(ids[1]).order, Some(2));

        click_named(&ctx, &mut editor, "Deselect all");
        assert!(editor.selected_media.is_empty());
        assert_eq!(editor.card_state(ids[1]).order, None);
    }

    #[test]
    fn ticking_one_card_at_a_time_numbers_them_in_that_order() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip(), second_clip()]);
        let ids: Vec<MediaId> = editor.media.iter().map(|entry| entry.id).collect();
        let boxes: Vec<egui::Pos2> = editor
            .card_thumbnails
            .iter()
            .map(|(_, thumbnail)| thumbnail.min + vec2(12.0, 12.0))
            .collect();

        click(&ctx, &mut editor, boxes[1]);
        click(&ctx, &mut editor, boxes[0]);

        assert_eq!(editor.selected_media, [ids[1], ids[0]]);
        assert_eq!(editor.card_state(ids[1]).order, Some(1));
        assert_eq!(editor.card_state(ids[0]).order, Some(2));

        click(&ctx, &mut editor, boxes[1]);
        assert_eq!(editor.selected_media, [ids[0]]);
        assert_eq!(editor.card_state(ids[0]).order, Some(1));
    }

    #[test]
    fn select_all_keeps_the_numbers_already_given() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip(), second_clip()]);
        let ids: Vec<MediaId> = editor.media.iter().map(|entry| entry.id).collect();
        editor.toggle_media(ids[1]);
        run_frame(&ctx, &mut editor, vec![]);

        click_named(&ctx, &mut editor, "Select all");

        assert_eq!(editor.selected_media, [ids[1], ids[0]]);
    }

    #[test]
    fn adding_the_ticked_videos_follows_their_order_as_one_undo_step() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip(), second_clip()]);
        let ids: Vec<MediaId> = editor.media.iter().map(|entry| entry.id).collect();
        editor.toggle_media(ids[1]);
        editor.toggle_media(ids[0]);
        run_frame(&ctx, &mut editor, vec![]);

        click_named(&ctx, &mut editor, "Add selected to timeline");

        let clips = editor.project.tracks.played().clips();
        assert_eq!(
            clips.iter().map(|clip| clip.media).collect::<Vec<_>>(),
            [ids[1], ids[0]]
        );
        assert!(editor.project.tracks.gaps().is_empty());
        assert!(editor.selected_media.is_empty());
        assert_eq!(editor.selected, Some(clips[1].id));

        editor.undo();
        assert_eq!(clip_count(&editor), 0);
    }

    #[test]
    fn removing_ticked_videos_that_are_unused_needs_no_question() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip(), second_clip()]);
        click_named(&ctx, &mut editor, "Select all");

        click_named(&ctx, &mut editor, "Remove selected");

        assert!(editor.confirm_remove.is_empty());
        assert_eq!(editor.visible_media().count(), 0);
        assert!(editor.selected_media.is_empty());

        editor.undo();
        assert_eq!(editor.visible_media().count(), 2);
    }

    #[test]
    fn removing_ticked_videos_in_use_asks_first_and_goes_as_one_undo_step() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip(), second_clip()]);
        click_named(&ctx, &mut editor, "Select all");
        click_named(&ctx, &mut editor, "Remove selected");
        assert_eq!(editor.confirm_remove.len(), 2);
        assert_eq!(clip_count(&editor), 2);
        for _ in 0..12 {
            run_frame(&ctx, &mut editor, vec![]);
        }

        click_named(&ctx, &mut editor, "Remove");

        assert_eq!(clip_count(&editor), 0);
        assert_eq!(editor.visible_media().count(), 0);

        editor.undo();
        assert_eq!(clip_count(&editor), 2);
        assert_eq!(editor.visible_media().count(), 2);
    }

    #[test]
    fn removing_one_video_unticks_it() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip(), second_clip()]);
        let ids: Vec<MediaId> = editor.media.iter().map(|entry| entry.id).collect();
        editor.toggle_media(ids[0]);
        editor.toggle_media(ids[1]);

        editor.remove_media(ids[0]);

        assert_eq!(editor.selected_media, [ids[1]]);
    }

    #[test]
    fn a_card_shows_when_its_video_is_on_the_timeline_and_when_its_clip_is_selected() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip(), second_clip()]);
        let ids: Vec<MediaId> = editor.media.iter().map(|entry| entry.id).collect();
        let on_timeline = |nodes: &[(String, egui::Rect, bool)]| {
            nodes
                .iter()
                .filter(|(label, ..)| label == "On the timeline")
                .count()
        };
        assert_eq!(on_timeline(&named_nodes(&ctx, &mut editor)), 0);

        editor.add_to_timeline(ids[0]);
        editor.add_to_timeline(ids[1]);
        assert_eq!(on_timeline(&named_nodes(&ctx, &mut editor)), 2);
        assert!(editor.card_state(ids[1]).linked);
        assert!(!editor.card_state(ids[0]).linked);

        let first = editor.project.tracks.played().clips()[0].id;
        editor.selected = Some(first);
        assert!(editor.card_state(ids[0]).linked);
        assert!(!editor.card_state(ids[1]).linked);
        assert!(editor.card_state(ids[1]).used);

        editor.selected = None;
        assert!(!editor.card_state(ids[0]).linked);
    }

    #[test]
    fn hovering_a_clip_is_noticed_and_stops_when_the_pointer_leaves() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let scale = editor.timeline_state.scale;
        let track = editor.timeline_state.track;
        let on_clip = pos2(scale.x_of(Ticks::from_seconds(2.0)), track.center().y);

        run_frame(&ctx, &mut editor, moved(on_clip));
        assert_eq!(
            editor.timeline_state.hovered_clip,
            Some(editor.project.tracks.played().clips()[0].id)
        );

        run_frame(&ctx, &mut editor, moved(pos2(700.0, 100.0)));
        assert_eq!(editor.timeline_state.hovered_clip, None);
    }

    #[test]
    fn a_snapped_drag_shows_a_line_at_what_it_lined_up_with() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip(), second_clip()]);
        let scale = editor.timeline_state.scale;
        let track = editor.timeline_state.track;
        let y = track.center().y;
        let x_of = |seconds: f64| scale.x_of(Ticks::from_seconds(seconds));
        // Pick the second clip up by its middle and move it far enough to count as a drag,
        // but close enough to the first clip to snap back against it.
        let from = pos2(x_of(6.0), y);
        let to = from + vec2(7.0, 0.0);

        begin_drag(&ctx, &mut editor, from, to);
        let first_end = editor.project.tracks.played().clips()[0].end();
        assert_eq!(editor.timeline_state.snap_line, Some(first_end));
        end_drag(&ctx, &mut editor, to);
        assert_eq!(editor.timeline_state.snap_line, None);
    }

    #[test]
    fn a_drag_far_from_anything_shows_no_snap_line() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let scale = editor.timeline_state.scale;
        let y = editor.timeline_state.track.center().y;
        let from = pos2(scale.x_of(Ticks::from_seconds(2.0)), y);
        let to = pos2(scale.x_of(Ticks::from_seconds(3.0)), y);
        editor.seek(Ticks(0));
        run_frame(&ctx, &mut editor, vec![]);

        begin_drag(&ctx, &mut editor, from, to);

        assert_eq!(editor.timeline_state.snap_line, None);
        end_drag(&ctx, &mut editor, to);
    }

    #[test]
    fn trimming_shows_the_clips_start_and_end_until_it_finishes() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let scale = editor.timeline_state.scale;
        let y = editor.timeline_state.track.center().y;
        let from = pos2(scale.x_of(Ticks::from_seconds(4.0)) - 4.0, y);
        let to = pos2(scale.x_of(Ticks::from_seconds(3.0)), y);

        begin_drag(&ctx, &mut editor, from, to);

        let clip = editor.project.tracks.played().clips()[0];
        // The readout was drawn before this frame's trim, so it can trail by one step.
        let (start, end) = editor.timeline_state.trim_readout.expect("it is showing");
        assert_eq!(start, clip.start);
        assert!((end.as_seconds() - clip.end().as_seconds()).abs() < 0.2);

        end_drag(&ctx, &mut editor, to);
        assert_eq!(editor.timeline_state.trim_readout, None);
    }

    #[test]
    fn dragging_the_playhead_shows_its_time_until_it_finishes() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let scale = editor.timeline_state.scale;
        let y = editor.timeline_state.track.top() - 12.0;
        let from = pos2(scale.x_of(Ticks::from_seconds(1.0)), y);
        let to = pos2(scale.x_of(Ticks::from_seconds(3.0)), y);
        assert!(!editor.timeline_state.playhead_bubble);

        begin_drag(&ctx, &mut editor, from, to);
        assert!(editor.timeline_state.playhead_bubble);

        end_drag(&ctx, &mut editor, to);
        assert!(!editor.timeline_state.playhead_bubble);
    }

    #[test]
    fn hovering_a_gap_on_the_ruler_or_the_track_finds_it() {
        let ctx = context();
        let mut editor = with_leading_gap(&ctx);
        let scale = editor.timeline_state.scale;
        let track = editor.timeline_state.track;
        let x = scale.x_of(Ticks::from_seconds(0.5));
        let gap = editor.project.tracks.gaps()[0];

        for y in [track.top() - 12.0, track.center().y] {
            run_frame(&ctx, &mut editor, moved(pos2(x, y)));
            assert_eq!(editor.timeline_state.hovered_gap, Some(gap));
        }

        run_frame(&ctx, &mut editor, moved(pos2(700.0, 100.0)));
        assert_eq!(editor.timeline_state.hovered_gap, None);
    }

    #[test]
    fn a_narrow_gap_still_offers_its_button_as_a_symbol() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let clip = editor.project.tracks.played().clips()[0].id;
        let scale = editor.timeline_state.scale;
        let wanted = 40.0 / pixels_per_second(&editor);
        editor.project.tracks.move_clip(
            clip,
            Ticks::from_seconds(f64::from(wanted)),
            TrackTarget::Existing(0),
        );
        run_frame(&ctx, &mut editor, vec![]);
        let track = editor.timeline_state.track;
        let middle = pos2(
            scale.x_of(Ticks::from_seconds(f64::from(wanted) / 2.0)),
            track.center().y,
        );
        run_frame(&ctx, &mut editor, moved(middle));
        run_frame(&ctx, &mut editor, moved(middle));

        click(&ctx, &mut editor, middle);

        assert!(editor.project.tracks.gaps().is_empty());
    }

    #[test]
    fn right_clicking_a_clip_selects_it_without_moving_the_playhead() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip(), second_clip()]);
        let first = editor.project.tracks.played().clips()[0].id;
        let scale = editor.timeline_state.scale;
        let y = editor.timeline_state.track.center().y;
        editor.seek(Ticks::from_seconds(6.0));
        run_frame(&ctx, &mut editor, vec![]);
        let position = editor.position();

        secondary_click(
            &ctx,
            &mut editor,
            pos2(scale.x_of(Ticks::from_seconds(1.0)), y),
        );

        assert_eq!(editor.selected, Some(first));
        assert_eq!(editor.position(), position);
        assert_eq!(
            editor.timeline_state.menu_target,
            Some(timeline::MenuTarget::Clip(first))
        );
    }

    #[test]
    fn the_menu_only_offers_split_when_the_playhead_is_on_that_clip() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip(), second_clip()]);
        let scale = editor.timeline_state.scale;
        let y = editor.timeline_state.track.center().y;
        let on_first = pos2(scale.x_of(Ticks::from_seconds(1.0)), y);
        editor.seek(Ticks::from_seconds(6.0));
        run_frame(&ctx, &mut editor, vec![]);

        secondary_click(&ctx, &mut editor, on_first);
        let (_, disabled) = menu_item(&ctx, &mut editor, "Split").expect("the menu has Split");
        assert!(disabled);
        run_frame(&ctx, &mut editor, press(Key::Escape));
        let_panel_finish(&ctx, &mut editor);

        editor.seek(Ticks::from_seconds(1.0));
        run_frame(&ctx, &mut editor, vec![]);
        secondary_click(&ctx, &mut editor, on_first);
        wait_for_frames(&ctx, &mut editor);
        click_menu_item(&ctx, &mut editor, "Split");
        assert_eq!(clip_count(&editor), 3);
    }

    /// Runs frames until the background decoding has caught up with the last seek.
    fn wait_for_frames(ctx: &egui::Context, editor: &mut Editor) {
        for _ in 0..10 {
            run_frame(ctx, editor, vec![]);
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn the_menu_duplicates_a_clip_as_one_undo_step() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let original = editor.project.tracks.played().clips()[0];
        let scale = editor.timeline_state.scale;
        let y = editor.timeline_state.track.center().y;

        secondary_click(
            &ctx,
            &mut editor,
            pos2(scale.x_of(Ticks::from_seconds(1.0)), y),
        );
        click_menu_item(&ctx, &mut editor, "Duplicate");

        let clips = editor.project.tracks.played().clips();
        assert_eq!(clips.len(), 2);
        assert_eq!(clips[1].start, original.end());
        assert_eq!(clips[1].source, original.source);
        assert_eq!(editor.selected, Some(clips[1].id));

        editor.undo();
        assert_eq!(clip_count(&editor), 1);
    }

    #[test]
    fn ctrl_d_duplicates_the_selected_clip() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);

        run_frame(&ctx, &mut editor, key_event(Key::D, Modifiers::COMMAND));

        assert_eq!(clip_count(&editor), 2);
    }

    #[test]
    fn ctrl_d_does_nothing_with_no_clip_selected() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.selected = None;

        run_frame(&ctx, &mut editor, key_event(Key::D, Modifiers::COMMAND));

        assert_eq!(clip_count(&editor), 1);
    }

    #[test]
    fn the_menu_deletes_a_clip_leaving_dead_space() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip(), second_clip()]);
        let scale = editor.timeline_state.scale;
        let y = editor.timeline_state.track.center().y;

        secondary_click(
            &ctx,
            &mut editor,
            pos2(scale.x_of(Ticks::from_seconds(1.0)), y),
        );
        click_menu_item(&ctx, &mut editor, "Delete");

        assert_eq!(clip_count(&editor), 1);
        assert_eq!(editor.project.tracks.gaps().len(), 1);
    }

    #[test]
    fn the_menu_on_a_gap_removes_that_gap() {
        let ctx = context();
        let mut editor = with_leading_gap(&ctx);
        let scale = editor.timeline_state.scale;
        // On the ruler, since the gap's own button takes up the middle of the track.
        let y = editor.timeline_state.track.top() - 12.0;

        secondary_click(
            &ctx,
            &mut editor,
            pos2(scale.x_of(Ticks::from_seconds(0.2)), y),
        );
        assert!(matches!(
            editor.timeline_state.menu_target,
            Some(timeline::MenuTarget::Gap(_))
        ));
        click_menu_item(&ctx, &mut editor, "Remove gap");

        assert!(editor.project.tracks.gaps().is_empty());
    }

    #[test]
    fn right_clicking_empty_timeline_opens_no_menu() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let scale = editor.timeline_state.scale;
        let y = editor.timeline_state.track.center().y;

        secondary_click(
            &ctx,
            &mut editor,
            pos2(scale.x_of(Ticks::from_seconds(5.0)), y),
        );

        assert_eq!(editor.timeline_state.menu_target, None);
        assert!(menu_item(&ctx, &mut editor, "Remove all gaps").is_none());
    }

    #[test]
    fn the_tool_removes_all_gaps_as_one_undo_step() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip(), second_clip()]);
        let ids: Vec<ClipId> = editor
            .project
            .tracks
            .played()
            .clips()
            .iter()
            .map(|clip| clip.id)
            .collect();
        editor
            .project
            .tracks
            .move_clip(ids[1], Ticks::from_seconds(9.0), TrackTarget::Existing(0));
        editor
            .project
            .tracks
            .move_clip(ids[0], Ticks::from_seconds(1.0), TrackTarget::Existing(0));
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.project.tracks.gaps().len(), 2);
        let moved_project = editor.project.clone();

        click_named(&ctx, &mut editor, "Remove all gaps");

        assert!(editor.project.tracks.gaps().is_empty());
        editor.undo();
        assert_eq!(editor.project, moved_project);
    }

    #[test]
    fn the_zoom_card_has_its_own_row_and_never_covers_a_clip() {
        let ctx = context();
        let editor = with_videos(&ctx, &[test_clip()]);
        let track = editor.timeline_state.track;

        for button in editor.timeline_state.zoom_buttons {
            assert!(button.top() >= track.bottom(), "{button:?} {track:?}");
        }
    }

    #[test]
    fn exporting_with_no_gaps_goes_straight_to_the_choices() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);

        click_named(&ctx, &mut editor, "Export");

        assert!(matches!(editor.export_state, ExportState::Choosing));
    }

    #[test]
    fn exporting_with_gaps_warns_first_from_the_button_and_the_shortcut() {
        let ctx = context();
        let mut editor = with_leading_gap(&ctx);

        click_named(&ctx, &mut editor, "Export");
        assert!(matches!(editor.export_state, ExportState::GapWarning));

        editor.export_state = ExportState::Closed;
        run_frame(&ctx, &mut editor, key_event(Key::E, Modifiers::COMMAND));
        assert!(matches!(editor.export_state, ExportState::GapWarning));
    }

    /// Opens the gap warning and waits for it to measure itself.
    fn open_gap_warning(ctx: &egui::Context) -> Editor {
        let mut editor = with_leading_gap(ctx);
        editor.open_export();
        for _ in 0..12 {
            run_frame(ctx, &mut editor, vec![]);
        }
        editor
    }

    /// Presses a button in a dialog, which may share its name with one behind the dialog.
    fn click_in_dialog(ctx: &egui::Context, editor: &mut Editor, name: &str) {
        let dialog = ctx
            .memory(|memory| memory.area_rect(egui::Id::unique("export_dialog")))
            .expect("the dialog is on screen");
        let rect = named_nodes(ctx, editor)
            .into_iter()
            .find(|(label, rect, _)| label == name && dialog.contains_rect(*rect))
            .unwrap_or_else(|| panic!("the dialog has no {name}"))
            .1;
        click(ctx, editor, rect.center());
    }

    #[test]
    fn the_gap_warning_can_remove_the_gaps_and_carry_on() {
        let ctx = context();
        let mut editor = open_gap_warning(&ctx);

        click_in_dialog(&ctx, &mut editor, "Remove all gaps");

        assert!(editor.project.tracks.gaps().is_empty());
        assert!(matches!(editor.export_state, ExportState::Choosing));
        editor.export_state = ExportState::Closed;
        editor.undo();
        assert_eq!(editor.project.tracks.gaps().len(), 1);
    }

    #[test]
    fn the_gap_warning_can_keep_the_gaps_and_carry_on() {
        let ctx = context();
        let mut editor = open_gap_warning(&ctx);

        click_in_dialog(&ctx, &mut editor, "Keep gaps");

        assert_eq!(editor.project.tracks.gaps().len(), 1);
        assert!(matches!(editor.export_state, ExportState::Choosing));
    }

    #[test]
    fn the_gap_warning_can_be_cancelled() {
        let ctx = context();
        let mut editor = open_gap_warning(&ctx);

        click_in_dialog(&ctx, &mut editor, "Cancel");

        assert_eq!(editor.project.tracks.gaps().len(), 1);
        assert!(matches!(editor.export_state, ExportState::Closed));
    }

    fn click(ctx: &egui::Context, editor: &mut Editor, at: egui::Pos2) {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        run_frame(ctx, editor, vec![egui::Event::PointerMoved(at)]);
        run_frame(ctx, editor, vec![button(true)]);
        run_frame(ctx, editor, vec![button(false)]);
        run_frame(ctx, editor, vec![]);
    }

    fn clip_count(editor: &Editor) -> usize {
        editor.project.tracks.played().clips().len()
    }

    #[test]
    fn the_empty_screen_draws_without_a_video() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);

        for _ in 0..2 {
            run_frame(&ctx, &mut editor, vec![]);
        }

        assert!(editor.project.tracks.is_empty());
    }

    #[test]
    fn adding_a_video_only_puts_it_in_the_panel() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip(), second_clip()]);

        assert_eq!(editor.visible_media().count(), 2);
        assert_eq!(clip_count(&editor), 0);
        assert!(!editor.history.can_undo());

        editor.seek(Ticks::from_seconds(1.0));
        assert_eq!(editor.position(), Ticks(0));
    }

    #[test]
    fn the_plus_on_a_card_adds_the_video_to_the_end_of_the_timeline() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip(), second_clip()]);

        for (index, expected_start) in [(0, 0.0), (1, 4.0)] {
            let thumbnail = editor.card_thumbnails[index].1;
            let plus = thumbnail.right_bottom() - vec2(20.0, 20.0);
            click(&ctx, &mut editor, plus);

            let clip = editor.project.tracks.played().clips()[index];
            assert_eq!(clip.media, editor.card_thumbnails[index].0);
            assert!((clip.start.as_seconds() - expected_start).abs() < 0.1);
        }
    }

    #[test]
    fn the_cross_on_a_card_removes_the_video_from_the_project() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip()]);
        let thumbnail = editor.card_thumbnails[0].1;

        click(&ctx, &mut editor, thumbnail.right_top() + vec2(-20.0, 20.0));

        assert_eq!(editor.visible_media().count(), 0);
    }

    #[test]
    fn clicking_the_middle_of_a_thumbnail_does_not_add_the_video() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip()]);
        let thumbnail = editor.card_thumbnails[0].1;

        click(&ctx, &mut editor, thumbnail.center());

        assert_eq!(clip_count(&editor), 0);
    }

    #[test]
    fn dragging_a_card_onto_the_timeline_places_it_where_it_was_dropped() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip()]);
        let from = editor.card_thumbnails[0].1.center();
        let scale = editor.timeline_state.scale;
        let to = pos2(
            scale.x_of(Ticks::from_seconds(3.0)),
            editor.timeline_state.track.center().y,
        );

        pointer_drag(&ctx, &mut editor, from, to);

        assert_eq!(clip_count(&editor), 1);
        let clip = editor.project.tracks.played().clips()[0];
        assert!(
            (clip.start.as_seconds() - 3.0).abs() < 0.1,
            "it landed at {}",
            clip.start.as_seconds()
        );
    }

    #[test]
    fn dragging_a_card_somewhere_else_does_nothing() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip()]);
        let from = editor.card_thumbnails[0].1.center();

        pointer_drag(&ctx, &mut editor, from, pos2(700.0, 200.0));

        assert_eq!(clip_count(&editor), 0);
    }

    #[test]
    fn a_video_can_be_added_from_the_panel_more_than_once() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let id = editor.media[0].id;

        editor.add_to_timeline(id);

        assert_eq!(clip_count(&editor), 2);
        assert_eq!(
            editor.selected,
            Some(editor.project.tracks.played().clips()[1].id)
        );
    }

    #[test]
    fn dropping_a_video_past_the_end_leaves_dead_space_before_it() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.import(&ctx, &[second_clip().to_path_buf()]);
        let second = editor.media[1].id;

        editor.drop_on_timeline(second, Ticks::from_seconds(6.0), TrackTarget::Existing(0));

        let clip = editor.project.tracks.played().clips()[1];
        assert_eq!(clip.media, second);
        assert_eq!(clip.start, Ticks::from_seconds(6.0));
        assert_eq!(editor.project.tracks.gaps().len(), 1);
    }

    #[test]
    fn dropping_a_video_on_a_clip_pushes_later_clips_along() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip(), test_clip()]);
        let id = editor.media[0].id;

        editor.drop_on_timeline(id, Ticks::from_seconds(1.0), TrackTarget::Existing(0));

        let starts: Vec<f64> = editor
            .project
            .tracks
            .played()
            .clips()
            .iter()
            .map(|clip| clip.start.as_seconds().round())
            .collect();
        assert_eq!(starts, [0.0, 4.0, 8.0]);
    }

    #[test]
    fn a_broken_file_leaves_the_project_alone_and_explains() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let broken = std::env::temp_dir().join("bareclip-not-a-video.mp4");
        std::fs::write(&broken, "not a video").expect("the file can be written");

        editor.import(&ctx, &[broken]);

        assert_eq!(editor.visible_media().count(), 1);
        assert!(
            editor
                .notice
                .as_deref()
                .is_some_and(|notice| notice.contains("could not be opened"))
        );
    }

    #[test]
    fn s_splits_the_clip_at_the_frame_on_screen() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);

        editor.seek(Ticks::from_seconds(1.51));
        run_frame(&ctx, &mut editor, press(Key::S));

        assert_eq!(clip_count(&editor), 2);
        let clips = editor.project.tracks.played().clips();
        // Frame 45 was on screen at 1.51 seconds, so the second part starts with it.
        assert!((clips[1].source.start.as_seconds() - 1.5).abs() < 0.001);
        assert_eq!(clips[0].source.end, clips[1].source.start);
        assert_eq!(editor.selected, Some(clips[1].id));
    }

    #[test]
    fn splitting_at_a_clips_very_start_explains_itself() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);

        editor.seek(Ticks(0));
        run_frame(&ctx, &mut editor, press(Key::S));

        assert_eq!(clip_count(&editor), 1);
        assert!(
            editor
                .notice
                .as_deref()
                .is_some_and(|notice| notice.contains("inside a clip"))
        );
    }

    #[test]
    fn splitting_with_no_video_explains_itself() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);

        run_frame(&ctx, &mut editor, press(Key::S));

        assert!(
            editor
                .notice
                .as_deref()
                .is_some_and(|notice| notice.contains("Add a video"))
        );
    }

    #[test]
    fn deleting_a_clip_leaves_dead_space() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip(), second_clip()]);
        let first = editor.project.tracks.played().clips()[0].id;
        let second_start = editor.project.tracks.played().clips()[1].start;
        editor.selected = Some(first);

        run_frame(&ctx, &mut editor, press(Key::Delete));

        assert_eq!(clip_count(&editor), 1);
        assert!(editor.selected.is_none());
        assert_eq!(
            editor.project.tracks.played().clips()[0].start,
            second_start
        );
        assert_eq!(editor.project.tracks.gaps().len(), 1);
    }

    /// One clip with its first second trimmed away, and the playhead at the start.
    fn with_leading_gap(ctx: &egui::Context) -> Editor {
        let mut editor = with_videos(ctx, &[test_clip()]);
        let id = editor.project.tracks.played().clips()[0].id;
        let duration = editor.media[0].info.duration;
        editor.project.tracks.trim(
            id,
            ClipEdge::Start,
            Ticks::from_seconds(1.0),
            duration,
            Ticks::from_seconds(MIN_CLIP_SECONDS),
        );
        editor.selected = None;
        editor.seek(Ticks(0));
        run_frame(ctx, &mut editor, vec![]);
        editor
    }

    #[test]
    fn clicking_dead_space_selects_it_and_delete_removes_it() {
        let ctx = context();
        let mut editor = with_leading_gap(&ctx);
        let scale = editor.timeline_state.scale;
        let y = editor.timeline_state.track.center().y;

        click(
            &ctx,
            &mut editor,
            pos2(scale.x_of(Ticks::from_seconds(0.1)), y),
        );
        assert!(editor.selected_gap.is_some());
        assert!(editor.selected.is_none());
        run_frame(&ctx, &mut editor, press(Key::Delete));

        assert!(editor.project.tracks.gaps().is_empty());
        assert_eq!(editor.project.tracks.played().clips()[0].start, Ticks(0));
        assert!(editor.selected_gap.is_none());

        editor.undo();
        assert_eq!(editor.project.tracks.gaps().len(), 1);
    }

    #[test]
    fn every_button_that_is_only_a_symbol_has_a_name() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);

        let nodes = named_nodes(&ctx, &mut editor);

        for name in [
            "Undo",
            "Redo",
            "Settings",
            "Go to start",
            "5 seconds back",
            "Play",
            "5 seconds forward",
            "Go to end",
            "Zoom out",
            "Zoom in",
            "Fit",
            "Add to timeline",
            "Remove from this project",
            "Close media panel",
            "My media",
        ] {
            assert!(
                nodes.iter().any(|(label, ..)| label == name),
                "nothing is named {name}"
            );
        }
    }

    #[test]
    fn named_actions_keep_their_words() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);

        let nodes = named_nodes(&ctx, &mut editor);

        for name in ["Export", "Add videos", "Split", "Delete"] {
            assert!(
                nodes.iter().any(|(label, ..)| label == name),
                "nothing is named {name}"
            );
        }
    }

    #[test]
    fn playback_buttons_are_disabled_with_nothing_to_play() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);

        let nodes = named_nodes(&ctx, &mut editor);

        for name in [
            "Go to start",
            "5 seconds back",
            "Play",
            "Go to end",
            "Export",
        ] {
            let (_, _, disabled) = nodes
                .iter()
                .find(|(label, ..)| label == name)
                .unwrap_or_else(|| panic!("nothing is named {name}"));
            assert!(*disabled, "{name} can be pressed with nothing to play");
        }
    }

    #[test]
    fn the_start_and_end_buttons_move_the_playhead() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let total = editor.project.tracks.duration();

        click_named(&ctx, &mut editor, "Go to end");
        assert_eq!(editor.position(), total);

        click_named(&ctx, &mut editor, "Go to start");
        assert_eq!(editor.position(), Ticks(0));
    }

    #[test]
    fn the_play_button_names_what_it_will_do() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);

        click_named(&ctx, &mut editor, "Play");

        let nodes = named_nodes(&ctx, &mut editor);
        assert!(nodes.iter().any(|(label, ..)| label == "Pause"));
        assert!(!nodes.iter().any(|(label, ..)| label == "Play"));
        click_named(&ctx, &mut editor, "Pause");
        assert!(!editor.preview.is_playing());
    }

    #[test]
    fn the_split_and_undo_buttons_work() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.seek(Ticks::from_seconds(1.0));

        click_named(&ctx, &mut editor, "Split");
        assert_eq!(editor.project.tracks.played().clips().len(), 2);

        click_named(&ctx, &mut editor, "Undo");
        assert_eq!(editor.project.tracks.played().clips().len(), 1);

        click_named(&ctx, &mut editor, "Redo");
        assert_eq!(editor.project.tracks.played().clips().len(), 2);
    }

    #[test]
    fn settings_change_the_theme() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);

        click_named(&ctx, &mut editor, "Settings");
        let_dialog_open(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Light");
        run_frame(&ctx, &mut editor, vec![]);

        assert_eq!(
            ctx.options(|options| options.theme_preference),
            egui::ThemePreference::Light
        );
    }

    #[test]
    fn the_export_dialog_switches_between_its_choices() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.export_state = ExportState::Choosing;
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.mode == CutMode::FrameExact);

        click_named(&ctx, &mut editor, "Fastest");
        assert!(editor.mode == CutMode::Keyframe);

        click_named(&ctx, &mut editor, "Frame-exact");
        assert!(editor.mode == CutMode::FrameExact);

        click_named(&ctx, &mut editor, "Cancel");
        assert!(matches!(editor.export_state, ExportState::Closed));
    }

    #[test]
    fn a_dialog_shrinks_to_what_it_is_showing() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let settle_dialog = |editor: &mut Editor| {
            for _ in 0..12 {
                run_frame(&ctx, editor, vec![]);
            }
            ctx.memory(|memory| memory.area_rect(egui::Id::unique("export_dialog")))
                .expect("the dialog is on screen")
        };

        editor.export_state = ExportState::Choosing;
        let choosing = settle_dialog(&mut editor);
        editor.export_state = ExportState::Failed("The disk is full.".to_owned());
        let failed = settle_dialog(&mut editor);

        assert!(
            failed.height() < choosing.height() - 100.0,
            "the short dialog is {} tall and the long one {}",
            failed.height(),
            choosing.height()
        );
    }

    #[test]
    fn the_remove_dialog_can_be_cancelled_or_confirmed() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip()]);
        let id = editor.media[0].id;

        editor.confirm_remove = vec![id];
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Cancel");
        assert!(editor.confirm_remove.is_empty());
        assert_eq!(editor.visible_media().count(), 1);

        editor.confirm_remove = vec![id];
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Remove");
        assert_eq!(editor.visible_media().count(), 0);
    }

    #[test]
    fn the_button_on_dead_space_removes_it() {
        let ctx = context();
        let mut editor = with_leading_gap(&ctx);
        let scale = editor.timeline_state.scale;
        let center = pos2(
            scale.x_of(Ticks::from_seconds(0.5)),
            editor.timeline_state.track.center().y,
        );

        click(&ctx, &mut editor, center);

        assert!(
            editor.project.tracks.gaps().is_empty(),
            "the gap is {} pixels wide",
            scale.x_of(Ticks::from_seconds(1.0)) - scale.x_of(Ticks(0))
        );
    }

    #[test]
    fn clicking_a_clip_deselects_dead_space() {
        let ctx = context();
        let mut editor = with_leading_gap(&ctx);
        editor.selected_gap = Some(Ticks::from_seconds(0.5));
        let scale = editor.timeline_state.scale;
        let y = editor.timeline_state.track.center().y;

        click(
            &ctx,
            &mut editor,
            pos2(scale.x_of(Ticks::from_seconds(2.0)), y),
        );

        assert!(editor.selected_gap.is_none());
        assert!(editor.selected.is_some());
    }

    #[test]
    fn the_preview_is_in_dead_space_when_the_playhead_is() {
        let ctx = context();
        let mut editor = with_leading_gap(&ctx);

        assert!(editor.preview.in_gap(editor.project.tracks.played()));

        editor.seek(Ticks::from_seconds(2.0));
        assert!(!editor.preview.in_gap(editor.project.tracks.played()));
    }

    #[test]
    fn splitting_in_dead_space_explains_itself() {
        let ctx = context();
        let mut editor = with_leading_gap(&ctx);

        run_frame(&ctx, &mut editor, press(Key::S));

        assert_eq!(clip_count(&editor), 1);
        assert!(
            editor
                .notice
                .as_deref()
                .is_some_and(|notice| notice.contains("inside a clip"))
        );
    }

    #[test]
    fn shift_arrows_can_step_through_dead_space() {
        let ctx = context();
        let mut editor = with_leading_gap(&ctx);
        editor.seek(Ticks::from_seconds(0.5));

        run_frame(&ctx, &mut editor, shift_key(Key::ArrowRight));
        assert!(editor.position() > Ticks::from_seconds(0.5));

        run_frame(&ctx, &mut editor, shift_key(Key::ArrowLeft));
        assert!((editor.position().as_seconds() - 0.5).abs() < 0.001);
    }

    #[test]
    fn shift_arrows_can_step_back_from_a_clip_into_dead_space() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip()]);
        let id = editor.media[0].id;
        editor.drop_on_timeline(id, Ticks::from_seconds(1.0), TrackTarget::Existing(0));
        editor.seek(Ticks::from_seconds(1.0));

        run_frame(&ctx, &mut editor, shift_key(Key::ArrowLeft));

        assert!(editor.position() < Ticks::from_seconds(1.0));
        assert!(editor.preview.in_gap(editor.project.tracks.played()));
    }

    #[test]
    fn exporting_includes_dead_space_at_its_full_length() {
        let ctx = context();
        let mut editor = with_leading_gap(&ctx);
        let output =
            std::env::temp_dir().join(format!("bareclip-ui-gap-{}.mp4", std::process::id()));
        let _ = std::fs::remove_file(&output);

        editor.start_export_to(&ctx, output.clone());
        wait_for_export(&ctx, &mut editor);

        let ExportState::Done { report, .. } = &editor.export_state else {
            panic!("the export did not finish");
        };
        assert_eq!(report.black_frames, 30);
        let info = probe(&output).expect("the export can be read");
        assert!((info.duration.as_seconds() - 4.0).abs() < 0.1);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn hovering_anywhere_on_the_timeline_shows_the_time_there() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let scale = editor.timeline_state.scale;
        let track = editor.timeline_state.track;

        // On a clip, on the ruler, and on the empty timeline after the video.
        for (time, y) in [
            (1.5, track.center().y),
            (2.5, track.top() - 12.0),
            (4.8, track.center().y),
        ] {
            let at = pos2(scale.x_of(Ticks::from_seconds(time)), y);
            run_frame(&ctx, &mut editor, vec![egui::Event::PointerMoved(at)]);
            let shown = editor.timeline_state.hover_time.expect("a time is shown");
            assert!((shown.as_seconds() - time).abs() < 0.02, "{shown:?}");
        }

        let away = pos2(track.center().x, 100.0);
        run_frame(&ctx, &mut editor, vec![egui::Event::PointerMoved(away)]);
        assert_eq!(editor.timeline_state.hover_time, None);
    }

    #[test]
    fn the_timeline_is_longer_than_the_video_and_the_video_is_part_of_it() {
        let ctx = context();
        let editor = with_videos(&ctx, &[test_clip()]);
        let scale = editor.timeline_state.scale;
        let end = scale.x_of(editor.project.tracks.duration());

        assert!(end < editor.timeline_state.track.right() - 50.0);
    }

    #[test]
    fn the_zoom_buttons_change_how_much_is_shown() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let start = editor.timeline_state.scale.visible;
        let fitted = editor.project.tracks.duration();
        let [zoom_out, zoom_in, fit] = editor.timeline_state.zoom_buttons;

        click(&ctx, &mut editor, zoom_out.center());
        assert!(editor.timeline_state.scale.visible > start);

        click(&ctx, &mut editor, fit.center());
        assert_eq!(editor.timeline_state.scale.visible, fitted);

        click(&ctx, &mut editor, zoom_in.center());
        assert!(editor.timeline_state.scale.visible < fitted);
    }

    #[test]
    fn dragging_the_scrollbar_scrolls_the_timeline() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let [_, zoom_in, _] = editor.timeline_state.zoom_buttons;
        for _ in 0..3 {
            click(&ctx, &mut editor, zoom_in.center());
        }
        editor.timeline_state.scroll = Ticks(0);
        run_frame(&ctx, &mut editor, vec![]);
        let bar = editor.timeline_state.scrollbar;
        let y = bar.center().y;

        pointer_drag(
            &ctx,
            &mut editor,
            pos2(bar.left() + 10.0, y),
            pos2(bar.left() + 200.0, y),
        );

        assert!(editor.timeline_state.scroll > Ticks(0));
        assert_eq!(clip_count(&editor), 1);
    }

    #[test]
    fn the_media_panel_can_be_dragged_wider_and_narrower() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip(), second_clip()]);
        let start = editor.media_panel_rect;
        assert!((start.width() - MEDIA_PANEL_WIDTH).abs() < 2.0, "{start:?}");
        let y = start.center().y;

        pointer_drag(
            &ctx,
            &mut editor,
            pos2(start.right(), y),
            pos2(start.right() + 200.0, y),
        );

        let wider = editor.media_panel_rect;
        assert!(
            (wider.width() - start.width() - 200.0).abs() < 4.0,
            "{wider:?}"
        );

        pointer_drag(&ctx, &mut editor, pos2(wider.right(), y), pos2(0.0, y));
        assert!(
            (editor.media_panel_rect.width() - MEDIA_PANEL_MIN_WIDTH).abs() < 2.0,
            "{:?}",
            editor.media_panel_rect
        );
    }

    #[test]
    fn a_wider_media_panel_fits_cards_side_by_side() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip(), second_clip()]);
        let [first, second] = [editor.card_thumbnails[0].1, editor.card_thumbnails[1].1];
        assert!(second.top() > first.bottom());
        let edge = editor.media_panel_rect.right();
        let y = editor.media_panel_rect.center().y;

        pointer_drag(&ctx, &mut editor, pos2(edge, y), pos2(edge + 250.0, y));
        run_frame(&ctx, &mut editor, vec![]);

        let [first, second] = [editor.card_thumbnails[0].1, editor.card_thumbnails[1].1];
        assert_eq!(first.top(), second.top());
        assert!(second.left() > first.right());
    }

    fn shift_key(key: Key) -> Vec<egui::Event> {
        key_event(key, Modifiers::SHIFT)
    }

    /// A long project: two clips of the same video, one after the other.
    fn two_clips(ctx: &egui::Context) -> Editor {
        with_videos(ctx, &[test_clip(), test_clip()])
    }

    #[test]
    fn arrow_keys_move_five_seconds_either_way() {
        let ctx = context();
        let mut editor = two_clips(&ctx);

        run_frame(&ctx, &mut editor, press(Key::ArrowRight));
        assert_eq!(editor.position(), Ticks::from_seconds(5.0));

        editor.seek(Ticks::from_seconds(6.0));
        run_frame(&ctx, &mut editor, press(Key::ArrowLeft));
        assert_eq!(editor.position(), Ticks::from_seconds(1.0));
    }

    #[test]
    fn arrow_keys_stop_at_the_ends_of_the_project() {
        let ctx = context();
        let mut editor = two_clips(&ctx);

        editor.seek(Ticks::from_seconds(1.0));
        run_frame(&ctx, &mut editor, press(Key::ArrowLeft));
        assert_eq!(editor.position(), Ticks(0));

        editor.seek(Ticks::from_seconds(7.0));
        run_frame(&ctx, &mut editor, press(Key::ArrowRight));
        assert_eq!(editor.position(), editor.project.tracks.duration());
    }

    #[test]
    fn shift_and_the_arrow_keys_step_one_frame() {
        let ctx = context();
        let mut editor = two_clips(&ctx);
        editor.seek(Ticks::from_seconds(1.0));
        let start = editor.position();

        run_frame(&ctx, &mut editor, shift_key(Key::ArrowRight));
        let moved = editor.position().as_seconds() - start.as_seconds();
        assert!((moved - 1.0 / 30.0).abs() < 0.001, "{moved}");

        run_frame(&ctx, &mut editor, shift_key(Key::ArrowLeft));
        assert_eq!(editor.position(), start);
    }

    #[test]
    fn the_buttons_beside_play_move_five_seconds() {
        let ctx = context();
        let mut editor = two_clips(&ctx);

        click_named(&ctx, &mut editor, "5 seconds forward");
        assert_eq!(editor.position(), Ticks::from_seconds(5.0));

        click_named(&ctx, &mut editor, "5 seconds back");
        assert_eq!(editor.position(), Ticks(0));
    }

    fn card_order(editor: &Editor) -> Vec<MediaId> {
        editor.listed_media().iter().map(|entry| entry.id).collect()
    }

    /// Three videos, renamed so their names run opposite to the order they were added.
    fn three_named(ctx: &egui::Context) -> Editor {
        let mut editor = in_panel_only(ctx, &[test_clip(), second_clip(), small_clip()]);
        for (entry, name) in editor
            .media
            .iter_mut()
            .zip(["c 10.mp4", "b 2.mp4", "A 1.mp4"])
        {
            entry.name = name.to_owned();
        }
        editor
    }

    #[test]
    fn videos_start_out_in_ascending_order_of_name() {
        let ctx = context();
        let editor = three_named(&ctx);

        assert_eq!(
            card_order(&editor),
            vec![MediaId(3), MediaId(2), MediaId(1)]
        );
    }

    #[test]
    fn numbers_in_names_sort_by_value() {
        let ctx = context();
        let mut editor = three_named(&ctx);
        for (entry, name) in editor.media.iter_mut().zip(["v10", "v9", "v100"]) {
            entry.name = name.to_owned();
        }

        assert_eq!(
            card_order(&editor),
            vec![MediaId(2), MediaId(1), MediaId(3)]
        );
    }

    #[test]
    fn the_cards_on_screen_follow_the_sort() {
        let ctx = context();
        let mut editor = three_named(&ctx);
        run_frame(&ctx, &mut editor, vec![]);

        let shown: Vec<MediaId> = editor.card_thumbnails.iter().map(|card| card.0).collect();

        assert_eq!(shown, card_order(&editor));
    }

    #[test]
    fn sorting_by_date_added_and_descending_change_the_order() {
        let ctx = context();
        let mut editor = three_named(&ctx);

        editor.media_sort = MediaSort::DateAdded;
        assert_eq!(
            card_order(&editor),
            vec![MediaId(1), MediaId(2), MediaId(3)]
        );

        editor.media_sort_descending = true;
        assert_eq!(
            card_order(&editor),
            vec![MediaId(3), MediaId(2), MediaId(1)]
        );
    }

    #[test]
    fn sorting_by_date_created_uses_the_files_dates() {
        let ctx = context();
        let mut editor = three_named(&ctx);
        let day = |days: u64| Some(SystemTime::UNIX_EPOCH + Duration::from_secs(86_400 * days));
        for (entry, days) in editor.media.iter_mut().zip([20, 30, 10]) {
            entry.created = day(days);
        }

        editor.media_sort = MediaSort::DateCreated;

        assert_eq!(
            card_order(&editor),
            vec![MediaId(3), MediaId(1), MediaId(2)]
        );
    }

    #[test]
    fn sorting_by_use_keeps_names_in_order_within_each_group() {
        let ctx = context();
        let mut editor = three_named(&ctx);
        editor.add_to_timeline(MediaId(1));

        editor.media_sort = MediaSort::UsedInVideo;
        assert_eq!(
            card_order(&editor),
            vec![MediaId(3), MediaId(2), MediaId(1)]
        );

        editor.media_sort_descending = true;
        assert_eq!(card_order(&editor)[0], MediaId(1));
    }

    #[test]
    fn the_sort_menu_lists_every_choice_and_applies_one() {
        let ctx = context();
        let mut editor = three_named(&ctx);
        click_named(&ctx, &mut editor, "Sort");

        for name in [
            "Name",
            "Date added",
            "Date created",
            "Used in video",
            "Ascending",
            "Descending",
        ] {
            assert!(menu_item(&ctx, &mut editor, name).is_some(), "{name}");
        }
        click_menu_item(&ctx, &mut editor, "Date added");

        assert_eq!(editor.media_sort, MediaSort::DateAdded);
    }

    #[test]
    fn the_filter_shows_videos_by_whether_they_are_on_the_timeline() {
        let ctx = context();
        let mut editor = three_named(&ctx);
        editor.add_to_timeline(MediaId(2));

        editor.media_filter = MediaFilter::OnTimeline;
        assert_eq!(card_order(&editor), vec![MediaId(2)]);

        editor.media_filter = MediaFilter::NotOnTimeline;
        assert_eq!(card_order(&editor), vec![MediaId(3), MediaId(1)]);

        editor.media_filter = MediaFilter::All;
        assert_eq!(card_order(&editor).len(), 3);
    }

    #[test]
    fn the_filter_menu_applies_a_choice() {
        let ctx = context();
        let mut editor = three_named(&ctx);
        click_named(&ctx, &mut editor, "Filter");

        click_menu_item(&ctx, &mut editor, "Not on the timeline");

        assert_eq!(editor.media_filter, MediaFilter::NotOnTimeline);
    }

    #[test]
    fn select_all_and_ticks_only_cover_what_the_filter_shows() {
        let ctx = context();
        let mut editor = three_named(&ctx);
        editor.add_to_timeline(MediaId(2));
        editor.selected_media = vec![MediaId(1), MediaId(2)];

        editor.media_filter = MediaFilter::OnTimeline;
        run_frame(&ctx, &mut editor, vec![]);

        assert_eq!(editor.selected_media, vec![MediaId(2)]);
    }

    #[test]
    fn a_filter_that_matches_nothing_says_so_and_keeps_the_panel_working() {
        let ctx = context();
        let mut editor = three_named(&ctx);

        editor.media_filter = MediaFilter::OnTimeline;
        run_frame(&ctx, &mut editor, vec![]);

        assert!(editor.card_thumbnails.is_empty());
        assert!(editor.visible_media().next().is_some());
    }

    #[test]
    fn a_few_videos_stack_and_many_go_side_by_side_across_then_down() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.card_thumbnails.len(), 1);

        let many: Vec<PathBuf> = (0..7).map(|_| test_clip().to_path_buf()).collect();
        editor.import(&ctx, &many);
        settle(&ctx, &mut editor);
        for _ in 0..3 {
            run_frame(&ctx, &mut editor, vec![]);
        }

        let cards: Vec<egui::Rect> = editor.card_thumbnails.iter().map(|card| card.1).collect();
        assert_eq!(cards[0].top(), cards[1].top(), "the first row fills first");
        assert!(cards[1].left() > cards[0].right());
        assert!(
            cards[2].top() > cards[0].bottom(),
            "then the next row starts"
        );
        assert_eq!(cards[2].left(), cards[0].left());
    }

    #[test]
    fn stage_space_around_the_picture_is_the_cards_colour() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let palette = theme::palette(&ctx);
        // Thumbnails keep their dark backing, so this looks with the panel closed.
        editor.media_panel_open = false;
        let_panel_finish(&ctx, &mut editor);

        let output = run_frame(&ctx, &mut editor, vec![]);

        let fills: Vec<Color32> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect) => Some(rect.fill),
                _ => None,
            })
            .collect();
        assert!(fills.contains(&palette.surface));
        assert!(!fills.contains(&palette.stage));
    }

    #[test]
    fn undo_export_and_the_window_buttons_all_exist_with_no_title_bar() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);

        let card_top = named_rect(&ctx, &mut editor, "Undo");
        let export = named_rect(&ctx, &mut editor, "Export");
        let close = named_rect(&ctx, &mut editor, "Close window");
        named_rect(&ctx, &mut editor, "Minimise window");
        named_rect(&ctx, &mut editor, "Maximise window");

        assert!(close.bottom() <= TITLE_STRIP_HEIGHT + 1.0);
        assert!(card_top.top() >= TITLE_STRIP_HEIGHT);
        assert!(export.top() >= TITLE_STRIP_HEIGHT);
        assert!(card_top.left() > editor.media_panel_rect.right());
    }

    #[test]
    fn the_window_resizes_from_its_edges_and_corners_only() {
        use egui::ResizeDirection as Direction;
        let window = egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 800.0));
        let direction = |x, y| resize_direction(window, pos2(x, y));

        assert_eq!(direction(500.0, 400.0), None);
        assert_eq!(direction(500.0, 1.0), Some(Direction::North));
        assert_eq!(direction(500.0, 799.0), Some(Direction::South));
        assert_eq!(direction(1.0, 400.0), Some(Direction::West));
        assert_eq!(direction(999.0, 400.0), Some(Direction::East));
        assert_eq!(direction(1.0, 1.0), Some(Direction::NorthWest));
        assert_eq!(direction(999.0, 10.0), Some(Direction::NorthEast));
        assert_eq!(direction(10.0, 799.0), Some(Direction::SouthWest));
        assert_eq!(direction(999.0, 799.0), Some(Direction::SouthEast));
    }

    fn let_timeline_finish(ctx: &egui::Context, editor: &mut Editor) {
        // Time only moves on between frames, so give the slide enough of them.
        for _ in 0..40 {
            run_frame(ctx, editor, vec![]);
        }
    }

    #[test]
    fn collapsing_the_timeline_gives_its_room_to_the_video_and_expanding_brings_it_back() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let_timeline_finish(&ctx, &mut editor);
        let open_height = editor.timeline_panel_rect.height();

        click_named(&ctx, &mut editor, "Collapse timeline");
        let_timeline_finish(&ctx, &mut editor);
        assert!(!editor.timeline_open);
        assert!(editor.timeline_panel_rect.height() < TRANSPORT_HEIGHT + 20.0);
        named_rect(&ctx, &mut editor, "Seek");

        click_named(&ctx, &mut editor, "Expand timeline");
        let_timeline_finish(&ctx, &mut editor);
        assert!(editor.timeline_open);
        assert!((editor.timeline_panel_rect.height() - open_height).abs() < 1.0);
    }

    #[test]
    fn collapsing_the_timeline_slides_rather_than_jumps() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let_timeline_finish(&ctx, &mut editor);
        let open_height = editor.timeline_panel_rect.height();

        editor.timeline_open = false;
        let input = |seconds: f64| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                pos2(0.0, 0.0),
                vec2(1280.0, 800.0),
            )),
            time: Some(seconds),
            predicted_dt: 0.05,
            ..Default::default()
        };
        let _ = ctx.run_ui(input(1000.0), |ui| editor.show(ui));
        let _ = ctx.run_ui(input(1000.05), |ui| editor.show(ui));

        let height = editor.timeline_panel_rect.height();
        assert!(height < open_height, "{height}");
        assert!(height > TRANSPORT_HEIGHT + 20.0, "{height}");
    }

    #[test]
    fn the_shortcut_collapses_and_expands_the_timeline() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);

        let keys = key_event(Key::Num3, Modifiers::COMMAND | Modifiers::ALT);
        run_frame(&ctx, &mut editor, keys.clone());
        assert!(!editor.timeline_open);

        run_frame(&ctx, &mut editor, keys);
        assert!(editor.timeline_open);
    }

    #[test]
    fn the_seek_bar_moves_the_playhead_while_the_timeline_is_collapsed() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.timeline_open = false;
        let_timeline_finish(&ctx, &mut editor);

        let bar = named_rect(&ctx, &mut editor, "Seek");
        click(&ctx, &mut editor, bar.center());

        let middle = editor.project.tracks.duration().as_seconds() / 2.0;
        assert!((editor.position().as_seconds() - middle).abs() < 0.1);
    }

    #[test]
    fn fit_makes_the_project_fill_the_width_of_the_timeline() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let scale = editor.timeline_state.scale;
        let before = scale.x_of(editor.project.tracks.duration()) - scale.left;
        assert!(before < scale.width * 0.8, "{before}");

        click_named(&ctx, &mut editor, "Fit");
        run_frame(&ctx, &mut editor, vec![]);

        let scale = editor.timeline_state.scale;
        let end = scale.x_of(editor.project.tracks.duration());
        assert!((end - (scale.left + scale.width)).abs() < 1.0);
    }

    #[test]
    fn a_taller_timeline_keeps_its_tracks_the_same_height() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let before = editor.timeline_state.track.height();

        editor.timeline_height = 500.0;
        let_timeline_finish(&ctx, &mut editor);

        assert_eq!(editor.timeline_state.track.height(), before);
        let layout = editor.timeline_state.layout;
        assert!(layout.new_lane_above().top() > layout.area.top());
    }

    fn track_of_first_clip(editor: &Editor) -> Option<usize> {
        let id = editor.project.tracks.tracks().iter().find_map(|track| {
            track
                .clips()
                .iter()
                .find(|clip| clip.media == editor.media[1].id)
                .map(|clip| clip.id)
        })?;
        editor.project.tracks.track_of(id)
    }

    /// Two videos on the timeline, one after the other on one track.
    fn two_videos(ctx: &egui::Context) -> Editor {
        with_videos(ctx, &[test_clip(), second_clip()])
    }

    /// The middle of the clip of the second video on screen.
    fn second_video_middle(editor: &Editor) -> egui::Pos2 {
        let clip = editor.project.tracks.played().clips()[1];
        let scale = editor.timeline_state.scale;
        let x = scale.x_of(Ticks((clip.start.0 + clip.end().0) / 2));
        pos2(x, editor.timeline_state.layout.track(0).center().y)
    }

    #[test]
    fn dragging_a_clip_onto_the_lane_below_makes_a_new_track() {
        let ctx = context();
        let mut editor = two_videos(&ctx);
        let from = second_video_middle(&editor);
        let lane = editor.timeline_state.layout.new_lane_below();

        let to = pos2(from.x, lane.center().y);
        begin_drag(&ctx, &mut editor, from, to);
        end_drag(&ctx, &mut editor, to);

        assert_eq!(editor.project.tracks.tracks().len(), 2);
        assert_eq!(track_of_first_clip(&editor), Some(1));
    }

    #[test]
    fn dragging_a_clip_onto_the_lane_above_makes_a_track_that_covers_the_other() {
        let ctx = context();
        let mut editor = two_videos(&ctx);
        let from = second_video_middle(&editor);
        let lane = editor.timeline_state.layout.new_lane_above();
        let start_x = editor.timeline_state.scale.x_of(Ticks::from_seconds(1.0));
        let grabbed = from.x
            - editor
                .timeline_state
                .scale
                .x_of(editor.project.tracks.played().clips()[1].start);

        let to = pos2(start_x + grabbed, lane.center().y);
        begin_drag(&ctx, &mut editor, from, to);
        end_drag(&ctx, &mut editor, to);

        assert_eq!(track_of_first_clip(&editor), Some(0));
        let on_top = editor.media[1].id;
        let played = editor.project.tracks.played().clips();
        assert_eq!(played.len(), 2);
        assert_eq!(played[1].media, on_top);
        assert_eq!(played[1].start, Ticks::from_seconds(1.0));
        assert_eq!(played[0].end(), Ticks::from_seconds(1.0));
    }

    #[test]
    fn undo_puts_a_clip_back_on_its_old_track() {
        let ctx = context();
        let mut editor = two_videos(&ctx);
        let before = editor.project.clone();
        let from = second_video_middle(&editor);
        let lane = editor.timeline_state.layout.new_lane_below();

        let to = pos2(from.x, lane.center().y);
        begin_drag(&ctx, &mut editor, from, to);
        end_drag(&ctx, &mut editor, to);
        assert_ne!(editor.project, before);

        editor.undo();

        assert_eq!(editor.project, before);
    }

    #[test]
    fn a_track_left_with_no_clips_goes_away() {
        let ctx = context();
        let mut editor = two_videos(&ctx);
        let moved = editor.project.tracks.played().clips()[1].id;
        editor
            .project
            .tracks
            .move_clip(moved, Ticks::from_seconds(20.0), TrackTarget::New(1));
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.project.tracks.tracks().len(), 2);

        editor.selected = Some(moved);
        editor.delete_selected();

        assert_eq!(editor.project.tracks.tracks().len(), 1);
    }

    #[test]
    fn dragging_a_card_onto_a_lane_puts_the_video_on_a_new_track() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.import(&ctx, &[second_clip().to_path_buf()]);
        settle(&ctx, &mut editor);
        run_frame(&ctx, &mut editor, vec![]);
        let second = editor.media[1].id;
        let card = editor
            .card_thumbnails
            .iter()
            .find(|card| card.0 == second)
            .expect("the card is listed")
            .1;
        let lane = editor.timeline_state.layout.new_lane_below();
        let x = editor.timeline_state.scale.x_of(Ticks::from_seconds(1.0));

        let to = pos2(x, lane.center().y);
        begin_drag(&ctx, &mut editor, card.center(), to);
        end_drag(&ctx, &mut editor, to);

        assert_eq!(track_of_first_clip(&editor), Some(1));
    }

    #[test]
    fn a_covered_clip_is_left_out_of_the_export() {
        let ctx = context();
        let mut editor = two_videos(&ctx);
        let on_top = editor.project.tracks.played().clips()[1].id;
        editor
            .project
            .tracks
            .move_clip(on_top, Ticks::from_seconds(1.0), TrackTarget::New(0));

        let parts = editor.export_parts().expect("the frames are read");

        let seconds = |ticks: Ticks| ticks.as_seconds();
        let ranges: Vec<(f64, f64)> = parts
            .iter()
            .map(|part| (seconds(part.range.start), seconds(part.range.end)))
            .collect();
        assert_eq!(ranges, vec![(0.0, 1.0), (0.0, 4.0)]);
        assert!(parts.iter().all(|part| part.gap_before == Ticks(0)));
    }

    #[test]
    fn a_format_only_a_hidden_clip_uses_does_not_stop_the_export() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip(), small_clip()]);
        assert!(editor.export_problem().is_some());

        let small = editor.project.tracks.played().clips()[1].id;
        editor.project.tracks.trim(
            small,
            ClipEdge::End,
            Ticks::from_seconds(1.0),
            Ticks::from_seconds(4.0),
            Ticks::from_seconds(0.05),
        );
        editor
            .project
            .tracks
            .move_clip(small, Ticks::from_seconds(1.0), TrackTarget::New(1));

        assert_eq!(editor.project.tracks.played().clips().len(), 1);
        assert!(editor.export_problem().is_none());
    }

    #[test]
    fn split_acts_on_a_selected_lower_clip_and_otherwise_on_the_one_on_screen() {
        let ctx = context();
        let mut editor = two_videos(&ctx);
        let clips = editor.project.tracks.played().clips().to_vec();
        let (low, high) = (clips[0].id, clips[1].id);
        editor
            .project
            .tracks
            .move_clip(high, Ticks::from_seconds(1.0), TrackTarget::New(0));
        editor.seek(Ticks::from_seconds(2.0));

        editor.selected = Some(low);
        editor.split_at_playhead();
        assert_eq!(editor.project.tracks.tracks()[1].clips().len(), 2);
        assert_eq!(editor.project.tracks.tracks()[0].clips().len(), 1);

        editor.selected = None;
        editor.seek(Ticks::from_seconds(2.0));
        editor.split_at_playhead();
        assert_eq!(editor.project.tracks.tracks()[0].clips().len(), 2);
    }

    #[test]
    fn cards_stack_while_they_fit_and_go_side_by_side_once_they_would_scroll() {
        let (width, room) = (MEDIA_CARD_MIN_WIDTH, 500.0);

        assert_eq!(media_columns(width, 400.0, room), 1);
        assert_eq!(media_columns(width, 500.0, room), 1);
        assert_eq!(media_columns(width, 501.0, room), 2);
    }

    #[test]
    fn a_wide_panel_fits_roomy_cards_side_by_side_whatever_their_number() {
        let two = MEDIA_CARD_MIN_WIDTH * 2.0 + MEDIA_CARD_SPACING;
        let three = MEDIA_CARD_MIN_WIDTH * 3.0 + MEDIA_CARD_SPACING * 2.0;

        assert_eq!(media_columns(two, 10.0, 500.0), 2);
        assert_eq!(media_columns(three, 10.0, 500.0), 3);
        assert_eq!(media_columns(three, 9000.0, 500.0), 3);
    }

    #[test]
    fn only_cards_narrower_than_a_roomy_one_are_compact() {
        assert!(!cards_are_compact(260.0, 1));
        assert!(cards_are_compact(260.0, 2));
        let two_roomy = MEDIA_CARD_MIN_WIDTH * 2.0 + MEDIA_CARD_SPACING;
        assert!(!cards_are_compact(two_roomy, 2));
    }

    #[test]
    fn the_timeline_can_be_dragged_taller_and_shorter() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let start = editor.timeline_panel_rect;
        assert!((start.height() - TIMELINE_HEIGHT).abs() < 2.0, "{start:?}");
        let x = start.center().x;

        pointer_drag(
            &ctx,
            &mut editor,
            pos2(x, start.top()),
            pos2(x, start.top() - 100.0),
        );
        let taller = editor.timeline_panel_rect;
        assert!(
            (taller.height() - start.height() - 100.0).abs() < 4.0,
            "{taller:?}"
        );

        pointer_drag(&ctx, &mut editor, pos2(x, taller.top()), pos2(x, 790.0));
        assert!(
            (editor.timeline_panel_rect.height() - TIMELINE_MIN_HEIGHT).abs() < 2.0,
            "{:?}",
            editor.timeline_panel_rect
        );
    }

    #[test]
    fn the_panels_hold_their_size_from_frame_to_frame() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let timeline = editor.timeline_panel_rect;
        let media = editor.media_panel_rect;

        for _ in 0..60 {
            run_frame(&ctx, &mut editor, vec![]);
        }

        assert_eq!(editor.timeline_panel_rect, timeline);
        assert_eq!(editor.media_panel_rect, media);
    }

    #[test]
    fn the_timeline_can_never_squeeze_out_the_preview() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let top = editor.timeline_panel_rect.top();
        let x = editor.timeline_panel_rect.center().x;

        pointer_drag(&ctx, &mut editor, pos2(x, top), pos2(x, 0.0));

        assert!(editor.timeline_panel_rect.height() < 800.0 * TIMELINE_MAX_SHARE);
    }

    #[test]
    fn undo_and_redo_bring_changes_back_and_forth() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.seek(Ticks::from_seconds(1.0));
        run_frame(&ctx, &mut editor, press(Key::S));
        assert_eq!(clip_count(&editor), 2);

        run_frame(&ctx, &mut editor, key_event(Key::Z, Modifiers::COMMAND));
        assert_eq!(clip_count(&editor), 1);

        run_frame(&ctx, &mut editor, key_event(Key::Y, Modifiers::COMMAND));
        assert_eq!(clip_count(&editor), 2);

        run_frame(&ctx, &mut editor, key_event(Key::Z, Modifiers::COMMAND));
        run_frame(
            &ctx,
            &mut editor,
            key_event(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT),
        );
        assert_eq!(clip_count(&editor), 2);
    }

    #[test]
    fn removing_a_video_in_use_asks_first() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let id = editor.media[0].id;

        editor.request_remove(id);

        assert_eq!(editor.confirm_remove, [id]);
        assert_eq!(clip_count(&editor), 1);
        assert_eq!(editor.visible_media().count(), 1);
    }

    #[test]
    fn removing_an_unused_video_needs_no_question() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.import(&ctx, &[second_clip().to_path_buf()]);
        let unused = editor.media[1].id;

        editor.request_remove(unused);

        assert!(editor.confirm_remove.is_empty());
        assert_eq!(editor.visible_media().count(), 1);
    }

    #[test]
    fn removing_a_video_takes_its_clips_and_undo_restores_both() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip(), second_clip()]);
        let id = editor.media[0].id;

        editor.remove_media(id);
        assert_eq!(clip_count(&editor), 1);
        assert_eq!(editor.visible_media().count(), 1);

        editor.undo();
        assert_eq!(clip_count(&editor), 2);
        assert_eq!(editor.visible_media().count(), 2);
    }

    #[test]
    fn the_last_clip_going_clears_the_picture() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.selected = Some(editor.project.tracks.played().clips()[0].id);

        editor.delete_selected();

        assert!(editor.project.tracks.is_empty());
        assert!(editor.picture.is_none());
    }

    #[test]
    fn dragging_a_clips_end_handle_trims_it() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        let scale = editor.timeline_state.scale;
        let track = editor.timeline_state.track;
        let y = track.center().y;
        let end_x = scale.x_of(editor.project.tracks.duration()) - 4.0;
        let target_x = scale.x_of(Ticks::from_seconds(2.0));

        pointer_drag(&ctx, &mut editor, pos2(end_x, y), pos2(target_x, y));

        let clip = editor.project.tracks.played().clips()[0];
        assert!(
            (clip.source.end.as_seconds() - 2.0).abs() < 0.1,
            "the clip now ends at {}",
            clip.source.end.as_seconds()
        );
        assert_eq!(clip.source.start, Ticks(0));
        assert!(editor.project.tracks.gaps().is_empty());

        editor.undo();
        assert!(
            editor.project.tracks.played().clips()[0]
                .source
                .end
                .as_seconds()
                > 3.9
        );
    }

    #[test]
    fn one_trim_drag_is_one_undo_step() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        let before = editor.project.clone();
        let scale = editor.timeline_state.scale;
        let y = editor.timeline_state.track.center().y;
        let end_x = scale.x_of(editor.project.tracks.duration()) - 4.0;

        pointer_drag(
            &ctx,
            &mut editor,
            pos2(end_x, y),
            pos2(scale.x_of(Ticks::from_seconds(2.0)), y),
        );
        editor.undo();

        assert_eq!(editor.project, before);
    }

    #[test]
    fn trimming_lands_on_a_real_frame() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        let scale = editor.timeline_state.scale;
        let y = editor.timeline_state.track.center().y;
        let end_x = scale.x_of(editor.project.tracks.duration()) - 4.0;

        pointer_drag(
            &ctx,
            &mut editor,
            pos2(end_x, y),
            pos2(scale.x_of(Ticks::from_seconds(2.013)), y),
        );

        let frames = editor.project.tracks.played().clips()[0]
            .source
            .end
            .as_seconds()
            * 30.0;
        assert!(
            (frames - frames.round()).abs() < 0.01,
            "ended {frames} frames in"
        );
    }

    #[test]
    fn dragging_a_clips_start_handle_trims_it() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        let scale = editor.timeline_state.scale;
        let y = editor.timeline_state.track.center().y;
        let start_x = scale.x_of(Ticks(0)) + 4.0;
        let target_x = scale.x_of(Ticks::from_seconds(1.0)) + 4.0;

        pointer_drag(&ctx, &mut editor, pos2(start_x, y), pos2(target_x, y));

        let clip = editor.project.tracks.played().clips()[0];
        assert!(
            (clip.source.start.as_seconds() - 1.0).abs() < 0.1,
            "the clip now starts at {}",
            clip.source.start.as_seconds()
        );
        assert!(clip.source.end.as_seconds() > 3.9);

        // The footage it keeps stays where it was, with dead space in front of it.
        assert_eq!(clip.start, clip.source.start);
        assert!((editor.project.tracks.duration().as_seconds() - 4.0).abs() < 0.001);
        assert_eq!(editor.project.tracks.gaps().len(), 1);
    }

    /// A clip from 0 to 4 s on the top track, and one from `start` for 4 s on the track below.
    fn clips_on_two_tracks(ctx: &egui::Context, start: f64) -> (Editor, ClipId, ClipId) {
        let mut editor = two_videos(ctx);
        let clips = editor.project.tracks.played().clips().to_vec();
        let (upper, lower) = (clips[0].id, clips[1].id);
        editor
            .project
            .tracks
            .move_clip(lower, Ticks::from_seconds(start), TrackTarget::New(1));
        run_frame(ctx, &mut editor, vec![]);
        (editor, upper, lower)
    }

    fn time_x(editor: &Editor, seconds: f64) -> f32 {
        editor
            .timeline_state
            .scale
            .x_of(Ticks::from_seconds(seconds))
    }

    #[test]
    fn trimming_an_end_up_to_a_clip_on_another_track_lines_up_exactly_and_shows_it() {
        let ctx = context();
        let (mut editor, _, lower) = clips_on_two_tracks(&ctx, 1.0);
        let y = editor.timeline_state.layout.track(1).center().y;
        let from = pos2(time_x(&editor, 5.0) - 4.0, y);
        // A few pixels short of the other clip's end.
        let to = pos2(time_x(&editor, 4.0) + 3.0 - 4.0, y);
        let target = Ticks::from_seconds(4.0);

        begin_drag(&ctx, &mut editor, from, to);
        run_frame(&ctx, &mut editor, moved(to));

        assert_eq!(editor.timeline_state.snap_line, Some(target));
        assert_eq!(
            editor.project.tracks.clip(lower).map(|c| c.end()),
            Some(target)
        );

        end_drag(&ctx, &mut editor, to);
        assert_eq!(editor.timeline_state.snap_line, None);
        assert_eq!(
            editor.project.tracks.clip(lower).map(|c| c.end()),
            Some(target)
        );
    }

    #[test]
    fn trimming_a_start_up_to_a_clip_on_another_track_lines_up_exactly_and_shows_it() {
        let ctx = context();
        let (mut editor, _, lower) = clips_on_two_tracks(&ctx, 1.0);
        let y = editor.timeline_state.layout.track(1).center().y;
        let from = pos2(time_x(&editor, 1.0) + 4.0, y);
        let to = pos2(time_x(&editor, 4.0) - 3.0 + 4.0, y);
        let target = Ticks::from_seconds(4.0);

        begin_drag(&ctx, &mut editor, from, to);
        run_frame(&ctx, &mut editor, moved(to));

        assert_eq!(editor.timeline_state.snap_line, Some(target));
        let clip = editor
            .project
            .tracks
            .clip(lower)
            .expect("the clip is there");
        assert_eq!(clip.start, target);
        assert_eq!(clip.source.start, Ticks::from_seconds(3.0));
        end_drag(&ctx, &mut editor, to);
    }

    #[test]
    fn a_trim_snapped_to_another_track_leaves_no_sliver_even_partway_through_a_frame() {
        let ctx = context();
        // A hundredth of a second in, so the other clip's end falls between this one's frames.
        let (mut editor, _, lower) = clips_on_two_tracks(&ctx, 1.01);
        let y = editor.timeline_state.layout.track(1).center().y;
        let from = pos2(time_x(&editor, 5.01) - 4.0, y);
        let to = pos2(time_x(&editor, 4.0) + 2.0 - 4.0, y);

        pointer_drag(&ctx, &mut editor, from, to);

        let clip = editor
            .project
            .tracks
            .clip(lower)
            .expect("the clip is there");
        assert_eq!(clip.end(), Ticks::from_seconds(4.0));
    }

    #[test]
    fn a_trim_away_from_anything_shows_no_line_and_lands_on_a_frame() {
        let ctx = context();
        let (mut editor, _, lower) = clips_on_two_tracks(&ctx, 1.0);
        let y = editor.timeline_state.layout.track(1).center().y;
        let from = pos2(time_x(&editor, 5.0) - 4.0, y);
        let to = pos2(time_x(&editor, 3.013) - 4.0, y);

        begin_drag(&ctx, &mut editor, from, to);
        run_frame(&ctx, &mut editor, moved(to));

        assert_eq!(editor.timeline_state.snap_line, None);
        let clip = editor
            .project
            .tracks
            .clip(lower)
            .expect("the clip is there");
        let frames = clip.source.end.as_seconds() * 30.0;
        assert!((frames - frames.round()).abs() < 0.01, "{frames}");
        end_drag(&ctx, &mut editor, to);
    }

    #[test]
    fn a_target_the_footage_cannot_reach_shows_no_line() {
        let ctx = context();
        // The upper clip is already its whole video, so its end can't reach 5 s.
        let (mut editor, upper, _) = clips_on_two_tracks(&ctx, 1.0);
        let y = editor.timeline_state.layout.track(0).center().y;
        let from = pos2(time_x(&editor, 4.0) - 4.0, y);
        let to = pos2(time_x(&editor, 5.0) - 3.0 - 4.0, y);

        begin_drag(&ctx, &mut editor, from, to);
        run_frame(&ctx, &mut editor, moved(to));

        assert_eq!(editor.timeline_state.snap_line, None);
        let end = editor.project.tracks.clip(upper).map(|clip| clip.end());
        assert_eq!(end, Some(Ticks::from_seconds(4.0)));
        end_drag(&ctx, &mut editor, to);
    }

    #[test]
    fn a_trim_snaps_to_the_playhead() {
        let ctx = context();
        let (mut editor, y, _, end_x) = trim_setup(&ctx, 2.0);
        let to = pos2(time_x(&editor, 2.0) + 3.0 - 4.0, y);

        begin_drag(&ctx, &mut editor, pos2(end_x, y), to);
        run_frame(&ctx, &mut editor, moved(to));

        let target = Ticks::from_seconds(2.0);
        assert_eq!(editor.timeline_state.snap_line, Some(target));
        assert_eq!(editor.project.tracks.played().clips()[0].end(), target);
        end_drag(&ctx, &mut editor, to);
    }

    fn cursor_over(ctx: &egui::Context, editor: &mut Editor, at: egui::Pos2) -> egui::CursorIcon {
        run_frame(ctx, editor, moved(at));
        let output = run_frame(ctx, editor, vec![]);
        output.platform_output.cursor_icon
    }

    #[test]
    fn the_buttons_on_a_card_show_an_ordinary_pointer_and_the_rest_of_it_shows_a_grab() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip()]);
        let card = editor.card_thumbnails[0].1;

        assert_eq!(
            cursor_over(&ctx, &mut editor, card.center()),
            egui::CursorIcon::Grab
        );
        for (name, at) in [
            ("add", card.right_bottom() - vec2(20.0, 20.0)),
            ("remove", card.right_top() + vec2(-20.0, 20.0)),
            ("select", card.left_top() + vec2(16.0, 16.0)),
        ] {
            let cursor = cursor_over(&ctx, &mut editor, at);
            assert_eq!(cursor, egui::CursorIcon::Default, "{name}");
        }
    }

    #[test]
    fn dragging_from_a_button_on_a_card_does_not_drag_the_card() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip()]);
        let card = editor.card_thumbnails[0].1;
        let plus = card.right_bottom() - vec2(20.0, 20.0);
        let on_timeline = editor.timeline_state.track.center();

        begin_drag(&ctx, &mut editor, plus, on_timeline);
        end_drag(&ctx, &mut editor, on_timeline);

        assert!(editor.project.tracks.is_empty());
    }

    #[test]
    fn the_top_strip_loads_the_small_logo_for_the_theme_at_about_its_size() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[]);
        ctx.set_theme(egui::Theme::Dark);
        run_frame(&ctx, &mut editor, vec![]);
        run_frame(&ctx, &mut editor, vec![]);

        let (dark, _, logo) = editor.strip_logo.clone().expect("the logo is loaded");
        assert!(dark);
        let height = logo.size_vec2().y;
        assert!(
            (STRIP_LOGO_HEIGHT..STRIP_LOGO_HEIGHT * 1.2).contains(&height),
            "{height}"
        );

        ctx.set_theme(egui::Theme::Light);
        run_frame(&ctx, &mut editor, vec![]);
        run_frame(&ctx, &mut editor, vec![]);
        let (dark, ..) = editor.strip_logo.clone().expect("the logo is loaded");
        assert!(!dark);
        ctx.set_theme(egui::Theme::Dark);
    }

    #[test]
    fn shrinking_a_picture_averages_without_a_dark_fringe() {
        // Solid white beside invisible black.
        let rgba = [255, 255, 255, 255, 0, 0, 0, 0];

        let shrunk = shrunk_image(&rgba, [2, 1], 2);

        assert_eq!(shrunk.size, [1, 1]);
        assert_eq!(
            shrunk.pixels[0],
            Color32::from_rgba_premultiplied(127, 127, 127, 127)
        );
    }

    #[test]
    fn shrinking_a_picture_keeps_its_leftover_edge() {
        let rgba = [200_u8; 4 * 5 * 3];

        let shrunk = shrunk_image(&rgba, [5, 3], 2);

        assert_eq!(shrunk.size, [3, 2]);
    }

    fn is_named(ctx: &egui::Context, editor: &mut Editor, name: &str) -> bool {
        named_nodes(ctx, editor)
            .iter()
            .any(|(label, ..)| label == name)
    }

    #[test]
    fn delete_only_appears_with_something_selected_to_delete() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        assert!(
            is_named(&ctx, &mut editor, "Delete"),
            "a new clip is selected"
        );

        editor.selected = None;
        assert!(!is_named(&ctx, &mut editor, "Delete"));

        editor.selected = editor.project.tracks.played().clips().first().map(|c| c.id);
        click_named(&ctx, &mut editor, "Delete");
        assert!(editor.project.tracks.is_empty());
        assert!(!is_named(&ctx, &mut editor, "Delete"));
    }

    #[test]
    fn delete_appears_for_selected_dead_space_too() {
        let ctx = context();
        let mut editor = with_leading_gap(&ctx);
        editor.selected = None;
        editor.selected_gap = Some(Ticks::from_seconds(0.5));

        assert!(is_named(&ctx, &mut editor, "Delete"));
    }

    #[test]
    fn remove_all_gaps_only_appears_while_there_are_gaps() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        assert!(!is_named(&ctx, &mut editor, "Remove all gaps"));

        let mut editor = with_leading_gap(&ctx);
        assert!(is_named(&ctx, &mut editor, "Remove all gaps"));

        click_named(&ctx, &mut editor, "Remove all gaps");
        assert!(editor.project.tracks.gaps().is_empty());
        assert!(!is_named(&ctx, &mut editor, "Remove all gaps"));
    }

    #[test]
    fn the_timeline_tools_are_only_symbols() {
        let ctx = context();
        let mut editor = with_leading_gap(&ctx);
        editor.selected_gap = Some(Ticks::from_seconds(0.5));

        for name in ["Split", "Delete", "Remove all gaps"] {
            let rect = named_rect(&ctx, &mut editor, name);
            assert!(rect.width() <= 32.0, "{name} is {} wide", rect.width());
        }
    }

    #[test]
    fn the_tools_stay_in_place_when_one_comes_or_goes() {
        let ctx = context();
        let mut editor = with_leading_gap(&ctx);
        editor.selected = None;
        editor.selected_gap = None;
        let without_delete = named_rect(&ctx, &mut editor, "Remove all gaps");

        editor.selected_gap = Some(Ticks::from_seconds(0.5));
        let with_delete = named_rect(&ctx, &mut editor, "Remove all gaps");

        assert_eq!(without_delete, with_delete);
    }

    #[test]
    fn the_timeline_scrollbar_only_shows_when_there_is_somewhere_to_scroll() {
        let ctx = context();
        let mut empty = Editor::new(&ctx, false);
        run_frame(&ctx, &mut empty, vec![]);
        assert!(!empty.timeline_state.scrollbar.is_positive());

        let mut editor = with_videos(&ctx, &[test_clip()]);
        assert!(editor.timeline_state.scrollbar.is_positive());

        let [zoom_out, ..] = editor.timeline_state.zoom_buttons;
        for _ in 0..10 {
            click(&ctx, &mut editor, zoom_out.center());
        }
        run_frame(&ctx, &mut editor, vec![]);
        assert!(!editor.timeline_state.scrollbar.is_positive());
    }

    /// Runs frames until a video's pictures and sound have been read.
    fn wait_for_strip(ctx: &egui::Context, editor: &mut Editor) {
        let started = Instant::now();
        while editor
            .media
            .iter()
            .any(|entry| entry.strip_reader.is_some())
        {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "reading pictures and sound took too long"
            );
            run_frame(ctx, editor, vec![]);
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_video_gets_pictures_in_order_for_its_clips() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip()]);

        wait_for_strip(&ctx, &mut editor);

        let entry = &editor.media[0];
        assert!(entry.strip_pictures_done);
        assert!(
            entry.strip_pictures.len() >= 2,
            "{}",
            entry.strip_pictures.len()
        );
        for pair in entry.strip_pictures.windows(2) {
            assert!(pair[0].0 < pair[1].0);
        }
        assert_eq!(entry.strip_pictures[0].0, Ticks(0));
    }

    #[test]
    fn a_video_with_no_sound_has_an_empty_shape() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip()]);

        wait_for_strip(&ctx, &mut editor);

        let shape = editor.media[0].sound_shape.as_ref().expect("it was read");
        assert!(shape.peaks.is_empty());
    }

    #[test]
    fn pictures_never_get_in_the_way_of_using_the_video() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip()]);

        // Straight away, whether or not any pictures have arrived.
        let id = editor.media[0].id;
        editor.add_to_timeline(id);
        run_frame(&ctx, &mut editor, vec![]);

        assert_eq!(editor.project.tracks.played().clips().len(), 1);
    }

    #[test]
    fn turning_thumbnails_off_stops_reading_and_forgets_them_and_on_reads_them_again() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip()]);
        wait_for_strip(&ctx, &mut editor);

        editor.settings.thumbnails = false;
        editor.apply_strip_settings(&ctx);

        assert!(editor.media[0].strip_pictures.is_empty());
        assert!(
            editor.media[0].strip_reader.is_none(),
            "the sound was already read"
        );
        assert!(editor.media[0].sound_shape.is_some());

        editor.settings.thumbnails = true;
        editor.apply_strip_settings(&ctx);
        wait_for_strip(&ctx, &mut editor);

        assert!(!editor.media[0].strip_pictures.is_empty());
    }

    #[test]
    fn turning_waveforms_off_forgets_them_and_on_reads_them_again() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip()]);
        wait_for_strip(&ctx, &mut editor);

        editor.settings.waveforms = false;
        editor.apply_strip_settings(&ctx);
        assert!(editor.media[0].sound_shape.is_none());
        assert!(
            !editor.media[0].strip_pictures.is_empty(),
            "pictures are kept"
        );

        editor.settings.waveforms = true;
        editor.apply_strip_settings(&ctx);
        wait_for_strip(&ctx, &mut editor);
        assert!(editor.media[0].sound_shape.is_some());
    }

    #[test]
    fn with_both_off_a_new_video_is_never_read_for_its_clips() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        editor.settings.thumbnails = false;
        editor.settings.waveforms = false;

        editor.import(&ctx, &[test_clip().to_path_buf()]);

        assert!(editor.media[0].strip_reader.is_none());
    }

    #[test]
    fn the_settings_dialog_turns_clip_pictures_off_and_remembers_it() {
        let ctx = context();
        let folder = project_folder("strip-settings");
        let mut editor = with_storage(&ctx, &folder);
        editor.import(&ctx, &[test_clip().to_path_buf()]);
        settle(&ctx, &mut editor);
        wait_for_strip(&ctx, &mut editor);

        click_named(&ctx, &mut editor, "Settings");
        let_dialog_open(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Thumbnails on clips");

        assert!(!editor.settings.thumbnails);
        assert!(editor.media[0].strip_pictures.is_empty());
        assert!(!with_storage(&ctx, &folder).settings.thumbnails);
    }

    #[test]
    fn closing_a_project_stops_reading_its_videos() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip()]);
        let cancel = editor.media[0]
            .strip_reader
            .as_ref()
            .map(|reader| reader.cancel.clone());

        editor.start_new_project();

        if let Some(cancel) = cancel {
            assert!(cancel.load(Ordering::Relaxed));
        }
    }

    /// One clip with the playhead part way through, ready to have an end dragged.
    fn trim_setup(ctx: &egui::Context, playhead: f64) -> (Editor, f32, f32, f32) {
        let mut editor = with_videos(ctx, &[test_clip()]);
        editor.seek(Ticks::from_seconds(playhead));
        run_frame(ctx, &mut editor, vec![]);
        let scale = editor.timeline_state.scale;
        let y = editor.timeline_state.track.center().y;
        let start_x = scale.x_of(Ticks(0)) + 4.0;
        let end_x = scale.x_of(editor.project.tracks.duration()) - 4.0;
        (editor, y, start_x, end_x)
    }

    #[test]
    fn trimming_either_end_leaves_the_playhead_where_it_is() {
        let ctx = context();
        let playhead = Ticks::from_seconds(2.0);

        let (mut editor, y, start_x, end_x) = trim_setup(&ctx, 2.0);
        let scale = editor.timeline_state.scale;
        let to = pos2(scale.x_of(Ticks::from_seconds(3.0)), y);
        pointer_drag(&ctx, &mut editor, pos2(end_x, y), to);
        assert_eq!(editor.position(), playhead);

        let to = pos2(scale.x_of(Ticks::from_seconds(1.0)) + 4.0, y);
        pointer_drag(&ctx, &mut editor, pos2(start_x, y), to);
        assert_eq!(editor.position(), playhead);
        assert!(!editor.preview.in_gap(editor.project.tracks.played()));
    }

    #[test]
    fn trimming_away_what_is_under_the_playhead_leaves_it_in_the_dead_space() {
        let ctx = context();
        let (mut editor, y, start_x, _) = trim_setup(&ctx, 1.0);
        let scale = editor.timeline_state.scale;

        let to = pos2(scale.x_of(Ticks::from_seconds(2.0)) + 4.0, y);
        pointer_drag(&ctx, &mut editor, pos2(start_x, y), to);

        assert_eq!(editor.position(), Ticks::from_seconds(1.0));
        assert!(editor.preview.in_gap(editor.project.tracks.played()));
    }

    #[test]
    fn trimming_the_project_shorter_than_the_playhead_stops_it_at_the_new_end() {
        let ctx = context();
        let (mut editor, y, _, end_x) = trim_setup(&ctx, 3.0);
        let scale = editor.timeline_state.scale;

        let to = pos2(scale.x_of(Ticks::from_seconds(2.0)), y);
        pointer_drag(&ctx, &mut editor, pos2(end_x, y), to);

        assert_eq!(editor.position(), editor.project.tracks.duration());
    }

    #[test]
    fn dragging_an_end_back_out_in_one_go_returns_the_playhead_to_its_place() {
        let ctx = context();
        let (mut editor, y, _, end_x) = trim_setup(&ctx, 3.0);
        let scale = editor.timeline_state.scale;

        let short = pos2(scale.x_of(Ticks::from_seconds(2.0)), y);
        begin_drag(&ctx, &mut editor, pos2(end_x, y), short);
        assert!(editor.position() < Ticks::from_seconds(3.0));

        let back = pos2(end_x, y);
        for _ in 0..3 {
            run_frame(&ctx, &mut editor, moved(back));
        }
        end_drag(&ctx, &mut editor, back);

        assert_eq!(editor.position(), Ticks::from_seconds(3.0));
    }

    fn let_dialog_open(ctx: &egui::Context, editor: &mut Editor) {
        for _ in 0..12 {
            run_frame(ctx, editor, vec![]);
        }
    }

    fn project_folder(name: &str) -> PathBuf {
        let folder = std::env::temp_dir()
            .join("bareclip-project-tests")
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).expect("the folder can be created");
        folder
    }

    /// Every clip as (track, media's file, start, source), which stays the same across a
    /// save and a load even though ids don't.
    fn arrangement(editor: &Editor) -> Vec<(usize, PathBuf, Ticks, TimeRange)> {
        editor
            .project
            .tracks
            .tracks()
            .iter()
            .enumerate()
            .flat_map(|(track, clips)| clips.clips().iter().map(move |clip| (track, *clip)))
            .map(|(track, clip)| {
                let path = editor.entry(clip.media).expect("the video is there");
                (track, path.info.path.clone(), clip.start, clip.source)
            })
            .collect()
    }

    /// Two clips on two tracks with the top one covering part of the other.
    fn stacked_project(ctx: &egui::Context) -> Editor {
        let mut editor = two_videos(ctx);
        let upper = editor.project.tracks.played().clips()[1].id;
        editor
            .project
            .tracks
            .move_clip(upper, Ticks::from_seconds(1.0), TrackTarget::New(0));
        run_frame(ctx, &mut editor, vec![]);
        editor
    }

    fn opened(ctx: &egui::Context, path: &Path) -> Editor {
        let mut editor = Editor::new(ctx, false);
        run_frame(ctx, &mut editor, vec![]);
        editor.open_project(ctx, path);
        settle(ctx, &mut editor);
        run_frame(ctx, &mut editor, vec![]);
        editor
    }

    #[test]
    fn a_saved_project_opens_again_exactly_as_it_was() {
        let ctx = context();
        let mut editor = stacked_project(&ctx);
        editor.project_name = "Final round".to_owned();
        let path = project_folder("round-trip").join("final.bareclip");

        assert!(editor.save_to(&path));
        let back = opened(&ctx, &path);

        assert_eq!(arrangement(&back), arrangement(&editor));
        assert_eq!(back.project_name, "Final round");
        assert_eq!(back.project_path.as_deref(), Some(path.as_path()));
        assert!(!back.has_unsaved_changes());
    }

    #[test]
    fn an_opened_project_plays_and_exports_what_was_saved() {
        let ctx = context();
        let mut editor = stacked_project(&ctx);
        let path = project_folder("plays").join("a.bareclip");
        editor.save_to(&path);

        let mut back = opened(&ctx, &path);

        let ranges = |editor: &Editor| -> Vec<TimeRange> {
            let parts = editor.export_parts().expect("the frames are read");
            parts.iter().map(|part| part.range).collect()
        };
        assert_eq!(ranges(&back), ranges(&editor));
        back.seek(Ticks::from_seconds(2.0));
        assert_eq!(back.position(), Ticks::from_seconds(2.0));
    }

    #[test]
    fn opening_a_project_starts_with_nothing_to_undo() {
        let ctx = context();
        let mut editor = stacked_project(&ctx);
        let path = project_folder("undo").join("a.bareclip");
        editor.save_to(&path);

        let back = opened(&ctx, &path);

        assert!(!back.history.can_undo());
    }

    #[test]
    fn saving_adds_the_extension_and_an_unnamed_project_takes_the_files_name() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let folder = project_folder("names");

        assert!(editor.save_to(&folder.join("my edit")));

        assert!(folder.join("my edit.bareclip").exists());
        assert_eq!(editor.project_name, "my edit");
    }

    #[test]
    fn a_project_with_its_own_name_keeps_it_whatever_the_file_is_called() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.project_name = "Chosen".to_owned();

        editor.save_to(&project_folder("own-name").join("other.bareclip"));

        assert_eq!(editor.project_name, "Chosen");
    }

    #[test]
    fn saving_to_somewhere_impossible_says_so_and_keeps_the_changes_unsaved() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let nowhere = project_folder("nowhere").join("no-such-folder").join("a");

        assert!(!editor.save_to(&nowhere));

        assert!(editor.has_unsaved_changes());
        assert!(editor.project_path.is_none());
        assert!(
            editor
                .notice
                .as_deref()
                .is_some_and(|n| n.contains("saved"))
        );
    }

    #[test]
    fn unsaved_changes_are_tracked_through_edits_saves_and_undo() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[]);
        assert!(
            !editor.has_unsaved_changes(),
            "a new project has no work in it"
        );

        editor.import(&ctx, &[test_clip().to_path_buf()]);
        settle(&ctx, &mut editor);
        assert!(editor.has_unsaved_changes());

        let id = editor.media[0].id;
        editor.add_to_timeline(id);
        assert!(editor.has_unsaved_changes());

        assert!(editor.save_to(&project_folder("tracking").join("a.bareclip")));
        assert!(!editor.has_unsaved_changes());

        let clip = editor.project.tracks.played().clips()[0].id;
        editor.selected = Some(clip);
        editor.delete_selected();
        assert!(editor.has_unsaved_changes());

        editor.undo();
        assert!(!editor.has_unsaved_changes(), "back to what is on disk");
    }

    #[test]
    fn the_top_strip_marks_unsaved_changes_after_the_name() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.project_name = "Edit".to_owned();

        assert!(is_named(&ctx, &mut editor, "Edit *"));

        editor.save_to(&project_folder("marker").join("a.bareclip"));
        assert!(is_named(&ctx, &mut editor, "Edit"));
        assert!(!is_named(&ctx, &mut editor, "Edit *"));
    }

    fn type_text(ctx: &egui::Context, editor: &mut Editor, text: &str) {
        run_frame(ctx, editor, vec![egui::Event::Text(text.to_owned())]);
    }

    #[test]
    fn clicking_the_name_renames_the_project_and_enter_keeps_it() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[]);

        click_named(&ctx, &mut editor, project::DEFAULT_NAME);
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.renaming.is_some());
        run_frame(&ctx, &mut editor, key_event(Key::A, Modifiers::COMMAND));
        type_text(&ctx, &mut editor, "Montage");
        run_frame(&ctx, &mut editor, press(Key::Enter));
        run_frame(&ctx, &mut editor, vec![]);

        assert_eq!(editor.project_name, "Montage");
        assert!(editor.renaming.is_none());
        assert!(editor.has_unsaved_changes());
    }

    #[test]
    fn escape_cancels_a_rename_and_an_empty_name_is_not_kept() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[]);

        click_named(&ctx, &mut editor, project::DEFAULT_NAME);
        run_frame(&ctx, &mut editor, vec![]);
        type_text(&ctx, &mut editor, "Nope");
        run_frame(&ctx, &mut editor, press(Key::Escape));
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.project_name, project::DEFAULT_NAME);

        click_named(&ctx, &mut editor, project::DEFAULT_NAME);
        run_frame(&ctx, &mut editor, vec![]);
        run_frame(&ctx, &mut editor, key_event(Key::A, Modifiers::COMMAND));
        run_frame(&ctx, &mut editor, press(Key::Backspace));
        run_frame(&ctx, &mut editor, press(Key::Enter));
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.project_name, project::DEFAULT_NAME);
    }

    #[test]
    fn typing_a_name_does_not_trigger_shortcuts() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.seek(Ticks::from_seconds(1.0));
        let before = editor.project.clone();

        click_named(&ctx, &mut editor, &format!("{} *", project::DEFAULT_NAME));
        run_frame(&ctx, &mut editor, vec![]);
        run_frame(&ctx, &mut editor, vec![]);
        for key in [Key::S, Key::Space, Key::Delete, Key::ArrowRight] {
            run_frame(&ctx, &mut editor, press(key));
        }

        assert_eq!(editor.project, before);
        assert!(!editor.preview.is_playing());
        assert_eq!(editor.position(), Ticks::from_seconds(1.0));
    }

    #[test]
    fn the_project_menu_offers_every_file_action() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[]);
        click_named(&ctx, &mut editor, "Project menu");

        for name in [
            "New project",
            "Open project",
            "Save",
            "Save as",
            "Auto Save",
        ] {
            assert!(menu_item(&ctx, &mut editor, name).is_some(), "{name}");
        }
    }

    #[test]
    fn a_new_project_clears_everything_when_nothing_would_be_lost() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.save_to(&project_folder("new-clean").join("a.bareclip"));

        run_frame(&ctx, &mut editor, key_event(Key::N, Modifiers::COMMAND));

        assert!(editor.leave_for.is_none());
        assert!(editor.project.tracks.is_empty());
        assert!(editor.media.is_empty());
        assert!(editor.project_path.is_none());
        assert!(!editor.history.can_undo());
        assert_eq!(editor.project_name, project::DEFAULT_NAME);
    }

    #[test]
    fn a_new_project_asks_first_when_work_would_be_lost() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);

        run_frame(&ctx, &mut editor, key_event(Key::N, Modifiers::COMMAND));
        assert_eq!(editor.leave_for, Some(LeaveFor::NewProject));
        assert!(!editor.project.tracks.is_empty());

        let_dialog_open(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Cancel");
        assert!(editor.leave_for.is_none());
        assert!(!editor.project.tracks.is_empty());

        run_frame(&ctx, &mut editor, key_event(Key::N, Modifiers::COMMAND));
        let_dialog_open(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Don't save");
        assert!(editor.project.tracks.is_empty());
    }

    #[test]
    fn save_in_the_dialog_saves_and_then_carries_on() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let path = project_folder("save-then-new").join("a.bareclip");
        editor.save_to(&path);
        let second = editor.media[0].id;
        editor.add_to_timeline(second);
        let saved_clips = editor.project.tracks.played().clips().len();

        run_frame(&ctx, &mut editor, key_event(Key::N, Modifiers::COMMAND));
        let_dialog_open(&ctx, &mut editor);
        let button = named_nodes(&ctx, &mut editor)
            .into_iter()
            .rfind(|(label, ..)| label == "Save")
            .expect("the dialog has Save")
            .1;
        click(&ctx, &mut editor, button.center());

        assert!(editor.project.tracks.is_empty());
        let back = ProjectFile::read(&path).expect("it was saved");
        assert_eq!(back.tracks[0].len(), saved_clips);
    }

    #[test]
    fn closing_with_unsaved_work_asks_and_cancel_keeps_the_window_open() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);

        editor.handle_close_request(&ctx);
        assert_eq!(editor.leave_for, Some(LeaveFor::CloseWindow));
        assert!(!editor.closing);

        let_dialog_open(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Cancel");
        assert!(editor.leave_for.is_none());
        assert!(!editor.closing);
    }

    #[test]
    fn closing_with_nothing_unsaved_does_not_ask() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[]);

        editor.handle_close_request(&ctx);

        assert!(editor.leave_for.is_none());
    }

    #[test]
    fn dont_save_lets_the_window_close_without_asking_again() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.handle_close_request(&ctx);
        let_dialog_open(&ctx, &mut editor);

        click_named(&ctx, &mut editor, "Don't save");

        assert!(editor.closing);
        editor.handle_close_request(&ctx);
        assert!(editor.leave_for.is_none());
    }

    #[test]
    fn a_damaged_project_says_so_and_leaves_the_open_one_alone() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let before = editor.project.clone();
        let path = project_folder("damaged").join("bad.bareclip");
        std::fs::write(&path, "not a project").expect("it can be written");

        editor.open_project(&ctx, &path);

        assert_eq!(editor.project, before);
        let notice = editor.notice.clone().expect("it says something");
        assert!(
            notice.contains("bad.bareclip") && notice.contains("damaged"),
            "{notice}"
        );
    }

    #[test]
    fn a_project_from_a_newer_version_says_so_and_leaves_the_open_one_alone() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let before = editor.project.clone();
        let path = project_folder("newer").join("new.bareclip");
        std::fs::write(&path, r#"{"version":999}"#).expect("it can be written");

        editor.open_project(&ctx, &path);

        assert_eq!(editor.project, before);
        assert!(
            editor
                .notice
                .as_deref()
                .is_some_and(|n| n.contains("newer"))
        );
    }

    #[test]
    fn dropping_a_project_file_opens_it_rather_than_adding_it_as_a_video() {
        let ctx = context();
        let mut editor = stacked_project(&ctx);
        let path = project_folder("dropped").join("a.bareclip");
        editor.save_to(&path);
        let mut other = in_panel_only(&ctx, &[]);

        other.import(&ctx, std::slice::from_ref(&path));
        settle(&ctx, &mut other);

        assert_eq!(other.project_path, Some(path));
        assert_eq!(arrangement(&other), arrangement(&editor));
    }

    /// Retries a change to a video until whatever last had it open has let go of it.
    fn once_free(mut change: impl FnMut() -> std::io::Result<()>) {
        let started = Instant::now();
        while let Err(error) = change() {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the video stayed in use: {error}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// A project whose video sits in its own folder, saved and closed.
    fn project_with_movable_video(ctx: &egui::Context, name: &str) -> (PathBuf, PathBuf) {
        let folder = project_folder(name);
        let videos = folder.join("videos");
        std::fs::create_dir_all(&videos).expect("the folder can be created");
        let video = videos.join("clip.mp4");
        std::fs::copy(test_clip(), &video).expect("the video can be copied");
        let mut editor = in_panel_only(ctx, &[&video]);
        let id = editor.media[0].id;
        editor.add_to_timeline(id);
        let project = folder.join("a.bareclip");
        assert!(editor.save_to(&project));
        (project, video)
    }

    #[test]
    fn a_moved_video_is_asked_about_and_finding_it_finishes_opening() {
        let ctx = context();
        let (project, video) = project_with_movable_video(&ctx, "moved");
        let moved = project.with_file_name("elsewhere.mp4");
        once_free(|| std::fs::rename(&video, &moved));

        let mut back = Editor::new(&ctx, false);
        back.open_project(&ctx, &project);
        assert!(back.missing_videos.is_some());
        assert!(back.project.tracks.is_empty(), "it waits for an answer");

        back.point_video_at(&ctx, 0, moved.clone());
        settle(&ctx, &mut back);

        assert!(back.missing_videos.is_none());
        assert_eq!(back.project.tracks.played().clips().len(), 1);
        assert_eq!(back.media[0].info.path, moved);
    }

    #[test]
    fn opening_without_a_missing_video_leaves_its_clips_out_and_counts_as_unsaved() {
        let ctx = context();
        let (project, video) = project_with_movable_video(&ctx, "without");
        once_free(|| std::fs::remove_file(&video));

        let mut back = Editor::new(&ctx, false);
        back.open_project(&ctx, &project);
        let_dialog_open(&ctx, &mut back);
        click_named(&ctx, &mut back, "Open without them");

        assert!(back.missing_videos.is_none());
        assert!(back.project.tracks.is_empty());
        assert!(back.has_unsaved_changes());
        let on_disk = ProjectFile::read(&project).expect("it can be read");
        assert_eq!(on_disk.tracks[0].len(), 1, "the file itself is untouched");
    }

    #[test]
    fn cancelling_over_a_missing_video_keeps_what_was_open() {
        let ctx = context();
        let (project, video) = project_with_movable_video(&ctx, "cancel");
        once_free(|| std::fs::remove_file(&video));
        let mut other = with_videos(&ctx, &[test_clip()]);
        let before = other.project.clone();

        other.open_project(&ctx, &project);
        let_dialog_open(&ctx, &mut other);
        click_named(&ctx, &mut other, "Cancel");

        assert!(other.missing_videos.is_none());
        assert_eq!(other.project, before);
    }

    #[test]
    fn a_video_beside_the_project_file_is_found_without_asking() {
        let ctx = context();
        let (project, video) = project_with_movable_video(&ctx, "beside");
        let beside = project.with_file_name("clip.mp4");
        once_free(|| std::fs::rename(&video, &beside));

        let back = opened(&ctx, &project);

        assert!(back.missing_videos.is_none());
        assert_eq!(back.media[0].info.path, beside);
    }

    fn with_storage(ctx: &egui::Context, folder: &Path) -> Editor {
        let mut editor = Editor::new(ctx, false);
        editor.use_storage(ctx, folder.to_owned());
        run_frame(ctx, &mut editor, vec![]);
        editor
    }

    /// Makes an autosave due, runs a frame and waits for the copy to reach the disk.
    fn autosave_now(ctx: &egui::Context, editor: &mut Editor) {
        editor.last_autosave_at = Instant::now() - Duration::from_secs(3600);
        run_frame(ctx, editor, vec![]);
        editor.recovery.as_mut().expect("it is on").wait();
    }

    /// Ends a run the way a crash does: the copy stays, and the lock is released.
    fn crash(editor: Editor) {
        let mut editor = editor;
        editor.recovery.take().expect("it is on").abandon();
    }

    #[test]
    fn after_a_crash_the_next_run_offers_the_work_back_and_restores_it() {
        let ctx = context();
        let folder = project_folder("crash");
        let mut first = with_storage(&ctx, &folder);
        let path = folder.join("saved.bareclip");
        first.import(&ctx, &[test_clip().to_path_buf()]);
        settle(&ctx, &mut first);
        let id = first.media[0].id;
        first.add_to_timeline(id);
        first.project_name = "Lost work".to_owned();
        first.save_to(&path);
        first.add_to_timeline(id);
        autosave_now(&ctx, &mut first);
        let expected = arrangement(&first);
        crash(first);

        let mut second = with_storage(&ctx, &folder);
        assert_eq!(second.recovery_offers.len(), 1);
        let_dialog_open(&ctx, &mut second);
        click_named(&ctx, &mut second, "Restore");
        settle(&ctx, &mut second);

        assert_eq!(arrangement(&second), expected);
        assert_eq!(second.project_name, "Lost work");
        assert_eq!(
            second.project_path,
            Some(path),
            "saving goes to the same file"
        );
        assert!(second.has_unsaved_changes(), "it isn't on disk yet");
        assert!(second.recovery_offers.is_empty());
        second.recovery.as_mut().expect("it is on").wait();
        assert!(storage::orphaned_recoveries(&folder).is_empty());
    }

    #[test]
    fn restored_work_survives_a_second_crash() {
        let ctx = context();
        let folder = project_folder("crash-twice");
        let mut first = with_storage(&ctx, &folder);
        first.import(&ctx, &[test_clip().to_path_buf()]);
        settle(&ctx, &mut first);
        let id = first.media[0].id;
        first.add_to_timeline(id);
        autosave_now(&ctx, &mut first);
        crash(first);

        let mut second = with_storage(&ctx, &folder);
        let_dialog_open(&ctx, &mut second);
        click_named(&ctx, &mut second, "Restore");
        settle(&ctx, &mut second);
        second.recovery.as_mut().expect("it is on").wait();
        crash(second);

        let third = with_storage(&ctx, &folder);
        assert_eq!(third.recovery_offers.len(), 1);
    }

    #[test]
    fn discarding_recovered_work_removes_it_for_good() {
        let ctx = context();
        let folder = project_folder("crash-discard");
        let mut first = with_storage(&ctx, &folder);
        first.import(&ctx, &[test_clip().to_path_buf()]);
        settle(&ctx, &mut first);
        autosave_now(&ctx, &mut first);
        crash(first);

        let mut second = with_storage(&ctx, &folder);
        let_dialog_open(&ctx, &mut second);
        click_named(&ctx, &mut second, "Discard");

        assert!(second.project.tracks.is_empty());
        assert!(second.media.is_empty());
        assert!(storage::orphaned_recoveries(&folder).is_empty());
    }

    #[test]
    fn backing_out_of_a_restore_over_a_missing_video_keeps_the_copy() {
        let ctx = context();
        let folder = project_folder("crash-missing");
        let video = folder.join("clip.mp4");
        std::fs::copy(test_clip(), &video).expect("the video can be copied");
        let mut first = with_storage(&ctx, &folder);
        first.import(&ctx, std::slice::from_ref(&video));
        settle(&ctx, &mut first);
        autosave_now(&ctx, &mut first);
        crash(first);
        once_free(|| std::fs::remove_file(&video));

        let mut second = with_storage(&ctx, &folder);
        let_dialog_open(&ctx, &mut second);
        click_named(&ctx, &mut second, "Restore");
        assert!(second.missing_videos.is_some());
        let_dialog_open(&ctx, &mut second);
        click_named(&ctx, &mut second, "Cancel");

        assert_eq!(storage::orphaned_recoveries(&folder).len(), 1);
    }

    #[test]
    fn autosave_waits_for_its_interval_and_skips_what_it_already_has() {
        let ctx = context();
        let folder = project_folder("interval");
        let mut editor = with_storage(&ctx, &folder);
        editor.import(&ctx, &[test_clip().to_path_buf()]);
        settle(&ctx, &mut editor);

        editor.last_autosave_at = Instant::now();
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.last_autosaved.is_none(), "too soon");

        autosave_now(&ctx, &mut editor);
        let written = editor.last_autosaved.clone();
        assert!(written.is_some());
        let at = editor.last_autosave_at;

        editor.last_autosave_at = at - Duration::from_secs(3600);
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.last_autosaved, written);
        assert!(editor.last_autosave_at < at, "nothing new to write");
    }

    #[test]
    fn saving_removes_the_recovery_copy() {
        let ctx = context();
        let folder = project_folder("saved-clears");
        let mut editor = with_storage(&ctx, &folder);
        editor.import(&ctx, &[test_clip().to_path_buf()]);
        settle(&ctx, &mut editor);
        autosave_now(&ctx, &mut editor);

        assert!(editor.save_to(&folder.join("a.bareclip")));
        crash(editor);

        assert!(storage::orphaned_recoveries(&folder).is_empty());
    }

    #[test]
    fn an_empty_project_never_writes_a_recovery_copy() {
        let ctx = context();
        let folder = project_folder("empty");
        let mut editor = with_storage(&ctx, &folder);

        autosave_now(&ctx, &mut editor);
        crash(editor);

        assert!(storage::orphaned_recoveries(&folder).is_empty());
    }

    #[test]
    fn a_recovery_copy_that_cannot_be_read_is_not_offered() {
        let ctx = context();
        let folder = project_folder("bad-copy");
        let recovery = folder.join("recovery");
        std::fs::create_dir_all(&recovery).expect("the folder can be created");
        std::fs::write(recovery.join("1-1.recovery"), "garbage").expect("it can be written");

        let mut editor = with_storage(&ctx, &folder);
        run_frame(&ctx, &mut editor, vec![]);

        assert!(editor.recovery_offers.is_empty());
        assert!(storage::orphaned_recoveries(&folder).is_empty());
    }

    #[test]
    fn shortcuts_wait_while_a_recovery_is_being_offered() {
        let ctx = context();
        let folder = project_folder("offer-blocks");
        let mut first = with_storage(&ctx, &folder);
        first.import(&ctx, &[test_clip().to_path_buf()]);
        settle(&ctx, &mut first);
        autosave_now(&ctx, &mut first);
        crash(first);

        let mut second = with_storage(&ctx, &folder);
        run_frame(&ctx, &mut second, key_event(Key::N, Modifiers::COMMAND));

        assert_eq!(second.recovery_offers.len(), 1);
        assert!(second.leave_for.is_none());
    }

    #[test]
    fn the_settings_button_replaces_the_theme_button() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[]);

        assert!(is_named(&ctx, &mut editor, "Settings"));
        assert!(!is_named(&ctx, &mut editor, "Theme"));
    }

    #[test]
    fn settings_are_kept_for_the_next_run() {
        let ctx = context();
        let folder = project_folder("settings");
        let mut editor = with_storage(&ctx, &folder);

        click_named(&ctx, &mut editor, "Settings");
        let_dialog_open(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Light");
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Fastest");
        assert_eq!(editor.mode, CutMode::Keyframe);
        click_named(&ctx, &mut editor, "Done");
        assert!(!editor.settings_open);
        ctx.set_theme(egui::ThemePreference::System);

        let next = with_storage(&ctx, &folder);

        assert_eq!(next.mode, CutMode::Keyframe);
        assert_eq!(
            ctx.options(|options| options.theme_preference),
            egui::ThemePreference::Light
        );
        ctx.set_theme(egui::ThemePreference::Dark);
    }

    #[test]
    fn recovery_copies_are_written_every_five_seconds() {
        let ctx = context();
        let folder = project_folder("interval-fixed");
        let mut editor = with_storage(&ctx, &folder);
        editor.import(&ctx, &[test_clip().to_path_buf()]);
        settle(&ctx, &mut editor);

        editor.last_autosave_at = Instant::now() - Duration::from_secs(4);
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.last_autosaved.is_none());

        editor.last_autosave_at = Instant::now() - Duration::from_millis(5100);
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.last_autosaved.is_some());
        editor.recovery.as_mut().expect("it is on").wait();
    }

    #[test]
    fn settings_no_longer_ask_how_often_to_autosave() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[]);

        click_named(&ctx, &mut editor, "Settings");
        let_dialog_open(&ctx, &mut editor);

        assert!(is_named(&ctx, &mut editor, "Fastest"));
        assert!(!is_named(&ctx, &mut editor, "5 seconds"));
    }

    /// A saved project with one clip on the timeline, and a second clip added since the save.
    fn with_a_change_since_saving(ctx: &egui::Context, name: &str) -> (Editor, PathBuf) {
        let mut editor = with_videos(ctx, &[test_clip()]);
        let path = project_folder(name).join("a.bareclip");
        assert!(editor.save_to(&path));
        let id = editor.media[0].id;
        editor.add_to_timeline(id);
        assert!(editor.has_unsaved_changes());
        (editor, path)
    }

    /// Makes Auto Save's wait over, runs a frame and lets its write finish.
    fn auto_save_now(ctx: &egui::Context, editor: &mut Editor) {
        editor.unsaved_since = Some(Instant::now() - Duration::from_secs(3600));
        run_frame(ctx, editor, vec![]);
        editor.wait_for_auto_save();
    }

    fn clips_in(path: &Path) -> usize {
        let file = ProjectFile::read(path).expect("the file can be read");
        file.tracks.iter().map(Vec::len).sum()
    }

    #[test]
    fn auto_save_starts_off_and_the_menu_turns_it_on_for_the_next_run_too() {
        let ctx = context();
        let folder = project_folder("auto-save-menu");
        let mut editor = with_storage(&ctx, &folder);
        assert!(!editor.settings.auto_save);

        click_named(&ctx, &mut editor, "Project menu");
        click_menu_item(&ctx, &mut editor, "Auto Save");
        assert!(editor.settings.auto_save);

        assert!(with_storage(&ctx, &folder).settings.auto_save);
    }

    #[test]
    fn with_auto_save_off_only_saving_changes_the_file() {
        let ctx = context();
        let (mut editor, path) = with_a_change_since_saving(&ctx, "auto-save-off");

        auto_save_now(&ctx, &mut editor);

        assert_eq!(clips_in(&path), 1);
        assert!(editor.has_unsaved_changes());
    }

    #[test]
    fn with_auto_save_on_a_change_reaches_the_file_after_five_seconds() {
        let ctx = context();
        let (mut editor, path) = with_a_change_since_saving(&ctx, "auto-save-on");
        editor.settings.auto_save = true;

        run_frame(&ctx, &mut editor, vec![]);
        editor.unsaved_since = Some(Instant::now() - Duration::from_secs(4));
        run_frame(&ctx, &mut editor, vec![]);
        editor.wait_for_auto_save();
        assert_eq!(clips_in(&path), 1, "too soon");

        auto_save_now(&ctx, &mut editor);

        assert_eq!(clips_in(&path), 2);
        assert!(!editor.has_unsaved_changes());
    }

    #[test]
    fn auto_save_writes_a_later_change_too() {
        let ctx = context();
        let (mut editor, path) = with_a_change_since_saving(&ctx, "auto-save-later");
        editor.settings.auto_save = true;
        auto_save_now(&ctx, &mut editor);

        let id = editor.media[0].id;
        editor.add_to_timeline(id);
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.unsaved_since.is_some(), "it waits again");
        assert_eq!(clips_in(&path), 2);

        auto_save_now(&ctx, &mut editor);
        assert_eq!(clips_in(&path), 3);
    }

    #[test]
    fn auto_save_leaves_a_project_with_no_file_unsaved_and_recoverable() {
        let ctx = context();
        let folder = project_folder("auto-save-unnamed");
        let mut editor = with_storage(&ctx, &folder);
        editor.settings.auto_save = true;
        editor.import(&ctx, &[test_clip().to_path_buf()]);
        settle(&ctx, &mut editor);

        auto_save_now(&ctx, &mut editor);
        autosave_now(&ctx, &mut editor);

        assert!(editor.project_path.is_none());
        assert!(editor.has_unsaved_changes());
        crash(editor);
        assert_eq!(storage::orphaned_recoveries(&folder).len(), 1);
    }

    #[test]
    fn writing_the_file_removes_the_recovery_copy() {
        let ctx = context();
        let folder = project_folder("auto-save-clears");
        let path = folder.join("a.bareclip");
        let mut editor = with_storage(&ctx, &folder);
        editor.import(&ctx, &[test_clip().to_path_buf()]);
        settle(&ctx, &mut editor);
        assert!(editor.save_to(&path));
        editor.settings.auto_save = true;
        let id = editor.media[0].id;
        editor.add_to_timeline(id);
        autosave_now(&ctx, &mut editor);
        assert!(editor.last_autosaved.is_some(), "the copy goes first");

        auto_save_now(&ctx, &mut editor);
        run_frame(&ctx, &mut editor, vec![]);
        crash(editor);

        assert_eq!(clips_in(&path), 1);
        assert!(storage::orphaned_recoveries(&folder).is_empty());
    }

    #[test]
    fn with_auto_save_on_leaving_saves_instead_of_asking() {
        let ctx = context();
        let (mut editor, path) = with_a_change_since_saving(&ctx, "auto-save-leave");
        editor.settings.auto_save = true;

        run_frame(&ctx, &mut editor, key_event(Key::N, Modifiers::COMMAND));

        assert!(editor.leave_for.is_none());
        assert_eq!(clips_in(&path), 2);
        assert!(editor.project.tracks.is_empty());
    }

    #[test]
    fn with_auto_save_on_closing_saves_instead_of_asking() {
        let ctx = context();
        let (mut editor, path) = with_a_change_since_saving(&ctx, "auto-save-close");
        editor.settings.auto_save = true;

        editor.handle_close_request(&ctx);

        assert!(editor.leave_for.is_none());
        assert_eq!(clips_in(&path), 2);
    }

    #[test]
    fn with_auto_save_on_a_project_with_no_file_still_asks_before_closing() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.settings.auto_save = true;

        editor.handle_close_request(&ctx);

        assert_eq!(editor.leave_for, Some(LeaveFor::CloseWindow));
    }

    #[test]
    fn saving_by_hand_during_an_auto_save_keeps_the_newest_work() {
        let ctx = context();
        let (mut editor, path) = with_a_change_since_saving(&ctx, "auto-save-race");
        editor.settings.auto_save = true;
        editor.unsaved_since = Some(Instant::now() - Duration::from_secs(3600));
        run_frame(&ctx, &mut editor, vec![]);
        let id = editor.media[0].id;
        editor.add_to_timeline(id);

        assert!(editor.save_to(&path));

        assert!(editor.auto_save_write.is_none());
        assert_eq!(clips_in(&path), 3);
        assert!(!editor.has_unsaved_changes());
    }

    #[test]
    fn a_failed_auto_save_reports_once_keeps_the_work_unsaved_and_tries_again_after_a_change() {
        let ctx = context();
        let (mut editor, path) = with_a_change_since_saving(&ctx, "auto-save-fails");
        editor.settings.auto_save = true;
        // A folder where the temporary file has to go makes the write fail.
        let blocker = path.with_file_name("a.bareclip.tmp");
        std::fs::create_dir(&blocker).expect("the folder can be created");

        auto_save_now(&ctx, &mut editor);
        assert!(editor.has_unsaved_changes());
        assert!(editor.notice.take().is_some());

        auto_save_now(&ctx, &mut editor);
        assert!(editor.notice.is_none(), "the same work isn't tried again");

        std::fs::remove_dir(&blocker).expect("the folder can be removed");
        let id = editor.media[0].id;
        editor.add_to_timeline(id);
        auto_save_now(&ctx, &mut editor);
        assert_eq!(clips_in(&path), 3);
        assert!(!editor.has_unsaved_changes());
    }

    #[test]
    fn auto_save_never_cuts_a_missing_video_out_of_the_file() {
        let ctx = context();
        let (project, video) = project_with_movable_video(&ctx, "auto-save-missing");
        once_free(|| std::fs::remove_file(&video));
        let mut editor = Editor::new(&ctx, false);
        editor.settings.auto_save = true;
        run_frame(&ctx, &mut editor, vec![]);
        editor.open_project(&ctx, &project);
        let_dialog_open(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Open without them");
        settle(&ctx, &mut editor);
        assert!(editor.has_unsaved_changes());
        assert!(
            editor
                .notice
                .as_deref()
                .is_some_and(|n| n.contains("Auto Save"))
        );

        auto_save_now(&ctx, &mut editor);
        editor.handle_close_request(&ctx);

        assert_eq!(clips_in(&project), 1);
        assert_eq!(editor.leave_for, Some(LeaveFor::CloseWindow));

        editor.leave_for = None;
        assert!(editor.save_to(&project));
        assert!(!editor.auto_save_held, "saving on purpose lifts the hold");
    }

    #[test]
    fn dragging_a_clip_into_empty_space_puts_it_there_and_undo_brings_it_back() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let before = editor.project.clone();
        let scale = editor.timeline_state.scale;
        let y = editor.timeline_state.track.center().y;

        pointer_drag(
            &ctx,
            &mut editor,
            pos2(scale.x_of(Ticks::from_seconds(1.0)), y),
            pos2(scale.x_of(Ticks::from_seconds(1.5)), y),
        );

        let clip = editor.project.tracks.played().clips()[0];
        assert!((clip.start.as_seconds() - 0.5).abs() < 0.1, "{clip:?}");
        assert_eq!(editor.project.tracks.gaps().len(), 1);

        editor.undo();
        assert_eq!(editor.project, before);
    }

    #[test]
    fn a_dragged_clip_snaps_back_against_the_start() {
        let ctx = context();
        let mut editor = with_leading_gap(&ctx);
        let scale = editor.timeline_state.scale;
        let y = editor.timeline_state.track.center().y;
        let px_per_second = scale.x_of(Ticks::from_seconds(1.0)) - scale.x_of(Ticks(0));

        // Ends just short of the start of the timeline, within snapping distance.
        let from = scale.x_of(Ticks::from_seconds(2.0));
        pointer_drag(
            &ctx,
            &mut editor,
            pos2(from, y),
            pos2(from - px_per_second + 4.0, y),
        );

        assert_eq!(editor.project.tracks.played().clips()[0].start, Ticks(0));
        assert!(editor.project.tracks.gaps().is_empty());
    }

    #[test]
    fn dragging_a_clip_past_another_changes_the_order() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip(), second_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        let first = editor.project.tracks.played().clips()[0].id;
        let scale = editor.timeline_state.scale;
        let y = editor.timeline_state.track.center().y;

        pointer_drag(
            &ctx,
            &mut editor,
            pos2(scale.x_of(Ticks::from_seconds(2.0)), y),
            pos2(scale.x_of(Ticks::from_seconds(7.9)), y),
        );

        assert_eq!(editor.project.tracks.played().clips()[1].id, first);

        assert_eq!(editor.project.tracks.gaps().len(), 1);

        editor.undo();
        assert_eq!(editor.project.tracks.played().clips()[0].id, first);
    }

    #[test]
    fn dragging_on_the_ruler_moves_the_playhead_only() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        let before = editor.project.clone();
        let scale = editor.timeline_state.scale;
        let y = editor.timeline_state.track.top() - 12.0;

        pointer_drag(
            &ctx,
            &mut editor,
            pos2(scale.x_of(Ticks::from_seconds(1.0)), y),
            pos2(scale.x_of(Ticks::from_seconds(3.0)), y),
        );

        assert!((editor.position().as_seconds() - 3.0).abs() < 0.1);
        assert_eq!(editor.project, before);
    }

    /// Presses at one point, moves in steps to another, and lets go.
    fn pointer_drag(ctx: &egui::Context, editor: &mut Editor, from: egui::Pos2, to: egui::Pos2) {
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        run_frame(ctx, editor, vec![egui::Event::PointerMoved(from)]);
        run_frame(ctx, editor, vec![button(from, true)]);
        for step in 1..=8 {
            let point = from + (to - from) * (step as f32 / 8.0);
            run_frame(ctx, editor, vec![egui::Event::PointerMoved(point)]);
        }
        run_frame(ctx, editor, vec![button(to, false)]);
        run_frame(ctx, editor, vec![]);
    }

    #[test]
    fn shortcuts_are_ignored_while_a_dialog_is_open() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.export_state = ExportState::Choosing;
        editor.seek(Ticks::from_seconds(1.0));

        run_frame(&ctx, &mut editor, press(Key::S));
        run_frame(&ctx, &mut editor, press(Key::Space));

        assert_eq!(clip_count(&editor), 1);
        assert!(!editor.preview.is_playing());
    }

    #[test]
    fn space_toggles_playback() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);

        run_frame(&ctx, &mut editor, press(Key::Space));
        assert!(editor.preview.is_playing());

        run_frame(&ctx, &mut editor, press(Key::Space));
        assert!(!editor.preview.is_playing());
    }

    #[test]
    fn shift_arrows_step_one_frame_exactly() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.seek(Ticks::from_seconds(1.0));

        run_frame(&ctx, &mut editor, shift_key(Key::ArrowRight));
        let gap = editor.position().as_seconds() - 1.0;
        assert!((gap - 1.0 / 30.0).abs() < 0.002, "stepped {gap}s");

        run_frame(&ctx, &mut editor, shift_key(Key::ArrowLeft));
        assert!((editor.position().as_seconds() - 1.0).abs() < 0.002);
    }

    #[test]
    fn the_dialog_describes_what_the_export_will_do() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let id = editor.project.tracks.played().clips()[0].id;
        let duration = editor.media[0].info.duration;
        let min = Ticks::from_seconds(MIN_CLIP_SECONDS);
        editor
            .project
            .tracks
            .trim(id, ClipEdge::Start, Ticks::from_seconds(0.5), duration, min);
        editor
            .project
            .tracks
            .trim(id, ClipEdge::End, Ticks::from_seconds(2.5), duration, min);

        let exact = editor.plan_description().expect("the frames are read");
        editor.mode = CutMode::Keyframe;
        let fast = editor.plan_description().expect("the frames are read");

        assert!(exact.contains("30 frames are copied"), "{exact}");
        assert!(exact.contains("30 around the cuts"), "{exact}");
        assert!(fast.contains("All 90 frames are copied"), "{fast}");
    }

    #[test]
    fn videos_in_the_same_format_have_no_export_problem() {
        let ctx = context();
        let editor = with_videos(&ctx, &[test_clip(), second_clip()]);

        assert_eq!(editor.export_problem(), None);
    }

    #[test]
    fn videos_in_different_formats_explain_the_problem_before_exporting() {
        let ctx = context();
        let editor = with_videos(&ctx, &[test_clip(), small_clip()]);

        let problem = editor.export_problem().expect("these cannot be joined");

        assert!(problem.contains("160 x 90"), "{problem}");
    }

    fn wait_for_export(ctx: &egui::Context, editor: &mut Editor) {
        let started = Instant::now();
        while matches!(editor.export_state, ExportState::Running(_)) {
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "the export took too long"
            );
            run_frame(ctx, editor, vec![]);
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn exporting_writes_the_timeline_in_order() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip(), second_clip()]);
        let output =
            std::env::temp_dir().join(format!("bareclip-ui-export-{}.mp4", std::process::id()));
        let _ = std::fs::remove_file(&output);

        editor.start_export_to(&ctx, output.clone());
        wait_for_export(&ctx, &mut editor);

        let ExportState::Done {
            output: written, ..
        } = &editor.export_state
        else {
            panic!("the export did not finish");
        };
        assert_eq!(written, &output);
        let info = probe(&output).expect("the export can be read");
        assert!((info.duration.as_seconds() - 8.0).abs() < 0.1);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn a_failed_export_explains_itself() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let output = std::env::temp_dir()
            .join("bareclip-no-such-folder")
            .join("out.mp4");

        editor.start_export_to(&ctx, output);
        wait_for_export(&ctx, &mut editor);

        assert!(matches!(editor.export_state, ExportState::Failed(_)));
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(clip_count(&editor), 1);
    }

    #[test]
    fn exporting_over_an_original_is_refused() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let before = std::fs::metadata(test_clip())
            .expect("the clip exists")
            .len();

        editor.start_export_to(&ctx, test_clip().to_owned());
        wait_for_export(&ctx, &mut editor);

        let ExportState::Failed(message) = &editor.export_state else {
            panic!("the export should have been refused");
        };
        assert!(message.contains("original stays untouched"), "{message}");
        assert_eq!(
            std::fs::metadata(test_clip())
                .expect("the clip exists")
                .len(),
            before
        );
    }
}
