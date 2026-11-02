//! The editing screen: a media panel, the preview, and the timeline below them.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::Duration,
};

use bareclip_core::{
    history::History,
    splice::{CutMode, FrameIndex},
    time::{Ticks, format_timecode},
    timeline::{ClipEdge, ClipId, MediaId, TimeRange, Timeline},
};
use bareclip_media::{
    DecodeStep, ExportPart, ExportReport, ExportRequest, MediaError, MediaInfo, PreviewFrame,
    VideoPreview, can_write_gaps, export, join_problem, plan_export, probe, scan_frames,
};
use eframe::egui::{
    self, Align, Color32, CornerRadius, FontFamily, FontId, Frame, Key, Layout, Margin, Modifiers,
    RichText, Sense, Stroke, TextureHandle, TextureOptions, pos2, vec2,
};

use crate::{
    components::{self, ActionButton, IconButton, Kind},
    fonts, icons,
    preview::Preview,
    text,
    theme::{self, Palette},
    timeline::{self, ClipView, MediaDrag, TimelineState, TimelineView},
};

const VIDEO_EXTENSIONS: [&str; 6] = ["mp4", "mkv", "mov", "webm", "avi", "ts"];
const LOGO_DARK: &[u8] = include_bytes!("../../../assets/brand/png/mark-dark-256.png");
const SHIFT_STEP_SECONDS: f64 = 1.0;
const BACKGROUND_TICK: Duration = Duration::from_millis(50);
const MIN_CLIP_SECONDS: f64 = 0.05;
/// Moving to a trimmed clip's end shows its last frame rather than what follows it.
const JUST_BEFORE_END_SECONDS: f64 = 0.001;
const THUMBNAIL_WIDTH: u32 = 160;
const THUMBNAIL_HEIGHT: u32 = 90;
const RAIL_WIDTH: f32 = 78.0;
const MEDIA_PANEL_WIDTH: f32 = 290.0;
const MEDIA_PANEL_MIN_WIDTH: f32 = 210.0;
const MEDIA_PANEL_MAX_WIDTH: f32 = 720.0;
/// Each card is at least this wide, so a wider panel fits more of them across.
const MEDIA_CARD_MIN_WIDTH: f32 = 220.0;
const MEDIA_CARD_SPACING: f32 = 10.0;
const CARD_BUTTON_SIZE: f32 = 28.0;
const CARD_RADIUS: u8 = 8;
const EMPTY_HINT_MAX_HEIGHT: f32 = 200.0;
const DIALOG_RADIUS: u8 = 10;
/// A dialog's own buttons are never narrower than this, so short names still feel solid.
const DIALOG_BUTTON_WIDTH: f32 = 84.0;
/// The playback buttons: four of 32, the play button, and the spaces between them.
const TRANSPORT_WIDTH: f32 = 180.0;
const CONTROL_SPACING: f32 = 4.0;
const TIMELINE_HEIGHT: f32 = 200.0;
const TIMELINE_MIN_HEIGHT: f32 = 160.0;
/// Whatever the timeline's height, the preview keeps this share of the room.
const TIMELINE_MAX_SHARE: f32 = 0.65;
/// How far the arrow keys step in dead space, which has no frames of its own.
const GAP_STEP_SECONDS: f64 = 1.0 / 30.0;

/// What a video's background reading sends back.
enum ScanMessage {
    Thumbnail(PreviewFrame),
    Frames(Result<FrameIndex, MediaError>),
}

/// A video in the project.
struct MediaEntry {
    id: MediaId,
    info: MediaInfo,
    name: String,
    index: Option<Arc<FrameIndex>>,
    scan: Option<Receiver<ScanMessage>>,
    cancel_scan: Arc<AtomicBool>,
    progress: Arc<AtomicU32>,
    thumbnail: Option<TextureHandle>,
}

impl Drop for MediaEntry {
    fn drop(&mut self) {
        self.cancel_scan.store(true, Ordering::Relaxed);
    }
}

/// The parts of the project that undo and redo bring back.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Snapshot {
    timeline: Timeline,
    /// Videos removed from the panel. They stay loaded so undo can restore them.
    hidden: Vec<MediaId>,
}

/// A trim in progress: where the clip's edge was when the drag started.
#[derive(Clone, Copy, Debug)]
struct TrimOrigin {
    source: Ticks,
    pointer: Ticks,
}

struct ExportTask {
    result: Receiver<Result<ExportReport, MediaError>>,
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    output: PathBuf,
}

enum ExportState {
    Closed,
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
    confirm_remove: Option<MediaId>,
    logo: Option<TextureHandle>,
    loaded_fallbacks: Vec<&'static str>,
    /// Where things were drawn in the last frame.
    media_panel_rect: egui::Rect,
    timeline_panel_rect: egui::Rect,
    card_thumbnails: Vec<(MediaId, egui::Rect)>,
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
                timeline: Timeline::default(),
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
            confirm_remove: None,
            logo: None,
            loaded_fallbacks: Vec::new(),
            media_panel_rect: egui::Rect::NOTHING,
            timeline_panel_rect: egui::Rect::NOTHING,
            card_thumbnails: Vec::new(),
        }
    }

    fn position(&self) -> Ticks {
        self.preview.position(&self.project.timeline)
    }

    fn entry(&self, id: MediaId) -> Option<&MediaEntry> {
        self.media.iter().find(|entry| entry.id == id)
    }

    fn visible_media(&self) -> impl Iterator<Item = &MediaEntry> {
        self.media
            .iter()
            .filter(|entry| !self.project.hidden.contains(&entry.id))
    }

    fn seek(&mut self, time: Ticks) {
        let Self {
            preview,
            project,
            media,
            ..
        } = self;
        preview.seek(&project.timeline, &|id| media_info(media, id), time);
        if self.project.timeline.is_empty() {
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
        preview.toggle(&project.timeline, &|id| media_info(media, id));
    }

    /// Remembers the project as it is now, ready for a change that can be undone.
    fn record(&mut self) {
        self.history.record(self.project.clone());
    }

    /// Adds videos to the project. They wait in the panel until put on the timeline.
    pub fn import(&mut self, ctx: &egui::Context, paths: &[PathBuf]) {
        for path in paths {
            match self.load(ctx, path) {
                Ok(entry) => self.media.push(entry),
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

        let id = MediaId(self.next_media);
        self.next_media += 1;
        Ok(MediaEntry {
            id,
            info,
            name,
            index: None,
            scan: Some(scan),
            cancel_scan,
            progress,
            thumbnail: None,
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
        self.selected = Some(self.project.timeline.push(id, range));
        self.after_clip_added();
    }

    /// Adds a whole video where it was dropped.
    fn drop_on_timeline(&mut self, id: MediaId, time: Ticks) {
        let Some(range) = self.full_range(id) else {
            return;
        };
        self.record();
        self.selected = Some(self.project.timeline.place(id, range, time));
        self.after_clip_added();
    }

    fn after_clip_added(&mut self) {
        self.selected_gap = None;
        self.timeline_state
            .content_added(self.project.timeline.duration());
        let position = self.position();
        self.seek(position);
    }

    /// Asks before removing a video that clips are using, since they go with it.
    fn request_remove(&mut self, id: MediaId) {
        if self.project.timeline.clips_using(id) > 0 {
            self.confirm_remove = Some(id);
        } else {
            self.remove_media(id);
        }
    }

    fn remove_media(&mut self, id: MediaId) {
        self.record();
        self.project.timeline.remove_media(id);
        self.project.hidden.push(id);
        self.after_timeline_change();
    }

    /// Tidies up after clips changed, so nothing points at a clip that has gone.
    fn after_timeline_change(&mut self) {
        if let Some(selected) = self.selected
            && self.project.timeline.index_of(selected).is_none()
        {
            self.selected = None;
        }
        if let Some(gap) = self.selected_gap
            && self.project.timeline.gap_at(gap).is_none()
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

    /// Splits the clip under the playhead. The frame on screen starts the second part.
    fn split_at_playhead(&mut self) {
        if self.project.timeline.is_empty() {
            self.notice = Some("Add a video to the timeline first.".to_owned());
            return;
        }
        let Some(location) = self.project.timeline.locate(self.position()) else {
            self.notice = Some("Move the playhead inside a clip to split it there.".to_owned());
            return;
        };
        self.preview.pause(&self.project.timeline);
        let split_at = self.snap_to_frame(location.clip.media, location.source_time);
        let before = self.project.clone();
        if let Some(second) = self.project.timeline.split(location.clip.id, split_at) {
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
            self.project.timeline.remove(selected);
            self.after_timeline_change();
        } else if let Some(gap) = self.selected_gap {
            self.remove_gap(gap);
        }
    }

    fn remove_gap(&mut self, time: Ticks) {
        let before = self.project.clone();
        if self.project.timeline.remove_gap(time) {
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

    fn step_frame(&mut self, forward: bool) {
        self.preview.pause(&self.project.timeline);
        let position = self.position();
        let gap_step = Ticks::from_seconds(if forward {
            GAP_STEP_SECONDS
        } else {
            -GAP_STEP_SECONDS
        });
        let Some(location) = self.project.timeline.locate(position) else {
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

        let Self {
            preview,
            project,
            media,
            picture,
            notice,
            timeline_state,
            ..
        } = self;
        let frame = preview.update(&project.timeline, &|id| media_info(media, id));
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
            let position = preview.position(&project.timeline);
            timeline_state.follow(position, project.timeline.duration());
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

    /// The timeline as parts to export, once every video on it has been read.
    fn export_parts(&self) -> Option<Vec<ExportPart>> {
        let mut previous_end = Ticks(0);
        self.project
            .timeline
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
            .timeline
            .clips()
            .iter()
            .filter_map(|clip| self.entry(clip.media).map(|entry| &entry.info));
        let first = infos.next()?;
        infos.find_map(|other| join_problem(first, other))
    }

    /// Why dead space can't be exported for the videos on the timeline, if it can't.
    fn gap_problem(&self) -> Option<String> {
        if self.project.timeline.gaps().is_empty() {
            return None;
        }
        let first = self
            .project
            .timeline
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
            .timeline
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
        let blocked =
            !matches!(self.export_state, ExportState::Closed) || self.confirm_remove.is_some();
        if blocked {
            return;
        }
        let consume = |modifiers, key| ctx.input_mut(|input| input.consume_key(modifiers, key));

        if consume(Modifiers::COMMAND, Key::O) {
            self.pick_files(ctx);
        }
        if consume(Modifiers::COMMAND, Key::E) && !self.project.timeline.is_empty() {
            self.export_state = ExportState::Choosing;
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
            let target = Ticks(self.position().0 - Ticks::from_seconds(SHIFT_STEP_SECONDS).0);
            self.seek(target);
        }
        if consume(Modifiers::SHIFT, Key::ArrowRight) {
            let target = Ticks(self.position().0 + Ticks::from_seconds(SHIFT_STEP_SECONDS).0);
            self.seek(target);
        }
        if consume(Modifiers::NONE, Key::ArrowLeft) {
            self.step_frame(false);
        }
        if consume(Modifiers::NONE, Key::ArrowRight) {
            self.step_frame(true);
        }
        if consume(Modifiers::NONE, Key::Home) {
            self.seek(Ticks(0));
        }
        if consume(Modifiers::NONE, Key::End) {
            let end = self.project.timeline.duration();
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
        if self.logo.is_none() {
            let icon = eframe::icon_data::from_png_bytes(LOGO_DARK).ok()?;
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            );
            self.logo = Some(ctx.load_texture("logo", image, TextureOptions::LINEAR));
        }
        self.logo.clone()
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let palette = theme::palette(&ctx);

        self.poll_background(&ctx);
        self.handle_shortcuts(&ctx);
        self.take_dropped_files(&ctx);

        self.top_bar(ui, palette);
        self.rail(ui, palette);
        self.media_panel(ui, palette);
        self.timeline_panel(ui, palette);
        self.transport_panel(ui, palette);
        self.stage(ui, palette);
        self.remove_dialog(&ctx, palette);
        self.export_dialog(&ctx, palette);
        drop_overlay(&ctx, palette);
    }

    fn top_bar(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let frame = Frame::new()
            .fill(palette.surface)
            .inner_margin(Margin::symmetric(14, 8));
        egui::Panel::top("top_bar")
            .exact_size(48.0)
            .frame(frame)
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = CONTROL_SPACING;
                    ui.label(
                        RichText::new("BareClip")
                            .font(fonts::strong(15.0))
                            .color(palette.accent),
                    );
                    components::divider(ui, palette);
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
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let can_export = !self.project.timeline.is_empty();
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
                            self.export_state = ExportState::Choosing;
                        }
                        let icon = theme::current_icon(ui.ctx());
                        let theme_button = ui
                            .add(IconButton::new(palette, Kind::Ghost, icon, "Theme"))
                            .on_hover_text("Theme");
                        egui::Popup::menu(&theme_button).show(|ui| {
                            theme::theme_picker(ui, palette);
                        });
                    });
                });
            });
    }

    /// The list of tools down the left. It only ever shows what works today.
    fn rail(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let frame = Frame::new()
            .fill(palette.bg_app)
            .inner_margin(Margin::symmetric(8, 10));
        egui::Panel::left("rail")
            .exact_size(RAIL_WIDTH)
            .frame(frame)
            .show(ui, |ui| {
                let (rect, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 58.0), Sense::hover());
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 6.0, palette.raised);
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        rect.left_top() + vec2(0.0, 12.0),
                        vec2(3.0, rect.height() - 24.0),
                    ),
                    1.0,
                    palette.accent,
                );
                icons::paint(
                    &painter,
                    icons::square_at(rect.center() - vec2(0.0, 9.0), 20.0),
                    &icons::FILM,
                    palette.accent,
                );
                painter.text(
                    rect.center() + vec2(0.0, 14.0),
                    egui::Align2::CENTER_CENTER,
                    "My media",
                    FontId::new(11.0, FontFamily::Proportional),
                    palette.text,
                );
            });
    }

    fn media_panel(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let frame = Frame::new()
            .fill(palette.surface)
            .inner_margin(Margin::same(14));
        let panel = egui::Panel::left("media_panel")
            .resizable(true)
            .default_size(MEDIA_PANEL_WIDTH)
            .size_range(MEDIA_PANEL_MIN_WIDTH..=MEDIA_PANEL_MAX_WIDTH)
            .frame(frame)
            .show(ui, |ui| {
                ui.label(RichText::new("My media").font(fonts::strong(15.0)));
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

                let mut add = None;
                let mut remove = None;
                let mut thumbnails = Vec::new();
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        let columns = media_columns(ui.available_width());
                        let entries: Vec<&MediaEntry> = self.visible_media().collect();
                        for row in entries.chunks(columns) {
                            ui.columns(columns, |cells| {
                                for (cell, entry) in cells.iter_mut().zip(row) {
                                    let thumbnail =
                                        media_card(cell, palette, entry, &mut add, &mut remove);
                                    thumbnails.push((entry.id, thumbnail));
                                }
                            });
                            ui.add_space(MEDIA_CARD_SPACING);
                        }
                    });
                self.card_thumbnails = thumbnails;
                if let Some(id) = add {
                    self.add_to_timeline(id);
                }
                if let Some(id) = remove {
                    self.request_remove(id);
                }
            });
        self.media_panel_rect = panel.response.rect;
    }

    fn timeline_panel(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let frame = Frame::new()
            .fill(palette.surface)
            .inner_margin(Margin::symmetric(8, 8));
        let tallest = (ui.available_height() * TIMELINE_MAX_SHARE).max(TIMELINE_MIN_HEIGHT);
        let panel = egui::Panel::bottom("timeline")
            .resizable(true)
            .default_size(TIMELINE_HEIGHT)
            .size_range(TIMELINE_MIN_HEIGHT..=tallest)
            .frame(frame)
            .show(ui, |ui| {
                self.timeline_tools(ui, palette);
                ui.add_space(4.0);
                self.timeline_track(ui, palette);
            });
        self.timeline_panel_rect = panel.response.rect;
    }

    fn timeline_tools(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let has_clips = !self.project.timeline.is_empty();
        let can_delete = self.selected.is_some() || self.selected_gap.is_some();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = CONTROL_SPACING;
            ui.add_space(8.0);
            let split = ActionButton::new(palette, Kind::Ghost, "Split")
                .icon(icons::SCISSORS)
                .small();
            if ui
                .add_enabled(has_clips, split)
                .on_hover_text("Split the clip at the playhead (S)")
                .clicked()
            {
                self.split_at_playhead();
            }
            let delete = ActionButton::new(palette, Kind::Ghost, "Delete")
                .icon(icons::TRASH_2)
                .small();
            if ui
                .add_enabled(can_delete, delete)
                .on_hover_text("Delete the selected clip or dead space (Delete)")
                .clicked()
            {
                self.delete_selected();
            }
        });
    }

    fn timeline_track(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let clips: Vec<ClipView> = self
            .project
            .timeline
            .clips()
            .iter()
            .map(|clip| ClipView {
                id: clip.id,
                start: clip.start,
                end: clip.end(),
                label: self
                    .entry(clip.media)
                    .map_or_else(String::new, |entry| entry.name.clone()),
            })
            .collect();
        let view = TimelineView {
            timeline: &self.project.timeline,
            clips: &clips,
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
            && let Some(clip) = self.project.timeline.clip(id)
        {
            self.preview.pause(&self.project.timeline);
            self.record();
            let source = match edge {
                ClipEdge::Start => clip.source.start,
                ClipEdge::End => clip.source.end,
            };
            self.trim_origin = Some(TrimOrigin { source, pointer });
        }
        if let (Some((id, edge, pointer)), Some(origin)) = (events.trim_to, self.trim_origin) {
            self.trim_clip(id, edge, origin, pointer);
        }
        if events.trim_finished {
            self.trim_origin = None;
        }
        if let Some((id, time)) = events.move_clip {
            let before = self.project.clone();
            self.project.timeline.move_clip(id, time);
            if self.project != before {
                self.history.record(before);
                self.after_timeline_change();
            }
        }
        if let Some(gap) = events.remove_gap {
            self.remove_gap(gap);
        }
        if let Some((media, time)) = events.drop_media {
            self.drop_on_timeline(media, time);
        }
        if let Some(seek) = events.seek {
            self.seek(seek);
        }
    }

    /// Moves a clip's edge by however far the pointer has travelled, and shows the
    /// frame at the new cut so its effect can be judged.
    fn trim_clip(&mut self, id: ClipId, edge: ClipEdge, origin: TrimOrigin, pointer: Ticks) {
        let Some(clip) = self.project.timeline.clip(id) else {
            return;
        };
        let Some(duration) = self.entry(clip.media).map(|entry| entry.info.duration) else {
            return;
        };
        let wanted = Ticks(origin.source.0 + pointer.0 - origin.pointer.0);
        let snapped = self.snap_to_frame(clip.media, wanted);
        self.project.timeline.trim(
            id,
            edge,
            snapped,
            duration,
            Ticks::from_seconds(MIN_CLIP_SECONDS),
        );

        let Some(clip) = self.project.timeline.clip(id) else {
            return;
        };
        let shown = match edge {
            ClipEdge::Start => clip.start,
            ClipEdge::End => Ticks(clip.end().0 - Ticks::from_seconds(JUST_BEFORE_END_SECONDS).0),
        };
        self.seek(shown);
    }

    fn transport_panel(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let frame = Frame::new()
            .fill(palette.surface)
            .inner_margin(Margin::symmetric(14, 8));
        egui::Panel::bottom("transport")
            .exact_size(52.0)
            .frame(frame)
            .show(ui, |ui| {
                let has_clips = !self.project.timeline.is_empty();
                let position = self.position();
                let total = self.project.timeline.duration();
                let playing = self.preview.is_playing();
                ui.columns_const(|[left, middle, right]: &mut [egui::Ui; 3]| {
                    left.horizontal_centered(|ui| {
                        ui.label(
                            RichText::new(format_timecode(position)).font(fonts::strong(15.0)),
                        );
                        ui.colored_label(
                            palette.text_muted,
                            format!("/ {}", format_timecode(total)),
                        );
                    });
                    middle.with_layout(
                        Layout::left_to_right(Align::Center).with_main_justify(false),
                        |ui| {
                            ui.spacing_mut().item_spacing.x = CONTROL_SPACING;
                            let spare = (ui.available_width() - TRANSPORT_WIDTH).max(0.0);
                            ui.add_space(spare / 2.0);
                            let quiet =
                                |icon, name| IconButton::new(palette, Kind::Ghost, icon, name);
                            if ui
                                .add_enabled(has_clips, quiet(icons::SKIP_BACK, "Go to start"))
                                .on_hover_text("Go to the start (Home)")
                                .clicked()
                            {
                                self.seek(Ticks(0));
                            }
                            if ui
                                .add_enabled(
                                    has_clips,
                                    quiet(icons::CHEVRON_LEFT, "One frame back"),
                                )
                                .on_hover_text("One frame back (Left arrow)")
                                .clicked()
                            {
                                self.step_frame(false);
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
                            if ui
                                .add_enabled(
                                    has_clips,
                                    quiet(icons::CHEVRON_RIGHT, "One frame forward"),
                                )
                                .on_hover_text("One frame forward (Right arrow)")
                                .clicked()
                            {
                                self.step_frame(true);
                            }
                            if ui
                                .add_enabled(has_clips, quiet(icons::SKIP_FORWARD, "Go to end"))
                                .on_hover_text("Go to the end (End)")
                                .clicked()
                            {
                                self.seek(total);
                            }
                        },
                    );
                    right.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if !self.preview.sound_works() {
                            ui.colored_label(
                                palette.text_muted,
                                "Preview is silent on this computer",
                            );
                        }
                    });
                });
            });
    }

    fn stage(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let frame = Frame::new().fill(palette.stage);
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            let rect = ui.available_rect_before_wrap();
            if self.preview.in_gap(&self.project.timeline) {
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
            if !self.project.timeline.is_empty() {
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
                            .color(Color32::from_rgb(0xF1, 0xEE, 0xE9)),
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
                let first = self.project.timeline.clips().first()?;
                let video = &self.entry(first.media)?.info.video;
                Some(vec2(video.width as f32, video.height as f32))
            })
    }

    fn remove_dialog(&mut self, ctx: &egui::Context, palette: &Palette) {
        let Some(id) = self.confirm_remove else {
            return;
        };
        let count = self.project.timeline.clips_using(id);
        let name = self
            .entry(id)
            .map_or_else(String::new, |entry| entry.name.clone());
        let mut confirmed = false;
        let mut cancelled = false;
        egui::Modal::new(egui::Id::unique("remove_dialog"))
            .frame(dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width(400.0);
                components::dialog_title(
                    ui,
                    &icons::CIRCLE_ALERT,
                    palette.danger,
                    "Remove this video?",
                );
                ui.add_space(12.0);
                ui.label(text::remove_warning(&name, count));
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
            self.confirm_remove = None;
            self.remove_media(id);
        } else if cancelled {
            self.confirm_remove = None;
        }
    }

    fn export_dialog(&mut self, ctx: &egui::Context, palette: &Palette) {
        if matches!(self.export_state, ExportState::Closed) {
            return;
        }
        let mut next_state = None;
        let mut begin_export = false;
        let problem = self.export_problem();
        let gap_problem = self.gap_problem();
        let ready = self.export_parts().is_some();
        let summary = self.plan_description();

        egui::Modal::new(egui::Id::unique("export_dialog")).frame(dialog_frame(ctx)).show(ctx, |ui| {
            ui.set_width(440.0);
            match &self.export_state {
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
        for clip in self.project.timeline.clips() {
            let entry = self.entry(clip.media)?;
            let index = entry.index.as_ref()?;
            let plan = plan_export(index, &[clip.source], entry.info.video.time_base, self.mode);
            copied += plan.copied_frames;
            encoded += plan.encoded_frames;
        }
        Some(text::plan_summary(copied, encoded))
    }
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

/// How many media cards fit across the panel.
fn media_columns(width: f32) -> usize {
    let per_card = MEDIA_CARD_MIN_WIDTH + MEDIA_CARD_SPACING;
    (((width + MEDIA_CARD_SPACING) / per_card).floor() as usize).max(1)
}

/// Draws one video in the panel, and returns where its thumbnail is.
fn media_card(
    ui: &mut egui::Ui,
    palette: &Palette,
    entry: &MediaEntry,
    add: &mut Option<MediaId>,
    remove: &mut Option<MediaId>,
) -> egui::Rect {
    let mut thumbnail_rect = egui::Rect::NOTHING;
    let card = Frame::new()
        .fill(palette.raised)
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .inner_margin(Margin::same(8))
        .stroke(Stroke::new(1.0, palette.border))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 2.0;
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
                // The stage stays dark in both themes, so this label reads the same in both.
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
            .on_hover_text("Drag onto the timeline");
            ui.add_space(6.0);
            ui.add(egui::Label::new(RichText::new(&entry.name).strong()).truncate());
            let detail = |ui: &mut egui::Ui, line: String, color| {
                ui.label(RichText::new(line).size(12.0).color(color));
            };
            let details = if entry.scan.is_some() {
                let fraction = f32::from_bits(entry.progress.load(Ordering::Relaxed)) * 100.0;
                format!("Reading frames {fraction:.0}%")
            } else {
                text::file_size(entry.info.size_bytes)
            };
            detail(ui, details, palette.text_muted);
            detail(ui, text::video_summary(&entry.info), palette.text_muted);
            for (position, track) in entry.info.audio.iter().enumerate() {
                detail(
                    ui,
                    text::audio_track_summary(position, track),
                    palette.text_muted,
                );
            }
            if entry.info.video.is_hdr {
                detail(
                    ui,
                    "HDR video: the preview shows it without HDR.".to_owned(),
                    palette.danger,
                );
            }
        });

    let hovered = ui.rect_contains_pointer(card.response.rect);
    if hovered {
        ui.painter().rect_stroke(
            card.response.rect,
            CARD_RADIUS,
            Stroke::new(1.0, palette.border_strong),
            egui::StrokeKind::Inside,
        );
    }
    let margin = 6.0;
    let size = vec2(CARD_BUTTON_SIZE, CARD_BUTTON_SIZE);
    let add_rect = egui::Rect::from_min_size(
        thumbnail_rect.right_bottom() - size - vec2(margin, margin),
        size,
    );
    let remove_rect = egui::Rect::from_min_size(
        thumbnail_rect.right_top() + vec2(-CARD_BUTTON_SIZE - margin, margin),
        size,
    );
    let added = IconButton::new(palette, Kind::Primary, icons::PLUS, "Add to timeline")
        .size(CARD_BUTTON_SIZE, 16.0)
        .hidden(!hovered)
        .show_at(
            ui,
            egui::Id::unique(("media-card-add", entry.id.0)),
            add_rect,
        )
        .on_hover_text("Add to timeline");
    if added.clicked() {
        *add = Some(entry.id);
    }
    let removed = IconButton::new(
        palette,
        Kind::Secondary,
        icons::X,
        "Remove from this project",
    )
    .size(CARD_BUTTON_SIZE, 16.0)
    .hidden(!hovered)
    .show_at(
        ui,
        egui::Id::unique(("media-card-remove", entry.id.0)),
        remove_rect,
    )
    .on_hover_text("Remove from this project");
    if removed.clicked() {
        *remove = Some(entry.id);
    }
    thumbnail_rect
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
        let theme_button = named_rect(&ctx, &mut editor, "Theme");
        click(&ctx, &mut editor, theme_button.center());
        for _ in 0..12 {
            run_frame(&ctx, &mut editor, moved(theme_button.center()));
        }
        camera.shoot(
            &ctx,
            &mut editor,
            moved(theme_button.center()),
            "16-theme-menu",
        );
        click(&ctx, &mut editor, theme_button.center());
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
            editor.confirm_remove = editor.media.first().map(|entry| entry.id);
            shoot_dialog(camera, editor, format!("{prefix}-remove-dialog"));
            editor.confirm_remove = None;
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
        editor.project.timeline.clips().len()
    }

    #[test]
    fn the_empty_screen_draws_without_a_video() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);

        for _ in 0..2 {
            run_frame(&ctx, &mut editor, vec![]);
        }

        assert!(editor.project.timeline.is_empty());
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

            let clip = editor.project.timeline.clips()[index];
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
        let clip = editor.project.timeline.clips()[0];
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
        assert_eq!(editor.selected, Some(editor.project.timeline.clips()[1].id));
    }

    #[test]
    fn dropping_a_video_past_the_end_leaves_dead_space_before_it() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.import(&ctx, &[second_clip().to_path_buf()]);
        let second = editor.media[1].id;

        editor.drop_on_timeline(second, Ticks::from_seconds(6.0));

        let clip = editor.project.timeline.clips()[1];
        assert_eq!(clip.media, second);
        assert_eq!(clip.start, Ticks::from_seconds(6.0));
        assert_eq!(editor.project.timeline.gaps().len(), 1);
    }

    #[test]
    fn dropping_a_video_on_a_clip_pushes_later_clips_along() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip(), test_clip()]);
        let id = editor.media[0].id;

        editor.drop_on_timeline(id, Ticks::from_seconds(1.0));

        let starts: Vec<f64> = editor
            .project
            .timeline
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
        let clips = editor.project.timeline.clips();
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
        let first = editor.project.timeline.clips()[0].id;
        let second_start = editor.project.timeline.clips()[1].start;
        editor.selected = Some(first);

        run_frame(&ctx, &mut editor, press(Key::Delete));

        assert_eq!(clip_count(&editor), 1);
        assert!(editor.selected.is_none());
        assert_eq!(editor.project.timeline.clips()[0].start, second_start);
        assert_eq!(editor.project.timeline.gaps().len(), 1);
    }

    /// One clip with its first second trimmed away, and the playhead at the start.
    fn with_leading_gap(ctx: &egui::Context) -> Editor {
        let mut editor = with_videos(ctx, &[test_clip()]);
        let id = editor.project.timeline.clips()[0].id;
        let duration = editor.media[0].info.duration;
        editor.project.timeline.trim(
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

        assert!(editor.project.timeline.gaps().is_empty());
        assert_eq!(editor.project.timeline.clips()[0].start, Ticks(0));
        assert!(editor.selected_gap.is_none());

        editor.undo();
        assert_eq!(editor.project.timeline.gaps().len(), 1);
    }

    #[test]
    fn every_button_that_is_only_a_symbol_has_a_name() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);

        let nodes = named_nodes(&ctx, &mut editor);

        for name in [
            "Undo",
            "Redo",
            "Theme",
            "Go to start",
            "One frame back",
            "Play",
            "One frame forward",
            "Go to end",
            "Zoom out",
            "Zoom in",
            "Fit",
            "Add to timeline",
            "Remove from this project",
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
            "One frame back",
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
        let total = editor.project.timeline.duration();

        click_named(&ctx, &mut editor, "Go to end");
        assert_eq!(editor.position(), total);

        click_named(&ctx, &mut editor, "Go to start");
        assert_eq!(editor.position(), Ticks(0));
    }

    #[test]
    fn the_frame_buttons_step_one_frame_each_way() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);

        click_named(&ctx, &mut editor, "One frame forward");
        let stepped = editor.position();
        assert!(stepped > Ticks(0));

        click_named(&ctx, &mut editor, "One frame back");
        assert!(editor.position() < stepped);
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
        click_named(&ctx, &mut editor, "One frame forward");

        click_named(&ctx, &mut editor, "Split");
        assert_eq!(editor.project.timeline.clips().len(), 2);

        click_named(&ctx, &mut editor, "Undo");
        assert_eq!(editor.project.timeline.clips().len(), 1);

        click_named(&ctx, &mut editor, "Redo");
        assert_eq!(editor.project.timeline.clips().len(), 2);
    }

    #[test]
    fn the_theme_menu_changes_the_theme() {
        let ctx = context();
        let mut editor = Editor::new(&ctx, false);

        click_named(&ctx, &mut editor, "Theme");
        run_frame(&ctx, &mut editor, vec![]);
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

        editor.confirm_remove = Some(id);
        run_frame(&ctx, &mut editor, vec![]);
        click_named(&ctx, &mut editor, "Cancel");
        assert_eq!(editor.confirm_remove, None);
        assert_eq!(editor.visible_media().count(), 1);

        editor.confirm_remove = Some(id);
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
            editor.project.timeline.gaps().is_empty(),
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

        assert!(editor.preview.in_gap(&editor.project.timeline));

        editor.seek(Ticks::from_seconds(2.0));
        assert!(!editor.preview.in_gap(&editor.project.timeline));
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
    fn arrow_keys_can_step_through_dead_space() {
        let ctx = context();
        let mut editor = with_leading_gap(&ctx);
        editor.seek(Ticks::from_seconds(0.5));

        run_frame(&ctx, &mut editor, press(Key::ArrowRight));
        assert!(editor.position() > Ticks::from_seconds(0.5));

        run_frame(&ctx, &mut editor, press(Key::ArrowLeft));
        assert!((editor.position().as_seconds() - 0.5).abs() < 0.001);
    }

    #[test]
    fn arrow_keys_can_step_back_from_a_clip_into_dead_space() {
        let ctx = context();
        let mut editor = in_panel_only(&ctx, &[test_clip()]);
        let id = editor.media[0].id;
        editor.drop_on_timeline(id, Ticks::from_seconds(1.0));
        editor.seek(Ticks::from_seconds(1.0));

        run_frame(&ctx, &mut editor, press(Key::ArrowLeft));

        assert!(editor.position() < Ticks::from_seconds(1.0));
        assert!(editor.preview.in_gap(&editor.project.timeline));
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
        let end = scale.x_of(editor.project.timeline.duration());

        assert!(end < editor.timeline_state.track.right() - 50.0);
    }

    #[test]
    fn the_zoom_buttons_change_how_much_is_shown() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let fitted = editor.timeline_state.scale.visible;
        let [zoom_out, zoom_in, fit] = editor.timeline_state.zoom_buttons;

        click(&ctx, &mut editor, zoom_out.center());
        assert!(editor.timeline_state.scale.visible > fitted);

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

    #[test]
    fn cards_stack_or_sit_side_by_side_by_width() {
        assert_eq!(media_columns(150.0), 1);
        assert_eq!(media_columns(MEDIA_CARD_MIN_WIDTH), 1);
        assert_eq!(
            media_columns(MEDIA_CARD_MIN_WIDTH * 2.0 + MEDIA_CARD_SPACING),
            2
        );
        assert_eq!(
            media_columns(MEDIA_CARD_MIN_WIDTH * 3.0 + MEDIA_CARD_SPACING * 2.0),
            3
        );
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

        assert_eq!(editor.confirm_remove, Some(id));
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

        assert_eq!(editor.confirm_remove, None);
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
        editor.selected = Some(editor.project.timeline.clips()[0].id);

        editor.delete_selected();

        assert!(editor.project.timeline.is_empty());
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
        let end_x = scale.x_of(editor.project.timeline.duration()) - 4.0;
        let target_x = scale.x_of(Ticks::from_seconds(2.0));

        pointer_drag(&ctx, &mut editor, pos2(end_x, y), pos2(target_x, y));

        let clip = editor.project.timeline.clips()[0];
        assert!(
            (clip.source.end.as_seconds() - 2.0).abs() < 0.1,
            "the clip now ends at {}",
            clip.source.end.as_seconds()
        );
        assert_eq!(clip.source.start, Ticks(0));
        assert!(editor.project.timeline.gaps().is_empty());

        editor.undo();
        assert!(editor.project.timeline.clips()[0].source.end.as_seconds() > 3.9);
    }

    #[test]
    fn one_trim_drag_is_one_undo_step() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        let before = editor.project.clone();
        let scale = editor.timeline_state.scale;
        let y = editor.timeline_state.track.center().y;
        let end_x = scale.x_of(editor.project.timeline.duration()) - 4.0;

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
        let end_x = scale.x_of(editor.project.timeline.duration()) - 4.0;

        pointer_drag(
            &ctx,
            &mut editor,
            pos2(end_x, y),
            pos2(scale.x_of(Ticks::from_seconds(2.013)), y),
        );

        let frames = editor.project.timeline.clips()[0].source.end.as_seconds() * 30.0;
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

        let clip = editor.project.timeline.clips()[0];
        assert!(
            (clip.source.start.as_seconds() - 1.0).abs() < 0.1,
            "the clip now starts at {}",
            clip.source.start.as_seconds()
        );
        assert!(clip.source.end.as_seconds() > 3.9);

        // The footage it keeps stays where it was, with dead space in front of it.
        assert_eq!(clip.start, clip.source.start);
        assert!((editor.project.timeline.duration().as_seconds() - 4.0).abs() < 0.001);
        assert_eq!(editor.project.timeline.gaps().len(), 1);
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

        let clip = editor.project.timeline.clips()[0];
        assert!((clip.start.as_seconds() - 0.5).abs() < 0.1, "{clip:?}");
        assert_eq!(editor.project.timeline.gaps().len(), 1);

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

        assert_eq!(editor.project.timeline.clips()[0].start, Ticks(0));
        assert!(editor.project.timeline.gaps().is_empty());
    }

    #[test]
    fn dragging_a_clip_past_another_changes_the_order() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip(), second_clip()]);
        run_frame(&ctx, &mut editor, vec![]);
        let first = editor.project.timeline.clips()[0].id;
        let scale = editor.timeline_state.scale;
        let y = editor.timeline_state.track.center().y;

        pointer_drag(
            &ctx,
            &mut editor,
            pos2(scale.x_of(Ticks::from_seconds(2.0)), y),
            pos2(scale.x_of(Ticks::from_seconds(7.9)), y),
        );

        assert_eq!(editor.project.timeline.clips()[1].id, first);

        assert_eq!(editor.project.timeline.gaps().len(), 1);

        editor.undo();
        assert_eq!(editor.project.timeline.clips()[0].id, first);
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
    fn arrow_keys_step_one_frame_exactly() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        editor.seek(Ticks::from_seconds(1.0));

        run_frame(&ctx, &mut editor, press(Key::ArrowRight));
        let gap = editor.position().as_seconds() - 1.0;
        assert!((gap - 1.0 / 30.0).abs() < 0.002, "stepped {gap}s");

        run_frame(&ctx, &mut editor, press(Key::ArrowLeft));
        assert!((editor.position().as_seconds() - 1.0).abs() < 0.002);
    }

    #[test]
    fn the_dialog_describes_what_the_export_will_do() {
        let ctx = context();
        let mut editor = with_videos(&ctx, &[test_clip()]);
        let id = editor.project.timeline.clips()[0].id;
        let duration = editor.media[0].info.duration;
        let min = Ticks::from_seconds(MIN_CLIP_SECONDS);
        editor
            .project
            .timeline
            .trim(id, ClipEdge::Start, Ticks::from_seconds(0.5), duration, min);
        editor
            .project
            .timeline
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
