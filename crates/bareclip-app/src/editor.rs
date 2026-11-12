//! The editing screen: a media panel, the preview, and the timeline below them.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, Receiver},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime},
};

use bareclip_audio::{Bus, MixSpan};
use bareclip_core::{
    art::Art,
    canvas::{BLACK, Canvas, Colour, ORIGINAL_NAME},
    history::History,
    picture::StillPicture,
    picture_effects::EffectKind,
    placement::Placement,
    project::{self, ProjectFile, write_atomic},
    sound::ClipSound,
    splice::{CutMode, FrameIndex},
    time::{Ticks, format_timecode},
    timeline::{ClipEdge, ClipId, MediaId, STILL_LENGTH, TimeRange, Timeline},
    title::{Title, TitleStyle},
    tracks::{OrderMove, TrackTarget, Tracks},
};
use bareclip_export::{
    CHOICES_REASON, ExportChoices, ExportRate, ExportSize, FullPlan, GIF_RATES,
    GIF_WARNING_SECONDS, LAYERED_REASON, LayeredClip, LayeredRequest, LayeredStretch,
    ProjectReport, Route, SIZES, SPEED_REASON, STILLS_REASON, SoundPlan, TITLES_REASON,
    copying_suits, export_layered_as, export_sound_only, export_with_new_sound, full_output_path,
    gif_size_for, gif_sizes_on_offer, mixed_rates, rates_on_offer, route_for, size_problem,
    size_with_shorter_side, sizes_on_offer, sound_labels,
};
use bareclip_media::{
    DecodeStep, ExportPart, ExportRequest, MediaError, MediaInfo, PreviewFrame, Quality,
    SoundLabel, SoundShape, StripPicture, VideoFormat, VideoPreview, can_write_gaps, export,
    picture_spacing, plan_export, probe, read_sound_shape, read_still, read_strip_pictures,
    scan_frames,
};
use bareclip_text::FontList;
use eframe::egui::{
    self, Align, Color32, CornerRadius, FontFamily, FontId, Frame, Key, Layout, Margin, Modifiers,
    RichText, Sense, Stroke, TextureHandle, TextureOptions, UiBuilder, pos2, vec2,
};

use crate::{
    components::{
        self, ActionButton, CheckBox, ColourSwatch, IconButton, Kind, MenuItem, NAME_BAR_PADDING,
        NameBar, SwitchRow,
    },
    fonts, hotkeys, icons,
    layers::{LayerDisplay, LayerPictures, Moment},
    preview::{PREPARE_AHEAD_SECONDS, Preview},
    stage::{self, StageClip, StageState, StageView},
    storage::{self, ExportCuts, GpuChoice, Recovery, RecoveryRecord, Settings},
    text,
    theme::{self, Palette},
    timeline::{
        self, ClipFace, ClipView, MediaDrag, MenuAction, TimelineDrag, TimelineState, TimelineView,
    },
};

const TRANSITIONS_REASON: &str = "Transitions show two clips at once, so the video is drawn again.";
const LOOKS_REASON: &str =
    "Filters, colours and picture fades change the picture, so the video is drawn again.";
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
/// How long a deleted recording keeps being tried, while whatever was reading it lets go.
const TRASH_RETRY_FOR: Duration = Duration::from_secs(3);
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
/// The panel's card comes out about 241 wide, which is 301 pixels on a 125%
/// display.
const MEDIA_PANEL_WIDTH: f32 = 250.0;
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
const TITLE_STRIP_HEIGHT: f32 = 48.0;
const WINDOW_BUTTON_SIZE: f32 = 32.0;
const PROJECT_NAME_WIDTH: f32 = 220.0;
const SETTINGS_ROW_GAP: f32 = 14.0;
/// The buttons that skip 5 seconds: an arrow arching over the number, each with room to be read.
const SKIP_ARROW_SIZE: f32 = 22.0;
const SKIP_ARROW_RAISE: f32 = 6.0;
const SKIP_NUMBER_SIZE: f32 = 12.0;
const SKIP_NUMBER_DROP: f32 = 7.0;
/// The whole Settings dialog: its tabs down the left, a line, and the open tab's page beside them.
const SETTINGS_WIDTH: f32 = 626.0;
const SETTINGS_TABS_WIDTH: f32 = 124.0;
/// The room either side of the line between the tabs and the page.
const SETTINGS_TABS_GAP: f32 = 14.0;
/// As tall as the tallest tab's page, so the dialog keeps its size when tabs are switched.
const SETTINGS_PAGE_HEIGHT: f32 = 450.0;
const SETTINGS_CHOICE_WIDTH: f32 = 96.0;
/// Room for a whole chip name, such as NVIDIA GeForce RTX 3060 Laptop GPU.
const GPU_DROPDOWN_WIDTH: f32 = 250.0;

/// The tabs of the Settings dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsTab {
    General,
    Recording,
    Keybinds,
}

impl SettingsTab {
    /// Each tab's symbol, label and name for screen readers, which can't be confused with the
    /// Export button.
    const ALL: [(Self, icons::Icon, &'static str, &'static str); 3] = [
        (
            Self::General,
            icons::SLIDERS_HORIZONTAL,
            "General",
            "General settings",
        ),
        (
            Self::Recording,
            icons::VIDEO,
            "Recording",
            "Recording settings",
        ),
        (
            Self::Keybinds,
            icons::KEYBOARD,
            "Keybinds",
            "Keybinds settings",
        ),
    ];
}

/// A graphics chip, as Settings lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuEntry {
    pub id: GpuChoice,
    pub name: String,
}

impl GpuEntry {
    pub fn of(info: &eframe::wgpu::AdapterInfo) -> Self {
        Self {
            id: GpuChoice {
                vendor: info.vendor,
                device: info.device,
            },
            name: info.name.clone(),
        }
    }
}

/// A shape in the Size menu: what it is called, and what it is for, which shows when the pointer
/// rests on it.
struct SizeChoice {
    ratio: (u32, u32),
    name: &'static str,
    numbers: &'static str,
    note: &'static str,
}

/// The shapes in the Size menu, the commonly used ones first. A test holds these to `RATIOS`.
const SIZE_GROUPS: [&[SizeChoice]; 2] = [
    &[
        SizeChoice {
            ratio: (16, 9),
            name: "Wide",
            numbers: "16:9",
            note: "YouTube and streaming sites",
        },
        SizeChoice {
            ratio: (9, 16),
            name: "Vertical",
            numbers: "9:16",
            note: "Instagram Reels and TikTok",
        },
        SizeChoice {
            ratio: (1, 1),
            name: "Square",
            numbers: "1:1",
            note: "Instagram posts",
        },
    ],
    &[
        SizeChoice {
            ratio: (4, 3),
            name: "Classic",
            numbers: "4:3",
            note: "Older TVs and slides",
        },
        SizeChoice {
            ratio: (4, 5),
            name: "Social",
            numbers: "4:5",
            note: "Instagram and Facebook feeds",
        },
        SizeChoice {
            ratio: (21, 9),
            name: "Cinema",
            numbers: "21:9",
            note: "Ultrawide, film look",
        },
        SizeChoice {
            ratio: (2, 3),
            name: "Portrait",
            numbers: "2:3",
            note: "Pinterest and tall photos",
        },
    ],
];
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
/// The logo on an empty stage, the gap under it and the room for the words under that.
const EMPTY_LOGO_SIZE: egui::Vec2 = vec2(96.0, 111.0);
const EMPTY_LOGO_GAP: f32 = 18.0;
const EMPTY_LOGO_SMALLEST: f32 = 40.0;
const EMPTY_HINT_TEXT_ROOM: f32 = 28.0;
const DIALOG_RADIUS: u8 = 10;
/// A dialog's own buttons are never narrower than this, so short names still feel solid.
const DIALOG_BUTTON_WIDTH: f32 = 84.0;
/// How far a dialog stays from the window's edges.
const DIALOG_WINDOW_MARGIN: f32 = 16.0;
const STILL_THUMBNAIL_WIDTH: usize = 320;
const SNAPPED_CLOSE_ENOUGH: i64 = 64;
const EXPORT_DROPDOWN_WIDTH: f32 = 128.0;
const MATCH_SOURCE: &str = "Match source";
/// The playback buttons: four of 32, the play button, and the spaces between them.
const TRANSPORT_WIDTH: f32 = 180.0;
/// Without the buttons for the start and end, and with whole seconds.
const TINY_TRANSPORT_WIDTH: f32 = 100.0;
const TINY_TIME_READOUT_WIDTH: f32 = 100.0;
const CONTROL_SPACING: f32 = 4.0;
const CARD_TOP_ROW_HEIGHT: f32 = 48.0;
/// These include the transport row.
const TIMELINE_HEIGHT: f32 = 330.0;
const TIMELINE_MIN_HEIGHT: f32 = 230.0;
/// Whatever the timeline's height, the preview keeps this share of the room.
const TIMELINE_MAX_SHARE: f32 = 0.65;
/// How far the arrow keys step in dead space, which has no frames of its own.
const GAP_STEP_SECONDS: f64 = 1.0 / 30.0;
/// The smallest the window can be, which fits a quarter of a 1080p screen at 150% scaling.
pub const SMALLEST_WINDOW: [f32; 2] = [640.0, 340.0];
/// Below this width the side panels float over the video, and the rails lose their names.
const NARROW_WIDTH: f32 = 1000.0;
/// Below this height the rails lose their names and the bars get a little shorter.
const SHORT_HEIGHT: f32 = 600.0;
/// Below this width the rows hold only what they can't do without.
const TINY_WIDTH: f32 = 720.0;
const COMPACT_RAIL_WIDTH: f32 = 56.0;
const SHORT_BAR_HEIGHT: f32 = 40.0;
const SHORT_TRANSPORT_HEIGHT: f32 = 44.0;
const TINY_PROJECT_NAME_WIDTH: f32 = 120.0;
/// What a short window's timeline needs below its transport row: the ruler, one row and the
/// scroll bar.
const SHORT_TIMELINE_ROOM: f32 = 130.0;
/// How much a short window keeps for the video before the timeline can grow.
const SHORT_PREVIEW_ROOM: f32 = 0.5;
/// How far a floating panel stays from the window's bottom edge.
const FLOATING_PANEL_GAP: f32 = 8.0;

/// How the window's size shapes the screen this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Fit {
    narrow: bool,
    short: bool,
    tiny: bool,
}

impl Fit {
    fn of(size: egui::Vec2) -> Self {
        Self {
            narrow: size.x < NARROW_WIDTH,
            short: size.y < SHORT_HEIGHT,
            tiny: size.x < TINY_WIDTH,
        }
    }

    /// The rails show symbols only, and say their names when hovered.
    fn compact_rails(self) -> bool {
        self.narrow || self.short
    }

    fn rail_width(self) -> f32 {
        if self.compact_rails() {
            COMPACT_RAIL_WIDTH
        } else {
            RAIL_WIDTH
        }
    }

    /// The panels beside the rails go over the video instead of squeezing it.
    fn floating_panels(self) -> bool {
        self.narrow
    }

    fn title_height(self) -> f32 {
        if self.short {
            SHORT_BAR_HEIGHT
        } else {
            TITLE_STRIP_HEIGHT
        }
    }

    fn top_row_height(self) -> f32 {
        if self.short {
            SHORT_BAR_HEIGHT
        } else {
            CARD_TOP_ROW_HEIGHT
        }
    }

    fn transport_height(self) -> f32 {
        if self.short {
            SHORT_TRANSPORT_HEIGHT
        } else {
            TRANSPORT_HEIGHT
        }
    }
}

/// Which side of the window a panel opens from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

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
    let want_pictures = want_pictures && !info.is_sound_only();
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
    /// An image has no frames to find, only itself to read.
    Still(Result<StillPicture, MediaError>),
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
    Videos,
    Audio,
    Images,
    OnTimeline,
    NotOnTimeline,
}

impl MediaFilter {
    const ALL: [(Self, &'static str); 6] = [
        (Self::All, "Everything"),
        (Self::Videos, "Videos"),
        (Self::Audio, "Audio"),
        (Self::Images, "Images"),
        (Self::OnTimeline, "On the timeline"),
        (Self::NotOnTimeline, "Not on the timeline"),
    ];

    fn keeps(self, used: bool, info: &MediaInfo) -> bool {
        match self {
            Self::All => true,
            Self::Videos => !info.is_sound_only() && !info.is_still(),
            Self::Audio => info.is_sound_only(),
            Self::Images => info.is_still(),
            Self::OnTimeline => used,
            Self::NotOnTimeline => !used,
        }
    }
}

/// A video in the project.
/// Why a project with nothing but sound is drawn from nothing.
const ONLY_SOUND_REASON: &str = "This project only has sound, so the picture is its background.";
/// Why a project whose sound outlasts its pictures can't be copied.
const SOUND_OUTLASTS_REASON: &str =
    "Sound carries on after the last video, so the background has to be drawn behind it.";
/// Why a file that can't hold the new sound is made into another.
const NEW_SOUND_REASON: &str =
    "The sound has changed, and this kind of file can't hold the new sound, so it becomes an MP4.";
/// Why video that can't be cut exactly is redrawn when its sound changes.
const EXACT_CUTS_REASON: &str =
    "The sound has changed, and this video can't be cut exactly to stay in time with it.";
/// The kinds of file known to take the sound a mix makes alongside a copied picture.
const CONTAINERS_FOR_NEW_SOUND: [&str; 4] = ["mp4", "m4v", "mov", "mkv"];

/// Everything that can be heard, as one mix.
fn heard_spans(media: &[MediaEntry], tracks: &Tracks) -> Vec<MixSpan> {
    let entry = |id: MediaId| media.iter().find(|entry| entry.id == id);
    let has_sound = |id: MediaId| entry(id).is_some_and(|entry| !entry.info.audio.is_empty());
    tracks
        .sound_spans(&has_sound)
        .into_iter()
        .filter_map(|span| {
            Some(MixSpan {
                path: entry(span.media)?.info.path.clone(),
                start: span.start,
                source: span.source,
                sound: span.sound,
                speed: span.speed,
                bus: Bus::All(0),
            })
        })
        .collect()
}

/// How to join videos with different frame rates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RatePick {
    Copy,
    Redraw,
}

#[path = "camera_recording.rs"]
mod camera_recording;
#[path = "captions.rs"]
mod captions;
#[path = "library.rs"]
mod library;
#[path = "looks.rs"]
mod looks;
#[path = "recording.rs"]
mod recording;
#[path = "screen_recording.rs"]
mod screen_recording;
#[path = "titles.rs"]
mod titles;
use looks::LookPictures;
use stage::{LookTab, RightPanel};
use titles::{CardHover, CopiedClip, DurationEdit, LeftPanel, TitleCard, TitleResize};

struct MediaEntry {
    id: MediaId,
    /// What a title's picture was last drawn from, for what canvas, and the scale that shows it.
    title_drawn: Option<(Title, (u32, u32), i32)>,
    info: MediaInfo,
    /// What it is called on screen: a name given in this project, or else `file_name`.
    name: String,
    file_name: String,
    /// When the file was created, on file systems that record it.
    created: Option<SystemTime>,
    /// Whether BareClip recorded it, so it can be deleted from My media.
    recorded: bool,
    index: Option<Arc<FrameIndex>>,
    scan: Option<Receiver<ScanMessage>>,
    cancel_scan: Arc<AtomicBool>,
    progress: Arc<AtomicU32>,
    thumbnail: Option<TextureHandle>,
    /// What its clips show, as far as it has been read, with pictures in order of time.
    strip_pictures: Vec<(Ticks, TextureHandle)>,
    strip_pictures_done: bool,
    /// What an image shows, once it has been read.
    still: Option<Arc<StillPicture>>,
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
    /// The finished video's size.
    canvas: Canvas,
    /// What each title says and how it looks, under the id its clip refers to.
    titles: BTreeMap<MediaId, Title>,
    /// Names given to videos in this project. The files themselves are never renamed.
    names: BTreeMap<MediaId, String>,
    /// Things from the library with a card in My media, in the order they were added.
    library: Vec<Art>,
}

/// A trim in progress: where the clip's edge and the playhead were when the drag started.
#[derive(Clone, Copy, Debug)]
struct TrimOrigin {
    source: Ticks,
    pointer: Ticks,
    playhead: Ticks,
}

struct ExportTask {
    result: Receiver<Result<ProjectReport, MediaError>>,
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
        report: ProjectReport,
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
    /// What each clip under the top one is showing, when several show at once.
    layer_pictures: LayerPictures,
    /// Where layers get drawn. Without it, only the top clip shows.
    layer_display: Option<LayerDisplay>,
    /// The graphics chips this PC has. Empty until the window has opened.
    gpu_chips: Vec<GpuEntry>,
    /// The chip the window really runs on.
    running_gpu: Option<GpuEntry>,
    /// What Settings said when the app started, which is what this run is using.
    gpu_at_start: Option<GpuChoice>,
    stage_state: StageState,
    /// The project from before a change on the stage began, until it becomes one undo step.
    stage_before: Option<Snapshot>,
    /// A colour code partway through being typed.
    background_text: Option<String>,
    selected: Option<ClipId>,
    /// Other clips selected along with `selected`, with Ctrl, Shift or Ctrl+A. They belong to
    /// `group_anchor`, so anything that selects a clip some other way leaves them behind.
    also_selected: Vec<ClipId>,
    group_anchor: Option<ClipId>,
    /// A moment in the dead space that was clicked on.
    selected_gap: Option<Ticks>,
    timeline_state: TimelineState,
    trim_origin: Option<TrimOrigin>,
    notice: Option<String>,
    export_state: ExportState,
    mode: CutMode,
    /// How to join videos with different frame rates, if the user has picked for this export.
    rate_pick: Option<RatePick>,
    /// Keep game, mic and music on tracks of their own in this export, rather than one mix.
    keep_sound_apart: bool,
    /// Size, quality and frame rate of the finished video in this export.
    export_choices: ExportChoices,
    /// This export saves only the sound.
    sound_only: bool,
    /// The lane for adding audio was pressed, so a file picker opens once this frame is drawn.
    add_audio_requested: bool,
    confirm_remove: Vec<MediaId>,
    /// A recording waiting for a yes before it is deleted.
    confirm_delete_recording: Option<MediaId>,
    /// How a file is moved to the Recycle Bin, which tests replace so nothing reaches the real one.
    trash_file: TrashFile,
    /// Recordings being moved to the Recycle Bin, each saying how it went.
    trashing: Vec<Receiver<Result<(), String>>>,
    project_name: String,
    /// The file this project saves to, once it has one.
    project_path: Option<PathBuf>,
    /// The project as it last was on disk, which unsaved changes are measured against.
    baseline: ProjectFile,
    leave_for: Option<LeaveFor>,
    missing_videos: Option<MissingVideos>,
    /// The name being typed, while the project is being renamed.
    renaming: Option<String>,
    /// The video being renamed in the panel, and what has been typed so far.
    media_rename: Option<(MediaId, String)>,
    window_title: String,
    closing: bool,
    settings: Settings,
    settings_open: bool,
    settings_tab: SettingsTab,
    /// The microphones Settings offers as the default, read when its Recording tab is first
    /// shown and let go when it closes, so devices aren't listed every frame.
    settings_microphones: Option<Vec<bareclip_audio::Microphone>>,
    /// The recording key Settings is waiting for new keys for, and why the last ones pressed
    /// couldn't be used.
    waiting_for_key: Option<hotkeys::RecordingKey>,
    key_problem: Option<String>,
    /// Where settings and recovery copies live. Without it, the app keeps nothing.
    app_folder: Option<PathBuf>,
    recovery: Option<Recovery>,
    seek_bar_held: bool,
    /// Playback was paused for the playhead being moved by hand, and carries on afterwards.
    resume_after_scrub: bool,
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
    /// Recordings that runs didn't stop, waiting to be offered.
    cut_short: Vec<storage::CutShort>,
    /// The recording bar, while it is open.
    record_bar: Option<recording::RecordBar>,
    /// Where the picture's area was last drawn, which the recording bar floats over.
    stage_rect: egui::Rect,
    /// How microphones are found and opened, which tests replace with made-up sound.
    open_microphone: recording::OpenMicrophone,
    list_microphones: recording::ListMicrophones,
    /// The screen recording bar, while it is open, and how screens and the PC's sound are opened,
    /// which tests replace too.
    screen_bar: Option<screen_recording::ScreenBar>,
    open_screen: screen_recording::OpenScreen,
    open_pc_sound: screen_recording::OpenPcSound,
    open_app_sound: screen_recording::OpenAppSound,
    list_sources: screen_recording::ListSources,
    /// The camera recording bar, while it is open, and how cameras are found and opened, which
    /// tests replace with a made-up camera.
    camera_bar: Option<camera_recording::CameraBar>,
    open_camera: camera_recording::OpenCamera,
    list_cameras: camera_recording::ListCameras,
    /// How a folder to save recordings in is picked, which tests replace too.
    pick_folder: recording::PickFolder,
    /// Swapped in tests, so no test starts a real BareClip.
    relaunch: Relaunch,
    /// Restart now was pressed in Settings this frame.
    restart_asked: bool,
    /// Files from "Edit with BareClip", waiting for any dialog that decides what the project is.
    waiting_edits: Vec<PathBuf>,
    /// Videos ticked in the panel, in the order they were ticked.
    selected_media: Vec<MediaId>,
    media_panel_open: bool,
    /// What the left panel shows when it is open.
    left_panel: LeftPanel,
    /// The last number used for a title's id. They are never used again, even after undo.
    next_title: u64,
    /// The undo step for a title being typed in or having a control held.
    title_before: Option<Snapshot>,
    title_resize: Option<TitleResize>,
    title_cards: HashMap<TitleStyle, TitleCard>,
    /// Where the Content library is, and what is typed in its search box.
    library_view: library::LibraryView,
    /// Each library item drawn for its card, the first time it is shown.
    library_thumbnails: HashMap<&'static str, TextureHandle>,
    /// The library card the pointer is on, if it moves.
    library_hover: Option<library::LibraryHover>,
    /// The clip whose transition out of it is selected on the timeline.
    selected_join: Option<ClipId>,
    /// A clip's menu opened from the picture, where, and whether it opened this frame.
    picture_menu: Option<(ClipId, egui::Pos2, bool)>,
    transition_search: String,
    filter_search: String,
    effect_search: String,
    /// The effect whose settings are open in the Effects tab.
    effect_settings: Option<EffectKind>,
    look_pictures: LookPictures,
    /// What the panel on the right is showing, or `None` while it is closed.
    right_panel: Option<RightPanel>,
    /// The card the pointer is resting on, which starts moving after a moment.
    card_hover: Option<CardHover>,
    /// Whether all the text styles are showing, after "See all" was pressed.
    all_text_styles: bool,
    title_frames: crate::title_frames::TitleFrames,
    /// The title that gets the logo picked next.
    pick_logo_for: Option<MediaId>,
    title_colour_text: Option<String>,
    /// Whether a change to a caption's look goes to every caption on its row.
    style_whole_row: bool,
    /// The text box should take the keyboard when it next shows.
    focus_title_text: bool,
    pick_captions_requested: bool,
    save_captions_requested: bool,
    /// The caption whose box should take the keyboard next, and the row to scroll to for it.
    captions_focus: Option<(ClipId, usize)>,
    font_list: Option<FontList>,
    font_search: String,
    /// Each font's name drawn in itself, or `None` if it can't show it.
    font_faces: HashMap<String, Option<TextureHandle>>,
    /// Fonts already reported as missing, so a redraw doesn't keep saying so.
    fonts_reported: Vec<String>,
    /// Clips copied to be pasted, in order of time.
    clipboard: Vec<CopiedClip>,
    duration_edit: Option<DurationEdit>,
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
    /// How the window's size shapes the screen, and whether panels floated last frame.
    fit: Fit,
    floated: bool,
    /// The timeline's height in a short window, once dragged there. Until then it is as short
    /// as it goes, so the video gets the room.
    short_timeline_height: Option<f32>,
    /// The free stretch of the row above the picture, for the clip's toolbar.
    row_above: Option<egui::Rangef>,
    /// Where things were drawn in the last frame.
    left_rail_rect: egui::Rect,
    right_rail_rect: egui::Rect,
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
    /// Opens BareClip again on the same project, for a new graphics chip to take effect.
    Restart,
}

/// Starts another BareClip, opening this project file if there is one.
type Relaunch = fn(Option<&Path>) -> std::io::Result<()>;

fn relaunch_bareclip(project: Option<&Path>) -> std::io::Result<()> {
    let mut command = std::process::Command::new(std::env::current_exe()?);
    if let Some(project) = project {
        command.arg(project);
    }
    command.spawn().map(drop)
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

/// Moves a file to the Recycle Bin. Tests swap it for one that never reaches the real one.
type TrashFile = fn(&Path) -> Result<(), String>;

/// Moves a file to the Recycle Bin, quietly, so it can be brought back from there.
fn move_to_recycle_bin(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW,
        SHFileOperationW,
    };
    let absolute = std::path::absolute(path).map_err(|error| error.to_string())?;
    if !absolute.exists() {
        return Err("The file isn't there any more.".to_owned());
    }
    // The list of files ends in two zeros.
    let from: Vec<u16> = absolute.as_os_str().encode_wide().chain([0, 0]).collect();
    let mut operation = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: from.as_ptr(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI) as u16,
        ..Default::default()
    };
    // SAFETY: the structure and the list it points to live until the call returns.
    let result = unsafe { SHFileOperationW(&mut operation) };
    if result != 0 || operation.fAnyOperationsAborted != 0 {
        return Err(format!("Windows gave error {result}."));
    }
    if absolute.exists() {
        return Err("It is still in use.".to_owned());
    }
    Ok(())
}

/// What was pressed on the media cards this frame.
#[derive(Default)]
struct CardActions {
    add: Option<MediaId>,
    remove: Option<MediaId>,
    delete_recording: Option<MediaId>,
    toggle: Option<MediaId>,
    rename: Option<(MediaId, String)>,
}

/// How a media card is drawn just now.
struct CardMode {
    /// Squeezed cards keep only their thumbnail and name.
    compact: bool,
    /// Something is ticked, so every card shows its box.
    selecting: bool,
}

/// Whether two paths name one file, however each was written.
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
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
                canvas: Canvas::default(),
                titles: BTreeMap::new(),
                names: BTreeMap::new(),
                library: Vec::new(),
            },
            history: History::default(),
            preview: Preview::new(use_sound, Arc::new(move || repaint.request_repaint())),
            picture: None,
            layer_pictures: LayerPictures::default(),
            layer_display: None,
            gpu_chips: Vec::new(),
            running_gpu: None,
            gpu_at_start: None,
            stage_state: StageState::default(),
            stage_before: None,
            background_text: None,
            selected: None,
            also_selected: Vec::new(),
            group_anchor: None,
            selected_gap: None,
            timeline_state: TimelineState::default(),
            trim_origin: None,
            notice: None,
            export_state: ExportState::Closed,
            mode: CutMode::FrameExact,
            rate_pick: None,
            keep_sound_apart: false,
            export_choices: ExportChoices::default(),
            sound_only: false,
            add_audio_requested: false,
            confirm_remove: Vec::new(),
            confirm_delete_recording: None,
            trash_file: move_to_recycle_bin,
            trashing: Vec::new(),
            project_name: project::DEFAULT_NAME.to_owned(),
            project_path: None,
            baseline: ProjectFile::default(),
            leave_for: None,
            missing_videos: None,
            renaming: None,
            media_rename: None,
            window_title: String::new(),
            closing: false,
            settings: Settings::default(),
            settings_open: false,
            settings_tab: SettingsTab::General,
            settings_microphones: None,
            waiting_for_key: None,
            key_problem: None,
            app_folder: None,
            recovery: None,
            seek_bar_held: false,
            resume_after_scrub: false,
            last_autosaved: None,
            last_autosave_at: Instant::now(),
            unsaved_since: None,
            auto_save_write: None,
            auto_save_failed: None,
            auto_save_held: false,
            recovery_offers: Vec::new(),
            cut_short: Vec::new(),
            record_bar: None,
            stage_rect: egui::Rect::NOTHING,
            open_microphone: recording::open_real_microphone,
            list_microphones: bareclip_audio::microphones,
            screen_bar: None,
            open_screen: screen_recording::open_real_screen,
            open_pc_sound: screen_recording::open_real_pc_sound,
            open_app_sound: screen_recording::open_real_app_sound,
            list_sources: screen_recording::list_real_sources,
            camera_bar: None,
            open_camera: camera_recording::open_real_camera,
            list_cameras: camera_recording::list_real_cameras,
            pick_folder: recording::pick_real_folder,
            relaunch: relaunch_bareclip,
            restart_asked: false,
            waiting_edits: Vec::new(),
            selected_media: Vec::new(),
            media_panel_open: true,
            left_panel: LeftPanel::default(),
            next_title: 0,
            title_before: None,
            title_resize: None,
            title_cards: HashMap::new(),
            library_view: library::LibraryView::default(),
            library_thumbnails: HashMap::new(),
            library_hover: None,
            selected_join: None,
            picture_menu: None,
            transition_search: String::new(),
            filter_search: String::new(),
            effect_search: String::new(),
            effect_settings: None,
            look_pictures: LookPictures::default(),
            right_panel: None,
            card_hover: None,
            all_text_styles: false,
            title_frames: crate::title_frames::TitleFrames::default(),
            pick_logo_for: None,
            title_colour_text: None,
            style_whole_row: true,
            focus_title_text: false,
            pick_captions_requested: false,
            save_captions_requested: false,
            captions_focus: None,
            font_list: None,
            font_search: String::new(),
            font_faces: HashMap::new(),
            fonts_reported: Vec::new(),
            clipboard: Vec::new(),
            duration_edit: None,
            media_sort: MediaSort::Name,
            media_sort_descending: false,
            media_filter: MediaFilter::All,
            timeline_height: TIMELINE_HEIGHT,
            timeline_open: true,
            logo: None,
            strip_logo: None,
            loaded_fallbacks: Vec::new(),
            fit: Fit::default(),
            floated: false,
            short_timeline_height: None,
            row_above: None,
            left_rail_rect: egui::Rect::NOTHING,
            right_rail_rect: egui::Rect::NOTHING,
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
        self.cut_short = storage::cut_short_recordings(&folder);
        match Recovery::start(&folder) {
            Ok(recovery) => self.recovery = Some(recovery),
            Err(error) => {
                self.notice = Some(format!("Your work can't be autosaved. {error}"));
            }
        }
        self.app_folder = Some(folder);
    }

    /// Where the window was when it last closed.
    pub fn saved_window(&self) -> Option<storage::WindowPlace> {
        self.settings.window
    }

    /// Keeps where the window is as it closes, for the next run.
    pub fn remember_window(&mut self, place: storage::WindowPlace) {
        if self.settings.window != Some(place) {
            self.settings.window = Some(place);
            self.save_settings();
        }
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
        let mut file = ProjectFile::capture(&self.project_name, media, &self.project.tracks);
        file.canvas = self.project.canvas;
        for saved in &mut file.media {
            saved.name = self.project.names.get(&MediaId(saved.id)).cloned();
            saved.recorded = self
                .entry(MediaId(saved.id))
                .is_some_and(|entry| entry.recorded);
        }
        self.write_titles_to(&mut file);
        file.library = self.project.library.clone();
        file
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
            LeaveFor::Restart => match (self.relaunch)(self.project_path.as_deref()) {
                Ok(()) => self.carry_out(ctx, LeaveFor::CloseWindow),
                Err(error) => {
                    self.notice = Some(format!("{} {error}", text::RESTART_FAILED));
                }
            },
        }
    }

    /// Restart now in Settings: finishes any recording, asks about unsaved changes like closing
    /// does, then opens BareClip again on the same project and closes this window.
    fn restart_for_new_chip(&mut self, ctx: &egui::Context) {
        self.settings_open = false;
        self.settings_microphones = None;
        self.finish_recordings(ctx);
        self.flush_auto_save();
        if self.has_unsaved_changes() {
            self.leave_for = Some(LeaveFor::Restart);
        } else {
            self.carry_out(ctx, LeaveFor::Restart);
        }
    }

    /// Finishes and adds any recording under way, so it's in the project before leaving it.
    fn finish_recordings(&mut self, ctx: &egui::Context) {
        if self
            .record_bar
            .as_ref()
            .is_some_and(recording::RecordBar::is_recording)
        {
            self.stop_recording(ctx);
        }
        if self
            .screen_bar
            .as_ref()
            .is_some_and(screen_recording::ScreenBar::is_recording)
        {
            self.stop_screen_recording(ctx);
        }
        if self
            .camera_bar
            .as_ref()
            .is_some_and(camera_recording::CameraBar::is_recording)
        {
            self.stop_camera_recording(ctx);
        }
    }

    /// Clears everything down to an empty project.
    fn start_new_project(&mut self) {
        self.wait_for_auto_save();
        self.preview.pause(self.project.tracks.played_videos());
        self.media.clear();
        self.project = Snapshot {
            tracks: Tracks::default(),
            canvas: Canvas::default(),
            titles: BTreeMap::new(),
            names: BTreeMap::new(),
            library: Vec::new(),
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
        self.media_rename = None;
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
            .set_file_name(format!(
                "{}.{}",
                text::safe_file_name(&self.project_name),
                project::EXTENSION
            ))
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
                Ok(mut entry) => {
                    entry.recorded = media.recorded;
                    new_ids.insert(media.id, entry.id);
                    durations.insert(media.id, entry.info.duration);
                    if let Some(name) = &media.name {
                        self.project.names.insert(entry.id, name.clone());
                    }
                    self.media.push(entry);
                }
                Err(error) => {
                    unreadable = true;
                    let name = media.path.display();
                    self.notice = Some(format!("{name} could not be opened. {error}"));
                }
            }
        }
        self.sync_media_names();
        self.restore_titles(&file.titles);
        self.project.library = file.library.iter().cloned().map(Art::checked).collect();
        for title in self.project.titles.keys() {
            new_ids.insert(title.0, *title);
            durations.insert(title.0, STILL_LENGTH);
        }
        let fitted = |rows: &[Vec<project::SavedClip>]| -> Vec<Vec<project::SavedClip>> {
            rows.iter()
                .map(|track| {
                    track
                        .iter()
                        .filter_map(|clip| clip.fitted_to(*durations.get(&clip.media)?))
                        .collect()
                })
                .collect()
        };
        self.project.tracks = Tracks::restore_with_sound(
            &fitted(&file.tracks),
            &fitted(&file.audio_tracks),
            &file.muted_rows(),
            |id| new_ids.get(&id).copied(),
        );
        self.project.canvas = file.canvas.checked();
        if file.predates_canvas() {
            // These were sized by their largest video, and shouldn't change just by being opened.
            self.project.canvas.original = self.project.tracks.canvas(&|id| self.video_size(id));
        }
        self.settle_canvas();
        self.sync_titles();
        self.project_name = file.name;
        self.project_path = path;
        self.media_panel_open = true;
        self.left_panel = LeftPanel::Media;
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
        // A recording is finished and added first, so it is in the project the save question
        // asks about.
        self.finish_recordings(ctx);
        self.flush_auto_save();
        if self.has_unsaved_changes() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.leave_for = Some(LeaveFor::CloseWindow);
        } else if let Some(recovery) = &mut self.recovery {
            recovery.clear();
        }
    }

    fn position(&self) -> Ticks {
        self.preview.position(self.project.tracks.played_videos())
    }

    fn entry(&self, id: MediaId) -> Option<&MediaEntry> {
        self.media.iter().find(|entry| entry.id == id)
    }

    fn visible_media(&self) -> impl Iterator<Item = &MediaEntry> {
        self.media.iter().filter(|entry| !entry.id.is_title())
    }

    /// The videos the panel lists, filtered and in order.
    fn listed_media(&self) -> Vec<&MediaEntry> {
        let tracks = &self.project.tracks;
        let used = |entry: &MediaEntry| tracks.clips_using(entry.id) > 0;
        let mut listed: Vec<&MediaEntry> = self
            .visible_media()
            .filter(|entry| self.media_filter.keeps(used(entry), &entry.info))
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
        preview.set_project_length(project.tracks.duration());
        preview.seek(
            project.tracks.played_videos(),
            &|id| media_info(media, id),
            time,
        );
        if self.project.tracks.is_empty() {
            self.picture = None;
        }
    }

    /// Playback waits while the playhead is moved by hand, so the picture can follow the
    /// pointer, and carries on from wherever it lands.
    fn hold_playback_while_scrubbing(&mut self) {
        let scrubbing = self.timeline_state.drag == TimelineDrag::Scrub || self.seek_bar_held;
        if scrubbing && self.preview.is_playing() {
            self.preview.pause(self.project.tracks.played_videos());
            self.resume_after_scrub = true;
        } else if !scrubbing && std::mem::take(&mut self.resume_after_scrub) {
            let at_the_end = self.position() >= self.project.tracks.duration();
            if !at_the_end && !self.preview.is_playing() {
                self.toggle_playback();
            }
        }
    }

    fn toggle_playback(&mut self) {
        let Self {
            preview,
            project,
            media,
            ..
        } = self;
        preview.toggle(project.tracks.played_videos(), &|id| media_info(media, id));
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
            if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(text::CAPTIONS_EXTENSION))
            {
                self.import_captions(path);
                continue;
            }
            match self.load(ctx, path) {
                Ok(entry) => {
                    self.media.push(entry);
                    self.media_panel_open = true;
                    self.left_panel = LeftPanel::Media;
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

    /// Files chosen with "Edit with BareClip" in Explorer, which go on the end of the
    /// timeline. They wait while a dialog is still deciding what the project is, so restoring
    /// lost work or opening another project can't throw them away.
    pub fn edit_files(&mut self, paths: &[PathBuf]) {
        self.waiting_edits.extend(paths.iter().cloned());
    }

    fn take_waiting_edits(&mut self, ctx: &egui::Context) {
        let deciding = self.leave_for.is_some()
            || self.record_bar.is_some()
            || self.screen_bar.is_some()
            || self.camera_bar.is_some()
            || self.missing_videos.is_some()
            || !self.recovery_offers.is_empty()
            || !matches!(self.export_state, ExportState::Closed);
        if self.waiting_edits.is_empty() || deciding {
            return;
        }
        let mut chosen = Vec::new();
        for path in std::mem::take(&mut self.waiting_edits) {
            let is_media = path.extension().is_some_and(|extension| {
                extension != project::EXTENSION
                    && !extension.eq_ignore_ascii_case(text::CAPTIONS_EXTENSION)
            });
            if !is_media {
                self.import(ctx, &[path]);
                continue;
            }
            // A video already in the panel is used again rather than listed twice.
            if let Some(entry) = self
                .visible_media()
                .find(|entry| same_file(&entry.info.path, &path))
            {
                chosen.push(entry.id);
                continue;
            }
            match self.load(ctx, &path) {
                Ok(entry) => {
                    chosen.push(entry.id);
                    self.media.push(entry);
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
        if chosen.is_empty() {
            return;
        }
        self.media_panel_open = true;
        self.left_panel = LeftPanel::Media;
        self.add_to_end(chosen);
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

        let progress = Arc::new(AtomicU32::new(0));
        let cancel_scan = Arc::new(AtomicBool::new(false));
        let sound_only = info.is_sound_only();
        let is_still = info.is_still();
        // Sound has no frames to find or pictures to show.
        let scan = (!sound_only).then(|| {
            let (sender, scan) = mpsc::channel();
            let scan_path = path.to_owned();
            if is_still {
                let repaint = ctx.clone();
                thread::spawn(move || {
                    let _ = sender.send(ScanMessage::Still(read_still(&scan_path)));
                    repaint.request_repaint();
                });
                return scan;
            }
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
            scan
        });

        let read_sound = self.settings.reads_sound_shape(sound_only);
        let strip_reader = (!is_still && (!sound_only || read_sound))
            .then(|| start_strip_reader(ctx, &info, true, read_sound));
        let id = MediaId(self.next_media);
        self.next_media += 1;
        Ok(MediaEntry {
            id,
            info,
            file_name: name.clone(),
            name,
            created,
            recorded: false,
            index: None,
            scan,
            cancel_scan,
            progress,
            thumbnail: None,
            strip_pictures: Vec::new(),
            strip_pictures_done: false,
            still: None,
            sound_shape: None,
            strip_reader,
            title_drawn: None,
        })
    }

    fn full_range(&self, id: MediaId) -> Option<TimeRange> {
        let entry = self.entry(id)?;
        if entry.info.is_still() {
            return Some(TimeRange::new_still());
        }
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
        self.selected = Some(self.push_clip(id, range));
        self.settle_canvas();
        self.after_clip_added();
    }

    /// Adds a clip to the end, on the kind of row it goes on.
    fn push_clip(&mut self, id: MediaId, range: TimeRange) -> ClipId {
        if self.is_sound_only(id) {
            self.project.tracks.push_sound(id, range)
        } else {
            self.project.tracks.push(id, range)
        }
    }

    /// Adds the ticked videos to the end of the timeline in the order they were ticked,
    /// as one step to undo.
    fn add_selected_to_timeline(&mut self) {
        let chosen = std::mem::take(&mut self.selected_media);
        self.add_to_end(chosen);
    }

    /// Adds whole videos to the end of the timeline in this order, as one step to undo.
    fn add_to_end(&mut self, chosen: Vec<MediaId>) {
        let ranges: Vec<(MediaId, TimeRange)> = chosen
            .into_iter()
            .filter_map(|id| Some((id, self.full_range(id)?)))
            .collect();
        if ranges.is_empty() {
            return;
        }
        self.record();
        for (id, range) in ranges {
            self.selected = Some(self.push_clip(id, range));
            self.settle_canvas();
            self.settle_canvas();
        }
        self.after_clip_added();
    }

    /// Ticks a video, or unticks it if it already is.
    /// Makes every video's name on screen what the project says it is.
    fn sync_media_names(&mut self) {
        for entry in &mut self.media {
            let name = self
                .project
                .names
                .get(&entry.id)
                .unwrap_or(&entry.file_name);
            if entry.name != *name {
                entry.name.clone_from(name);
            }
        }
    }

    /// Gives a video a name in this project, as one undo step. Nothing typed, or its file's own
    /// name, goes back to going by the file.
    fn rename_media(&mut self, id: MediaId, typed: &str) {
        let Some(entry) = self.media.iter().find(|entry| entry.id == id) else {
            return;
        };
        let typed = typed.trim();
        let wanted = (!typed.is_empty() && typed != entry.file_name).then(|| typed.to_owned());
        if wanted.as_ref() == self.project.names.get(&id) {
            return;
        }
        self.record();
        match wanted {
            Some(name) => self.project.names.insert(id, name),
            None => self.project.names.remove(&id),
        };
        self.sync_media_names();
    }

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
        let placed = if self.is_sound_only(id) {
            self.project.tracks.place_sound(id, range, time, target)
        } else {
            self.project.tracks.place(id, range, time, target)
        };
        self.selected = Some(placed);
        self.settle_canvas();
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

    /// Takes videos out of the project for good, with their clips. Undo and redo never bring them
    /// back, as if they had never been added, so their files are closed and free straight away.
    fn remove_many(&mut self, ids: &[MediaId]) {
        // The size each video left would give the project, the way `settle_canvas` sees it.
        let sizes: HashMap<MediaId, (u32, u32)> = self
            .media
            .iter()
            .filter(|entry| !ids.contains(&entry.id) && !entry.info.is_still())
            .map(|entry| (entry.id, (entry.info.video.width, entry.info.video.height)))
            .collect();
        let size_of = |id: MediaId| sizes.get(&id).copied();
        let forget = |snapshot: &mut Snapshot| {
            for id in ids {
                snapshot.tracks.remove_media(*id);
                snapshot.names.remove(id);
            }
            snapshot.canvas.settle(&snapshot.tracks, &size_of);
        };
        forget(&mut self.project);
        for id in ids {
            self.preview.close(*id);
        }
        // Dropping a video stops whatever is still reading it in the background.
        self.media.retain(|entry| !ids.contains(&entry.id));
        self.selected_media.retain(|id| !ids.contains(id));
        if self.clipboard.iter().any(|copied| copied.is_from_any(ids)) {
            self.clipboard.clear();
        }
        let tracks = &self.project.tracks;
        self.layer_pictures
            .keep_clips(|clip| tracks.clip(clip).is_some());
        self.after_timeline_change();
        self.history.rewrite(&self.project, forget);
    }

    /// Tidies up after clips changed, so nothing points at a clip that has gone.
    fn after_timeline_change(&mut self) {
        self.tidy_selection();
        self.settle_canvas();
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
        self.preview.pause(self.project.tracks.played_videos());
        let split_at = self.snap_to_frame(clip.media, clip.source_at(position));
        let before = self.project.clone();
        if let Some(second) = self.project.tracks.split(clip.id, split_at) {
            self.history.record(before);
            self.give_own_title(second);
            self.selected = Some(second);
            self.notice = None;
        } else {
            self.notice = Some("Move the playhead inside a clip to split it there.".to_owned());
        }
    }

    /// Every selected clip: the main one first, then the rest.
    fn selected_clips(&self) -> Vec<ClipId> {
        self.selected
            .into_iter()
            .chain(self.also_selected.iter().copied())
            .collect()
    }

    /// Whether more than one clip is selected.
    fn several_selected(&self) -> bool {
        self.selected.is_some() && !self.also_selected.is_empty()
    }

    /// The clips an action on `id` covers: every selected clip if it is one of them, otherwise
    /// just it.
    fn group_with(&self, id: ClipId) -> Vec<ClipId> {
        let selected = self.selected_clips();
        if selected.contains(&id) {
            selected
        } else {
            vec![id]
        }
    }

    fn select_only(&mut self, id: Option<ClipId>) {
        self.selected = id;
        self.also_selected.clear();
        self.group_anchor = id;
    }

    /// Selects several clips at once, the first as the main one.
    fn select_group(&mut self, ids: &[ClipId]) {
        self.selected = ids.first().copied();
        self.also_selected = ids.iter().skip(1).copied().collect();
        self.group_anchor = self.selected;
    }

    /// Changes what is selected after a click on the timeline: Ctrl adds or takes away one clip,
    /// Shift takes everything from the clip clicked last, and a drag or right-click keeps the
    /// selection if the clip is part of it.
    fn select_on_timeline(&mut self, clicked: Option<ClipId>, how: timeline::SelectHow) {
        use timeline::SelectHow;
        self.tidy_selection();
        let Some(id) = clicked else {
            self.select_only(None);
            return;
        };
        let already = self.selected_clips().contains(&id);
        match how {
            SelectHow::Only => self.select_only(Some(id)),
            SelectHow::Keep if already => {}
            SelectHow::Keep => self.select_only(Some(id)),
            SelectHow::Toggle => {
                let mut group = self.selected_clips();
                if already {
                    group.retain(|other| *other != id);
                } else {
                    group.push(id);
                }
                self.select_group(&group);
            }
            SelectHow::Range => match self.selected {
                Some(anchor) => {
                    let mut group = vec![anchor];
                    group.extend(
                        self.clips_between(anchor, id)
                            .into_iter()
                            .filter(|other| *other != anchor),
                    );
                    self.select_group(&group);
                }
                None => self.select_only(Some(id)),
            },
        }
    }

    /// Every clip on the rows from one clip's to the other's that starts between their starts,
    /// both included, in order of row and time.
    fn clips_between(&self, from: ClipId, to: ClipId) -> Vec<ClipId> {
        let tracks = &self.project.tracks;
        let (Some(first), Some(last), Some(first_row), Some(last_row)) = (
            tracks.clip(from),
            tracks.clip(to),
            tracks.track_of(from),
            tracks.track_of(to),
        ) else {
            return vec![to];
        };
        let (earliest, latest) = (first.start.min(last.start), first.start.max(last.start));
        let rows = first_row.min(last_row)..=first_row.max(last_row);
        tracks.tracks()[rows]
            .iter()
            .flat_map(|track| track.clips())
            .filter(|clip| clip.start >= earliest && clip.start <= latest)
            .map(|clip| clip.id)
            .collect()
    }

    /// Ctrl+A: every clip on the selected clip's row. Nothing happens with nothing selected.
    fn select_row(&mut self) {
        let Some(id) = self.selected else {
            return;
        };
        let Some(row) = self.project.tracks.track_of(id) else {
            return;
        };
        let mut group = vec![id];
        group.extend(
            self.project.tracks.tracks()[row]
                .clips()
                .iter()
                .map(|clip| clip.id)
                .filter(|other| *other != id),
        );
        self.select_group(&group);
        self.selected_gap = None;
        self.selected_join = None;
    }

    /// Lets go of clips that have gone, and of the others if the main clip was changed some other
    /// way, such as by adding a clip.
    fn tidy_selection(&mut self) {
        if self.selected != self.group_anchor {
            self.also_selected.clear();
            self.group_anchor = self.selected;
        }
        let tracks = &self.project.tracks;
        let main = self.selected;
        self.also_selected
            .retain(|id| Some(*id) != main && tracks.clip(*id).is_some());
        if self.selected.is_some_and(|id| tracks.clip(id).is_none()) {
            self.selected = None;
        }
        if self.selected.is_none() && !self.also_selected.is_empty() {
            let first = self.also_selected.remove(0);
            self.selected = Some(first);
        }
        self.group_anchor = self.selected;
    }

    /// Duplicates one clip straight after itself, or several together straight after the last.
    fn duplicate_clips(&mut self, ids: &[ClipId]) {
        match ids {
            [one] => self.duplicate_clip(*one),
            _ => self.duplicate_group(ids),
        }
    }

    /// Deletes the selected clips, leaving dead space, or closes the selected dead space.
    fn delete_selected(&mut self) {
        self.tidy_selection();
        if self.several_selected() {
            self.record();
            for id in self.selected_clips() {
                self.project.tracks.remove(id);
            }
            self.select_only(None);
            self.after_timeline_change();
        } else if let Some(selected) = self.selected {
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
            self.give_own_title(copy);
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
        self.rate_pick = None;
        self.keep_sound_apart = false;
        self.export_choices = ExportChoices::default();
        self.sound_only = false;
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
            self.sync_media_names();
            self.after_timeline_change();
        }
    }

    fn redo(&mut self) {
        let current = self.project.clone();
        if let Ok(next) = self.history.redo(current) {
            self.project = next;
            self.sync_media_names();
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
        self.preview.pause(self.project.tracks.played_videos());
        let position = self.position();
        let gap_step = Ticks::from_seconds(if forward {
            GAP_STEP_SECONDS
        } else {
            -GAP_STEP_SECONDS
        });
        let Some(location) = self.project.tracks.played_videos().locate(position) else {
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
            let at = location.clip.time_of(Ticks(shown_time.0 + gap));
            // Rounding must not land it just before the frame, which would show the last one.
            let nudge = i64::from(!location.clip.speed.is_normal());
            Ticks(at.0 + nudge)
        });
        self.seek(target);
    }

    fn poll_background(&mut self, ctx: &egui::Context) {
        let mut problem = None;
        let mut still_arrived = false;
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
                    ScanMessage::Still(Ok(still)) => {
                        let image = shrunk_still(&still);
                        entry.thumbnail = Some(ctx.load_texture(
                            format!("thumbnail-{}", entry.id.0),
                            image,
                            TextureOptions::LINEAR,
                        ));
                        // The same picture stands in all along its clips.
                        if let Some(thumbnail) = entry.thumbnail.clone() {
                            entry.strip_pictures = vec![(Ticks(0), thumbnail)];
                            entry.strip_pictures_done = true;
                        }
                        entry.still = Some(Arc::new(still));
                        still_arrived = true;
                        finished = true;
                    }
                    ScanMessage::Still(Err(error)) => {
                        problem = Some(format!("{} can't be shown. {error}", entry.name));
                        finished = true;
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
        if still_arrived {
            self.layer_pictures.picture_arrived();
        }
        self.poll_strip_readers(ctx);

        self.settle_canvas();
        let canvas = self.final_canvas();
        let layered = self.layer_display.is_some() && self.takes_layers();
        let Self {
            preview,
            project,
            media,
            picture,
            notice,
            timeline_state,
            layer_pictures,
            layer_display,
            stage_state,
            title_frames,
            selected,
            ..
        } = self;
        let lookup = |id: MediaId| media_info(media, id);
        if preview.planes() != layered {
            preview.set_planes(layered, project.tracks.played_videos(), &lookup);
            *layer_pictures = LayerPictures::default();
            if let Some(display) = layer_display {
                display.forget();
            }
        }
        let mut sharp: Vec<MediaId> = project
            .tracks
            .picture_tracks()
            .iter()
            .flat_map(Timeline::clips)
            .filter(|clip| clip.look.effects.zooms_in())
            .map(|clip| clip.media)
            .collect();
        sharp.sort_unstable();
        sharp.dedup();
        preview.set_sharp(sharp, project.tracks.played_videos(), &lookup);
        preview.set_sound(
            heard_spans(media, &project.tracks),
            project.tracks.duration(),
        );
        let frame = preview.update(project.tracks.played_videos(), &lookup);
        if let (true, Some(canvas), Some(display)) = (layered, canvas, layer_display.as_mut()) {
            let size_of = |id: MediaId| {
                media
                    .iter()
                    .find(|entry| entry.id == id)
                    .map(|entry| (entry.info.video.width, entry.info.video.height))
            };
            let now = last_moment_by(
                preview.position(project.tracks.played_videos()),
                project.tracks.duration(),
            );
            let mut stack = project.tracks.stack_at(now, canvas, &size_of);
            // The clip being cropped shows whole, with what would be cut dimmed on top.
            if let Some(clip) = stage_state
                .cropping()
                .and_then(|id| stack.iter_mut().find(|clip| clip.id == id))
                && let Some(size) = size_of(clip.media)
            {
                clip.placement = clip.placement.uncropped(size, canvas);
            }
            let moment = Moment {
                now,
                playing: preview.is_playing(),
                jumped: preview.take_jump(),
                coming: project.tracks.next_change(
                    now,
                    Ticks::from_seconds(PREPARE_AHEAD_SECONDS),
                    canvas,
                    &size_of,
                ),
            };
            let top = frame.zip(preview.last_picture_from());
            let repaint = ctx.clone();
            let notify: crate::player::Notify = Arc::new(move || repaint.request_repaint());
            if let Some(problem) = layer_pictures.update(&stack, moment, top, &lookup, &notify) {
                *notice = Some(problem);
            }
            let background = project.canvas.background;
            // A selected title rests while nothing plays, because it is being changed and may be
            // invisible at this moment of its motion.
            let held = (*selected).filter(|_| !preview.is_playing());
            if title_frames.pose_stack(&mut stack, now, canvas, &project.titles, &size_of, held) {
                layer_pictures.picture_arrived();
            }
            let title_frames = &*title_frames;
            let shown_size_of = |id: MediaId| title_frames.shown_size(id).or_else(|| size_of(id));
            let still_of = |id: MediaId| {
                title_frames.shown_picture(id).or_else(|| {
                    media
                        .iter()
                        .find(|entry| entry.id == id)
                        .and_then(|entry| entry.still.clone())
                })
            };
            if let Err(error) = display.draw(
                layer_pictures,
                &stack,
                now,
                (canvas, background),
                &shown_size_of,
                &still_of,
            ) {
                *notice = Some(error.to_string());
            }
        } else if let Some(frame) = frame {
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
            let position = preview.position(project.tracks.played_videos());
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

    /// Brings what is being read for the clips into line with the settings. Turning waveforms off
    /// forgets what nothing needs any more. Pictures carry on from where they had got to, and are
    /// always wanted.
    fn apply_strip_settings(&mut self, ctx: &egui::Context) {
        let settings = self.settings.clone();
        for entry in &mut self.media {
            entry.strip_reader = None;
            // An image is its own picture, already in memory, with nothing to read.
            if entry.info.is_still() {
                continue;
            }
            let read_sound = settings.reads_sound_shape(entry.info.is_sound_only());
            if !read_sound {
                entry.sound_shape = None;
            }
            let want_pictures = !entry.info.is_sound_only() && !entry.strip_pictures_done;
            let want_sound = read_sound && entry.sound_shape.is_none();
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

    /// A video's full size, once it has been read.
    fn video_size(&self, id: MediaId) -> Option<(u32, u32)> {
        self.entry(id)
            .map(|entry| (entry.info.video.width, entry.info.video.height))
            // A title only has a picture while it is near the playhead. Elsewhere it only needs
            // to count as something that shows, and a title never hides what is under it.
            .or_else(|| id.is_title().then_some((1, 1)))
    }

    /// The size of the finished video.
    fn final_canvas(&self) -> Option<(u32, u32)> {
        (!self.project.tracks.is_empty()).then(|| self.project.canvas.size())
    }

    /// Lets the first video set the project's size, and an empty timeline forget it.
    fn settle_canvas(&mut self) {
        let mut canvas = self.project.canvas;
        // Only videos set the size, because a photo can be far larger than any video needs.
        let size_of = |id: MediaId| {
            self.entry(id)
                .filter(|entry| !entry.info.is_still())
                .map(|entry| (entry.info.video.width, entry.info.video.height))
        };
        canvas.settle(&self.project.tracks, &size_of);
        self.project.canvas = canvas;
    }

    /// Changes the project's shape, as one step to undo. `None` is the first video's own.
    fn choose_ratio(&mut self, ratio: Option<(u32, u32)>) {
        if self.project.canvas.ratio != ratio {
            self.record();
            self.project.canvas.ratio = ratio;
        }
    }

    /// Whether this project needs several pictures drawn together, rather than one at a time.
    fn takes_layers(&self) -> bool {
        self.final_canvas().is_some_and(|canvas| {
            let size_of = |id: MediaId| self.video_size(id);
            let tracks = &self.project.tracks;
            tracks.needs_layers(canvas, &size_of)
                || (self.project.canvas.background != BLACK
                    && tracks.shows_canvas(canvas, &size_of))
        })
    }

    /// A clip of a file with no picture, which goes on a sound row.
    fn is_sound_only(&self, id: MediaId) -> bool {
        self.entry(id)
            .is_some_and(|entry| entry.info.is_sound_only())
    }

    /// The whole project as an export that draws layers.
    fn layered_stretches(&self) -> Vec<LayeredStretch> {
        let Some(canvas) = self.final_canvas() else {
            return Vec::new();
        };
        self.project
            .tracks
            .stretches(canvas, &|id| self.video_size(id))
            .into_iter()
            .map(|stretch| LayeredStretch {
                start: stretch.start,
                end: stretch.end,
                stack: stretch
                    .stack
                    .iter()
                    .filter_map(|clip| {
                        let title = self.title_of(*clip).cloned();
                        let input = if title.is_some() {
                            PathBuf::new()
                        } else {
                            self.entry(clip.media)?.info.path.clone()
                        };
                        let (start, source) = self.project.tracks.with_room_for_joins(clip);
                        let tracks = &self.project.tracks;
                        Some(LayeredClip {
                            key: clip.id.0,
                            input,
                            start,
                            source,
                            placement: clip.placement,
                            speed: clip.speed,
                            title,
                            look: clip.look,
                            join_in: tracks.join_into(clip.id).map(|(cut, transition)| {
                                bareclip_export::ExportJoin { cut, transition }
                            }),
                            join_out: clip.transition.map(|transition| {
                                bareclip_export::ExportJoin {
                                    cut: clip.end(),
                                    transition,
                                }
                            }),
                        })
                    })
                    .collect(),
            })
            .collect()
    }

    /// Each different video that shows on the timeline, in the order they first show.
    fn videos_on_timeline(&self) -> Vec<MediaInfo> {
        let mut seen = Vec::new();
        let mut videos = Vec::new();
        let Some(canvas) = self.final_canvas() else {
            return videos;
        };
        let stretches = self
            .project
            .tracks
            .stretches(canvas, &|id| self.video_size(id));
        for clip in stretches.iter().flat_map(|stretch| &stretch.stack) {
            // A title isn't a video, and has no size or frame rate for the export to go by.
            if !clip.is_title() && !seen.contains(&clip.media) {
                seen.push(clip.media);
                if let Some(entry) = self.entry(clip.media) {
                    videos.push(entry.info.clone());
                }
            }
        }
        videos
    }

    /// How the timeline would be exported, leaving aside any difference in frame rates.
    fn route_before_rates(&self, videos: &[MediaInfo]) -> Route {
        if self.takes_layers() {
            let other_speeds = self.final_canvas().is_some_and(|canvas| {
                self.project
                    .tracks
                    .shows_other_speeds(canvas, &|id| self.video_size(id))
            });
            let has_titles = self
                .project
                .tracks
                .picture_tracks()
                .iter()
                .flat_map(Timeline::clips)
                .any(|clip| clip.is_title());
            let has_transitions = self
                .project
                .tracks
                .picture_tracks()
                .iter()
                .flat_map(Timeline::clips)
                .any(|clip| clip.transition.is_some());
            let has_looks = self
                .project
                .tracks
                .picture_tracks()
                .iter()
                .flat_map(Timeline::clips)
                .any(|clip| !clip.look.is_default());
            let reason = if has_titles {
                TITLES_REASON
            } else if has_transitions {
                TRANSITIONS_REASON
            } else if has_looks {
                LOOKS_REASON
            } else if videos.iter().any(MediaInfo::is_still) {
                STILLS_REASON
            } else if other_speeds {
                SPEED_REASON
            } else {
                LAYERED_REASON
            };
            return Route::Full(reason.to_owned());
        }
        let tracks = &self.project.tracks;
        if tracks.played().is_empty() {
            return Route::Full(ONLY_SOUND_REASON.to_owned());
        }
        if tracks.sound_outlasts_pictures() {
            return Route::Full(SOUND_OUTLASTS_REASON.to_owned());
        }
        let route = route_for(videos, !tracks.picture_gaps().is_empty());
        if route != Route::Fast {
            return route;
        }
        if !self.sound_is_plain() {
            if !self.first_video_can_hold_new_sound() {
                return Route::Full(NEW_SOUND_REASON.to_owned());
            }
            // New sound follows the timeline exactly, so the picture has to be cut exactly too.
            if videos
                .iter()
                .any(|video| !can_write_gaps(&video.video.codec))
            {
                return Route::Full(EXACT_CUTS_REASON.to_owned());
            }
        }
        let misfit = self
            .final_canvas()
            .and_then(|canvas| videos.iter().find_map(|video| size_problem(canvas, video)));
        misfit.map_or(Route::Fast, Route::Full)
    }

    fn has_sound(&self, id: MediaId) -> bool {
        self.entry(id)
            .is_some_and(|entry| !entry.info.audio.is_empty())
    }

    /// Whether the sound is just what each video recorded, so it can simply be copied.
    fn sound_is_plain(&self) -> bool {
        self.project.tracks.sound_is_plain(&|id| self.has_sound(id))
    }

    /// Whether the kind of file a fast export would write can take the sound a mix makes.
    fn first_video_can_hold_new_sound(&self) -> bool {
        self.suggested_fast_output().is_none_or(|path| {
            path.extension().is_some_and(|extension| {
                let extension = extension.to_string_lossy();
                CONTAINERS_FOR_NEW_SOUND
                    .iter()
                    .any(|known| known.eq_ignore_ascii_case(&extension))
            })
        })
    }

    /// How many sound tracks each picture row's clips would keep if the sound stayed apart.
    fn own_tracks_kept_apart(&self) -> Vec<SoundLabel> {
        let tracks = &self.project.tracks;
        let picture_videos: Vec<MediaInfo> = tracks
            .picture_tracks()
            .iter()
            .flat_map(Timeline::clips)
            .filter_map(|clip| self.entry(clip.media))
            .map(|entry| entry.info.clone())
            .collect();
        sound_labels(&picture_videos)
    }

    /// Whether there is more than one track to choose between keeping apart or mixing.
    fn can_keep_sound_apart(&self) -> bool {
        let tracks = &self.project.tracks;
        !self.sound_is_plain()
            && self.own_tracks_kept_apart().len() + tracks.audio_tracks().len() > 1
    }

    /// Everything heard just as it was recorded, with each of its tracks kept apart. This goes by
    /// what can be heard rather than what is on top, or the sound of a video under an image or a
    /// silent overlay would stop while that was showing.
    fn recorded_sound_plan(&self) -> SoundPlan {
        let spans = self.project.tracks.sound_spans(&|id| self.has_sound(id));
        let mut heard: Vec<MediaInfo> = Vec::new();
        let spans = spans
            .into_iter()
            .filter_map(|span| {
                let info = &self.entry(span.media)?.info;
                if !heard.iter().any(|known| known.path == info.path) {
                    heard.push(info.clone());
                }
                Some(MixSpan {
                    path: info.path.clone(),
                    start: span.start,
                    source: span.source,
                    sound: span.sound,
                    speed: span.speed,
                    bus: Bus::ByTrack(0),
                })
            })
            .collect();
        SoundPlan {
            spans,
            tracks: sound_labels(&heard),
        }
    }

    /// What the finished video's sound comes from, and the tracks it goes on.
    fn sound_plan(&self) -> SoundPlan {
        let tracks = &self.project.tracks;
        if self.sound_is_plain() && !self.sound_only {
            return self.recorded_sound_plan();
        }
        let apart = self.keep_sound_apart && self.can_keep_sound_apart() && !self.sound_only;
        let own = self.own_tracks_kept_apart();
        let spans: Vec<MixSpan> = tracks
            .sound_spans(&|id| self.has_sound(id))
            .into_iter()
            .filter_map(|span| {
                let bus = match span.sound_row {
                    _ if !apart => Bus::All(0),
                    Some(row) => Bus::All(own.len() + row),
                    None => Bus::ByTrack(0),
                };
                Some(MixSpan {
                    path: self.entry(span.media)?.info.path.clone(),
                    start: span.start,
                    source: span.source,
                    sound: span.sound,
                    speed: span.speed,
                    bus,
                })
            })
            .collect();
        let labels = if apart {
            let rows = (1..=tracks.audio_tracks().len()).map(|row| SoundLabel {
                title: Some(format!("Audio {row}")),
                language: None,
            });
            own.iter().cloned().chain(rows).collect()
        } else if spans.is_empty() {
            Vec::new()
        } else {
            vec![SoundLabel::default()]
        };
        SoundPlan {
            spans,
            tracks: labels,
        }
    }

    /// Why joining these videos is a choice between copying and redrawing, if it is. Videos that
    /// have to be redrawn anyway have no choice to make.
    fn rate_choice(&self) -> Option<String> {
        if self.export_choices.need_redrawing() {
            return None;
        }
        let videos = self.videos_on_timeline();
        (self.route_before_rates(&videos) == Route::Fast)
            .then(|| mixed_rates(&videos))
            .flatten()
    }

    /// Which way the choice starts, before the user picks. Copying is only for videos already
    /// uneven inside, so one whose frames haven't been read yet counts against it.
    fn rate_pick_at_first(&self) -> RatePick {
        let tables: Option<Vec<_>> = self
            .videos_on_timeline()
            .iter()
            .map(|info| {
                self.media
                    .iter()
                    .find(|entry| entry.info.path == info.path)?
                    .index
                    .clone()
            })
            .collect();
        if tables.is_some_and(|tables| copying_suits(tables.iter().map(|frames| &**frames))) {
            RatePick::Copy
        } else {
            RatePick::Redraw
        }
    }

    fn current_rate_pick(&self) -> RatePick {
        self.rate_pick.unwrap_or_else(|| self.rate_pick_at_first())
    }

    /// How the timeline would be exported as it is now.
    fn export_route(&self) -> Route {
        let route = self.route_before_rates(&self.videos_on_timeline());
        if route != Route::Fast {
            return route;
        }
        if self.export_choices.need_redrawing() {
            return Route::Full(CHOICES_REASON.to_owned());
        }
        match self.rate_choice() {
            Some(reason) if self.current_rate_pick() == RatePick::Redraw => Route::Full(reason),
            _ => Route::Fast,
        }
    }

    /// The finished video's size, with the export window's choice applied.
    fn export_size(&self) -> Option<(u32, u32)> {
        let canvas = self.final_canvas()?;
        if self.export_choices.format == VideoFormat::Gif {
            return Some(gif_size_for(canvas, self.export_choices.gif_size));
        }
        Some(match self.export_choices.size {
            ExportSize::MatchSource => canvas,
            ExportSize::Shorter(shorter) => size_with_shorter_side(canvas, shorter),
        })
    }

    /// The finished video's frame rate. A GIF has its own choice of those.
    fn export_rate(&self) -> ExportRate {
        if self.export_choices.format == VideoFormat::Gif {
            ExportRate::Fps(self.export_choices.gif_rate)
        } else {
            self.export_choices.rate
        }
    }

    /// The plan a redrawn export would follow, with the export window's choices applied.
    fn export_plan(&self) -> FullPlan {
        let (width, height) = self.export_size().unwrap_or(self.project.canvas.size());
        FullPlan::on_canvas(&self.videos_on_timeline(), (width, height))
            .with_choices(self.export_choices.quality, self.export_rate())
            .with_format(self.export_choices.format)
    }

    /// How long a GIF of this project would run: only the pictures count, because it has no sound.
    fn gif_seconds(&self) -> f64 {
        self.project.tracks.played().duration().as_seconds()
    }

    /// Which sizes the export window offers.
    fn export_sizes(&self) -> Vec<(u32, &'static str)> {
        let pictures: Vec<(u32, u32)> = self
            .videos_on_timeline()
            .iter()
            .map(|video| (video.video.width, video.video.height))
            .collect();
        sizes_on_offer(self.project.canvas.size(), &pictures)
    }

    /// The shape and size of the finished video, which the preview shows too.
    fn canvas_size(&self) -> Option<egui::Vec2> {
        self.final_canvas()
            .map(|(width, height)| vec2(width as f32, height as f32))
    }

    /// The file a fast export would start on, named after the first clip's kind of file.
    fn suggested_fast_output(&self) -> Option<PathBuf> {
        let tracks = &self.project.tracks;
        let first = tracks
            .played()
            .clips()
            .first()
            .or_else(|| tracks.tracks().iter().find_map(|row| row.clips().first()))
            .and_then(|clip| self.entry(clip.media))?;
        Some(text::default_output_path(
            &self.project_name,
            &first.info.path,
        ))
    }

    /// The file the export dialog starts on, or nothing if nothing is on the timeline.
    fn suggested_output(&self) -> Option<PathBuf> {
        let suggested = self.suggested_fast_output()?;
        Some(if self.sound_only {
            suggested.with_extension("m4a")
        } else if self.export_choices.format == VideoFormat::Gif {
            suggested.with_extension("gif")
        } else if matches!(self.export_route(), Route::Full(_)) {
            full_output_path(&suggested)
        } else {
            suggested
        })
    }

    fn start_export(&mut self, ctx: &egui::Context) {
        let Some(suggested) = self.suggested_output() else {
            return;
        };
        let title = if self.sound_only {
            "Export sound as"
        } else {
            "Export video as"
        };
        let mut dialog = rfd::FileDialog::new().set_title(title);
        if let Some(folder) = suggested.parent() {
            dialog = dialog.set_directory(folder);
        }
        if let Some(name) = suggested.file_name() {
            dialog = dialog.set_file_name(name.to_string_lossy());
        }
        if let Some(extension) = suggested.extension() {
            let extension = extension.to_string_lossy().into_owned();
            let kind = if self.sound_only { "sound" } else { "video" };
            dialog =
                dialog.add_filter(format!("{} {kind}", extension.to_uppercase()), &[extension]);
        }
        if let Some(output) = dialog.save_file() {
            self.start_export_to(ctx, output);
        }
    }

    fn start_export_to(&mut self, ctx: &egui::Context, output: PathBuf) {
        // Only copying needs to know where each frame of a video is.
        let parts = self.export_parts();
        let redrawn = matches!(self.export_route(), Route::Full(_));
        if parts.is_none() && !self.sound_only && !redrawn {
            return;
        }
        let parts = parts.unwrap_or_default();
        // Worked out here because the project may change while the export runs.
        let sound = self.sound_plan();
        let copy_sound = self.sound_is_plain();
        let sound_only = self.sound_only;
        let layered = matches!(self.export_route(), Route::Full(_)).then(|| LayeredRequest {
            output: output.clone(),
            stretches: self.layered_stretches(),
            sound: sound.clone(),
            canvas: self.export_size(),
            background: self.project.canvas.background,
            quality: self.export_choices.quality,
            rate: self.export_rate(),
        });
        let format = self.export_choices.format;
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
            let progress = |fraction: f32| {
                worker_progress.store(fraction.to_bits(), Ordering::Relaxed);
            };
            // The route has already been chosen, including any pick between Copy and Redraw.
            let outcome = match &layered {
                _ if sound_only => {
                    export_sound_only(&sound, &request.output, &worker_cancel, &progress)
                        .map(ProjectReport::Sound)
                }
                Some(layered) => export_layered_as(layered, format, &worker_cancel, &progress)
                    .map(ProjectReport::Full),
                None if copy_sound => {
                    export(&request, &worker_cancel, &progress).map(ProjectReport::Fast)
                }
                None => export_with_new_sound(&request, &sound, &worker_cancel, &progress)
                    .map(ProjectReport::Fast),
            };
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

    /// Asks for sound to add to the project, as the lane under the rows offers.
    fn pick_audio(&mut self, ctx: &egui::Context) {
        let before: Vec<MediaId> = self.media.iter().map(|entry| entry.id).collect();
        let picked = rfd::FileDialog::new()
            .set_title("Add audio")
            .add_filter("Audio files", &text::AUDIO_EXTENSIONS)
            .add_filter("All files", &["*"])
            .pick_files();
        let Some(paths) = picked else {
            return;
        };
        self.import(ctx, &paths);
        let added: Vec<MediaId> = self
            .media
            .iter()
            .map(|entry| entry.id)
            .filter(|id| !before.contains(id))
            .collect();
        for id in added {
            self.add_to_timeline(id);
        }
    }

    fn pick_files(&mut self, ctx: &egui::Context) {
        let picked = rfd::FileDialog::new()
            .set_title("Add media")
            .add_filter("Video, audio and image files", &text::MEDIA_EXTENSIONS)
            .add_filter("All files", &["*"])
            .pick_files();
        if let Some(paths) = picked {
            self.import(ctx, &paths);
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let blocked = !matches!(self.export_state, ExportState::Closed)
            || !self.confirm_remove.is_empty()
            || self.confirm_delete_recording.is_some()
            || self.leave_for.is_some()
            || self.missing_videos.is_some()
            || !self.recovery_offers.is_empty()
            || self.settings_open
            || self.duration_edit.is_some()
            || self.record_bar.as_ref().is_some_and(recording::RecordBar::is_recording)
            || self.screen_bar.as_ref().is_some_and(screen_recording::ScreenBar::is_recording)
            || self.camera_bar.as_ref().is_some_and(camera_recording::CameraBar::is_recording)
            || !self.cut_short.is_empty()
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
        self.tidy_selection();
        if consume(Modifiers::COMMAND, Key::A) {
            self.select_row();
        }
        if consume(Modifiers::COMMAND, Key::D) && self.selected.is_some() {
            self.duplicate_clips(&self.selected_clips());
        }
        if consume(Modifiers::COMMAND, Key::C) && self.selected.is_some() {
            self.copy_clips(&self.selected_clips());
        }
        if consume(Modifiers::COMMAND, Key::V) {
            self.paste_clip();
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
        if (consume(Modifiers::NONE, Key::Delete) || consume(Modifiers::NONE, Key::Backspace))
            && !self.delete_selected_join()
        {
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

    /// Gives the editor a GPU to draw layers on.
    pub fn use_gpu(&mut self, display: LayerDisplay) {
        self.layer_display = Some(display);
    }

    /// Fills in the graphics chip setting. Call this after the settings have been read.
    pub fn list_gpus(&mut self, running: GpuEntry, mut chips: Vec<GpuEntry>) {
        let mut seen = Vec::new();
        chips.retain(|chip| {
            let fresh = !seen.contains(&chip.id);
            seen.push(chip.id);
            fresh
        });
        self.gpu_chips = chips;
        self.running_gpu = Some(running);
        self.gpu_at_start = self.settings.gpu;
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if std::mem::take(&mut self.add_audio_requested) {
            self.pick_audio(&ctx);
        }
        if std::mem::take(&mut self.pick_captions_requested) {
            self.pick_captions();
        }
        if std::mem::take(&mut self.save_captions_requested) {
            self.save_captions();
        }
        if let Some(title) = self.pick_logo_for.take() {
            self.pick_logo(title);
        }
        let palette = theme::palette(&ctx);
        // The background shows through the gaps between the panels.
        ui.painter().rect_filled(ui.max_rect(), 0.0, palette.bg_app);

        if ctx.input(|input| input.viewport().close_requested()) {
            self.handle_close_request(&ctx);
        }
        self.sync_titles();
        self.settle_selected_join();
        self.poll_background(&ctx);
        self.handle_shortcuts(&ctx);
        self.take_dropped_files(&ctx);
        self.take_waiting_edits(&ctx);
        self.autosave(&ctx);
        self.update_window_title(&ctx);
        self.fit = Fit::of(ui.max_rect().size());
        // Going narrow closes the panels once, so the video isn't suddenly covered.
        if self.fit.floating_panels() && !self.floated {
            self.media_panel_open = false;
            self.right_panel = None;
        }
        self.floated = self.fit.floating_panels();

        self.title_strip(ui, palette);
        self.rail(ui, palette);
        self.media_panel(ui, palette);
        // The rail comes first so it sits at the window's edge, outside the panel it opens.
        self.clip_rail(ui, palette);
        self.show_right_panel(ui, palette);
        // The video and the timeline share one card, from here down to the stage.
        self.card_top_row(ui, palette);
        self.seek_bar_held = false;
        self.timeline_panel(ui, palette);
        self.stage(ui, palette);
        self.hold_playback_while_scrubbing();
        self.settle_title_edit(&ctx);
        self.remove_dialog(&ctx, palette);
        self.delete_recording_dialog(&ctx, palette);
        self.check_trashing();
        self.edit_duration_dialog(&ctx, palette);
        self.export_dialog(&ctx, palette);
        self.settings_dialog(&ctx, palette);
        self.missing_videos_dialog(&ctx, palette);
        self.recovery_dialog(&ctx, palette);
        self.record_bar_dialog(&ctx, palette);
        self.screen_bar_dialog(&ctx, palette);
        self.camera_bar_dialog(&ctx, palette);
        self.cut_short_dialog(&ctx, palette);
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
            .exact_size(self.fit.title_height())
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
                    if !self.fit.tiny {
                        ui.label(
                            RichText::new("BareClip")
                                .font(fonts::strong(components::NAME_BAR_TEXT))
                                .color(palette.accent),
                        );
                    }
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
        let width = if self.fit.tiny {
            TINY_PROJECT_NAME_WIDTH
        } else {
            PROJECT_NAME_WIDTH
        };
        if let Some(mut typed) = self.renaming.take() {
            let edit = ui.add(
                egui::TextEdit::singleline(&mut typed)
                    .id(egui::Id::unique("project_name_edit"))
                    .font(FontId::new(
                        components::NAME_BAR_TEXT,
                        FontFamily::Proportional,
                    ))
                    .margin(Margin::symmetric(NAME_BAR_PADDING as i8, 0))
                    .min_size(vec2(width, components::NAME_BAR_HEIGHT))
                    .align(egui::Align2::LEFT_CENTER)
                    .desired_width(width - 2.0 * NAME_BAR_PADDING),
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
            let name = NameBar::new(palette, &label, width);
            if ui
                .add(name)
                .on_hover_text("Rename this project and its exported video")
                .clicked()
            {
                self.renaming = Some(self.project_name.clone());
            }
        }
        let menu = ui
            .add(
                IconButton::new(palette, Kind::Ghost, icons::CHEVRON_DOWN, "Project menu")
                    .size(components::NAME_BAR_HEIGHT, 16.0),
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
        if self.settings_tab == SettingsTab::Recording && self.settings_microphones.is_none() {
            self.settings_microphones = Some((self.list_microphones)());
        }
        if self.settings_tab != SettingsTab::Keybinds {
            self.waiting_for_key = None;
        }
        // Keys pressed while waiting for a recording key go to it, Escape backing out, before
        // anything else sees them, so Escape doesn't close Settings.
        changed |= self.take_recording_keys(ctx);
        let modal = egui::Modal::new(egui::Id::unique("settings_dialog"))
            .frame(dialog_frame(ctx))
            .show(ctx, |ui| {
                let room = dialog_room(ctx);
                dialog_scroll(ui, |ui| {
                    ui.set_width(SETTINGS_WIDTH.min(room.x));
                    components::dialog_title(ui, &icons::SETTINGS, palette.accent, "Settings");
                    ui.add_space(SETTINGS_ROW_GAP);
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.allocate_ui_with_layout(
                            vec2(SETTINGS_TABS_WIDTH, SETTINGS_PAGE_HEIGHT),
                            Layout::top_down(Align::Min),
                            |ui| {
                                ui.set_width(SETTINGS_TABS_WIDTH);
                                ui.spacing_mut().item_spacing.y = 2.0;
                                for (tab, icon, label, name) in SettingsTab::ALL {
                                    let chosen = self.settings_tab == tab;
                                    let side_tab = components::SideTab::new(
                                        palette,
                                        icon,
                                        (label, name),
                                        chosen,
                                    );
                                    if ui.add(side_tab).clicked() {
                                        self.settings_tab = tab;
                                    }
                                }
                            },
                        );
                        // A line down between the tabs and the page, so they read as apart.
                        ui.add_space(SETTINGS_TABS_GAP);
                        let (line, _) = ui.allocate_exact_size(
                            vec2(1.0, SETTINGS_PAGE_HEIGHT),
                            egui::Sense::hover(),
                        );
                        ui.painter().vline(
                            line.center().x,
                            line.y_range(),
                            egui::Stroke::new(1.0, palette.border),
                        );
                        ui.add_space(SETTINGS_TABS_GAP);
                        let page_width = ui.available_width();
                        ui.allocate_ui_with_layout(
                            vec2(page_width, SETTINGS_PAGE_HEIGHT),
                            Layout::top_down(Align::Min),
                            |ui| {
                                ui.set_width(page_width);
                                ui.set_min_height(SETTINGS_PAGE_HEIGHT);
                                match self.settings_tab {
                                    SettingsTab::General => {
                                        let (page_changed, page_strip) =
                                            self.general_settings(ui, palette);
                                        changed |= page_changed;
                                        strip_changed |= page_strip;
                                    }
                                    SettingsTab::Recording => {
                                        changed |= self.recording_settings(ui, palette);
                                    }
                                    SettingsTab::Keybinds => self.keybind_settings(ui, palette),
                                }
                            },
                        );
                    });
                    ui.add_space(12.0);
                    // The version sits at the bottom left whichever tab is open, level with Done.
                    components::dialog_footer(ui, palette, &text::version_line(), |ui| {
                        close = ui.add(dialog_button(palette, "Done")).clicked();
                    });
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
            self.settings_microphones = None;
            self.waiting_for_key = None;
            self.key_problem = None;
        }
        if std::mem::take(&mut self.restart_asked) {
            self.restart_for_new_chip(ctx);
        }
    }

    /// Settings' General tab: the theme, the graphics chip, waveforms and exports. Returns
    /// whether anything changed, and whether it was the waveforms.
    fn general_settings(&mut self, ui: &mut egui::Ui, palette: &Palette) -> (bool, bool) {
        let mut changed = false;
        let mut strip_changed = false;
        components::settings_group(ui, palette, "Appearance", |ui| {
            components::settings_row(
                ui,
                palette,
                ("Theme", None),
                theme::THEME_PICKER_WIDTH,
                |ui| {
                    theme::theme_picker(ui, palette);
                },
            );
        });
        let theme = ui.ctx().options(|options| options.theme_preference).into();
        if theme != self.settings.theme {
            self.settings.theme = theme;
            changed = true;
        }
        ui.add_space(SETTINGS_ROW_GAP);
        if !self.gpu_chips.is_empty() {
            changed |= self.gpu_setting(ui, palette);
            ui.add_space(SETTINGS_ROW_GAP);
        }
        components::settings_group(ui, palette, "Waveforms", |ui| {
            let on_clips = self.settings.waveforms_on_clips;
            let row = SwitchRow::new(palette, "Waveforms on clips", on_clips);
            if ui.add(row).clicked() {
                self.settings.waveforms_on_clips = !on_clips;
                strip_changed = true;
            }
            components::settings_divider(ui, palette);
            let on_audio = self.settings.waveforms_on_audio;
            let row = SwitchRow::new(palette, "Waveforms on audio", on_audio);
            if ui.add(row).clicked() {
                self.settings.waveforms_on_audio = !on_audio;
                strip_changed = true;
            }
        });
        ui.add_space(SETTINGS_ROW_GAP);
        changed |= self.export_settings(ui, palette);
        (changed, strip_changed)
    }

    /// The export choice on Settings' General tab. Returns whether it changed.
    fn export_settings(&mut self, ui: &mut egui::Ui, palette: &Palette) -> bool {
        let mut changed = false;
        components::settings_group(ui, palette, "Export", |ui| {
            components::settings_row(
                ui,
                palette,
                ("Export cuts", Some("Where the export window starts")),
                SETTINGS_CHOICE_WIDTH * 2.0 + CONTROL_SPACING,
                |ui| {
                    ui.spacing_mut().item_spacing.x = CONTROL_SPACING;
                    for (cuts, name) in [
                        (ExportCuts::FrameExact, "Frame-exact"),
                        (ExportCuts::Fastest, "Fastest"),
                    ] {
                        let chosen = self.settings.export_cuts == cuts;
                        let button = ActionButton::new(palette, Kind::Ghost, name)
                            .small()
                            .width(SETTINGS_CHOICE_WIDTH)
                            .selected(chosen);
                        if ui.add(button).clicked() && !chosen {
                            self.settings.export_cuts = cuts;
                            self.mode = cuts.into();
                            changed = true;
                        }
                    }
                },
            );
        });
        changed
    }

    /// The graphics chip choice in Settings. Returns whether it changed.
    fn gpu_setting(&mut self, ui: &mut egui::Ui, palette: &Palette) -> bool {
        let mut picked = None;
        let chosen = self.settings.gpu;
        let name_of = |id| {
            self.gpu_chips
                .iter()
                .find(|chip| chip.id == id)
                .map(|chip| chip.name.as_str())
        };
        let shown = chosen.and_then(name_of).unwrap_or(text::AUTOMATIC_GPU);
        let running = self
            .running_gpu
            .as_ref()
            .map_or("", |chip| chip.name.as_str());
        let gone = chosen.is_some_and(|id| name_of(id).is_none());
        let note = text::gpu_note(
            running,
            (chosen != self.gpu_at_start).then_some(shown),
            gone,
        );
        // Only a chip that's there to switch to is worth restarting for.
        let waiting = chosen != self.gpu_at_start && !gone;
        let mut restart = false;
        let chips = self.gpu_chips.clone();
        components::settings_group(ui, palette, "Graphics", |ui| {
            components::settings_row(
                ui,
                palette,
                ("Graphics chip", None),
                GPU_DROPDOWN_WIDTH,
                |ui| {
                    let name = format!("Graphics chip: {shown}");
                    let button = ui.add(components::Dropdown::new(
                        palette,
                        &name,
                        shown,
                        GPU_DROPDOWN_WIDTH,
                    ));
                    let width = components::menu_width(&button);
                    egui::Popup::menu(&button).show(|ui| {
                        let automatic =
                            MenuItem::choice(palette, text::AUTOMATIC_GPU, chosen.is_none())
                                .width(width);
                        if ui.add(automatic).clicked() {
                            picked = Some(None);
                        }
                        for chip in &chips {
                            let item =
                                MenuItem::choice(palette, &chip.name, chosen == Some(chip.id))
                                    .width(width);
                            if ui.add(item).clicked() {
                                picked = Some(Some(chip.id));
                            }
                        }
                    });
                },
            );
            components::settings_note(ui, palette, &note, None);
            if waiting {
                components::settings_wide_control(ui, |ui, _| {
                    let button = ActionButton::new(palette, Kind::Secondary, "Restart now")
                        .icon(icons::ROTATE_CW)
                        .small();
                    restart |= ui.add(button).clicked();
                });
            }
        });
        self.restart_asked |= restart;
        match picked {
            Some(choice) if choice != chosen => {
                self.settings.gpu = choice;
                true
            }
            _ => false,
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
                .add_filter("Video, audio and image files", &text::MEDIA_EXTENSIONS)
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

    /// The shape of the finished video. It sits in the top row rather than coming and going with
    /// the selection, so it is always there to be found.
    fn size_picker(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let name = format!("Size: {}", self.project.canvas.name());
        let (width, height) = self.project.canvas.size();
        let button = if self.fit.narrow {
            ui.add(IconButton::new(palette, Kind::Ghost, icons::RATIO, &name))
        } else {
            ui.add(ActionButton::new(palette, Kind::Ghost, &name).icon(icons::RATIO))
        };
        let button = button.on_hover_text(format!(
            "{name}. The shape of the finished video, now {width} x {height}"
        ));
        egui::Popup::menu(&button).show(|ui| {
            if let Some(original) = self.project.canvas.original {
                let chosen = self.project.canvas.is_chosen(None);
                let row = ui.add(MenuItem::shape_choice(
                    palette,
                    original,
                    ORIGINAL_NAME,
                    "",
                    chosen,
                ));
                let (width, height) = original;
                let note = format!("Same as your first video, {width} x {height}");
                components::note_on_rest(ui, palette, &row, &note);
                if row.clicked() {
                    self.choose_ratio(None);
                }
                ui.separator();
            }
            for (index, group) in SIZE_GROUPS.iter().enumerate() {
                if index > 0 {
                    ui.separator();
                }
                for choice in *group {
                    let chosen = self.project.canvas.is_chosen(Some(choice.ratio));
                    let row = ui.add(MenuItem::shape_choice(
                        palette,
                        choice.ratio,
                        choice.name,
                        choice.numbers,
                        chosen,
                    ));
                    components::note_on_rest(ui, palette, &row, choice.note);
                    if row.clicked() {
                        self.choose_ratio(Some(choice.ratio));
                    }
                }
            }
        });
    }

    /// Changes the background. A run of changes while a pointer is held is one step to undo.
    fn change_background(&mut self, colour: Colour) {
        if self.project.canvas.background != colour {
            if self.stage_before.is_none() {
                self.stage_before = Some(self.project.clone());
            }
            self.project.canvas.background = colour;
        }
    }

    /// The colour behind the picture: a set of swatches and any colour you like. It only shows
    /// where no picture covers the canvas, so it is out in the row with the project's size.
    fn background_picker(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let current = self.project.canvas.background;
        let [red, green, blue] = current;
        let shown = Color32::from_rgb(red, green, blue);
        let button = ui
            .add(IconButton::new(
                palette,
                Kind::Ghost,
                icons::PAINT_BUCKET,
                "Background colour",
            ))
            .on_hover_text("The colour behind the picture");
        // A dot on the button says what it is set to without opening it.
        ui.painter().circle(
            button.rect.right_bottom() - vec2(7.0, 7.0),
            4.0,
            shown,
            Stroke::new(1.0, palette.border_strong),
        );
        let mut picked = None;
        egui::Popup::menu(&button)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                picked =
                    components::colour_choices(ui, palette, current, &mut self.background_text);
            });
        if let Some(colour) = picked {
            self.change_background(colour);
        }
    }

    /// The top of the card: undo and redo, anything that needs saying, and Export.
    fn card_top_row(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let short = self.fit.short;
        let frame = Frame::new()
            .fill(palette.surface)
            .inner_margin(Margin::symmetric(14, if short { 4 } else { 8 }))
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
        let mut row_above = None;
        egui::Panel::top("card_top_row")
            .exact_size(self.fit.top_row_height())
            .show_separator_line(false)
            .frame(frame)
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = CONTROL_SPACING;
                    // Export goes in first, so whatever is said on the left gives way to it.
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let export = self.export_button(ui, palette);
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            self.top_row_left(ui, palette);
                            let free =
                                egui::Rangef::new(ui.cursor().left() + 8.0, export.left() - 8.0);
                            row_above = (free.span() > 0.0).then_some(free);
                        });
                    });
                });
            });
        self.row_above = row_above;
    }

    fn export_button(&mut self, ui: &mut egui::Ui, palette: &Palette) -> egui::Rect {
        let can_export = !self.project.tracks.is_empty();
        let export = if self.fit.tiny {
            ui.add_enabled(
                can_export,
                IconButton::new(palette, Kind::Primary, icons::UPLOAD, "Export").solid(),
            )
        } else {
            ui.add_enabled(
                can_export,
                ActionButton::new(palette, Kind::Primary, "Export").icon(icons::UPLOAD),
            )
        };
        let export = if can_export {
            export.on_hover_text("Export the timeline (Ctrl+E)")
        } else {
            export.on_disabled_hover_text("Add a video to the timeline first")
        };
        if export.clicked() {
            self.open_export();
        }
        export.rect
    }

    /// Undo and redo, the project's size and background, and anything that needs saying.
    fn top_row_left(&mut self, ui: &mut egui::Ui, palette: &Palette) {
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
        ui.add_space(8.0);
        self.size_picker(ui, palette);
        self.background_picker(ui, palette);
        if let Some(notice) = self.notice.clone() {
            ui.add_space(8.0);
            components::symbol(ui, &icons::CIRCLE_ALERT, 16.0, palette.danger);
            ui.add(egui::Label::new(RichText::new(&notice).color(palette.danger)).truncate())
                .on_hover_text(notice);
        } else if !self.preview.sound_works() {
            ui.add_space(8.0);
            let silent =
                RichText::new("Preview is silent on this computer").color(palette.text_muted);
            ui.add(egui::Label::new(silent).truncate());
        }
    }

    /// The list of tools down the left. It only ever shows what works today.
    fn rail(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let frame = Frame::new().fill(palette.bg_app).inner_margin(Margin {
            left: 8,
            right: 8,
            top: 0,
            bottom: 8,
        });
        let compact = self.fit.compact_rails();
        let rail = egui::Panel::left("rail")
            .exact_size(self.fit.rail_width())
            .show_separator_line(false)
            .frame(frame)
            .show(ui, |ui| {
                components::rail_scroll(ui, |ui| {
                    for (tab, icon, name) in [
                        (LeftPanel::Media, &icons::FILM, "My media"),
                        (LeftPanel::Record, &icons::VIDEO, "Record"),
                        (LeftPanel::Library, &icons::LIBRARY_BIG, "Content library"),
                        (LeftPanel::Text, &icons::TYPE, "Text"),
                        (LeftPanel::Transitions, &icons::BLEND, "Transitions"),
                    ] {
                        let open = self.media_panel_open && self.left_panel == tab;
                        let hint = if open {
                            format!("Close {name}")
                        } else {
                            format!("Open {name}")
                        };
                        if components::rail_tab(ui, palette, icon, name, name, open, compact)
                            .on_hover_text(hint)
                            .clicked()
                        {
                            self.media_panel_open = !open;
                            self.left_panel = tab;
                        }
                    }
                });
            });
        self.left_rail_rect = rail.response.rect;
    }

    fn media_panel(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let frame = panel_card(palette.surface, Margin::same(14))
            .corner_radius(CornerRadius::same(PANEL_RADIUS));
        if self.fit.floating_panels() {
            self.media_panel_rect = egui::Rect::NOTHING;
            if !self.media_panel_open {
                return;
            }
            let ctx = ui.ctx().clone();
            let rect = self.floating_rect(&ctx, Side::Left, MEDIA_PANEL_WIDTH);
            let mut close = false;
            let shown = floating_panel(&ctx, "media_panel_floating", rect, frame, |ui| {
                close = self.media_panel_body(ui, palette);
            });
            if close || pressed_elsewhere(&ctx, [shown, self.left_rail_rect]) {
                self.media_panel_open = false;
            }
            self.media_panel_rect = shown;
            return;
        }
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
                close = self.media_panel_body(ui, palette);
            });
        if close {
            self.media_panel_open = false;
        }
        self.media_panel_rect = panel.map_or(egui::Rect::NOTHING, |panel| panel.response.rect);
    }

    /// What the left panel holds, whichever way it is shown. Says whether it was closed.
    fn media_panel_body(&mut self, ui: &mut egui::Ui, palette: &Palette) -> bool {
        let mut close = false;
        let on_text = self.left_panel == LeftPanel::Text;
        let on_transitions = self.left_panel == LeftPanel::Transitions;
        let on_library = self.left_panel == LeftPanel::Library;
        ui.horizontal(|ui| {
            let heading = match self.left_panel {
                LeftPanel::Media => "My media",
                LeftPanel::Record => "Record",
                LeftPanel::Library => "Content library",
                LeftPanel::Text => "Text",
                LeftPanel::Transitions => "Transitions",
            };
            ui.label(RichText::new(heading).font(fonts::strong(15.0)));
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
        if on_text {
            self.text_panel(ui, palette);
            return close;
        }
        if on_transitions {
            self.transitions_panel(ui, palette);
            return close;
        }
        if on_library {
            self.library_panel(ui, palette);
            return close;
        }
        if self.left_panel == LeftPanel::Record {
            self.record_panel(ui, palette);
            return close;
        }
        let width = ui.available_width();
        // Once there are videos, what comes next is on the cards and the timeline.
        let kind = if self.visible_media().next().is_none() {
            Kind::Primary
        } else {
            Kind::Secondary
        };
        let clicked = ui
            .add(
                ActionButton::new(palette, kind, "Add media")
                    .icon(icons::PLUS)
                    .width(width),
            )
            .on_hover_text("Add videos and audio to this project (Ctrl+O)")
            .clicked();
        if clicked {
            let ctx = ui.ctx().clone();
            self.pick_files(&ctx);
        }
        ui.add_space(12.0);

        if self.visible_media().next().is_none() {
            if self.project.library.is_empty() {
                empty_media_hint(ui, palette);
            } else {
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| self.library_media_cards(ui, palette));
            }
            return close;
        }

        self.media_selection_row(ui, palette);
        ui.add_space(10.0);

        let mut actions = CardActions::default();
        let mut rename = self.media_rename.take();
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
                let selecting = !self.selected_media.is_empty();
                for row in entries.chunks(columns) {
                    ui.columns(columns, |cells| {
                        for (cell, entry) in cells.iter_mut().zip(row) {
                            let state = self.card_state(entry.id);
                            let thumbnail = media_card(
                                cell,
                                palette,
                                entry,
                                state,
                                CardMode { compact, selecting },
                                &mut rename,
                                &mut actions,
                            );
                            thumbnails.push((entry.id, thumbnail));
                        }
                    });
                    ui.add_space(MEDIA_CARD_SPACING);
                }
                if !self.project.library.is_empty() {
                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(6.0);
                    self.library_media_cards(ui, palette);
                }
            });
        self.card_thumbnails = thumbnails;
        if let Some(id) = actions.add {
            self.add_to_timeline(id);
        }
        if let Some(id) = actions.remove {
            self.request_remove(id);
        }
        if let Some(id) = actions.delete_recording {
            self.confirm_delete_recording = Some(id);
        }
        self.media_rename = rename;
        if let Some((id, typed)) = actions.rename {
            self.rename_media(id, &typed);
        }
        if let Some(id) = actions.toggle {
            self.toggle_media(id);
        }
        close
    }

    /// Where a side panel goes when it floats over the card, beside its rail.
    fn floating_rect(&self, ctx: &egui::Context, side: Side, width: f32) -> egui::Rect {
        let screen = ctx.content_rect();
        let rail = match side {
            Side::Left => self.left_rail_rect,
            Side::Right => self.right_rail_rect,
        };
        let room = screen.width() - self.fit.rail_width() * 2.0 - FLOATING_PANEL_GAP;
        let width = width.min(room);
        let left = match side {
            Side::Left => rail.right(),
            Side::Right => rail.left() - width,
        };
        egui::Rect::from_min_max(
            pos2(left, rail.top()),
            pos2(left + width, screen.bottom() - FLOATING_PANEL_GAP),
        )
    }

    /// A panel on the right: beside the card, or over it in a narrow window, where a press
    /// anywhere else closes it.
    pub(super) fn right_side_panel(
        &mut self,
        ui: &mut egui::Ui,
        id: &'static str,
        frame: Frame,
        add: impl FnOnce(&mut Self, &mut egui::Ui),
    ) {
        if self.fit.floating_panels() {
            let ctx = ui.ctx().clone();
            let rect = self.floating_rect(&ctx, Side::Right, looks::LOOK_PANEL_WIDTH);
            let shown = floating_panel(&ctx, id, rect, frame, |ui| add(self, ui));
            if pressed_elsewhere(&ctx, [shown, self.right_rail_rect]) {
                self.right_panel = None;
            }
            // Counts as the one widget a docked panel would, so nothing after it is renumbered.
            ui.skip_ahead_auto_ids(1);
        } else {
            egui::Panel::right(id)
                .exact_size(looks::LOOK_PANEL_WIDTH)
                .show_separator_line(false)
                .frame(frame)
                .show(ui, |ui| add(self, ui));
        }
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
            let some = chosen > 0 && !all;
            if ui
                .add(CheckBox::new(palette, all, name).partly(some))
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
            if chosen > 0 {
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
        let available = ui.available_height();
        let transport = self.fit.transport_height();
        // In a short window the video comes first: the timeline shows one row until dragged
        // taller, and can only take half the room.
        let full = if self.fit.short {
            let lowest = transport + SHORT_TIMELINE_ROOM;
            let tallest = (available * (1.0 - SHORT_PREVIEW_ROOM)).max(lowest);
            self.short_timeline_height
                .unwrap_or(lowest)
                .clamp(lowest, tallest)
        } else {
            let tallest = (available * TIMELINE_MAX_SHARE).max(TIMELINE_MIN_HEIGHT);
            self.timeline_height = self.timeline_height.clamp(TIMELINE_MIN_HEIGHT, tallest);
            self.timeline_height
        };
        let openness = ui.ctx().animate_bool_with_time(
            egui::Id::unique("timeline_open"),
            self.timeline_open,
            TIMELINE_SLIDE_SECONDS,
        );
        let closed_height = transport + f32::from(PANEL_GAP);
        let height = closed_height + (full - closed_height) * openness;
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
            self.timeline_resize_edge(ui, panel.response.rect, full);
        }
    }

    /// Dragging the line above the transport row changes the timeline's height.
    fn timeline_resize_edge(&mut self, ui: &mut egui::Ui, panel: egui::Rect, full: f32) {
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
            let height = full - response.drag_delta().y;
            if self.fit.short {
                self.short_timeline_height = Some(height);
            } else {
                self.timeline_height = height;
            }
        }
    }

    /// The row between the video and the timeline. When the timeline is collapsed it is
    /// all that is left of it, so it takes over the job of moving the playhead.
    fn transport_row(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let (row, _) = ui.allocate_exact_size(
            vec2(ui.available_width(), self.fit.transport_height()),
            Sense::hover(),
        );
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
            let wanted = if self.fit.tiny {
                TINY_TRANSPORT_WIDTH + TINY_TIME_READOUT_WIDTH
            } else {
                TRANSPORT_WIDTH + TIME_READOUT_WIDTH
            };
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
                    if let Some(time) = timeline::seek_bar(
                        ui,
                        palette,
                        room.max(40.0),
                        position,
                        total,
                        &mut self.seek_bar_held,
                    ) {
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
        // A very small window keeps Home and End for these.
        let ends = !self.fit.tiny;
        if ends
            && ui
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
            icons::UNDO,
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
            icons::REDO,
            "5 seconds forward",
            "Forward 5 seconds (Right arrow). One frame: Shift+Right",
        )
        .clicked()
        {
            self.skip(true);
        }
        if ends
            && ui
                .add_enabled(has_clips, quiet(icons::SKIP_FORWARD, "Go to end"))
                .on_hover_text("Go to the end (End)")
                .clicked()
        {
            self.seek(total);
        }
    }

    fn time_readout(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        ui.add_space(8.0);
        // A very small window shows whole seconds.
        let shown = |time| {
            let full = format_timecode(time);
            if self.fit.tiny {
                whole_seconds(&full).to_owned()
            } else {
                full
            }
        };
        ui.label(RichText::new(shown(self.position())).font(fonts::strong(15.0)));
        ui.colored_label(
            palette.text_muted,
            format!("/ {}", shown(self.project.tracks.duration())),
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
                label: match self.title_of(*clip) {
                    Some(title) => Self::title_label(title),
                    None => self
                        .entry(clip.media)
                        .map_or_else(String::new, |entry| entry.name.clone()),
                },
                media: clip.media,
                source_start: clip.source.start,
                has_sound: self.has_sound(clip.media),
                muted: clip.sound.muted,
                noise_suppression: clip.sound.noise_suppression,
                sound_only: self.project.tracks.is_sound_row(track),
                shows_waveform: self
                    .settings
                    .shows_waveform(self.project.tracks.is_sound_row(track)),
                is_still: clip.is_still(),
                is_title: clip.is_title(),
                art: self.art_of(*clip).map(Art::kind),
                size: self.video_size(clip.media),
                speed: clip.speed,
                fade_in: clip.sound.fade_in_on(clip.duration()),
                fade_out: clip.sound.fade_out_on(clip.duration()),
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
        let joins = self.join_views();
        let view = TimelineView {
            tracks: &self.project.tracks,
            canvas: self.project.canvas.size(),
            clips: &clips,
            faces: &faces,
            position: self.position(),
            selected: self.selected,
            also_selected: &self.also_selected,
            selected_gap: self.selected_gap,
            can_paste: !self.clipboard.is_empty(),
            joins: &joins,
            selected_join: self.selected_join,
        };
        let mut events = timeline::timeline(ui, palette, &mut self.timeline_state, &view);
        if let Some((id, at, just_opened)) = self.picture_menu {
            let (picked, open) = timeline::clip_menu_at(
                ui,
                palette,
                &mut self.timeline_state,
                &view,
                (id, at),
                just_opened,
                &mut events.changes,
            );
            events.menu = events.menu.or(picked);
            self.picture_menu = open.then_some((id, at, false));
        }
        let pointer_down = ui.input(|input| input.pointer.any_down());
        self.apply_timeline_events(events, pointer_down);
    }

    #[cfg(test)]
    fn change_filter_for_screen(&mut self, id: ClipId) {
        let look = bareclip_core::effects::ClipLook {
            filter: bareclip_core::effects::Filter::OrangeAndTeal,
            ..Default::default()
        };
        self.project.tracks.set_look(id, look);
    }

    /// Every place two clips touch on a row, for the timeline.
    fn join_views(&self) -> Vec<timeline::JoinView> {
        let tracks = &self.project.tracks;
        tracks
            .joins()
            .into_iter()
            .filter_map(|(clip, cut)| {
                Some(timeline::JoinView {
                    clip,
                    row: tracks.track_of(clip)?,
                    cut,
                    has_transition: tracks.clip(clip)?.transition.is_some(),
                })
            })
            .collect()
    }

    fn toggle_clip_mute(&mut self, id: ClipId) {
        if let Some(clip) = self.project.tracks.clip(id) {
            self.record();
            let muted = !clip.sound.muted;
            self.project.tracks.set_clip_sound(
                id,
                ClipSound {
                    muted,
                    ..clip.sound
                },
            );
        }
    }

    fn apply_timeline_events(&mut self, events: timeline::TimelineEvents, pointer_down: bool) {
        if let Some(id) = events.toggle_clip_mute {
            self.toggle_clip_mute(id);
        }
        // The clip's menu changes things the same way the clip's own controls do, as one undo step.
        if events.changes != stage::StageEvents::default() {
            self.apply_stage_events(events.changes, pointer_down);
        }
        if events.add_audio {
            self.add_audio_requested = true;
        }
        if events.add_text {
            self.media_panel_open = true;
            self.left_panel = LeftPanel::Media;
            self.left_panel = LeftPanel::Text;
        }
        if let Some((style, time, target)) = events.drop_title {
            self.add_title(style, time, Some(target));
        }
        if let Some((key, time, target)) = events.drop_art
            && let Some(item) = bareclip_core::art::item_named(key)
        {
            self.add_art(Art::of(item), time, Some(target), None);
        }
        if let Some(selection) = events.select {
            self.select_on_timeline(selection, events.select_how);
            self.selected_gap = events.select_gap;
            self.selected_join = None;
        }
        if let Some((id, kind)) = events.drop_transition {
            self.add_transition(id, kind);
        }
        if events.transition_missed {
            self.notice = Some("Drop a transition where two clips touch on a row.".to_owned());
        }
        if let Some(id) = events.select_join {
            self.select_join(id);
        }
        if let Some(id) = events.delete_join {
            self.selected_join = Some(id);
            self.delete_selected_join();
        }
        if let Some((id, edge, pointer)) = events.trim_started
            && let Some(clip) = self.project.tracks.clip(id)
        {
            self.preview.pause(self.project.tracks.played_videos());
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
        if let Some((by, rows)) = events.move_group {
            let before = self.project.clone();
            let group = self.selected_clips();
            self.project.tracks.move_clips(&group, by, rows);
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
            Some(MenuAction::Duplicate(id)) => self.duplicate_clips(&self.group_with(id)),
            Some(MenuAction::Copy(id)) => self.copy_clips(&self.group_with(id)),
            Some(MenuAction::Paste) => self.paste_clip(),
            Some(MenuAction::EditDuration(id)) => self.open_edit_duration(id),
            Some(MenuAction::Delete(id)) => {
                if !self.selected_clips().contains(&id) || !self.several_selected() {
                    self.select_only(Some(id));
                }
                self.delete_selected();
            }
            Some(MenuAction::Detach(id)) => self.detach_sound(id),
            Some(MenuAction::Crop(id)) => {
                self.selected = Some(id);
                self.selected_gap = None;
                self.selected_join = None;
                self.stage_state.begin_crop(id);
            }
            Some(MenuAction::OpenPanel(id, tab)) => self.open_clip_panel(id, tab),
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
        let Some(duration) = self.footage_of(clip.media) else {
            return;
        };
        // The pointer moves along the timeline, and a fast clip gets through more footage there.
        let moved = clip.speed.in_footage(Ticks(pointer.0 - origin.pointer.0));
        let wanted = Ticks(origin.source.0 + moved.0);
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
            trimmed.is_some_and(|clip| {
                let edge_at = match edge {
                    ClipEdge::Start => clip.start,
                    ClipEdge::End => clip.end(),
                };
                // At another speed, rounding can leave it a few ticks off, far under a frame.
                (edge_at.0 - moment.0).abs() <= SNAPPED_CLOSE_ENOUGH
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
            self.stage_rect = rect;
            self.take_art_dropped_on_picture(ui, palette, rect);
            let canvas = self.canvas_size();
            let played = self.project.tracks.played();
            let only_sound = played.is_empty() && !self.project.tracks.is_empty();
            // With an image there may be no video playing at all, and what shows is drawn anyway.
            let drawn_stills = self.layer_display.is_some() && self.project.tracks.has_stills();
            if (self.preview.in_gap(self.project.tracks.played_videos()) || only_sound)
                && !drawn_stills
            {
                if let Some(size) = canvas {
                    let [red, green, blue] = self.project.canvas.background;
                    ui.painter().rect_filled(
                        fitted(size, rect),
                        0.0,
                        Color32::from_rgb(red, green, blue),
                    );
                }
                self.stage_controls(ui, rect, palette);
                return;
            }
            let layers = self.takes_layers();
            let layered_picture = self
                .layer_display
                .as_ref()
                .filter(|_| layers)
                .and_then(LayerDisplay::picture);
            if let (Some(texture), Some(size)) = (layered_picture, canvas) {
                ui.painter().image(
                    texture,
                    fitted(size, rect),
                    egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                self.stage_controls(ui, rect, palette);
                return;
            }
            if let Some(picture) = &self.picture {
                let size = picture.size_vec2();
                // A video of another shape gets black bars here, as it does in the export.
                let frame = fitted(canvas.unwrap_or(size), rect);
                let shown = fitted(size, frame);
                if shown.size() != frame.size() {
                    ui.painter().rect_filled(frame, 0.0, Color32::BLACK);
                }
                ui.painter().image(
                    picture.id(),
                    shown,
                    egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                self.stage_controls(ui, rect, palette);
                return;
            }
            if !self.project.tracks.is_empty() {
                self.stage_controls(ui, rect, palette);
                return;
            }

            // The shape of what will be made, so a ratio picked now shows what it does.
            let (width, height) = self.project.canvas.size();
            ui.painter().rect_stroke(
                fitted(vec2(width as f32, height as f32), rect),
                0.0,
                Stroke::new(1.0, palette.border),
                egui::StrokeKind::Inside,
            );
            let ctx = ui.ctx().clone();
            let logo = self.logo(&ctx);
            let hint = if self.visible_media().next().is_none() {
                "Add a video to get started"
            } else {
                "Press + on a video to put it on the timeline"
            };
            // In a short window the logo shrinks, and goes once it would be too small to read.
            let logo_height =
                (rect.height() - EMPTY_HINT_TEXT_ROOM - EMPTY_LOGO_GAP).min(EMPTY_LOGO_SIZE.y);
            let logo = logo.filter(|_| logo_height >= EMPTY_LOGO_SMALLEST);
            let content = if logo.is_some() {
                logo_height + EMPTY_LOGO_GAP + EMPTY_HINT_TEXT_ROOM
            } else {
                EMPTY_HINT_TEXT_ROOM
            };
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(((rect.height() - content) / 2.0).max(0.0));
                    if let Some(logo) = logo {
                        let size = EMPTY_LOGO_SIZE * (logo_height / EMPTY_LOGO_SIZE.y);
                        ui.add(egui::Image::new(&logo).fit_to_exact_size(size));
                        ui.add_space(EMPTY_LOGO_GAP);
                    }
                    ui.label(
                        RichText::new(hint)
                            .font(fonts::strong(18.0))
                            .color(palette.text),
                    );
                });
            });
            self.stage_controls(ui, rect, palette);
        });
    }

    /// Adds something from the Content library that was dragged onto the picture, where it was let
    /// go. A frame, overlay or background covers the canvas, so only its time comes from this.
    fn take_art_dropped_on_picture(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        area: egui::Rect,
    ) {
        let ctx = ui.ctx().clone();
        let carrying = egui::DragAndDrop::payload::<MediaDrag>(&ctx).and_then(|drag| drag.art);
        let Some(key) = carrying else {
            return;
        };
        let zone = ui.interact(area, ui.scope_id().with("stage-art-drop"), Sense::hover());
        let canvas = self.project.canvas.size();
        let frame = fitted(vec2(canvas.0 as f32, canvas.1 as f32), area);
        if zone.contains_pointer() {
            ui.painter().rect_stroke(
                frame,
                0.0,
                Stroke::new(2.0, palette.accent),
                egui::StrokeKind::Inside,
            );
        }
        if zone.dnd_release_payload::<MediaDrag>().is_none() {
            return;
        }
        let Some(item) = bareclip_core::art::item_named(key) else {
            return;
        };
        let pointer = ctx.input(|input| input.pointer.latest_pos());
        let middle = pointer.map(|at| {
            (
                f64::from((at.x - frame.left()) / frame.width().max(1.0)),
                f64::from((at.y - frame.top()) / frame.height().max(1.0)),
            )
        });
        let time = self.position();
        self.add_art(Art::of(item), time, None, middle);
    }

    /// The handles and toolbar on the picture, and what they ask for.
    /// The selected clip's sound, if its file has any.
    fn selected_sound(&self) -> Option<stage::SelectedSound> {
        let id = self.selected?;
        let clip = self.project.tracks.clip(id)?;
        self.has_sound(clip.media)
            .then(|| stage::SelectedSound::of(&self.project.tracks, id))
            .flatten()
    }

    /// The selected clip's speed. An image has no footage to play through.
    fn selected_speed(&self) -> Option<stage::SelectedSpeed> {
        let id = self.selected?;
        let clip = self.project.tracks.clip(id)?;
        (!clip.is_still()).then(|| stage::SelectedSpeed {
            id,
            speed: clip.speed,
            has_sound: self.has_sound(clip.media),
        })
    }

    /// Gives a clip's sound tracks clips of their own.
    fn detach_sound(&mut self, id: ClipId) {
        let Some(clip) = self.project.tracks.clip(id) else {
            return;
        };
        let Some(count) = self.entry(clip.media).map(|entry| entry.info.audio.len()) else {
            return;
        };
        self.record();
        self.project.tracks.detach_sound(id, count as u32);
        self.after_timeline_change();
    }

    fn stage_controls(&mut self, ui: &mut egui::Ui, area: egui::Rect, palette: &Palette) {
        let canvas = self.project.canvas.size();
        let size = vec2(canvas.0 as f32, canvas.1 as f32);
        let position = last_moment_by(self.position(), self.project.tracks.duration());
        let showing = self
            .project
            .tracks
            .stack_at(position, canvas, &|id| self.video_size(id));
        // Lowest first, including a clip so hidden it doesn't show, so that selecting one on the
        // timeline still gives it handles.
        let clips: Vec<StageClip> = self
            .project
            .tracks
            .picture_tracks()
            .iter()
            .rev()
            .filter_map(|track| {
                track
                    .clips()
                    .iter()
                    .find(|clip| position >= clip.start && position < clip.end())
            })
            .filter_map(|clip| {
                Some(StageClip {
                    id: clip.id,
                    placement: clip.placement,
                    size: self.video_size(clip.media)?,
                    visible: showing.iter().any(|shown| shown.id == clip.id),
                })
            })
            .collect();
        let view = StageView {
            frame: fitted(size, area),
            area,
            canvas,
            clips: &clips,
            // With several selected, the picture's controls step aside, as they change one clip.
            selected: self.selected.filter(|_| !self.several_selected()),
            row_above: self.row_above,
            can_go_forward: self.selected.is_some_and(|id| {
                self.project
                    .tracks
                    .can_move_in_order(id, OrderMove::Forward)
            }),
            can_go_back: self.selected.is_some_and(|id| {
                self.project
                    .tracks
                    .can_move_in_order(id, OrderMove::Backward)
            }),
            selected_is_title: self.selected_title().is_some(),
            art: self.selected_art().map(|(_, art)| stage::ArtBar {
                fill: art.fill,
                outline: (art.kind() == bareclip_core::art::ArtKind::Shape).then_some(art.outline),
                has_more: matches!(
                    art.kind(),
                    bareclip_core::art::ArtKind::Shape | bareclip_core::art::ArtKind::Background
                ),
                places_itself: art.kind().fills_canvas(),
            }),
        };
        let events = stage::show(ui, palette, &mut self.stage_state, &view);
        let pointer_down = ui.input(|input| input.pointer.any_down());
        self.apply_stage_events(events, pointer_down);
    }

    /// A change stays one undo step for as long as a pointer is held down, however it began.
    fn apply_stage_events(&mut self, events: stage::StageEvents, pointer_down: bool) {
        if let Some(selection) = events.select {
            self.select_only(selection);
            self.selected_gap = None;
        }
        if let Some((id, order)) = events.order {
            self.record();
            self.project.tracks.move_in_order(id, order);
            self.after_timeline_change();
        }
        if let Some((id, placement)) = events.place {
            let before = self.project.clone();
            let placement = self.title_placement(id, placement);
            self.project.tracks.set_placement(id, placement);
            if self.project != before && self.stage_before.is_none() {
                self.stage_before = Some(before);
            }
        }
        if !pointer_down {
            self.title_resize = None;
        }
        if let Some((id, sound)) = events.sound {
            let before = self.project.clone();
            if self.project.tracks.set_clip_sound(id, sound) && self.stage_before.is_none() {
                self.stage_before = Some(before);
            }
        }
        if let Some((id, speed)) = events.speed {
            // Always worked out from how things were when the slider was taken hold of, so that
            // clips pushed along by slowing down go back if the slider comes back.
            let before = self
                .stage_before
                .clone()
                .unwrap_or_else(|| self.project.clone());
            let mut tracks = before.tracks.clone();
            tracks.set_speed(id, speed);
            if tracks != self.project.tracks {
                self.project.tracks = tracks;
                self.stage_before.get_or_insert(before);
                let position = self.position();
                self.seek(position);
            }
        }
        if let Some((id, at)) = events.menu {
            self.picture_menu = Some((id, at, true));
        }
        if let Some(id) = events.detach {
            self.detach_sound(id);
        }
        if let Some((id, change)) = events.art
            && let Some(media) = self.project.tracks.clip(id).map(|clip| clip.media)
        {
            match change {
                stage::ArtChange::Fill(fill) => self.change_art(media, |art| art.fill = fill),
                stage::ArtChange::Outline(width) => {
                    self.change_art(media, |art| art.outline = width);
                }
                stage::ArtChange::Spot(spot) => {
                    self.change_title(media, |title| title.spot = Some(spot));
                }
                stage::ArtChange::MoreOptions => self.open_clip_panel(id, LookTab::Shape),
            }
        }
        if !pointer_down
            && let Some(before) = self.stage_before.take()
            && before != self.project
        {
            self.history.record(before);
        }
    }

    /// The shape of the picture, even before any frame of it has been shown.
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

    /// Asks before deleting a recording: it leaves the project for good, and its file goes to the
    /// Recycle Bin.
    fn delete_recording_dialog(&mut self, ctx: &egui::Context, palette: &Palette) {
        let Some(id) = self.confirm_delete_recording else {
            return;
        };
        let Some(entry) = self.entry(id) else {
            self.confirm_delete_recording = None;
            return;
        };
        let name = entry.name.clone();
        let clips = self.project.tracks.clips_using(id);
        let mut confirmed = false;
        let mut cancelled = false;
        egui::Modal::new(egui::Id::unique("delete_recording_dialog"))
            .frame(dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width(400.0);
                components::dialog_title(
                    ui,
                    &icons::CIRCLE_ALERT,
                    palette.danger,
                    "Delete this recording?",
                );
                ui.add_space(12.0);
                ui.label(text::delete_recording_warning(&name, clips));
                ui.add_space(4.0);
                ui.colored_label(
                    palette.text_muted,
                    "It can be brought back from the Recycle Bin, but not with Undo.",
                );
                ui.add_space(18.0);
                components::dialog_buttons(ui, |ui| {
                    confirmed = ui
                        .add(
                            ActionButton::new(palette, Kind::Danger, "Move to Recycle Bin")
                                .icon(icons::TRASH_2)
                                .width(DIALOG_BUTTON_WIDTH * 1.8),
                        )
                        .clicked();
                    cancelled = ui.add(dialog_button(palette, "Cancel")).clicked();
                });
            });
        if cancelled || ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape)) {
            self.confirm_delete_recording = None;
        } else if confirmed {
            self.confirm_delete_recording = None;
            self.delete_recording(id);
        }
    }

    /// Takes a recording out of the project for good and moves its file to the Recycle Bin. The
    /// file is let go of first, and anything still reading it in the background may take a
    /// moment to stop, so the move is tried again for a little while.
    fn delete_recording(&mut self, id: MediaId) {
        let Some(path) = self.entry(id).map(|entry| entry.info.path.clone()) else {
            return;
        };
        self.remove_many(&[id]);
        let trash = self.trash_file;
        let (sender, receiver) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let started = Instant::now();
            let result = loop {
                match trash(&path) {
                    Ok(()) => break Ok(()),
                    Err(_) if started.elapsed() < TRASH_RETRY_FOR => {
                        thread::sleep(Duration::from_millis(100));
                    }
                    Err(problem) => {
                        let name = path
                            .file_name()
                            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
                        break Err(format!(
                            "{name} was removed from the project but could not be moved to the Recycle Bin. {problem}"
                        ));
                    }
                }
            };
            let _ = sender.send(result);
        });
        self.trashing.push(receiver);
    }

    /// Says if a recording couldn't be moved to the Recycle Bin.
    fn check_trashing(&mut self) {
        let mut problems = Vec::new();
        self.trashing.retain(|receiver| match receiver.try_recv() {
            Ok(result) => {
                if let Err(problem) = result {
                    problems.push(problem);
                }
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => false,
        });
        if !problems.is_empty() {
            self.notice = Some(problems.join(" "));
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
        let route = self.export_route();
        let full_plan = self.export_plan();
        let ready =
            self.export_parts().is_some() || self.sound_only || matches!(route, Route::Full(_));
        let sizes = self.export_sizes();
        let rates = rates_on_offer(&self.videos_on_timeline());
        let has_sound = !self.sound_spans_for_export().is_empty();
        let summary = self.plan_description();
        let rate_choice = self.rate_choice();
        let rate_pick = self.current_rate_pick();
        let making_gif = self.export_choices.format == VideoFormat::Gif;
        let can_keep_apart = self.can_keep_sound_apart() && !making_gif;
        let plain_sound = self.sound_is_plain();
        let has_picture = !self.project.tracks.played().clips().is_empty();
        let canvas = self.final_canvas();
        let gif_seconds = self.gif_seconds();

        egui::Modal::new(egui::Id::unique("export_dialog")).frame(dialog_frame(ctx)).show(ctx, |ui| {
            dialog_scroll(ui, |ui| {
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
                    if has_sound {
                        ui.horizontal(|ui| {
                            let video = ActionButton::new(palette, Kind::Ghost, "Video").icon(icons::FILM).selected(!self.sound_only);
                            if ui.add(video).clicked() {
                                self.sound_only = false;
                            }
                            let sound = ActionButton::new(palette, Kind::Ghost, "Sound only").icon(icons::AUDIO_LINES).selected(self.sound_only);
                            if ui.add(sound).clicked() {
                                self.sound_only = true;
                            }
                        });
                        ui.add_space(14.0);
                    }
                    if self.sound_only {
                        ui.label(RichText::new("Sound only").font(fonts::strong(13.0)));
                        ui.add_space(4.0);
                        ui.colored_label(
                            palette.text_muted,
                            "Everything that can be heard, mixed together and saved as an M4A file with no picture.",
                        );
                        ui.add_space(18.0);
                        components::dialog_buttons(ui, |ui| {
                            let start = ActionButton::new(palette, Kind::Primary, "Choose file and export")
                                .icon(icons::UPLOAD);
                            if ui.add_enabled(has_sound, start).clicked() {
                                begin_export = true;
                            }
                            if ui.add(dialog_button(palette, "Cancel")).clicked() {
                                next_state = Some(ExportState::Closed);
                            }
                        });
                        return;
                    }
                    export_format_row(ui, palette, &mut self.export_choices, has_picture);
                    ui.add_space(10.0);
                    if self.export_choices.format == VideoFormat::Gif {
                        gif_choice_row(ui, palette, &mut self.export_choices, canvas);
                    } else {
                        export_choice_row(ui, palette, &mut self.export_choices, &sizes, &rates);
                    }
                    ui.add_space(14.0);
                    if self.export_choices.format == VideoFormat::Gif
                        && gif_seconds > GIF_WARNING_SECONDS
                    {
                        ui.colored_label(palette.accent, text::gif_length_warning(gif_seconds));
                        ui.add_space(14.0);
                    }
                    if let Some(reason) = &rate_choice {
                        ui.label(
                            RichText::new("These videos have different frame rates")
                                .font(fonts::strong(13.0)),
                        );
                        ui.add_space(4.0);
                        ui.colored_label(palette.text_muted, reason);
                        ui.add_space(10.0);
                        if components::choice_card(
                            ui,
                            palette,
                            &icons::ZAP,
                            "Copy",
                            "Fast, at full quality. The pace changes where they join, which some software handles badly.",
                            rate_pick == RatePick::Copy,
                        )
                        .clicked()
                        {
                            self.rate_pick = Some(RatePick::Copy);
                        }
                        ui.add_space(8.0);
                        if components::choice_card(
                            ui,
                            palette,
                            &icons::PENCIL,
                            "Redraw",
                            "Slower and re-encoded, but one steady rate that all software handles.",
                            rate_pick == RatePick::Redraw,
                        )
                        .clicked()
                        {
                            self.rate_pick = Some(RatePick::Redraw);
                        }
                        ui.add_space(14.0);
                    }
                    if can_keep_apart {
                        ui.label(
                            RichText::new("Sound in the finished video").font(fonts::strong(13.0)),
                        );
                        ui.add_space(6.0);
                        if components::choice_card(
                            ui,
                            palette,
                            &icons::VOLUME_2,
                            "One mix",
                            "Everything together on one track, which sounds right in every player.",
                            !self.keep_sound_apart,
                        )
                        .clicked()
                        {
                            self.keep_sound_apart = false;
                        }
                        ui.add_space(8.0);
                        if components::choice_card(
                            ui,
                            palette,
                            &icons::AUDIO_LINES,
                            "Keep separate",
                            "Each on its own track, for editing later. Some players only play the first.",
                            self.keep_sound_apart,
                        )
                        .clicked()
                        {
                            self.keep_sound_apart = true;
                        }
                        ui.add_space(14.0);
                    }
                    if let Route::Full(reason) = &route {
                        ui.label(RichText::new("Full export").font(fonts::strong(13.0)));
                        ui.add_space(4.0);
                        if rate_choice.is_none() {
                            ui.colored_label(palette.text_muted, reason);
                            ui.add_space(8.0);
                        }
                        ui.label(text::full_export_plan(&full_plan));
                        ui.add_space(18.0);
                        components::dialog_buttons(ui, |ui| {
                            let start = ActionButton::new(palette, Kind::Primary, "Choose file and export")
                                .icon(icons::UPLOAD);
                            if ui.add_enabled(ready, start).clicked() {
                                begin_export = true;
                            }
                            if ui.add(dialog_button(palette, "Cancel")).clicked() {
                                next_state = Some(ExportState::Closed);
                            }
                        });
                        return;
                    }
                    ui.label(RichText::new("Fast export").font(fonts::strong(13.0)));
                    ui.add_space(4.0);
                    if rate_choice.is_none() {
                        ui.colored_label(
                            palette.text_muted,
                            if plain_sound {
                                "These videos match, so they can be copied rather than redrawn."
                            } else {
                                "The picture is copied rather than redrawn. Only the sound is rebuilt."
                            },
                        );
                    }
                    ui.add_space(10.0);
                    // New sound follows the timeline exactly, so only exact cuts stay in time with it.
                    if plain_sound {
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
                    }

                    if let Some(summary) = &summary {
                        ui.label(summary);
                    } else {
                        ui.colored_label(palette.text_muted, "Still reading the videos' frames.");
                    }
                    ui.add_space(18.0);
                    components::dialog_buttons(ui, |ui| {
                        let start = ActionButton::new(palette, Kind::Primary, "Choose file and export")
                            .icon(icons::UPLOAD);
                        if ui.add_enabled(ready, start).clicked() {
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

    /// What could be heard in an export, whichever way it goes.
    fn sound_spans_for_export(&self) -> Vec<bareclip_core::tracks::SoundSpan> {
        self.project.tracks.sound_spans(&|id| self.has_sound(id))
    }

    fn plan_description(&self) -> Option<String> {
        let mut copied = 0;
        let mut encoded = 0;
        for clip in self.project.tracks.played().clips() {
            let entry = self.entry(clip.media)?;
            let index = entry.index.as_ref()?;
            let mode = if self.sound_is_plain() {
                self.mode
            } else {
                CutMode::FrameExact
            };
            let plan = plan_export(index, &[clip.source], entry.info.video.time_base, mode);
            copied += plan.copied_frames;
            encoded += plan.encoded_frames;
        }
        Some(text::plan_summary(copied, encoded))
    }
}

/// One of the export window's dropdowns: its name above a button showing the current choice.
fn export_dropdown(
    ui: &mut egui::Ui,
    palette: &Palette,
    title: &str,
    current: &str,
    choices: impl FnOnce(&mut egui::Ui),
) {
    ui.vertical(|ui| {
        ui.set_width(EXPORT_DROPDOWN_WIDTH);
        ui.colored_label(palette.text_muted, title);
        ui.add_space(2.0);
        let name = format!("{title}: {current}");
        let button = ui.add(components::Dropdown::new(
            palette,
            &name,
            current,
            EXPORT_DROPDOWN_WIDTH,
        ));
        egui::Popup::menu(&button).show(choices);
    });
}

/// The kind of file to make. A sound-only project has no pictures to make a GIF from.
fn export_format_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    choices: &mut ExportChoices,
    has_picture: bool,
) {
    export_dropdown(
        ui,
        palette,
        "Format",
        text::format_name(choices.format),
        |ui| {
            for format in [VideoFormat::H264, VideoFormat::Av1, VideoFormat::Gif] {
                if format == VideoFormat::Gif && !has_picture {
                    continue;
                }
                let name = text::format_name(format);
                if ui
                    .add(MenuItem::choice(palette, name, choices.format == format))
                    .clicked()
                {
                    choices.format = format;
                }
            }
        },
    );
}

/// The Size and Frame rate choices a GIF has.
fn gif_choice_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    choices: &mut ExportChoices,
    canvas: Option<(u32, u32)>,
) {
    let offered = canvas.map_or_else(Vec::new, gif_sizes_on_offer);
    // A project smaller than the chosen size stays at its own.
    let shown = canvas.map_or(choices.gif_size, |size| {
        choices.gif_size.min(size.0.min(size.1))
    });
    ui.horizontal_top(|ui| {
        export_dropdown(ui, palette, "Resolution", &format!("{shown}p"), |ui| {
            for shorter in offered {
                let name = format!("{shorter}p");
                if ui
                    .add(MenuItem::choice(palette, &name, shorter == shown))
                    .clicked()
                {
                    choices.gif_size = shorter;
                }
            }
        });
        let rate_name = format!("{} fps", choices.gif_rate);
        export_dropdown(ui, palette, "Frame rate", &rate_name, |ui| {
            for fps in GIF_RATES {
                let name = format!("{fps} fps");
                if ui
                    .add(MenuItem::choice(palette, &name, fps == choices.gif_rate))
                    .clicked()
                {
                    choices.gif_rate = fps;
                }
            }
        });
    });
}

/// The Resolution, Quality and Frame rate choices.
fn export_choice_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    choices: &mut ExportChoices,
    sizes: &[(u32, &'static str)],
    rates: &[u32],
) {
    ui.horizontal_top(|ui| {
        let size_name = match choices.size {
            ExportSize::MatchSource => MATCH_SOURCE,
            ExportSize::Shorter(shorter) => SIZES
                .iter()
                .find(|(side, _)| *side == shorter)
                .map_or(MATCH_SOURCE, |(_, name)| *name),
        };
        export_dropdown(ui, palette, "Resolution", size_name, |ui| {
            let chosen = choices.size == ExportSize::MatchSource;
            if ui
                .add(MenuItem::choice(palette, MATCH_SOURCE, chosen))
                .clicked()
            {
                choices.size = ExportSize::MatchSource;
            }
            for (shorter, name) in sizes {
                let chosen = choices.size == ExportSize::Shorter(*shorter);
                if ui.add(MenuItem::choice(palette, name, chosen)).clicked() {
                    choices.size = ExportSize::Shorter(*shorter);
                }
            }
        });
        export_dropdown(
            ui,
            palette,
            "Quality",
            text::quality_name(choices.quality),
            |ui| {
                for quality in Quality::ALL {
                    let chosen = choices.quality == quality;
                    if ui
                        .add(MenuItem::choice(
                            palette,
                            text::quality_name(quality),
                            chosen,
                        ))
                        .clicked()
                    {
                        choices.quality = quality;
                    }
                }
            },
        );
        let rate_name = match choices.rate {
            ExportRate::MatchSource => MATCH_SOURCE.to_owned(),
            ExportRate::Fps(fps) => format!("{fps} fps"),
        };
        export_dropdown(ui, palette, "Frame rate", &rate_name, |ui| {
            let chosen = choices.rate == ExportRate::MatchSource;
            if ui
                .add(MenuItem::choice(palette, MATCH_SOURCE, chosen))
                .clicked()
            {
                choices.rate = ExportRate::MatchSource;
            }
            for fps in rates {
                let chosen = choices.rate == ExportRate::Fps(*fps);
                if ui
                    .add(MenuItem::choice(palette, &format!("{fps} fps"), chosen))
                    .clicked()
                {
                    choices.rate = ExportRate::Fps(*fps);
                }
            }
        });
    });
}

/// The largest place something of this size fits inside a rectangle, in the middle.
/// A moment that still shows something: the playhead stops at the very end of the project, where
/// nothing is left, but its last picture stays up.
fn last_moment_by(time: Ticks, end: Ticks) -> Ticks {
    Ticks(time.0.min(end.0 - 1).max(0))
}

fn fitted(size: egui::Vec2, within: egui::Rect) -> egui::Rect {
    let scale = (within.width() / size.x).min(within.height() / size.y);
    egui::Rect::from_center_size(within.center(), size * scale)
}

/// Shows a side panel over the card, as it is in a narrow window, and says where it went.
fn floating_panel(
    ctx: &egui::Context,
    id: &'static str,
    rect: egui::Rect,
    frame: Frame,
    add: impl FnOnce(&mut egui::Ui),
) -> egui::Rect {
    let frame = frame
        .outer_margin(Margin::ZERO)
        .shadow(ctx.global_style().visuals.popup_shadow);
    let inner = (rect.size() - frame.total_margin().sum()).max(vec2(0.0, 0.0));
    let shown = egui::Area::new(egui::Id::unique(id))
        .order(egui::Order::Middle)
        .fixed_pos(rect.min)
        .constrain(false)
        .show(ctx, |ui| {
            frame.show(ui, |ui| {
                ui.set_min_size(inner);
                ui.set_max_size(inner);
                add(ui);
            });
        })
        .response;
    // It stays above the clip's toolbar, which floats on the video too.
    ctx.move_to_top(shown.layer_id);
    shown.rect
}

/// Whether a press this frame landed on the screen outside these places, which closes a
/// floating panel. Presses on menus and dialogs don't count, as they belong to what is open.
fn pressed_elsewhere(ctx: &egui::Context, keep: [egui::Rect; 2]) -> bool {
    let Some(at) = ctx.input(|input| {
        input
            .pointer
            .primary_pressed()
            .then(|| input.pointer.press_origin())
            .flatten()
    }) else {
        return false;
    };
    !keep.iter().any(|rect| rect.contains(at))
        && ctx
            .layer_id_at(at)
            .is_none_or(|layer| layer.order == egui::Order::Background)
}

/// A time as shown, without its hundredths.
fn whole_seconds(time: &str) -> &str {
    time.split('.').next().unwrap_or(time)
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

/// How much of the window a dialog's contents can take, leaving its frame and a margin clear,
/// so a long one scrolls in a small window instead of running off it.
fn dialog_room(ctx: &egui::Context) -> egui::Vec2 {
    let around = 2.0 * (20.0 + DIALOG_WINDOW_MARGIN);
    (ctx.content_rect().size() - vec2(around, around)).max(vec2(200.0, 120.0))
}

/// Lets a dialog's contents scroll when the window is too short for them. Its height comes from
/// the window, not from where the dialog sat last frame, so a dialog never shrinks itself.
fn dialog_scroll<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let room = dialog_room(ui.ctx());
    egui::ScrollArea::vertical()
        .max_height(room.y)
        .min_scrolled_height(room.y)
        .auto_shrink([true, true])
        .show(ui, add)
        .inner
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
    let first = if entry.scan.is_some() && entry.info.is_still() {
        "Reading".to_owned()
    } else if entry.scan.is_some() {
        let fraction = f32::from_bits(entry.progress.load(Ordering::Relaxed)) * 100.0;
        format!("Reading frames {fraction:.0}%")
    } else {
        text::file_size(entry.info.size_bytes)
    };
    let mut details = vec![(first, false)];
    if !entry.info.is_sound_only() {
        details.push((text::video_summary(&entry.info), false));
    }
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
    let arrow = IconButton::new(palette, Kind::Ghost, icon, name)
        .size(32.0, SKIP_ARROW_SIZE)
        .icon_raised(SKIP_ARROW_RAISE);
    let button = ui.add_enabled(enabled, arrow).on_hover_text(hint);
    let color = if enabled {
        palette.text
    } else {
        palette.text_muted
    };
    ui.painter().text(
        button.rect.center() + vec2(0.0, SKIP_NUMBER_DROP),
        egui::Align2::CENTER_CENTER,
        "5",
        fonts::strong(SKIP_NUMBER_SIZE),
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

/// A small copy of an image, for its card and clips. Its colours are already scaled by how
/// see-through it is, so they can simply be averaged.
fn shrunk_still(still: &StillPicture) -> egui::ColorImage {
    let (width, height) = (still.width as usize, still.height as usize);
    let factor = (width / STILL_THUMBNAIL_WIDTH).max(1);
    let size = [width.div_ceil(factor), height.div_ceil(factor)];
    let mut shrunk = Vec::with_capacity(size[0] * size[1] * 4);
    for y in 0..size[1] {
        for x in 0..size[0] {
            let mut sum = [0_u32; 4];
            let mut count = 0;
            for source_y in (y * factor)..((y + 1) * factor).min(height) {
                for source_x in (x * factor)..((x + 1) * factor).min(width) {
                    let pixel = &still.rgba[(source_y * width + source_x) * 4..][..4];
                    for (total, value) in sum.iter_mut().zip(pixel) {
                        *total += u32::from(*value);
                    }
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
    mode: CardMode,
    rename: &mut Option<(MediaId, String)>,
    actions: &mut CardActions,
) -> egui::Rect {
    let CardMode { compact, selecting } = mode;
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
                length: if entry.info.is_still() {
                    TimeRange::new_still().duration()
                } else {
                    entry.info.duration
                },
                sound_only: entry.info.is_sound_only(),
                title: None,
                art: None,
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
                } else if entry.info.is_sound_only() {
                    let size = rect.height() * 0.4;
                    icons::paint(
                        &painter,
                        icons::square_at(rect.center(), size),
                        &icons::MUSIC,
                        palette.clip_video_text,
                    );
                }
                // An image has no length of its own to mention.
                if entry.info.is_still() {
                    return;
                }
                // The stage colour stays dark in both themes, so this label reads the same.
                let length = painter.layout_no_wrap(
                    text::whole_time(entry.info.duration),
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
            card_name(ui, palette, entry, state, rename, actions);
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
    let more = IconButton::new(palette, Kind::Secondary, icons::ELLIPSIS, "Video options")
        .size(button_size, icon_size)
        .hidden(!hovered)
        .show_at(
            ui,
            egui::Id::unique(("media-card-more", entry.id.0)),
            remove_rect,
        )
        .on_hover_text("Options")
        .on_hover_cursor(egui::CursorIcon::Default);
    egui::Popup::menu(&more).show(|ui| {
        if ui
            .add(MenuItem::new(palette, icons::PENCIL, "Rename", ""))
            .clicked()
        {
            *rename = Some((entry.id, entry.name.clone()));
        }
        if ui
            .add(MenuItem::new(
                palette,
                icons::TRASH_2,
                "Remove from this project",
                "",
            ))
            .clicked()
        {
            actions.remove = Some(entry.id);
        }
        if entry.recorded
            && ui
                .add(MenuItem::new(
                    palette,
                    icons::TRASH_2,
                    "Delete recording",
                    "",
                ))
                .clicked()
        {
            actions.delete_recording = Some(entry.id);
        }
    });
    let mut over_a_button = added.hovered() || more.hovered();
    // The boxes only exist once something is ticked, and they number what is.
    if selecting {
        let select_name = format!("Select {}", entry.name);
        let ticked = state.order.is_some();
        let select = CheckBox::new(palette, ticked, &select_name)
            .number(state.order)
            .show_in_corner(
                ui,
                egui::Id::unique(("media-card-select", entry.id.0)),
                thumbnail_rect,
                SELECT_BOX_MARGIN,
            )
            .on_hover_text(if ticked { "Deselect" } else { "Select" })
            .on_hover_cursor(egui::CursorIcon::Default);
        over_a_button |= select.hovered();
        if select.clicked() {
            actions.toggle = Some(entry.id);
        }
    }
    let ctrl_clicked = ui.input(|input| input.pointer.primary_clicked() && input.modifiers.ctrl);
    if hovered && ctrl_clicked && !over_a_button {
        actions.toggle = Some(entry.id);
    }
    thumbnail_rect
}

/// What hovering a thumbnail says. Squeezed cards keep their details here instead.
fn card_hint(entry: &MediaEntry, compact: bool) -> String {
    let drag = "Drag onto the timeline\nCtrl+click to select";
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
fn card_name(
    ui: &mut egui::Ui,
    palette: &Palette,
    entry: &MediaEntry,
    state: CardState,
    rename: &mut Option<(MediaId, String)>,
    actions: &mut CardActions,
) {
    let (row, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), CARD_NAME_HEIGHT), Sense::hover());
    if let Some((_, typed)) = rename.as_mut().filter(|(id, _)| *id == entry.id) {
        let edit = ui.put(
            row,
            egui::TextEdit::singleline(typed)
                .id(egui::Id::unique(("media-rename", entry.id)))
                .margin(Margin::symmetric(4, 0))
                .desired_width(row.width()),
        );
        edit.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Video name"));
        let cancelled = ui.input(|input| input.key_pressed(Key::Escape));
        if edit.lost_focus() {
            if !cancelled {
                actions.rename = Some((entry.id, typed.clone()));
            }
            *rename = None;
        } else if !edit.has_focus() {
            edit.request_focus();
        }
        return;
    }
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
    use bareclip_core::effects::{Filter, TRANSITION_GROUPS, Transition, TransitionKind};

    use super::*;
    use bareclip_core::placement::{Placement, WHOLE};
    use bareclip_core::speed::Speed;
    use bareclip_core::title::TitleSpot;
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

    /// A short clip at a steady rate.
    fn clip_at_rate(name: &str, rate: u32) -> PathBuf {
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
            .arg(format!("testsrc2=s=320x180:r={rate}:d=2,format=yuv420p"))
            .args(["-c:v", "libopenh264", "-b:v", "1M", "-g", "30"])
            .arg(&path)
            .status()
            .expect("ffmpeg is on PATH");
        assert!(status.success());
        path
    }

    /// A short clip whose frames keep changing pace, like real screen recordings.
    fn uneven_clip(name: &str, rate: u32) -> PathBuf {
        let folder = std::env::temp_dir().join("bareclip-app-tests");
        std::fs::create_dir_all(&folder).expect("the folder can be created");
        let path = folder.join(format!("{name}-{}.mp4", std::process::id()));
        let frame = 1.0 / f64::from(rate);
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
            .arg(format!("testsrc2=s=320x180:r={rate}:d=2,format=yuv420p"))
            .args([
                "-vf",
                &format!("setpts='(N*{frame}+0.3*{frame}*sin(N*2.1))/TB'"),
                "-fps_mode",
                "passthrough",
                "-c:v",
                "libopenh264",
                "-b:v",
                "1M",
                "-g",
                "30",
            ])
            .arg(&path)
            .status()
            .expect("ffmpeg is on PATH");
        assert!(status.success());
        path
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
            screen_rect: Some(screen()),
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
        renderer: Arc<egui::mutex::RwLock<egui_wgpu::Renderer>>,
        folder: PathBuf,
    }

    const CAMERA_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
    thread_local! {
        /// The window's size in tests, which a test can change for its own thread.
        static SCREEN: std::cell::Cell<[u32; 2]> = const { std::cell::Cell::new([1280, 800]) };
    }

    fn screen() -> egui::Rect {
        let [width, height] = SCREEN.get();
        egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(width as f32, height as f32))
    }

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
            let renderer = Arc::new(egui::mutex::RwLock::new(egui_wgpu::Renderer::new(
                &device,
                CAMERA_FORMAT,
                Default::default(),
            )));
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
            self.save(ctx, output, name);
        }

        /// Draws a frame that has already run, wherever it came from, and saves it.
        fn save(&mut self, ctx: &egui::Context, output: egui::FullOutput, name: &str) {
            let mut textures = KEPT_TEXTURES
                .with_borrow_mut(|kept| std::mem::take(kept.as_mut().expect("the camera is on")));
            for delta in &textures {
                for (id, images) in &delta.set {
                    for image in images {
                        self.renderer
                            .write()
                            .update_texture(&self.device, &self.queue, *id, image);
                    }
                }
            }
            let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
            let screen = egui_wgpu::ScreenDescriptor {
                size_in_pixels: SCREEN.get(),
                pixels_per_point: output.pixels_per_point,
            };
            let size = wgpu::Extent3d {
                width: SCREEN.get()[0],
                height: SCREEN.get()[1],
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
                (SCREEN.get()[0] * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
            let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: u64::from(row_bytes * SCREEN.get()[1]),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });

            let mut encoder = self.device.create_command_encoder(&Default::default());
            let extra = self.renderer.write().update_buffers(
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
                self.renderer.read().render(&mut pass, &jobs, &screen);
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
                format!("P6 {} {} 255\n", SCREEN.get()[0], SCREEN.get()[1]).into_bytes();
            for row in data.chunks(row_bytes as usize) {
                for pixel in row[..SCREEN.get()[0] as usize * 4].chunks(4) {
                    picture.extend_from_slice(&pixel[..3]);
                }
            }
            std::fs::write(self.folder.join(format!("{name}.ppm")), picture)
                .expect("the picture can be saved");
            drop(data);
            for delta in &mut textures {
                for id in &delta.free {
                    self.renderer.write().free_texture(id);
                }
                delta.clear();
            }
        }
    }

    fn moved(at: egui::Pos2) -> Vec<egui::Event> {
        vec![egui::Event::PointerMoved(at)]
    }

    /// Draws this review's changes on the real screen, with no window. Run with
    /// `$env:BARECLIP_SCREENS = '<folder>'; ./scripts/cargo.ps1 test -p bareclip-app draw_the_tidying_screens '--' --ignored`
    #[test]
    #[ignore = "needs a real GPU, and draws pictures to look at rather than checking anything"]
    fn draw_the_tidying_screens() {
        let folder = std::env::var_os("BARECLIP_SCREENS").map_or_else(
            || std::env::temp_dir().join("bareclip-screens"),
            PathBuf::from,
        );
        let ctx = context();
        let mut camera = Camera::new(folder);

        let mut one = two_videos(&ctx);
        camera.shoot(&ctx, &mut one, vec![], "tidy-01-one-track-lanes");

        let mut editor = on_two_tracks(&ctx);
        camera.shoot(&ctx, &mut editor, vec![], "tidy-02-two-tracks-no-lanes");
        let area = editor.timeline_state.layout.area;
        let far = pos2(area.right() - 30.0, area.center().y);
        camera.shoot(&ctx, &mut editor, moved(far), "tidy-03-hover-past-the-end");

        // Where a new track lands now that there are no lanes.
        let top = editor.timeline_state.layout.track(0);
        let from = pos2(
            editor.timeline_state.scale.x_of(Ticks::from_seconds(0.5)),
            top.center().y,
        );
        let above = pos2(from.x, top.top() - 4.0);
        begin_drag(&ctx, &mut editor, from, above);
        camera.shoot(&ctx, &mut editor, moved(above), "tidy-04-new-track-above");
        let below = pos2(from.x, editor.timeline_state.layout.track(1).bottom() + 6.0);
        camera.shoot(&ctx, &mut editor, moved(below), "tidy-05-new-track-below");
        run_frame(
            &ctx,
            &mut editor,
            pointer_button(from, egui::PointerButton::Primary, false),
        );
        editor.undo();
        run_frame(&ctx, &mut editor, moved(pos2(640.0, 100.0)));

        open_size_menu(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "tidy-06-size-menu");
        let row = named_rect(&ctx, &mut editor, "Vertical 9:16");
        run_frame(&ctx, &mut editor, moved(row.center()));
        for _ in 0..60 {
            run_frame(&ctx, &mut editor, vec![]);
        }
        camera.shoot(&ctx, &mut editor, vec![], "tidy-07-size-menu-note");
        click(&ctx, &mut editor, pos2(640.0, 400.0));
        run_frame(&ctx, &mut editor, vec![]);

        click_named(&ctx, &mut editor, "Settings");
        let_dialog_open(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Waveforms on audio");
        for _ in 0..30 {
            run_frame(&ctx, &mut editor, vec![]);
        }
        camera.shoot(&ctx, &mut editor, vec![], "tidy-08-settings-dark");
        ctx.set_theme(egui::ThemePreference::Light);
        run_frame(&ctx, &mut editor, vec![]);
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "tidy-09-settings-light");
        click_named(&ctx, &mut editor, "Done");
        run_frame(&ctx, &mut editor, vec![]);

        // A trim, from each end, on the light theme and then the dark.
        let mut trimming = with_videos(&ctx, &[test_clip()]);
        let scale = trimming.timeline_state.scale;
        let y = trimming.timeline_state.track.center().y;
        let end = pos2(scale.x_of(Ticks::from_seconds(4.0)) - 4.0, y);
        let to = pos2(scale.x_of(Ticks::from_seconds(3.0)), y);
        begin_drag(&ctx, &mut trimming, end, to);
        camera.shoot(&ctx, &mut trimming, moved(to), "tidy-10-trim-end-light");
        ctx.set_theme(egui::ThemePreference::Dark);
        run_frame(&ctx, &mut trimming, vec![]);
        camera.shoot(&ctx, &mut trimming, moved(to), "tidy-11-trim-end-dark");
        end_drag(&ctx, &mut trimming, to);
        run_frame(&ctx, &mut trimming, vec![]);
        let start = pos2(scale.x_of(Ticks(0)) + 4.0, y);
        let to = pos2(scale.x_of(Ticks::from_seconds(1.0)), y);
        begin_drag(&ctx, &mut trimming, start, to);
        camera.shoot(&ctx, &mut trimming, moved(to), "tidy-12-trim-start-dark");
        end_drag(&ctx, &mut trimming, to);
    }

    /// Pictures of a mixed project: its export window, and black bars in the preview.
    #[test]
    #[ignore = "needs a GPU, and writes pictures to look at"]
    fn draw_the_mixed_project_screens() {
        let folder = std::env::var_os("BARECLIP_SCREENS").map_or_else(
            || std::env::temp_dir().join("bareclip-screens"),
            PathBuf::from,
        );
        let ctx = context();
        let mut camera = Camera::new(folder);
        let tall = make_clip("editor-tall", "180x320");
        let mut editor = with_videos(&ctx, &[test_clip(), &tall]);
        editor.media_panel_open = false;
        let_panel_finish(&ctx, &mut editor);

        editor.seek(Ticks::from_seconds(1.0));
        for _ in 0..30 {
            run_frame(&ctx, &mut editor, vec![]);
            thread::sleep(Duration::from_millis(10));
        }
        camera.shoot(&ctx, &mut editor, vec![], "mixed-1-same-shape");

        editor.seek(Ticks::from_seconds(5.0));
        for _ in 0..30 {
            run_frame(&ctx, &mut editor, vec![]);
            thread::sleep(Duration::from_millis(10));
        }
        camera.shoot(&ctx, &mut editor, vec![], "mixed-2-black-bars");

        editor.export_state = ExportState::Choosing;
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "mixed-3-export");
        editor.export_state = ExportState::Closed;
        let_dialog_open(&ctx, &mut editor);

        camera.shoot(&ctx, &mut editor, vec![], "name-1-at-rest");
        editor.project_name = "a very long project name that does not fit ".repeat(4);
        camera.shoot(&ctx, &mut editor, vec![], "name-2-long");
        editor.project_name = project::DEFAULT_NAME.to_owned();
        editor.renaming = Some("Final round".to_owned());
        run_frame(&ctx, &mut editor, vec![]);
        camera.shoot(&ctx, &mut editor, vec![], "name-3-typing");
    }

    /// The usual size, half and a quarter of a 1080p screen at 125%, and the smallest allowed.
    const WINDOW_SIZES: [[u32; 2]; 4] = [[1280, 800], [768, 816], [768, 408], [640, 340]];

    /// An editor with a clip on the timeline and selected, so its toolbar and tools show.
    fn editor_with_a_selected_clip(ctx: &egui::Context) -> Editor {
        let mut editor = with_videos(ctx, &[test_clip()]);
        let clip = editor.project.tracks.tracks()[0].clips()[0].id;
        editor.selected = Some(clip);
        settle_animations(ctx, &mut editor);
        editor
    }

    #[test]
    fn every_main_control_is_inside_the_window_and_clear_of_the_others_at_every_size() {
        for size in WINDOW_SIZES {
            SCREEN.set(size);
            let ctx = context();
            let mut editor = editor_with_a_selected_clip(&ctx);
            let nodes = named_nodes(&ctx, &mut editor);
            let mut must_show = vec![
                "Close window",
                "Maximise window",
                "Minimise window",
                "Settings",
                "Project menu",
                "Undo",
                "Redo",
                "Background colour",
                "Export",
                "Split",
                "5 seconds back",
                "Play",
                "5 seconds forward",
                "Zoom out",
                "Zoom in",
                "My media",
                "Captions",
                "Crop",
                "Flip",
                "More",
            ];
            if size[0] as f32 >= TINY_WIDTH {
                must_show.extend(["Go to start", "Go to end", "Fit"]);
            }
            let found: Vec<(&str, egui::Rect)> = must_show
                .iter()
                .map(|name| {
                    let rect = nodes
                        .iter()
                        .find(|(label, ..)| label == name)
                        .unwrap_or_else(|| panic!("{name} is missing at {size:?}"))
                        .1;
                    (*name, rect)
                })
                .collect();
            for (name, rect) in &found {
                assert!(
                    screen().contains_rect(*rect),
                    "{name} at {rect:?} is outside the {size:?} window"
                );
            }
            for (index, (name, rect)) in found.iter().enumerate() {
                for (other, other_rect) in &found[index + 1..] {
                    assert!(
                        !rect.shrink(1.0).intersects(other_rect.shrink(1.0)),
                        "{name} {rect:?} and {other} {other_rect:?} overlap at {size:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_time_is_shown_whole_and_clear_of_the_right_rail_at_every_size() {
        for size in WINDOW_SIZES {
            SCREEN.set(size);
            let ctx = context();
            let mut editor = editor_with_a_selected_clip(&ctx);
            let output = run_frame(&ctx, &mut editor, vec![]);
            let time = if size[0] as f32 >= TINY_WIDTH {
                "0:00.00"
            } else {
                "0:00"
            };
            let shown = text_rect(&output, time)
                .unwrap_or_else(|| panic!("the time is missing at {size:?}"));
            assert!(
                shown.right() <= editor.right_rail_rect.left(),
                "the time {shown:?} runs under the rail at {size:?}"
            );
        }
    }

    #[test]
    fn the_rails_lose_their_names_in_a_narrow_or_short_window_only() {
        for (size, compact) in [
            ([1280, 800], false),
            ([768, 816], true),
            ([1280, 500], true),
        ] {
            SCREEN.set(size);
            let ctx = context();
            let mut editor = Editor::new(&ctx, false);
            let output = run_frame(&ctx, &mut editor, vec![]);
            let rail = editor.left_rail_rect;
            assert_eq!(
                text_in(&output, rail).iter().any(|text| text == "My media"),
                !compact,
                "{size:?}"
            );
            let width = if compact {
                COMPACT_RAIL_WIDTH
            } else {
                RAIL_WIDTH
            };
            assert!((rail.width() - width).abs() < 1.0, "{size:?} {rail:?}");
        }
    }

    #[test]
    fn in_a_narrow_window_my_media_floats_over_the_video_and_a_press_elsewhere_closes_it() {
        SCREEN.set([1280, 800]);
        let ctx = context();
        let mut editor = editor_with_a_selected_clip(&ctx);
        editor.media_panel_open = true;
        settle_animations(&ctx, &mut editor);
        SCREEN.set([768, 816]);
        settle_animations(&ctx, &mut editor);
        assert!(!editor.media_panel_open, "going narrow closes the panel");
        let timeline_left = editor.timeline_panel_rect.left();

        click_named(&ctx, &mut editor, "My media");
        settle_animations(&ctx, &mut editor);

        assert!(editor.media_panel_open);
        let panel = editor.media_panel_rect;
        assert!((panel.left() - editor.left_rail_rect.right()).abs() < 1.0);
        assert!(panel.right() > timeline_left, "it covers the card");
        assert!(
            (editor.timeline_panel_rect.left() - timeline_left).abs() < 1.0,
            "the video and timeline stay put"
        );
        assert!(screen().contains_rect(panel));

        let elsewhere = editor.stage_rect.right_bottom() - vec2(20.0, 20.0);
        click(&ctx, &mut editor, elsewhere);

        assert!(!editor.media_panel_open);
    }

    #[test]
    fn in_a_narrow_window_a_clips_tools_float_and_close_on_a_press_elsewhere() {
        SCREEN.set([768, 816]);
        let ctx = context();
        let mut editor = editor_with_a_selected_clip(&ctx);
        let timeline_right = editor.timeline_panel_rect.right();

        click_named(&ctx, &mut editor, "Captions");
        settle_animations(&ctx, &mut editor);

        assert_eq!(editor.right_panel, Some(RightPanel::Captions));
        assert!(
            (editor.timeline_panel_rect.right() - timeline_right).abs() < 1.0,
            "the video and timeline stay put"
        );
        let elsewhere = editor.stage_rect.left_bottom() + vec2(20.0, -20.0);
        click(&ctx, &mut editor, elsewhere);

        assert_eq!(editor.right_panel, None);
    }

    #[test]
    fn in_a_wide_window_my_media_still_sits_beside_the_video() {
        SCREEN.set([1280, 800]);
        let ctx = context();
        let mut editor = editor_with_a_selected_clip(&ctx);
        if !editor.media_panel_open {
            click_named(&ctx, &mut editor, "My media");
            settle_animations(&ctx, &mut editor);
        }

        assert!(editor.timeline_panel_rect.left() >= editor.media_panel_rect.right());

        let video = editor.stage_rect.center();
        click(&ctx, &mut editor, video);

        assert!(
            editor.media_panel_open,
            "a press on the video leaves it open"
        );
    }

    #[test]
    fn in_a_short_window_the_timeline_shows_one_row_so_the_video_gets_the_room() {
        SCREEN.set([768, 408]);
        let ctx = context();
        let editor = editor_with_a_selected_clip(&ctx);
        let timeline = editor.timeline_panel_rect.height();

        assert!(
            timeline <= SHORT_TRANSPORT_HEIGHT + SHORT_TIMELINE_ROOM + f32::from(PANEL_GAP) + 1.0,
            "{timeline}"
        );
        assert!(
            editor.stage_rect.height() >= 100.0,
            "{:?}",
            editor.stage_rect
        );
    }

    #[test]
    fn a_short_window_keeps_the_timeline_height_for_when_it_is_tall_again() {
        let ctx = context();
        SCREEN.set([1280, 800]);
        let mut editor = editor_with_a_selected_clip(&ctx);
        let tall = editor.timeline_panel_rect.height();

        SCREEN.set([1280, 420]);
        settle_animations(&ctx, &mut editor);
        assert!(editor.timeline_panel_rect.height() < tall);
        SCREEN.set([1280, 800]);
        settle_animations(&ctx, &mut editor);

        assert!((editor.timeline_panel_rect.height() - tall).abs() < 1.0);
    }

    #[test]
    fn settings_fit_in_the_smallest_window() {
        SCREEN.set([640, 340]);
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        editor.settings_open = true;
        settle_animations(&ctx, &mut editor);

        for name in ["General settings", "Recording settings", "Keybinds"] {
            let shown = named_nodes(&ctx, &mut editor)
                .into_iter()
                .find(|(label, ..)| label.starts_with(name))
                .unwrap_or_else(|| panic!("{name} is missing"))
                .1;
            assert!(screen().contains_rect(shown), "{name} {shown:?}");
        }
    }

    #[test]
    fn an_empty_projects_hint_fits_on_the_stage_at_every_size() {
        for size in WINDOW_SIZES {
            SCREEN.set(size);
            let ctx = context();
            let mut editor = Editor::new(&ctx, false);
            settle_animations(&ctx, &mut editor);
            let output = run_frame(&ctx, &mut editor, vec![]);
            let hint = text_rect(&output, "Add a video to get started")
                .unwrap_or_else(|| panic!("the hint is missing at {size:?}"));

            assert!(
                editor.stage_rect.contains_rect(hint),
                "{hint:?} runs off the stage {:?} at {size:?}",
                editor.stage_rect
            );
        }
    }

    #[test]
    fn whole_seconds_drop_the_hundredths_only() {
        assert_eq!(whole_seconds("0:04.25"), "0:04");
        assert_eq!(whole_seconds("1:02:03.99"), "1:02:03");
        assert_eq!(whole_seconds("0:04"), "0:04");
    }

    #[test]
    fn the_window_never_goes_below_a_quarter_of_a_1080p_screen_at_150_percent() {
        let quarter = [1920.0 / 1.5 / 2.0, 1020.0 / 1.5 / 2.0];
        assert!(SMALLEST_WINDOW[0] <= quarter[0] && SMALLEST_WINDOW[1] <= quarter[1]);
    }

    /// Saves pictures of the editor at half, a quarter and the smallest size, with the side
    /// panels shut and open, in the folder named by `BARECLIP_SCREENS`.
    #[test]
    #[ignore = "needs a GPU, and writes pictures to look at"]
    fn draw_the_small_window_screens() {
        let folder = std::env::var_os("BARECLIP_SCREENS").map_or_else(
            || std::env::temp_dir().join("bareclip-screens"),
            PathBuf::from,
        );
        for size in WINDOW_SIZES {
            SCREEN.set(size);
            let [width, height] = size;
            let ctx = context();
            let mut camera = Camera::new(folder.clone());
            let mut editor = editor_with_a_selected_clip(&ctx);
            camera.shoot(&ctx, &mut editor, vec![], &format!("{width}x{height}-shut"));
            click_named(&ctx, &mut editor, "My media");
            settle_animations(&ctx, &mut editor);
            camera.shoot(
                &ctx,
                &mut editor,
                vec![],
                &format!("{width}x{height}-my-media"),
            );
            click_named(&ctx, &mut editor, "Captions");
            settle_animations(&ctx, &mut editor);
            camera.shoot(
                &ctx,
                &mut editor,
                vec![],
                &format!("{width}x{height}-captions"),
            );
            editor.right_panel = None;
            editor.settings_open = true;
            settle_animations(&ctx, &mut editor);
            camera.shoot(
                &ctx,
                &mut editor,
                vec![],
                &format!("{width}x{height}-settings"),
            );
        }
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
                report: ProjectReport::Fast(bareclip_media::ExportReport {
                    mode_requested: CutMode::FrameExact,
                    mode_used: CutMode::FrameExact,
                    sections: 2,
                    copied_frames: 200,
                    encoded_frames: 12,
                    black_frames: 0,
                    encoder: Some("h264_nvenc".to_owned()),
                    audio_tracks: 2,
                    elapsed: Duration::from_millis(2100),
                }),
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
                    name: None,
                    recorded: false,
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

        // Layers, drawn by the real compositor and shown by the real renderer.
        let mut layered = stacked_clips(&ctx);
        layered.use_gpu(LayerDisplay::new(
            camera.device.clone(),
            camera.queue.clone(),
            camera.renderer.clone(),
        ));
        let (top, _) = top_and_below(&layered);
        layered.project.tracks.set_placement(
            top,
            Placement {
                center_x: WHOLE * 3 / 10,
                center_y: WHOLE * 3 / 10,
                scale: WHOLE * 2 / 5,
                ..Placement::default()
            },
        );
        layered.seek(Ticks::from_seconds(1.0));
        wait_for_layers(&ctx, &mut layered);
        layered.selected = None;
        camera.shoot(&ctx, &mut layered, moved(pos2(1100.0, 80.0)), "60-layers");
        layered.selected = Some(top);
        run_frame(&ctx, &mut layered, vec![]);
        camera.shoot(
            &ctx,
            &mut layered,
            moved(pos2(1100.0, 80.0)),
            "61-layers-selected",
        );
        let frame = layered.stage_state.frame.expect("the stage was drawn");
        let on_clip = frame.left_top() + frame.size() * 0.3;
        begin_drag(&ctx, &mut layered, on_clip, frame.center());
        camera.shoot(&ctx, &mut layered, vec![], "62-layers-snapped-to-centre");
        end_drag(&ctx, &mut layered, frame.center());
        layered.project.tracks.set_placement(
            top,
            Placement {
                center_x: WHOLE / 2,
                center_y: WHOLE / 2,
                scale: WHOLE,
                opacity: WHOLE / 2,
                ..Placement::default()
            },
        );
        wait_for_layers(&ctx, &mut layered);
        camera.shoot(
            &ctx,
            &mut layered,
            moved(pos2(1100.0, 80.0)),
            "63-layers-see-through",
        );
        ctx.set_theme(egui::Theme::Light);
        layered.project.tracks.set_placement(
            top,
            Placement {
                center_x: WHOLE,
                center_y: WHOLE / 8,
                scale: WHOLE / 2,
                opacity: WHOLE,
                ..Placement::default()
            },
        );
        wait_for_layers(&ctx, &mut layered);
        camera.shoot(
            &ctx,
            &mut layered,
            moved(pos2(1100.0, 80.0)),
            "64-light-layers-off-the-edge",
        );
        ctx.set_theme(egui::Theme::Dark);

        // Turned and flipped.
        layered.project.tracks.set_placement(
            top,
            Placement {
                center_x: WHOLE * 2 / 5,
                center_y: WHOLE / 2,
                scale: WHOLE / 2,
                rotation: 30_000,
                flip_x: true,
                ..Placement::default()
            },
        );
        wait_for_layers(&ctx, &mut layered);
        camera.shoot(
            &ctx,
            &mut layered,
            moved(pos2(1100.0, 80.0)),
            "68-turned-and-flipped",
        );

        // A coloured background, around a resized clip and with its menu open.
        layered.project.canvas.background = [255, 148, 34];
        layered.project.tracks.remove(top_and_below(&layered).1);
        // Cropping, with the part being cut dimmed.
        layered.project.tracks.set_placement(
            top,
            Placement {
                center_x: WHOLE / 2,
                center_y: WHOLE / 2,
                scale: WHOLE * 7 / 10,
                crop_left: WHOLE / 5,
                crop_bottom: WHOLE * 3 / 10,
                ..Placement::default()
            },
        );
        layered.selected = Some(top);
        wait_for_layers(&ctx, &mut layered);
        camera.shoot(&ctx, &mut layered, moved(pos2(1100.0, 80.0)), "72-cropped");
        click_named(&ctx, &mut layered, "Crop");
        wait_for_layers(&ctx, &mut layered);
        camera.shoot(&ctx, &mut layered, moved(pos2(1100.0, 80.0)), "73-cropping");
        click_named(&ctx, &mut layered, "Finish cropping");
        layered.project.tracks.set_placement(
            top,
            Placement {
                center_x: WHOLE * 2 / 5,
                center_y: WHOLE / 2,
                scale: WHOLE / 2,
                rotation: 30_000,
                flip_x: true,
                ..Placement::default()
            },
        );
        wait_for_layers(&ctx, &mut layered);
        camera.shoot(
            &ctx,
            &mut layered,
            moved(pos2(1100.0, 80.0)),
            "69-background",
        );
        click_named(&ctx, &mut layered, "Background colour");
        wait_for_layers(&ctx, &mut layered);
        camera.shoot(
            &ctx,
            &mut layered,
            moved(pos2(1100.0, 80.0)),
            "70-background-menu",
        );
        run_frame(
            &ctx,
            &mut layered,
            vec![egui::Event::Key {
                key: Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
        );
        run_frame(&ctx, &mut layered, vec![]);
        click_named(&ctx, &mut layered, "More");
        wait_for_layers(&ctx, &mut layered);
        camera.shoot(
            &ctx,
            &mut layered,
            moved(pos2(1100.0, 80.0)),
            "71-more-menu",
        );

        // The project's shape, on a tall canvas and with its menu open.
        let mut tall = with_videos(&ctx, &[test_clip()]);
        tall.use_gpu(LayerDisplay::new(
            camera.device.clone(),
            camera.queue.clone(),
            camera.renderer.clone(),
        ));
        wait_for_layers(&ctx, &mut tall);
        pick_size(&ctx, &mut tall, "9:16");
        wait_for_layers(&ctx, &mut tall);
        camera.shoot(
            &ctx,
            &mut tall,
            moved(pos2(1100.0, 80.0)),
            "65-tall-project",
        );
        let button = named_nodes(&ctx, &mut tall)
            .into_iter()
            .find(|(label, ..)| label.starts_with("Size:"))
            .expect("the Size button is there")
            .1;
        click(&ctx, &mut tall, button.center());
        wait_for_layers(&ctx, &mut tall);
        camera.shoot(&ctx, &mut tall, moved(button.center()), "66-size-menu");
        let mut empty_shape = Editor::new(&ctx, false);
        run_frame(&ctx, &mut empty_shape, vec![]);
        camera.shoot(&ctx, &mut empty_shape, vec![], "67-empty-project-shape");
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

    /// Every piece of text a frame drew inside a part of the screen.
    fn text_in(output: &egui::FullOutput, area: egui::Rect) -> Vec<String> {
        fn gather(shape: &egui::Shape, area: egui::Rect, found: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) if area.contains(text.pos) => {
                    found.push(text.galley.text().to_owned());
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        gather(shape, area, found);
                    }
                }
                _ => {}
            }
        }
        let mut found = Vec::new();
        for clipped in &output.shapes {
            gather(&clipped.shape, area, &mut found);
        }
        found
    }

    /// Where this exact text is drawn, if it is.
    fn text_rect(output: &egui::FullOutput, wanted: &str) -> Option<egui::Rect> {
        fn find(shape: &egui::Shape, wanted: &str) -> Option<egui::Rect> {
            match shape {
                egui::Shape::Text(text) if text.galley.text() == wanted => {
                    Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, wanted)),
                _ => None,
            }
        }
        output
            .shapes
            .iter()
            .find_map(|clipped| find(&clipped.shape, wanted))
    }

    /// Runs enough frames for everything that animates to finish, so a frame asks for no more.
    fn settle_animations(ctx: &egui::Context, editor: &mut Editor) {
        for _ in 0..40 {
            run_frame(ctx, editor, vec![]);
        }
    }

    /// Where the control with this name is, so a test presses it the way a person would.
    fn named_rect(ctx: &egui::Context, editor: &mut Editor, name: &str) -> egui::Rect {
        named_nodes(ctx, editor)
            .into_iter()
            .find(|(label, ..)| label == name)
            .unwrap_or_else(|| panic!("nothing is named {name}"))
            .1
    }

    fn has_named(ctx: &egui::Context, editor: &mut Editor, name: &str) -> bool {
        named_nodes(ctx, editor)
            .iter()
            .any(|(label, ..)| label == name)
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
        assert!(!names.iter().any(|(label, ..)| label == "Add media"));
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

    fn ctrl_click(ctx: &egui::Context, editor: &mut Editor, at: egui::Pos2) {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::CTRL,
        };
        run_frame(ctx, editor, vec![egui::Event::PointerMoved(at)]);
        let held = egui::Event::ModifiersChanged(Modifiers::CTRL);
        run_frame(ctx, editor, vec![held, button(true)]);
        run_frame(ctx, editor, vec![button(false)]);
        let released = egui::Event::ModifiersChanged(Modifiers::NONE);
        run_frame(ctx, editor, vec![released]);
    }

    fn card_box_named(editor: &Editor, index: usize) -> String {
        format!("Select {}", editor.media[index].name)
    }

    #[test]
    fn cards_have_no_boxes_until_something_is_ticked() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip(), second_clip()]);
        let name = card_box_named(&editor, 0);
        assert!(!is_named(&ctx, &mut editor, &name));

        let (_, thumbnail) = editor.card_thumbnails[0];
        run_frame(&ctx, &mut editor, moved(thumbnail.center()));
        assert!(!is_named(&ctx, &mut editor, &name), "not on hover either");
        // Where the box would be does nothing while it isn't there.
        click(&ctx, &mut editor, thumbnail.min + vec2(12.0, 12.0));
        assert!(editor.selected_media.is_empty());

        click_named(&ctx, &mut editor, "Select all");
        assert!(is_named(&ctx, &mut editor, &name));

        let ids: Vec<MediaId> = editor.media.iter().map(|entry| entry.id).collect();
        for id in ids {
            editor.toggle_media(id);
        }
        assert!(!is_named(&ctx, &mut editor, &name), "nothing is ticked now");
    }

    #[test]
    fn the_box_above_says_select_all_with_no_words_beside_it_until_some_are_ticked() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip(), second_clip()]);
        let area = editor.media_panel_rect;
        let output = run_frame(&ctx, &mut editor, vec![]);
        assert!(
            !text_in(&output, area)
                .iter()
                .any(|text| text == "Select all")
        );
        click_named(&ctx, &mut editor, "Select all");
        let output = run_frame(&ctx, &mut editor, vec![]);
        assert!(
            text_in(&output, area)
                .iter()
                .any(|text| text == "2 selected")
        );
    }

    #[test]
    fn a_part_filled_box_ticks_the_rest() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip(), second_clip()]);
        let ids: Vec<MediaId> = editor.media.iter().map(|entry| entry.id).collect();
        editor.toggle_media(ids[1]);
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Select all");
        assert_eq!(editor.selected_media, [ids[1], ids[0]]);
    }

    #[test]
    fn ctrl_click_ticks_a_card_and_numbers_them_in_that_order() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip(), second_clip()]);
        let ids: Vec<MediaId> = editor.media.iter().map(|entry| entry.id).collect();
        let middles: Vec<egui::Pos2> = editor
            .card_thumbnails
            .iter()
            .map(|(_, thumbnail)| thumbnail.center())
            .collect();

        ctrl_click(&ctx, &mut editor, middles[1]);
        ctrl_click(&ctx, &mut editor, middles[0]);
        assert_eq!(editor.selected_media, [ids[1], ids[0]]);
        assert_eq!(editor.card_state(ids[1]).order, Some(1));
        assert_eq!(editor.card_state(ids[0]).order, Some(2));

        ctrl_click(&ctx, &mut editor, middles[1]);
        assert_eq!(editor.selected_media, [ids[0]]);
        assert_eq!(editor.card_state(ids[0]).order, Some(1));
    }

    #[test]
    fn a_plain_click_on_a_card_ticks_nothing() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip()]);
        let (_, thumbnail) = editor.card_thumbnails[0];
        click(&ctx, &mut editor, thumbnail.center());
        assert!(editor.selected_media.is_empty());
    }

    #[test]
    fn a_ticked_cards_box_unticks_it() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip(), second_clip()]);
        let ids: Vec<MediaId> = editor.media.iter().map(|entry| entry.id).collect();
        click_named(&ctx, &mut editor, "Select all");
        let name = card_box_named(&editor, 0);
        click_named(&ctx, &mut editor, &name);
        assert_eq!(editor.selected_media, [ids[1]]);
    }

    // Renaming.

    #[test]
    fn a_new_name_shows_everywhere_and_leaves_the_file_alone() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let id = editor.media[0].id;
        let path = editor.media[0].info.path.clone();
        editor.rename_media(id, "  Boss fight ");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.media[0].name, "Boss fight");
        assert_eq!(editor.media[0].info.path, path);
        assert!(path.exists());
        assert_eq!(
            editor.current_file().media[0].path.file_name(),
            path.file_name()
        );
    }

    #[test]
    fn renaming_is_one_undo_step() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let (id, original) = (editor.media[0].id, editor.media[0].name.clone());
        editor.rename_media(id, "Boss fight");
        editor.undo();
        assert_eq!(editor.media[0].name, original);
        editor.redo();
        assert_eq!(editor.media[0].name, "Boss fight");
    }

    #[test]
    fn nothing_typed_or_the_files_own_name_goes_back_to_the_file() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let (id, original) = (editor.media[0].id, editor.media[0].name.clone());
        editor.rename_media(id, "Boss fight");
        editor.rename_media(id, "   ");
        assert_eq!(editor.media[0].name, original);
        assert!(editor.project.names.is_empty());
        editor.rename_media(id, "Boss fight");
        editor.rename_media(id, &original);
        assert!(editor.project.names.is_empty());
    }

    #[test]
    fn typing_the_same_name_makes_no_undo_step() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let id = editor.media[0].id;
        editor.rename_media(id, "Boss fight");
        let before = editor.project.clone();
        editor.rename_media(id, "Boss fight");
        editor.undo();
        assert_ne!(editor.project, before, "the undo went back past the rename");
    }

    #[test]
    fn a_name_is_saved_with_the_project_and_comes_back() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let id = editor.media[0].id;
        editor.rename_media(id, "Boss fight");
        assert!(editor.has_unsaved_changes());
        let file = editor.current_file();
        assert_eq!(file.media[0].name.as_deref(), Some("Boss fight"));

        let mut opened = Editor::new(&ctx, false);
        run_frame(&ctx, &mut opened, vec![]);
        opened.begin_loading(&ctx, file, None, None);
        settle(&ctx, &mut opened);
        run_frame(&ctx, &mut opened, vec![]);
        assert_eq!(opened.media[0].name, "Boss fight");
        assert!(!opened.has_unsaved_changes());
    }

    #[test]
    fn a_video_with_no_new_name_saves_nothing_extra() {
        let ctx = context();
        let editor = with_videos(&ctx, &[test_clip()]);
        let file = editor.current_file();
        let written = serde_json::to_string(&file.media[0]).expect("it can be written");
        assert!(!written.contains("\"name\""), "{written}");
    }

    #[test]
    fn the_cards_menu_renames_and_removes() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip()]);
        click_named(&ctx, &mut editor, "Video options");
        let_dialog_open(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Rename");
        let_dialog_open(&ctx, &mut editor);
        assert!(is_named(&ctx, &mut editor, "Video name"));
        run_frame(&ctx, &mut editor, press(Key::End));
        run_frame(&ctx, &mut editor, typed("!"));
        run_frame(&ctx, &mut editor, press(Key::Enter));
        run_frame(&ctx, &mut editor, vec![]);
        assert!(
            editor.media[0].name.ends_with('!'),
            "{}",
            editor.media[0].name
        );
        assert!(!is_named(&ctx, &mut editor, "Video name"));

        click_named(&ctx, &mut editor, "Video options");
        let_dialog_open(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Remove from this project");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.visible_media().count(), 0);
    }

    #[test]
    fn escape_leaves_the_name_as_it_was() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip()]);
        let original = editor.media[0].name.clone();
        editor.media_rename = Some((editor.media[0].id, original.clone()));
        let_dialog_open(&ctx, &mut editor);
        run_frame(&ctx, &mut editor, typed("zzz"));
        run_frame(&ctx, &mut editor, press(Key::Escape));
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.media[0].name, original);
        assert!(editor.media_rename.is_none());
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
        assert_eq!(
            editor.visible_media().count(),
            0,
            "removing can't be undone"
        );
    }

    #[test]
    fn removing_ticked_videos_in_use_asks_first_and_cant_be_undone() {
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
        assert_eq!(clip_count(&editor), 0);
        assert_eq!(editor.visible_media().count(), 0);
        assert!(
            !editor.history.can_undo(),
            "only those videos were ever added"
        );
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
    fn trimming_shows_the_clips_length_and_where_it_starts_and_ends() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let scale = editor.timeline_state.scale;
        let y = editor.timeline_state.track.center().y;
        let from = pos2(scale.x_of(Ticks::from_seconds(4.0)) - 4.0, y);
        let to = pos2(scale.x_of(Ticks::from_seconds(3.0)), y);

        begin_drag(&ctx, &mut editor, from, to);
        run_frame(&ctx, &mut editor, vec![]);
        let output = run_frame(&ctx, &mut editor, vec![]);

        let clip = editor.project.tracks.played().clips()[0];
        let on_ruler = text_in(&output, editor.timeline_state.ruler);
        assert!(on_ruler.contains(&text::clip_length(clip.duration())));
        let (start, end, edge) = editor.timeline_state.trim_readout.expect("it is showing");
        assert_eq!((start, end), (clip.start, clip.end()));
        assert_eq!(edge, ClipEdge::End);
        for time in [start, end] {
            let whole = text::nearest_second(time);
            assert!(on_ruler.contains(&whole), "{whole} isn't in {on_ruler:?}");
        }

        end_drag(&ctx, &mut editor, to);
        assert_eq!(editor.timeline_state.trim_readout, None);
    }

    #[test]
    fn trimming_the_start_lights_up_the_start() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let scale = editor.timeline_state.scale;
        let y = editor.timeline_state.track.center().y;
        let from = pos2(scale.x_of(Ticks(0)) + 4.0, y);
        let to = pos2(scale.x_of(Ticks::from_seconds(1.0)), y);

        begin_drag(&ctx, &mut editor, from, to);
        run_frame(&ctx, &mut editor, vec![]);

        let (.., edge) = editor.timeline_state.trim_readout.expect("it is showing");
        assert_eq!(edge, ClipEdge::Start);
        end_drag(&ctx, &mut editor, to);
    }

    #[test]
    fn moving_a_clip_or_only_selecting_it_doesnt_show_where_it_starts_and_ends() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.selected = Some(editor.project.tracks.played().clips()[0].id);
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.timeline_state.trim_readout, None);

        let scale = editor.timeline_state.scale;
        let y = editor.timeline_state.track.center().y;
        let from = pos2(scale.x_of(Ticks::from_seconds(2.0)), y);
        let to = pos2(scale.x_of(Ticks::from_seconds(3.0)), y);
        begin_drag(&ctx, &mut editor, from, to);
        run_frame(&ctx, &mut editor, vec![]);

        assert_eq!(editor.timeline_state.trim_readout, None);
        end_drag(&ctx, &mut editor, to);
    }

    #[test]
    fn the_time_has_no_hours_until_it_reaches_one() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);

        let output = run_frame(&ctx, &mut editor, vec![]);

        let everything = egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 800.0));
        let drawn = text_in(&output, everything);
        assert!(drawn.contains(&"0:00.00".to_owned()), "{drawn:?}");
        assert!(
            !drawn.iter().any(|text| text.contains("00:00:")),
            "{drawn:?}"
        );
    }

    #[test]
    fn a_videos_name_shows_only_after_the_pointer_rests_on_it_and_fades_away_after() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let name = editor.media[0].name.clone();
        let area = editor.timeline_state.layout.area;
        let output = run_frame(&ctx, &mut editor, vec![]);
        assert!(!text_in(&output, area).contains(&name), "not at rest");

        let scale = editor.timeline_state.scale;
        let over = pos2(
            scale.x_of(Ticks::from_seconds(1.0)),
            editor.timeline_state.track.center().y,
        );
        run_frame(&ctx, &mut editor, moved(over));
        for _ in 0..30 {
            run_frame(&ctx, &mut editor, vec![]);
        }
        assert_eq!(editor.timeline_state.named_clip, None, "half a second in");

        let mut output = run_frame(&ctx, &mut editor, vec![]);
        for _ in 0..45 {
            output = run_frame(&ctx, &mut editor, vec![]);
        }
        let clip = editor.project.tracks.played().clips()[0].id;
        assert_eq!(editor.timeline_state.named_clip, Some(clip));
        assert!(text_in(&output, area).contains(&name));

        run_frame(&ctx, &mut editor, moved(pos2(640.0, 100.0)));
        assert_eq!(editor.timeline_state.named_clip, None);
        for _ in 0..30 {
            output = run_frame(&ctx, &mut editor, vec![]);
        }
        assert!(!text_in(&output, area).contains(&name), "it has gone again");
    }

    #[test]
    fn resting_on_a_video_asks_for_a_frame_when_the_wait_ends_rather_than_every_frame() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        wait_for_strip(&ctx, &mut editor);
        settle_animations(&ctx, &mut editor);
        let scale = editor.timeline_state.scale;
        let over = pos2(
            scale.x_of(Ticks::from_seconds(1.0)),
            editor.timeline_state.track.center().y,
        );

        let wait_after = |output: &egui::FullOutput| {
            output
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .expect("the window is there")
                .repaint_delay
        };
        let resting = run_frame(&ctx, &mut editor, vec![]);
        assert!(
            wait_after(&resting) > Duration::from_secs(3600),
            "it stays idle"
        );

        run_frame(&ctx, &mut editor, moved(over));
        // A pointer moving always asks for one more frame, whatever is under it.
        run_frame(&ctx, &mut editor, vec![]);
        let output = run_frame(&ctx, &mut editor, vec![]);

        let wait = wait_after(&output);
        assert!(wait > Duration::from_millis(500), "{wait:?}");
        assert!(wait <= Duration::from_secs(1), "{wait:?}");
    }

    #[test]
    fn sound_and_images_keep_their_names_on_them() {
        let ctx = context();
        let image = image_file("named", "800x600", "orange");
        let mut editor = with_videos(&ctx, &[image.as_path(), song_file()]);
        let names: Vec<String> = editor
            .media
            .iter()
            .map(|entry| entry.name.clone())
            .collect();

        let output = run_frame(&ctx, &mut editor, vec![]);

        let on_tracks = text_in(&output, editor.timeline_state.layout.area);
        for name in names {
            assert!(on_tracks.contains(&name), "{name} is not in {on_tracks:?}");
        }
    }

    #[test]
    fn a_clip_at_another_speed_says_so_with_no_need_to_point_at_it() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let clip = editor.project.tracks.played().clips()[0].id;
        let area = editor.timeline_state.layout.area;
        let output = run_frame(&ctx, &mut editor, vec![]);
        assert!(!text_in(&output, area).contains(&"1x".to_owned()));

        editor
            .project
            .tracks
            .set_speed(clip, Speed::default().with_factor(2.0));
        let output = run_frame(&ctx, &mut editor, vec![]);

        assert!(text_in(&output, area).contains(&"2x".to_owned()));
    }

    #[test]
    fn the_title_strip_is_tall_enough_for_a_name_box_the_size_of_the_logos_text() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);

        let label = text::project_title(project::DEFAULT_NAME, editor.has_unsaved_changes());
        let name = named_rect(&ctx, &mut editor, &label);

        assert_eq!(name.height(), components::NAME_BAR_HEIGHT);
        assert!(name.top() >= 0.0);
        assert!(name.bottom() <= TITLE_STRIP_HEIGHT);
    }

    /// Right-clicks the first clip, ready to use its menu.
    fn open_clip_menu(ctx: &egui::Context, editor: &mut Editor) -> ClipId {
        let clip = editor.project.tracks.tracks()[0].clips()[0];
        let at = pos2(
            editor.timeline_state.scale.x_of(Ticks::from_seconds(1.0)),
            editor.timeline_state.track.center().y,
        );
        secondary_click(ctx, editor, at);
        let_dialog_open(ctx, editor);
        clip.id
    }

    /// Points at a line of the menu, which opens what is beside it. The toolbar over the picture
    /// has a Speed button too, and the menu's is the lower one.
    fn open_flyout(ctx: &egui::Context, editor: &mut Editor, name: &str) {
        // The rail at the window's right edge has tools with the same names.
        let rail = ctx.content_rect().right() - RAIL_WIDTH;
        let line = named_nodes(ctx, editor)
            .into_iter()
            .filter(|(label, rect, _)| label == name && rect.right() <= rail)
            .map(|(_, rect, _)| rect)
            .max_by(|a, b| a.top().total_cmp(&b.top()))
            .unwrap_or_else(|| panic!("nothing is named {name}"));
        run_frame(ctx, editor, moved(line.center()));
        let_dialog_open(ctx, editor);
    }

    fn type_and_press(ctx: &egui::Context, editor: &mut Editor, typed: &str, key: Key) {
        run_frame(ctx, editor, vec![egui::Event::Text(typed.to_owned())]);
        run_frame(
            ctx,
            editor,
            vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
        );
        run_frame(ctx, editor, vec![]);
        run_frame(ctx, editor, vec![]);
    }

    #[test]
    fn the_clips_menu_changes_its_speed_stays_open_and_is_one_undo_step() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        let id = open_clip_menu(&ctx, &mut editor);
        let before = picture_clip(&editor);
        open_flyout(&ctx, &mut editor, "Speed");

        click_named(&ctx, &mut editor, "2x");

        assert_eq!(picture_clip(&editor).speed, times(2.0));
        assert_eq!(picture_clip(&editor).id, id);
        assert_eq!(
            editor.timeline_state.menu_target,
            Some(timeline::MenuTarget::Clip(id)),
            "a press beside the menu leaves it open"
        );
        editor.undo();
        assert_eq!(picture_clip(&editor).speed, before.speed);
    }

    #[test]
    fn the_box_in_the_clips_menu_takes_a_finer_speed_than_the_slider() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        open_clip_menu(&ctx, &mut editor);
        open_flyout(&ctx, &mut editor, "Speed");

        click_named(&ctx, &mut editor, "Speed value");
        type_and_press(&ctx, &mut editor, "0.75", Key::Enter);

        assert_eq!(picture_clip(&editor).speed, times(0.75));
        editor.undo();
        assert_eq!(picture_clip(&editor).speed, Speed::default());
    }

    #[test]
    fn clicking_away_from_the_speed_box_uses_what_was_typed() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        open_clip_menu(&ctx, &mut editor);
        open_flyout(&ctx, &mut editor, "Speed");

        click_named(&ctx, &mut editor, "Speed value");
        run_frame(&ctx, &mut editor, vec![egui::Event::Text("3".to_owned())]);
        click(&ctx, &mut editor, pos2(640.0, 100.0));

        assert_eq!(picture_clip(&editor).speed, times(3.0));
        assert_eq!(editor.timeline_state.menu_target, None);
    }

    #[test]
    fn escape_leaves_the_speed_as_it_was_and_nonsense_changes_nothing() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        open_clip_menu(&ctx, &mut editor);
        open_flyout(&ctx, &mut editor, "Speed");

        click_named(&ctx, &mut editor, "Speed value");
        type_and_press(&ctx, &mut editor, "fast", Key::Enter);
        assert_eq!(picture_clip(&editor).speed, Speed::default());

        click_named(&ctx, &mut editor, "Speed value");
        type_and_press(&ctx, &mut editor, "4", Key::Escape);
        assert_eq!(picture_clip(&editor).speed, Speed::default());
    }

    #[test]
    fn typing_in_the_speed_box_doesnt_set_off_shortcuts() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.seek(Ticks::from_seconds(1.0));
        run_frame(&ctx, &mut editor, vec![]);
        open_clip_menu(&ctx, &mut editor);
        open_flyout(&ctx, &mut editor, "Speed");
        click_named(&ctx, &mut editor, "Speed value");
        let clips = editor.project.tracks.tracks()[0].clips().len();

        type_and_press(&ctx, &mut editor, "s", Key::S);

        assert_eq!(editor.project.tracks.tracks()[0].clips().len(), clips);
    }

    #[test]
    fn dragging_the_speed_slider_in_the_menu_lands_on_tenths_past_the_menu_and_is_one_undo_step() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        open_clip_menu(&ctx, &mut editor);
        open_flyout(&ctx, &mut editor, "Speed");
        let slider = named_rect(&ctx, &mut editor, "Clip speed");

        // Far enough left to pass over the menu's own lines.
        let to = pos2(slider.left() - 200.0, slider.center().y);
        begin_drag(&ctx, &mut editor, slider.center(), to);
        assert!(is_named(&ctx, &mut editor, "Clip speed"), "it stays open");
        assert_eq!(picture_clip(&editor).speed, times(0.1));

        let partway = pos2(slider.center().x + 7.0, slider.center().y);
        run_frame(&ctx, &mut editor, moved(partway));
        run_frame(&ctx, &mut editor, vec![]);
        let factor = picture_clip(&editor).speed.as_factor();
        assert!(
            ((factor * 10.0).round() - factor * 10.0).abs() < 1e-6,
            "{factor}"
        );

        end_drag(&ctx, &mut editor, partway);
        assert!(is_named(&ctx, &mut editor, "Clip speed"));
        editor.undo();
        assert_eq!(picture_clip(&editor).speed, Speed::default());
    }

    #[test]
    fn the_clips_menu_mutes_from_audio_staying_open_and_detaching_closes_it() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        let id = open_clip_menu(&ctx, &mut editor);
        open_flyout(&ctx, &mut editor, "Audio");

        click_menu_item(&ctx, &mut editor, "Mute clip");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(picture_clip(&editor).sound.muted);
        assert!(editor.timeline_state.menu_target.is_some());
        click_menu_item(&ctx, &mut editor, "Unmute clip");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(!picture_clip(&editor).sound.muted);

        click_menu_item(&ctx, &mut editor, "Detach sound");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.timeline_state.menu_target, None);
        assert_eq!(editor.project.tracks.audio_tracks().len(), 1);
        assert_eq!(
            editor.project.tracks.clip(id).map(|clip| clip.sound.muted),
            Some(true)
        );
    }

    #[test]
    fn the_clips_menu_has_volume_and_fades_and_a_change_is_one_undo_step() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        open_clip_menu(&ctx, &mut editor);
        open_flyout(&ctx, &mut editor, "Audio");
        for line in [
            "Volume",
            "Fade in",
            "Fade out",
            "Mute clip",
            "Noise suppression",
        ] {
            assert!(is_named(&ctx, &mut editor, line), "{line}");
        }

        let slider = named_nodes(&ctx, &mut editor)
            .into_iter()
            .find(|(label, ..)| label == "Volume")
            .expect("the slider is there")
            .1;
        click(
            &ctx,
            &mut editor,
            pos2(slider.left() + slider.width() * 0.25, slider.center().y),
        );
        run_frame(&ctx, &mut editor, vec![]);

        assert!(picture_clip(&editor).sound.volume < bareclip_core::sound::FULL_VOLUME);
        assert!(editor.timeline_state.menu_target.is_some(), "it stays open");
        editor.undo();
        assert_eq!(
            picture_clip(&editor).sound.volume,
            bareclip_core::sound::FULL_VOLUME
        );
    }

    #[test]
    fn the_clips_menu_arranges_a_clip_with_everything_on_the_toolbar_and_in_more() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let id = open_clip_menu(&ctx, &mut editor);
        open_flyout(&ctx, &mut editor, "Arrange");
        for line in [
            "Crop",
            "Rotate by 90 degrees",
            "Flip left to right",
            "Flip top to bottom",
            "Opacity",
            "Picture in picture",
            "Order",
            "Reset size and place",
        ] {
            assert!(is_named(&ctx, &mut editor, line), "{line}");
        }
        assert!(
            !is_named(&ctx, &mut editor, "Top left"),
            "corners wait beside"
        );
        open_side_menu(&ctx, &mut editor, "Order", "Send to back");
        assert!(is_named(&ctx, &mut editor, "Bring to front"));
        open_side_menu(&ctx, &mut editor, "Picture in picture", "Bottom right");
        click_menu_item(&ctx, &mut editor, "Bottom right");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(placement_of(&editor, id).scale < WHOLE / 2);
        assert!(
            editor.timeline_state.menu_target.is_some(),
            "a pick beside Arrange is inside the menu"
        );
        editor.undo();

        click_menu_item(&ctx, &mut editor, "Flip left to right");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(placement_of(&editor, id).flip_x);
        assert!(editor.timeline_state.menu_target.is_some(), "it stays open");
        click_menu_item(&ctx, &mut editor, "Rotate by 90 degrees");
        run_frame(&ctx, &mut editor, vec![]);
        assert_ne!(placement_of(&editor, id).rotation, 0);

        click_menu_item(&ctx, &mut editor, "Reset size and place");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(placement_of(&editor, id), Placement::default());
        editor.undo();
        assert_ne!(placement_of(&editor, id).rotation, 0);
    }

    #[test]
    fn crop_on_the_clips_menu_starts_cropping_and_waits_for_the_playhead() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let id = open_clip_menu(&ctx, &mut editor);
        open_flyout(&ctx, &mut editor, "Arrange");
        click_menu_item(&ctx, &mut editor, "Crop");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.stage_state.cropping(), Some(id));
        assert_eq!(editor.selected, Some(id));
        assert_eq!(editor.timeline_state.menu_target, None);

        let mut away = with_videos(&ctx, &[test_clip()]);
        away.seek(away.project.tracks.duration());
        run_frame(&ctx, &mut away, vec![]);
        open_clip_menu(&ctx, &mut away);
        open_flyout(&ctx, &mut away, "Arrange");
        let (_, disabled) = menu_item(&ctx, &mut away, "Crop").expect("it is listed");
        assert!(disabled, "the clip isn't on the picture");
    }

    #[test]
    fn the_clips_menu_opens_the_panel_on_each_look_tool() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.selected = None;
        for (line, tab) in [
            ("Filters", LookTab::Filters),
            ("Effects", LookTab::Effects),
            ("Adjust colours", LookTab::Colours),
            ("Fade", LookTab::Fade),
        ] {
            let id = open_clip_menu(&ctx, &mut editor);
            click_menu_item(&ctx, &mut editor, line);
            let_dialog_open(&ctx, &mut editor);
            assert_eq!(editor.selected, Some(id));
            assert!(editor.looking_at().is_some(), "{line}");
            assert_eq!(editor.right_panel, Some(RightPanel::Tool(tab)));
            assert_eq!(editor.timeline_state.menu_target, None);
        }
    }

    #[test]
    fn a_sound_clip_has_no_picture_tools_on_its_menu() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[song_file()]);
        open_clip_menu(&ctx, &mut editor);
        for line in ["Arrange", "Filters", "Effects", "Adjust colours"] {
            assert!(menu_item(&ctx, &mut editor, line).is_none(), "{line}");
        }
        assert!(menu_item(&ctx, &mut editor, "Audio").is_some());
    }

    #[test]
    fn a_title_only_has_colours_and_fade_and_arranges_at_the_playhead() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        let title = with_a_title(&ctx, &mut editor, TitleStyle::Plain);

        right_click_menu_on(&ctx, &mut editor, title);
        assert!(menu_item(&ctx, &mut editor, "Filters").is_none());
        assert!(menu_item(&ctx, &mut editor, "Effects").is_none());
        let (_, disabled) = menu_item(&ctx, &mut editor, "Arrange").expect("it is listed");
        assert!(!disabled);
        open_flyout(&ctx, &mut editor, "Arrange");
        assert!(menu_item(&ctx, &mut editor, "Crop").is_none());
        assert!(menu_item(&ctx, &mut editor, "Flip left to right").is_none());
        assert!(menu_item(&ctx, &mut editor, "Rotate by 90 degrees").is_some());

        click_menu_item(&ctx, &mut editor, "Adjust colours");
        let_dialog_open(&ctx, &mut editor);
        assert_eq!(editor.right_panel, Some(RightPanel::Tool(LookTab::Colours)));

        let end = editor.project.tracks.duration();
        editor.seek(end);
        run_frame(&ctx, &mut editor, vec![]);
        right_click_menu_on(&ctx, &mut editor, title);
        let (_, disabled) = menu_item(&ctx, &mut editor, "Arrange").expect("it is listed");
        assert!(disabled, "a title's size is only known at the playhead");
    }

    #[test]
    fn the_toolbar_holds_only_crop_rotate_flip_and_more() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        run_frame(&ctx, &mut editor, vec![]);
        for name in ["Crop", "Rotate by 90 degrees", "Flip", "More"] {
            assert!(is_named(&ctx, &mut editor, name), "{name}");
        }
        assert!(
            !is_named(&ctx, &mut editor, "Flip left to right"),
            "it's in Flip's menu"
        );
        assert!(!is_named(&ctx, &mut editor, "Filters and colours"));
        let rail = ctx.content_rect().right() - RAIL_WIDTH;
        for name in ["Speed", "Audio", "Filters"] {
            let (_, rect, _) = named_nodes(&ctx, &mut editor)
                .into_iter()
                .find(|(label, ..)| label == name)
                .unwrap_or_else(|| panic!("nothing is named {name}"));
            assert!(
                rect.left() >= rail,
                "{name} is on the rail, not the toolbar"
            );
        }
    }

    #[test]
    fn the_rail_only_offers_what_the_selected_clip_can_do() {
        let ctx = context();
        let sound = clip_with_sound();
        let picture = image_file("rail", "64x64", "red");
        let mut editor = with_videos(&ctx, &[&sound, test_clip(), &picture, song_file()]);
        let tabs_of = |editor: &mut Editor, id| {
            editor.selected = Some(id);
            editor.clip_tabs()
        };
        let look = [
            LookTab::Filters,
            LookTab::Effects,
            LookTab::Colours,
            LookTab::Fade,
        ];
        let picture_clips: Vec<_> = editor.project.tracks.picture_tracks()[0].clips().to_vec();
        assert_eq!(
            tabs_of(&mut editor, picture_clips[0].id),
            [&look[..], &[LookTab::Speed, LookTab::Audio]].concat()
        );
        assert_eq!(
            tabs_of(&mut editor, picture_clips[1].id),
            [&look[..], &[LookTab::Speed]].concat()
        );
        assert_eq!(tabs_of(&mut editor, picture_clips[2].id), look.to_vec());
        let song = sound_row_clip(&editor).id;
        assert_eq!(
            tabs_of(&mut editor, song),
            vec![LookTab::Speed, LookTab::Audio]
        );

        let title = with_a_title(&ctx, &mut editor, TitleStyle::Plain);
        assert_eq!(
            tabs_of(&mut editor, title),
            vec![LookTab::Text, LookTab::Colours, LookTab::Fade]
        );
        editor.selected = None;
        assert!(editor.clip_tabs().is_empty());
    }

    #[test]
    fn a_rail_tab_opens_its_panel_and_pressing_it_again_closes_it() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.looking_at().is_none());

        click_named(&ctx, &mut editor, "Audio");
        let_dialog_open(&ctx, &mut editor);
        assert_eq!(editor.right_panel, Some(RightPanel::Tool(LookTab::Audio)));
        assert!(editor.looking_at().is_some());
        assert!(is_named(&ctx, &mut editor, "Volume"));

        click_named(&ctx, &mut editor, "Speed");
        let_dialog_open(&ctx, &mut editor);
        assert_eq!(editor.right_panel, Some(RightPanel::Tool(LookTab::Speed)));
        assert!(is_named(&ctx, &mut editor, "Clip speed"));

        click_named(&ctx, &mut editor, "Speed");
        let_dialog_open(&ctx, &mut editor);
        assert!(editor.looking_at().is_none());
    }

    #[test]
    fn the_panel_goes_to_captions_on_a_clip_without_that_tool_and_stays_there() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound, test_clip()]);
        let clips: Vec<_> = editor.project.tracks.picture_tracks()[0].clips().to_vec();
        editor.selected = Some(clips[0].id);
        editor.right_panel = Some(RightPanel::Tool(LookTab::Audio));
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.looking_at().is_some());

        editor.selected = Some(clips[1].id);
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.right_panel, Some(RightPanel::Captions));

        editor.selected = Some(clips[0].id);
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.right_panel, Some(RightPanel::Captions));
    }

    #[test]
    fn the_panel_stays_on_a_tool_the_next_clip_also_has() {
        let ctx = context();
        let (mut editor, first, second) = with_two_touching_clips(&ctx);
        editor.selected = Some(first);
        editor.right_panel = Some(RightPanel::Tool(LookTab::Effects));
        run_frame(&ctx, &mut editor, vec![]);
        editor.selected = Some(second);
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.right_panel, Some(RightPanel::Tool(LookTab::Effects)));
    }

    #[test]
    fn deselecting_goes_to_captions_and_selecting_again_doesnt_go_back() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let clip = editor.selected.expect("a new clip is selected");
        editor.right_panel = Some(RightPanel::Tool(LookTab::Filters));
        run_frame(&ctx, &mut editor, vec![]);

        editor.selected = None;
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.right_panel, Some(RightPanel::Captions));
        assert!(is_named(&ctx, &mut editor, "Close captions"));

        editor.selected = Some(clip);
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.right_panel, Some(RightPanel::Captions));
    }

    #[test]
    fn selecting_never_opens_a_closed_panel_and_only_a_close_closes_one() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let clip = editor.selected.expect("a new clip is selected");
        editor.selected = None;
        run_frame(&ctx, &mut editor, vec![]);
        editor.selected = Some(clip);
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.right_panel, None);

        click_named(&ctx, &mut editor, "Captions");
        assert_eq!(editor.right_panel, Some(RightPanel::Captions));
        click_named(&ctx, &mut editor, "Close captions");
        assert_eq!(editor.right_panel, None);

        click_named(&ctx, &mut editor, "Filters");
        assert_eq!(editor.right_panel, Some(RightPanel::Tool(LookTab::Filters)));
        click_named(&ctx, &mut editor, "Close look");
        assert_eq!(editor.right_panel, None);
    }

    #[test]
    fn the_rail_is_there_with_nothing_selected_and_tools_go_under_captions() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let clip = editor.selected.expect("a new clip is selected");
        editor.selected = None;
        let rail = ctx.content_rect().right() - RAIL_WIDTH;
        run_frame(&ctx, &mut editor, vec![]);
        assert!(named_rect(&ctx, &mut editor, "Captions").left() >= rail);
        assert!(!is_named(&ctx, &mut editor, "Filters"));

        editor.selected = Some(clip);
        let captions = named_rect(&ctx, &mut editor, "Captions");
        let filters = named_rect(&ctx, &mut editor, "Filters");
        assert!(filters.top() > captions.bottom());
    }

    #[test]
    fn selecting_and_deselecting_never_moves_the_picture() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let clip = editor.selected.expect("a new clip is selected");
        let frame = |ctx: &egui::Context, editor: &mut Editor| {
            run_frame(ctx, editor, vec![]);
            run_frame(ctx, editor, vec![]);
            editor.stage_state.frame.expect("the picture shows")
        };

        editor.selected = None;
        let without = frame(&ctx, &mut editor);
        editor.selected = Some(clip);
        assert_eq!(frame(&ctx, &mut editor), without);

        // With a tool open, deselecting swaps it for Captions at the same width.
        editor.right_panel = Some(RightPanel::Tool(LookTab::Filters));
        let open = frame(&ctx, &mut editor);
        editor.selected = None;
        assert_eq!(frame(&ctx, &mut editor), open);
        assert_eq!(editor.right_panel, Some(RightPanel::Captions));
        editor.selected = Some(clip);
        assert_eq!(frame(&ctx, &mut editor), open);
    }

    #[test]
    fn a_press_that_selects_a_clip_doesnt_move_the_picture_under_its_drag() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.selected = None;
        run_frame(&ctx, &mut editor, vec![]);
        let middle = editor
            .stage_state
            .frame
            .expect("the picture shows")
            .center();
        let frame_before = editor.stage_state.frame;

        let to = middle + vec2(20.0, 0.0);
        begin_drag(&ctx, &mut editor, middle, to);
        assert!(editor.selected.is_some(), "the drag selected it");
        assert_eq!(editor.stage_state.frame, frame_before);
        end_drag(&ctx, &mut editor, to);
    }

    #[test]
    fn a_new_title_opens_text_but_clicking_one_opens_nothing() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let title = with_a_title(&ctx, &mut editor, TitleStyle::Plain);
        assert_eq!(editor.right_panel, Some(RightPanel::Tool(LookTab::Text)));
        assert!(is_named(&ctx, &mut editor, "Title text"));

        click_named(&ctx, &mut editor, "Close look");
        editor.selected = None;
        run_frame(&ctx, &mut editor, vec![]);
        editor.selected = Some(title);
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.right_panel, None);
        assert!(!is_named(&ctx, &mut editor, "Title text"));

        click_named(&ctx, &mut editor, "Edit text");
        assert!(is_named(&ctx, &mut editor, "Title text"));
    }

    // The Captions panel.

    const OUT_OF_ORDER_CAPTIONS: &str = "1\n00:00:03,000 --> 00:00:04,000\nSecond\n\n\
                                2\n00:00:01,000 --> 00:00:02,000\nFirst\n\n";

    fn with_captions_open(ctx: &egui::Context, name: &str) -> Editor {
        let mut editor = with_videos(ctx, &[test_clip()]);
        let path = subtitle_file(name, OUT_OF_ORDER_CAPTIONS);
        editor.import_captions(&path);
        let _ = std::fs::remove_file(&path);
        editor.right_panel = Some(RightPanel::Captions);
        run_frame(ctx, &mut editor, vec![]);
        editor
    }

    fn at(seconds: f64) -> Ticks {
        Ticks::from_seconds(seconds)
    }

    #[test]
    fn the_list_has_captions_in_time_order_and_no_other_titles() {
        let ctx = context();
        let mut editor = with_captions_open(&ctx, "panel-list");
        with_a_title(&ctx, &mut editor, TitleStyle::Plain);
        let starts: Vec<Ticks> = editor.caption_rows().iter().map(|row| row.start).collect();
        assert_eq!(starts, [at(1.0), at(3.0)]);
    }

    #[test]
    fn an_empty_project_says_how_to_get_captions_and_cant_save() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.right_panel = Some(RightPanel::Captions);
        let nodes = named_nodes(&ctx, &mut editor);
        let (.., disabled) = nodes
            .iter()
            .find(|(label, ..)| label == "Save SRT file")
            .expect("it is there");
        assert!(*disabled);
        assert!(is_named(&ctx, &mut editor, "Import SRT file"));
    }

    #[test]
    fn pressing_a_caption_selects_it_and_moves_the_playhead() {
        let ctx = context();
        let mut editor = with_captions_open(&ctx, "panel-jump");
        let second = editor.caption_rows()[1];
        let name = format!(
            "Go to caption {}",
            bareclip_core::time::format_timecode(second.start)
        );
        let box_top = named_rect(&ctx, &mut editor, &name).left_top() + vec2(20.0, 8.0);
        click(&ctx, &mut editor, box_top);
        assert_eq!(editor.selected, Some(second.clip));
        assert_eq!(editor.position(), second.start);
        assert_eq!(editor.right_panel, Some(RightPanel::Captions));
    }

    fn focus_caption(ctx: &egui::Context, editor: &mut Editor, row: usize) {
        let start = editor.caption_rows()[row].start;
        let name = format!("Caption {}", bareclip_core::time::format_timecode(start));
        click_named(ctx, editor, &name);
    }

    fn typed(words: &str) -> Vec<egui::Event> {
        vec![egui::Event::Text(words.to_owned())]
    }

    fn caption_words(editor: &Editor, row: usize) -> String {
        let row = editor.caption_rows()[row];
        editor.project.titles[&row.media].text.clone()
    }

    #[test]
    fn typing_in_the_list_changes_that_caption_as_one_undo_step() {
        let ctx = context();
        let mut editor = with_captions_open(&ctx, "panel-type");
        focus_caption(&ctx, &mut editor, 0);
        assert_eq!(editor.selected, Some(editor.caption_rows()[0].clip));
        run_frame(&ctx, &mut editor, typed("A"));
        run_frame(&ctx, &mut editor, typed("B"));
        assert!(caption_words(&editor, 0).contains("AB"));

        run_frame(&ctx, &mut editor, press(Key::Escape));
        run_frame(&ctx, &mut editor, vec![]);
        editor.undo();
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(caption_words(&editor, 0), "First");
    }

    #[test]
    fn enter_goes_to_the_next_caption_and_keeps_their_undo_steps_apart() {
        let ctx = context();
        let mut editor = with_captions_open(&ctx, "panel-enter");
        focus_caption(&ctx, &mut editor, 0);
        run_frame(&ctx, &mut editor, typed("A"));
        run_frame(&ctx, &mut editor, press(Key::Enter));
        run_frame(&ctx, &mut editor, vec![]);
        run_frame(&ctx, &mut editor, vec![]);
        let second = editor.caption_rows()[1];
        assert_eq!(editor.selected, Some(second.clip));
        assert_eq!(editor.position(), second.start);
        run_frame(&ctx, &mut editor, typed("B"));
        assert!(caption_words(&editor, 1).contains('B'));
        assert!(caption_words(&editor, 0).contains('A'));

        run_frame(&ctx, &mut editor, press(Key::Escape));
        run_frame(&ctx, &mut editor, vec![]);
        editor.undo();
        assert_eq!(caption_words(&editor, 1), "Second");
        assert!(caption_words(&editor, 0).contains('A'));
    }

    #[test]
    fn the_saved_file_follows_the_timeline_and_reads_back() {
        let ctx = context();
        let mut editor = with_captions_open(&ctx, "panel-save");
        let cues = bareclip_core::srt::parse_srt(&editor.captions_file());
        assert_eq!(cues.len(), 2);
        assert_eq!(
            (cues[0].start, cues[0].end, cues[0].text.as_str()),
            (at(1.0), at(2.0), "First")
        );
        assert_eq!(cues[1].text, "Second");
        let row = editor.caption_rows()[0];
        editor.change_title(row.media, |title| title.text = "Changed".to_owned());
        let cues = bareclip_core::srt::parse_srt(&editor.captions_file());
        assert_eq!(cues[0].text, "Changed");
    }

    #[test]
    fn thousands_of_captions_only_lay_out_what_shows() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let path = subtitle_file("panel-many", &long_subtitles(3000));
        editor.import_captions(&path);
        let _ = std::fs::remove_file(&path);
        editor.right_panel = Some(RightPanel::Captions);
        let nodes = named_nodes(&ctx, &mut editor);
        let shown = nodes
            .iter()
            .filter(|(label, ..)| label.starts_with("Go to caption"))
            .count();
        assert!(shown > 0 && shown < 40, "{shown} rows were laid out");
    }

    #[test]
    #[ignore = "draws pictures to look at"]
    fn draw_the_clip_tools_screens() {
        let folder = std::env::var_os("BARECLIP_SCREENS").map_or_else(
            || std::env::temp_dir().join("bareclip-screens"),
            PathBuf::from,
        );
        let ctx = context();
        let mut camera = Camera::new(folder);
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        for _ in 0..60 {
            run_frame(&ctx, &mut editor, vec![]);
            thread::sleep(Duration::from_millis(10));
        }
        camera.shoot(&ctx, &mut editor, vec![], "tools-01-rail-and-toolbar");
        let selected = editor.selected;
        editor.selected = None;
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "tools-00-rail-nothing-selected");
        let path = subtitle_file("screens", OUT_OF_ORDER_CAPTIONS);
        editor.import_captions(&path);
        let _ = std::fs::remove_file(&path);
        click_named(&ctx, &mut editor, "Captions");
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "tools-00-captions");
        click_named(&ctx, &mut editor, "Close captions");
        editor.undo();
        editor.selected = selected;
        let_dialog_open(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Select all");
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "tools-00-ticking");
        click_named(&ctx, &mut editor, "Deselect all");
        click_named(&ctx, &mut editor, "Video options");
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "tools-00-card-menu");
        click_named(&ctx, &mut editor, "Rename");
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "tools-00-renaming");
        run_frame(&ctx, &mut editor, press(Key::Escape));
        let_dialog_open(&ctx, &mut editor);

        click_named(&ctx, &mut editor, "Flip");
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "tools-02-flip");
        run_frame(&ctx, &mut editor, key_event(Key::Escape, Modifiers::NONE));

        click_named(&ctx, &mut editor, "Filters");
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "tools-03-filters");
        click_named(&ctx, &mut editor, "Speed");
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "tools-04-speed");
        click_named(&ctx, &mut editor, "Audio");
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "tools-05-audio");
        click_named(&ctx, &mut editor, "Audio");
        let_dialog_open(&ctx, &mut editor);

        open_clip_menu(&ctx, &mut editor);
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "tools-06-menu");
        open_flyout(&ctx, &mut editor, "Audio");
        let line = named_rect(&ctx, &mut editor, "Volume");
        let rest = moved(line.center());
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, rest, "tools-07-menu-audio");
        open_flyout(&ctx, &mut editor, "Arrange");
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "tools-08-menu-arrange");
    }

    #[test]
    fn the_clips_menu_turns_noise_suppression_on_for_sound_and_greys_it_out_for_a_video() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[song_file()]);
        open_clip_menu(&ctx, &mut editor);
        open_flyout(&ctx, &mut editor, "Audio");

        click_menu_item(&ctx, &mut editor, "Noise suppression");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(sound_row_clip(&editor).sound.noise_suppression);
        editor.undo();
        assert!(!sound_row_clip(&editor).sound.noise_suppression);

        let sound = clip_with_sound();
        let mut video = with_videos(&ctx, &[&sound]);
        open_clip_menu(&ctx, &mut video);
        open_flyout(&ctx, &mut video, "Audio");
        let (_, disabled) =
            menu_item(&ctx, &mut video, "Noise suppression").expect("it is listed for a video too");
        assert!(disabled, "a video has to have its sound detached first");
    }

    #[test]
    fn the_clips_menu_only_offers_what_the_clip_can_do() {
        let ctx = context();
        let mut silent = with_videos(&ctx, &[test_clip()]);
        open_clip_menu(&ctx, &mut silent);
        assert!(is_named(&ctx, &mut silent, "Speed"));
        assert!(!is_named(&ctx, &mut silent, "Audio"));

        let image = image_file("menu", "800x600", "orange");
        let mut still = with_videos(&ctx, &[image.as_path()]);
        open_clip_menu(&ctx, &mut still);
        assert!(!is_named(&ctx, &mut still, "Speed"));
        assert!(!is_named(&ctx, &mut still, "Audio"));
    }

    #[test]
    fn moving_on_to_another_line_closes_whats_beside_the_menu_and_a_click_outside_closes_it() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        open_clip_menu(&ctx, &mut editor);
        open_flyout(&ctx, &mut editor, "Speed");
        assert!(is_named(&ctx, &mut editor, "Clip speed"));

        let duplicate = named_rect(&ctx, &mut editor, "Duplicate");
        run_frame(&ctx, &mut editor, moved(duplicate.center()));
        let_dialog_open(&ctx, &mut editor);
        assert!(!is_named(&ctx, &mut editor, "Clip speed"));
        assert!(is_named(&ctx, &mut editor, "Duplicate"));

        click(&ctx, &mut editor, pos2(640.0, 100.0));
        assert!(!is_named(&ctx, &mut editor, "Duplicate"));
        assert_eq!(editor.timeline_state.menu_target, None);
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
            "Video options",
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

        for name in ["Export", "Add media", "Split", "Delete"] {
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
        let ProjectReport::Fast(report) = report else {
            panic!("videos in one format take the fast route");
        };
        assert_eq!(report.black_frames, 30);
        let info = probe(&output).expect("the export can be read");
        assert!((info.duration.as_seconds() - 4.0).abs() < 0.1);
        let _ = std::fs::remove_file(&output);
    }

    /// Rests the pointer somewhere, and gives the frame after it a chance to react.
    fn hover_readout_at(ctx: &egui::Context, editor: &mut Editor, at: egui::Pos2) -> Option<Ticks> {
        run_frame(ctx, editor, moved(at));
        run_frame(ctx, editor, vec![]);
        editor.timeline_state.hover_readout
    }

    #[test]
    fn past_the_last_clip_the_time_under_the_pointer_waits_at_the_end() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let total = editor.project.tracks.duration();
        let scale = editor.timeline_state.scale;
        let track = editor.timeline_state.track;
        let area = editor.timeline_state.layout.area;

        // Just past the end, far past it, on the empty track and on the ruler.
        let end = scale.x_of(total);
        for at in [
            pos2(end + 3.0, track.center().y),
            pos2(area.right() - 20.0, track.center().y),
            pos2(area.right() - 20.0, track.top() - 12.0),
        ] {
            assert_eq!(
                hover_readout_at(&ctx, &mut editor, at),
                Some(total),
                "{at:?}"
            );
        }
    }

    #[test]
    fn an_empty_timeline_has_no_time_under_the_pointer() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip()]);
        let area = editor.timeline_state.layout.area;

        assert_eq!(hover_readout_at(&ctx, &mut editor, area.center()), None);
    }

    #[test]
    fn hovering_the_empty_timeline_shows_the_time_there() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let scale = editor.timeline_state.scale;
        let track = editor.timeline_state.track;

        // On the ruler, even above a clip.
        let at = pos2(scale.x_of(Ticks::from_seconds(2.5)), track.top() - 12.0);
        let shown = hover_readout_at(&ctx, &mut editor, at).expect("a time is shown");
        assert!((shown.as_seconds() - 2.5).abs() < 0.02, "{shown:?}");

        let away = pos2(track.center().x, 100.0);
        assert_eq!(hover_readout_at(&ctx, &mut editor, away), None);
    }

    #[test]
    fn the_hover_time_stays_away_from_clips() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let scale = editor.timeline_state.scale;
        let y = editor.timeline_state.track.center().y;

        // The middle of a clip, and its trim handle.
        for time in [1.5, 4.0] {
            let at = pos2(scale.x_of(Ticks::from_seconds(time)), y);
            assert_eq!(hover_readout_at(&ctx, &mut editor, at), None, "at {time}");
        }
    }

    #[test]
    fn the_hover_time_stays_away_from_the_playhead() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.seek(Ticks::from_seconds(2.0));
        run_frame(&ctx, &mut editor, vec![]);
        let scale = editor.timeline_state.scale;
        let track = editor.timeline_state.track;
        let x = scale.x_of(Ticks::from_seconds(2.0));
        let ruler_y = track.top() - 12.0;

        assert_eq!(hover_readout_at(&ctx, &mut editor, pos2(x, ruler_y)), None);
        let beside = pos2(x + 30.0, ruler_y);
        assert!(hover_readout_at(&ctx, &mut editor, beside).is_some());
    }

    #[test]
    fn the_hover_time_stays_away_from_a_gaps_button_but_shows_on_the_gap() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip(), second_clip()]);
        let second = editor.project.tracks.played().clips()[1];
        let total = editor.project.tracks.duration();
        let moved_to = Ticks(total.0 + Ticks::from_seconds(4.0).0);
        editor
            .project
            .tracks
            .move_clip(second.id, moved_to, TrackTarget::Existing(0));
        run_frame(&ctx, &mut editor, vec![]);
        editor.timeline_state.fit(editor.project.tracks.duration());
        run_frame(&ctx, &mut editor, vec![]);
        let gap = editor.project.tracks.gaps()[0];
        let scale = editor.timeline_state.scale;
        let track = editor.timeline_state.track;
        let middle = scale.x_of(Ticks((gap.start.0 + gap.end.0) / 2));

        let on_gap = pos2(middle - 60.0, track.center().y);
        assert!(hover_readout_at(&ctx, &mut editor, on_gap).is_some());

        let on_button = pos2(middle, track.center().y);
        assert_eq!(hover_readout_at(&ctx, &mut editor, on_button), None);
    }

    #[test]
    fn dragging_the_playhead_seeks_as_it_moves_and_not_while_it_rests() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let scale = editor.timeline_state.scale;
        let ruler_y = editor.timeline_state.track.top() - 12.0;
        let x_of = |seconds| scale.x_of(Ticks::from_seconds(seconds));
        let before = editor.preview.seeks_sent();

        begin_drag(
            &ctx,
            &mut editor,
            pos2(x_of(1.0), ruler_y),
            pos2(x_of(3.0), ruler_y),
        );
        let after_moving = editor.preview.seeks_sent();
        assert!(after_moving > before);

        for _ in 0..10 {
            run_frame(&ctx, &mut editor, vec![]);
        }
        assert_eq!(editor.preview.seeks_sent(), after_moving);

        end_drag(&ctx, &mut editor, pos2(x_of(3.0), ruler_y));
    }

    #[test]
    fn moving_the_playhead_pauses_playback_until_it_is_let_go() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let scale = editor.timeline_state.scale;
        let ruler_y = editor.timeline_state.track.top() - 12.0;
        let (from, to) = (
            pos2(scale.x_of(Ticks::from_seconds(1.0)), ruler_y),
            pos2(scale.x_of(Ticks::from_seconds(3.0)), ruler_y),
        );
        editor.toggle_playback();

        begin_drag(&ctx, &mut editor, from, to);
        assert!(!editor.preview.is_playing());

        end_drag(&ctx, &mut editor, to);
        assert!(editor.preview.is_playing());
        assert!(!editor.resume_after_scrub);
    }

    #[test]
    fn a_paused_video_stays_paused_after_moving_the_playhead() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let scale = editor.timeline_state.scale;
        let ruler_y = editor.timeline_state.track.top() - 12.0;
        let (from, to) = (
            pos2(scale.x_of(Ticks::from_seconds(1.0)), ruler_y),
            pos2(scale.x_of(Ticks::from_seconds(3.0)), ruler_y),
        );

        begin_drag(&ctx, &mut editor, from, to);
        end_drag(&ctx, &mut editor, to);

        assert!(!editor.preview.is_playing());
    }

    #[test]
    fn letting_go_at_the_very_end_does_not_start_over() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let scale = editor.timeline_state.scale;
        let ruler_y = editor.timeline_state.track.top() - 12.0;
        let total = editor.project.tracks.duration();
        let (from, to) = (
            pos2(scale.x_of(Ticks::from_seconds(1.0)), ruler_y),
            pos2(scale.x_of(total) + 30.0, ruler_y),
        );
        editor.toggle_playback();

        begin_drag(&ctx, &mut editor, from, to);
        end_drag(&ctx, &mut editor, to);

        assert!(!editor.preview.is_playing());
        assert_eq!(editor.position(), total);
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
        assert!(before < scale.width * 0.9, "{before}");

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

    /// Two videos, moved so that each has its own track.
    fn on_two_tracks(ctx: &egui::Context) -> Editor {
        let mut editor = two_videos(ctx);
        let from = second_video_middle(&editor);
        let lane = editor.timeline_state.layout.new_lane_below();
        let to = pos2(from.x, lane.center().y);
        begin_drag(ctx, &mut editor, from, to);
        end_drag(ctx, &mut editor, to);
        run_frame(ctx, &mut editor, vec![]);
        assert_eq!(editor.project.tracks.tracks().len(), 2);
        editor
    }

    fn press_without_following_up(ctx: &egui::Context, editor: &mut Editor, at: egui::Pos2) {
        run_frame(ctx, editor, moved(at));
        run_frame(
            ctx,
            editor,
            pointer_button(at, egui::PointerButton::Primary, true),
        );
        run_frame(
            ctx,
            editor,
            pointer_button(at, egui::PointerButton::Primary, false),
        );
    }

    #[test]
    fn the_lanes_go_once_there_are_two_tracks_and_come_back_with_one() {
        let ctx = context();
        let mut editor = two_videos(&ctx);
        let area = editor.timeline_state.layout.area;
        let lanes_say = |output: &egui::FullOutput| {
            let shown = text_in(output, area);
            ["Add text", "Add audio"].map(|hint| shown.contains(&hint.to_owned()))
        };
        let output = run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(lanes_say(&output), [true, true]);

        let from = second_video_middle(&editor);
        let lane = editor.timeline_state.layout.new_lane_below();
        let to = pos2(from.x, lane.center().y);
        begin_drag(&ctx, &mut editor, from, to);
        end_drag(&ctx, &mut editor, to);
        run_frame(&ctx, &mut editor, vec![]);
        let output = run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.project.tracks.tracks().len(), 2);
        assert_eq!(lanes_say(&output), [false, false]);

        editor.undo();
        run_frame(&ctx, &mut editor, vec![]);
        let output = run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(lanes_say(&output), [true, true]);
    }

    #[test]
    fn pressing_the_lane_below_one_track_adds_audio() {
        let ctx = context();
        let mut editor = two_videos(&ctx);
        let lane = editor.timeline_state.layout.new_lane_below();

        press_without_following_up(&ctx, &mut editor, lane.center());

        // Taken back before the next frame, which would open the file picker.
        assert!(std::mem::take(&mut editor.add_audio_requested));
    }

    #[test]
    fn pressing_below_two_tracks_only_moves_the_playhead() {
        let ctx = context();
        let mut editor = on_two_tracks(&ctx);
        let lane = editor.timeline_state.layout.new_lane_below();

        press_without_following_up(&ctx, &mut editor, lane.center());

        assert!(!std::mem::take(&mut editor.add_audio_requested));
    }

    #[test]
    fn a_clip_can_still_make_a_new_track_past_either_end_of_two_tracks() {
        let ctx = context();
        let row_of = |editor: &Editor, row: usize| editor.project.tracks.tracks()[row].clips()[0];
        let middle_of = |editor: &Editor, row: usize| {
            let clip = row_of(editor, row);
            pos2(
                editor
                    .timeline_state
                    .scale
                    .x_of(Ticks(clip.start.0 + clip.duration().0 / 2)),
                editor.timeline_state.layout.track(row).center().y,
            )
        };

        // Off the bottom of the last track, and the clip from the top row goes under it.
        let mut below = on_two_tracks(&ctx);
        let moving = row_of(&below, 0).id;
        let from = middle_of(&below, 0);
        let to = pos2(from.x, below.timeline_state.layout.track(1).bottom() + 20.0);
        begin_drag(&ctx, &mut below, from, to);
        end_drag(&ctx, &mut below, to);
        assert_eq!(below.project.tracks.tracks().len(), 2, "its own row went");
        assert_eq!(row_of(&below, 1).id, moving);

        // Off the top of the first track, and the clip from the bottom row goes over it.
        let mut above = on_two_tracks(&ctx);
        let moving = row_of(&above, 1).id;
        let from = middle_of(&above, 1);
        let to = pos2(from.x, above.timeline_state.layout.track(0).top() - 2.0);
        begin_drag(&ctx, &mut above, from, to);
        end_drag(&ctx, &mut above, to);
        assert_eq!(above.project.tracks.tracks().len(), 2, "its own row went");
        assert_eq!(row_of(&above, 0).id, moving);
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
    fn a_format_only_a_hidden_clip_uses_does_not_force_a_full_export() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip(), small_clip()]);
        assert!(matches!(editor.export_route(), Route::Full(_)));

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
        assert_eq!(editor.export_route(), Route::Fast);
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

    fn clips_of(editor: &Editor, media: MediaId) -> usize {
        editor
            .project
            .tracks
            .tracks()
            .iter()
            .flat_map(Timeline::clips)
            .filter(|clip| clip.media == media)
            .count()
    }

    #[test]
    fn removing_a_video_takes_its_clips_for_good_and_undo_skips_what_only_it_was_in() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip(), second_clip()]);
        let gone = editor.media[0].id;
        let kept = editor.media[1].id;
        let gone_clip = editor
            .project
            .tracks
            .played()
            .clips()
            .iter()
            .find(|clip| clip.media == gone)
            .expect("it's on the timeline")
            .id;
        editor.toggle_clip_mute(gone_clip);
        let kept_clips = clips_of(&editor, kept);

        editor.remove_media(gone);
        assert_eq!(clips_of(&editor, gone), 0);
        assert_eq!(clips_of(&editor, kept), kept_clips);
        assert!(editor.entry(gone).is_none(), "it's gone from the project");

        // Going all the way back and forward again, it never returns, and every step changes
        // something: muting it, which only it was in, is no longer a step.
        let mut before = editor.project.clone();
        while editor.history.can_undo() {
            editor.undo();
            assert_eq!(clips_of(&editor, gone), 0);
            assert_ne!(editor.project, before, "an undo changed nothing");
            before = editor.project.clone();
        }
        while editor.history.can_redo() {
            editor.redo();
            assert_eq!(clips_of(&editor, gone), 0);
            assert_ne!(editor.project, before, "a redo changed nothing");
            before = editor.project.clone();
        }
        assert_eq!(clips_of(&editor, kept), kept_clips);
        assert_eq!(editor.visible_media().count(), 1);
    }

    #[test]
    fn a_removed_video_can_be_deleted_straight_away() {
        let ctx = context();
        let folder = std::env::temp_dir().join("bareclip-app-tests");
        std::fs::create_dir_all(&folder).expect("the folder exists");
        let copy = folder.join(format!("removed-{}.mp4", std::process::id()));
        std::fs::copy(test_clip(), &copy).expect("the video copies");
        let mut editor = with_videos(&ctx, &[&copy]);
        let id = editor.media[0].id;
        editor.seek(Ticks::from_seconds(0.5));
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.preview.has_open(id), "playing it opened the file");

        editor.remove_media(id);

        assert!(!editor.preview.has_open(id));
        // Reading in the background stops at its next step, which is a moment away at most.
        let started = Instant::now();
        while let Err(error) = std::fs::remove_file(&copy) {
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "the file is still held: {error}"
            );
            thread::sleep(Duration::from_millis(20));
        }
        editor.undo();
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.notice.is_none(), "{:?}", editor.notice);
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
            ("options", card.right_top() + vec2(-20.0, 20.0)),
        ] {
            let cursor = cursor_over(&ctx, &mut editor, at);
            assert_eq!(cursor, egui::CursorIcon::Default, "{name}");
        }
        assert_eq!(
            cursor_over(&ctx, &mut editor, card.left_top() + vec2(16.0, 16.0)),
            egui::CursorIcon::Grab,
            "there is no box until something is ticked"
        );
        click_named(&ctx, &mut editor, "Select all");
        assert_eq!(
            cursor_over(&ctx, &mut editor, card.left_top() + vec2(16.0, 16.0)),
            egui::CursorIcon::Default,
            "select"
        );
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
        editor.settings.waveforms_on_clips = true;
        editor.apply_strip_settings(&ctx);

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
    fn turning_waveforms_on_reads_them_and_off_forgets_them_keeping_the_pictures() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip()]);
        wait_for_strip(&ctx, &mut editor);
        assert!(editor.media[0].sound_shape.is_none(), "they start off");
        assert!(!editor.media[0].strip_pictures.is_empty());

        editor.settings.waveforms_on_clips = true;
        editor.apply_strip_settings(&ctx);
        wait_for_strip(&ctx, &mut editor);
        assert!(editor.media[0].sound_shape.is_some());

        editor.settings.waveforms_on_clips = false;
        editor.apply_strip_settings(&ctx);
        assert!(editor.media[0].sound_shape.is_none());
        assert!(
            editor.media[0].strip_reader.is_none(),
            "the pictures were already read"
        );
        assert!(!editor.media[0].strip_pictures.is_empty());
    }

    #[test]
    fn a_sound_file_isnt_read_for_its_clip_while_waveforms_are_off() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);

        editor.import(&ctx, &[song_file().to_path_buf()]);

        assert!(editor.media[0].strip_reader.is_none());
    }

    #[test]
    fn only_waveforms_on_audio_read_a_sound_files_shape() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        editor.import(&ctx, &[song_file().to_path_buf()]);

        editor.settings.waveforms_on_clips = true;
        editor.apply_strip_settings(&ctx);
        assert!(editor.media[0].strip_reader.is_none(), "that's for videos");

        editor.settings.waveforms_on_clips = false;
        editor.settings.waveforms_on_audio = true;
        editor.apply_strip_settings(&ctx);
        assert!(editor.media[0].strip_reader.is_some());
        wait_for_strip(&ctx, &mut editor);
        assert!(editor.media[0].sound_shape.is_some());

        editor.settings.waveforms_on_audio = false;
        editor.apply_strip_settings(&ctx);
        assert!(editor.media[0].sound_shape.is_none(), "it's forgotten");
    }

    #[test]
    fn the_settings_dialog_offers_two_waveform_switches_and_remembers_each() {
        let ctx = context();
        let folder = project_folder("strip-settings");
        let mut editor = with_storage(&ctx, &folder);
        editor.import(&ctx, &[test_clip().to_path_buf()]);
        settle(&ctx, &mut editor);
        wait_for_strip(&ctx, &mut editor);

        click_named(&ctx, &mut editor, "Settings");
        let_dialog_open(&ctx, &mut editor);
        assert!(!is_named(&ctx, &mut editor, "Thumbnails on clips"));
        click_named(&ctx, &mut editor, "Waveforms on clips");

        assert!(editor.settings.waveforms_on_clips);
        assert!(!editor.settings.waveforms_on_audio);
        let saved = with_storage(&ctx, &folder).settings;
        assert!(saved.waveforms_on_clips && !saved.waveforms_on_audio);

        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Waveforms on audio");
        assert!(editor.settings.waveforms_on_audio);
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Waveforms on clips");
        assert!(!editor.settings.waveforms_on_clips);
        assert!(editor.settings.waveforms_on_audio, "it doesn't change both");
        let saved = with_storage(&ctx, &folder).settings;
        assert!(!saved.waveforms_on_clips && saved.waveforms_on_audio);
    }

    fn laptop_chips(editor: &mut Editor) -> (GpuChoice, GpuChoice) {
        let nvidia = GpuEntry {
            id: GpuChoice {
                vendor: 0x10de,
                device: 0x2520,
            },
            name: "NVIDIA test chip".to_owned(),
        };
        let amd = GpuEntry {
            id: GpuChoice {
                vendor: 0x1002,
                device: 0x1638,
            },
            name: "AMD test chip".to_owned(),
        };
        let ids = (nvidia.id, amd.id);
        editor.list_gpus(nvidia.clone(), vec![nvidia, amd]);
        ids
    }

    #[test]
    fn a_chip_picked_in_settings_is_remembered_and_needs_a_restart() {
        let ctx = context();
        let folder = project_folder("gpu-setting");
        let mut editor = with_storage(&ctx, &folder);
        let (_, amd) = laptop_chips(&mut editor);

        click_named(&ctx, &mut editor, "Settings");
        let_dialog_open(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Graphics chip: Automatic");
        settle_animations(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "AMD test chip");
        run_frame(&ctx, &mut editor, vec![]);

        assert_eq!(editor.settings.gpu, Some(amd));
        assert_eq!(with_storage(&ctx, &folder).settings.gpu, Some(amd));
        assert!(is_named(&ctx, &mut editor, "Graphics chip: AMD test chip"));
    }

    thread_local! {
        /// What each restart asked to open, on this test's thread.
        static RELAUNCHED: std::cell::RefCell<Vec<Option<PathBuf>>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    fn noted_relaunch(project: Option<&Path>) -> std::io::Result<()> {
        RELAUNCHED.with(|asked| asked.borrow_mut().push(project.map(Path::to_path_buf)));
        Ok(())
    }

    fn failed_relaunch(_project: Option<&Path>) -> std::io::Result<()> {
        Err(std::io::Error::other("made-up failure"))
    }

    fn relaunched() -> Vec<Option<PathBuf>> {
        RELAUNCHED.with(|asked| asked.borrow().clone())
    }

    /// Opens Settings and picks the AMD chip, so a switch is waiting for a restart.
    fn pick_another_chip(ctx: &egui::Context, editor: &mut Editor) {
        laptop_chips(editor);
        click_named(ctx, editor, "Settings");
        let_dialog_open(ctx, editor);
        assert!(
            !is_named(ctx, editor, "Restart now"),
            "nothing to restart for"
        );
        click_named(ctx, editor, "Graphics chip: Automatic");
        settle_animations(ctx, editor);
        click_named(ctx, editor, "AMD test chip");
        run_frame(ctx, editor, vec![]);
    }

    #[test]
    fn restart_now_opens_the_saved_project_again_and_closes() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let path = project_folder("restart-saved").join("a.bareclip");
        assert!(editor.save_to(&path));
        editor.relaunch = noted_relaunch;
        pick_another_chip(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Restart now");
        run_frame(&ctx, &mut editor, vec![]);

        assert_eq!(relaunched(), vec![Some(path)]);
        assert!(editor.closing);
        assert!(!editor.settings_open);
    }

    #[test]
    fn restart_now_asks_about_unsaved_changes_first() {
        let ctx = context();
        let (mut editor, path) = with_a_change_since_saving(&ctx, "restart-unsaved");
        editor.relaunch = noted_relaunch;
        pick_another_chip(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Restart now");
        let_dialog_open(&ctx, &mut editor);
        assert!(relaunched().is_empty(), "it waits for an answer");
        assert!(!editor.closing);

        click_named(&ctx, &mut editor, "Cancel");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(relaunched().is_empty());
        assert!(!editor.closing);

        click_named(&ctx, &mut editor, "Settings");
        let_dialog_open(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Restart now");
        let_dialog_open(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Save");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(relaunched(), vec![Some(path)]);
        assert!(editor.closing);
        assert!(!editor.has_unsaved_changes(), "the change was saved first");
    }

    #[test]
    fn restart_now_starts_an_untitled_project_with_nothing_to_open() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        editor.relaunch = noted_relaunch;
        pick_another_chip(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Restart now");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(relaunched(), vec![None]);
        assert!(editor.closing);
    }

    #[test]
    fn a_restart_that_fails_says_so_and_stays_open() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        editor.relaunch = failed_relaunch;
        pick_another_chip(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Restart now");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(!editor.closing);
        assert!(
            editor
                .notice
                .as_deref()
                .is_some_and(|notice| notice.starts_with(text::RESTART_FAILED))
        );
    }

    #[test]
    fn going_back_to_automatic_saves_that_too() {
        let ctx = context();
        let folder = project_folder("gpu-automatic");
        let mut editor = with_storage(&ctx, &folder);
        let (_, amd) = laptop_chips(&mut editor);
        editor.settings.gpu = Some(amd);

        click_named(&ctx, &mut editor, "Settings");
        let_dialog_open(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Graphics chip: AMD test chip");
        settle_animations(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Automatic");
        run_frame(&ctx, &mut editor, vec![]);

        assert_eq!(editor.settings.gpu, None);
        assert_eq!(with_storage(&ctx, &folder).settings.gpu, None);
    }

    #[test]
    fn a_picked_chip_that_is_gone_reads_automatic() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        laptop_chips(&mut editor);
        editor.settings.gpu = Some(GpuChoice {
            vendor: 0x8086,
            device: 0x46a6,
        });

        click_named(&ctx, &mut editor, "Settings");
        let_dialog_open(&ctx, &mut editor);

        assert!(is_named(&ctx, &mut editor, "Graphics chip: Automatic"));
    }

    #[test]
    fn the_settings_window_says_which_version_this_is_at_its_bottom_left_on_every_tab() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);

        click_named(&ctx, &mut editor, "Settings");
        let_dialog_open(&ctx, &mut editor);

        for tab in [
            "General settings",
            "Recording settings",
            "Keybinds settings",
        ] {
            click_named(&ctx, &mut editor, tab);
            let_dialog_open(&ctx, &mut editor);
            // Under the tabs, level with Done.
            let tabs = named_rect(&ctx, &mut editor, "General settings");
            let done = named_rect(&ctx, &mut editor, "Done");
            let bottom_left = egui::Rect::from_min_max(
                pos2(tabs.left(), done.top()),
                pos2(tabs.right(), done.bottom()),
            );
            let output = run_frame(&ctx, &mut editor, vec![]);
            assert!(
                text_in(&output, bottom_left).contains(&text::version_line()),
                "{tab}"
            );
            let version = text_rect(&output, &text::version_line()).expect("it is drawn");
            assert!(
                (version.center().y - done.center().y).abs() < 1.0,
                "{tab}: the version at {version:?} is level with Done at {done:?}"
            );
        }
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
    fn a_new_project_exports_as_untitled_beside_the_first_video_with_its_type() {
        let ctx = context();
        let editor = with_videos(&ctx, &[test_clip()]);

        let suggested = editor
            .suggested_output()
            .expect("a video is on the timeline");

        assert_eq!(project::DEFAULT_NAME, "untitled");
        assert_eq!(suggested.parent(), test_clip().parent());
        assert_eq!(
            suggested.file_name().and_then(|name| name.to_str()),
            Some("untitled.mp4")
        );
    }

    #[test]
    fn renaming_the_project_renames_the_exported_video() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);

        click_named(&ctx, &mut editor, &format!("{} *", project::DEFAULT_NAME));
        run_frame(&ctx, &mut editor, vec![]);
        run_frame(&ctx, &mut editor, key_event(Key::A, Modifiers::COMMAND));
        type_text(&ctx, &mut editor, "Final round");
        run_frame(&ctx, &mut editor, press(Key::Enter));
        run_frame(&ctx, &mut editor, vec![]);

        let suggested = editor
            .suggested_output()
            .expect("a video is on the timeline");
        assert_eq!(
            suggested.file_name().and_then(|name| name.to_str()),
            Some("Final round.mp4")
        );
    }

    #[test]
    fn a_full_export_is_suggested_as_an_mp4_under_the_project_name() {
        let ctx = context();
        let tall = make_clip("editor-suggested-tall", "180x320");
        let mut editor = with_videos(&ctx, &[test_clip(), &tall]);
        let_panel_finish(&ctx, &mut editor);
        assert!(matches!(editor.export_route(), Route::Full(_)));
        editor.project_name = "my.mixed cut".to_owned();

        let suggested = editor
            .suggested_output()
            .expect("a video is on the timeline");

        assert_eq!(
            suggested.file_name().and_then(|name| name.to_str()),
            Some("my.mixed cut.mp4")
        );
    }

    #[test]
    fn nothing_is_suggested_without_a_video_on_the_timeline() {
        let ctx = context();
        let editor = in_panel_only(&ctx, &[]);

        assert!(editor.suggested_output().is_none());
    }

    #[test]
    fn a_long_name_stays_inside_the_name_bar() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[]);
        let short = named_rect(&ctx, &mut editor, project::DEFAULT_NAME);

        let long = "a very long project name ".repeat(10);
        let long = long.trim().to_owned();
        editor.project_name.clone_from(&long);
        let held = named_rect(&ctx, &mut editor, &format!("{long} *"));

        assert_eq!(held.width(), PROJECT_NAME_WIDTH);
        assert_eq!(held, short);
    }

    #[test]
    fn opening_the_name_bar_for_typing_does_not_move_what_is_beside_it() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[]);
        let at_rest = named_rect(&ctx, &mut editor, "Project menu");

        click_named(&ctx, &mut editor, project::DEFAULT_NAME);
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.renaming.is_some());
        let typing = named_rect(&ctx, &mut editor, "Project menu");

        assert_eq!(at_rest.left(), typing.left());
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

    /// The file names of the clips on the picture rows, in the order they play.
    fn played_names(editor: &Editor) -> Vec<String> {
        editor
            .project
            .tracks
            .played()
            .clips()
            .iter()
            .map(|clip| {
                let entry = editor.entry(clip.media).expect("its video is known");
                entry
                    .info
                    .path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    fn file_name(path: &Path) -> String {
        path.file_name().unwrap().to_string_lossy().into_owned()
    }

    #[test]
    fn edit_with_bareclip_puts_files_on_the_end_of_the_timeline_as_one_undo_step() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let first_end = editor.project.tracks.duration();

        editor.edit_files(&[second_clip().to_path_buf(), song_file().to_path_buf()]);
        run_frame(&ctx, &mut editor, vec![]);
        settle(&ctx, &mut editor);

        assert_eq!(
            played_names(&editor),
            [file_name(test_clip()), file_name(second_clip())]
        );
        let clips = editor.project.tracks.played().clips().to_vec();
        assert_eq!(
            clips[1].start, first_end,
            "it goes straight after what was there"
        );
        let sound_rows = editor.project.tracks.audio_tracks();
        assert_eq!(sound_rows.len(), 1, "sound goes on a sound row");
        assert_eq!(sound_rows[0].clips().len(), 1);
        assert!(editor.media_panel_open);
        assert_eq!(
            editor.visible_media().count(),
            3,
            "both are listed in the panel"
        );

        editor.undo();
        assert_eq!(played_names(&editor), [file_name(test_clip())]);
        assert!(editor.project.tracks.audio_tracks().is_empty());
    }

    #[test]
    fn edit_with_bareclip_uses_a_video_already_in_the_panel() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip()]);
        let id = editor.media[0].id;
        editor.toggle_media(id);

        editor.edit_files(&[test_clip().to_path_buf()]);
        run_frame(&ctx, &mut editor, vec![]);

        assert_eq!(editor.visible_media().count(), 1, "it isn't listed twice");
        assert_eq!(clip_count(&editor), 1);
        assert_eq!(editor.project.tracks.played().clips()[0].media, id);
        assert_eq!(editor.selected_media, [id], "the ticks are left alone");
    }

    #[test]
    fn a_file_that_cant_be_read_says_so_and_adds_nothing() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let missing = project_folder("edit-missing").join("gone.mp4");

        editor.edit_files(&[missing]);
        run_frame(&ctx, &mut editor, vec![]);

        assert_eq!(clip_count(&editor), 1);
        let notice = editor.notice.clone().expect("it says what went wrong");
        assert!(notice.contains("gone.mp4"), "{notice}");
        editor.undo();
        assert_eq!(
            clip_count(&editor),
            0,
            "the only step to undo is the first clip"
        );
    }

    #[test]
    fn files_from_explorer_wait_for_the_restore_question_then_join_the_restored_work() {
        let ctx = context();
        let folder = project_folder("edit-after-crash");
        let mut first = with_storage(&ctx, &folder);
        first.import(&ctx, &[test_clip().to_path_buf()]);
        settle(&ctx, &mut first);
        let id = first.media[0].id;
        first.add_to_timeline(id);
        autosave_now(&ctx, &mut first);
        crash(first);

        let mut second = with_storage(&ctx, &folder);
        second.edit_files(&[second_clip().to_path_buf()]);
        run_frame(&ctx, &mut second, vec![]);
        assert_eq!(
            clip_count(&second),
            0,
            "nothing goes in while the question is open"
        );
        assert_eq!(second.recovery_offers.len(), 1);

        let_dialog_open(&ctx, &mut second);
        click_named(&ctx, &mut second, "Restore");
        settle(&ctx, &mut second);
        run_frame(&ctx, &mut second, vec![]);
        settle(&ctx, &mut second);

        assert_eq!(
            played_names(&second),
            [file_name(test_clip()), file_name(second_clip())]
        );
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
    fn videos_in_the_same_format_export_by_the_fast_route() {
        let ctx = context();
        let editor = with_videos(&ctx, &[test_clip(), second_clip()]);

        assert_eq!(editor.export_route(), Route::Fast);
    }

    #[test]
    fn steady_videos_at_different_rates_start_on_redraw_and_can_be_copied() {
        let ctx = context();
        let slow = clip_at_rate("steady-24", 24);
        let fast = clip_at_rate("steady-60", 60);
        let mut editor = with_videos(&ctx, &[&slow, &fast]);

        assert!(editor.rate_choice().is_some());
        assert_eq!(editor.current_rate_pick(), RatePick::Redraw);
        assert!(matches!(editor.export_route(), Route::Full(_)));

        editor.rate_pick = Some(RatePick::Copy);
        assert_eq!(editor.export_route(), Route::Fast);
    }

    #[test]
    fn videos_uneven_inside_at_different_rates_start_on_copy_and_can_be_redrawn() {
        let ctx = context();
        let first = uneven_clip("uneven-a", 25);
        let second = uneven_clip("uneven-b", 50);
        let mut editor = with_videos(&ctx, &[&first, &second]);

        assert!(editor.rate_choice().is_some());
        assert_eq!(editor.current_rate_pick(), RatePick::Copy);
        assert_eq!(editor.export_route(), Route::Fast);

        editor.rate_pick = Some(RatePick::Redraw);
        assert!(matches!(editor.export_route(), Route::Full(_)));
    }

    #[test]
    fn the_export_window_offers_copy_and_redraw_only_when_rates_differ() {
        let ctx = context();
        let slow = clip_at_rate("offered-24", 24);
        let fast = clip_at_rate("offered-60", 60);
        let mut mixed = with_videos(&ctx, &[&slow, &fast]);
        mixed.export_state = ExportState::Choosing;
        let_dialog_open(&ctx, &mut mixed);
        assert!(is_named(&ctx, &mut mixed, "Copy"));
        assert!(is_named(&ctx, &mut mixed, "Redraw"));

        let mut same = with_videos(&ctx, &[test_clip(), second_clip()]);
        same.export_state = ExportState::Choosing;
        let_dialog_open(&ctx, &mut same);
        assert!(!is_named(&ctx, &mut same, "Copy"));
        assert!(!is_named(&ctx, &mut same, "Redraw"));
    }

    #[test]
    fn a_pick_between_copy_and_redraw_only_lasts_for_one_export() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.rate_pick = Some(RatePick::Copy);

        editor.open_export();

        assert_eq!(editor.rate_pick, None);
    }

    #[test]
    fn videos_with_a_different_size_have_no_rate_choice_to_make() {
        let ctx = context();
        let slow = clip_at_rate("sized-24", 24);
        let small = small_clip();
        let editor = with_videos(&ctx, &[&slow, small]);

        assert_eq!(editor.rate_choice(), None);
    }

    /// A sound file with no picture: a steady tone twelve seconds long.
    fn song_file() -> &'static Path {
        static FILE: OnceLock<PathBuf> = OnceLock::new();
        FILE.get_or_init(|| {
            let folder = std::env::temp_dir().join("bareclip-app-tests");
            std::fs::create_dir_all(&folder).expect("the folder can be created");
            let path = folder.join(format!("song-{}.wav", std::process::id()));
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
                .arg("sine=f=1000:r=48000:d=12")
                .arg(&path)
                .status()
                .expect("ffmpeg is on PATH");
            assert!(status.success());
            path
        })
    }

    fn sound_row_clip(editor: &Editor) -> bareclip_core::timeline::Clip {
        editor.project.tracks.audio_tracks()[0].clips()[0]
    }

    fn picture_clip(editor: &Editor) -> bareclip_core::timeline::Clip {
        editor.project.tracks.picture_tracks()[0].clips()[0]
    }

    fn output_called(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("bareclip-{name}-{}.mp4", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn exported_report(ctx: &egui::Context, editor: &mut Editor, output: &Path) -> ProjectReport {
        editor.start_export_to(ctx, output.to_path_buf());
        wait_for_export(ctx, editor);
        match &editor.export_state {
            ExportState::Done { report, .. } => report.clone(),
            _ => panic!("the export did not finish"),
        }
    }

    #[test]
    fn a_sound_file_is_added_without_looking_for_frames_or_pictures() {
        let ctx = context();
        let editor = in_panel_only(&ctx, &[song_file()]);

        let entry = &editor.media[0];
        assert!(entry.info.is_sound_only());
        assert!(entry.scan.is_none());
        assert!(
            !card_details(entry)
                .iter()
                .any(|(line, _)| line.contains("fps"))
        );
    }

    #[test]
    fn sound_goes_on_its_own_row_with_videos_above_it() {
        let ctx = context();
        let editor = with_videos(&ctx, &[song_file(), &clip_with_sound()]);

        let tracks = &editor.project.tracks;
        assert_eq!(tracks.picture_rows(), 1);
        assert_eq!(tracks.audio_tracks().len(), 1);
        assert_eq!(picture_clip(&editor).start, Ticks(0));
        assert_eq!(sound_row_clip(&editor).start, Ticks(0));
    }

    #[test]
    fn sound_never_changes_the_projects_size() {
        let ctx = context();
        let editor = with_videos(&ctx, &[song_file(), test_clip()]);

        assert_eq!(editor.project.canvas.size(), (320, 180));
    }

    #[test]
    fn a_sound_file_dropped_on_a_video_row_lands_on_a_sound_row() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.import(&ctx, &[song_file().to_path_buf()]);
        let song = editor.media.last().expect("it was added").id;

        editor.drop_on_timeline(song, Ticks(0), TrackTarget::Existing(0));

        assert_eq!(editor.project.tracks.audio_tracks().len(), 1);
        assert_eq!(editor.project.tracks.picture_rows(), 1);
    }

    #[test]
    fn the_filter_can_show_only_videos_or_only_audio() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip(), song_file()]);
        let names = |editor: &Editor| -> Vec<bool> {
            editor
                .listed_media()
                .iter()
                .map(|entry| entry.info.is_sound_only())
                .collect()
        };
        assert_eq!(names(&editor).len(), 2);

        editor.media_filter = MediaFilter::Audio;
        assert_eq!(names(&editor), [true]);

        editor.media_filter = MediaFilter::Videos;
        assert_eq!(names(&editor), [false]);
    }

    #[test]
    fn pressing_a_clips_speaker_mutes_it_and_one_undo_brings_it_back() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        run_frame(&ctx, &mut editor, vec![]);

        click_named(&ctx, &mut editor, "Mute clip");
        assert!(picture_clip(&editor).sound.muted);

        editor.undo();
        assert!(!picture_clip(&editor).sound.muted);
    }

    #[test]
    fn a_clip_with_no_sound_has_no_speaker() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);

        assert!(!is_named(&ctx, &mut editor, "Mute clip"));
    }

    #[test]
    fn rows_have_no_sound_buttons_of_their_own() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound, song_file()]);

        for name in ["Mute row 1", "Mute row 2", "Solo row 1", "Solo row 2"] {
            assert!(!is_named(&ctx, &mut editor, name), "{name}");
        }
        assert!(is_named(&ctx, &mut editor, "Mute clip"));
    }

    #[test]
    fn every_clip_plays_unless_it_is_muted() {
        let ctx = context();
        let sound = clip_with_sound();
        let editor = with_videos(&ctx, &[&sound, song_file()]);

        let spans = heard_spans(&editor.media, &editor.project.tracks);

        assert_eq!(spans.len(), 2);
        assert!(spans.iter().all(|span| span.bus == Bus::All(0)));
    }

    #[test]
    fn adding_music_keeps_the_picture_copied_and_only_rebuilds_the_sound() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound, song_file()]);
        let clip = picture_clip(&editor);
        let song = sound_row_clip(&editor);
        editor.project.tracks.trim(
            song.id,
            ClipEdge::End,
            clip.end(),
            Ticks::from_seconds(12.0),
            Ticks(1),
        );
        assert!(!editor.sound_is_plain());
        assert_eq!(editor.export_route(), Route::Fast);

        let output = output_called("music");
        let report = exported_report(&ctx, &mut editor, &output);

        let ProjectReport::Fast(report) = report else {
            panic!("it should have copied the picture");
        };
        assert_eq!(report.encoded_frames, 0);
        assert_eq!(report.audio_tracks, 1);
        let info = probe(&output).expect("the export can be read");
        assert_eq!(info.audio.len(), 1);
        assert_eq!(info.audio[0].codec, "aac");
        assert!((info.duration.as_seconds() - 8.0).abs() < 0.2);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn new_sound_only_offers_exact_cuts_and_uses_them_whatever_was_picked_before() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound, song_file()]);
        editor.mode = CutMode::Keyframe;
        let clip = picture_clip(&editor);
        // A cut away from any keyframe, which Fastest would have moved.
        editor.project.tracks.trim(
            clip.id,
            ClipEdge::End,
            Ticks::from_seconds(4.3),
            Ticks::from_seconds(8.0),
            Ticks(1),
        );
        let song = sound_row_clip(&editor);
        editor.project.tracks.trim(
            song.id,
            ClipEdge::End,
            Ticks::from_seconds(4.3),
            Ticks::from_seconds(12.0),
            Ticks(1),
        );
        editor.export_state = ExportState::Choosing;
        let_dialog_open(&ctx, &mut editor);
        assert!(!is_named(&ctx, &mut editor, "Fastest"));
        assert!(!is_named(&ctx, &mut editor, "Frame-exact"));

        let output = output_called("exact-for-new-sound");
        let report = exported_report(&ctx, &mut editor, &output);

        let ProjectReport::Fast(report) = report else {
            panic!("it should have copied the picture");
        };
        assert_eq!(report.mode_used, CutMode::FrameExact);
        assert!(report.encoded_frames > 0, "the cut was made exactly");
        let info = probe(&output).expect("it can be read");
        assert!(
            (info.duration.as_seconds() - 4.3).abs() < 0.1,
            "{}",
            info.duration.as_seconds()
        );
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn sound_nobody_changed_still_offers_fastest() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        editor.export_state = ExportState::Choosing;
        let_dialog_open(&ctx, &mut editor);

        assert!(is_named(&ctx, &mut editor, "Fastest"));
    }

    #[test]
    fn sound_kept_apart_comes_out_on_tracks_of_its_own() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound, song_file()]);
        let clip = picture_clip(&editor);
        let song = sound_row_clip(&editor);
        editor.project.tracks.trim(
            song.id,
            ClipEdge::End,
            clip.end(),
            Ticks::from_seconds(12.0),
            Ticks(1),
        );
        assert!(editor.can_keep_sound_apart());
        assert_eq!(editor.sound_plan().tracks.len(), 1, "one mix by default");
        editor.keep_sound_apart = true;
        assert_eq!(editor.sound_plan().tracks.len(), 2);

        let output = output_called("apart");
        exported_report(&ctx, &mut editor, &output);

        assert_eq!(probe(&output).expect("it can be read").audio.len(), 2);
        let _ = std::fs::remove_file(&output);
    }

    /// An image of a given size and colour, made once for each name.
    fn image_file(name: &str, size: &str, colour: &str) -> PathBuf {
        let folder = std::env::temp_dir().join("bareclip-app-tests");
        std::fs::create_dir_all(&folder).expect("the folder can be created");
        let path = folder.join(format!("{name}-{}.png", std::process::id()));
        if !path.exists() {
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
                .arg(format!("color={colour}:s={size},format=rgba"))
                .args(["-frames:v", "1"])
                .arg(&path)
                .status()
                .expect("ffmpeg is on PATH");
            assert!(status.success());
        }
        path
    }

    fn times(factor: f64) -> Speed {
        Speed::default().with_factor(factor)
    }

    /// Changes the selected clip's speed the way the toolbar does.
    fn speed_change(id: ClipId, speed: Speed) -> stage::StageEvents {
        stage::StageEvents {
            speed: Some((id, speed)),
            ..stage::StageEvents::default()
        }
    }

    #[test]
    fn the_rail_offers_speed_on_a_video_and_a_press_changes_it_as_one_undo_step() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        let clip = picture_clip(&editor);
        editor.selected = Some(clip.id);
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Speed");
        let_dialog_open(&ctx, &mut editor);

        click_named(&ctx, &mut editor, "2x");
        run_frame(&ctx, &mut editor, vec![]);
        run_frame(&ctx, &mut editor, vec![]);

        assert_eq!(picture_clip(&editor).speed, times(2.0));
        assert_eq!(
            picture_clip(&editor).duration(),
            Ticks(clip.duration().0 / 2)
        );
        editor.undo();
        assert_eq!(picture_clip(&editor).speed, Speed::default());
        assert_eq!(picture_clip(&editor).duration(), clip.duration());
    }

    #[test]
    fn the_quick_speeds_are_two_even_rows_of_four_up_to_the_limit() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        let clip = picture_clip(&editor);
        editor.selected = Some(clip.id);
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Speed");
        let_dialog_open(&ctx, &mut editor);

        let places: Vec<egui::Rect> = ["0.25x", "0.5x", "1x", "1.5x", "2x", "4x", "8x", "16x"]
            .map(|name| named_rect(&ctx, &mut editor, name))
            .to_vec();
        for row in places.chunks(4) {
            assert!(
                row.iter()
                    .all(|place| (place.top() - row[0].top()).abs() < 0.5)
            );
            assert!(
                row.iter()
                    .all(|place| (place.width() - row[0].width()).abs() < 0.5)
            );
            assert!(row.windows(2).all(|pair| pair[0].right() < pair[1].left()));
        }
        assert!(places[0].bottom() <= places[4].top());
        for column in 0..4 {
            assert!((places[column].left() - places[column + 4].left()).abs() < 0.5);
        }

        click_named(&ctx, &mut editor, "16x");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(picture_clip(&editor).speed.as_factor(), 16.0);
    }

    #[test]
    fn keep_pitch_starts_on_and_the_menu_turns_it_off() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        editor.selected = Some(picture_clip(&editor).id);
        run_frame(&ctx, &mut editor, vec![]);
        assert!(picture_clip(&editor).speed.keep_pitch);
        click_named(&ctx, &mut editor, "Speed");
        let_dialog_open(&ctx, &mut editor);

        click_named(&ctx, &mut editor, "Keep pitch");
        run_frame(&ctx, &mut editor, vec![]);

        assert!(!picture_clip(&editor).speed.keep_pitch);
    }

    #[test]
    fn a_clip_with_no_sound_has_no_pitch_to_keep_and_an_image_has_no_speed() {
        let ctx = context();
        let image = image_file("no-speed", "64x36", "white");
        let silent = test_clip().to_path_buf();
        let mut editor = with_videos(&ctx, &[&silent, &image]);
        let clips = editor.project.tracks.picture_tracks()[0].clips().to_vec();

        editor.selected = Some(clips[0].id);
        assert!(
            editor
                .selected_speed()
                .is_some_and(|speed| !speed.has_sound)
        );
        editor.selected = Some(clips[1].id);
        assert!(editor.selected_speed().is_none());
    }

    #[test]
    fn one_slider_drag_is_one_undo_and_coming_back_puts_later_clips_back() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound, &sound]);
        let clips = editor.project.tracks.picture_tracks()[0].clips().to_vec();
        let (first, second) = (clips[0], clips[1]);
        let can_undo_before = editor.history.can_undo();

        editor.apply_stage_events(speed_change(first.id, times(0.5)), true);
        let pushed = editor.project.tracks.clip(second.id).expect("it is there");
        assert_eq!(pushed.start, Ticks(first.duration().0 * 2));

        editor.apply_stage_events(speed_change(first.id, times(0.75)), true);
        editor.apply_stage_events(speed_change(first.id, times(1.0)), true);
        let back = editor.project.tracks.clip(second.id).expect("it is there");
        assert_eq!(back.start, second.start, "it goes back where it was");

        editor.apply_stage_events(speed_change(first.id, times(2.0)), true);
        editor.apply_stage_events(stage::StageEvents::default(), false);
        assert_eq!(
            editor.project.tracks.clip(first.id).map(|clip| clip.speed),
            Some(times(2.0))
        );
        assert_eq!(
            editor.project.tracks.clip(second.id).map(|clip| clip.start),
            Some(second.start),
            "speeding up leaves dead space and moves nothing"
        );
        editor.undo();
        assert_eq!(
            editor.project.tracks.clip(first.id).map(|clip| clip.speed),
            Some(Speed::default())
        );
        assert_eq!(editor.history.can_undo(), can_undo_before);
    }

    #[test]
    fn a_video_at_another_speed_is_redrawn_says_why_and_comes_out_shorter_with_its_sound() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        let clip = picture_clip(&editor);
        editor.project.tracks.set_speed(clip.id, times(2.0));

        assert_eq!(editor.export_route(), Route::Full(SPEED_REASON.to_owned()));
        let output = output_called("video-at-speed");
        let report = exported_report(&ctx, &mut editor, &output);

        assert!(matches!(report, ProjectReport::Full(_)), "{report:?}");
        let info = probe(&output).expect("it can be read");
        assert!(
            (info.duration.as_seconds() - 4.0).abs() < 0.2,
            "{:?}",
            info.duration
        );
        assert_eq!(info.audio.len(), 1);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn music_at_another_speed_keeps_the_picture_copied() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound, song_file()]);
        let clip = picture_clip(&editor);
        let song = sound_row_clip(&editor);
        editor.project.tracks.set_speed(song.id, times(2.0));
        let song = sound_row_clip(&editor);
        assert!(song.end() <= clip.end());

        assert_eq!(editor.export_route(), Route::Fast);
        let output = output_called("music-at-speed");
        let report = exported_report(&ctx, &mut editor, &output);

        assert!(matches!(report, ProjectReport::Fast(_)), "{report:?}");
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn a_fast_clip_plays_through_in_its_shorter_time_and_stops_at_the_end() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        let clip = picture_clip(&editor);
        editor.project.tracks.set_speed(clip.id, times(8.0));
        run_frame(&ctx, &mut editor, vec![]);
        let length = picture_clip(&editor).duration();
        assert_eq!(length, Ticks::from_seconds(1.0));

        editor.toggle_playback();
        let started = Instant::now();
        while editor.preview.is_playing() {
            assert!(
                started.elapsed() < Duration::from_secs(6),
                "it never finished"
            );
            run_frame(&ctx, &mut editor, vec![]);
            thread::sleep(Duration::from_millis(5));
        }

        assert_eq!(editor.position(), length);
    }

    #[test]
    fn splitting_a_fast_clip_at_the_playhead_cuts_the_footage_that_is_playing_there() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        let clip = picture_clip(&editor);
        editor.project.tracks.set_speed(clip.id, times(2.0));
        editor.seek(Ticks::from_seconds(1.0));
        editor.selected = Some(clip.id);
        run_frame(&ctx, &mut editor, vec![]);

        editor.split_at_playhead();

        let clips = editor.project.tracks.picture_tracks()[0].clips();
        assert_eq!(clips.len(), 2);
        let cut = clips[0].source.end.as_seconds();
        assert!((cut - 2.0).abs() < 0.05, "it cut the footage at {cut} s");
        assert_eq!(clips[1].speed, times(2.0));
        assert_eq!(clips[0].end(), clips[1].start);
    }

    #[test]
    fn a_clips_speed_survives_saving_and_opening() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        let wanted = Speed {
            keep_pitch: false,
            ..times(0.5)
        };
        editor
            .project
            .tracks
            .set_speed(picture_clip(&editor).id, wanted);

        let folder = project_folder("speed");
        let path = folder.join("speed.bareclip");
        assert!(editor.save_to(&path));
        let mut reopened = Editor::new(&ctx, false);
        reopened.open_project(&ctx, &path);
        settle(&ctx, &mut reopened);

        assert_eq!(picture_clip(&reopened).speed, wanted);
        assert_eq!(
            picture_clip(&reopened).duration(),
            picture_clip(&editor).duration()
        );
    }

    #[test]
    fn a_new_image_goes_on_a_picture_row_for_four_seconds_with_its_own_kind_of_card() {
        let ctx = context();
        let image = image_file("card", "640x480", "red");
        let editor = with_videos(&ctx, &[&image]);

        let clip = picture_clip(&editor);
        assert!(clip.is_still());
        assert_eq!(clip.duration(), Ticks::from_seconds(4.0));
        assert!(editor.project.tracks.audio_tracks().is_empty());
        let entry = &editor.media[0];
        assert!(
            entry.thumbnail.is_some(),
            "its picture is ready for its card"
        );
        assert!(entry.still.is_some());
        assert_eq!(entry.strip_pictures.len(), 1, "and for its clips");
        assert!(entry.index.is_none(), "it has no frames to look through");
        let details: Vec<String> = card_details(entry)
            .into_iter()
            .map(|(line, _)| line)
            .collect();
        assert!(
            details.contains(&"PNG, 640 x 480".to_owned()),
            "{details:?}"
        );
    }

    #[test]
    fn filter_tells_images_from_videos_and_sound() {
        let ctx = context();
        let image = image_file("filter", "64x36", "red");
        let sound = clip_with_sound();
        let mut editor = Editor::new(&ctx, false);
        editor.import(&ctx, &[image, sound, song_file().to_path_buf()]);
        settle(&ctx, &mut editor);

        let shown = |editor: &mut Editor, filter| {
            editor.media_filter = filter;
            editor.listed_media().len()
        };
        assert_eq!(shown(&mut editor, MediaFilter::All), 3);
        assert_eq!(shown(&mut editor, MediaFilter::Images), 1);
        assert_eq!(shown(&mut editor, MediaFilter::Videos), 1);
        assert_eq!(shown(&mut editor, MediaFilter::Audio), 1);
    }

    #[test]
    fn an_image_doesnt_set_the_projects_size_but_the_first_video_does() {
        let ctx = context();
        let image = image_file("size", "4032x3024", "red");
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&image]);
        assert_eq!(editor.project.canvas.size(), (1920, 1080));
        assert_eq!(editor.project.canvas.name(), "16:9");

        editor.import(&ctx, &[sound]);
        settle(&ctx, &mut editor);
        let video = editor.media[1].id;
        editor.add_to_timeline(video);
        run_frame(&ctx, &mut editor, vec![]);

        let info = &editor.media[1].info;
        assert_eq!(
            editor.project.canvas.size(),
            (info.video.width, info.video.height)
        );
    }

    #[test]
    fn a_ratio_picked_by_hand_stays_when_only_an_image_is_on_the_timeline() {
        let ctx = context();
        let image = image_file("ratio", "640x480", "red");
        let mut editor = with_videos(&ctx, &[&image]);

        editor.choose_ratio(Some((9, 16)));
        run_frame(&ctx, &mut editor, vec![]);

        let (width, height) = editor.project.canvas.size();
        assert!(height > width);
    }

    #[test]
    fn an_image_can_be_stretched_split_and_moved_like_any_clip() {
        let ctx = context();
        let image = image_file("stretch", "64x36", "red");
        let mut editor = with_videos(&ctx, &[&image]);
        let clip = picture_clip(&editor);
        let length = editor.media[0].info.duration;

        editor.project.tracks.trim(
            clip.id,
            ClipEdge::End,
            Ticks(clip.source.start.0 + Ticks::from_seconds(600.0).0),
            length,
            Ticks(1),
        );
        assert_eq!(picture_clip(&editor).duration(), Ticks::from_seconds(600.0));

        let second = editor
            .project
            .tracks
            .split(
                clip.id,
                Ticks(clip.source.start.0 + Ticks::from_seconds(10.0).0),
            )
            .expect("it splits");
        let halves = editor.project.tracks.picture_tracks()[0].clips();
        assert_eq!(halves.len(), 2);
        assert!(halves.iter().all(|half| half.is_still()));
        assert_eq!(
            editor.project.tracks.clip(second).map(|clip| clip.start),
            Some(Ticks::from_seconds(10.0))
        );
    }

    #[test]
    fn an_image_takes_the_full_route_and_says_why_with_no_frames_to_wait_for() {
        let ctx = context();
        let image = image_file("route", "320x180", "white");
        let mut editor = with_videos(&ctx, &[&image]);

        assert_eq!(editor.export_route(), Route::Full(STILLS_REASON.to_owned()));
        let output = output_called("only-image");
        let report = exported_report(&ctx, &mut editor, &output);

        assert!(matches!(report, ProjectReport::Full(_)), "{report:?}");
        let info = probe(&output).expect("it can be read");
        assert_eq!((info.video.width, info.video.height), (1920, 1080));
        assert!(info.audio.is_empty());
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn an_image_over_a_video_keeps_the_videos_sound_all_the_way_through() {
        let ctx = context();
        let image = image_file("over", "64x36", "white");
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        editor.import(&ctx, &[image]);
        settle(&ctx, &mut editor);
        let still = editor.media[1].id;
        editor.drop_on_timeline(still, Ticks(0), TrackTarget::New(0));
        run_frame(&ctx, &mut editor, vec![]);
        let video = editor
            .project
            .tracks
            .picture_tracks()
            .iter()
            .flat_map(|row| row.clips())
            .find(|clip| !clip.is_still())
            .copied()
            .expect("the video is still there");

        let plan = editor.sound_plan();

        assert_eq!(plan.spans.len(), 1);
        assert_eq!(
            plan.spans[0].source, video.source,
            "sound doesn't stop under the image"
        );
        assert_eq!(plan.tracks.len(), 1);
        let output = output_called("image-over-sound");
        exported_report(&ctx, &mut editor, &output);
        assert_eq!(probe(&output).expect("it can be read").audio.len(), 1);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn a_silent_overlay_doesnt_stop_the_sound_of_the_video_under_it() {
        let ctx = context();
        let silent = test_clip().to_path_buf();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        editor.import(&ctx, &[silent]);
        settle(&ctx, &mut editor);
        let overlay = editor.media[1].id;
        assert!(!editor.has_sound(overlay));
        editor.drop_on_timeline(overlay, Ticks(0), TrackTarget::New(0));
        run_frame(&ctx, &mut editor, vec![]);

        let plan = editor.sound_plan();

        assert_eq!(plan.spans.len(), 1);
        assert_eq!(plan.spans[0].start, Ticks(0));
        assert_eq!(
            plan.spans[0].source.duration(),
            editor.media[0].info.duration
        );
    }

    #[test]
    fn a_project_of_only_images_plays_through_to_its_end() {
        let ctx = context();
        let image = image_file("plays", "64x36", "white");
        let mut editor = with_videos(&ctx, &[&image]);
        let clip = picture_clip(&editor);
        editor.project.tracks.trim(
            clip.id,
            ClipEdge::End,
            Ticks(clip.source.start.0 + Ticks::from_seconds(0.3).0),
            editor.media[0].info.duration,
            Ticks(1),
        );

        editor.toggle_playback();
        assert!(editor.preview.is_playing());
        let started = Instant::now();
        while editor.preview.is_playing() {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "it never finished"
            );
            run_frame(&ctx, &mut editor, vec![]);
            thread::sleep(Duration::from_millis(5));
        }

        assert_eq!(editor.position(), Ticks::from_seconds(0.3));
    }

    #[test]
    fn a_video_plays_on_after_an_image_ends_and_an_image_after_a_video_ends() {
        let ctx = context();
        let image = image_file("after", "64x36", "white");
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound, &image]);
        let video_end = editor.media[0].info.duration;
        assert_eq!(editor.project.tracks.played_videos().duration(), video_end);
        assert_eq!(
            editor.project.tracks.duration(),
            Ticks(video_end.0 + Ticks::from_seconds(4.0).0)
        );

        editor.seek(Ticks(video_end.0 + Ticks::from_seconds(1.0).0));
        run_frame(&ctx, &mut editor, vec![]);

        assert!(editor.preview.in_gap(editor.project.tracks.played_videos()));
        assert_eq!(
            editor.position(),
            Ticks(video_end.0 + Ticks::from_seconds(1.0).0)
        );
    }

    #[test]
    fn an_image_has_no_sound_controls() {
        let ctx = context();
        let image = image_file("no-sound", "64x36", "white");
        let mut editor = with_videos(&ctx, &[&image]);
        editor.selected = Some(picture_clip(&editor).id);

        assert!(editor.selected_sound().is_none());
    }

    #[test]
    fn an_image_clip_survives_saving_and_opening() {
        let ctx = context();
        let folder = std::env::temp_dir().join("bareclip-app-tests");
        std::fs::create_dir_all(&folder).expect("the folder can be created");
        let path = folder.join(format!("image-project-{}.bareclip", std::process::id()));
        let image = image_file("saved", "64x36", "white");
        let mut editor = with_videos(&ctx, &[&image]);
        let clip = picture_clip(&editor);
        assert!(editor.save_to(&path));

        let mut reopened = Editor::new(&ctx, false);
        reopened.open_project(&ctx, &path);
        settle(&ctx, &mut reopened);
        run_frame(&ctx, &mut reopened, vec![]);

        let back = picture_clip(&reopened);
        assert!(back.is_still());
        assert_eq!(back.source, clip.source);
        assert_eq!(back.start, clip.start);
        assert!(reopened.media[0].still.is_some());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn choosing_anything_but_match_source_redraws_and_going_back_copies_again() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        assert_eq!(editor.export_route(), Route::Fast);

        for choices in [
            ExportChoices {
                size: ExportSize::Shorter(480),
                ..ExportChoices::default()
            },
            ExportChoices {
                quality: Quality::Smaller,
                ..ExportChoices::default()
            },
            ExportChoices {
                rate: ExportRate::Fps(24),
                ..ExportChoices::default()
            },
        ] {
            editor.export_choices = choices;
            assert_eq!(
                editor.export_route(),
                Route::Full(CHOICES_REASON.to_owned())
            );
        }

        editor.export_choices = ExportChoices::default();
        assert_eq!(editor.export_route(), Route::Fast);
    }

    #[test]
    fn choosing_av1_or_gif_redraws_the_video() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);

        for format in [VideoFormat::Av1, VideoFormat::Gif] {
            editor.export_choices.format = format;
            assert_eq!(
                editor.export_route(),
                Route::Full(CHOICES_REASON.to_owned())
            );
        }
    }

    #[test]
    fn a_gif_uses_its_own_size_and_rate_and_a_gif_file_name() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        let canvas = editor.project.canvas.size();
        editor.export_choices.format = VideoFormat::Gif;
        editor.export_choices.gif_size = 120;
        editor.export_choices.gif_rate = 10;

        assert_eq!(editor.export_size(), Some(gif_size_for(canvas, 120)));
        assert_eq!(editor.export_plan().settings.frame_rate, (10, 1));
        let output = editor
            .suggested_output()
            .expect("there's a file to suggest");
        assert_eq!(output.extension().unwrap(), "gif");
    }

    #[test]
    fn a_gif_comes_out_silent_at_what_was_chosen() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        let canvas = editor.project.canvas.size();
        editor.export_choices.format = VideoFormat::Gif;
        editor.export_choices.gif_size = 120;
        editor.export_choices.gif_rate = 10;
        let length = editor.gif_seconds();

        let output = output_called("as-a-gif").with_extension("gif");
        let report = exported_report(&ctx, &mut editor, &output);

        assert!(matches!(report, ProjectReport::Full(_)), "{report:?}");
        let info = probe(&output).expect("it can be read");
        assert_eq!(info.video.codec, "gif");
        assert_eq!(
            (info.video.width, info.video.height),
            gif_size_for(canvas, 120)
        );
        assert!(info.audio.is_empty(), "a GIF has no sound");
        // BareClip reads a GIF as a picture when adding one, so its frames are counted here.
        let mut reader = bareclip_media::PictureReader::open(&output).expect("it opens");
        let (mut frames, mut last) = (0, Ticks(0));
        while let Some(picture) = reader.next_picture().expect("it decodes") {
            frames += 1;
            last = picture.time();
        }
        assert_eq!(
            frames,
            (length * 10.0).round() as i32,
            "one for each tenth of a second"
        );
        assert!(
            (last.as_seconds() - (length - 0.1)).abs() < 0.05,
            "{last:?}"
        );
        let bytes = std::fs::read(&output).expect("it can be read");
        let loops = bytes.windows(11).any(|part| part == b"NETSCAPE2.0");
        assert!(loops, "it says to loop");
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn av1_comes_out_as_av1_at_the_chosen_size() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        let canvas = editor.project.canvas.size();
        let wanted = size_with_shorter_side(canvas, canvas.0.min(canvas.1) / 2);
        editor.export_choices.format = VideoFormat::Av1;
        editor.export_choices.size = ExportSize::Shorter(canvas.0.min(canvas.1) / 2);

        let output = output_called("as-av1");
        let report = exported_report(&ctx, &mut editor, &output);

        assert!(matches!(report, ProjectReport::Full(_)), "{report:?}");
        let info = probe(&output).expect("it can be read");
        assert_eq!(info.video.codec, "av1");
        assert_eq!((info.video.width, info.video.height), wanted);
        assert!(!info.audio.is_empty(), "the sound comes along");
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn the_export_window_warns_about_a_long_gif_only() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        editor.open_export();
        editor.export_choices.format = VideoFormat::Gif;
        let_dialog_open(&ctx, &mut editor);
        let warning = text::gif_length_warning(editor.gif_seconds());
        let output = run_frame(&ctx, &mut editor, vec![]);
        assert!(!text_in(&output, egui::Rect::EVERYTHING).contains(&warning));

        let id = editor.media[0].id;
        while editor.gif_seconds() <= GIF_WARNING_SECONDS {
            editor.add_to_timeline(id);
        }
        let warning = text::gif_length_warning(editor.gif_seconds());
        let output = run_frame(&ctx, &mut editor, vec![]);
        assert!(text_in(&output, egui::Rect::EVERYTHING).contains(&warning));
    }

    #[test]
    fn the_export_window_starts_on_match_source_every_time() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        editor.export_choices.quality = Quality::Smaller;
        editor.sound_only = true;

        editor.open_export();

        assert_eq!(editor.export_choices, ExportChoices::default());
        assert!(!editor.sound_only);
    }

    #[test]
    fn a_chosen_resolution_sets_the_finished_size_without_changing_the_project() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        let canvas = editor.project.canvas.size();
        editor.export_choices.size = ExportSize::Shorter(240);

        let wanted = size_with_shorter_side(canvas, 240);
        assert_eq!(editor.export_size(), Some(wanted));
        assert_eq!(editor.export_plan().settings.width, wanted.0);
        assert_eq!(editor.project.canvas.size(), canvas);
    }

    #[test]
    fn a_redraw_for_a_chosen_quality_comes_out_with_what_was_chosen() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        let canvas = editor.project.canvas.size();
        let wanted = size_with_shorter_side(canvas, canvas.0.min(canvas.1) / 2);
        editor.export_choices = ExportChoices {
            size: ExportSize::Shorter(canvas.0.min(canvas.1) / 2),
            quality: Quality::Balanced,
            rate: ExportRate::Fps(15),
            ..ExportChoices::default()
        };

        let output = output_called("chosen-choices");
        let report = exported_report(&ctx, &mut editor, &output);

        assert!(matches!(report, ProjectReport::Full(_)), "{report:?}");
        let info = probe(&output).expect("it can be read");
        assert_eq!((info.video.width, info.video.height), wanted);
        assert!((info.video.frame_rate - 15.0).abs() < 0.5);
        assert!(!info.audio.is_empty(), "the sound comes along");
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn sound_only_saves_what_can_be_heard_as_one_mix() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound, song_file()]);
        editor.keep_sound_apart = true;
        editor.sound_only = true;
        assert_eq!(editor.sound_plan().tracks.len(), 1);

        let output = output_called("sound-only").with_extension("m4a");
        let report = exported_report(&ctx, &mut editor, &output);

        assert!(matches!(report, ProjectReport::Sound(_)), "{report:?}");
        let info = probe(&output).expect("it can be read");
        assert!(info.is_sound_only());
        assert_eq!(info.audio.len(), 1);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn sound_only_suggests_a_sound_file_named_after_the_project() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        editor.sound_only = true;

        let suggested = editor
            .suggested_output()
            .expect("there is something to save");

        assert_eq!(
            suggested.extension().and_then(|ext| ext.to_str()),
            Some("m4a")
        );
    }

    #[test]
    fn the_export_window_offers_each_choice_by_name() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        editor.open_export();
        run_frame(&ctx, &mut editor, vec![]);

        click_named(&ctx, &mut editor, "Quality: Match source");
        let_dialog_open(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Smaller file");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.export_choices.quality, Quality::Smaller);

        click_named(&ctx, &mut editor, "Sound only");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.sound_only);
    }

    #[test]
    fn the_export_window_only_offers_keeping_sound_apart_when_there_is_something_to_keep_apart() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut mixed = with_videos(&ctx, &[&sound, song_file()]);
        mixed.export_state = ExportState::Choosing;
        let_dialog_open(&ctx, &mut mixed);
        assert!(is_named(&ctx, &mut mixed, "One mix"));
        assert!(is_named(&ctx, &mut mixed, "Keep separate"));

        let mut plain = with_videos(&ctx, &[&sound]);
        plain.export_state = ExportState::Choosing;
        let_dialog_open(&ctx, &mut plain);
        assert!(!is_named(&ctx, &mut plain, "Keep separate"));
    }

    #[test]
    fn opening_the_export_window_goes_back_to_one_mix() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.keep_sound_apart = true;

        editor.open_export();

        assert!(!editor.keep_sound_apart);
    }

    #[test]
    fn sound_nobody_changed_is_still_copied_exactly() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        assert!(editor.sound_is_plain());
        assert_eq!(editor.export_route(), Route::Fast);

        let output = output_called("plain-sound");
        let report = exported_report(&ctx, &mut editor, &output);

        assert!(matches!(report, ProjectReport::Fast(_)));
        assert_eq!(probe(&output).expect("it can be read").audio.len(), 1);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn picking_copy_really_copies_videos_at_different_rates() {
        let ctx = context();
        let slow = clip_at_rate("really-24", 24);
        let fast = clip_at_rate("really-60", 60);
        let mut editor = with_videos(&ctx, &[&slow, &fast]);
        editor.rate_pick = Some(RatePick::Copy);

        let output = output_called("really-copied");
        let report = exported_report(&ctx, &mut editor, &output);

        assert!(matches!(report, ProjectReport::Fast(_)), "{report:?}");
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn music_that_outlasts_the_video_takes_the_full_route_and_says_why() {
        let ctx = context();
        let editor = with_videos(&ctx, &[test_clip(), song_file()]);

        assert_eq!(
            editor.export_route(),
            Route::Full(SOUND_OUTLASTS_REASON.to_owned())
        );
    }

    #[test]
    fn a_project_of_nothing_but_sound_exports_as_its_background() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[song_file()]);
        let song = sound_row_clip(&editor);
        editor.project.tracks.trim(
            song.id,
            ClipEdge::End,
            Ticks::from_seconds(1.0),
            Ticks::from_seconds(12.0),
            Ticks(1),
        );
        assert_eq!(
            editor.export_route(),
            Route::Full(ONLY_SOUND_REASON.to_owned())
        );

        let output = output_called("only-sound");
        let report = exported_report(&ctx, &mut editor, &output);

        assert!(matches!(report, ProjectReport::Full(_)));
        let info = probe(&output).expect("it can be read");
        assert!((info.duration.as_seconds() - 1.0).abs() < 0.2);
        assert_eq!(info.audio.len(), 1);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn a_kind_of_file_that_cant_hold_new_sound_becomes_an_mp4() {
        assert!(CONTAINERS_FOR_NEW_SOUND.contains(&"mkv"));
        assert!(!CONTAINERS_FOR_NEW_SOUND.contains(&"webm"));
    }

    #[test]
    fn the_rail_offers_audio_only_for_a_clip_that_has_some() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut with_sound = with_videos(&ctx, &[&sound]);
        run_frame(&ctx, &mut with_sound, vec![]);
        assert!(is_named(&ctx, &mut with_sound, "Audio"));

        let mut silent = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut silent, vec![]);
        assert!(!is_named(&ctx, &mut silent, "Audio"));
    }

    #[test]
    fn a_clip_of_nothing_but_sound_gets_its_tools_on_the_rail() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[song_file()]);
        run_frame(&ctx, &mut editor, vec![]);

        assert!(is_named(&ctx, &mut editor, "Audio"));
    }

    #[test]
    fn dragging_volume_is_one_undo_step_for_as_long_as_the_pointer_is_held() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        let id = picture_clip(&editor).id;
        let level = |volume: u32| stage::StageEvents {
            sound: Some((
                id,
                ClipSound {
                    volume,
                    ..ClipSound::default()
                },
            )),
            ..Default::default()
        };

        editor.apply_stage_events(level(800_000), true);
        editor.apply_stage_events(level(400_000), true);
        editor.apply_stage_events(stage::StageEvents::default(), false);
        assert_eq!(picture_clip(&editor).sound.volume, 400_000);

        editor.undo();
        assert!(picture_clip(&editor).sound.is_default());
    }

    #[test]
    fn detach_sound_from_the_audio_panel_makes_a_clip_in_time_and_one_undo_puts_it_back() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        run_frame(&ctx, &mut editor, vec![]);

        click_named(&ctx, &mut editor, "Audio");
        let_dialog_open(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Detach sound");

        let clip = picture_clip(&editor);
        let detached = sound_row_clip(&editor);
        assert!(clip.sound.muted);
        assert_eq!(detached.start, clip.start);
        assert_eq!(detached.source, clip.source);
        assert_eq!(detached.sound.only_track, Some(0));
        assert_eq!(editor.project.tracks.sound_spans(&|_| true).len(), 1);

        editor.undo();
        assert!(editor.project.tracks.audio_tracks().is_empty());
        assert!(!picture_clip(&editor).sound.muted);
    }

    #[test]
    fn noise_suppression_switches_on_from_the_audio_panel_for_detached_sound_as_one_undo_step() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        editor.detach_sound(picture_clip(&editor).id);
        editor.selected = Some(sound_row_clip(&editor).id);
        run_frame(&ctx, &mut editor, vec![]);

        click_named(&ctx, &mut editor, "Audio");
        let_dialog_open(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Noise suppression");
        run_frame(&ctx, &mut editor, vec![]);

        assert!(sound_row_clip(&editor).sound.noise_suppression);
        assert!(!picture_clip(&editor).sound.noise_suppression);
        editor.undo();
        assert!(!sound_row_clip(&editor).sound.noise_suppression);
        assert_eq!(
            editor.project.tracks.audio_tracks().len(),
            1,
            "only the switch was undone"
        );
    }

    #[test]
    fn a_videos_own_sound_shows_noise_suppression_greyed_out() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        run_frame(&ctx, &mut editor, vec![]);

        click_named(&ctx, &mut editor, "Audio");
        let_dialog_open(&ctx, &mut editor);
        let switch = named_nodes(&ctx, &mut editor)
            .into_iter()
            .find(|(label, ..)| label == "Noise suppression")
            .expect("the switch is there");
        assert!(switch.2, "it can't be turned on");
        click_named(&ctx, &mut editor, "Noise suppression");
        run_frame(&ctx, &mut editor, vec![]);

        assert!(!picture_clip(&editor).sound.noise_suppression);
        assert!(
            editor
                .selected_sound()
                .is_some_and(|sound| !sound.can_clean)
        );
    }

    #[test]
    fn a_clip_already_detached_has_nothing_left_to_detach() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        let id = picture_clip(&editor).id;
        assert!(
            editor
                .selected_sound()
                .is_some_and(|sound| sound.can_detach)
        );

        editor.detach_sound(id);

        editor.selected = Some(id);
        assert!(
            !editor
                .selected_sound()
                .is_some_and(|sound| sound.can_detach)
        );
        editor.selected = Some(sound_row_clip(&editor).id);
        assert!(
            !editor
                .selected_sound()
                .is_some_and(|sound| sound.can_detach)
        );
    }

    #[test]
    fn a_sound_clip_can_be_split_and_trimmed_like_any_other() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[song_file()]);
        editor.selected = Some(sound_row_clip(&editor).id);
        editor.seek(Ticks::from_seconds(5.0));

        editor.split_at_playhead();

        let row = &editor.project.tracks.audio_tracks()[0];
        assert_eq!(row.clips().len(), 2);
        assert_eq!(row.clips()[1].start, Ticks::from_seconds(5.0));
    }

    #[test]
    fn sound_and_its_settings_survive_saving_and_opening() {
        let ctx = context();
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound, song_file()]);
        let song = sound_row_clip(&editor);
        let changed = ClipSound {
            volume: 600_000,
            fade_in: Ticks::from_seconds(1.0).0,
            ..ClipSound::default()
        };
        editor.project.tracks.set_clip_sound(song.id, changed);

        let folder = project_folder("sound-settings");
        let path = folder.join("sound.bareclip");
        assert!(editor.save_to(&path));
        let mut reopened = Editor::new(&ctx, false);
        reopened.open_project(&ctx, &path);
        settle(&ctx, &mut reopened);

        assert_eq!(reopened.project.tracks.audio_tracks().len(), 1);
        assert_eq!(sound_row_clip(&reopened).sound, changed);
    }

    /// Draws the speed controls on the real screen, with no window. Run with
    /// `$env:BARECLIP_SCREENS = '<folder>'; ./scripts/cargo.ps1 test -p bareclip-app draw_the_speed_screens '--' --ignored`
    #[test]
    #[ignore = "needs a real GPU, and draws pictures to look at rather than checking anything"]
    fn draw_the_speed_screens() {
        let folder = std::env::var_os("BARECLIP_SCREENS").map_or_else(
            || std::env::temp_dir().join("bareclip-screens"),
            PathBuf::from,
        );
        let ctx = context();
        let mut camera = Camera::new(folder);
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound, &sound]);
        editor.use_gpu(LayerDisplay::new(
            camera.device.clone(),
            camera.queue.clone(),
            camera.renderer.clone(),
        ));
        let first = picture_clip(&editor);
        editor.project.tracks.set_speed(first.id, times(2.0));
        editor.selected = Some(first.id);
        editor.seek(Ticks::from_seconds(1.0));
        for _ in 0..40 {
            run_frame(&ctx, &mut editor, vec![]);
            thread::sleep(Duration::from_millis(10));
        }
        camera.shoot(&ctx, &mut editor, vec![], "speed-01-faster-clip");

        click_named(&ctx, &mut editor, "Speed");
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "speed-02-menu");
        click(&ctx, &mut editor, pos2(640.0, 60.0));

        open_clip_menu(&ctx, &mut editor);
        open_flyout(&ctx, &mut editor, "Speed");
        camera.shoot(&ctx, &mut editor, vec![], "speed-03-right-click");
        open_flyout(&ctx, &mut editor, "Audio");
        camera.shoot(&ctx, &mut editor, vec![], "speed-04-right-click-audio");
        click(&ctx, &mut editor, pos2(640.0, 60.0));

        // The name of a video after the pointer has rested on it.
        let over = pos2(
            editor.timeline_state.scale.x_of(Ticks::from_seconds(1.0)),
            editor.timeline_state.track.center().y,
        );
        run_frame(&ctx, &mut editor, moved(over));
        for _ in 0..90 {
            run_frame(&ctx, &mut editor, vec![]);
        }
        camera.shoot(&ctx, &mut editor, vec![], "speed-05-name-on-hover");
        click_named(
            &ctx,
            &mut editor,
            &text::project_title(project::DEFAULT_NAME, true),
        );
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "speed-06-renaming");
    }

    /// Draws images on the real screen, with no window. Run with
    /// `$env:BARECLIP_SCREENS = '<folder>'; ./scripts/cargo.ps1 test -p bareclip-app draw_the_image_screens '--' --ignored`
    #[test]
    #[ignore = "needs a real GPU, and draws pictures to look at rather than checking anything"]
    fn draw_the_image_screens() {
        let folder = std::env::var_os("BARECLIP_SCREENS").map_or_else(
            || std::env::temp_dir().join("bareclip-screens"),
            PathBuf::from,
        );
        let ctx = context();
        let mut camera = Camera::new(folder);
        let image = image_file("screen", "800x600", "orange@0.6");
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        editor.use_gpu(LayerDisplay::new(
            camera.device.clone(),
            camera.queue.clone(),
            camera.renderer.clone(),
        ));
        editor.import(&ctx, &[image]);
        settle(&ctx, &mut editor);
        let still = editor.media[1].id;
        editor.drop_on_timeline(still, Ticks::from_seconds(1.0), TrackTarget::New(0));
        for _ in 0..30 {
            run_frame(&ctx, &mut editor, vec![]);
            thread::sleep(Duration::from_millis(10));
        }
        editor.seek(Ticks::from_seconds(2.0));
        for _ in 0..30 {
            run_frame(&ctx, &mut editor, vec![]);
            thread::sleep(Duration::from_millis(10));
        }
        camera.shoot(&ctx, &mut editor, vec![], "image-01-over-video");

        editor.selected = None;
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Filter");
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "image-02-filter");
    }

    /// Draws the export window on the real screen, with no window. Run with
    /// `$env:BARECLIP_SCREENS = '<folder>'; ./scripts/cargo.ps1 test -p bareclip-app draw_the_export_screens '--' --ignored`
    #[test]
    #[ignore = "needs a real GPU, and draws pictures to look at rather than checking anything"]
    fn draw_the_export_screens() {
        let folder = std::env::var_os("BARECLIP_SCREENS").map_or_else(
            || std::env::temp_dir().join("bareclip-screens"),
            PathBuf::from,
        );
        let ctx = context();
        let mut camera = Camera::new(folder);
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        for _ in 0..30 {
            run_frame(&ctx, &mut editor, vec![]);
            thread::sleep(Duration::from_millis(10));
        }
        editor.open_export();
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "export-01-match-source");

        click_named(&ctx, &mut editor, "Quality: Match source");
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "export-02-quality-menu");

        click_named(&ctx, &mut editor, "Balanced");
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "export-03-balanced");

        click_named(&ctx, &mut editor, "Sound only");
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "export-04-sound-only");
    }

    // Transitions and looks.

    /// Saves the Transitions panel, a transition on the timeline and a clip's look, to look at. Set
    /// `BARECLIP_SCREENS` to a folder.
    #[test]
    #[ignore = "makes pictures to look at, not a check"]
    fn draw_the_look_screens() {
        let folder = std::env::var_os("BARECLIP_SCREENS").map_or_else(
            || std::env::temp_dir().join("bareclip-screens"),
            PathBuf::from,
        );
        let ctx = context();
        let mut camera = Camera::new(folder);
        let (mut editor, first, _) = with_two_touching_clips(&ctx);
        editor.use_gpu(LayerDisplay::new(
            camera.device.clone(),
            camera.queue.clone(),
            camera.renderer.clone(),
        ));
        let settle = |ctx: &egui::Context, editor: &mut Editor| {
            for _ in 0..40 {
                run_frame(ctx, editor, vec![]);
                thread::sleep(Duration::from_millis(10));
            }
        };
        settle(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Transitions");
        settle(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "look-01-transitions");

        editor.add_transition(first, TransitionKind::HardWipeRight);
        let cut = editor
            .project
            .tracks
            .clip(first)
            .expect("it is there")
            .end();
        editor.seek(cut);
        settle(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "look-02-selected-transition");

        editor.selected_join = None;
        editor.selected = Some(first);
        editor.right_panel = Some(RightPanel::Tool(LookTab::Filters));
        editor.seek(Ticks::from_seconds(0.5));
        for _ in 0..80 {
            run_frame(&ctx, &mut editor, vec![]);
            thread::sleep(Duration::from_millis(10));
        }
        camera.shoot(&ctx, &mut editor, vec![], "look-03-filters");

        editor.right_panel = Some(RightPanel::Tool(LookTab::Colours));
        editor.change_filter_for_screen(first);
        settle(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "look-04-colours");

        editor.right_panel = Some(RightPanel::Tool(LookTab::Fade));
        settle(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "look-05-fade");

        editor.right_panel = Some(RightPanel::Tool(LookTab::Filters));
        settle(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Filter Bold and blue");
        settle(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "look-06-filter-intensity");

        editor.right_panel = Some(RightPanel::Tool(LookTab::Effects));
        for _ in 0..120 {
            run_frame(&ctx, &mut editor, vec![]);
            thread::sleep(Duration::from_millis(10));
        }
        camera.shoot(&ctx, &mut editor, vec![], "look-07-effects");

        click_named(&ctx, &mut editor, "Effect Pulse");
        settle(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Effect Flash");
        settle(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "look-08-effect-settings");
    }

    /// Two videos side by side on one row.
    fn with_two_touching_clips(ctx: &egui::Context) -> (Editor, ClipId, ClipId) {
        let editor = with_videos(ctx, &[test_clip(), test_clip()]);
        let row = editor.project.tracks.tracks()[0].clips();
        let (first, second) = (row[0].id, row[1].id);
        assert_eq!(row[0].end(), row[1].start);
        (editor, first, second)
    }

    #[test]
    fn dragging_a_transition_onto_a_join_adds_it_there() {
        let ctx = context();
        let (mut editor, first, _) = with_two_touching_clips(&ctx);
        click_named(&ctx, &mut editor, "Transitions");
        run_frame(&ctx, &mut editor, vec![]);
        let from = named_rect(&ctx, &mut editor, "Add Cross blur").center();
        let cut = editor
            .project
            .tracks
            .clip(first)
            .expect("it is there")
            .end();
        let to = pos2(
            editor.timeline_state.scale.x_of(cut),
            editor.timeline_state.track.center().y,
        );

        pointer_drag(&ctx, &mut editor, from, to);

        let clip = editor.project.tracks.clip(first).expect("it is there");
        assert_eq!(
            clip.transition.map(|t| t.kind),
            Some(TransitionKind::CrossBlur)
        );
        assert_eq!(editor.selected_join, Some(first));
    }

    #[test]
    fn dragging_a_transition_away_from_any_join_adds_nothing_and_says_so() {
        let ctx = context();
        let (mut editor, first, _) = with_two_touching_clips(&ctx);
        click_named(&ctx, &mut editor, "Transitions");
        run_frame(&ctx, &mut editor, vec![]);
        let from = named_rect(&ctx, &mut editor, "Add Cross blur").center();
        let clip = editor.project.tracks.clip(first).expect("it is there");
        let to = pos2(
            editor.timeline_state.scale.x_of(Ticks(clip.start.0 + 1)) + 10.0,
            editor.timeline_state.track.center().y,
        );

        pointer_drag(&ctx, &mut editor, from, to);

        let clip = editor.project.tracks.clip(first).expect("it is there");
        assert_eq!(clip.transition, None);
        assert!(editor.notice.is_some());
    }

    #[test]
    fn right_clicking_a_clip_on_the_picture_opens_its_menu_and_delete_takes_it_away() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        settle_animations(&ctx, &mut editor);
        let picture = stage_frame(&editor);
        let events = vec![
            egui::Event::PointerMoved(picture.center()),
            egui::Event::PointerButton {
                pos: picture.center(),
                button: egui::PointerButton::Secondary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: picture.center(),
                button: egui::PointerButton::Secondary,
                pressed: false,
                modifiers: Modifiers::NONE,
            },
        ];
        run_frame(&ctx, &mut editor, events);
        run_frame(&ctx, &mut editor, vec![]);
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.picture_menu.is_some(), "the menu opens");
        assert_eq!(clip_count(&editor), 1);
        click_named(&ctx, &mut editor, "Delete");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(clip_count(&editor), 0);
        assert!(editor.picture_menu.is_none());
    }

    #[test]
    fn right_clicking_where_no_clip_shows_opens_nothing() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        settle_animations(&ctx, &mut editor);
        let events = stage::StageEvents::default();
        editor.apply_stage_events(events, false);
        assert!(editor.picture_menu.is_none());
    }

    #[test]
    fn the_transitions_tab_lists_all_44_with_a_search_box() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Transitions");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.left_panel, LeftPanel::Transitions);
        let names: Vec<String> = named_nodes(&ctx, &mut editor)
            .into_iter()
            .map(|(name, ..)| name)
            .collect();
        assert!(names.contains(&"Search transitions".to_owned()));
        assert!(names.contains(&"Add Cross fade".to_owned()));
        editor.transition_search = "barn".to_owned();
        run_frame(&ctx, &mut editor, vec![]);
        let found: Vec<String> = named_nodes(&ctx, &mut editor)
            .into_iter()
            .map(|(name, ..)| name)
            .filter(|name| name.starts_with("Add "))
            .collect();
        assert_eq!(found.len(), 2, "{found:?}");
    }

    #[test]
    fn adding_a_transition_selects_it_opens_its_panel_and_moves_nothing() {
        let ctx = context();
        let (mut editor, first, second) = with_two_touching_clips(&ctx);
        let before = editor.project.tracks.duration();
        editor.add_transition(first, TransitionKind::Burn);
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.project.tracks.duration(), before);
        assert_eq!(editor.selected_join, Some(first));
        assert_eq!(editor.left_panel, LeftPanel::Transitions);
        let clip = editor.project.tracks.clip(first).expect("it is there");
        assert_eq!(clip.transition.map(|t| t.kind), Some(TransitionKind::Burn));
        assert_eq!(
            editor
                .project
                .tracks
                .clip(second)
                .expect("it is there")
                .start,
            clip.end()
        );
    }

    #[test]
    fn a_transition_is_one_undo_step_and_comes_back_with_redo() {
        let ctx = context();
        let (mut editor, first, _) = with_two_touching_clips(&ctx);
        editor.add_transition(first, TransitionKind::Burn);
        run_frame(&ctx, &mut editor, vec![]);
        let has_one = |editor: &Editor| {
            editor.project.tracks.tracks()[0].clips()[0]
                .transition
                .is_some()
        };
        assert!(has_one(&editor));
        editor.undo();
        run_frame(&ctx, &mut editor, vec![]);
        assert!(!has_one(&editor));
        assert_eq!(editor.selected_join, None, "nothing is left to be selected");
        editor.redo();
        run_frame(&ctx, &mut editor, vec![]);
        assert!(has_one(&editor));
    }

    #[test]
    fn swapping_a_transition_keeps_how_long_it_lasts() {
        let ctx = context();
        let (mut editor, first, _) = with_two_touching_clips(&ctx);
        editor.add_transition(first, TransitionKind::Burn);
        let length = Ticks::from_seconds(0.25).0;
        let transition = Transition {
            kind: TransitionKind::Burn,
            duration: length,
        };
        editor
            .project
            .tracks
            .set_transition(first, Some(transition));
        editor.add_transition(first, TransitionKind::Spin);
        let clip = editor.project.tracks.clip(first).expect("it is there");
        let got = clip.transition.expect("it has one");
        assert_eq!(got.kind, TransitionKind::Spin);
        assert_eq!(got.duration, length);
    }

    #[test]
    fn the_delete_key_takes_a_selected_transition_away_and_leaves_the_clips() {
        let ctx = context();
        let (mut editor, first, second) = with_two_touching_clips(&ctx);
        editor.add_transition(first, TransitionKind::Burn);
        run_frame(&ctx, &mut editor, vec![]);
        run_frame(&ctx, &mut editor, key_event(Key::Delete, Modifiers::NONE));
        let clip = editor.project.tracks.clip(first).expect("the clip stays");
        assert_eq!(clip.transition, None);
        assert!(editor.project.tracks.clip(second).is_some());
    }

    #[test]
    fn pressing_a_transition_on_the_timeline_selects_it() {
        let ctx = context();
        let (mut editor, first, _) = with_two_touching_clips(&ctx);
        editor.add_transition(first, TransitionKind::Burn);
        editor.selected_join = None;
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Transition");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.selected_join, Some(first));
    }

    #[test]
    fn selecting_a_clip_lets_go_of_the_transition() {
        let ctx = context();
        let (mut editor, first, _) = with_two_touching_clips(&ctx);
        editor.add_transition(first, TransitionKind::Burn);
        let events = timeline::TimelineEvents {
            select: Some(Some(first)),
            ..Default::default()
        };
        editor.apply_timeline_events(events, false);
        assert_eq!(editor.selected_join, None);
        assert_eq!(editor.selected, Some(first));
    }

    #[test]
    fn dropping_a_transition_away_from_a_join_says_where_it_goes() {
        let ctx = context();
        let (mut editor, ..) = with_two_touching_clips(&ctx);
        let events = timeline::TimelineEvents {
            transition_missed: true,
            ..Default::default()
        };
        editor.apply_timeline_events(events, false);
        assert!(editor.notice.is_some());
    }

    #[test]
    fn a_transition_or_a_filter_means_redrawing_on_export() {
        let ctx = context();
        let (mut editor, first, _) = with_two_touching_clips(&ctx);
        assert!(matches!(editor.export_route(), Route::Fast));
        editor.add_transition(first, TransitionKind::Burn);
        let Route::Full(reason) = editor.export_route() else {
            panic!("it should be redrawn");
        };
        assert!(reason.contains("Transitions"), "{reason}");
    }

    #[test]
    fn the_open_panel_lets_filters_be_chosen() {
        let ctx = context();
        let (mut editor, first, _) = with_two_touching_clips(&ctx);
        editor.selected = Some(first);
        editor.right_panel = Some(RightPanel::Tool(LookTab::Filters));
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.looking_at().is_some());
        click_named(&ctx, &mut editor, "Filter Winter");
        run_frame(&ctx, &mut editor, vec![]);
        let clip = editor.project.tracks.clip(first).expect("it is there");
        assert_eq!(clip.look.filter, Filter::Winter);
        editor.settle_title_edit(&ctx);
        run_frame(&ctx, &mut editor, vec![]);
        editor.undo();
        run_frame(&ctx, &mut editor, vec![]);
        let clip = editor.project.tracks.tracks()[0].clips()[0];
        assert_eq!(clip.look.filter, Filter::None);
    }

    #[test]
    fn a_chosen_filter_shows_its_intensity_which_starts_where_it_was_measured() {
        let ctx = context();
        let (mut editor, first, _) = with_two_touching_clips(&ctx);
        editor.selected = Some(first);
        editor.right_panel = Some(RightPanel::Tool(LookTab::Filters));
        run_frame(&ctx, &mut editor, vec![]);
        assert!(!has_named(&ctx, &mut editor, "Intensity"));
        click_named(&ctx, &mut editor, "Filter Orange and teal");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(has_named(&ctx, &mut editor, "Intensity"));
        let clip = editor.project.tracks.clip(first).expect("it is there");
        assert_eq!(
            clip.look.filter_intensity,
            bareclip_core::effects::FILTER_INTENSITY_START
        );
    }

    #[test]
    fn effects_turn_on_and_off_several_at_once_and_none_clears_them() {
        let ctx = context();
        let (mut editor, first, _) = with_two_touching_clips(&ctx);
        editor.selected = Some(first);
        editor.right_panel = Some(RightPanel::Tool(LookTab::Filters));
        editor.right_panel = Some(RightPanel::Tool(LookTab::Effects));
        run_frame(&ctx, &mut editor, vec![]);
        let effects = |editor: &Editor| {
            editor
                .project
                .tracks
                .clip(first)
                .expect("it is there")
                .look
                .effects
        };
        click_named(&ctx, &mut editor, "Effect Flash");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(effects(&editor).is_on(EffectKind::Flash));
        // Turning one on shows its settings.
        assert_eq!(editor.effect_settings, Some(EffectKind::Flash));
        assert!(has_named(&ctx, &mut editor, "Speed"));
        click_named(&ctx, &mut editor, "Effect VHS");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(effects(&editor).is_on(EffectKind::Flash));
        assert!(effects(&editor).is_on(EffectKind::Vhs));
        click_named(&ctx, &mut editor, "Effect Flash");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(!effects(&editor).is_on(EffectKind::Flash));
        click_named(&ctx, &mut editor, "Effect None");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(!effects(&editor).any());
    }

    #[test]
    fn an_effects_settings_change_it_and_its_button_opens_and_closes_them() {
        let ctx = context();
        let (mut editor, first, _) = with_two_touching_clips(&ctx);
        editor.selected = Some(first);
        editor.right_panel = Some(RightPanel::Tool(LookTab::Filters));
        editor.right_panel = Some(RightPanel::Tool(LookTab::Effects));
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Effect Crash zoom");
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Top left");
        run_frame(&ctx, &mut editor, vec![]);
        let clip = editor.project.tracks.clip(first).expect("it is there");
        assert_eq!(clip.look.effects.value(EffectKind::CrashZoom, 0), 0);
        click_named(&ctx, &mut editor, "Effect settings");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.effect_settings, None);
        assert!(!has_named(&ctx, &mut editor, "Top left"));
        click_named(&ctx, &mut editor, "Effect settings");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.effect_settings, Some(EffectKind::CrashZoom));
    }

    #[test]
    fn a_zoom_effect_keeps_its_video_sharp_in_the_preview() {
        let ctx = context();
        let (mut editor, first, _) = with_two_touching_clips(&ctx);
        let media = editor
            .project
            .tracks
            .clip(first)
            .expect("it is there")
            .media;
        editor.selected = Some(first);
        editor.right_panel = Some(RightPanel::Tool(LookTab::Filters));
        editor.right_panel = Some(RightPanel::Tool(LookTab::Effects));
        run_frame(&ctx, &mut editor, vec![]);
        assert!(!editor.preview.is_sharp(media));
        click_named(&ctx, &mut editor, "Effect Pulse");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.preview.is_sharp(media));
        click_named(&ctx, &mut editor, "Effect None");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(!editor.preview.is_sharp(media));
    }

    #[test]
    fn an_effect_means_redrawing_on_export_and_comes_back_after_undo() {
        let ctx = context();
        let (mut editor, first, _) = with_two_touching_clips(&ctx);
        editor.selected = Some(first);
        editor.right_panel = Some(RightPanel::Tool(LookTab::Filters));
        editor.right_panel = Some(RightPanel::Tool(LookTab::Effects));
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Effect Pulse");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(matches!(editor.export_route(), Route::Full(_)));
        editor.settle_title_edit(&ctx);
        run_frame(&ctx, &mut editor, vec![]);
        editor.undo();
        run_frame(&ctx, &mut editor, vec![]);
        let clip = editor.project.tracks.tracks()[0].clips()[0];
        assert!(!clip.look.effects.any());
    }

    #[test]
    fn a_title_has_its_own_tools_on_the_rail() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        with_a_title(&ctx, &mut editor, TitleStyle::Plain);
        assert!(is_named(&ctx, &mut editor, "Edit text"));
        click_named(&ctx, &mut editor, "Fade");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.right_panel, Some(RightPanel::Tool(LookTab::Fade)));
    }

    #[test]
    fn every_transition_has_a_card() {
        let total: usize = TRANSITION_GROUPS.iter().map(|(_, kinds)| kinds.len()).sum();
        assert_eq!(total, 44);
    }

    // Titles.

    #[test]
    fn a_style_card_only_starts_moving_once_the_pointer_has_rested_on_it() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Text");
        run_frame(&ctx, &mut editor, vec![]);
        let rect = named_rect(&ctx, &mut editor, "Add Typewriter");
        run_frame(
            &ctx,
            &mut editor,
            vec![egui::Event::PointerMoved(rect.center())],
        );
        run_frame(&ctx, &mut editor, vec![]);
        let hover = editor
            .card_hover
            .as_mut()
            .expect("the pointer is on a card");
        assert!(hover.moving_yet().is_none(), "it waits before it moves");
        hover.wait_over();
        run_frame(&ctx, &mut editor, vec![]);
        let hover = editor
            .card_hover
            .as_ref()
            .expect("the pointer is still on it");
        assert!(hover.moving_yet().is_some(), "it moves after the wait");

        run_frame(
            &ctx,
            &mut editor,
            vec![egui::Event::PointerMoved(pos2(900.0, 700.0))],
        );
        run_frame(&ctx, &mut editor, vec![]);
        assert!(
            editor.card_hover.is_none(),
            "it stops when the pointer goes"
        );
    }

    #[test]
    fn a_card_that_has_no_motion_never_starts() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Text");
        run_frame(&ctx, &mut editor, vec![]);
        let rect = named_rect(&ctx, &mut editor, "Add Text");
        run_frame(
            &ctx,
            &mut editor,
            vec![egui::Event::PointerMoved(rect.center())],
        );
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.card_hover.is_none());
    }

    #[test]
    fn see_all_opens_every_text_style_and_back_returns() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Text");
        run_frame(&ctx, &mut editor, vec![]);
        let names = |ctx: &egui::Context, editor: &mut Editor| -> Vec<String> {
            named_nodes(ctx, editor)
                .into_iter()
                .map(|(name, ..)| name)
                .collect()
        };
        let before = names(&ctx, &mut editor);
        assert!(before.contains(&"Add Pride".to_owned()));
        assert!(!before.contains(&"Add Neon".to_owned()));
        click_named(&ctx, &mut editor, "See all");
        run_frame(&ctx, &mut editor, vec![]);
        let all = names(&ctx, &mut editor);
        assert!(all.contains(&"Add Neon".to_owned()));
        assert!(
            !all.contains(&"Add Text".to_owned()),
            "only the text styles show"
        );
        click_named(&ctx, &mut editor, "Back to all the text");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(names(&ctx, &mut editor).contains(&"Add Text".to_owned()));
    }

    #[test]
    fn every_title_style_can_be_added_and_played_through() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        for (_, styles) in bareclip_core::title::STYLE_GROUPS {
            for style in styles {
                let id = with_a_title(&ctx, &mut editor, *style);
                let clip = editor.project.tracks.clip(id).expect("the clip is there");
                editor.seek(Ticks(clip.start.0 + Ticks::from_seconds(0.3).0));
                run_frame(&ctx, &mut editor, vec![]);
                editor.selected = None;
                run_frame(&ctx, &mut editor, vec![]);
                editor.seek(clip.end());
            }
        }
    }

    #[test]
    fn a_logo_that_cannot_be_read_is_refused_and_says_so() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let id = with_a_title(&ctx, &mut editor, TitleStyle::IntroMirror);
        let media = editor.project.tracks.clip(id).expect("it is there").media;
        editor.set_title_logo(media, PathBuf::from("no such picture.png"));
        assert!(title_of_clip(&editor, id).logo.is_none());
        assert!(editor.notice.is_some());
    }

    /// A title added at the playhead, and its clip.
    fn with_a_title(ctx: &egui::Context, editor: &mut Editor, style: TitleStyle) -> ClipId {
        let time = editor.position();
        editor.add_title(style, time, None);
        run_frame(ctx, editor, vec![]);
        editor.selected.expect("the new title is selected")
    }

    fn title_of_clip(editor: &Editor, id: ClipId) -> Title {
        let clip = editor.project.tracks.clip(id).expect("the clip is there");
        editor.title_of(clip).expect("it is a title").clone()
    }

    fn set_title_text(ctx: &egui::Context, editor: &mut Editor, id: ClipId, words: &str) {
        let clip = editor.project.tracks.clip(id).expect("the clip is there");
        editor.change_title(clip.media, |title| title.text = words.to_owned());
        run_frame(ctx, editor, vec![]);
    }

    #[test]
    fn the_text_tab_opens_the_text_panel_and_my_media_goes_back() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.left_panel, LeftPanel::Media);

        click_named(&ctx, &mut editor, "Text");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.left_panel, LeftPanel::Text);
        assert!(editor.media_panel_open);
        assert!(is_named(&ctx, &mut editor, "Add Outlined"));

        click_named(&ctx, &mut editor, "My media");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.left_panel, LeftPanel::Media);
        assert!(!is_named(&ctx, &mut editor, "Add Outlined"));
    }

    #[test]
    fn pressing_the_open_tab_closes_the_panel() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Text");
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Text");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(!editor.media_panel_open);
    }

    #[test]
    fn pressing_a_style_adds_it_at_the_playhead_and_selects_it_for_five_seconds() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        editor.seek(Ticks::from_seconds(1.0));
        let video = editor.project.tracks.played().clips()[0].id;
        let video_start = editor.project.tracks.clip(video).map(|clip| clip.start);
        run_frame(&ctx, &mut editor, vec![]);

        click_named(&ctx, &mut editor, "Text");
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Add Pill");
        run_frame(&ctx, &mut editor, vec![]);

        let id = editor.selected.expect("the title is selected");
        assert_ne!(id, video);
        let clip = editor
            .project
            .tracks
            .clip(id)
            .expect("it is on the timeline");
        assert!(clip.is_title());
        assert_eq!(clip.start, Ticks::from_seconds(1.0));
        assert_eq!(clip.duration(), Ticks::from_seconds(5.0));
        assert_eq!(title_of_clip(&editor, id).style, TitleStyle::Pill);
        assert!(editor.project.tracks.is_text_row(0));
        assert_eq!(
            editor.project.tracks.clip(video).map(|clip| clip.start),
            video_start,
            "adding a title moves nothing else"
        );
    }

    #[test]
    fn the_lane_above_opens_the_text_panel_and_adds_nothing() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        let rows = editor.project.tracks.tracks().len();

        let lane = editor.timeline_state.layout.new_lane_above();
        press_without_following_up(&ctx, &mut editor, lane.center());
        run_frame(&ctx, &mut editor, vec![]);

        assert_eq!(editor.left_panel, LeftPanel::Text);
        assert!(editor.media_panel_open);
        assert_eq!(editor.project.tracks.tracks().len(), rows);
    }

    #[test]
    fn dropping_a_style_on_the_timeline_makes_a_title_where_it_lands() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);

        let events = timeline::TimelineEvents {
            drop_title: Some((
                TitleStyle::Framed,
                Ticks::from_seconds(2.0),
                TrackTarget::Existing(0),
            )),
            ..Default::default()
        };
        editor.apply_timeline_events(events, false);
        run_frame(&ctx, &mut editor, vec![]);

        let id = editor.selected.expect("the title is selected");
        let clip = editor
            .project
            .tracks
            .clip(id)
            .expect("it is on the timeline");
        assert_eq!(clip.start, Ticks::from_seconds(2.0));
        assert_eq!(editor.project.tracks.track_of(id), Some(0));
        assert_eq!(
            editor.project.tracks.tracks().len(),
            2,
            "on a row of its own"
        );
    }

    #[test]
    fn titles_stay_out_of_the_media_panel() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        with_a_title(&ctx, &mut editor, TitleStyle::Plain);
        assert_eq!(editor.visible_media().count(), 0);
        assert_eq!(editor.listed_media().len(), 0);
        assert!(editor.media.iter().any(|entry| entry.id.is_title()));
    }

    #[test]
    fn a_title_on_its_own_makes_a_canvas_and_a_later_video_sets_the_size() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let id = with_a_title(&ctx, &mut editor, TitleStyle::Plain);
        let drawn_for = editor.project.canvas.size();
        assert_eq!(drawn_for, bareclip_core::canvas::EMPTY_SIZE);

        let videos = with_videos(&ctx, &[test_clip()]);
        let size = (
            videos.media[0].info.video.width,
            videos.media[0].info.video.height,
        );
        drop(videos);
        let added = editor.load(&ctx, test_clip()).expect("it opens");
        let media = added.id;
        editor.media.push(added);
        editor.add_to_timeline(media);
        run_frame(&ctx, &mut editor, vec![]);
        run_frame(&ctx, &mut editor, vec![]);

        assert_eq!(editor.project.canvas.size(), size);
        let clip = editor.project.tracks.clip(id).expect("it stays");
        assert!(clip.start.0 == 0);
        let entry = editor.entry(clip.media).expect("it has an entry");
        assert_eq!(entry.title_drawn.as_ref().map(|drawn| drawn.1), Some(size));
    }

    #[test]
    fn selecting_a_title_shows_its_controls_and_other_clips_dont_have_them() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        let video = Some(editor.project.tracks.played().clips()[0].id);
        assert!(!is_named(&ctx, &mut editor, "Title text"));

        with_a_title(&ctx, &mut editor, TitleStyle::Plain);
        for control in [
            "Title text",
            "Font: Hanken Grotesk",
            "Title size",
            "Text colour",
            "Background colour",
            "Middle",
            "Bottom right",
        ] {
            assert!(is_named(&ctx, &mut editor, control), "{control}");
        }
        assert!(!is_named(&ctx, &mut editor, "Second line text"));
        assert!(!is_named(&ctx, &mut editor, "Accent colour"));

        editor.selected = video;
        run_frame(&ctx, &mut editor, vec![]);
        assert!(!is_named(&ctx, &mut editor, "Title text"));
    }

    #[test]
    fn styles_only_show_the_controls_they_have() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        with_a_title(&ctx, &mut editor, TitleStyle::Underlined);
        assert!(is_named(&ctx, &mut editor, "Second line text"));
        assert!(is_named(&ctx, &mut editor, "Accent colour"));
    }

    #[test]
    fn a_titles_toolbar_has_no_crop_flips_or_picture_in_picture() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        with_a_title(&ctx, &mut editor, TitleStyle::Plain);
        run_frame(&ctx, &mut editor, vec![]);
        assert!(is_named(&ctx, &mut editor, "Rotate by 90 degrees"));
        assert!(is_named(&ctx, &mut editor, "More"));
        assert!(!is_named(&ctx, &mut editor, "Crop"));
        assert!(!is_named(&ctx, &mut editor, "Flip left to right"));
        assert!(!is_named(&ctx, &mut editor, "Speed"));
        assert!(!is_named(&ctx, &mut editor, "Audio"));
    }

    #[test]
    fn changing_the_words_redraws_the_picture_and_keeps_its_place_and_sharpness() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let id = with_a_title(&ctx, &mut editor, TitleStyle::Plain);
        let media = editor.project.tracks.clip(id).expect("it is there").media;
        let before = editor
            .entry(media)
            .and_then(|entry| entry.still.clone())
            .expect("drawn");

        set_title_text(&ctx, &mut editor, id, "A good many more words");

        let after = editor
            .entry(media)
            .and_then(|entry| entry.still.clone())
            .expect("drawn");
        assert!(after.width > before.width);
        assert_ne!(after.id, before.id, "the GPU has to be sent the new one");
        let clip = editor.project.tracks.clip(id).expect("it is there");
        let canvas = editor.project.canvas.size();
        let rect = clip.placement.rect((after.width, after.height), canvas);
        assert!(
            (rect.width - after.width as f32).abs() < 1.0,
            "it shows its own pixels"
        );
        assert_eq!(
            TitleSpot::of(clip.placement, (after.width, after.height), canvas),
            Some(TitleSpot::Middle),
            "it stays in the middle"
        );
    }

    #[test]
    fn a_title_in_a_corner_stays_there_as_it_grows() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let id = with_a_title(&ctx, &mut editor, TitleStyle::LowerThird);
        let clip = editor.project.tracks.clip(id).expect("it is there");
        let canvas = editor.project.canvas.size();
        let size = editor.video_size(clip.media).expect("it has a size");
        assert_eq!(
            TitleSpot::of(clip.placement, size, canvas),
            Some(TitleSpot::BottomLeft)
        );

        set_title_text(&ctx, &mut editor, id, "Much, much longer words than before");

        let clip = editor.project.tracks.clip(id).expect("it is there");
        let size = editor.video_size(clip.media).expect("it has a size");
        assert_eq!(
            TitleSpot::of(clip.placement, size, canvas),
            Some(TitleSpot::BottomLeft)
        );
    }

    #[test]
    fn pressing_a_position_moves_the_title_as_one_undo_step() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let id = with_a_title(&ctx, &mut editor, TitleStyle::Plain);
        let before = editor
            .project
            .tracks
            .clip(id)
            .expect("it is there")
            .placement;

        click_named(&ctx, &mut editor, "Top right");
        run_frame(&ctx, &mut editor, vec![]);

        let clip = editor.project.tracks.clip(id).expect("it is there");
        let canvas = editor.project.canvas.size();
        let size = editor.video_size(clip.media).expect("it has a size");
        assert_eq!(
            TitleSpot::of(clip.placement, size, canvas),
            Some(TitleSpot::TopRight)
        );
        editor.undo();
        assert_eq!(
            editor
                .project
                .tracks
                .clip(id)
                .expect("it is there")
                .placement,
            before
        );
    }

    #[test]
    fn one_go_of_typing_is_one_undo_step() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let id = with_a_title(&ctx, &mut editor, TitleStyle::Plain);
        let clip = editor.project.tracks.clip(id).expect("it is there");
        // As if the box still had the keyboard.
        editor.change_title(clip.media, |title| title.text = "O".to_owned());
        editor.change_title(clip.media, |title| title.text = "On".to_owned());
        editor.change_title(clip.media, |title| title.text = "One".to_owned());
        assert!(editor.title_before.is_some());

        // Nothing has the keyboard any more, so the step ends.
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.title_before.is_none());
        assert_eq!(title_of_clip(&editor, id).text, "One");

        editor.undo();
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(title_of_clip(&editor, id).text, TitleStyle::Plain.name());
        let media = clip.media;
        let drawn = editor
            .entry(media)
            .and_then(|entry| entry.title_drawn.clone());
        assert_eq!(
            drawn.map(|drawn| drawn.0.text),
            Some(TitleStyle::Plain.name().to_owned())
        );
    }

    #[test]
    fn undoing_a_new_title_takes_it_off_the_timeline_and_redo_brings_it_back() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        with_a_title(&ctx, &mut editor, TitleStyle::Plain);
        editor.undo();
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.project.tracks.is_empty());
        assert!(editor.project.titles.is_empty());
        editor.redo();
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.project.titles.len(), 1);
        assert_eq!(editor.project.tracks.tracks().len(), 1);
    }

    #[test]
    fn a_corner_drag_changes_the_titles_size_and_it_is_redrawn_when_let_go() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let id = with_a_title(&ctx, &mut editor, TitleStyle::Plain);
        let start = editor.project.tracks.clip(id).expect("it is there");
        let size = title_of_clip(&editor, id).size;
        let bigger = Placement {
            scale: start.placement.scale * 2,
            ..start.placement
        };

        let event = stage::StageEvents {
            place: Some((id, bigger)),
            ..Default::default()
        };
        editor.apply_stage_events(event, true);
        assert_eq!(title_of_clip(&editor, id).size, size * 2);
        assert!(editor.title_resize.is_some(), "held, so not drawn yet");
        editor.sync_titles();
        let held = editor.project.tracks.clip(id).expect("it is there");
        assert_eq!(held.placement.scale, bigger.scale);

        editor.apply_stage_events(stage::StageEvents::default(), false);
        editor.sync_titles();
        let let_go = editor.project.tracks.clip(id).expect("it is there");
        let canvas = editor.project.canvas.size();
        let picture = editor.video_size(let_go.media).expect("it has a size");
        let rect = let_go.placement.rect(picture, canvas);
        assert!(
            (rect.width - picture.0 as f32).abs() < 1.0,
            "sharp again at its new size"
        );
        assert!(picture.1 > editor_start_height(start, &editor));

        editor.undo();
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(title_of_clip(&editor, id).size, size);
    }

    /// How tall a clip's title showed when it started, from its placement.
    fn editor_start_height(start: bareclip_core::timeline::Clip, editor: &Editor) -> u32 {
        let canvas = editor.project.canvas.size();
        let share = f64::from(start.placement.scale) / f64::from(bareclip_core::placement::WHOLE);
        // A plain title is far wider than it is tall, so it fits the canvas by its width.
        (share * f64::from(canvas.1)) as u32 / 3
    }

    #[test]
    fn reset_puts_a_title_back_how_its_style_started_rather_than_filling_the_picture() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let id = with_a_title(&ctx, &mut editor, TitleStyle::Subtitle);
        let started = editor.project.tracks.clip(id).expect("it is there");
        let media = started.media;
        editor.change_title(media, |title| title.size *= 3);
        run_frame(&ctx, &mut editor, vec![]);

        let event = stage::StageEvents {
            place: Some((id, Placement::default())),
            ..Default::default()
        };
        editor.apply_stage_events(event, false);
        run_frame(&ctx, &mut editor, vec![]);

        let clip = editor.project.tracks.clip(id).expect("it is there");
        let canvas = editor.project.canvas.size();
        let size = editor.video_size(media).expect("it has a size");
        assert_eq!(
            title_of_clip(&editor, id).size,
            TitleStyle::Subtitle.starting_size()
        );
        assert_eq!(
            TitleSpot::of(clip.placement, size, canvas),
            Some(TitleSpot::BottomMiddle)
        );
        assert_eq!(clip.placement.scale, started.placement.scale);
    }

    #[test]
    fn a_split_and_a_duplicate_each_get_a_title_of_their_own() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let first = with_a_title(&ctx, &mut editor, TitleStyle::Plain);
        editor.seek(Ticks::from_seconds(2.0));
        run_frame(&ctx, &mut editor, vec![]);
        editor.split_at_playhead();
        run_frame(&ctx, &mut editor, vec![]);
        let second = editor.selected.expect("the second part is selected");
        assert_ne!(first, second);

        set_title_text(&ctx, &mut editor, second, "Changed");
        assert_eq!(title_of_clip(&editor, first).text, TitleStyle::Plain.name());
        assert_eq!(title_of_clip(&editor, second).text, "Changed");

        editor.duplicate_clip(second);
        run_frame(&ctx, &mut editor, vec![]);
        let copy = editor.selected.expect("the copy is selected");
        set_title_text(&ctx, &mut editor, copy, "Copy");
        assert_eq!(title_of_clip(&editor, second).text, "Changed");
        assert_eq!(title_of_clip(&editor, copy).text, "Copy");
    }

    #[test]
    fn a_title_saves_and_comes_back_with_everything_about_it() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let id = with_a_title(&ctx, &mut editor, TitleStyle::Boxed);
        let media = editor.project.tracks.clip(id).expect("it is there").media;
        editor.change_title(media, |title| {
            title.text = "Saved words".to_owned();
            title.second_text = "And these".to_owned();
            title.font = "Abril Fatface".to_owned();
            title.colour = [1, 2, 3];
            title.background = Some([4, 5, 6]);
        });
        run_frame(&ctx, &mut editor, vec![]);
        let saved = title_of_clip(&editor, id);
        let placement = editor
            .project
            .tracks
            .clip(id)
            .expect("it is there")
            .placement;
        let path =
            std::env::temp_dir().join(format!("bareclip-title-{}.bareclip", std::process::id()));
        assert!(editor.save_to(&path), "it saves");

        let mut back = Editor::new(&ctx, false);
        back.open_project(&ctx, &path);
        run_frame(&ctx, &mut back, vec![]);

        assert!(!back.has_unsaved_changes(), "opening it changes nothing");
        let row = &back.project.tracks.tracks()[0];
        let clip = row.clips()[0];
        assert!(clip.is_title());
        assert_eq!(back.title_of(clip), Some(&saved));
        assert_eq!(clip.placement, placement);
        assert!(
            back.entry(clip.media)
                .and_then(|entry| entry.still.clone())
                .is_some()
        );
        assert_eq!(clip.duration(), Ticks::from_seconds(5.0));
        let _ = std::fs::remove_file(&path);

        // A title made after opening doesn't take the id of one already there.
        let another = with_a_title(&ctx, &mut back, TitleStyle::Plain);
        let new = back
            .project
            .tracks
            .clip(another)
            .expect("it is there")
            .media;
        assert_ne!(new, clip.media);
    }

    #[test]
    fn a_deleted_titles_words_dont_stay_in_the_file() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let id = with_a_title(&ctx, &mut editor, TitleStyle::Plain);
        assert_eq!(editor.current_file().titles.len(), 1);
        editor.selected = Some(id);
        editor.delete_selected();
        assert!(editor.current_file().titles.is_empty());
    }

    #[test]
    fn a_missing_font_is_reported_once_and_kept_in_the_file() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let id = with_a_title(&ctx, &mut editor, TitleStyle::Plain);
        let media = editor.project.tracks.clip(id).expect("it is there").media;
        editor.change_title(media, |title| {
            title.font = "Nobody Has This Font".to_owned()
        });
        run_frame(&ctx, &mut editor, vec![]);
        let notice = editor.notice.take().expect("it says so");
        assert!(notice.contains("Nobody Has This Font"), "{notice}");

        set_title_text(&ctx, &mut editor, id, "More words");
        assert!(editor.notice.is_none(), "only said once");
        assert_eq!(title_of_clip(&editor, id).font, "Nobody Has This Font");
    }

    #[test]
    fn a_title_on_the_timeline_says_its_first_line_of_words() {
        let title = Title {
            text: "\nFirst line\nSecond line".to_owned(),
            ..Title::default()
        };
        assert_eq!(Editor::title_label(&title), "First line");
        let empty = Title {
            text: String::new(),
            ..Title::in_style(TitleStyle::Framed)
        };
        assert_eq!(Editor::title_label(&empty), "Framed");
    }

    #[test]
    fn a_project_with_a_title_says_why_it_is_redrawn() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.export_route(), Route::Fast);
        with_a_title(&ctx, &mut editor, TitleStyle::Plain);
        assert_eq!(editor.export_route(), Route::Full(TITLES_REASON.to_owned()));
    }

    #[test]
    fn a_title_exports_over_the_video_as_it_shows() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        for _ in 0..20 {
            run_frame(&ctx, &mut editor, vec![]);
            thread::sleep(Duration::from_millis(10));
        }
        // A big white box in the middle is easy to find in the finished video, and plain text has no
        // motion, so it is there from the very first picture.
        let id = with_a_title(&ctx, &mut editor, TitleStyle::Plain);
        let media = editor.project.tracks.clip(id).expect("it is there").media;
        editor.change_title(media, |title| {
            title.background = Some([255, 255, 255]);
            title.colour = [255, 255, 255];
            title.size = 200_000;
        });
        run_frame(&ctx, &mut editor, vec![]);
        let output = output_called("with-title");

        let report = exported_report(&ctx, &mut editor, &output);

        assert!(matches!(report, ProjectReport::Full(_)), "{report:?}");
        let mut reader = bareclip_media::PictureReader::open(&output).expect("it opens");
        let picture = reader
            .next_picture()
            .expect("it decodes")
            .expect("it has a picture");
        let view = picture.view();
        let middle = view.height as usize / 2 * view.y_stride + view.width as usize / 2;
        assert!(
            view.y[middle] > 200,
            "the title shows, at {}",
            view.y[middle]
        );
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn a_project_of_only_a_title_exports() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        with_a_title(&ctx, &mut editor, TitleStyle::Plain);
        let output = output_called("only-title");
        let report = exported_report(&ctx, &mut editor, &output);
        assert!(matches!(report, ProjectReport::Full(_)), "{report:?}");
        let info = probe(&output).expect("it can be read");
        assert!((info.duration.as_seconds() - 5.0).abs() < 0.2);
        let _ = std::fs::remove_file(&output);
    }

    // Captions.

    fn subtitle_file(name: &str, contents: &str) -> PathBuf {
        let folder = std::env::temp_dir().join("bareclip-app-tests");
        std::fs::create_dir_all(&folder).expect("the folder can be created");
        let path = folder.join(format!("{name}-{}.srt", std::process::id()));
        std::fs::write(&path, contents).expect("it can be written");
        path
    }

    /// As many captions as asked for, a second each with half a second between.
    fn long_subtitles(count: usize) -> String {
        let time = |millis: usize| {
            format!(
                "{:02}:{:02}:{:02},{:03}",
                millis / 3_600_000,
                millis / 60_000 % 60,
                millis / 1000 % 60,
                millis % 1000
            )
        };
        (0..count)
            .map(|number| {
                let start = number * 1500;
                format!(
                    "{}\n{} --> {}\nCaption number {number}\n\n",
                    number + 1,
                    time(start),
                    time(start + 1000)
                )
            })
            .collect()
    }

    const TWO_CAPTIONS: &str = "1\n00:00:01,000 --> 00:00:02,000\nFirst\n\n2\n00:00:03,000 --> 00:00:04,500\nSecond\nline\n";

    fn captions_on_top(editor: &Editor) -> Vec<bareclip_core::timeline::Clip> {
        editor.project.tracks.tracks()[0].clips().to_vec()
    }

    #[test]
    fn importing_captions_puts_each_at_its_time_above_everything_as_one_undo_step() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let video = editor.project.tracks.played().clips()[0];
        let path = subtitle_file("two", TWO_CAPTIONS);

        editor.import_captions(&path);
        run_frame(&ctx, &mut editor, vec![]);

        let captions = captions_on_top(&editor);
        assert_eq!(captions.len(), 2);
        assert_eq!(captions[0].start, Ticks::from_seconds(1.0));
        assert_eq!(captions[1].start, Ticks::from_seconds(3.0));
        assert_eq!(captions[1].duration(), Ticks::from_seconds(1.5));
        let second = editor.title_of(captions[1]).expect("it is a title");
        assert_eq!(second.text, "Second\nline");
        assert_eq!(second.style, TitleStyle::Subtitle);
        assert_eq!(second.spot, Some(TitleSpot::BottomMiddle));
        assert_eq!(
            editor.project.tracks.clip(video.id),
            Some(video),
            "nothing else moved"
        );
        assert!(editor.notice.is_none(), "this space is for problems");
        assert_eq!(
            editor.visible_media().count(),
            1,
            "they aren't in the media panel"
        );

        editor.undo();
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.project.tracks.tracks().len(), 1);
        assert!(editor.project.titles.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_subtitle_file_added_like_any_other_goes_in_as_captions() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let path = subtitle_file("dropped", TWO_CAPTIONS);
        editor.import(&ctx, std::slice::from_ref(&path));
        assert_eq!(captions_on_top(&editor).len(), 2);
        assert_eq!(editor.visible_media().count(), 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_file_with_no_captions_says_so_and_changes_nothing() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let path = subtitle_file("empty", "Just some notes\n");
        editor.import_captions(&path);
        assert!(editor.project.tracks.is_empty());
        assert!(!editor.history.can_undo());
        assert!(
            editor
                .notice
                .as_deref()
                .is_some_and(|said| said.contains("no captions"))
        );
        let _ = std::fs::remove_file(&path);

        editor.notice = None;
        editor.import_captions(&path);
        assert!(
            editor
                .notice
                .as_deref()
                .is_some_and(|said| said.contains("could not be opened"))
        );
    }

    #[test]
    fn only_titles_near_the_playhead_have_a_picture() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let path = subtitle_file("many", &long_subtitles(100));
        editor.import_captions(&path);
        run_frame(&ctx, &mut editor, vec![]);
        let drawn = |editor: &Editor| {
            editor
                .media
                .iter()
                .filter(|entry| entry.id.is_title())
                .count()
        };
        assert!(drawn(&editor) <= 2, "{} are drawn", drawn(&editor));

        editor.seek(Ticks::from_seconds(60.2));
        run_frame(&ctx, &mut editor, vec![]);
        assert!(drawn(&editor) <= 2, "{} are drawn", drawn(&editor));
        let showing = editor.project.tracks.stack_at(
            Ticks::from_seconds(60.2),
            editor.project.canvas.size(),
            &|id| editor.video_size(id),
        );
        let entry = editor
            .entry(showing[0].media)
            .expect("the one showing is drawn");
        assert!(entry.still.is_some());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn playing_through_captions_doesnt_make_a_saved_project_look_changed() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let path = subtitle_file("saved", &long_subtitles(20));
        editor.import_captions(&path);
        run_frame(&ctx, &mut editor, vec![]);
        let project =
            std::env::temp_dir().join(format!("bareclip-captions-{}.bareclip", std::process::id()));
        assert!(editor.save_to(&project));
        assert!(!editor.has_unsaved_changes());

        for seconds in [4.2, 15.2, 25.7] {
            editor.seek(Ticks::from_seconds(seconds));
            run_frame(&ctx, &mut editor, vec![]);
            assert!(!editor.has_unsaved_changes(), "at {seconds}");
        }

        let mut back = Editor::new(&ctx, false);
        back.open_project(&ctx, &project);
        run_frame(&ctx, &mut back, vec![]);
        assert_eq!(back.project.titles.len(), 20);
        assert!(!back.has_unsaved_changes());
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&project);
    }

    #[test]
    fn a_caption_comes_back_right_after_undo_even_if_it_was_drawn_since() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let path = subtitle_file("undo", TWO_CAPTIONS);
        editor.import_captions(&path);
        // Something to undo, from before the caption at 3 seconds was ever drawn.
        let first = captions_on_top(&editor)[0].id;
        editor.duplicate_clip(first);
        editor.seek(Ticks::from_seconds(3.2));
        run_frame(&ctx, &mut editor, vec![]);
        let drawn = captions_on_top(&editor)
            .last()
            .copied()
            .expect("it is there");

        editor.undo();
        editor.seek(Ticks::from_seconds(3.2));
        run_frame(&ctx, &mut editor, vec![]);

        let back = captions_on_top(&editor)
            .last()
            .copied()
            .expect("it is there");
        assert_eq!(
            back.placement.scale, drawn.placement.scale,
            "it still fits its picture"
        );
        let _ = std::fs::remove_file(&path);
    }

    fn selected_caption_with_keyboard(ctx: &egui::Context, editor: &mut Editor) -> ClipId {
        let id = with_a_title(ctx, editor, TitleStyle::Subtitle);
        editor.focus_title_text = true;
        run_frame(ctx, editor, vec![]);
        run_frame(ctx, editor, vec![]);
        id
    }

    #[test]
    fn enter_in_a_caption_starts_the_next_one_right_after_it_looking_the_same() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let first = selected_caption_with_keyboard(&ctx, &mut editor);
        let media = editor
            .project
            .tracks
            .clip(first)
            .expect("it is there")
            .media;
        editor.change_title(media, |title| {
            title.text = "One".to_owned();
            title.font = "Abel".to_owned();
        });
        run_frame(&ctx, &mut editor, key_event(Key::Enter, Modifiers::NONE));
        run_frame(&ctx, &mut editor, vec![]);

        let second = editor.selected.expect("the new one is selected");
        assert_ne!(second, first);
        let (one, two) = (
            editor.project.tracks.clip(first).expect("it is there"),
            editor.project.tracks.clip(second).expect("it is there"),
        );
        assert_eq!(two.start, one.end());
        assert_eq!(two.duration(), one.duration());
        assert_eq!(
            editor.project.tracks.track_of(first),
            editor.project.tracks.track_of(second)
        );
        let title = title_of_clip(&editor, second);
        assert_eq!(title.text, "");
        assert_eq!(title.font, "Abel");
        assert_eq!(title_of_clip(&editor, first).text, "One");
        assert_eq!(editor.position(), two.start, "it shows what is being typed");
        assert!(
            ctx.egui_wants_keyboard_input(),
            "the box keeps the keyboard"
        );

        editor.undo();
        assert!(editor.project.tracks.clip(second).is_none());
        assert_eq!(
            title_of_clip(&editor, first).text,
            "One",
            "the typing is its own step"
        );
    }

    #[test]
    fn enter_goes_to_the_caption_already_there_rather_than_making_another() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let path = subtitle_file(
            "touching",
            "00:00:01,000 --> 00:00:02,000\nOne\n\n00:00:02,020 --> 00:00:03,000\nTwo\n",
        );
        editor.import_captions(&path);
        let captions = captions_on_top(&editor);
        editor.selected = Some(captions[0].id);
        editor.right_panel = Some(RightPanel::Tool(LookTab::Text));
        editor.focus_title_text = true;
        run_frame(&ctx, &mut editor, vec![]);
        run_frame(&ctx, &mut editor, vec![]);

        run_frame(&ctx, &mut editor, key_event(Key::Enter, Modifiers::NONE));

        assert_eq!(editor.selected, Some(captions[1].id));
        assert_eq!(captions_on_top(&editor).len(), 2);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_new_caption_stops_short_of_the_next_one() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let path = subtitle_file(
            "room",
            "00:00:01,000 --> 00:00:05,000\nLong\n\n00:00:06,000 --> 00:00:07,000\nNext\n",
        );
        editor.import_captions(&path);
        let captions = captions_on_top(&editor);
        editor.selected = Some(captions[0].id);
        run_frame(&ctx, &mut editor, vec![]);

        editor.next_caption();

        let after = captions_on_top(&editor);
        assert_eq!(after.len(), 3);
        assert_eq!(after[1].start, Ticks::from_seconds(5.0));
        assert_eq!(after[1].duration(), Ticks::from_seconds(1.0));
        assert_eq!(
            after[2].start,
            Ticks::from_seconds(6.0),
            "the next one didn't move"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn enter_only_starts_a_new_caption_on_captions() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let id = with_a_title(&ctx, &mut editor, TitleStyle::Plain);
        editor.focus_title_text = true;
        run_frame(&ctx, &mut editor, vec![]);
        run_frame(&ctx, &mut editor, vec![]);
        run_frame(&ctx, &mut editor, key_event(Key::Enter, Modifiers::NONE));
        assert_eq!(editor.selected, Some(id));
        assert_eq!(editor.project.titles.len(), 1);
    }

    #[test]
    fn a_change_to_a_captions_look_goes_to_the_whole_row_but_never_its_words() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let path = subtitle_file("row", TWO_CAPTIONS);
        editor.import_captions(&path);
        let captions = captions_on_top(&editor);
        editor.selected = Some(captions[0].id);
        editor.right_panel = Some(RightPanel::Tool(LookTab::Text));
        run_frame(&ctx, &mut editor, vec![]);
        assert!(is_named(
            &ctx,
            &mut editor,
            "Change every caption on this row"
        ));

        editor.change_title_look(captions[0].media, |title| {
            title.font = "Abel".to_owned();
            title.colour = [1, 2, 3];
        });
        run_frame(&ctx, &mut editor, vec![]);
        for caption in &captions {
            let title = editor.title_of(*caption).expect("it is a title");
            assert_eq!(title.font, "Abel");
            assert_eq!(title.colour, [1, 2, 3]);
        }
        assert_eq!(
            editor.title_of(captions[0]).map(|t| t.text.as_str()),
            Some("First")
        );
        assert_eq!(
            editor.title_of(captions[1]).map(|t| t.text.as_str()),
            Some("Second\nline")
        );

        editor.undo();
        run_frame(&ctx, &mut editor, vec![]);
        for caption in &captions {
            assert_eq!(
                editor.title_of(*caption).map(|t| t.colour),
                Some([255, 255, 255])
            );
        }

        click_named(&ctx, &mut editor, "Change every caption on this row");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(!editor.style_whole_row);
        editor.change_title_look(captions[0].media, |title| title.colour = [9, 9, 9]);
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(
            editor.title_of(captions[0]).map(|t| t.colour),
            Some([9, 9, 9])
        );
        assert_eq!(
            editor.title_of(captions[1]).map(|t| t.colour),
            Some([255, 255, 255])
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn other_styles_have_no_row_to_change() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let id = with_a_title(&ctx, &mut editor, TitleStyle::Plain);
        editor.duplicate_clip(id);
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.caption_mates().is_empty());
        assert!(!is_named(
            &ctx,
            &mut editor,
            "Change every caption on this row"
        ));
    }

    #[test]
    fn a_title_moved_by_hand_stops_following_its_position() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let id = with_a_title(&ctx, &mut editor, TitleStyle::Plain);
        let clip = editor.project.tracks.clip(id).expect("it is there");
        let moved = Placement {
            center_x: clip.placement.center_x + 100_000,
            ..clip.placement
        };
        let event = stage::StageEvents {
            place: Some((id, moved)),
            ..Default::default()
        };
        editor.apply_stage_events(event, false);
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(title_of_clip(&editor, id).spot, None);

        set_title_text(&ctx, &mut editor, id, "Longer words than before");
        let after = editor.project.tracks.clip(id).expect("it is there");
        assert_eq!(
            after.placement.center_x, moved.center_x,
            "it stays where it was put"
        );
    }

    #[test]
    fn captions_export_at_their_own_times() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let path = subtitle_file(
            "export",
            "00:00:00,000 --> 00:00:01,000\nOne\n\n00:00:02,000 --> 00:00:03,000\nTwo\n",
        );
        editor.import_captions(&path);
        let captions = captions_on_top(&editor);
        editor.selected = Some(captions[0].id);
        run_frame(&ctx, &mut editor, vec![]);
        // A big white box at the bottom is easy to find in the finished video.
        editor.change_title_look(captions[0].media, |title| {
            title.background = Some([255, 255, 255]);
            title.colour = [255, 255, 255];
            title.size = 200_000;
        });
        run_frame(&ctx, &mut editor, vec![]);
        let output = output_called("captions");

        let report = exported_report(&ctx, &mut editor, &output);
        assert!(matches!(report, ProjectReport::Full(_)), "{report:?}");

        let mut reader = bareclip_media::PictureReader::open(&output).expect("it opens");
        let mut seen = Vec::new();
        while let Some(picture) = reader.next_picture().expect("it decodes") {
            let view = picture.view();
            let at = (view.height as usize * 9 / 10) * view.y_stride + view.width as usize / 2;
            seen.push((picture.time().as_seconds(), view.y[at]));
        }
        let bright_at = |moment: f64| {
            seen.iter()
                .min_by(|a, b| (a.0 - moment).abs().total_cmp(&(b.0 - moment).abs()))
                .is_some_and(|(_, level)| *level > 200)
        };
        assert!(bright_at(0.5), "the first shows");
        assert!(!bright_at(1.5), "nothing shows between them");
        assert!(bright_at(2.5), "the second shows");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&output);
    }

    /// What a long subtitle file costs. `./scripts/cargo.ps1 test --release -p bareclip-app --bin bareclip how_long_captions_take '--' --ignored --nocapture`
    #[test]
    #[ignore = "a measurement, not a check"]
    fn how_long_captions_take() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        for count in [500, 3000] {
            let path = subtitle_file(&format!("long-{count}"), &long_subtitles(count));
            let started = Instant::now();
            editor.import_captions(&path);
            println!("{count} captions: imported in {:?}", started.elapsed());
            run_frame(&ctx, &mut editor, vec![]);
            let started = Instant::now();
            for _ in 0..60 {
                run_frame(&ctx, &mut editor, vec![]);
            }
            println!("  one frame at rest: {:?}", started.elapsed() / 60);
            let started = Instant::now();
            for step in 0..60 {
                editor.seek(Ticks::from_seconds(f64::from(step) * 37.0));
                run_frame(&ctx, &mut editor, vec![]);
            }
            println!("  one frame jumping about: {:?}", started.elapsed() / 60);
            let started = Instant::now();
            let _ = editor.current_file();
            println!("  writing the project down: {:?}", started.elapsed());
            let started = Instant::now();
            let _ = editor.project.clone();
            println!("  taking an undo step: {:?}", started.elapsed());
            let _ = std::fs::remove_file(&path);
            editor.undo();
            run_frame(&ctx, &mut editor, vec![]);
        }
    }

    // Copying, pasting and how long a clip lasts.

    fn right_click_menu_on(ctx: &egui::Context, editor: &mut Editor, id: ClipId) {
        let row = editor.project.tracks.track_of(id).expect("it is on a row");
        let clip = editor.project.tracks.clip(id).expect("it is there");
        let scale = editor.timeline_state.scale;
        let x = scale.x_of(Ticks((clip.start.0 + clip.end().0) / 2));
        let y = editor.timeline_state.layout.track(row).center().y;
        secondary_click(ctx, editor, pos2(x, y));
        let_dialog_open(ctx, editor);
    }

    #[test]
    fn the_clip_menu_has_the_same_lines_on_every_kind_of_clip() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        let video = editor.project.tracks.played().clips()[0].id;
        let title = with_a_title(&ctx, &mut editor, TitleStyle::Plain);
        for id in [video, title] {
            right_click_menu_on(&ctx, &mut editor, id);
            for line in [
                "Duplicate",
                "Copy",
                "Paste",
                "Delete",
                "Split",
                "Edit duration",
            ] {
                assert!(is_named(&ctx, &mut editor, line), "{line}");
            }
            run_frame(&ctx, &mut editor, key_event(Key::Escape, Modifiers::NONE));
        }
    }

    #[test]
    fn copying_a_clip_and_pasting_it_puts_a_copy_at_the_playhead_as_one_undo_step() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        let original = editor.project.tracks.played().clips()[0].id;
        let source = editor
            .project
            .tracks
            .clip(original)
            .expect("it is there")
            .source;
        let end = editor
            .project
            .tracks
            .clip(original)
            .expect("it is there")
            .end();
        let placed = Placement {
            opacity: 500_000,
            ..Placement::default()
        };
        editor.project.tracks.set_placement(original, placed);

        run_frame(&ctx, &mut editor, key_event(Key::C, Modifiers::COMMAND));
        editor.seek(Ticks(end.0 + Ticks::from_seconds(5.0).0));
        run_frame(&ctx, &mut editor, vec![]);
        let at = editor.position();
        run_frame(&ctx, &mut editor, key_event(Key::V, Modifiers::COMMAND));

        let pasted = editor.selected.expect("the copy is selected");
        assert_ne!(pasted, original);
        let clip = editor
            .project
            .tracks
            .clip(pasted)
            .expect("it is on the timeline");
        assert_eq!(clip.source, source);
        assert_eq!(clip.placement, placed);
        assert_eq!(clip.start, at);
        assert_eq!(
            editor.project.tracks.track_of(pasted),
            editor.project.tracks.track_of(original)
        );

        editor.undo();
        assert!(editor.project.tracks.clip(pasted).is_none());
        assert!(editor.project.tracks.clip(original).is_some());
    }

    #[test]
    fn pasting_with_nothing_copied_does_nothing() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        run_frame(&ctx, &mut editor, key_event(Key::V, Modifiers::COMMAND));
        assert!(editor.project.tracks.is_empty());
        assert!(!editor.history.can_undo());
    }

    #[test]
    fn a_pasted_title_has_a_title_of_its_own_and_lands_on_a_row_of_titles() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let original = with_a_title(&ctx, &mut editor, TitleStyle::Plain);
        editor.copy_clip(original);
        editor.seek(Ticks::from_seconds(10.0));
        run_frame(&ctx, &mut editor, vec![]);
        editor.paste_clip();
        run_frame(&ctx, &mut editor, vec![]);

        let pasted = editor.selected.expect("the copy is selected");
        assert!(
            editor
                .project
                .tracks
                .is_text_row(editor.project.tracks.track_of(pasted).expect("it landed"))
        );
        set_title_text(&ctx, &mut editor, pasted, "Pasted");
        assert_eq!(
            title_of_clip(&editor, original).text,
            TitleStyle::Plain.name()
        );
    }

    #[test]
    fn a_copied_clip_goes_with_its_video_when_that_is_removed() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        let media = editor.media[0].id;
        editor.add_to_timeline(media);
        run_frame(&ctx, &mut editor, vec![]);
        editor.copy_clip(editor.selected.expect("it is selected"));
        editor.remove_media(media);
        assert_eq!(editor.visible_media().count(), 0);
        editor.paste_clip();
        assert_eq!(editor.visible_media().count(), 0);
        assert!(editor.project.tracks.is_empty());
    }

    #[test]
    fn typing_a_duration_makes_a_title_that_long_as_one_undo_step() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let id = with_a_title(&ctx, &mut editor, TitleStyle::Plain);
        editor.open_edit_duration(id);
        let_dialog_open(&ctx, &mut editor);
        assert_eq!(
            editor
                .duration_edit
                .as_ref()
                .map(|edit| edit.typed.as_str()),
            Some("5")
        );
        assert!(is_named(&ctx, &mut editor, "Duration"));

        editor.duration_edit.as_mut().expect("it is open").typed = "1:30".to_owned();
        click_named(&ctx, &mut editor, "Apply");
        run_frame(&ctx, &mut editor, vec![]);

        assert!(editor.duration_edit.is_none());
        let clip = editor.project.tracks.clip(id).expect("it is there");
        assert_eq!(clip.duration(), Ticks::from_seconds(90.0));
        editor.undo();
        let clip = editor.project.tracks.clip(id).expect("it is there");
        assert_eq!(clip.duration(), Ticks::from_seconds(5.0));
    }

    #[test]
    fn something_that_isnt_a_duration_leaves_the_clip_alone_and_the_window_open() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        let id = with_a_title(&ctx, &mut editor, TitleStyle::Plain);
        editor.open_edit_duration(id);
        let_dialog_open(&ctx, &mut editor);
        editor.duration_edit.as_mut().expect("it is open").typed = "soon".to_owned();
        click_named(&ctx, &mut editor, "Apply");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.duration_edit.is_some());
        assert!(editor.notice.is_some());
        let clip = editor.project.tracks.clip(id).expect("it is there");
        assert_eq!(clip.duration(), Ticks::from_seconds(5.0));
    }

    #[test]
    fn a_video_cant_be_made_longer_than_its_footage_and_says_so() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        let footage = editor.media[0].info.duration;
        let id = editor.project.tracks.played().clips()[0].id;

        assert!(editor.apply_duration(id, "9000"));

        let clip = editor.project.tracks.clip(id).expect("it is there");
        assert_eq!(clip.source.end, footage);
        assert!(editor.notice.is_some());
    }

    /// Each card that fills its square, at rest and at moments of its motion, early on because
    /// Circular opens within 0.35 s. Files are named after the style and the moment.
    /// `$env:BARECLIP_SCREENS = '<folder>'; ./scripts/cargo.ps1 test -p bareclip-app --bin bareclip draw_the_full_frame_cards '--' --ignored`
    #[test]
    #[ignore = "makes pictures to look at, not a check"]
    fn draw_the_full_frame_cards() {
        let folder = std::env::var_os("BARECLIP_SCREENS").map_or_else(
            || std::env::temp_dir().join("bareclip-screens"),
            PathBuf::from,
        );
        let ctx = context();
        let mut camera = Camera::new(folder);
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Text");
        for _ in 0..12 {
            run_frame(&ctx, &mut editor, vec![]);
        }
        let filling = bareclip_core::title::STYLE_GROUPS
            .iter()
            .flat_map(|(_, styles)| styles.iter().copied())
            .filter(|style| style.fills_its_card());
        for style in filling {
            let name = style.name();
            let label = format!("Add {name}");
            for _ in 0..60 {
                let rect = named_rect(&ctx, &mut editor, &label);
                if rect.top() > 120.0 && rect.bottom() < 760.0 {
                    break;
                }
                let delta = if rect.top() <= 120.0 { 60.0 } else { -60.0 };
                run_frame(
                    &ctx,
                    &mut editor,
                    vec![
                        egui::Event::PointerMoved(pos2(200.0, 400.0)),
                        egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            delta: vec2(0.0, delta),
                            modifiers: egui::Modifiers::NONE,
                            phase: egui::TouchPhase::Move,
                            source: egui::MouseWheelSource::Unknown,
                        },
                    ],
                );
            }
            for _ in 0..30 {
                run_frame(&ctx, &mut editor, vec![]);
            }
            let rect = named_rect(&ctx, &mut editor, &label);
            println!("{name} is at {rect:?}");
            let slug = name.replace(' ', "-");
            run_frame(
                &ctx,
                &mut editor,
                vec![egui::Event::PointerMoved(pos2(5.0, 790.0))],
            );
            camera.shoot(&ctx, &mut editor, vec![], &format!("{slug}-rest"));
            for seconds in [0.1, 0.2, 1.5, 2.85] {
                run_frame(
                    &ctx,
                    &mut editor,
                    vec![egui::Event::PointerMoved(rect.center())],
                );
                run_frame(&ctx, &mut editor, vec![]);
                let now = ctx.input(|input| input.time);
                let Some(hover) = editor.card_hover.as_mut() else {
                    break;
                };
                hover.play_to(seconds, now);
                camera.shoot(&ctx, &mut editor, vec![], &format!("{slug}-{seconds}"));
            }
        }
    }

    /// `$env:BARECLIP_SCREENS = '<folder>'; ./scripts/cargo.ps1 test -p bareclip-app --bin bareclip draw_the_title_screens '--' --ignored`
    #[test]
    #[ignore = "makes pictures to look at, not a check"]
    fn draw_the_title_screens() {
        let folder = std::env::var_os("BARECLIP_SCREENS").map_or_else(
            || std::env::temp_dir().join("bareclip-screens"),
            PathBuf::from,
        );
        let ctx = context();
        let mut camera = Camera::new(folder);
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound]);
        editor.use_gpu(LayerDisplay::new(
            camera.device.clone(),
            camera.queue.clone(),
            camera.renderer.clone(),
        ));
        for _ in 0..30 {
            run_frame(&ctx, &mut editor, vec![]);
            thread::sleep(Duration::from_millis(10));
        }
        for _ in 0..12 {
            run_frame(&ctx, &mut editor, vec![]);
        }
        camera.shoot(&ctx, &mut editor, vec![], "title-01-lanes");

        click_named(&ctx, &mut editor, "Text");
        for _ in 0..12 {
            run_frame(&ctx, &mut editor, vec![]);
        }
        camera.shoot(&ctx, &mut editor, vec![], "title-02-text-panel");

        click_named(&ctx, &mut editor, "See all");
        for _ in 0..12 {
            run_frame(&ctx, &mut editor, vec![]);
        }
        camera.shoot(&ctx, &mut editor, vec![], "title-02-see-all");
        click_named(&ctx, &mut editor, "Back to all the text");
        for _ in 0..4 {
            run_frame(&ctx, &mut editor, vec![]);
        }

        with_a_title(&ctx, &mut editor, TitleStyle::Outlined);
        for _ in 0..12 {
            run_frame(&ctx, &mut editor, vec![]);
        }
        camera.shoot(&ctx, &mut editor, vec![], "title-03-selected");

        click_named(&ctx, &mut editor, "Font: Hanken Grotesk");
        for _ in 0..24 {
            run_frame(&ctx, &mut editor, vec![]);
        }
        camera.shoot(&ctx, &mut editor, vec![], "title-04-font-list");
        run_frame(&ctx, &mut editor, key_event(Key::Escape, Modifiers::NONE));

        click_named(&ctx, &mut editor, "Text colour");
        for _ in 0..12 {
            run_frame(&ctx, &mut editor, vec![]);
        }
        camera.shoot(&ctx, &mut editor, vec![], "title-05-colour");
        run_frame(&ctx, &mut editor, key_event(Key::Escape, Modifiers::NONE));

        with_a_title(&ctx, &mut editor, TitleStyle::LowerThird);
        for _ in 0..12 {
            run_frame(&ctx, &mut editor, vec![]);
        }
        camera.shoot(&ctx, &mut editor, vec![], "title-06-two-titles");

        let id = editor.selected.expect("the title is selected");
        right_click_menu_on(&ctx, &mut editor, id);
        for _ in 0..12 {
            run_frame(&ctx, &mut editor, vec![]);
        }
        camera.shoot(&ctx, &mut editor, vec![], "title-07-clip-menu");
        run_frame(&ctx, &mut editor, key_event(Key::Escape, Modifiers::NONE));

        editor.open_edit_duration(id);
        for _ in 0..12 {
            run_frame(&ctx, &mut editor, vec![]);
        }
        camera.shoot(&ctx, &mut editor, vec![], "title-08-duration");
        editor.duration_edit = None;

        // A full-size project, to judge how sharp the words really are.
        let big = make_clip("title-screen-big", "1920x1080");
        let mut editor = with_videos(&ctx, &[&big]);
        editor.use_gpu(LayerDisplay::new(
            camera.device.clone(),
            camera.queue.clone(),
            camera.renderer.clone(),
        ));
        for _ in 0..30 {
            run_frame(&ctx, &mut editor, vec![]);
            thread::sleep(Duration::from_millis(10));
        }
        with_a_title(&ctx, &mut editor, TitleStyle::Underlined);
        for _ in 0..30 {
            run_frame(&ctx, &mut editor, vec![]);
            thread::sleep(Duration::from_millis(10));
        }
        camera.shoot(&ctx, &mut editor, vec![], "title-09-full-size");

        // Captions from a file, with the controls they add.
        let path = subtitle_file("screen", TWO_CAPTIONS);
        editor.import_captions(&path);
        editor.left_panel = LeftPanel::Text;
        let captions = captions_on_top(&editor);
        editor.selected = Some(captions[0].id);
        editor.seek(Ticks::from_seconds(1.2));
        for _ in 0..30 {
            run_frame(&ctx, &mut editor, vec![]);
            thread::sleep(Duration::from_millis(10));
        }
        camera.shoot(&ctx, &mut editor, vec![], "title-10-captions");
        let _ = std::fs::remove_file(&path);
    }

    /// `$env:BARECLIP_SCREENS = '<folder>'; ./scripts/cargo.ps1 test -p bareclip-app draw_the_sound_screens '--' --ignored`
    #[test]
    #[ignore = "needs a real GPU, and draws pictures to look at rather than checking anything"]
    fn draw_the_sound_screens() {
        let folder = std::env::var_os("BARECLIP_SCREENS").map_or_else(
            || std::env::temp_dir().join("bareclip-screens"),
            PathBuf::from,
        );
        let ctx = context();
        let mut camera = Camera::new(folder);
        let sound = clip_with_sound();
        let mut editor = with_videos(&ctx, &[&sound, &sound, song_file()]);
        for _ in 0..60 {
            run_frame(&ctx, &mut editor, vec![]);
            thread::sleep(Duration::from_millis(10));
        }
        camera.shoot(&ctx, &mut editor, vec![], "sound-01-rows");

        let first = picture_clip(&editor);
        editor.project.tracks.set_clip_sound(
            first.id,
            ClipSound {
                fade_in: Ticks::from_seconds(1.5).0,
                fade_out: Ticks::from_seconds(1.0).0,
                ..ClipSound::default()
            },
        );
        editor.selected = Some(first.id);
        run_frame(&ctx, &mut editor, vec![]);
        camera.shoot(&ctx, &mut editor, vec![], "sound-02-fades");

        click_named(&ctx, &mut editor, "Audio");
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "sound-03-controls");

        click_named(&ctx, &mut editor, "Detach sound");
        run_frame(&ctx, &mut editor, vec![]);
        camera.shoot(&ctx, &mut editor, vec![], "sound-04-detached");

        editor.selected = Some(sound_row_clip(&editor).id);
        run_frame(&ctx, &mut editor, vec![]);
        camera.shoot(&ctx, &mut editor, vec![], "sound-05-sound-clip-selected");

        editor.export_state = ExportState::Choosing;
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "sound-06-export-window");

        editor.export_state = ExportState::Closed;
        run_frame(&ctx, &mut editor, vec![]);
        // The Audio panel stays open from before, and pressing its lit button would close it.
        if !is_named(&ctx, &mut editor, "Noise suppression") {
            click_named(&ctx, &mut editor, "Audio");
        }
        let_dialog_open(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Noise suppression");
        let_dialog_open(&ctx, &mut editor);
        let switch = named_rect(&ctx, &mut editor, "Noise suppression");
        let rest = vec![egui::Event::PointerMoved(switch.center())];
        for _ in 0..40 {
            run_frame(&ctx, &mut editor, rest.clone());
            thread::sleep(Duration::from_millis(20));
        }
        camera.shoot(&ctx, &mut editor, rest, "sound-07-noise-suppression-on");
    }

    #[test]
    fn videos_in_different_formats_say_why_they_take_the_full_route() {
        let ctx = context();
        let editor = with_videos(&ctx, &[test_clip(), small_clip()]);

        let Route::Full(reason) = editor.export_route() else {
            panic!("these cannot be copied together");
        };

        assert!(reason.contains("160 x 90"), "{reason}");
    }

    #[test]
    fn the_export_window_says_which_route_it_will_take() {
        let ctx = context();
        let mut fast = with_videos(&ctx, &[test_clip(), second_clip()]);
        fast.export_state = ExportState::Choosing;
        let_dialog_open(&ctx, &mut fast);
        assert!(is_named(&ctx, &mut fast, "Frame-exact"));
        assert!(is_named(&ctx, &mut fast, "Fastest"));

        let mut full = with_videos(&ctx, &[test_clip(), small_clip()]);
        full.export_state = ExportState::Choosing;
        let_dialog_open(&ctx, &mut full);
        assert!(
            !is_named(&ctx, &mut full, "Frame-exact"),
            "nothing to choose"
        );
        assert!(is_named(&ctx, &mut full, "Choose file and export"));
    }

    #[test]
    fn videos_of_different_sizes_export_together_at_the_first_videos_size() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[small_clip(), test_clip()]);
        let output =
            std::env::temp_dir().join(format!("bareclip-ui-mixed-{}.mp4", std::process::id()));
        let _ = std::fs::remove_file(&output);

        editor.start_export_to(&ctx, output.clone());
        wait_for_export(&ctx, &mut editor);

        let ExportState::Done { report, .. } = &editor.export_state else {
            panic!("the export did not finish: {:?}", failure_of(&editor));
        };
        assert!(matches!(report, ProjectReport::Full(_)), "{report:?}");
        let info = probe(&output).expect("the export can be read");
        assert_eq!((info.video.width, info.video.height), (160, 90));
        assert_eq!(info.video.codec, "h264");
        assert!((info.duration.as_seconds() - 8.0).abs() < 0.1);
        let _ = std::fs::remove_file(&output);
    }

    fn failure_of(editor: &Editor) -> Option<&str> {
        match &editor.export_state {
            ExportState::Failed(message) => Some(message),
            _ => None,
        }
    }

    #[test]
    fn the_canvas_is_the_first_video_on_the_timeline() {
        let ctx = context();
        let editor = with_videos(&ctx, &[small_clip(), test_clip()]);

        assert_eq!(editor.canvas_size(), Some(vec2(160.0, 90.0)));
    }

    #[test]
    fn an_empty_timeline_has_no_canvas() {
        let ctx = context();
        let editor = in_panel_only(&ctx, &[test_clip()]);

        assert_eq!(editor.canvas_size(), None);
    }

    #[test]
    fn a_picture_of_another_shape_fits_inside_with_room_for_bars() {
        let stage = egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 1000.0));
        let frame = fitted(vec2(1920.0, 1080.0), stage);
        assert_eq!(frame.width(), 1000.0);
        assert_eq!(frame.height(), 562.5);

        let picture = fitted(vec2(1080.0, 1920.0), frame);

        assert_eq!(picture.height(), 562.5);
        assert!((picture.width() - 316.4).abs() < 0.1);
        assert_eq!(picture.center(), frame.center());
    }

    /// Waits for an export. A busy machine can make one slow, so this only gives up when one stops
    /// moving for a long time, which would be a real freeze, and says how far it had got.
    fn wait_for_export(ctx: &egui::Context, editor: &mut Editor) {
        let started = Instant::now();
        let mut furthest = f32::NEG_INFINITY;
        let mut moved = started;
        while let ExportState::Running(task) = &editor.export_state {
            let fraction = f32::from_bits(task.progress.load(Ordering::Relaxed));
            if fraction > furthest {
                furthest = fraction;
                moved = Instant::now();
            }
            assert!(
                moved.elapsed() < Duration::from_secs(60),
                "the export froze at {:.0}%, {:?} in",
                furthest * 100.0,
                started.elapsed()
            );
            assert!(
                started.elapsed() < Duration::from_secs(240),
                "the export is moving, but is only at {:.0}% after {:?}",
                furthest * 100.0,
                started.elapsed()
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

    /// Two videos of the same size on two tracks, both from the start, with the first on top.
    fn stacked_clips(ctx: &egui::Context) -> Editor {
        let mut editor = in_panel_only(ctx, &[test_clip(), second_clip()]);
        let ids: Vec<MediaId> = editor.media.iter().map(|entry| entry.id).collect();
        let source = TimeRange {
            start: Ticks(0),
            end: Ticks::from_seconds(3.0),
        };
        for id in ids.iter().rev() {
            editor
                .project
                .tracks
                .place(*id, source, Ticks(0), TrackTarget::New(0));
        }
        editor.seek(Ticks::from_seconds(1.0));
        wait_for_stage(ctx, &mut editor);
        editor
    }

    /// The top clip and the one under it.
    fn top_and_below(editor: &Editor) -> (ClipId, ClipId) {
        let tracks = editor.project.tracks.tracks();
        (tracks[0].clips()[0].id, tracks[1].clips()[0].id)
    }

    fn placement_of(editor: &Editor, id: ClipId) -> Placement {
        editor.project.tracks.clip(id).expect("the clip").placement
    }

    /// Runs frames until the picture and the controls on it are up.
    fn wait_for_stage(ctx: &egui::Context, editor: &mut Editor) {
        let started = Instant::now();
        while editor.stage_state.frame.is_none() {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the picture never came"
            );
            run_frame(ctx, editor, vec![]);
            thread::sleep(Duration::from_millis(5));
        }
        run_frame(ctx, editor, vec![]);
    }

    /// Runs frames until the layers have been drawn and have settled.
    fn wait_for_layers(ctx: &egui::Context, editor: &mut Editor) {
        for _ in 0..60 {
            run_frame(ctx, editor, vec![]);
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn stage_frame(editor: &Editor) -> egui::Rect {
        editor.stage_state.frame.expect("the stage was drawn")
    }

    /// A place on the canvas, in shares of it.
    fn on_canvas(editor: &Editor, x: f32, y: f32) -> egui::Pos2 {
        let frame = stage_frame(editor);
        frame.left_top() + vec2(frame.width() * x, frame.height() * y)
    }

    /// Half the size, away from every line it could snap to.
    fn small_in_a_corner() -> Placement {
        Placement {
            center_x: WHOLE * 3 / 10,
            center_y: WHOLE * 3 / 10,
            scale: WHOLE * 2 / 5,
            ..Placement::default()
        }
    }

    #[test]
    fn clicking_the_picture_selects_the_clip_on_top_there() {
        let ctx = context();
        let mut editor = stacked_clips(&ctx);
        let (top, below) = top_and_below(&editor);
        editor
            .project
            .tracks
            .set_placement(top, small_in_a_corner());
        run_frame(&ctx, &mut editor, vec![]);

        let on_top = on_canvas(&editor, 0.3, 0.3);
        click(&ctx, &mut editor, on_top);
        assert_eq!(editor.selected, Some(top));

        let on_below = on_canvas(&editor, 0.8, 0.8);
        click(&ctx, &mut editor, on_below);
        assert_eq!(editor.selected, Some(below));
    }

    #[test]
    fn clicking_where_no_clip_is_drawn_clears_the_selection() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let clip = editor.project.tracks.played().clips()[0].id;
        editor
            .project
            .tracks
            .set_placement(clip, small_in_a_corner());
        editor.selected = Some(clip);
        wait_for_stage(&ctx, &mut editor);

        let empty = on_canvas(&editor, 0.9, 0.9);
        click(&ctx, &mut editor, empty);

        assert_eq!(editor.selected, None);
    }

    #[test]
    fn a_clip_completely_hidden_cant_be_clicked_on_the_picture() {
        let ctx = context();
        let mut editor = stacked_clips(&ctx);
        let (top, _) = top_and_below(&editor);

        let middle = on_canvas(&editor, 0.5, 0.5);
        click(&ctx, &mut editor, middle);

        assert_eq!(editor.selected, Some(top));
    }

    #[test]
    fn dragging_a_clip_moves_it_as_one_undo_step() {
        let ctx = context();
        let mut editor = stacked_clips(&ctx);
        let (top, _) = top_and_below(&editor);
        editor
            .project
            .tracks
            .set_placement(top, small_in_a_corner());
        run_frame(&ctx, &mut editor, vec![]);
        let before = editor.project.clone();

        let from = on_canvas(&editor, 0.3, 0.3);
        let to = on_canvas(&editor, 0.37, 0.43);
        begin_drag(&ctx, &mut editor, from, to);
        end_drag(&ctx, &mut editor, to);

        let placement = placement_of(&editor, top);
        assert!(
            (placement.center_x - 370_000).abs() < 3_000,
            "{placement:?}"
        );
        assert!(
            (placement.center_y - 430_000).abs() < 3_000,
            "{placement:?}"
        );
        assert_eq!(placement.scale, WHOLE * 2 / 5);
        assert_eq!(editor.selected, Some(top), "dragging selects it too");

        editor.undo();
        assert_eq!(editor.project, before, "one undo undoes the whole drag");
        editor.redo();
        assert_eq!(placement_of(&editor, top), placement);
    }

    #[test]
    fn a_click_with_no_movement_changes_nothing_to_undo() {
        let ctx = context();
        let mut editor = stacked_clips(&ctx);
        let can_undo_before = editor.history.can_undo();

        let middle = on_canvas(&editor, 0.5, 0.5);
        click(&ctx, &mut editor, middle);

        assert_eq!(editor.history.can_undo(), can_undo_before);
    }

    #[test]
    fn dragging_a_corner_resizes_with_the_opposite_corner_still() {
        let ctx = context();
        let mut editor = stacked_clips(&ctx);
        let (top, _) = top_and_below(&editor);
        editor
            .project
            .tracks
            .set_placement(top, small_in_a_corner());
        editor.selected = Some(top);
        run_frame(&ctx, &mut editor, vec![]);
        let before = editor.project.clone();

        // It spans 0.1 to 0.5 each way, and its bottom right corner goes out to 0.7.
        let from = on_canvas(&editor, 0.5, 0.5);
        let to = on_canvas(&editor, 0.67, 0.67);
        begin_drag(&ctx, &mut editor, from, to);
        end_drag(&ctx, &mut editor, to);

        let placement = placement_of(&editor, top);
        let (scale, x, y) = (placement.scale, placement.center_x, placement.center_y);
        assert!((scale - 570_000).abs() < 8_000, "{placement:?}");
        let half = scale / 2;
        assert!(
            (x - half - 100_000).abs() < 2_000,
            "left stays: {placement:?}"
        );
        assert!(
            (y - half - 100_000).abs() < 2_000,
            "top stays: {placement:?}"
        );

        editor.undo();
        assert_eq!(editor.project, before);
    }

    #[test]
    fn a_drag_close_to_the_middle_snaps_there_and_shows_a_line_until_it_ends() {
        let ctx = context();
        let mut editor = stacked_clips(&ctx);
        let (top, _) = top_and_below(&editor);
        editor
            .project
            .tracks
            .set_placement(top, small_in_a_corner());
        run_frame(&ctx, &mut editor, vec![]);

        let from = on_canvas(&editor, 0.3, 0.3);
        let to = on_canvas(&editor, 0.501, 0.3);
        begin_drag(&ctx, &mut editor, from, to);

        assert_eq!(placement_of(&editor, top).center_x, WHOLE / 2);
        assert!(editor.stage_state.has_guides());
        end_drag(&ctx, &mut editor, to);
        assert!(!editor.stage_state.has_guides());
    }

    #[test]
    fn the_toolbar_only_shows_for_a_selected_clip() {
        let ctx = context();
        let mut editor = stacked_clips(&ctx);
        editor.selected = None;
        run_frame(&ctx, &mut editor, vec![]);
        assert!(!is_named(&ctx, &mut editor, "Rotate by 90 degrees"));

        let (top, _) = top_and_below(&editor);
        editor.selected = Some(top);
        run_frame(&ctx, &mut editor, vec![]);
        assert!(is_named(&ctx, &mut editor, "Rotate by 90 degrees"));
        click_named(&ctx, &mut editor, "More");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(is_named(&ctx, &mut editor, "Reset size and place"));
        assert!(is_named(&ctx, &mut editor, "Opacity"));
    }

    #[test]
    fn reset_puts_a_clip_back_as_one_undo_step() {
        let ctx = context();
        let mut editor = stacked_clips(&ctx);
        let (top, _) = top_and_below(&editor);
        editor
            .project
            .tracks
            .set_placement(top, small_in_a_corner());
        editor.selected = Some(top);
        run_frame(&ctx, &mut editor, vec![]);

        click_named(&ctx, &mut editor, "More");
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Reset size and place");

        assert!(placement_of(&editor, top).is_default());
        editor.undo();
        assert_eq!(placement_of(&editor, top), small_in_a_corner());
    }

    #[test]
    fn the_opacity_slider_changes_only_how_see_through_a_clip_is() {
        let ctx = context();
        let mut editor = stacked_clips(&ctx);
        let (top, _) = top_and_below(&editor);
        editor
            .project
            .tracks
            .set_placement(top, small_in_a_corner());
        editor.selected = Some(top);
        run_frame(&ctx, &mut editor, vec![]);

        click_named(&ctx, &mut editor, "More");
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Opacity");
        run_frame(&ctx, &mut editor, vec![]);

        let placement = placement_of(&editor, top);
        assert!(
            (placement.opacity - WHOLE / 2).abs() < WHOLE / 10,
            "{placement:?}"
        );
        assert_eq!(placement.scale, small_in_a_corner().scale);

        editor.undo();
        assert_eq!(placement_of(&editor, top).opacity, WHOLE);
    }

    #[test]
    fn pressing_and_dragging_the_opacity_slider_is_one_undo_step() {
        let ctx = context();
        let mut editor = stacked_clips(&ctx);
        let (top, _) = top_and_below(&editor);
        editor
            .project
            .tracks
            .set_placement(top, small_in_a_corner());
        editor.selected = Some(top);
        run_frame(&ctx, &mut editor, vec![]);
        let before = editor.project.clone();

        click_named(&ctx, &mut editor, "More");
        run_frame(&ctx, &mut editor, vec![]);
        let slider = named_rect(&ctx, &mut editor, "Opacity");
        let from = slider.center();
        let to = pos2(slider.left() + 4.0, from.y);
        begin_drag(&ctx, &mut editor, from, to);
        end_drag(&ctx, &mut editor, to);

        assert!(placement_of(&editor, top).opacity < WHOLE / 5);
        editor.undo();
        assert_eq!(editor.project, before);
    }

    #[test]
    fn the_arrow_keys_still_move_the_playhead_after_using_the_slider() {
        let ctx = context();
        let mut editor = stacked_clips(&ctx);
        let (top, _) = top_and_below(&editor);
        editor.selected = Some(top);
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "More");
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Opacity");
        run_frame(&ctx, &mut editor, vec![]);
        let opacity = placement_of(&editor, top).opacity;
        let position = editor.position();

        run_frame(
            &ctx,
            &mut editor,
            key_event(Key::ArrowLeft, Modifiers::SHIFT),
        );
        run_frame(&ctx, &mut editor, vec![]);

        assert!(editor.position() < position);
        assert_eq!(placement_of(&editor, top).opacity, opacity);
    }

    #[test]
    fn a_resized_clip_takes_the_full_route_and_says_why_and_reset_brings_the_fast_one_back() {
        let ctx = context();
        let mut editor = stacked_clips(&ctx);
        let (top, _) = top_and_below(&editor);
        assert_eq!(editor.export_route(), Route::Fast);

        editor
            .project
            .tracks
            .set_placement(top, small_in_a_corner());

        assert_eq!(
            editor.export_route(),
            Route::Full(LAYERED_REASON.to_owned())
        );
        assert_eq!(editor.videos_on_timeline().len(), 2);

        editor
            .project
            .tracks
            .set_placement(top, Placement::default());

        assert_eq!(editor.export_route(), Route::Fast);
        assert_eq!(editor.videos_on_timeline().len(), 1, "the other is hidden");
    }

    #[test]
    fn a_resized_clip_exports_through_the_real_editor() {
        let ctx = context();
        let mut editor = stacked_clips(&ctx);
        let (top, _) = top_and_below(&editor);
        editor
            .project
            .tracks
            .set_placement(top, small_in_a_corner());
        let output =
            std::env::temp_dir().join(format!("bareclip-ui-layers-{}.mp4", std::process::id()));
        let _ = std::fs::remove_file(&output);

        editor.start_export_to(&ctx, output.clone());
        wait_for_export(&ctx, &mut editor);

        let ExportState::Done { report, .. } = &editor.export_state else {
            panic!("the export did not finish: {:?}", failure_of(&editor));
        };
        let ProjectReport::Full(report) = report else {
            panic!("it took the fast route: {report:?}");
        };
        assert!(report.gpu.is_some(), "{report:?}");
        assert!(report.composited_frames > 0);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn a_layout_is_saved_and_comes_back_with_the_project() {
        let ctx = context();
        let mut editor = stacked_clips(&ctx);
        let (top, _) = top_and_below(&editor);
        editor
            .project
            .tracks
            .set_placement(top, small_in_a_corner());
        let folder = project_folder("layout");
        let path = folder.join("layout.bareclip");

        assert!(editor.save_to(&path));
        let mut reopened = Editor::new(&ctx, false);
        reopened.open_project(&ctx, &path);
        settle(&ctx, &mut reopened);
        run_frame(&ctx, &mut reopened, vec![]);

        let clip = reopened.project.tracks.tracks()[0].clips()[0];
        assert_eq!(clip.placement, small_in_a_corner());
    }

    #[test]
    fn the_picture_stays_up_at_the_very_end_of_a_project() {
        assert_eq!(last_moment_by(Ticks(100), Ticks(100)), Ticks(99));
        assert_eq!(last_moment_by(Ticks(50), Ticks(100)), Ticks(50));
        assert_eq!(last_moment_by(Ticks(0), Ticks(0)), Ticks(0));
    }

    /// What the Size menu calls a ratio, from its numbers.
    fn size_row(numbers: &str) -> String {
        SIZE_GROUPS
            .iter()
            .flat_map(|group| group.iter())
            .find(|choice| choice.numbers == numbers)
            .map_or_else(
                || numbers.to_owned(),
                |choice| format!("{} {}", choice.name, choice.numbers),
            )
    }

    fn open_size_menu(ctx: &egui::Context, editor: &mut Editor) {
        let button = named_nodes(ctx, editor)
            .into_iter()
            .find(|(label, ..)| label.starts_with("Size:"))
            .expect("the Size button is there")
            .1;
        click(ctx, editor, button.center());
        let_dialog_open(ctx, editor);
    }

    #[test]
    fn the_size_menu_lists_every_ratio_once_with_its_numbers() {
        let listed: Vec<_> = SIZE_GROUPS.iter().flat_map(|group| group.iter()).collect();
        assert_eq!(listed.len(), bareclip_core::canvas::RATIOS.len());
        for (ratio, numbers) in bareclip_core::canvas::RATIOS {
            let choice = listed
                .iter()
                .find(|choice| choice.ratio == ratio)
                .unwrap_or_else(|| panic!("{numbers} is listed"));
            assert_eq!(choice.numbers, numbers);
            assert!(!choice.name.is_empty() && !choice.note.is_empty());
        }
    }

    #[test]
    fn two_by_three_can_be_picked_from_the_size_menu() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);

        pick_size(&ctx, &mut editor, "2:3");

        assert_eq!(size_button_says(&ctx, &mut editor), "Size: 2:3");
        // The test video is 320 x 180, and encoders need even sizes.
        assert_eq!(editor.project.canvas.size(), (214, 320));
    }

    #[test]
    fn a_shapes_explanation_only_shows_once_the_pointer_has_rested_on_it() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        open_size_menu(&ctx, &mut editor);
        let whole = egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 800.0));
        let note = "Instagram Reels and TikTok";
        let row = named_rect(&ctx, &mut editor, "Vertical 9:16");
        let output = run_frame(&ctx, &mut editor, vec![]);
        assert!(!text_in(&output, whole).contains(&note.to_owned()));

        run_frame(&ctx, &mut editor, moved(row.center()));
        let mut output = run_frame(&ctx, &mut editor, vec![]);
        for _ in 0..20 {
            output = run_frame(&ctx, &mut editor, vec![]);
        }
        assert!(
            !text_in(&output, whole).contains(&note.to_owned()),
            "a third of a second in"
        );

        for _ in 0..30 {
            output = run_frame(&ctx, &mut editor, vec![]);
        }
        assert!(text_in(&output, whole).contains(&note.to_owned()));

        // Moving on starts the wait again, for what is now under the pointer.
        let other = named_rect(&ctx, &mut editor, "Square 1:1");
        run_frame(&ctx, &mut editor, moved(other.center()));
        for _ in 0..20 {
            output = run_frame(&ctx, &mut editor, vec![]);
        }
        let shown = text_in(&output, whole);
        assert!(!shown.contains(&note.to_owned()), "it has gone");
        assert!(
            !shown.contains(&"Instagram posts".to_owned()),
            "right at the wait's end"
        );
        for _ in 0..30 {
            output = run_frame(&ctx, &mut editor, vec![]);
        }
        assert!(text_in(&output, whole).contains(&"Instagram posts".to_owned()));
    }

    /// Picks a choice from the Size menu.
    fn pick_size(ctx: &egui::Context, editor: &mut Editor, name: &str) {
        let button = named_nodes(ctx, editor)
            .into_iter()
            .find(|(label, ..)| label.starts_with("Size:"))
            .expect("the Size button is there")
            .1;
        click(ctx, editor, button.center());
        run_frame(ctx, editor, vec![]);
        click_named(ctx, editor, &size_row(name));
        run_frame(ctx, editor, vec![]);
    }

    fn size_button_says(ctx: &egui::Context, editor: &mut Editor) -> String {
        named_nodes(ctx, editor)
            .into_iter()
            .find(|(label, ..)| label.starts_with("Size:"))
            .expect("the Size button is there")
            .0
    }

    #[test]
    fn an_empty_project_is_16_by_9_and_says_so() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);

        assert_eq!(editor.project.canvas.size(), (1920, 1080));
        assert_eq!(size_button_says(&ctx, &mut editor), "Size: 16:9");
    }

    #[test]
    fn the_first_video_on_the_timeline_sets_the_exact_size() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[small_clip(), test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);

        assert_eq!(editor.final_canvas(), Some((160, 90)));
        assert_eq!(size_button_says(&ctx, &mut editor), "Size: Original");
    }

    #[test]
    fn a_video_only_in_the_panel_sets_nothing() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[small_clip()]);
        run_frame(&ctx, &mut editor, vec![]);

        assert_eq!(editor.project.canvas.size(), (1920, 1080));
    }

    #[test]
    fn original_is_only_offered_once_there_is_a_video() {
        let ctx = context();
        let mut empty = Editor::new(&ctx, false);
        run_frame(&ctx, &mut empty, vec![]);
        let button = named_nodes(&ctx, &mut empty)
            .into_iter()
            .find(|(label, ..)| label.starts_with("Size:"))
            .expect("the Size button is there")
            .1;
        click(&ctx, &mut empty, button.center());
        run_frame(&ctx, &mut empty, vec![]);
        assert!(is_named(&ctx, &mut empty, "Cinema 21:9"));
        assert!(!is_named(&ctx, &mut empty, "Original"));
    }

    #[test]
    fn picking_a_ratio_changes_the_size_and_one_undo_brings_it_back() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.final_canvas(), Some((320, 180)));

        pick_size(&ctx, &mut editor, "9:16");

        assert_eq!(editor.final_canvas(), Some((180, 320)));
        assert_eq!(size_button_says(&ctx, &mut editor), "Size: 9:16");
        editor.undo();
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.final_canvas(), Some((320, 180)));
    }

    #[test]
    fn picking_original_goes_back_to_the_first_videos_size() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        pick_size(&ctx, &mut editor, "1:1");
        assert_eq!(editor.final_canvas(), Some((180, 180)));

        pick_size(&ctx, &mut editor, "Original");

        assert_eq!(editor.final_canvas(), Some((320, 180)));
    }

    #[test]
    fn a_ratio_picked_on_an_empty_project_stays_when_a_video_arrives() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        pick_size(&ctx, &mut editor, "9:16");

        let id = editor.media[0].id;
        editor.project.tracks.push(
            id,
            TimeRange {
                start: Ticks(0),
                end: Ticks::from_seconds(1.0),
            },
        );
        run_frame(&ctx, &mut editor, vec![]);

        assert_eq!(editor.project.canvas.name(), "9:16");
        assert_eq!(editor.final_canvas(), Some((180, 320)));
    }

    #[test]
    fn deleting_everything_goes_back_to_16_by_9_and_undo_brings_the_size_back() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[small_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        let clip = editor.project.tracks.played().clips()[0].id;

        editor.selected = Some(clip);
        editor.delete_selected();
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.project.canvas.size(), (1920, 1080));

        editor.undo();
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.project.canvas.size(), (160, 90));
    }

    #[test]
    fn a_project_of_the_first_videos_size_still_takes_the_fast_route() {
        let ctx = context();
        let editor = with_videos(&ctx, &[test_clip(), second_clip()]);

        assert_eq!(editor.export_route(), Route::Fast);
    }

    #[test]
    fn a_ratio_that_doesnt_fit_the_video_takes_the_full_route_and_says_why() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        pick_size(&ctx, &mut editor, "9:16");

        let Route::Full(reason) = editor.export_route() else {
            panic!("it should take the full route");
        };
        assert!(reason.contains("180 x 320"), "{reason}");
    }

    #[test]
    fn picking_a_ratio_exports_at_that_size_through_the_real_editor() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        pick_size(&ctx, &mut editor, "1:1");
        let output =
            std::env::temp_dir().join(format!("bareclip-ui-square-{}.mp4", std::process::id()));
        let _ = std::fs::remove_file(&output);

        editor.start_export_to(&ctx, output.clone());
        wait_for_export(&ctx, &mut editor);

        let ExportState::Done { report, .. } = &editor.export_state else {
            panic!("the export did not finish: {:?}", failure_of(&editor));
        };
        let ProjectReport::Full(report) = report else {
            panic!("it took the fast route: {report:?}");
        };
        assert_eq!((report.width, report.height), (180, 180));
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn a_projects_size_comes_back_when_it_is_opened_again() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        pick_size(&ctx, &mut editor, "4:5");
        let folder = project_folder("size");
        let path = folder.join("size.bareclip");
        assert!(editor.save_to(&path));

        let mut reopened = Editor::new(&ctx, false);
        reopened.open_project(&ctx, &path);
        settle(&ctx, &mut reopened);
        run_frame(&ctx, &mut reopened, vec![]);

        assert_eq!(reopened.project.canvas.name(), "4:5");
        assert_eq!(reopened.final_canvas(), editor.final_canvas());
        assert!(!reopened.has_unsaved_changes());
    }

    #[test]
    fn a_project_from_before_sizes_keeps_coming_out_at_its_largest_video() {
        let ctx = context();
        let folder = project_folder("old-size");
        let path = folder.join("old.bareclip");
        let saved = |media: u64| project::SavedClip {
            media,
            start: if media == 1 {
                0
            } else {
                Ticks::from_seconds(1.0).0
            },
            source_start: 0,
            source_end: Ticks::from_seconds(1.0).0,
            placement: Placement::default(),
            sound: Default::default(),
            speed: Default::default(),
            look: Default::default(),
            transition: None,
        };
        let mut file = ProjectFile {
            titles: Vec::new(),
            library: Vec::new(),
            audio_tracks: Vec::new(),
            row_sound: Vec::new(),
            version: 2,
            name: "old".to_owned(),
            media: vec![
                project::SavedMedia {
                    name: None,
                    recorded: false,
                    id: 1,
                    path: small_clip().to_path_buf(),
                },
                project::SavedMedia {
                    name: None,
                    recorded: false,
                    id: 2,
                    path: test_clip().to_path_buf(),
                },
            ],
            tracks: vec![vec![saved(1), saved(2)]],
            canvas: Canvas::default(),
        };
        file.version = 2;
        std::fs::write(&path, file.to_text()).expect("it can be written");

        let mut editor = Editor::new(&ctx, false);
        editor.open_project(&ctx, &path);
        settle(&ctx, &mut editor);
        run_frame(&ctx, &mut editor, vec![]);

        assert_eq!(editor.final_canvas(), Some((320, 180)));
        assert!(!editor.has_unsaved_changes());
    }

    #[test]
    fn the_rotate_button_turns_a_clip_a_quarter_and_takes_the_full_route() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let clip = editor.project.tracks.played().clips()[0].id;
        editor.selected = Some(clip);
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.export_route(), Route::Fast);

        click_named(&ctx, &mut editor, "Rotate by 90 degrees");

        assert_eq!(placement_of(&editor, clip).rotation, 90_000);
        assert_eq!(
            editor.export_route(),
            Route::Full(LAYERED_REASON.to_owned())
        );
        editor.undo();
        assert!(placement_of(&editor, clip).is_default());
        assert_eq!(editor.export_route(), Route::Fast);
    }

    #[test]
    fn the_flip_buttons_flip_a_clip_as_one_undo_step_each() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let clip = editor.project.tracks.played().clips()[0].id;
        editor.selected = Some(clip);
        run_frame(&ctx, &mut editor, vec![]);

        click_named(&ctx, &mut editor, "Flip");
        let_dialog_open(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Flip left to right");
        assert!(placement_of(&editor, clip).flip_x);
        click_named(&ctx, &mut editor, "Flip");
        let_dialog_open(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Flip top to bottom");
        assert!(placement_of(&editor, clip).flip_y);

        editor.undo();
        assert!(!placement_of(&editor, clip).flip_y);
        assert!(placement_of(&editor, clip).flip_x);
        editor.undo();
        assert!(!placement_of(&editor, clip).flip_x);
    }

    #[test]
    fn dragging_the_rotate_handle_turns_a_clip_as_one_undo_step() {
        let ctx = context();
        let mut editor = stacked_clips(&ctx);
        let (top, _) = top_and_below(&editor);
        editor
            .project
            .tracks
            .set_placement(top, small_in_a_corner());
        editor.selected = Some(top);
        run_frame(&ctx, &mut editor, vec![]);
        let before = editor.project.clone();
        let handle = editor.stage_state.rotate_handle.expect("the handle shows");
        let middle = on_canvas(&editor, 0.3, 0.3);
        let quarter_round = middle + vec2(-(handle.y - middle.y), 0.0);

        begin_drag(&ctx, &mut editor, handle, quarter_round);
        end_drag(&ctx, &mut editor, quarter_round);

        assert_eq!(placement_of(&editor, top).rotation, 90_000);
        assert_eq!(placement_of(&editor, top).scale, small_in_a_corner().scale);
        editor.undo();
        assert_eq!(editor.project, before);
    }

    #[test]
    fn four_quarter_turns_put_a_project_back_on_the_fast_route() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let clip = editor.project.tracks.played().clips()[0].id;
        editor.selected = Some(clip);
        run_frame(&ctx, &mut editor, vec![]);

        for _ in 0..4 {
            click_named(&ctx, &mut editor, "Rotate by 90 degrees");
            run_frame(&ctx, &mut editor, vec![]);
        }

        assert!(placement_of(&editor, clip).is_default());
        assert_eq!(editor.export_route(), Route::Fast);
    }

    fn pick_background(ctx: &egui::Context, editor: &mut Editor, name: &str) {
        click_named(ctx, editor, "Background colour");
        for _ in 0..3 {
            run_frame(ctx, editor, vec![]);
        }
        click_named(ctx, editor, name);
        run_frame(ctx, editor, vec![]);
    }

    #[test]
    fn a_new_project_has_a_black_background() {
        let ctx = context();
        let editor = Editor::new(&ctx, false);

        assert_eq!(editor.project.canvas.background, BLACK);
    }

    #[test]
    fn picking_a_swatch_sets_the_background_as_one_undo_step() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);

        pick_background(&ctx, &mut editor, "Orange");

        assert_eq!(editor.project.canvas.background, [255, 148, 34]);
        editor.undo();
        assert_eq!(editor.project.canvas.background, BLACK);
    }

    #[test]
    fn a_background_nothing_can_show_keeps_the_fast_route() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);

        pick_background(&ctx, &mut editor, "Orange");

        assert_eq!(editor.export_route(), Route::Fast);
    }

    #[test]
    fn a_background_showing_in_dead_space_takes_the_full_route() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip()]);
        let id = editor.media[0].id;
        editor.project.tracks.place(
            id,
            TimeRange {
                start: Ticks(0),
                end: Ticks::from_seconds(1.0),
            },
            Ticks::from_seconds(1.0),
            TrackTarget::New(0),
        );
        run_frame(&ctx, &mut editor, vec![]);

        pick_background(&ctx, &mut editor, "Red");

        assert_eq!(
            editor.export_route(),
            Route::Full(LAYERED_REASON.to_owned())
        );
    }

    #[test]
    fn a_background_showing_around_a_resized_clip_exports_through_the_real_editor() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let clip = editor.project.tracks.played().clips()[0].id;
        editor
            .project
            .tracks
            .set_placement(clip, small_in_a_corner());
        run_frame(&ctx, &mut editor, vec![]);
        pick_background(&ctx, &mut editor, "Blue");
        let output =
            std::env::temp_dir().join(format!("bareclip-ui-background-{}.mp4", std::process::id()));
        let _ = std::fs::remove_file(&output);

        editor.start_export_to(&ctx, output.clone());
        wait_for_export(&ctx, &mut editor);

        let ExportState::Done { .. } = &editor.export_state else {
            panic!("the export did not finish: {:?}", failure_of(&editor));
        };
        let mut reader = bareclip_media::PictureReader::open(&output).expect("it opens");
        let picture = reader
            .next_picture()
            .expect("it decodes")
            .expect("a picture");
        let view = picture.view();
        let far = (view.height as usize / 2 - 4) * view.uv_stride + view.width as usize / 2 - 4;
        assert!(view.u[far] > 180, "the background shows: {}", view.u[far]);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn a_background_comes_back_when_a_project_is_opened_again() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        pick_background(&ctx, &mut editor, "Navy");
        let folder = project_folder("background");
        let path = folder.join("background.bareclip");
        assert!(editor.save_to(&path));

        let mut reopened = Editor::new(&ctx, false);
        reopened.open_project(&ctx, &path);
        settle(&ctx, &mut reopened);
        run_frame(&ctx, &mut reopened, vec![]);

        assert_eq!(reopened.project.canvas.background, [0, 26, 122]);
        assert!(!reopened.has_unsaved_changes());
    }

    #[test]
    fn a_colour_code_sets_the_background_once_it_is_whole() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Background colour");
        for _ in 0..3 {
            run_frame(&ctx, &mut editor, vec![]);
        }
        let field = named_rect(&ctx, &mut editor, "Colour code");
        click(&ctx, &mut editor, field.center());
        run_frame(
            &ctx,
            &mut editor,
            vec![egui::Event::Key {
                key: Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::COMMAND,
            }],
        );

        run_frame(&ctx, &mut editor, vec![egui::Event::Text("#12".to_owned())]);
        assert_eq!(editor.project.canvas.background, BLACK, "not whole yet");

        run_frame(
            &ctx,
            &mut editor,
            vec![egui::Event::Text("3456".to_owned())],
        );
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.project.canvas.background, [0x12, 0x34, 0x56]);
    }

    fn open_more_menu(ctx: &egui::Context, editor: &mut Editor) {
        click_named(ctx, editor, "More");
        for _ in 0..3 {
            run_frame(ctx, editor, vec![]);
        }
    }

    /// Opens the part of a menu beside the line `name`, unless `inside` already shows.
    fn open_side_menu(ctx: &egui::Context, editor: &mut Editor, name: &str, inside: &str) {
        if is_named(ctx, editor, inside) {
            return;
        }
        click_named(ctx, editor, name);
        for _ in 0..3 {
            run_frame(ctx, editor, vec![]);
        }
    }

    #[test]
    fn picture_in_picture_puts_a_clip_small_in_a_corner_as_one_undo_step() {
        let ctx = context();
        let mut editor = stacked_clips(&ctx);
        let (top, _) = top_and_below(&editor);
        editor.selected = Some(top);
        run_frame(&ctx, &mut editor, vec![]);
        open_more_menu(&ctx, &mut editor);
        assert!(!is_named(&ctx, &mut editor, "Bottom right"), "one line");
        open_side_menu(&ctx, &mut editor, "Picture in picture", "Bottom right");

        click_named(&ctx, &mut editor, "Bottom right");

        let placed = placement_of(&editor, top);
        assert!(placed.scale < WHOLE / 2, "{placed:?}");
        assert!(
            placed.center_x > WHOLE / 2 && placed.center_y > WHOLE / 2,
            "{placed:?}"
        );
        editor.undo();
        assert!(placement_of(&editor, top).is_default());
    }

    #[test]
    fn the_order_control_changes_which_clip_is_in_front_as_one_undo_step() {
        let ctx = context();
        let mut editor = stacked_clips(&ctx);
        let (top, below) = top_and_below(&editor);
        editor.selected = Some(below);
        run_frame(&ctx, &mut editor, vec![]);
        open_more_menu(&ctx, &mut editor);
        open_side_menu(&ctx, &mut editor, "Order", "Bring to front");

        click_named(&ctx, &mut editor, "Bring to front");
        run_frame(&ctx, &mut editor, vec![]);

        assert_eq!(editor.project.tracks.track_of(below), Some(0));
        assert_eq!(editor.project.tracks.track_of(top), Some(1));
        assert_eq!(editor.selected, Some(below), "it stays selected");
        editor.undo();
        assert_eq!(editor.project.tracks.track_of(top), Some(0));
    }

    #[test]
    fn the_order_control_only_offers_what_can_happen() {
        let ctx = context();
        let mut editor = stacked_clips(&ctx);
        let (top, _) = top_and_below(&editor);
        editor.selected = Some(top);
        run_frame(&ctx, &mut editor, vec![]);
        open_more_menu(&ctx, &mut editor);
        open_side_menu(&ctx, &mut editor, "Order", "Bring forward");

        // The clip on top has nowhere further forward to go, so pressing that does nothing.
        click_named(&ctx, &mut editor, "Bring forward");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.project.tracks.track_of(top), Some(0));
        assert!(!editor.history.can_undo());

        click_named(&ctx, &mut editor, "Send backward");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.project.tracks.track_of(top), Some(1));
    }

    #[test]
    fn a_clip_selected_on_the_timeline_but_completely_hidden_can_be_brought_forward() {
        let ctx = context();
        let mut editor = stacked_clips(&ctx);
        let (_, below) = top_and_below(&editor);
        editor.selected = Some(below);
        run_frame(&ctx, &mut editor, vec![]);
        // The clip above covers it completely, so only the timeline can select it.
        assert_eq!(editor.export_route(), Route::Fast);

        open_more_menu(&ctx, &mut editor);
        open_side_menu(&ctx, &mut editor, "Order", "Bring forward");
        click_named(&ctx, &mut editor, "Bring forward");
        run_frame(&ctx, &mut editor, vec![]);

        assert_eq!(
            editor
                .project
                .tracks
                .topmost_at(Ticks::from_seconds(1.0))
                .map(|c| c.id),
            Some(below)
        );
        assert_eq!(
            editor.export_route(),
            Route::Fast,
            "still one picture at a time"
        );
    }

    #[test]
    fn cropping_cuts_a_side_away_and_leaves_the_rest_where_it_was_as_one_undo_step() {
        let ctx = context();
        let mut editor = stacked_clips(&ctx);
        let (top, _) = top_and_below(&editor);
        editor
            .project
            .tracks
            .set_placement(top, small_in_a_corner());
        editor.selected = Some(top);
        run_frame(&ctx, &mut editor, vec![]);
        let before = editor.project.clone();

        click_named(&ctx, &mut editor, "Crop");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.stage_state.cropping(), Some(top));

        // It spans 0.1 to 0.5 across, so its left edge in the middle of its height is at (0.1, 0.3).
        let from = on_canvas(&editor, 0.1, 0.3);
        let to = on_canvas(&editor, 0.2, 0.3);
        begin_drag(&ctx, &mut editor, from, to);
        end_drag(&ctx, &mut editor, to);

        let cropped = placement_of(&editor, top);
        assert!(
            (cropped.crop_left - WHOLE / 4).abs() < WHOLE / 50,
            "{cropped:?}"
        );
        assert_eq!(cropped.crop_right, 0);
        assert_eq!(
            cropped.scale,
            small_in_a_corner().scale,
            "it doesn't resize"
        );
        let whole = cropped.uncropped((320, 180), (320, 180));
        assert!(
            (whole.center_x - 300_000).abs() < 100,
            "the rest stays: {whole:?}"
        );

        editor.undo();
        assert_eq!(editor.project, before);
    }

    #[test]
    fn a_drag_while_cropping_never_moves_or_resizes_the_clip() {
        let ctx = context();
        let mut editor = stacked_clips(&ctx);
        let (top, _) = top_and_below(&editor);
        editor
            .project
            .tracks
            .set_placement(top, small_in_a_corner());
        editor.selected = Some(top);
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Crop");
        run_frame(&ctx, &mut editor, vec![]);

        let from = on_canvas(&editor, 0.3, 0.3);
        let to = on_canvas(&editor, 0.4, 0.4);
        begin_drag(&ctx, &mut editor, from, to);
        end_drag(&ctx, &mut editor, to);

        assert_eq!(placement_of(&editor, top), small_in_a_corner());
    }

    #[test]
    fn finishing_clicking_away_or_deselecting_all_end_cropping() {
        let ctx = context();
        let mut editor = stacked_clips(&ctx);
        let (top, _) = top_and_below(&editor);
        editor
            .project
            .tracks
            .set_placement(top, small_in_a_corner());
        editor.selected = Some(top);
        run_frame(&ctx, &mut editor, vec![]);
        let begin = |ctx: &egui::Context, editor: &mut Editor| {
            click_named(ctx, editor, "Crop");
            run_frame(ctx, editor, vec![]);
            assert_eq!(editor.stage_state.cropping(), Some(top));
        };

        begin(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Finish cropping");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.stage_state.cropping(), None);

        begin(&ctx, &mut editor);
        let away = on_canvas(&editor, 0.9, 0.9);
        click(&ctx, &mut editor, away);
        assert_eq!(editor.stage_state.cropping(), None);
        assert_eq!(editor.selected, Some(top), "it stays selected");

        begin(&ctx, &mut editor);
        editor.selected = None;
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.stage_state.cropping(), None);
    }

    #[test]
    fn reset_crop_puts_back_what_was_cut_with_the_rest_still_in_place() {
        let ctx = context();
        let mut editor = stacked_clips(&ctx);
        let (top, _) = top_and_below(&editor);
        let start = small_in_a_corner();
        let cropped = start.cropped_to(
            bareclip_core::placement::CropSides {
                left: 0.25,
                ..Default::default()
            },
            (320, 180),
            (320, 180),
        );
        editor.project.tracks.set_placement(top, cropped);
        editor.selected = Some(top);
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Crop");
        run_frame(&ctx, &mut editor, vec![]);

        click_named(&ctx, &mut editor, "Reset crop");
        run_frame(&ctx, &mut editor, vec![]);

        let back = placement_of(&editor, top);
        assert!(!back.is_cropped());
        assert!((back.center_x - start.center_x).abs() <= 2, "{back:?}");
    }

    #[test]
    fn a_cropped_clip_takes_the_full_route() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let clip = editor.project.tracks.played().clips()[0].id;
        assert_eq!(editor.export_route(), Route::Fast);

        editor.project.tracks.set_placement(
            clip,
            Placement {
                crop_left: WHOLE / 10,
                ..Placement::default()
            },
        );

        assert_eq!(
            editor.export_route(),
            Route::Full(LAYERED_REASON.to_owned())
        );
    }

    #[test]
    fn a_crop_turn_and_flip_come_back_when_a_project_is_opened_again() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let clip = editor.project.tracks.played().clips()[0].id;
        let placement = Placement {
            rotation: 33_000,
            flip_y: true,
            crop_bottom: WHOLE / 5,
            ..small_in_a_corner()
        };
        editor.project.tracks.set_placement(clip, placement);
        let folder = project_folder("look");
        let path = folder.join("look.bareclip");
        assert!(editor.save_to(&path));

        let mut reopened = Editor::new(&ctx, false);
        reopened.open_project(&ctx, &path);
        settle(&ctx, &mut reopened);
        run_frame(&ctx, &mut reopened, vec![]);

        assert_eq!(
            reopened.project.tracks.tracks()[0].clips()[0].placement,
            placement
        );
        assert!(!reopened.has_unsaved_changes());
    }

    fn library_item(key: &str) -> &'static bareclip_core::art::ArtItem {
        bareclip_core::art::item_named(key).expect("it is in the library")
    }

    fn art_of_clip(editor: &Editor, id: ClipId) -> Art {
        title_of_clip(editor, id)
            .art
            .expect("it is from the library")
    }

    /// What screen readers call a category's or collection's card.
    fn group_name(name: &str, count: usize) -> String {
        format!("Open {name}, {count} items")
    }

    #[test]
    fn pressing_a_shape_in_the_library_adds_it_at_the_playhead_on_a_new_row() {
        use bareclip_core::art::{ArtKind, CATALOGUE, items_in};
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        editor.seek(Ticks::from_seconds(1.0));
        let video = editor.project.tracks.played().clips()[0].id;
        let video_start = editor.project.tracks.clip(video).map(|clip| clip.start);
        let rows = editor.project.tracks.tracks().len();
        run_frame(&ctx, &mut editor, vec![]);

        click_named(&ctx, &mut editor, "Content library");
        let shapes = CATALOGUE
            .iter()
            .filter(|item| item.kind == ArtKind::Shape)
            .count();
        click_named(&ctx, &mut editor, &group_name("Shapes", shapes));
        let basic = items_in(ArtKind::Shape, "Basic").len();
        click_named(&ctx, &mut editor, &group_name("Basic", basic));
        assert!(has_named(&ctx, &mut editor, "Add Heart"));
        click_named(&ctx, &mut editor, "Add Square");
        run_frame(&ctx, &mut editor, vec![]);

        let id = editor.selected.expect("the shape is selected");
        let clip = editor
            .project
            .tracks
            .clip(id)
            .expect("it is on the timeline");
        assert_eq!(art_of_clip(&editor, id).item, "square");
        assert_eq!(clip.start, Ticks::from_seconds(1.0));
        assert_eq!(clip.duration(), Ticks::from_seconds(5.0));
        assert_eq!(editor.project.tracks.tracks().len(), rows + 1);
        assert_eq!(editor.project.tracks.track_of(id), Some(0));
        assert_eq!(
            editor.project.tracks.clip(video).map(|clip| clip.start),
            video_start,
            "adding a shape moves nothing else"
        );
        assert_eq!(
            editor.clip_tabs(),
            vec![
                LookTab::Shape,
                LookTab::Animate,
                LookTab::Effects,
                LookTab::Colours
            ]
        );
        assert!(
            editor.project.library.is_empty(),
            "a shape gets no card in My media"
        );
        assert!(editor.right_panel.is_none(), "adding one opens nothing");

        editor.undo();
        assert!(
            editor.project.tracks.clip(id).is_none(),
            "one undo takes it away"
        );
    }

    #[test]
    fn the_chips_step_back_out_of_a_collection() {
        use bareclip_core::art::{ArtKind, items_in};
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Content library");
        let frames = bareclip_core::art::CATALOGUE
            .iter()
            .filter(|item| item.kind == ArtKind::Frame)
            .count();
        click_named(&ctx, &mut editor, &group_name("Frames & borders", frames));
        let corners = items_in(ArtKind::Frame, "Corners").len();
        click_named(&ctx, &mut editor, &group_name("Corners", corners));
        assert!(has_named(&ctx, &mut editor, "Add Round corners"));

        click_named(&ctx, &mut editor, "Corners");
        assert!(has_named(
            &ctx,
            &mut editor,
            &group_name("Corners", corners)
        ));
        click_named(&ctx, &mut editor, "All");
        assert!(has_named(
            &ctx,
            &mut editor,
            &group_name("Frames & borders", frames)
        ));
    }

    #[test]
    fn searching_the_library_finds_things_in_every_category() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Content library");
        editor.library_view.search = "arrow".to_owned();
        let names: Vec<String> = named_nodes(&ctx, &mut editor)
            .into_iter()
            .map(|(name, ..)| name)
            .filter(|name| name.starts_with("Add "))
            .collect();
        for wanted in ["Add Arrow", "Add Thin arrow", "Add Loop arrow"] {
            assert!(
                names.iter().any(|name| name == wanted),
                "{wanted} in {names:?}"
            );
        }
        assert!(!names.iter().any(|name| name == "Add Square"));
    }

    #[test]
    fn an_annotation_lasts_four_seconds_and_gets_one_card_in_my_media() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        let circle = library_item("marker-circle");

        editor.add_art(Art::of(circle), Ticks::from_seconds(0.5), None, None);
        let first = editor.selected.expect("it is selected");
        editor.add_art(Art::of(circle), Ticks::from_seconds(0.5), None, None);
        run_frame(&ctx, &mut editor, vec![]);

        let clip = editor.project.tracks.clip(first).expect("it is there");
        assert_eq!(clip.start, Ticks::from_seconds(0.5));
        assert_eq!(clip.duration(), Ticks::from_seconds(4.0));
        assert_eq!(
            editor.project.library.len(),
            1,
            "adding it twice makes one card"
        );
        assert_eq!(editor.project.library[0].item, "marker-circle");
        assert!(has_named(
            &ctx,
            &mut editor,
            "Marker circle from the library"
        ));
        assert_eq!(
            editor.clip_tabs(),
            vec![
                LookTab::Fade,
                LookTab::Filters,
                LookTab::Effects,
                LookTab::Colours
            ]
        );

        editor.undo();
        editor.undo();
        assert!(
            editor.project.library.is_empty(),
            "undo takes the card away too"
        );
    }

    #[test]
    fn a_frame_covers_the_whole_project_from_its_start() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        let length = editor.project.tracks.duration();
        editor.seek(Ticks::from_seconds(1.0));

        editor.add_art(
            Art::of(library_item("film-strip")),
            Ticks::from_seconds(1.0),
            None,
            None,
        );
        let id = editor.selected.expect("it is selected");

        let clip = editor.project.tracks.clip(id).expect("it is there");
        assert_eq!(clip.start, Ticks(0));
        assert_eq!(clip.duration(), length);
        assert_eq!(
            editor.project.tracks.duration(),
            length,
            "it makes nothing longer"
        );
    }

    #[test]
    fn something_in_an_empty_project_still_gets_a_length() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);

        editor.add_art(Art::of(library_item("snow")), Ticks(0), None, None);
        run_frame(&ctx, &mut editor, vec![]);

        let id = editor.selected.expect("it is selected");
        let clip = editor.project.tracks.clip(id).expect("it is there");
        assert_eq!(clip.duration(), Ticks::from_seconds(5.0));
    }

    #[test]
    fn dropping_a_library_item_on_the_timeline_adds_it_where_it_lands() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);

        let events = timeline::TimelineEvents {
            drop_art: Some(("star", Ticks::from_seconds(2.0), TrackTarget::New(0))),
            ..Default::default()
        };
        editor.apply_timeline_events(events, false);
        run_frame(&ctx, &mut editor, vec![]);

        let id = editor.selected.expect("it is selected");
        let clip = editor.project.tracks.clip(id).expect("it is there");
        assert_eq!(clip.start, Ticks::from_seconds(2.0));
        assert_eq!(art_of_clip(&editor, id).item, "star");
    }

    #[test]
    fn dropping_a_shape_on_the_picture_puts_it_there() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);

        editor.add_art(
            Art::of(library_item("heart")),
            Ticks(0),
            None,
            Some((0.25, 0.75)),
        );
        run_frame(&ctx, &mut editor, vec![]);

        let id = editor.selected.expect("it is selected");
        let clip = editor.project.tracks.clip(id).expect("it is there");
        assert_eq!(clip.placement.center_x, WHOLE / 4);
        assert_eq!(clip.placement.center_y, WHOLE * 3 / 4);
        assert_eq!(
            title_of_clip(&editor, id).spot,
            None,
            "it stays where it was put"
        );
    }

    #[test]
    fn the_bar_recolours_a_shape_and_each_change_is_one_undo_step() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        editor.add_art(Art::of(library_item("circle")), Ticks(0), None, None);
        let id = editor.selected.expect("it is selected");
        run_frame(&ctx, &mut editor, vec![]);

        let events = stage::StageEvents {
            art: Some((id, stage::ArtChange::Fill(Some([10, 20, 30])))),
            ..Default::default()
        };
        editor.apply_stage_events(events, false);
        run_frame(&ctx, &mut editor, vec![]);
        let events = stage::StageEvents {
            art: Some((id, stage::ArtChange::Outline(12))),
            ..Default::default()
        };
        editor.apply_stage_events(events, false);
        run_frame(&ctx, &mut editor, vec![]);

        let art = art_of_clip(&editor, id);
        assert_eq!(art.fill, Some([10, 20, 30]));
        assert_eq!(art.outline, 12);
        editor.undo();
        run_frame(&ctx, &mut editor, vec![]);
        let art = art_of_clip(&editor, id);
        assert_eq!(art.fill, Some([10, 20, 30]), "only the outline went back");
        assert_eq!(art.outline, 0);

        let events = stage::StageEvents {
            art: Some((id, stage::ArtChange::MoreOptions)),
            ..Default::default()
        };
        editor.apply_stage_events(events, false);
        assert_eq!(editor.right_panel, Some(RightPanel::Tool(LookTab::Shape)));
    }

    #[test]
    fn position_lines_a_shape_up_one_way_at_a_time() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        editor.add_art(Art::of(library_item("square")), Ticks(0), None, None);
        let id = editor.selected.expect("it is selected");
        for _ in 0..3 {
            run_frame(&ctx, &mut editor, vec![]);
        }
        let open_position = |ctx: &egui::Context, editor: &mut Editor| {
            // Picking a line leaves the menu open, so another can be picked straight after.
            if !is_named(ctx, editor, "Position") {
                open_more_menu(ctx, editor);
            }
            assert!(!is_named(ctx, editor, "Top left"), "one line, not nine");
            if !is_named(ctx, editor, "Left") {
                click_named(ctx, editor, "Position");
            }
            for _ in 0..3 {
                run_frame(ctx, editor, vec![]);
            }
        };

        open_position(&ctx, &mut editor);
        for line in ["Left", "Centre", "Right", "Top", "Middle", "Bottom"] {
            assert!(is_named(&ctx, &mut editor, line), "{line}");
        }
        click_named(&ctx, &mut editor, "Left");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(
            title_of_clip(&editor, id).spot,
            Some(TitleSpot::MiddleLeft),
            "it keeps its place top to bottom"
        );
        editor.undo();
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(title_of_clip(&editor, id).spot, Some(TitleSpot::Middle));

        // Moved by hand, it only moves the one way.
        let placed = placement_of(&editor, id);
        let moved = Placement {
            center_x: WHOLE / 3,
            center_y: WHOLE / 4,
            ..placed
        };
        editor.apply_stage_events(
            stage::StageEvents {
                place: Some((id, moved)),
                ..Default::default()
            },
            false,
        );
        for _ in 0..3 {
            run_frame(&ctx, &mut editor, vec![]);
        }
        open_position(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Bottom");
        run_frame(&ctx, &mut editor, vec![]);
        let after = placement_of(&editor, id);
        assert_eq!(after.center_x, WHOLE / 3);
        assert!(after.center_y > WHOLE / 2, "{after:?}");
        assert_eq!(title_of_clip(&editor, id).spot, None);
    }

    #[test]
    fn choosing_an_animation_shows_it_from_the_start_of_the_clip() {
        use bareclip_core::art::Animate;
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        editor.add_art(Art::of(library_item("flower")), Ticks(0), None, None);
        let id = editor.selected.expect("it is selected");
        editor.open_clip_panel(id, LookTab::Animate);
        editor.seek(Ticks::from_seconds(2.0));
        run_frame(&ctx, &mut editor, vec![]);

        click_named(&ctx, &mut editor, "Natural");
        run_frame(&ctx, &mut editor, vec![]);

        assert_eq!(art_of_clip(&editor, id).animate, Animate::Natural);
        assert_eq!(editor.position(), Ticks(0));
        editor.undo();
        assert_eq!(art_of_clip(&editor, id).animate, Animate::None);
    }

    #[test]
    fn the_clip_menu_offers_the_same_tools_as_the_rail() {
        use bareclip_core::art::ArtKind;
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        for (key, kind) in [
            ("square", ArtKind::Shape),
            ("tick", ArtKind::Annotation),
            ("corners", ArtKind::Frame),
            ("rain", ArtKind::Overlay),
            ("sunset", ArtKind::Background),
        ] {
            editor.add_art(Art::of(library_item(key)), Ticks(0), None, None);
            let menu: Vec<LookTab> = timeline::menu_tools(true, Some(kind))
                .into_iter()
                .map(|(_, _, tab)| tab)
                .collect();
            let mut rail = editor.clip_tabs();
            let mut sorted = menu.clone();
            rail.sort_by_key(|tab| format!("{tab:?}"));
            sorted.sort_by_key(|tab| format!("{tab:?}"));
            assert_eq!(sorted, rail, "{key}");
        }
    }

    #[test]
    fn library_items_and_their_cards_save_and_come_back() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);
        run_frame(&ctx, &mut editor, vec![]);
        editor.add_art(Art::of(library_item("speech-bubble")), Ticks(0), None, None);
        let shape = editor.selected.expect("it is selected");
        let media = editor.project.tracks.clip(shape).expect("there").media;
        editor.change_art(media, |art| {
            art.fill = Some([200, 10, 10]);
            art.animate = bareclip_core::art::Animate::BounceScale;
        });
        editor.add_art(Art::of(library_item("dust")), Ticks(0), None, None);
        run_frame(&ctx, &mut editor, vec![]);
        let saved = art_of_clip(&editor, shape);
        let path =
            std::env::temp_dir().join(format!("bareclip-library-{}.bareclip", std::process::id()));
        assert!(editor.save_to(&path), "it saves");

        let mut back = Editor::new(&ctx, false);
        back.open_project(&ctx, &path);
        run_frame(&ctx, &mut back, vec![]);

        assert!(!back.has_unsaved_changes(), "opening it changes nothing");
        let arts: Vec<Art> = back
            .project
            .tracks
            .tracks()
            .iter()
            .flat_map(|row| row.clips())
            .filter_map(|clip| back.art_of(*clip).cloned())
            .collect();
        assert!(arts.contains(&saved));
        assert!(arts.iter().any(|art| art.item == "dust"));
        assert_eq!(back.project.library.len(), 1);
        assert_eq!(back.project.library[0].item, "dust");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_rating_titles_panel_keeps_its_width_beside_the_picture() {
        let ctx = context();
        // The card holding the picture ends where the panel starts, and Export sits at its edge.
        let edge_of_picture_card = |style: TitleStyle| {
            let mut editor = with_videos(&ctx, &[test_clip()]);
            run_frame(&ctx, &mut editor, vec![]);
            with_a_title(&ctx, &mut editor, style);
            settle_animations(&ctx, &mut editor);
            named_rect(&ctx, &mut editor, "Export").right()
        };
        let plain = edge_of_picture_card(TitleStyle::Plain);
        let rating = edge_of_picture_card(TitleStyle::Rating);
        assert!(
            (plain - rating).abs() < 1.0,
            "the Stars slider let the picture's card grow over the panel, from {plain} to {rating}"
        );
    }

    #[test]
    fn the_shape_panel_keeps_its_width_beside_the_picture() {
        let ctx = context();
        let edge_with = |tab: LookTab| {
            let mut editor = with_videos(&ctx, &[test_clip()]);
            run_frame(&ctx, &mut editor, vec![]);
            editor.add_art(Art::of(library_item("square")), Ticks(0), None, None);
            let id = editor.selected.expect("it is selected");
            editor.open_clip_panel(id, tab);
            settle_animations(&ctx, &mut editor);
            named_rect(&ctx, &mut editor, "Export").right()
        };
        let animate = edge_with(LookTab::Animate);
        let shape = edge_with(LookTab::Shape);
        assert!((animate - shape).abs() < 1.0, "{animate} against {shape}");
    }

    #[test]
    fn a_library_clip_says_what_it_is_on_the_timeline() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        editor.add_art(Art::of(library_item("cog")), Ticks(0), None, None);
        let id = editor.selected.expect("it is selected");
        let title = title_of_clip(&editor, id);
        assert_eq!(Editor::title_label(&title), "Cog");
    }

    /// `$env:BARECLIP_SCREENS = '<folder>'; ./scripts/cargo.ps1 test -p bareclip-app --bin bareclip draw_the_library_screens '--' --ignored`
    #[test]
    #[ignore = "needs a real GPU, and draws pictures to look at rather than checking anything"]
    fn draw_the_library_screens() {
        use bareclip_core::art::{ArtKind, CATALOGUE, items_in};
        let folder = std::env::var_os("BARECLIP_SCREENS").map_or_else(
            || std::env::temp_dir().join("bareclip-screens"),
            PathBuf::from,
        );
        let ctx = context();
        let mut camera = Camera::new(folder);
        let big = make_clip("library-screen", "1280x720");
        let mut editor = with_videos(&ctx, &[&big]);
        editor.use_gpu(LayerDisplay::new(
            camera.device.clone(),
            camera.queue.clone(),
            camera.renderer.clone(),
        ));
        let settle_frames = |ctx: &egui::Context, editor: &mut Editor, count: usize| {
            for _ in 0..count {
                run_frame(ctx, editor, vec![]);
                thread::sleep(Duration::from_millis(10));
            }
        };
        settle_frames(&ctx, &mut editor, 30);
        click_named(&ctx, &mut editor, "Content library");
        settle_frames(&ctx, &mut editor, 12);
        camera.shoot(&ctx, &mut editor, vec![], "library-01-categories");

        let count = |kind: ArtKind| CATALOGUE.iter().filter(|item| item.kind == kind).count();
        click_named(
            &ctx,
            &mut editor,
            &group_name("Shapes", count(ArtKind::Shape)),
        );
        settle_frames(&ctx, &mut editor, 12);
        camera.shoot(&ctx, &mut editor, vec![], "library-02-shape-collections");

        let basic = items_in(ArtKind::Shape, "Basic").len();
        click_named(&ctx, &mut editor, &group_name("Basic", basic));
        settle_frames(&ctx, &mut editor, 12);
        camera.shoot(&ctx, &mut editor, vec![], "library-03-basic");

        click_named(&ctx, &mut editor, "Add Star");
        editor.seek(Ticks::from_seconds(1.0));
        settle_frames(&ctx, &mut editor, 30);
        camera.shoot(&ctx, &mut editor, vec![], "library-04-star-selected");

        let id = editor.selected.expect("the star is selected");
        editor.open_clip_panel(id, LookTab::Shape);
        settle_frames(&ctx, &mut editor, 40);
        camera.shoot(&ctx, &mut editor, vec![], "library-05-shape-panel");

        editor.open_clip_panel(id, LookTab::Animate);
        settle_frames(&ctx, &mut editor, 12);
        camera.shoot(&ctx, &mut editor, vec![], "library-06-animate-panel");

        click_named(&ctx, &mut editor, "More");
        settle_frames(&ctx, &mut editor, 12);
        camera.shoot(&ctx, &mut editor, vec![], "library-07-more-menu");
        click_named(&ctx, &mut editor, "Position");
        settle_frames(&ctx, &mut editor, 12);
        camera.shoot(&ctx, &mut editor, vec![], "library-07-position-menu");
        click_named(&ctx, &mut editor, "Order");
        settle_frames(&ctx, &mut editor, 12);
        camera.shoot(&ctx, &mut editor, vec![], "library-07-order-menu");
        run_frame(&ctx, &mut editor, key_event(Key::Escape, Modifiers::NONE));
        settle_frames(&ctx, &mut editor, 4);

        let square = items_in(ArtKind::Shape, "Outline")
            .into_iter()
            .find(|item| item.key == "outline-square")
            .expect("there is a square outline");
        editor.add_art(Art::of(square), Ticks::from_seconds(1.0), None, None);
        let outlined = editor.selected.expect("the square is selected");
        editor.apply_stage_events(
            stage::StageEvents {
                art: Some((outlined, stage::ArtChange::Outline(12))),
                ..Default::default()
            },
            false,
        );
        settle_frames(&ctx, &mut editor, 30);
        camera.shoot(&ctx, &mut editor, vec![], "library-07-square-outline");

        editor.add_art(Art::of(library_item("marker-circle")), Ticks(0), None, None);
        editor.add_art(Art::of(library_item("neon-glow")), Ticks(0), None, None);
        editor.add_art(Art::of(library_item("snow")), Ticks(0), None, None);
        editor.selected = None;
        editor.seek(Ticks::from_seconds(1.5));
        click_named(&ctx, &mut editor, "My media");
        settle_frames(&ctx, &mut editor, 40);
        camera.shoot(
            &ctx,
            &mut editor,
            vec![],
            "library-08-my-media-and-timeline",
        );

        click_named(&ctx, &mut editor, "Content library");
        click_named(&ctx, &mut editor, "All");
        editor.library_view.search = "frame".to_owned();
        settle_frames(&ctx, &mut editor, 12);
        camera.shoot(&ctx, &mut editor, vec![], "library-09-search");

        // A Rating title's Stars slider, which shows its number beside it the same way.
        with_a_title(&ctx, &mut editor, TitleStyle::Rating);
        settle_frames(&ctx, &mut editor, 40);
        camera.shoot(&ctx, &mut editor, vec![], "library-10-rating-stars");
    }

    /// Stands in for a microphone: a quiet tone, arriving in real time, until the bar lets go.
    fn made_up_microphone(_name: Option<&str>) -> Result<recording::MicSource, String> {
        struct StopFeeding(Arc<AtomicBool>);
        impl Drop for StopFeeding {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        const RATE: u32 = 44_100;
        let (mut producer, queue) = rtrb::RingBuffer::new(RATE as usize * 8);
        let level = Arc::new(bareclip_audio::Level::default());
        let stop = Arc::new(AtomicBool::new(false));
        let (feeding, heard) = (stop.clone(), level.clone());
        thread::spawn(move || {
            let mut frame = 0_u64;
            let started = Instant::now();
            while !feeding.load(Ordering::Acquire) {
                let due = (started.elapsed().as_secs_f64() * f64::from(RATE)) as u64;
                let mut block = Vec::new();
                while frame < due {
                    let sample = (frame as f32 * 0.06).sin() * 0.25;
                    block.extend([sample, sample]);
                    frame += 1;
                }
                heard.note(&block);
                for sample in block {
                    let _ = producer.push(sample);
                }
                thread::sleep(Duration::from_millis(5));
            }
        });
        Ok(recording::MicSource {
            queue: Some(queue),
            rate: RATE,
            level,
            _keep: Box::new(StopFeeding(stop)),
        })
    }

    thread_local! {
        /// Every microphone this test's thread opened, by name, `None` being Windows' default.
        static MICROPHONES_OPENED: std::cell::RefCell<Vec<Option<String>>> =
            const { std::cell::RefCell::new(Vec::new()) };
    }

    /// The made-up microphone, noting each time one is opened and which.
    fn counted_microphone(name: Option<&str>) -> Result<recording::MicSource, String> {
        MICROPHONES_OPENED.with(|opened| opened.borrow_mut().push(name.map(str::to_owned)));
        made_up_microphone(name)
    }

    fn microphones_opened() -> Vec<Option<String>> {
        MICROPHONES_OPENED.with(|opened| opened.borrow().clone())
    }

    /// The control with this name furthest right, which tells a recording window's control from
    /// the Record panel's card of the same name.
    fn click_rightmost_named(ctx: &egui::Context, editor: &mut Editor, name: &str) {
        let rect = named_nodes(ctx, editor)
            .into_iter()
            .filter(|(label, ..)| label == name)
            .map(|(_, rect, _)| rect)
            .max_by(|a, b| a.center().x.total_cmp(&b.center().x))
            .unwrap_or_else(|| panic!("nothing is named {name}"));
        click(ctx, editor, rect.center());
    }

    fn no_microphone(_name: Option<&str>) -> Result<recording::MicSource, String> {
        Err("No microphone was found.".to_owned())
    }

    fn made_up_microphones() -> Vec<bareclip_audio::Microphone> {
        ["Desk microphone", "Headset microphone"]
            .iter()
            .enumerate()
            .map(|(index, name)| bareclip_audio::Microphone {
                name: (*name).to_owned(),
                is_default: index == 0,
            })
            .collect()
    }

    /// An editor with a video whose recordings go to a folder of their own and come from a
    /// made-up microphone, with no countdown and the video kept still.
    fn ready_to_record(ctx: &egui::Context, name: &str) -> (Editor, PathBuf) {
        let mut editor = with_videos(ctx, &[test_clip()]);
        let folder = std::env::temp_dir()
            .join("bareclip-app-recordings")
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        editor.settings.recordings_folder = Some(folder.clone());
        editor.settings.countdown = false;
        editor.settings.play_while_recording = false;
        editor.open_microphone = made_up_microphone;
        editor.list_microphones = made_up_microphones;
        run_frame(ctx, &mut editor, vec![]);
        (editor, folder)
    }

    /// Runs frames for about this long, as the bar would be watched.
    fn run_for(ctx: &egui::Context, editor: &mut Editor, seconds: f64) {
        let started = Instant::now();
        while started.elapsed().as_secs_f64() < seconds {
            run_frame(ctx, editor, vec![]);
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn open_record_bar_by_hand(ctx: &egui::Context, editor: &mut Editor) {
        // Pressing Record again would close the panel.
        if !is_named(ctx, editor, "Microphone") {
            click_named(ctx, editor, "Record");
            run_frame(ctx, editor, vec![]);
        }
        click_named(ctx, editor, "Microphone");
        for _ in 0..3 {
            run_frame(ctx, editor, vec![]);
        }
    }

    fn recorded_files(folder: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(folder)
            .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn a_microphone_recording_lands_at_the_playhead_on_a_new_row_as_one_undo_step() {
        let ctx = context();
        let (mut editor, folder) = ready_to_record(&ctx, "lands");
        let rows_before = editor.project.tracks.tracks().len();
        let tracks_before = editor.project.tracks.clone();
        editor.seek(Ticks::from_seconds(1.0));
        run_frame(&ctx, &mut editor, vec![]);

        open_record_bar_by_hand(&ctx, &mut editor);
        assert!(editor.record_bar.is_some());
        assert!(is_named(&ctx, &mut editor, "Microphone: Desk microphone"));
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 1.5);
        assert!(is_named(&ctx, &mut editor, "Pause recording"));
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);

        assert!(editor.record_bar.is_none());
        let files = recorded_files(&folder);
        assert_eq!(files.len(), 1);
        let name = files[0].file_name().expect("named").to_string_lossy();
        assert!(
            name.starts_with("Microphone ") && name.ends_with(".m4a"),
            "{name}"
        );

        let entry = editor.media.last().expect("it is in My media");
        assert!(entry.info.is_sound_only());
        let id = editor.selected.expect("the new clip is selected");
        let clip = editor
            .project
            .tracks
            .clip(id)
            .expect("it is on the timeline");
        assert_eq!(clip.media, entry.id);
        assert_eq!(clip.start, Ticks::from_seconds(1.0));
        let row = editor.project.tracks.track_of(id).expect("on a row");
        assert!(editor.project.tracks.is_sound_row(row));
        assert_eq!(editor.project.tracks.tracks().len(), rows_before + 1);
        let seconds = clip.duration().as_seconds();
        assert!(seconds > 1.0 && seconds < 2.5, "{seconds}");

        editor.undo();
        assert_eq!(
            editor.project.tracks, tracks_before,
            "one step, and nothing moved"
        );
    }

    #[test]
    fn recording_with_the_video_playing_lands_where_the_video_was_when_it_began() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record(&ctx, "play-along");
        editor.settings.play_while_recording = true;
        editor.seek(Ticks::from_seconds(0.5));
        run_frame(&ctx, &mut editor, vec![]);
        open_record_bar_by_hand(&ctx, &mut editor);
        assert!(is_named(
            &ctx,
            &mut editor,
            "Play the video while recording"
        ));
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 1.2);

        assert!(editor.preview.is_playing());
        assert!(
            !is_named(&ctx, &mut editor, "Pause recording"),
            "pausing would lose step"
        );
        let lands = match editor.record_bar.as_ref().map(|bar| &bar.stage) {
            Some(recording::BarStage::Recording(live)) => live.lands_at(),
            _ => panic!("it is recording"),
        };
        assert!(
            lands >= Ticks::from_seconds(0.5) && lands < Ticks::from_seconds(1.0),
            "{lands:?}"
        );
        // The video under the playhead has moved on as long as the recording has run.
        let recorded = match editor.record_bar.as_ref().map(|bar| &bar.stage) {
            Some(recording::BarStage::Recording(live)) => live.seconds(),
            _ => 0.0,
        };
        let moved = (editor.position().0 - lands.0) as f64 / Ticks::from_seconds(1.0).0 as f64;
        assert!(
            (moved - recorded).abs() < 0.15,
            "{moved} against {recorded}"
        );

        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(!editor.preview.is_playing(), "stopping stops the video too");
        let id = editor.selected.expect("the recording is selected");
        assert_eq!(editor.project.tracks.clip(id).expect("placed").start, lands);
    }

    #[test]
    fn pausing_leaves_the_paused_stretch_out() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record(&ctx, "paused");
        open_record_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.8);
        click_named(&ctx, &mut editor, "Pause recording");
        run_for(&ctx, &mut editor, 1.5);
        assert!(is_named(&ctx, &mut editor, "Resume recording"));
        click_named(&ctx, &mut editor, "Resume recording");
        run_for(&ctx, &mut editor, 0.8);
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);

        let id = editor.selected.expect("the new clip is selected");
        let seconds = editor
            .project
            .tracks
            .clip(id)
            .expect("on the timeline")
            .duration()
            .as_seconds();
        assert!(seconds > 1.2 && seconds < 2.4, "{seconds}");
    }

    #[test]
    fn the_countdown_comes_before_the_recording() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record(&ctx, "countdown");
        editor.settings.countdown = true;
        open_record_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Start recording");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(matches!(
            editor.record_bar.as_ref().map(|bar| &bar.stage),
            Some(recording::BarStage::Counting(_))
        ));
        assert!(!is_named(&ctx, &mut editor, "Stop"));

        // Rather than wait three seconds, the countdown is let run out.
        if let Some(bar) = &mut editor.record_bar {
            bar.stage = recording::BarStage::Counting(0.0);
        }
        run_frame(&ctx, &mut editor, vec![]);
        run_frame(&ctx, &mut editor, vec![]);
        assert!(is_named(&ctx, &mut editor, "Stop"));
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.record_bar.is_none());
    }

    #[test]
    fn escape_closes_the_bar_only_before_recording() {
        let ctx = context();
        let (mut editor, folder) = ready_to_record(&ctx, "escape");
        open_record_bar_by_hand(&ctx, &mut editor);
        run_frame(&ctx, &mut editor, key_event(Key::Escape, Modifiers::NONE));
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.record_bar.is_none());
        assert!(recorded_files(&folder).is_empty(), "nothing was recorded");

        open_record_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.3);
        run_frame(&ctx, &mut editor, key_event(Key::Escape, Modifiers::NONE));
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.record_bar.is_some(), "a recording isn't thrown away");
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);
    }

    #[test]
    fn a_missing_microphone_says_so_and_cant_start() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record(&ctx, "missing");
        editor.open_microphone = no_microphone;
        open_record_bar_by_hand(&ctx, &mut editor);
        assert!(
            editor
                .record_bar
                .as_ref()
                .is_some_and(|bar| bar.problem().is_some())
        );
        let disabled = named_nodes(&ctx, &mut editor)
            .into_iter()
            .find(|(label, ..)| label == "Start recording")
            .map(|(.., disabled)| disabled)
            .expect("the button is there");
        assert!(disabled);
    }

    #[test]
    fn a_microphone_picked_in_the_bar_lasts_for_that_recording_only() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record(&ctx, "choose");
        editor.open_microphone = counted_microphone;
        open_record_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Microphone: Desk microphone");
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Headset microphone");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(is_named(
            &ctx,
            &mut editor,
            "Microphone: Headset microphone"
        ));
        assert_eq!(
            microphones_opened().last(),
            Some(&Some("Headset microphone".to_owned())),
            "it listens to the one picked"
        );
        assert_eq!(editor.settings.microphone, None, "the default stays");

        click_named(&ctx, &mut editor, "Cancel");
        run_frame(&ctx, &mut editor, vec![]);
        open_record_bar_by_hand(&ctx, &mut editor);
        assert!(is_named(&ctx, &mut editor, "Microphone: Desk microphone"));
    }

    #[test]
    fn the_default_microphone_in_settings_is_where_each_recording_starts() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record(&ctx, "default-microphone");
        click_named(&ctx, &mut editor, "Settings");
        let_dialog_open(&ctx, &mut editor);
        assert!(!is_named(
            &ctx,
            &mut editor,
            "Default microphone: Windows default"
        ));
        click_named(&ctx, &mut editor, "Recording settings");
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Default microphone: Windows default");
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Headset microphone");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(
            editor.settings.microphone.as_deref(),
            Some("Headset microphone")
        );
        assert!(is_named(
            &ctx,
            &mut editor,
            "Default microphone: Headset microphone"
        ));
        click_named(&ctx, &mut editor, "Done");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.settings_microphones.is_none(), "the list is let go");

        open_record_bar_by_hand(&ctx, &mut editor);
        assert!(is_named(
            &ctx,
            &mut editor,
            "Microphone: Headset microphone"
        ));
        click_named(&ctx, &mut editor, "Cancel");
        run_frame(&ctx, &mut editor, vec![]);
        editor.open_screen = made_up_screen;
        editor.open_pc_sound = made_up_pc_sound;
        editor.list_sources = made_up_sources;
        open_screen_bar_by_hand(&ctx, &mut editor);
        assert!(is_named(
            &ctx,
            &mut editor,
            "Microphone: Headset microphone"
        ));
    }

    #[test]
    fn a_default_microphone_that_is_unplugged_says_so_and_windows_default_is_used() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record(&ctx, "default-gone");
        editor.settings.microphone = Some("Old microphone".to_owned());
        editor.settings_tab = SettingsTab::Recording;
        editor.settings_open = true;
        let_dialog_open(&ctx, &mut editor);
        let output = run_frame(&ctx, &mut editor, vec![]);
        assert!(
            text_in(&output, egui::Rect::EVERYTHING)
                .iter()
                .any(|shown| shown == text::DEFAULT_MICROPHONE_GONE)
        );
        assert!(is_named(
            &ctx,
            &mut editor,
            "Default microphone: Old microphone"
        ));
        editor.settings_open = false;
        run_frame(&ctx, &mut editor, vec![]);

        editor.open_microphone = counted_microphone;
        open_record_bar_by_hand(&ctx, &mut editor);
        assert!(is_named(&ctx, &mut editor, "Microphone: Desk microphone"));
        assert_eq!(microphones_opened().last(), Some(&None));
    }

    #[test]
    fn settings_shows_one_tab_at_a_time_and_keeps_its_size() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record(&ctx, "settings-tabs");
        click_named(&ctx, &mut editor, "Settings");
        let_dialog_open(&ctx, &mut editor);
        let done = named_rect(&ctx, &mut editor, "Done");
        // Export's one choice lives on General, with no tab of its own.
        assert!(is_named(&ctx, &mut editor, "Waveforms on clips"));
        assert!(is_named(&ctx, &mut editor, "Fastest"));
        assert!(!is_named(&ctx, &mut editor, "Export settings"));
        assert!(!is_named(&ctx, &mut editor, "Count down from 3"));

        click_named(&ctx, &mut editor, "Recording settings");
        let_dialog_open(&ctx, &mut editor);
        assert!(is_named(&ctx, &mut editor, "Count down from 3"));
        assert!(is_named(
            &ctx,
            &mut editor,
            "Default microphone: Windows default"
        ));
        assert!(is_named(&ctx, &mut editor, "Recording quality: Balanced"));
        assert_eq!(
            named_rect(&ctx, &mut editor, "Done"),
            done,
            "it keeps its size"
        );

        click_named(&ctx, &mut editor, "Keybinds settings");
        let_dialog_open(&ctx, &mut editor);
        assert!(is_named(
            &ctx,
            &mut editor,
            "Stop recording: Ctrl+Alt+Shift+R"
        ));
        assert!(!is_named(&ctx, &mut editor, "Count down from 3"));
        assert_eq!(
            named_rect(&ctx, &mut editor, "Done"),
            done,
            "it keeps its size"
        );
    }

    #[test]
    fn closing_the_window_while_recording_keeps_the_recording_in_the_project() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record(&ctx, "closing");
        open_record_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.6);
        editor.handle_close_request(&ctx);
        assert!(editor.record_bar.is_none());
        let id = editor.selected.expect("the recording is selected");
        assert!(editor.project.tracks.clip(id).is_some());
        assert!(
            editor.leave_for.is_some(),
            "it asks about saving the project"
        );
    }

    #[test]
    fn a_recording_cut_short_is_offered_and_added() {
        let ctx = context();
        let (mut editor, folder) = ready_to_record(&ctx, "cut-short");
        // A real recording to offer, made the same way.
        open_record_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.6);
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);
        editor.undo();
        let file = recorded_files(&folder).pop().expect("a file");
        let marker = folder.join("marker.recording");
        std::fs::write(&marker, file.to_string_lossy().as_bytes()).expect("written");
        editor.cut_short = vec![storage::CutShort {
            marker: marker.clone(),
            file: file.clone(),
        }];
        run_frame(&ctx, &mut editor, vec![]);
        assert!(is_named(&ctx, &mut editor, "Add to project"));
        click_named(&ctx, &mut editor, "Add to project");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.cut_short.is_empty());
        assert!(!marker.exists());
        let id = editor.selected.expect("it was added");
        let clip = editor.project.tracks.clip(id).expect("on the timeline");
        assert_eq!(
            editor
                .entry(clip.media)
                .map(|entry| entry.info.path.clone()),
            Some(file)
        );
    }

    #[test]
    #[ignore = "draws pictures to look at rather than checking anything"]
    fn draw_the_record_screens() {
        let folder = std::env::var_os("BARECLIP_SCREENS").map_or_else(
            || std::env::temp_dir().join("bareclip-screens"),
            PathBuf::from,
        );
        let ctx = context();
        let mut camera = Camera::new(folder);
        let (mut editor, _) = ready_to_record(&ctx, "screens");
        editor.settings.play_while_recording = true;
        for _ in 0..30 {
            run_frame(&ctx, &mut editor, vec![]);
        }
        click_named(&ctx, &mut editor, "Record");
        for _ in 0..12 {
            run_frame(&ctx, &mut editor, vec![]);
        }
        camera.shoot(&ctx, &mut editor, vec![], "record-01-panel");
        open_record_bar_by_hand(&ctx, &mut editor);
        run_for(&ctx, &mut editor, 0.4);
        camera.shoot(&ctx, &mut editor, vec![], "record-02-bar");
        editor.settings.countdown = true;
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.3);
        camera.shoot(&ctx, &mut editor, vec![], "record-03-countdown");
        if let Some(bar) = &mut editor.record_bar {
            bar.stage = recording::BarStage::Counting(0.0);
        }
        run_for(&ctx, &mut editor, 1.2);
        camera.shoot(&ctx, &mut editor, vec![], "record-04-recording");
        click_named(&ctx, &mut editor, "Stop");
        for _ in 0..12 {
            run_frame(&ctx, &mut editor, vec![]);
        }
        camera.shoot(&ctx, &mut editor, vec![], "record-06-added");
        editor.settings_tab = SettingsTab::Recording;
        editor.settings_open = true;
        for _ in 0..12 {
            run_frame(&ctx, &mut editor, vec![]);
        }
        camera.shoot(&ctx, &mut editor, vec![], "record-07-settings");
        editor.settings_open = false;
        editor.settings_microphones = None;
        editor.settings_tab = SettingsTab::General;
        editor.open_screen = made_up_screen;
        editor.open_pc_sound = made_up_pc_sound;
        editor.open_app_sound = made_up_app_sound;
        editor.list_sources = made_up_sources;
        editor.settings.countdown = false;
        open_screen_bar_by_hand(&ctx, &mut editor);
        run_for(&ctx, &mut editor, 0.5);
        camera.shoot(&ctx, &mut editor, vec![], "record-08-screen-bar");
        choose_the_game_window(&ctx, &mut editor);
        run_for(&ctx, &mut editor, 0.3);
        camera.shoot(&ctx, &mut editor, vec![], "record-08b-window-app-sound");
        click_named(&ctx, &mut editor, "Size: Full size (640 x 360)");
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "record-08c-size-menu");
        editor.screen_bar = None;
        run_frame(&ctx, &mut editor, vec![]);
        open_screen_bar_by_hand(&ctx, &mut editor);
        run_for(&ctx, &mut editor, 0.3);
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.8);
        camera.shoot(&ctx, &mut editor, vec![], "record-09-screen-recording");
        click_named(&ctx, &mut editor, "Show only Pause and Stop");
        run_for(&ctx, &mut editor, 0.3);
        camera.shoot(&ctx, &mut editor, vec![], "record-10-screen-folded");
        click_named(&ctx, &mut editor, "Stop");
        run_for(&ctx, &mut editor, 0.3);
        editor.open_camera = made_up_camera;
        editor.list_cameras = made_up_cameras;
        editor.settings.play_while_recording = false;
        open_camera_bar_by_hand(&ctx, &mut editor);
        run_for(&ctx, &mut editor, 0.4);
        camera.shoot(&ctx, &mut editor, vec![], "record-11-camera-bar");
        editor.settings.countdown = true;
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.3);
        camera.shoot(&ctx, &mut editor, vec![], "record-12-camera-countdown");
        if let Some(bar) = &mut editor.camera_bar {
            bar.stage = camera_recording::CameraStage::Counting(0.0);
        }
        run_for(&ctx, &mut editor, 0.8);
        camera.shoot(&ctx, &mut editor, vec![], "record-13-camera-recording");
        click_named(&ctx, &mut editor, "Stop");
        run_for(&ctx, &mut editor, 0.3);
        editor.list_cameras = no_cameras;
        open_camera_bar_by_hand(&ctx, &mut editor);
        run_for(&ctx, &mut editor, 0.2);
        camera.shoot(&ctx, &mut editor, vec![], "record-14-no-camera");
        editor.camera_bar = None;
        editor.list_cameras = made_up_cameras;
        editor.settings.screen_microphone = true;
        open_screen_and_camera_bar_by_hand(&ctx, &mut editor);
        run_for(&ctx, &mut editor, 0.5);
        camera.shoot(&ctx, &mut editor, vec![], "record-15-screen-and-camera-bar");
        editor.settings.countdown = false;
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.4);
        click_named(&ctx, &mut editor, "Show the camera");
        run_for(&ctx, &mut editor, 0.4);
        camera.shoot(&ctx, &mut editor, vec![], "record-16-overlay-with-camera");
        click_named(&ctx, &mut editor, "Stop");
        run_for(&ctx, &mut editor, 0.5);
        for _ in 0..12 {
            run_frame(&ctx, &mut editor, vec![]);
        }
        camera.shoot(&ctx, &mut editor, vec![], "record-17-three-clips");

        // Long names go onto a second line in their menus rather than out of them.
        editor.list_sources = long_named_sources;
        editor.list_microphones = long_named_microphones;
        editor.screen_bar = None;
        editor.camera_bar = None;
        run_frame(&ctx, &mut editor, vec![]);
        open_screen_bar_by_hand(&ctx, &mut editor);
        run_for(&ctx, &mut editor, 0.3);
        click_named(&ctx, &mut editor, "Record: Screen 1 (main), 640 x 360");
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "record-18-long-window-names");
        let microphone = format!("Microphone: {}", long_named_microphones()[0].name);
        click_named(&ctx, &mut editor, &microphone);
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "record-19-long-microphone-names");
        editor.screen_bar = None;
        run_frame(&ctx, &mut editor, vec![]);

        // Each tab of Settings, with a chip whose name and note are long.
        laptop_chips(&mut editor);
        editor.settings.gpu = Some(GpuChoice {
            vendor: 0x1002,
            device: 0x1638,
        });
        editor.settings_tab = SettingsTab::General;
        editor.settings_open = true;
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "record-20-settings-general");
        click_named(&ctx, &mut editor, "Recording settings");
        let_dialog_open(&ctx, &mut editor);
        let default = format!("Default microphone: {}", text::WINDOWS_DEFAULT_MICROPHONE);
        click_named(&ctx, &mut editor, &default);
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "record-22-settings-microphones");
        run_frame(&ctx, &mut editor, press(Key::Escape));
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "record-23-settings-recording");
        click_named(&ctx, &mut editor, "Keybinds settings");
        let_dialog_open(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Stop recording: Ctrl+Alt+Shift+R");
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(&ctx, &mut editor, vec![], "record-24-keybinds-waiting");
        run_frame(&ctx, &mut editor, press(Key::K));
        let_dialog_open(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Pause or resume: Ctrl+Alt+Shift+P");
        run_frame(&ctx, &mut editor, vec![]);
        run_frame(&ctx, &mut editor, press(Key::K));
        let_dialog_open(&ctx, &mut editor);
        camera.shoot(
            &ctx,
            &mut editor,
            vec![],
            "record-25-keybinds-plain-and-refused",
        );
        editor.settings.stop_key = hotkeys::Shortcut::STOP;
        editor.key_problem = None;
        editor.settings_open = false;
        editor.settings_microphones = None;
        run_frame(&ctx, &mut editor, vec![]);
        // Pictures still arriving from the video leave texture changes nobody will draw.
        KEPT_TEXTURES.with_borrow_mut(|kept| {
            for delta in kept.iter_mut().flatten() {
                delta.clear();
            }
            *kept = None;
        });
    }

    /// Stands in for the screen: a picture that changes colour every so often.
    struct MadeUpScreen {
        started: Instant,
        size: (u32, u32),
    }

    impl bareclip_capture::PictureSource for MadeUpScreen {
        fn latest(&self) -> Option<(bareclip_capture::Picture, u64)> {
            let tick = (self.started.elapsed().as_millis() / 250) as u64;
            let shade = (tick * 40 % 200) as u8;
            let (width, height) = self.size;
            Some((
                bareclip_capture::Picture {
                    width,
                    height,
                    bgra: [shade, 80, 200, 255].repeat((width * height) as usize),
                    shown_at: 0,
                },
                tick + 1,
            ))
        }

        fn size(&self) -> (u32, u32) {
            self.size
        }
    }

    fn made_up_screen(
        _source: bareclip_capture::Source,
        _pointer: bool,
    ) -> Result<Box<dyn bareclip_capture::PictureSource>, String> {
        Ok(Box::new(MadeUpScreen {
            started: Instant::now(),
            size: (640, 360),
        }))
    }

    /// A screen twice as large, which smaller sizes can be picked for.
    fn made_up_large_screen(
        _source: bareclip_capture::Source,
        _pointer: bool,
    ) -> Result<Box<dyn bareclip_capture::PictureSource>, String> {
        Ok(Box::new(MadeUpScreen {
            started: Instant::now(),
            size: (1280, 720),
        }))
    }

    fn made_up_pc_sound() -> Result<recording::MicSource, String> {
        made_up_microphone(None)
    }

    /// Stands in for one program's sound, as `made_up_pc_sound` does for all of it.
    fn made_up_app_sound(
        _source: bareclip_capture::Source,
    ) -> Result<recording::MicSource, String> {
        made_up_microphone(None)
    }

    /// One program's sound, on a PC where that can't be done.
    fn no_app_sound(_source: bareclip_capture::Source) -> Result<recording::MicSource, String> {
        Err("Only this app's sound can't be recorded on this PC.".to_owned())
    }

    fn made_up_sources() -> Vec<bareclip_capture::Offered> {
        vec![
            bareclip_capture::Offered {
                source: bareclip_capture::Source::Screen(1),
                name: "Screen 1 (main), 640 x 360".to_owned(),
                size: (640, 360),
                is_main: true,
            },
            bareclip_capture::Offered {
                source: bareclip_capture::Source::Window(2),
                name: "A game".to_owned(),
                size: (640, 360),
                is_main: false,
            },
        ]
    }

    /// Screens and windows whose names are too long for one line of a menu.
    fn long_named_sources() -> Vec<bareclip_capture::Offered> {
        let mut offered = made_up_sources();
        offered.push(bareclip_capture::Offered {
            source: bareclip_capture::Source::Window(3),
            name: "Ranking the fifty best players in the league this season, purely on form \
                   this time : r/soccer - Zen Browser"
                .to_owned(),
            size: (640, 360),
            is_main: false,
        });
        offered
    }

    /// Microphones whose names are too long for one line of a menu.
    fn long_named_microphones() -> Vec<bareclip_audio::Microphone> {
        [
            "Headset Microphone (HyperX Virtual Surround Sound)",
            "Microphone Array (Realtek(R) Audio) with a much longer name than most",
        ]
        .iter()
        .enumerate()
        .map(|(index, name)| bareclip_audio::Microphone {
            name: (*name).to_owned(),
            is_default: index == 0,
        })
        .collect()
    }

    /// An editor ready to record a made-up screen with made-up PC sound and no microphone.
    fn ready_to_record_the_screen(ctx: &egui::Context, name: &str) -> (Editor, PathBuf) {
        let (mut editor, folder) = ready_to_record(ctx, name);
        editor.open_screen = made_up_screen;
        editor.open_pc_sound = made_up_pc_sound;
        editor.open_app_sound = made_up_app_sound;
        editor.list_sources = made_up_sources;
        editor.settings.screen_pc_sound = true;
        editor.settings.screen_microphone = false;
        (editor, folder)
    }

    fn open_screen_bar_by_hand(ctx: &egui::Context, editor: &mut Editor) {
        if !is_named(ctx, editor, "Screen") {
            click_named(ctx, editor, "Record");
            run_frame(ctx, editor, vec![]);
        }
        click_named(ctx, editor, "Screen");
        for _ in 0..3 {
            run_frame(ctx, editor, vec![]);
        }
    }

    #[test]
    fn a_screen_recording_lands_as_a_video_at_the_playhead_on_a_new_top_row() {
        let ctx = context();
        let (mut editor, folder) = ready_to_record_the_screen(&ctx, "screen-lands");
        let tracks_before = editor.project.tracks.clone();
        editor.seek(Ticks::from_seconds(1.0));
        run_frame(&ctx, &mut editor, vec![]);
        open_screen_bar_by_hand(&ctx, &mut editor);
        assert!(editor.screen_bar.is_some());
        assert!(is_named(
            &ctx,
            &mut editor,
            "Record: Screen 1 (main), 640 x 360"
        ));
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 1.5);
        assert!(is_named(&ctx, &mut editor, "Pause recording"));
        assert!(
            is_named(&ctx, &mut editor, "Turn the microphone on"),
            "the microphone was switched off, so it starts silenced"
        );
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);

        assert!(editor.screen_bar.is_none());
        let files = recorded_files(&folder);
        assert_eq!(files.len(), 1);
        let name = files[0].file_name().expect("named").to_string_lossy();
        assert!(
            name.starts_with("Screen ") && name.ends_with(".mp4"),
            "{name}"
        );
        let info = bareclip_media::probe(&files[0]).expect("it can be read");
        assert_eq!((info.video.width, info.video.height), (640, 360));
        let titles: Vec<_> = info
            .audio
            .iter()
            .map(|track| track.title.as_deref())
            .collect();
        assert_eq!(titles, [Some("PC sound"), Some("Microphone")]);

        let id = editor.selected.expect("the recording is selected");
        let clip = editor.project.tracks.clip(id).expect("on the timeline");
        assert_eq!(clip.start, Ticks::from_seconds(1.0));
        assert_eq!(
            editor.project.tracks.track_of(id),
            Some(0),
            "a new row on top"
        );
        let seconds = clip.duration().as_seconds();
        assert!(seconds > 1.0 && seconds < 2.3, "{seconds}");
        editor.undo();
        assert_eq!(
            editor.project.tracks, tracks_before,
            "one step, and nothing moved"
        );
    }

    fn choose_the_game_window(ctx: &egui::Context, editor: &mut Editor) {
        click_named(ctx, editor, "Record: Screen 1 (main), 640 x 360");
        run_frame(ctx, editor, vec![]);
        click_named(ctx, editor, "A game");
        run_frame(ctx, editor, vec![]);
    }

    /// Records the screen bar that's open for a moment, and gives the new file.
    fn record_a_moment(ctx: &egui::Context, editor: &mut Editor, folder: &Path) -> PathBuf {
        let before = recorded_files(folder);
        click_named(ctx, editor, "Start recording");
        run_for(ctx, editor, 0.6);
        click_named(ctx, editor, "Stop");
        run_frame(ctx, editor, vec![]);
        recorded_files(folder)
            .into_iter()
            .find(|file| !before.contains(file))
            .expect("a new file")
    }

    #[test]
    fn recording_a_window_takes_only_its_apps_sound_unless_switched_off() {
        let ctx = context();
        let (mut editor, folder) = ready_to_record_the_screen(&ctx, "screen-app-sound");
        open_screen_bar_by_hand(&ctx, &mut editor);
        assert!(
            !is_named(&ctx, &mut editor, text::ONLY_THIS_APPS_SOUND),
            "a whole screen records all PC sound"
        );
        choose_the_game_window(&ctx, &mut editor);
        assert!(is_named(&ctx, &mut editor, text::ONLY_THIS_APPS_SOUND));
        let file = record_a_moment(&ctx, &mut editor, &folder);
        assert_eq!(
            sound_track_names(&file),
            [Some("App sound".to_owned()), Some("Microphone".to_owned())]
        );

        // Switched off, the same window records everything the PC plays, and that's remembered.
        open_screen_bar_by_hand(&ctx, &mut editor);
        choose_the_game_window(&ctx, &mut editor);
        click_named(&ctx, &mut editor, text::ONLY_THIS_APPS_SOUND);
        run_frame(&ctx, &mut editor, vec![]);
        assert!(!editor.settings.screen_app_sound_only);
        let file = record_a_moment(&ctx, &mut editor, &folder);
        assert_eq!(
            sound_track_names(&file),
            [Some("PC sound".to_owned()), Some("Microphone".to_owned())]
        );
    }

    #[test]
    fn without_one_apps_sound_all_pc_sound_is_recorded_and_it_says_so() {
        let ctx = context();
        let (mut editor, folder) = ready_to_record_the_screen(&ctx, "screen-no-app-sound");
        editor.open_app_sound = no_app_sound;
        open_screen_bar_by_hand(&ctx, &mut editor);
        choose_the_game_window(&ctx, &mut editor);
        let said = format!(
            "Only this app's sound can't be recorded on this PC. {}",
            text::ALL_PC_SOUND_INSTEAD
        );
        assert!(shows_text(&ctx, &mut editor, &said));
        let file = record_a_moment(&ctx, &mut editor, &folder);
        assert_eq!(
            sound_track_names(&file),
            [Some("PC sound".to_owned()), Some("Microphone".to_owned())]
        );
    }

    #[test]
    fn a_smaller_size_picked_for_one_recording_shrinks_it_and_leaves_settings_alone() {
        let ctx = context();
        let (mut editor, folder) = ready_to_record_the_screen(&ctx, "screen-smaller");
        editor.open_screen = made_up_large_screen;
        open_screen_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Size: Full size (1280 x 720)");
        run_frame(&ctx, &mut editor, vec![]);
        // Only sizes smaller than the screen are offered.
        assert!(!is_named(&ctx, &mut editor, "1080p (1280 x 720)"));
        assert!(!is_named(&ctx, &mut editor, "720p (1280 x 720)"));
        click_named(&ctx, &mut editor, "480p (854 x 480)");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(is_named(&ctx, &mut editor, "Size: 480p (854 x 480)"));
        assert_eq!(
            editor.settings.screen_size,
            storage::RecordingSize::Full,
            "Settings keep the default"
        );
        let file = record_a_moment(&ctx, &mut editor, &folder);
        let info = bareclip_media::probe(&file).expect("it can be read");
        assert_eq!((info.video.width, info.video.height), (854, 480));

        // The next recording starts on Settings' default again.
        open_screen_bar_by_hand(&ctx, &mut editor);
        assert!(is_named(&ctx, &mut editor, "Size: Full size (1280 x 720)"));
    }

    #[test]
    fn the_size_in_settings_is_where_each_window_starts_unless_its_too_large() {
        let ctx = context();
        let (mut editor, folder) = ready_to_record_the_screen(&ctx, "screen-size-default");
        editor.settings.screen_size = storage::RecordingSize::P480;
        editor.open_screen = made_up_large_screen;
        open_screen_bar_by_hand(&ctx, &mut editor);
        assert!(is_named(&ctx, &mut editor, "Size: 480p (854 x 480)"));
        editor.screen_bar = None;
        run_frame(&ctx, &mut editor, vec![]);

        // 480p would make a 640 x 360 screen larger, so it's recorded at its own size.
        editor.open_screen = made_up_screen;
        open_screen_bar_by_hand(&ctx, &mut editor);
        assert!(is_named(&ctx, &mut editor, "Size: Full size (640 x 360)"));
        let file = record_a_moment(&ctx, &mut editor, &folder);
        let info = bareclip_media::probe(&file).expect("it can be read");
        assert_eq!((info.video.width, info.video.height), (640, 360));
    }

    #[test]
    fn the_quality_and_size_picked_are_what_the_screen_is_recorded_with() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record_the_screen(&ctx, "screen-quality");
        editor.open_screen = made_up_large_screen;
        open_screen_bar_by_hand(&ctx, &mut editor);
        assert!(is_named(&ctx, &mut editor, "Quality: Balanced"));
        click_named(&ctx, &mut editor, "Quality: Balanced");
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Smaller file");
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Size: Full size (1280 x 720)");
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "480p (854 x 480)");
        run_frame(&ctx, &mut editor, vec![]);
        let bar = editor.screen_bar.as_ref().expect("the bar is open");
        assert_eq!(
            bar.screen_settings(60),
            bareclip_capture::ScreenSettings {
                frame_rate: 60,
                size: Some((854, 480)),
                quality: Quality::Smaller,
            }
        );
        assert_eq!(
            editor.settings.recording_quality,
            storage::RecordingQuality::Balanced,
            "Settings keep the default"
        );
    }

    #[test]
    fn the_quality_in_settings_is_where_screen_and_camera_windows_start() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record_the_screen(&ctx, "quality-default");
        editor.open_camera = made_up_camera;
        editor.list_cameras = made_up_cameras;
        editor.settings.recording_quality = storage::RecordingQuality::MatchSource;
        open_screen_bar_by_hand(&ctx, &mut editor);
        assert!(is_named(&ctx, &mut editor, "Quality: Match source"));
        editor.screen_bar = None;
        run_frame(&ctx, &mut editor, vec![]);
        open_camera_bar_by_hand(&ctx, &mut editor);
        run_for(&ctx, &mut editor, 0.3);
        assert!(is_named(&ctx, &mut editor, "Quality: Match source"));
        click_named(&ctx, &mut editor, "Quality: Match source");
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Smaller file");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(is_named(&ctx, &mut editor, "Quality: Smaller file"));
        assert_eq!(
            editor.settings.recording_quality,
            storage::RecordingQuality::MatchSource
        );
    }

    /// Opens Settings on its Keybinds tab.
    fn open_keybind_settings(ctx: &egui::Context, editor: &mut Editor) {
        editor.settings_tab = SettingsTab::Keybinds;
        editor.settings_open = true;
        let_dialog_open(ctx, editor);
    }

    #[test]
    fn a_recording_key_is_changed_by_pressing_the_new_keys_in_settings() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record_the_screen(&ctx, "keys-changed");
        open_keybind_settings(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Stop recording: Ctrl+Alt+Shift+R");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(is_named(
            &ctx,
            &mut editor,
            &format!("Stop recording: {}", text::PRESS_KEYS)
        ));
        run_frame(
            &ctx,
            &mut editor,
            key_event(Key::F9, Modifiers::CTRL | Modifiers::SHIFT),
        );
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.settings.stop_key.name(), "Ctrl+Shift+F9");
        assert!(is_named(&ctx, &mut editor, "Stop recording: Ctrl+Shift+F9"));
        assert!(editor.settings_open);
        editor.settings_open = false;
        run_frame(&ctx, &mut editor, vec![]);

        // Hovering Start recording in the screen window says which keys.
        open_screen_bar_by_hand(&ctx, &mut editor);
        let keys = "From any program, Ctrl+Shift+F9 stops, Ctrl+Alt+Shift+P pauses and \
                    Ctrl+Alt+Shift+M turns the microphone on or off.";
        assert!(!shows_text(&ctx, &mut editor, keys), "only when hovered");
        // Tests don't wait for the pointer to rest, so the hint shows straight away.
        ctx.all_styles_mut(|style| {
            style.interaction.tooltip_delay = 0.0;
            style.interaction.show_tooltips_only_when_still = false;
        });
        let start = named_rect(&ctx, &mut editor, "Start recording");
        run_frame(
            &ctx,
            &mut editor,
            vec![egui::Event::PointerMoved(start.center())],
        );
        run_frame(&ctx, &mut editor, vec![]);
        assert!(shows_text(&ctx, &mut editor, keys));
    }

    #[test]
    fn the_recording_keys_pause_turn_the_microphone_on_and_off_and_stop() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record_the_screen(&ctx, "keys-pressed");
        open_screen_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.3);
        assert!(is_named(&ctx, &mut editor, "Turn the microphone on"));

        let press = |editor: &mut Editor, key| editor.recording_key_pressed(&ctx, key);
        assert!(!press(&mut editor, hotkeys::RecordingKey::Microphone));
        run_frame(&ctx, &mut editor, vec![]);
        assert!(is_named(&ctx, &mut editor, "Turn the microphone off"));
        assert!(!press(&mut editor, hotkeys::RecordingKey::Microphone));
        run_frame(&ctx, &mut editor, vec![]);
        assert!(is_named(&ctx, &mut editor, "Turn the microphone on"));

        assert!(!press(&mut editor, hotkeys::RecordingKey::Pause));
        run_frame(&ctx, &mut editor, vec![]);
        assert!(is_named(&ctx, &mut editor, "Resume recording"));
        assert!(press(&mut editor, hotkeys::RecordingKey::Stop));
        assert!(editor.screen_bar.is_none());
    }

    #[test]
    fn the_microphone_key_can_be_changed_like_the_others() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record(&ctx, "keys-microphone");
        open_keybind_settings(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Microphone on or off: Ctrl+Alt+Shift+M");
        run_frame(&ctx, &mut editor, vec![]);
        run_frame(&ctx, &mut editor, key_event(Key::F8, Modifiers::ALT));
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.settings.microphone_key.name(), "Alt+F8");
        // And can't take another's keys.
        click_named(&ctx, &mut editor, "Microphone on or off: Alt+F8");
        run_frame(&ctx, &mut editor, vec![]);
        run_frame(
            &ctx,
            &mut editor,
            key_event(Key::P, Modifiers::CTRL | Modifiers::ALT | Modifiers::SHIFT),
        );
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.settings.microphone_key.name(), "Alt+F8");
    }

    #[test]
    fn escape_keeps_the_recording_key_and_settings_open() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record(&ctx, "keys-escape");
        open_keybind_settings(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Pause or resume: Ctrl+Alt+Shift+P");
        run_frame(&ctx, &mut editor, vec![]);
        run_frame(&ctx, &mut editor, press(Key::Escape));
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.settings_open, "Escape backed out of the key only");
        assert_eq!(editor.settings.pause_key, hotkeys::Shortcut::PAUSE);
        assert!(is_named(
            &ctx,
            &mut editor,
            "Pause or resume: Ctrl+Alt+Shift+P"
        ));
    }

    #[test]
    fn the_other_recording_key_cant_be_used_twice() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record(&ctx, "keys-twice");
        open_keybind_settings(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Pause or resume: Ctrl+Alt+Shift+P");
        run_frame(&ctx, &mut editor, vec![]);
        run_frame(
            &ctx,
            &mut editor,
            key_event(Key::R, Modifiers::CTRL | Modifiers::ALT | Modifiers::SHIFT),
        );
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.settings.pause_key, hotkeys::Shortcut::PAUSE);
        assert!(shows_text(
            &ctx,
            &mut editor,
            "Ctrl+Alt+Shift+R is already another recording key."
        ));
    }

    #[test]
    fn a_plain_key_is_kept_with_a_warning() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record(&ctx, "keys-plain");
        open_keybind_settings(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Stop recording: Ctrl+Alt+Shift+R");
        run_frame(&ctx, &mut editor, vec![]);
        run_frame(&ctx, &mut editor, press(Key::Backtick));
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.settings.stop_key.name(), "`");
        let warning = text::plain_key_warning(editor.settings.stop_key);
        assert!(shows_text(&ctx, &mut editor, &warning));
        // A function key alone isn't warned about.
        click_named(&ctx, &mut editor, "Stop recording: `");
        run_frame(&ctx, &mut editor, vec![]);
        run_frame(&ctx, &mut editor, press(Key::F10));
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.settings.stop_key.name(), "F10");
        assert!(!shows_text(&ctx, &mut editor, &warning));
        let warning = text::plain_key_warning(editor.settings.stop_key);
        assert!(!shows_text(&ctx, &mut editor, &warning));
    }

    /// Where a recording window sits on screen, by the name of its area.
    fn bar_rect(ctx: &egui::Context, area: &str) -> egui::Rect {
        ctx.memory(|memory| memory.area_rect(egui::Id::unique(area)))
            .expect("it's shown")
    }

    #[test]
    fn recording_windows_open_in_the_middle_and_record_over_the_bottom_of_the_video() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record_the_screen(&ctx, "bars-centred");
        editor.open_camera = made_up_camera;
        editor.list_cameras = made_up_cameras;
        let middle = ctx.content_rect().center();
        let centred = |rect: egui::Rect| (rect.center() - middle).length() < 1.5;

        open_record_bar_by_hand(&ctx, &mut editor);
        run_for(&ctx, &mut editor, 0.2);
        let open = bar_rect(&ctx, "record_bar");
        assert!(centred(open), "{open:?} against {middle:?}");
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.4);
        let slim = bar_rect(&ctx, "record_bar");
        assert!(
            (slim.bottom() - (editor.stage_rect.bottom() - 16.0)).abs() < 1.5,
            "{slim:?} over the bottom of {:?}",
            editor.stage_rect
        );
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);

        open_screen_bar_by_hand(&ctx, &mut editor);
        run_for(&ctx, &mut editor, 0.2);
        let screen = bar_rect(&ctx, "screen_bar");
        assert!(centred(screen), "{screen:?}");
        editor.screen_bar = None;
        run_frame(&ctx, &mut editor, vec![]);

        open_camera_bar_by_hand(&ctx, &mut editor);
        run_for(&ctx, &mut editor, 0.3);
        let camera = bar_rect(&ctx, "camera_bar");
        assert!(centred(camera), "{camera:?}");
        editor.camera_bar = None;
        run_frame(&ctx, &mut editor, vec![]);

        open_screen_and_camera_bar_by_hand(&ctx, &mut editor);
        run_for(&ctx, &mut editor, 0.3);
        let both = bar_rect(&ctx, "screen_bar");
        assert!(centred(both), "{both:?}");
        assert!(both.top() >= 0.0 && both.bottom() <= ctx.content_rect().bottom());
    }

    #[test]
    fn a_window_can_be_chosen_instead_of_the_screen() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record_the_screen(&ctx, "screen-window");
        open_screen_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Record: Screen 1 (main), 640 x 360");
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "A game");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(is_named(&ctx, &mut editor, "Record: A game"));
    }

    #[test]
    fn pausing_a_screen_recording_from_its_overlay_leaves_the_stretch_out() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record_the_screen(&ctx, "screen-paused");
        open_screen_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.8);
        click_named(&ctx, &mut editor, "Pause recording");
        run_for(&ctx, &mut editor, 1.2);
        assert!(is_named(&ctx, &mut editor, "Resume recording"));
        click_named(&ctx, &mut editor, "Resume recording");
        run_for(&ctx, &mut editor, 0.8);
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);
        let id = editor.selected.expect("the recording is selected");
        let seconds = editor
            .project
            .tracks
            .clip(id)
            .expect("on the timeline")
            .duration()
            .as_seconds();
        assert!(seconds > 1.2 && seconds < 2.4, "{seconds}");
    }

    #[test]
    fn closing_the_window_while_recording_the_screen_keeps_it() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record_the_screen(&ctx, "screen-closing");
        open_screen_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.6);
        editor.handle_close_request(&ctx);
        assert!(editor.screen_bar.is_none());
        let id = editor.selected.expect("the recording is selected");
        assert!(editor.project.tracks.clip(id).is_some());
    }

    /// Presses a control by name, giving the moments just before and just after the press.
    fn timed_click(ctx: &egui::Context, editor: &mut Editor, name: &str) -> (Instant, Instant) {
        let before = Instant::now();
        click_named(ctx, editor, name);
        (before, Instant::now())
    }

    /// How loud a recording is between two moments, from 0 to 1 against its loudest.
    fn loudness_between(path: &Path, from: f64, to: f64) -> f32 {
        let shape = bareclip_media::read_sound_shape(path, &AtomicBool::new(false), |_| {})
            .expect("the sound can be read")
            .expect("not cancelled");
        shape.peak_between(Ticks::from_seconds(from), Ticks::from_seconds(to))
    }

    #[test]
    fn turning_the_microphone_on_midway_is_heard_from_then_on() {
        let ctx = context();
        let (mut editor, folder) = ready_to_record_the_screen(&ctx, "screen-mic-midway");
        editor.settings.screen_pc_sound = false;
        open_screen_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Start recording");
        let started = Instant::now();
        run_for(&ctx, &mut editor, 1.0);
        let on_pressed = Instant::now();
        click_named(&ctx, &mut editor, "Turn the microphone on");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(is_named(&ctx, &mut editor, "Turn the microphone off"));
        run_for(&ctx, &mut editor, 1.0);
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);

        let file = recorded_files(&folder).pop().expect("a file");
        let info = bareclip_media::probe(&file).expect("it can be read");
        assert_eq!(info.audio.len(), 1);
        // A press can take seconds on a busy machine, so the microphone is only known to be off
        // until the press began, and heard somewhere between then and the end.
        let turned_on_after = (on_pressed - started).as_secs_f64();
        let silent = loudness_between(&file, 0.1, turned_on_after - 0.2);
        let heard = loudness_between(&file, turned_on_after + 0.2, seconds_of(&file));

        assert!(silent < 0.05, "{silent} before it was turned on");
        assert!(heard > 0.5, "{heard} after it was turned on");
    }

    #[test]
    fn turning_the_microphone_off_midway_silences_it() {
        let ctx = context();
        let (mut editor, folder) = ready_to_record_the_screen(&ctx, "screen-mic-off");
        editor.settings.screen_pc_sound = false;
        editor.settings.screen_microphone = true;
        open_screen_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 1.0);
        click_named(&ctx, &mut editor, "Turn the microphone off");
        run_for(&ctx, &mut editor, 1.0);
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);

        let file = recorded_files(&folder).pop().expect("a file");
        let heard = loudness_between(&file, 0.2, 0.7);
        let silent = loudness_between(&file, 1.4, 1.8);
        assert!(heard > 0.5, "{heard} while on");
        assert!(silent < 0.05, "{silent} once turned off");
    }

    #[test]
    fn without_a_microphone_its_button_is_greyed_out() {
        let ctx = context();
        let (mut editor, folder) = ready_to_record_the_screen(&ctx, "screen-no-mic");
        editor.open_microphone = no_microphone;
        open_screen_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.5);
        let disabled = named_nodes(&ctx, &mut editor)
            .into_iter()
            .filter(|(label, ..)| label == "No microphone")
            .map(|(.., disabled)| disabled)
            .collect::<Vec<_>>();
        assert!(!disabled.is_empty() && disabled.iter().all(|disabled| *disabled));
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);
        let file = recorded_files(&folder).pop().expect("a file");
        let info = bareclip_media::probe(&file).expect("it can be read");
        assert_eq!(info.audio.len(), 1, "only the PC's sound");
    }

    /// How many controls have this name, in the bar and the overlay together.
    fn count_named(ctx: &egui::Context, editor: &mut Editor, name: &str) -> usize {
        named_nodes(ctx, editor)
            .iter()
            .filter(|(label, ..)| label == name)
            .count()
    }

    #[test]
    fn the_overlay_folds_down_to_pause_and_stop_and_back() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record_the_screen(&ctx, "screen-fold");
        open_screen_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.4);
        // The bar in BareClip's window and the overlay, both with every control.
        assert_eq!(count_named(&ctx, &mut editor, "Turn the microphone on"), 2);
        assert_eq!(count_named(&ctx, &mut editor, "Pause recording"), 2);
        assert!(is_named(&ctx, &mut editor, "Move the recording bar"));

        click_named(&ctx, &mut editor, "Show only Pause and Stop");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(count_named(&ctx, &mut editor, "Turn the microphone on"), 1);
        assert_eq!(count_named(&ctx, &mut editor, "Pause recording"), 2);
        assert_eq!(count_named(&ctx, &mut editor, "Stop"), 2);
        assert!(is_named(&ctx, &mut editor, "Move the recording bar"));
        // Folded, it still pauses.
        let pauses: Vec<_> = named_nodes(&ctx, &mut editor)
            .into_iter()
            .filter(|(label, ..)| label == "Pause recording")
            .map(|(_, rect, _)| rect)
            .collect();
        let grip = named_rect(&ctx, &mut editor, "Move the recording bar");
        let overlay_pause = pauses
            .into_iter()
            .min_by(|a, b| {
                (a.center().distance(grip.center())).total_cmp(&b.center().distance(grip.center()))
            })
            .expect("a pause button");
        click(&ctx, &mut editor, overlay_pause.center());
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(count_named(&ctx, &mut editor, "Resume recording"), 2);

        click_named(&ctx, &mut editor, "Show all controls");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(count_named(&ctx, &mut editor, "Turn the microphone on"), 2);
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);

        // The next recording starts unfolded again.
        open_screen_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.3);
        assert!(is_named(&ctx, &mut editor, "Show only Pause and Stop"));
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);
    }

    #[test]
    fn the_overlay_is_exactly_as_wide_as_its_controls() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record_the_screen(&ctx, "screen-overlay-size");
        open_screen_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.3);
        let padding = 2.0 * f32::from(screen_recording::OVERLAY_PADDING);
        for (fold, folded) in [
            ("Show only Pause and Stop", false),
            ("Show all controls", true),
        ] {
            let grip = named_rect(&ctx, &mut editor, "Move the recording bar");
            let end = named_rect(&ctx, &mut editor, fold);
            let wanted = screen_recording::overlay_size(folded, None);
            let width = end.right() - grip.left() + padding;
            assert!((width - wanted.x).abs() < 0.5, "{width} against {wanted:?}");
            assert!(grip.height() + padding <= wanted.y + 0.5);
            click_named(&ctx, &mut editor, fold);
            run_frame(&ctx, &mut editor, vec![]);
        }
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);
    }

    /// Stands in for the folder picker, always picking the same folder.
    fn picks_another_folder(_from: &Path) -> Option<PathBuf> {
        Some(
            std::env::temp_dir()
                .join("bareclip-app-recordings")
                .join(format!("browsed-{}", std::process::id())),
        )
    }

    #[test]
    fn browse_saves_one_screen_recording_elsewhere_and_leaves_the_default() {
        let ctx = context();
        let (mut editor, folder) = ready_to_record_the_screen(&ctx, "screen-browse");
        fn picks_screen_folder(from: &Path) -> Option<PathBuf> {
            picks_another_folder(from).map(|folder| folder.join("screen"))
        }
        let browsed = picks_screen_folder(Path::new("")).expect("a folder");
        let _ = std::fs::remove_dir_all(&browsed);
        editor.pick_folder = picks_screen_folder;
        open_screen_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, &components::FolderBox::name(&folder));
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.5);
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(recorded_files(&browsed).len(), 1);
        assert!(recorded_files(&folder).is_empty());
        assert_eq!(editor.settings.recordings_folder.as_ref(), Some(&folder));

        // The next recording goes back to the default.
        open_screen_bar_by_hand(&ctx, &mut editor);
        assert!(
            editor
                .screen_bar
                .as_ref()
                .is_some_and(|bar| bar.folder.is_none())
        );
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.3);
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(recorded_files(&folder).len(), 1);
    }

    #[test]
    fn browse_saves_one_microphone_recording_elsewhere() {
        let ctx = context();
        let (mut editor, folder) = ready_to_record(&ctx, "mic-browse");
        fn picks_microphone_folder(from: &Path) -> Option<PathBuf> {
            picks_another_folder(from).map(|folder| folder.join("microphone"))
        }
        let browsed = picks_microphone_folder(Path::new("")).expect("a folder");
        let _ = std::fs::remove_dir_all(&browsed);
        editor.pick_folder = picks_microphone_folder;
        open_record_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, &components::FolderBox::name(&folder));
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.5);
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(recorded_files(&browsed).len(), 1);
        assert!(recorded_files(&folder).is_empty());
        open_record_bar_by_hand(&ctx, &mut editor);
        assert!(
            editor
                .record_bar
                .as_ref()
                .is_some_and(|bar| bar.folder.is_none())
        );
    }

    #[test]
    fn escape_closes_the_screen_bar_before_recording() {
        let ctx = context();
        let (mut editor, folder) = ready_to_record_the_screen(&ctx, "screen-escape");
        open_screen_bar_by_hand(&ctx, &mut editor);
        run_frame(&ctx, &mut editor, key_event(Key::Escape, Modifiers::NONE));
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.screen_bar.is_none());
        assert!(recorded_files(&folder).is_empty());
    }

    /// Stops a made-up camera's pictures when it is closed.
    struct StopCamera(Arc<AtomicBool>);

    impl Drop for StopCamera {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    /// Stands in for a camera: red on its left half and blue on its right, 30 times a second,
    /// in either of two modes. "Failing camera" stops after a moment, like one unplugged.
    fn made_up_camera(
        camera: &bareclip_capture::Camera,
        wanted: Option<bareclip_capture::CameraMode>,
    ) -> camera_recording::CameraSource {
        use bareclip_capture::{CameraMode, CameraState, CameraView, Picture};
        let modes = vec![
            CameraMode {
                width: 640,
                height: 360,
                rate: (30, 1),
            },
            CameraMode {
                width: 320,
                height: 180,
                rate: (15, 1),
            },
        ];
        let mode = wanted
            .and_then(|wanted| modes.iter().copied().find(|mode| mode.same_as(wanted)))
            .unwrap_or(modes[0]);
        let fails = camera.name == "Failing camera";
        let view = CameraView::new();
        let stop = Arc::new(AtomicBool::new(false));
        let (stopping, shown) = (stop.clone(), view.clone());
        thread::spawn(move || {
            let (width, height) = (mode.width as usize, mode.height as usize);
            let mut row = [0_u8, 0, 200, 255].repeat(width / 2);
            row.extend([200_u8, 0, 0, 255].repeat(width - width / 2));
            let bgra = row.repeat(height);
            shown.set_state(CameraState::Running { modes, mode });
            let started = Instant::now();
            while !stopping.load(Ordering::Acquire) {
                if fails && started.elapsed() > Duration::from_millis(600) {
                    shown.set_state(CameraState::Failed(
                        "The camera stopped. It may have been unplugged.".to_owned(),
                    ));
                    return;
                }
                shown.show(Picture {
                    width: mode.width,
                    height: mode.height,
                    bgra: bgra.clone(),
                    shown_at: 0,
                });
                thread::sleep(Duration::from_millis(33));
            }
        });
        camera_recording::CameraSource {
            view,
            _keep: Box::new(StopCamera(stop)),
        }
    }

    fn made_up_cameras() -> Vec<bareclip_capture::Camera> {
        ["Desk camera", "Phone camera"]
            .iter()
            .map(|name| bareclip_capture::Camera {
                name: (*name).to_owned(),
                link: format!("made-up {name}"),
            })
            .collect()
    }

    fn no_cameras() -> Vec<bareclip_capture::Camera> {
        Vec::new()
    }

    fn failing_cameras() -> Vec<bareclip_capture::Camera> {
        vec![bareclip_capture::Camera {
            name: "Failing camera".to_owned(),
            link: "made-up failing".to_owned(),
        }]
    }

    /// An editor ready to record a made-up camera with a made-up microphone.
    fn ready_to_record_the_camera(ctx: &egui::Context, name: &str) -> (Editor, PathBuf) {
        let (mut editor, folder) = ready_to_record(ctx, name);
        editor.open_camera = made_up_camera;
        editor.list_cameras = made_up_cameras;
        editor.settings.camera_microphone = true;
        (editor, folder)
    }

    /// Runs frames until the camera has started or failed, which it does on a thread of its own
    /// and can take a while on a busy machine. Pressing Start before then would do nothing.
    fn wait_for_the_camera(ctx: &egui::Context, editor: &mut Editor) {
        let started = Instant::now();
        loop {
            run_frame(ctx, editor, vec![]);
            let starting = editor.camera_pick().is_some_and(|pick| {
                matches!(pick.state(), bareclip_capture::CameraState::Starting)
            });
            if !starting || started.elapsed() > Duration::from_secs(5) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        run_for(ctx, editor, 0.1);
    }

    fn open_camera_bar_by_hand(ctx: &egui::Context, editor: &mut Editor) {
        if !is_named(ctx, editor, "Camera") {
            click_named(ctx, editor, "Record");
            run_frame(ctx, editor, vec![]);
        }
        click_named(ctx, editor, "Camera");
        wait_for_the_camera(ctx, editor);
    }

    fn is_disabled(ctx: &egui::Context, editor: &mut Editor, name: &str) -> bool {
        named_nodes(ctx, editor)
            .into_iter()
            .find(|(label, ..)| label == name)
            .map(|(.., disabled)| disabled)
            .unwrap_or_else(|| panic!("{name} is there"))
    }

    /// Whether a frame draws this text anywhere.
    fn shows_text(ctx: &egui::Context, editor: &mut Editor, wanted: &str) -> bool {
        let output = run_frame(ctx, editor, vec![]);
        let everything =
            egui::Rect::from_min_size(pos2(-10_000.0, -10_000.0), vec2(20_000.0, 20_000.0));
        text_in(&output, everything)
            .iter()
            .any(|text| text == wanted)
    }

    fn sound_track_names(path: &Path) -> Vec<Option<String>> {
        bareclip_media::probe(path)
            .expect("it can be read")
            .audio
            .iter()
            .map(|track| track.title.clone())
            .collect()
    }

    #[test]
    fn a_camera_recording_lands_as_a_video_with_the_microphone_at_the_playhead() {
        let ctx = context();
        let (mut editor, folder) = ready_to_record_the_camera(&ctx, "camera-lands");
        let tracks_before = editor.project.tracks.clone();
        editor.seek(Ticks::from_seconds(1.0));
        run_frame(&ctx, &mut editor, vec![]);
        open_camera_bar_by_hand(&ctx, &mut editor);
        assert!(editor.camera_bar.is_some());
        assert!(is_named(&ctx, &mut editor, "Camera: Desk camera"));
        assert!(is_named(&ctx, &mut editor, "Size: 640 x 360, 30 fps"));
        assert!(is_named(&ctx, &mut editor, "Microphone: Desk microphone"));
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 1.5);
        assert!(is_named(&ctx, &mut editor, "Pause recording"));
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);

        assert!(editor.camera_bar.is_none());
        let files = recorded_files(&folder);
        assert_eq!(files.len(), 1);
        let name = files[0].file_name().expect("named").to_string_lossy();
        assert!(
            name.starts_with("Camera ") && name.ends_with(".mp4"),
            "{name}"
        );
        let info = bareclip_media::probe(&files[0]).expect("it can be read");
        assert_eq!((info.video.width, info.video.height), (640, 360));
        assert_eq!(
            sound_track_names(&files[0]),
            [Some("Microphone".to_owned())]
        );

        let id = editor.selected.expect("the recording is selected");
        let clip = editor.project.tracks.clip(id).expect("on the timeline");
        assert_eq!(clip.start, Ticks::from_seconds(1.0));
        assert_eq!(
            editor.project.tracks.track_of(id),
            Some(0),
            "a new row on top"
        );
        let seconds = clip.duration().as_seconds();
        assert!(seconds > 1.0 && seconds < 2.3, "{seconds}");
        editor.undo();
        assert_eq!(
            editor.project.tracks, tracks_before,
            "one step, and nothing moved"
        );
    }

    #[test]
    fn the_camera_and_its_size_are_remembered_and_the_microphone_lasts_one_recording() {
        let ctx = context();
        let (mut editor, folder) = ready_to_record_the_camera(&ctx, "camera-choose");
        open_camera_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Camera: Desk camera");
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Phone camera");
        run_for(&ctx, &mut editor, 0.2);
        assert_eq!(editor.settings.camera.as_deref(), Some("Phone camera"));
        click_named(&ctx, &mut editor, "Size: 640 x 360, 30 fps");
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "320 x 180, 15 fps");
        run_for(&ctx, &mut editor, 0.2);
        assert_eq!(
            editor.settings.camera_mode,
            Some(storage::CameraModeChoice {
                width: 320,
                height: 180,
                rate: (15, 1),
            })
        );
        click_named(&ctx, &mut editor, "Microphone: Desk microphone");
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Headset microphone");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(is_named(
            &ctx,
            &mut editor,
            "Microphone: Headset microphone"
        ));
        assert_eq!(editor.settings.microphone, None, "Settings' default stays");

        // Opened again, it starts on the camera and size, and on the default microphone.
        click_named(&ctx, &mut editor, "Cancel");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.camera_bar.is_none());
        open_camera_bar_by_hand(&ctx, &mut editor);
        assert!(is_named(&ctx, &mut editor, "Camera: Phone camera"));
        assert!(is_named(&ctx, &mut editor, "Size: 320 x 180, 15 fps"));
        assert!(is_named(&ctx, &mut editor, "Microphone: Desk microphone"));
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.6);
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);
        let file = recorded_files(&folder).pop().expect("a file");
        let info = bareclip_media::probe(&file).expect("it can be read");
        assert_eq!((info.video.width, info.video.height), (320, 180));

        // Another camera starts on its own usual size.
        open_camera_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Camera: Phone camera");
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Desk camera");
        run_for(&ctx, &mut editor, 0.2);
        assert_eq!(editor.settings.camera_mode, None);
        assert!(is_named(&ctx, &mut editor, "Size: 640 x 360, 30 fps"));
    }

    #[test]
    fn with_the_microphone_off_a_camera_records_only_the_picture() {
        let ctx = context();
        let (mut editor, folder) = ready_to_record_the_camera(&ctx, "camera-silent");
        editor.open_microphone = counted_microphone;
        open_camera_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Record the microphone");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(!editor.settings.camera_microphone);
        // The list stays, and picking from it while it's off listens to nothing.
        let opened = microphones_opened().len();
        click_named(&ctx, &mut editor, "Microphone: Desk microphone");
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Headset microphone");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(is_named(
            &ctx,
            &mut editor,
            "Microphone: Headset microphone"
        ));
        assert_eq!(microphones_opened().len(), opened);
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.6);
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);
        let file = recorded_files(&folder).pop().expect("a file");
        assert!(sound_track_names(&file).is_empty());
        assert!(editor.selected.is_some(), "it still lands");
    }

    #[test]
    fn a_camera_recording_with_the_video_playing_lands_where_the_video_was_when_it_began() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record_the_camera(&ctx, "camera-along");
        editor.settings.play_while_recording = true;
        editor.seek(Ticks::from_seconds(0.5));
        run_frame(&ctx, &mut editor, vec![]);
        open_camera_bar_by_hand(&ctx, &mut editor);
        assert!(shows_text(
            &ctx,
            &mut editor,
            "Wear headphones, or the microphone will hear the video too."
        ));
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 1.2);

        assert!(editor.preview.is_playing());
        assert!(
            !is_named(&ctx, &mut editor, "Pause recording"),
            "pausing would lose step"
        );
        let (lands, recorded) = match editor.camera_bar.as_ref().map(|bar| &bar.stage) {
            Some(camera_recording::CameraStage::Recording(live)) => {
                (live.lands_at(), live.seconds())
            }
            _ => panic!("it is recording"),
        };
        assert!(
            lands >= Ticks::from_seconds(0.5) && lands < Ticks::from_seconds(1.0),
            "{lands:?}"
        );
        let moved = (editor.position().0 - lands.0) as f64 / Ticks::from_seconds(1.0).0 as f64;
        assert!(
            (moved - recorded).abs() < 0.15,
            "{moved} against {recorded}"
        );

        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(!editor.preview.is_playing(), "stopping stops the video too");
        let id = editor.selected.expect("the recording is selected");
        assert_eq!(editor.project.tracks.clip(id).expect("placed").start, lands);
    }

    #[test]
    fn the_countdown_comes_before_a_camera_recording() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record_the_camera(&ctx, "camera-countdown");
        editor.settings.countdown = true;
        open_camera_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Start recording");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(matches!(
            editor.camera_bar.as_ref().map(|bar| &bar.stage),
            Some(camera_recording::CameraStage::Counting(_))
        ));
        assert!(!is_named(&ctx, &mut editor, "Stop"));
        if let Some(bar) = &mut editor.camera_bar {
            bar.stage = camera_recording::CameraStage::Counting(0.0);
        }
        run_frame(&ctx, &mut editor, vec![]);
        run_frame(&ctx, &mut editor, vec![]);
        assert!(is_named(&ctx, &mut editor, "Stop"));
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.camera_bar.is_none());
    }

    #[test]
    fn without_a_camera_the_bar_says_so_and_can_look_again() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record_the_camera(&ctx, "camera-none");
        editor.list_cameras = no_cameras;
        open_camera_bar_by_hand(&ctx, &mut editor);
        assert!(is_disabled(&ctx, &mut editor, "Start recording"));
        assert!(!is_named(&ctx, &mut editor, "Camera: Desk camera"));

        // Plugged in, and found again.
        editor.list_cameras = made_up_cameras;
        click_named(&ctx, &mut editor, "Look again");
        run_for(&ctx, &mut editor, 0.2);
        assert!(is_named(&ctx, &mut editor, "Camera: Desk camera"));
        assert!(!is_disabled(&ctx, &mut editor, "Start recording"));
    }

    #[test]
    fn a_camera_lost_while_recording_is_shown_and_what_was_recorded_still_lands() {
        let ctx = context();
        let (mut editor, folder) = ready_to_record_the_camera(&ctx, "camera-lost");
        editor.list_cameras = failing_cameras;
        open_camera_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 1.0);
        assert!(shows_text(&ctx, &mut editor, "Camera lost"));
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(recorded_files(&folder).len(), 1);
        assert!(editor.selected.is_some());
    }

    #[test]
    fn escape_closes_the_camera_bar_only_before_recording() {
        let ctx = context();
        let (mut editor, folder) = ready_to_record_the_camera(&ctx, "camera-escape");
        open_camera_bar_by_hand(&ctx, &mut editor);
        run_frame(&ctx, &mut editor, key_event(Key::Escape, Modifiers::NONE));
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.camera_bar.is_none());
        assert!(recorded_files(&folder).is_empty());

        open_camera_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.3);
        run_frame(&ctx, &mut editor, key_event(Key::Escape, Modifiers::NONE));
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.camera_bar.is_some(), "a recording isn't thrown away");
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);
    }

    #[test]
    fn closing_the_window_while_recording_the_camera_keeps_the_recording() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record_the_camera(&ctx, "camera-closing");
        open_camera_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.6);
        editor.handle_close_request(&ctx);
        assert!(editor.camera_bar.is_none());
        let id = editor.selected.expect("the recording is selected");
        assert!(editor.project.tracks.clip(id).is_some());
        assert!(
            editor.leave_for.is_some(),
            "it asks about saving the project"
        );
    }

    #[test]
    fn opening_one_recording_bar_closes_the_others() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record_the_camera(&ctx, "camera-switch");
        editor.open_screen = made_up_screen;
        editor.open_pc_sound = made_up_pc_sound;
        editor.list_sources = made_up_sources;
        open_camera_bar_by_hand(&ctx, &mut editor);
        open_screen_bar_by_hand(&ctx, &mut editor);
        assert!(editor.camera_bar.is_none() && editor.screen_bar.is_some());
        open_camera_bar_by_hand(&ctx, &mut editor);
        assert!(editor.screen_bar.is_none() && editor.camera_bar.is_some());
        open_record_bar_by_hand(&ctx, &mut editor);
        assert!(editor.camera_bar.is_none() && editor.record_bar.is_some());
        open_screen_and_camera_bar_by_hand(&ctx, &mut editor);
        assert!(editor.record_bar.is_none());
        assert!(
            editor
                .screen_bar
                .as_ref()
                .is_some_and(|bar| bar.camera.is_some())
        );
        open_screen_bar_by_hand(&ctx, &mut editor);
        assert!(
            editor
                .screen_bar
                .as_ref()
                .is_some_and(|bar| bar.camera.is_none()),
            "the plain screen bar has no camera"
        );
    }

    /// An editor ready to record a made-up screen and camera, with made-up PC sound and
    /// microphone.
    fn ready_to_record_the_screen_and_camera(ctx: &egui::Context, name: &str) -> (Editor, PathBuf) {
        let (mut editor, folder) = ready_to_record_the_screen(ctx, name);
        editor.open_camera = made_up_camera;
        editor.list_cameras = made_up_cameras;
        editor.settings.screen_microphone = true;
        (editor, folder)
    }

    fn open_screen_and_camera_bar_by_hand(ctx: &egui::Context, editor: &mut Editor) {
        if !is_named(ctx, editor, "Screen and camera") {
            click_named(ctx, editor, "Record");
            run_frame(ctx, editor, vec![]);
        }
        click_named(ctx, editor, "Screen and camera");
        wait_for_the_camera(ctx, editor);
    }

    /// The clip on the timeline playing a file whose name starts with `kind`, and its row.
    fn clip_of_kind(editor: &Editor, kind: &str) -> Option<(bareclip_core::timeline::Clip, usize)> {
        let tracks = editor.project.tracks.tracks();
        tracks.iter().enumerate().find_map(|(row, track)| {
            track.clips().iter().find_map(|clip| {
                let path = &editor.entry(clip.media)?.info.path;
                let name = path.file_name()?.to_string_lossy();
                name.starts_with(kind).then_some((*clip, row))
            })
        })
    }

    fn file_of_kind(folder: &Path, kind: &str) -> Option<PathBuf> {
        recorded_files(folder).into_iter().find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(kind))
        })
    }

    fn seconds_of(path: &Path) -> f64 {
        bareclip_media::probe(path)
            .expect("it can be read")
            .duration
            .as_seconds()
    }

    #[test]
    fn a_screen_and_camera_recording_lands_as_three_clips_in_step_as_one_undo_step() {
        let ctx = context();
        let (mut editor, folder) =
            ready_to_record_the_screen_and_camera(&ctx, "screen-camera-lands");
        let tracks_before = editor.project.tracks.clone();
        editor.seek(Ticks::from_seconds(1.0));
        run_frame(&ctx, &mut editor, vec![]);
        open_screen_and_camera_bar_by_hand(&ctx, &mut editor);
        assert!(is_named(
            &ctx,
            &mut editor,
            "Record: Screen 1 (main), 640 x 360"
        ));
        assert!(is_named(&ctx, &mut editor, "Camera: Desk camera"));
        assert!(is_named(&ctx, &mut editor, "Size: 640 x 360, 30 fps"));
        assert!(is_named(&ctx, &mut editor, "Microphone: Desk microphone"));
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 1.5);
        assert!(is_named(&ctx, &mut editor, "Turn the microphone off"));
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.screen_bar.is_none());

        // Three files, named for the same moment.
        let screen = file_of_kind(&folder, "Screen ").expect("the screen's file");
        let camera = file_of_kind(&folder, "Camera ").expect("the camera's file");
        let microphone = file_of_kind(&folder, "Microphone ").expect("the microphone's file");
        assert_eq!(recorded_files(&folder).len(), 3);
        let when = |path: &Path| {
            let name = path
                .file_stem()
                .expect("named")
                .to_string_lossy()
                .into_owned();
            name.split_once(' ').map(|(_, when)| when.to_owned())
        };
        assert_eq!(when(&screen), when(&camera));
        assert_eq!(when(&screen), when(&microphone));
        assert_eq!(sound_track_names(&screen), [Some("PC sound".to_owned())]);
        assert!(
            sound_track_names(&camera).is_empty(),
            "the camera is silent"
        );
        let heard = bareclip_media::probe(&microphone).expect("it can be read");
        assert!(heard.is_sound_only() && heard.audio.len() == 1);
        let info = bareclip_media::probe(&camera).expect("it can be read");
        assert_eq!((info.video.width, info.video.height), (640, 360));
        // They began and ended together.
        let lengths = [
            seconds_of(&screen),
            seconds_of(&camera),
            seconds_of(&microphone),
        ];
        for length in lengths {
            assert!((length - lengths[0]).abs() < 0.15, "{lengths:?}");
        }

        let (camera_clip, camera_row) = clip_of_kind(&editor, "Camera ").expect("the camera");
        let (screen_clip, screen_row) = clip_of_kind(&editor, "Screen ").expect("the screen");
        let (microphone_clip, microphone_row) =
            clip_of_kind(&editor, "Microphone ").expect("the microphone");
        assert_eq!(
            (camera_row, screen_row),
            (0, 1),
            "the camera over the screen"
        );
        assert!(editor.project.tracks.is_sound_row(microphone_row));
        for clip in [&camera_clip, &screen_clip, &microphone_clip] {
            assert_eq!(clip.start, Ticks::from_seconds(1.0));
        }
        let canvas = editor.project.canvas.size();
        assert_eq!(
            camera_clip.placement,
            bareclip_core::placement::Placement::default().in_corner(
                bareclip_core::placement::CanvasCorner::BottomRight,
                (640, 360),
                canvas
            ),
            "Picture in picture's size, bottom right"
        );
        assert_eq!(
            screen_clip.placement,
            bareclip_core::placement::Placement::default()
        );
        editor.undo();
        assert_eq!(
            editor.project.tracks, tracks_before,
            "one step, and nothing moved"
        );
    }

    #[test]
    fn a_microphone_never_turned_on_leaves_no_file_or_clip() {
        let ctx = context();
        let (mut editor, folder) =
            ready_to_record_the_screen_and_camera(&ctx, "screen-camera-silent");
        editor.settings.screen_microphone = false;
        open_screen_and_camera_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.8);
        assert!(is_named(&ctx, &mut editor, "Turn the microphone on"));
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(recorded_files(&folder).len(), 2);
        assert!(file_of_kind(&folder, "Microphone ").is_none());
        assert!(clip_of_kind(&editor, "Microphone ").is_none());
        assert!(clip_of_kind(&editor, "Camera ").is_some());
    }

    #[test]
    fn a_microphone_turned_on_midway_is_kept_and_silent_until_then() {
        let ctx = context();
        let (mut editor, folder) =
            ready_to_record_the_screen_and_camera(&ctx, "screen-camera-midway");
        editor.settings.screen_microphone = false;
        open_screen_and_camera_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.6);
        click_named(&ctx, &mut editor, "Turn the microphone on");
        run_for(&ctx, &mut editor, 0.6);
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);
        let microphone = file_of_kind(&folder, "Microphone ").expect("kept");
        let screen = file_of_kind(&folder, "Screen ").expect("the screen");
        assert!((seconds_of(&microphone) - seconds_of(&screen)).abs() < 0.15);
        assert!(clip_of_kind(&editor, "Microphone ").is_some());
    }

    #[test]
    fn pausing_pauses_the_screen_the_camera_and_the_microphone_together() {
        let ctx = context();
        let (mut editor, folder) =
            ready_to_record_the_screen_and_camera(&ctx, "screen-camera-paused");
        open_screen_and_camera_bar_by_hand(&ctx, &mut editor);
        // Timed as it really runs, since a busy machine stretches every wait. Each press is timed
        // before and after, because on a busy machine a press itself takes a while, and the
        // recording only knows it happened somewhere between the two.
        let started = timed_click(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.7);
        let paused = timed_click(&ctx, &mut editor, "Pause recording");
        run_for(&ctx, &mut editor, 1.0);
        let resumed = timed_click(&ctx, &mut editor, "Resume recording");
        run_for(&ctx, &mut editor, 0.7);
        let stopped = timed_click(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);
        let seconds = |later: Instant, earlier: Instant| (later - earlier).as_secs_f64();
        let shortest = seconds(paused.0, started.1) + seconds(stopped.0, resumed.1);
        let longest = seconds(paused.1, started.0) + seconds(stopped.1, resumed.0);
        let lengths = ["Screen ", "Camera ", "Microphone "]
            .map(|kind| seconds_of(&file_of_kind(&folder, kind).expect("a file")));
        for length in lengths {
            assert!(
                length > shortest - 0.3 && length < longest + 0.3,
                "{lengths:?} against {shortest} to {longest}"
            );
        }
        let [screen, camera, microphone] = lengths;
        assert!((screen - camera).abs() < 0.15, "{lengths:?}");
        assert!((screen - microphone).abs() < 0.25, "{lengths:?}");
    }

    #[test]
    fn the_overlay_shows_the_camera_on_request_and_folding_closes_it() {
        let ctx = context();
        let (mut editor, _folder) =
            ready_to_record_the_screen_and_camera(&ctx, "screen-camera-overlay");
        open_screen_and_camera_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.4);
        assert!(
            is_named(&ctx, &mut editor, "Show the camera"),
            "it starts hidden"
        );
        assert!(!is_named(&ctx, &mut editor, "The camera's picture"));
        click_named(&ctx, &mut editor, "Show the camera");
        run_for(&ctx, &mut editor, 0.2);
        assert!(is_named(&ctx, &mut editor, "Hide the camera"));
        assert!(is_named(&ctx, &mut editor, "The camera's picture"));

        // Folding takes the picture and its switch away, and unfolding doesn't bring it back.
        click_named(&ctx, &mut editor, "Show only Pause and Stop");
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Show all controls");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(is_named(&ctx, &mut editor, "Show the camera"));
        assert!(!is_named(&ctx, &mut editor, "The camera's picture"));
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);
    }

    #[test]
    fn a_screen_on_its_own_has_no_camera_switch() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record_the_screen(&ctx, "screen-no-camera");
        open_screen_bar_by_hand(&ctx, &mut editor);
        click_named(&ctx, &mut editor, "Start recording");
        run_for(&ctx, &mut editor, 0.3);
        assert!(!is_named(&ctx, &mut editor, "Show the camera"));
        click_named(&ctx, &mut editor, "Stop");
        run_frame(&ctx, &mut editor, vec![]);
    }

    #[test]
    fn without_a_camera_screen_and_camera_cant_start() {
        let ctx = context();
        let (mut editor, _folder) =
            ready_to_record_the_screen_and_camera(&ctx, "screen-camera-none");
        editor.list_cameras = no_cameras;
        open_screen_and_camera_bar_by_hand(&ctx, &mut editor);
        assert!(is_disabled(&ctx, &mut editor, "Start recording"));
        editor.list_cameras = made_up_cameras;
        click_named(&ctx, &mut editor, "Look again");
        run_for(&ctx, &mut editor, 0.2);
        assert!(!is_disabled(&ctx, &mut editor, "Start recording"));
    }

    #[test]
    fn the_screen_bars_microphone_can_be_chosen_while_off_and_is_used_once_on() {
        let ctx = context();
        let (mut editor, _folder) = ready_to_record_the_screen(&ctx, "screen-choose-mic");
        editor.open_microphone = counted_microphone;
        open_screen_bar_by_hand(&ctx, &mut editor);
        let opened = microphones_opened().len();
        click_named(&ctx, &mut editor, "Microphone: Desk microphone");
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Headset microphone");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(is_named(
            &ctx,
            &mut editor,
            "Microphone: Headset microphone"
        ));
        assert_eq!(
            microphones_opened().len(),
            opened,
            "nothing is listened to while it's off"
        );
        assert_eq!(editor.settings.microphone, None, "Settings' default stays");

        click_rightmost_named(&ctx, &mut editor, "Microphone");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.settings.screen_microphone);
        assert_eq!(
            microphones_opened().last(),
            Some(&Some("Headset microphone".to_owned()))
        );
    }

    /// Four two-second clips: three on the top row at 0, 3 and 6 seconds, and one on the row
    /// below at 4 seconds.
    fn clips_to_select(ctx: &egui::Context) -> (Editor, Vec<ClipId>) {
        let mut editor = in_panel_only(ctx, &[test_clip()]);
        let media = editor.media[0].id;
        let piece = TimeRange {
            start: Ticks(0),
            end: Ticks::from_seconds(2.0),
        };
        let at = Ticks::from_seconds;
        let tracks = &mut editor.project.tracks;
        let ids = vec![
            tracks.place(media, piece, at(0.0), TrackTarget::New(0)),
            tracks.place(media, piece, at(3.0), TrackTarget::Existing(0)),
            tracks.place(media, piece, at(6.0), TrackTarget::Existing(0)),
            tracks.place(media, piece, at(4.0), TrackTarget::New(1)),
        ];
        editor.settle_canvas();
        editor.after_clip_added();
        editor.select_only(None);
        for _ in 0..3 {
            run_frame(ctx, &mut editor, vec![]);
        }
        (editor, ids)
    }

    /// The middle of a clip on the timeline.
    fn clip_point(editor: &Editor, id: ClipId) -> egui::Pos2 {
        let tracks = &editor.project.tracks;
        let clip = tracks.clip(id).expect("on the timeline");
        let row = tracks.track_of(id).expect("on a row");
        let middle = Ticks(clip.start.0 + clip.duration().0 / 2);
        pos2(
            editor.timeline_state.scale.x_of(middle),
            editor.timeline_state.layout.track(row).center().y,
        )
    }

    fn click_holding(ctx: &egui::Context, editor: &mut Editor, at: egui::Pos2, held: Modifiers) {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: held,
        };
        let holding = egui::Event::ModifiersChanged(held);
        run_frame(ctx, editor, vec![egui::Event::PointerMoved(at)]);
        run_frame(ctx, editor, vec![holding.clone(), button(true)]);
        run_frame(ctx, editor, vec![holding, button(false)]);
        run_frame(
            ctx,
            editor,
            vec![egui::Event::ModifiersChanged(Modifiers::NONE)],
        );
    }

    fn click_clip(ctx: &egui::Context, editor: &mut Editor, id: ClipId, held: Modifiers) {
        let at = clip_point(editor, id);
        click_holding(ctx, editor, at, held);
    }

    fn selection_of(editor: &Editor) -> Vec<ClipId> {
        let mut ids = editor.selected_clips();
        ids.sort();
        ids
    }

    fn sorted(mut ids: Vec<ClipId>) -> Vec<ClipId> {
        ids.sort();
        ids
    }

    #[test]
    fn ctrl_click_adds_and_takes_away_clips_and_a_plain_click_selects_one_again() {
        let ctx = context();
        let (mut editor, ids) = clips_to_select(&ctx);
        click_clip(&ctx, &mut editor, ids[0], Modifiers::NONE);
        assert_eq!(selection_of(&editor), [ids[0]]);
        click_clip(&ctx, &mut editor, ids[2], Modifiers::CTRL);
        click_clip(&ctx, &mut editor, ids[3], Modifiers::CTRL);
        assert_eq!(selection_of(&editor), sorted(vec![ids[0], ids[2], ids[3]]));
        click_clip(&ctx, &mut editor, ids[2], Modifiers::CTRL);
        assert_eq!(selection_of(&editor), sorted(vec![ids[0], ids[3]]));
        click_clip(&ctx, &mut editor, ids[1], Modifiers::NONE);
        assert_eq!(selection_of(&editor), [ids[1]]);
    }

    #[test]
    fn shift_click_selects_everything_that_starts_between_across_the_rows() {
        let ctx = context();
        let (mut editor, ids) = clips_to_select(&ctx);
        click_clip(&ctx, &mut editor, ids[0], Modifiers::NONE);
        click_clip(&ctx, &mut editor, ids[3], Modifiers::SHIFT);
        // From 0 s on the top row to 4 s on the row below: not the clip at 6 s.
        assert_eq!(selection_of(&editor), sorted(vec![ids[0], ids[1], ids[3]]));
        assert_eq!(
            editor.selected,
            Some(ids[0]),
            "the first clicked stays the main one"
        );
    }

    #[test]
    fn ctrl_a_selects_the_selected_clips_row_and_nothing_without_a_selection() {
        let ctx = context();
        let (mut editor, ids) = clips_to_select(&ctx);
        run_frame(&ctx, &mut editor, key_event(Key::A, Modifiers::COMMAND));
        assert!(editor.selected_clips().is_empty());

        click_clip(&ctx, &mut editor, ids[1], Modifiers::NONE);
        run_frame(&ctx, &mut editor, key_event(Key::A, Modifiers::COMMAND));
        assert_eq!(selection_of(&editor), sorted(vec![ids[0], ids[1], ids[2]]));
    }

    #[test]
    fn deleting_several_clips_is_one_step_to_undo() {
        let ctx = context();
        let (mut editor, ids) = clips_to_select(&ctx);
        let before = editor.project.tracks.clone();
        click_clip(&ctx, &mut editor, ids[0], Modifiers::NONE);
        click_clip(&ctx, &mut editor, ids[2], Modifiers::CTRL);
        run_frame(&ctx, &mut editor, press(Key::Delete));
        let tracks = &editor.project.tracks;
        assert!(tracks.clip(ids[0]).is_none() && tracks.clip(ids[2]).is_none());
        assert!(tracks.clip(ids[1]).is_some() && tracks.clip(ids[3]).is_some());
        assert!(editor.selected_clips().is_empty());
        editor.undo();
        assert_eq!(editor.project.tracks, before);
    }

    #[test]
    fn copy_and_paste_keep_the_gaps_and_rows_and_select_what_was_pasted() {
        let ctx = context();
        let (mut editor, ids) = clips_to_select(&ctx);
        click_clip(&ctx, &mut editor, ids[0], Modifiers::NONE);
        click_clip(&ctx, &mut editor, ids[3], Modifiers::CTRL);
        run_frame(&ctx, &mut editor, key_event(Key::C, Modifiers::COMMAND));
        editor.seek(Ticks::from_seconds(10.0));
        let before = editor.project.tracks.clone();
        run_frame(&ctx, &mut editor, key_event(Key::V, Modifiers::COMMAND));

        let pasted = editor.selected_clips();
        assert_eq!(pasted.len(), 2);
        let tracks = &editor.project.tracks;
        let mut placed: Vec<(Ticks, usize)> = pasted
            .iter()
            .map(|id| {
                (
                    tracks.clip(*id).expect("placed").start,
                    tracks.track_of(*id).expect("on a row"),
                )
            })
            .collect();
        placed.sort();
        // The project ends at 8 s, so that is where the playhead waits.
        let at = editor.position();
        assert_eq!(at, Ticks::from_seconds(8.0));
        assert_eq!(
            placed,
            [(at, 0), (Ticks(at.0 + Ticks::from_seconds(4.0).0), 1)]
        );
        editor.undo();
        assert_eq!(editor.project.tracks, before, "one step to undo");
    }

    #[test]
    fn duplicating_several_puts_the_copies_after_the_last_keeping_their_gaps() {
        let ctx = context();
        let (mut editor, ids) = clips_to_select(&ctx);
        click_clip(&ctx, &mut editor, ids[1], Modifiers::NONE);
        click_clip(&ctx, &mut editor, ids[2], Modifiers::CTRL);
        run_frame(&ctx, &mut editor, key_event(Key::D, Modifiers::COMMAND));
        let copies = editor.selected_clips();
        assert_eq!(copies.len(), 2);
        assert!(!copies.contains(&ids[1]) && !copies.contains(&ids[2]));
        let tracks = &editor.project.tracks;
        let mut starts: Vec<Ticks> = copies
            .iter()
            .map(|id| tracks.clip(*id).expect("placed").start)
            .collect();
        starts.sort();
        // The last ends at 8 s, and the two were 3 s apart.
        assert_eq!(
            starts,
            [Ticks::from_seconds(8.0), Ticks::from_seconds(11.0)]
        );
    }

    #[test]
    fn dragging_one_of_several_moves_them_all_by_the_same_amount() {
        let ctx = context();
        let (mut editor, ids) = clips_to_select(&ctx);
        click_clip(&ctx, &mut editor, ids[0], Modifiers::NONE);
        click_clip(&ctx, &mut editor, ids[3], Modifiers::CTRL);
        let before = editor.project.tracks.clone();
        let from = clip_point(&editor, ids[0]);
        let to = from + vec2(time_x(&editor, 1.0) - time_x(&editor, 0.0), 0.0);
        begin_drag(&ctx, &mut editor, from, to);
        end_drag(&ctx, &mut editor, to);

        let start = |id| editor.project.tracks.clip(id).map(|clip| clip.start);
        assert_eq!(start(ids[0]), Some(Ticks::from_seconds(1.0)));
        assert_eq!(
            start(ids[3]),
            Some(Ticks::from_seconds(5.0)),
            "moved along with it"
        );
        assert_eq!(
            start(ids[1]),
            Some(Ticks::from_seconds(3.0)),
            "not selected, so it stays"
        );
        assert_eq!(
            selection_of(&editor),
            sorted(vec![ids[0], ids[3]]),
            "still both"
        );
        editor.undo();
        assert_eq!(editor.project.tracks, before, "one step to undo");
    }

    #[test]
    fn right_clicking_one_of_several_keeps_them_all() {
        let ctx = context();
        let (mut editor, ids) = clips_to_select(&ctx);
        click_clip(&ctx, &mut editor, ids[0], Modifiers::NONE);
        click_clip(&ctx, &mut editor, ids[2], Modifiers::CTRL);
        let at = clip_point(&editor, ids[2]);
        secondary_click(&ctx, &mut editor, at);
        assert_eq!(selection_of(&editor), sorted(vec![ids[0], ids[2]]));
        // A right-click on a clip outside them selects only it.
        run_frame(&ctx, &mut editor, press(Key::Escape));
        let at = clip_point(&editor, ids[1]);
        secondary_click(&ctx, &mut editor, at);
        assert_eq!(selection_of(&editor), [ids[1]]);
    }

    #[test]
    fn a_clips_own_tools_step_aside_while_several_are_selected() {
        let ctx = context();
        let (mut editor, ids) = clips_to_select(&ctx);
        click_clip(&ctx, &mut editor, ids[0], Modifiers::NONE);
        assert!(!editor.clip_tabs().is_empty());
        click_clip(&ctx, &mut editor, ids[1], Modifiers::CTRL);
        assert!(editor.clip_tabs().is_empty());
    }

    #[test]
    fn selecting_a_clip_another_way_lets_go_of_the_others() {
        let ctx = context();
        let (mut editor, ids) = clips_to_select(&ctx);
        click_clip(&ctx, &mut editor, ids[0], Modifiers::NONE);
        click_clip(&ctx, &mut editor, ids[1], Modifiers::CTRL);
        let media = editor.media[0].id;
        editor.add_to_timeline(media);
        run_frame(&ctx, &mut editor, vec![]);
        assert_eq!(editor.selected_clips().len(), 1, "only the new clip");
        assert!(!editor.selected_clips().contains(&ids[0]));
    }

    /// Stands in for the Recycle Bin: a folder of the test's own.
    fn test_bin() -> PathBuf {
        std::env::temp_dir()
            .join("bareclip-test-bin")
            .join(std::process::id().to_string())
    }

    fn moves_to_test_bin(path: &Path) -> Result<(), String> {
        let bin = test_bin();
        std::fs::create_dir_all(&bin).map_err(|error| error.to_string())?;
        let name = path.file_name().ok_or("no name")?;
        std::fs::rename(path, bin.join(name)).map_err(|error| error.to_string())
    }

    /// A project holding only a recording just made, which its card can delete.
    fn only_a_recording(ctx: &egui::Context, name: &str) -> (Editor, PathBuf) {
        let (mut editor, folder) = ready_to_record(ctx, name);
        editor.trash_file = moves_to_test_bin;
        open_record_bar_by_hand(ctx, &mut editor);
        click_named(ctx, &mut editor, "Start recording");
        run_for(ctx, &mut editor, 0.6);
        click_named(ctx, &mut editor, "Stop");
        run_frame(ctx, &mut editor, vec![]);
        let video = editor.media[0].id;
        editor.remove_media(video);
        run_frame(ctx, &mut editor, vec![]);
        let file = recorded_files(&folder).pop().expect("a recording");
        (editor, file)
    }

    #[test]
    fn a_recording_can_be_deleted_from_my_media_after_a_yes() {
        let ctx = context();
        let (mut editor, file) = only_a_recording(&ctx, "delete-recording");
        assert!(editor.media[0].recorded);
        click_named(&ctx, &mut editor, "My media");
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Video options");
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Delete recording");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(
            is_named(&ctx, &mut editor, "Move to Recycle Bin"),
            "it asks first"
        );
        assert!(file.exists(), "nothing happens before the yes");
        click_named(&ctx, &mut editor, "Move to Recycle Bin");
        run_for(&ctx, &mut editor, 0.5);

        assert!(!file.exists());
        let name = file.file_name().expect("named");
        assert!(test_bin().join(name).exists(), "moved to the bin");
        assert!(editor.media.is_empty(), "gone from My media");
        assert!(
            editor.project.tracks.is_empty(),
            "and its clip from the timeline"
        );
        assert!(editor.notice.is_none(), "{:?}", editor.notice);
        editor.undo();
        assert!(
            editor.project.tracks.is_empty(),
            "undo doesn't bring it back"
        );
    }

    #[test]
    fn cancelling_keeps_the_recording() {
        let ctx = context();
        let (mut editor, file) = only_a_recording(&ctx, "keep-recording");
        editor.confirm_delete_recording = Some(editor.media[0].id);
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Cancel");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(editor.confirm_delete_recording.is_none());
        assert!(file.exists());
        assert_eq!(editor.media.len(), 1);
    }

    #[test]
    fn only_recordings_can_be_deleted_from_my_media() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        click_named(&ctx, &mut editor, "Video options");
        run_frame(&ctx, &mut editor, vec![]);
        assert!(is_named(&ctx, &mut editor, "Remove from this project"));
        assert!(!is_named(&ctx, &mut editor, "Delete recording"));
    }

    #[test]
    fn a_recording_is_still_known_as_one_once_saved() {
        let ctx = context();
        let (editor, _file) = only_a_recording(&ctx, "recorded-saved");
        let file = editor.current_file();
        assert!(file.media.iter().all(|saved| saved.recorded));
        let written = serde_json::to_string(&file).expect("it can be written");
        let read: ProjectFile = serde_json::from_str(&written).expect("and read back");
        assert!(read.media[0].recorded);
    }

    #[test]
    fn a_file_that_isnt_there_cant_go_to_the_recycle_bin() {
        let missing = std::env::temp_dir().join("bareclip-no-such-recording.mp4");
        assert!(move_to_recycle_bin(&missing).is_err());
    }

    #[test]
    #[ignore = "draws pictures to look at rather than checking anything"]
    fn draw_the_selection_screens() {
        let folder = std::env::var_os("BARECLIP_SCREENS").map_or_else(
            || std::env::temp_dir().join("bareclip-screens"),
            PathBuf::from,
        );
        let ctx = context();
        let mut camera = Camera::new(folder);
        let (mut editor, ids) = clips_to_select(&ctx);
        click_clip(&ctx, &mut editor, ids[0], Modifiers::NONE);
        click_clip(&ctx, &mut editor, ids[3], Modifiers::SHIFT);
        for _ in 0..12 {
            run_frame(&ctx, &mut editor, vec![]);
        }
        camera.shoot(&ctx, &mut editor, vec![], "select-01-several");
        let from = clip_point(&editor, ids[0]);
        let to = from + vec2(time_x(&editor, 1.5) - time_x(&editor, 0.0), 0.0);
        begin_drag(&ctx, &mut editor, from, to);
        camera.shoot(&ctx, &mut editor, moved(to), "select-02-dragging");
        end_drag(&ctx, &mut editor, to);
        let (mut editor, _file) = only_a_recording(&ctx, "select-screens");
        editor.confirm_delete_recording = editor.media.first().map(|entry| entry.id);
        for _ in 0..12 {
            run_frame(&ctx, &mut editor, vec![]);
        }
        camera.shoot(&ctx, &mut editor, vec![], "select-03-delete-recording");
        editor.confirm_delete_recording = None;
        KEPT_TEXTURES.with_borrow_mut(|kept| {
            for delta in kept.iter_mut().flatten() {
                delta.clear();
            }
            *kept = None;
        });
    }
}
