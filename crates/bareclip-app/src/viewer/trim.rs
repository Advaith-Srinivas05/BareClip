//! The quick trimmer: two handles over a strip of the whole file, which keep one stretch, and
//! saving that stretch as a new file beside the original or wherever the Save window says.
//! Videos are cut the editor's frame-exact way, copying all but the stretches around the cuts,
//! and sound is cut by `cut_sound`.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
};

use bareclip_core::{
    splice::{CutMode, FrameIndex},
    time::{Ticks, format_timecode},
    timeline::TimeRange,
};
use bareclip_media::{ExportPart, ExportRequest, SoundShape, StripPicture};
use eframe::egui::{
    self, Align, Color32, Layout, Rect, RichText, Sense, Stroke, TextureHandle, TextureOptions,
    UiBuilder, pos2, vec2,
};

use super::Viewer;
use crate::{
    components::{self, ActionButton, Kind},
    icons, text,
    theme::Palette,
};

/// The kinds of video a trim is copied into as they are. Others come out as an MP4, redrawn.
const COPIED_CONTAINERS: [&str; 4] = ["mp4", "m4v", "mov", "mkv"];
/// The most pictures along the strip.
const STRIP_PICTURES: u32 = 48;
pub const STRIP_HEIGHT: f32 = 46.0;
const HANDLE_WIDTH: f32 = 10.0;
/// The trimmer's rows: the strip with its times, then its buttons.
pub const TRIM_ROW_HEIGHT: f32 = STRIP_HEIGHT + 30.0;
pub const ACTION_ROW_HEIGHT: f32 = 40.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Handle {
    Start,
    End,
}

/// A trim being saved in the background.
struct Saving {
    receiver: Receiver<Result<(PathBuf, Vec<String>), String>>,
    /// Thousandths done.
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
}

pub struct Trimmer {
    /// The kept stretch, in the file's own time.
    pub start: Ticks,
    pub end: Ticks,
    held: Option<Handle>,
    pictures: Vec<(Ticks, TextureHandle)>,
    pub(super) arriving: Option<Receiver<StripPicture>>,
    shape: Option<SoundShape>,
    pub(super) shape_arriving: Option<Receiver<Option<SoundShape>>>,
    /// Stops the strip's readers when the trimmer closes.
    cancel: Arc<AtomicBool>,
    saving: Option<Saving>,
    /// The file last saved, and what the save did.
    pub saved: Option<(PathBuf, Vec<String>)>,
    pub problem: Option<String>,
    /// Save… is waiting for an answer about becoming an MP4.
    asking_mp4: bool,
}

impl Drop for Trimmer {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Whether a trim of this file is saved as the same kind of file.
pub fn keeps_its_kind(path: &Path, sound: bool) -> bool {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    sound || COPIED_CONTAINERS.contains(&extension.as_str())
}

/// Where Save as copy puts a trim: beside the original, as `<name> (trimmed).<ext>`, then
/// `(trimmed 2)` and so on, as an MP4 if the kind can't be kept.
pub fn copy_name(path: &Path, sound: bool) -> PathBuf {
    let stem = path
        .file_stem()
        .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
    let extension = if keeps_its_kind(path, sound) {
        path.extension().map_or_else(
            || "mp4".to_owned(),
            |extension| extension.to_string_lossy().into_owned(),
        )
    } else {
        "mp4".to_owned()
    };
    let mut number = 1;
    loop {
        let label = if number == 1 {
            "trimmed".to_owned()
        } else {
            format!("trimmed {number}")
        };
        let candidate = path.with_file_name(format!("{stem} ({label}).{extension}"));
        if !candidate.exists() {
            return candidate;
        }
        number += 1;
    }
}

impl Viewer {
    #[cfg(test)]
    pub fn is_trimming(&self) -> bool {
        self.trim.is_some()
    }

    /// Opens the trimmer, keeping the whole file, and starts reading its strip.
    pub fn open_trimmer(&mut self, ctx: &egui::Context) {
        let Some(file) = &self.file else {
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let path = file.info.path.clone();
        let duration = file.info.duration;
        let mut trimmer = Trimmer {
            start: Ticks(0),
            end: duration,
            held: None,
            pictures: Vec::new(),
            arriving: None,
            shape: None,
            shape_arriving: None,
            cancel: cancel.clone(),
            saving: None,
            saved: None,
            problem: None,
            asking_mp4: false,
        };
        let repaint = ctx.clone();
        if file.is_sound() {
            let (sender, receiver) = mpsc::channel();
            thread::spawn(move || {
                let shape = bareclip_media::read_sound_shape(&path, &cancel, |_| {})
                    .ok()
                    .flatten();
                let _ = sender.send(shape);
                repaint.request_repaint();
            });
            trimmer.shape_arriving = Some(receiver);
        } else {
            let (sender, receiver) = mpsc::channel();
            let spacing = bareclip_media::picture_spacing(
                duration,
                Ticks::from_seconds(0.25),
                STRIP_PICTURES,
            );
            thread::spawn(move || {
                let _ = bareclip_media::read_strip_pictures(
                    &path,
                    duration,
                    spacing,
                    (160, 90),
                    &cancel,
                    |picture| {
                        let _ = sender.send(picture);
                        repaint.request_repaint();
                    },
                );
            });
            trimmer.arriving = Some(receiver);
        }
        self.trim = Some(trimmer);
    }

    /// Leaves the trimmer and forgets the handles.
    pub fn close_trimmer(&mut self) {
        if let Some(trimmer) = &self.trim
            && let Some(saving) = &trimmer.saving
        {
            saving.cancel.store(true, Ordering::Relaxed);
        }
        self.trim = None;
    }

    /// The length of one frame near a moment, for the handles' smallest gap.
    fn frame_length(&self) -> Ticks {
        let rate = self
            .file
            .as_ref()
            .map(|file| file.info.video.frame_rate)
            .filter(|rate| *rate > 0.0)
            .unwrap_or(1000.0);
        Ticks::from_seconds(1.0 / rate)
    }

    /// The start of the frame showing at a moment, which is where a start handle lands.
    fn frame_start(&self, time: Ticks) -> Ticks {
        let Some(file) = &self.file else {
            return time;
        };
        let base = file.info.video.time_base;
        file.frames
            .as_ref()
            .and_then(|frames| frames.frame_at(time.to_pts(base)))
            .map_or(time, |pts| Ticks::from_pts(pts, base))
    }

    /// The end of the frame showing at a moment, which is where an end handle lands, so the
    /// frame it shows is the last one kept.
    fn frame_end(&self, time: Ticks) -> Ticks {
        let Some(file) = &self.file else {
            return time;
        };
        let base = file.info.video.time_base;
        let Some(frames) = &file.frames else {
            return time;
        };
        let Some(shown) = frames.frame_at(time.to_pts(base)) else {
            return time;
        };
        frames
            .next_frame_after(shown)
            .map_or(file.info.duration, |next| Ticks::from_pts(next, base))
    }

    /// The start of the last frame kept before an end. Half a frame back, because a moment
    /// a tick before the end can round to the next frame's own time.
    fn last_kept_frame(&self, end: Ticks) -> Ticks {
        self.frame_start(Ticks(end.0 - self.frame_length().0 / 2))
    }

    /// Moves the start handle to the frame showing at a moment, never past the end.
    pub fn set_trim_start(&mut self, time: Ticks) {
        let snapped = self.frame_start(time.max(Ticks(0)));
        let least = self.frame_length();
        if let Some(trimmer) = &mut self.trim {
            trimmer.start = snapped.min(Ticks(trimmer.end.0 - least.0)).max(Ticks(0));
        }
    }

    /// Moves the end handle to just after the frame showing at a moment, never before the
    /// start.
    pub fn set_trim_end(&mut self, time: Ticks) {
        let snapped = self.frame_end(time.min(self.duration()));
        let least = self.frame_length();
        let duration = self.duration();
        if let Some(trimmer) = &mut self.trim {
            trimmer.end = snapped.max(Ticks(trimmer.start.0 + least.0)).min(duration);
        }
    }

    /// The kept stretch.
    pub fn trim_range(&self) -> Option<TimeRange> {
        self.trim.as_ref().map(|trimmer| TimeRange {
            start: trimmer.start,
            end: trimmer.end,
        })
    }

    /// While trimming, play runs between the handles and stops at the end one.
    pub(super) fn keep_within_trim(&mut self) {
        let Some(range) = self.trim_range() else {
            return;
        };
        if self.is_playing() && self.position() >= range.end {
            self.pause();
            self.seek(self.last_kept_frame(range.end));
        }
    }

    /// Play from inside the kept stretch, or from its start.
    pub(super) fn play_in_trim(&mut self) {
        if let Some(range) = self.trim_range() {
            let at = self.position();
            if at < range.start || at >= Ticks(range.end.0 - self.frame_length().0 / 2) {
                self.seek(range.start);
            }
        }
        self.play();
    }

    /// Saves the kept stretch, beside the original or where given, in the background.
    pub fn save_trim(&mut self, ctx: &egui::Context, target: Option<PathBuf>) {
        let Some(file) = &self.file else {
            return;
        };
        let Some(range) = self.trim_range() else {
            return;
        };
        let sound = file.is_sound();
        let source = file.info.path.clone();
        let output = target.unwrap_or_else(|| copy_name(&source, sound));
        if bareclip_media::same_file(&source, &output) {
            if let Some(trimmer) = &mut self.trim {
                trimmer.problem =
                    Some("Choose another name, so the original stays as it is.".to_owned());
            }
            return;
        }
        let frames = file.frames.clone();
        if !sound && frames.is_none() {
            if let Some(trimmer) = &mut self.trim {
                trimmer.problem =
                    Some("Still reading the video. Try again in a moment.".to_owned());
            }
            return;
        }
        let info = file.info.clone();
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();
        let (shown, stop, repaint) = (progress.clone(), cancel.clone(), ctx.clone());
        thread::spawn(move || {
            let report = |share: f32| {
                shown.store((share.clamp(0.0, 1.0) * 1000.0) as u32, Ordering::Relaxed);
                repaint.request_repaint();
            };
            let result = if sound {
                bareclip_media::cut_sound(&source, &output, range, &stop, &report).map(|how| {
                    vec![match how {
                        bareclip_media::SoundCut::Exact => "Cut to the exact sample.".to_owned(),
                        bareclip_media::SoundCut::Packets => {
                            "Copied with nothing re-encoded, to within a few hundredths of a \
                             second of each handle."
                                .to_owned()
                        }
                    }]
                })
            } else {
                let request = ExportRequest {
                    output: output.clone(),
                    parts: vec![ExportPart {
                        input: source.clone(),
                        frames: Arc::new(frames.unwrap_or_else(|| FrameIndex::new(Vec::new()))),
                        range,
                        gap_before: Ticks(0),
                    }],
                    mode: CutMode::FrameExact,
                };
                let exported = if keeps_its_kind(&source, false) {
                    bareclip_export::export_project(&request, &stop, &report)
                } else {
                    bareclip_export::export_full(
                        &request,
                        &bareclip_export::FullPlan::for_videos(&[info]),
                        &stop,
                        &report,
                    )
                    .map(bareclip_export::ProjectReport::Full)
                };
                exported.map(|report| text::export_summary(&report))
            };
            let _ = sender.send(result.map(|summary| (output, summary)).map_err(
                |error| match error {
                    bareclip_media::MediaError::Cancelled => String::new(),
                    error => format!("The trim couldn't be saved. {error}"),
                },
            ));
            repaint.request_repaint();
        });
        if let Some(trimmer) = &mut self.trim {
            trimmer.problem = None;
            trimmer.saved = None;
            trimmer.saving = Some(Saving {
                receiver,
                progress,
                cancel,
            });
        }
    }

    /// Save…: Windows' Save window in the file's folder, saying first if it becomes an MP4.
    pub fn save_trim_as(&mut self, ctx: &egui::Context) {
        let Some(file) = &self.file else {
            return;
        };
        let sound = file.is_sound();
        if !keeps_its_kind(&file.info.path, sound)
            && let Some(trimmer) = &mut self.trim
            && !trimmer.asking_mp4
        {
            trimmer.asking_mp4 = true;
            return;
        }
        if let Some(trimmer) = &mut self.trim {
            trimmer.asking_mp4 = false;
        }
        let suggested = copy_name(&file.info.path, sound);
        let extension = suggested
            .extension()
            .map(|extension| extension.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut dialog = rfd::FileDialog::new()
            .set_title("Save the trimmed clip")
            .set_file_name(
                suggested
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            )
            .add_filter(extension.to_uppercase(), &[extension.as_str()]);
        if let Some(folder) = file.info.path.parent() {
            dialog = dialog.set_directory(folder);
        }
        if let Some(target) = dialog.save_file() {
            self.save_trim(ctx, Some(target));
        }
    }

    /// Takes in what the strip readers and a save have sent.
    pub(super) fn poll_trimmer(&mut self, ctx: &egui::Context) {
        let Some(trimmer) = &mut self.trim else {
            return;
        };
        if let Some(arriving) = &trimmer.arriving {
            for picture in arriving.try_iter().take(8) {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [picture.width as usize, picture.height as usize],
                    &picture.rgba,
                );
                let texture = ctx.load_texture("trim-strip", image, TextureOptions::LINEAR);
                trimmer.pictures.push((picture.time, texture));
            }
        }
        if let Some(arriving) = &trimmer.shape_arriving
            && let Ok(shape) = arriving.try_recv()
        {
            trimmer.shape = shape;
            trimmer.shape_arriving = None;
        }
        let finished = trimmer
            .saving
            .as_ref()
            .and_then(|saving| saving.receiver.try_recv().ok());
        if let Some(result) = finished {
            trimmer.saving = None;
            match result {
                Ok(saved) => trimmer.saved = Some(saved),
                Err(problem) if problem.is_empty() => {}
                Err(problem) => trimmer.problem = Some(problem),
            }
        }
    }

    /// Shows a save partway, for pictures of the screen.
    #[cfg(test)]
    pub fn pretend_saving(
        &mut self,
        share: f32,
    ) -> mpsc::Sender<Result<(PathBuf, Vec<String>), String>> {
        let (sender, receiver) = mpsc::channel();
        if let Some(trimmer) = &mut self.trim {
            trimmer.saving = Some(Saving {
                receiver,
                progress: Arc::new(AtomicU32::new((share * 1000.0) as u32)),
                cancel: Arc::new(AtomicBool::new(false)),
            });
        }
        sender
    }

    /// Stops a save that is under way, leaving no file.
    #[cfg(test)]
    pub fn cancel_trim_save(&self) {
        if let Some(saving) = self
            .trim
            .as_ref()
            .and_then(|trimmer| trimmer.saving.as_ref())
        {
            saving.cancel.store(true, Ordering::Relaxed);
        }
    }

    #[cfg(test)]
    pub fn is_saving_trim(&self) -> bool {
        self.trim
            .as_ref()
            .is_some_and(|trimmer| trimmer.saving.is_some())
    }

    /// The strip with its handles, and the start, end and kept length under it.
    pub(super) fn trim_row(&mut self, ui: &mut egui::Ui, palette: &Palette, row: Rect) {
        let Some(trimmer) = &self.trim else {
            return;
        };
        let duration = self.duration();
        let strip = Rect::from_min_size(
            row.min + vec2(16.0, 6.0),
            vec2(row.width() - 32.0, STRIP_HEIGHT),
        );
        let x_of = |time: Ticks| {
            strip.left() + strip.width() * (time.0 as f64 / duration.0.max(1) as f64) as f32
        };
        let time_at = |x: f32| {
            Ticks(
                (((x - strip.left()) / strip.width()).clamp(0.0, 1.0) as f64 * duration.0 as f64)
                    as i64,
            )
        };
        let painter = ui.painter_at(strip.expand(HANDLE_WIDTH));
        painter.rect_filled(strip, 4.0, palette.stage);
        if let Some(shape) = &trimmer.shape {
            let bars = (strip.width() / 3.0) as usize;
            for bar in 0..bars {
                let from = Ticks(duration.0 * bar as i64 / bars.max(1) as i64);
                let to = Ticks(duration.0 * (bar as i64 + 1) / bars.max(1) as i64);
                let peak = shape.peak_between(from, to).max(0.04);
                let x = strip.left() + bar as f32 * 3.0 + 1.0;
                let height = (strip.height() - 8.0) * peak;
                painter.rect_filled(
                    Rect::from_center_size(pos2(x, strip.center().y), vec2(2.0, height)),
                    1.0,
                    palette.clip_audio,
                );
            }
        } else if !trimmer.pictures.is_empty() {
            let size = trimmer.pictures[0].1.size_vec2();
            let width = (STRIP_HEIGHT * size.x / size.y.max(1.0)).max(8.0);
            let slots = (strip.width() / width).ceil() as usize;
            for slot in 0..slots {
                let left = strip.left() + slot as f32 * width;
                let at = time_at(left);
                let picture = trimmer
                    .pictures
                    .iter()
                    .rev()
                    .find(|(time, _)| *time <= at)
                    .unwrap_or(&trimmer.pictures[0]);
                painter.image(
                    picture.1.id(),
                    Rect::from_min_size(pos2(left, strip.top()), vec2(width, STRIP_HEIGHT)),
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
        }
        // What is cut away is dimmed.
        let (start_x, end_x) = (x_of(trimmer.start), x_of(trimmer.end));
        let shade = Color32::from_black_alpha(170);
        painter.rect_filled(
            Rect::from_min_max(strip.min, pos2(start_x, strip.bottom())),
            0.0,
            shade,
        );
        painter.rect_filled(
            Rect::from_min_max(pos2(end_x, strip.top()), strip.max),
            0.0,
            shade,
        );
        painter.rect_stroke(
            Rect::from_min_max(pos2(start_x, strip.top()), pos2(end_x, strip.bottom())),
            2.0,
            Stroke::new(2.0, palette.accent),
            egui::StrokeKind::Inside,
        );
        let playhead = x_of(self.position());
        painter.vline(
            playhead,
            (strip.top() - 3.0)..=(strip.bottom() + 3.0),
            Stroke::new(2.0, palette.playhead),
        );

        // A click on the strip moves the playhead, and the handles drag.
        let strip_response = ui.interact(strip, egui::Id::unique("trim_strip"), Sense::click());
        if strip_response.clicked()
            && let Some(pointer) = strip_response.interact_pointer_pos()
        {
            self.seek(time_at(pointer.x));
        }
        for (handle, x, name) in [
            (Handle::Start, start_x, "Trim start"),
            (Handle::End, end_x, "Trim end"),
        ] {
            let rect = Rect::from_center_size(
                pos2(x, strip.center().y),
                vec2(HANDLE_WIDTH, STRIP_HEIGHT + 8.0),
            );
            let response = ui.interact(rect, egui::Id::unique(name), Sense::drag());
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, true, name));
            let fill = if response.hovered() || response.dragged() {
                palette.accent_hover
            } else {
                palette.accent
            };
            ui.painter().rect_filled(rect, 3.0, fill);
            if response.hovered() || response.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
            if response.drag_started() {
                self.pause();
                if let Some(trimmer) = &mut self.trim {
                    trimmer.held = Some(handle);
                }
            }
            if response.dragged()
                && let Some(pointer) = response.interact_pointer_pos()
            {
                let time = time_at(pointer.x);
                match handle {
                    Handle::Start => {
                        self.set_trim_start(time);
                        let shown = self.trim_range().map_or(time, |range| range.start);
                        self.seek(shown);
                    }
                    Handle::End => {
                        self.set_trim_end(time);
                        let end = self.trim_range().map_or(time, |range| range.end);
                        // The last frame kept is the one shown.
                        self.seek(self.last_kept_frame(end));
                    }
                }
            }
            if response.drag_stopped()
                && let Some(trimmer) = &mut self.trim
            {
                trimmer.held = None;
            }
        }

        let Some(range) = self.trim_range() else {
            return;
        };
        let labels = Rect::from_min_max(
            pos2(strip.left(), strip.bottom() + 4.0),
            pos2(strip.right(), row.bottom()),
        );
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(labels)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 18.0;
                for line in [
                    format!("Start {}", format_timecode(range.start)),
                    format!("End {}", format_timecode(range.end)),
                    format!("Kept {}", text::clip_length(range.duration())),
                ] {
                    ui.add(
                        egui::Label::new(RichText::new(line).size(12.0).color(palette.text_muted))
                            .selectable(false),
                    );
                }
            },
        );
    }

    /// Cancel, Save as copy, Save… and Edit in BareClip, or the save's progress, or what it did.
    pub(super) fn trim_actions(&mut self, ui: &mut egui::Ui, palette: &Palette, row: Rect) {
        let ctx = ui.ctx().clone();
        let Some(trimmer) = &self.trim else {
            return;
        };
        let saving = trimmer
            .saving
            .as_ref()
            .map(|saving| saving.progress.load(Ordering::Relaxed) as f32 / 1000.0);
        let saved = trimmer.saved.clone();
        let problem = trimmer.problem.clone();
        let inner = row.shrink2(vec2(16.0, 4.0));
        let mut cancel_save = false;
        let mut leave = false;
        let mut copy = false;
        let mut save_as = false;
        let mut play_saved = None;
        let mut edit = false;
        // What happened, on the left.
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(inner)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if let Some(share) = saving {
                    ui.allocate_ui(
                        vec2((inner.width() - 120.0).clamp(80.0, 360.0), 24.0),
                        |ui| {
                            components::progress_bar(ui, palette, share);
                        },
                    );
                    cancel_save = ui
                        .add(ActionButton::new(palette, Kind::Secondary, "Cancel saving").small())
                        .clicked();
                } else if let Some((path, summary)) = &saved {
                    components::symbol(ui, &icons::CIRCLE_CHECK, 16.0, palette.success);
                    let name = path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("Saved {name}"))
                                .size(12.5)
                                .color(palette.text),
                        )
                        .truncate()
                        .selectable(false),
                    )
                    .on_hover_text(summary.join("\n"));
                    if ui
                        .add(ActionButton::new(palette, Kind::Secondary, "Play").small())
                        .clicked()
                    {
                        play_saved = Some(path.clone());
                    }
                    if ui
                        .add(ActionButton::new(palette, Kind::Secondary, "Show in folder").small())
                        .clicked()
                    {
                        super::show_in_folder(path);
                    }
                } else if let Some(problem) = &problem {
                    ui.add(
                        egui::Label::new(RichText::new(problem).size(12.5).color(palette.danger))
                            .truncate(),
                    );
                }
            },
        );
        if saving.is_none() {
            ui.scope_builder(
                UiBuilder::new()
                    .max_rect(inner)
                    .layout(Layout::right_to_left(Align::Center)),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    edit = ui
                        .add(
                            ActionButton::new(palette, Kind::Secondary, "Edit in BareClip")
                                .icon(icons::CLAPPERBOARD)
                                .small(),
                        )
                        .on_hover_text("Send the clip as trimmed to the editor")
                        .clicked();
                    save_as = ui
                        .add(ActionButton::new(palette, Kind::Secondary, "Save…").small())
                        .on_hover_text("Choose where to save the trimmed clip")
                        .clicked();
                    copy = ui
                        .add(ActionButton::new(palette, Kind::Primary, "Save as copy").small())
                        .on_hover_text("Save the trimmed clip beside the original")
                        .clicked();
                    leave = ui
                        .add(ActionButton::new(palette, Kind::Secondary, "Cancel").small())
                        .on_hover_text("Leave the trimmer (Escape)")
                        .clicked();
                },
            );
        }
        if cancel_save
            && let Some(saving) = self
                .trim
                .as_ref()
                .and_then(|trimmer| trimmer.saving.as_ref())
        {
            saving.cancel.store(true, Ordering::Relaxed);
        }
        if leave {
            self.close_trimmer();
        }
        if copy {
            self.save_trim(&ctx, None);
        }
        if save_as {
            self.save_trim_as(&ctx);
        }
        if edit {
            self.edit_in_bareclip();
        }
        if let Some(path) = play_saved {
            self.close_trimmer();
            self.open(&ctx, &path);
        }
    }

    /// Asks before Save… makes an MP4 of a kind of video that can't be kept.
    pub(super) fn mp4_question(&mut self, ctx: &egui::Context, palette: &Palette) {
        if !self.trim.as_ref().is_some_and(|trimmer| trimmer.asking_mp4) {
            return;
        }
        let mut go_on = false;
        let mut cancel = false;
        egui::Modal::new(egui::Id::unique("viewer_mp4_question"))
            .frame(crate::editor::dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width(440.0);
                components::dialog_title(ui, &icons::INFO, palette.accent, "This will be an MP4");
                ui.add_space(10.0);
                ui.colored_label(
                    palette.text_muted,
                    "This kind of video can't be cut by copying, so the trimmed clip is drawn \
                     again into an MP4.",
                );
                ui.add_space(18.0);
                components::dialog_buttons(ui, |ui| {
                    go_on = ui
                        .add(ActionButton::new(palette, Kind::Primary, "Choose where"))
                        .clicked();
                    cancel = ui
                        .add(ActionButton::new(palette, Kind::Secondary, "Cancel"))
                        .clicked();
                });
            });
        if cancel {
            if let Some(trimmer) = &mut self.trim {
                trimmer.asking_mp4 = false;
            }
        } else if go_on {
            self.save_trim_as(ctx);
        }
    }
}

impl Viewer {
    /// Edit in BareClip: sends the whole file, or the clip as trimmed, to the editor used last,
    /// and pauses. With no editor open, this window becomes the editor.
    pub fn edit_in_bareclip(&mut self) {
        let Some(file) = &self.file else {
            return;
        };
        let range = self.trim_range().unwrap_or(TimeRange {
            start: Ticks(0),
            end: file.info.duration,
        });
        let clip = crate::handoff::Handed {
            path: file.info.path.clone(),
            range: Some(range),
        };
        self.pause();
        if !crate::handoff::send_clip_to_editor(&self.editor_prefix, &clip) {
            self.become_editor = Some(clip);
        }
    }

    /// The clip to start an editor with, once, when this window is to become the editor.
    pub fn take_become_editor(&mut self) -> Option<crate::handoff::Handed> {
        self.become_editor.take()
    }
}
