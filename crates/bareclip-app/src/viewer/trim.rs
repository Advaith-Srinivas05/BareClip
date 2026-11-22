//! The quick trimmer: two handles on a line along the whole file, which keep one stretch, and
//! saving that stretch as a new file beside the original. It is laid out like the Photos app's:
//! "Trim" and the save buttons along the top, and a pill along the bottom with play, the volume,
//! the line between the start and end times, and "…" with the trimmer's tools.
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
    time::Ticks,
    timeline::TimeRange,
};
use bareclip_media::{ExportPart, ExportRequest};
use eframe::egui::{
    self, Align, Align2, FontId, Layout, Pos2, Rangef, Rect, RichText, Sense, Stroke, UiBuilder,
    pos2, vec2,
};

use super::{
    Viewer,
    bars::Flyout,
    note::{Note, NotePress},
    pill::{self, PILL_HEIGHT, PILL_RADIUS, raised_card},
};
use crate::{
    components::{self, ActionButton, IconButton, Kind, MenuItem},
    fonts, icons, text,
    theme::Palette,
};

/// The kinds of video a trim is copied into as they are. Others come out as an MP4, redrawn.
const COPIED_CONTAINERS: [&str; 4] = ["mp4", "m4v", "mov", "mkv"];
/// How wide a save's progress is.
const SAVING_BAR_WIDTH: f32 = 220.0;
/// The card a save's progress shows in.
const SAVING_CARD_WIDTH: f32 = 440.0;
/// The room between Save as copy and Cancel and the window's buttons.
const TITLE_BUTTON_GAP: f32 = 16.0;
/// "Trim" in the top bar: its symbol, the room after it, and the mark under the word.
const TRIM_ICON_SIZE: f32 = 16.0;
const TRIM_ICON_GAP: f32 = 8.0;
const TRIM_MARK_WIDTH: f32 = 22.0;
/// The room under the pill, so it doesn't sit tight on the window's edge.
const PILL_BOTTOM_ROOM: f32 = 12.0;
/// The bubble with the playhead's time, over the pill.
const BUBBLE_WIDTH: f32 = 104.0;
const BUBBLE_HEIGHT: f32 = 32.0;
const BUBBLE_GAP: f32 = 10.0;
/// Room over the bubble, and how tall the bottom is while trimming.
const TRIM_BAR_TOP: f32 = 16.0;
pub const TRIM_BAR_HEIGHT: f32 =
    TRIM_BAR_TOP + BUBBLE_HEIGHT + 8.0 + BUBBLE_GAP + PILL_HEIGHT + PILL_BOTTOM_ROOM;
/// The line: how thick it is, its handles, and the playhead's thumb on it, which is the seek
/// line's: a light dot ringed in the accent that grows under the mouse.
const LINE_THICKNESS: f32 = 4.0;
const HANDLE_WIDTH: f32 = 8.0;
const HANDLE_HEIGHT: f32 = 24.0;
const GRIP_LENGTH: f32 = 10.0;
const THUMB: f32 = 6.0;
const THUMB_HOVER: f32 = 8.0;
/// How tall the stretch is that a click or drag seeks along.
const SEEK_REACH: f32 = 32.0;
/// How long the thumb takes to grow and settle.
const SWELL_TIME: f32 = 0.12;

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
    saving: Option<Saving>,
    /// The file last saved, and what the save did.
    pub saved: Option<(PathBuf, Vec<String>)>,
    pub problem: Option<String>,
    /// Playing goes round the kept stretch rather than stopping at its end.
    pub looping: bool,
    /// The saved trim leaves the sound behind.
    pub without_sound: bool,
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

    /// Opens the trimmer, keeping the whole file.
    pub fn open_trimmer(&mut self, _ctx: &egui::Context) {
        let Some(file) = &self.file else {
            return;
        };
        self.flyout.close();
        self.trim = Some(Trimmer {
            start: Ticks(0),
            end: file.info.duration,
            held: None,
            saving: None,
            saved: None,
            problem: None,
            looping: false,
            without_sound: false,
        });
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

    /// While trimming, play runs between the handles and stops at the end one, or goes round
    /// to the start one with Loop the kept part on.
    pub(super) fn keep_within_trim(&mut self) {
        let Some(range) = self.trim_range() else {
            return;
        };
        let looping = self.trim.as_ref().is_some_and(|trimmer| trimmer.looping);
        if self.is_playing() && self.position() >= range.end {
            if looping {
                self.seek(range.start);
            } else {
                self.pause();
                self.seek(self.last_kept_frame(range.end));
            }
        }
    }

    /// Both handles back at the file's ends.
    pub fn reset_trim(&mut self) {
        let duration = self.duration();
        if let Some(trimmer) = &mut self.trim {
            trimmer.start = Ticks(0);
            trimmer.end = duration;
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
        let keep_sound = sound
            || !self
                .trim
                .as_ref()
                .is_some_and(|trimmer| trimmer.without_sound);
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
                let exported = if !keeps_its_kind(&source, false) {
                    let mut plan = bareclip_export::FullPlan::for_videos(&[info]);
                    if !keep_sound {
                        plan.settings.sound_tracks.clear();
                    }
                    bareclip_export::export_full(&request, &plan, &stop, &report)
                        .map(bareclip_export::ProjectReport::Full)
                } else if keep_sound {
                    bareclip_export::export_project(&request, &stop, &report)
                } else {
                    // One file is always copied the fast way, so the sound is simply left out.
                    bareclip_media::export_picture_only(&request, &stop, &report)
                        .map(bareclip_export::ProjectReport::Fast)
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

    /// Takes in what a save has sent.
    pub(super) fn poll_trimmer(&mut self, _ctx: &egui::Context) {
        let Some(trimmer) = &mut self.trim else {
            return;
        };
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

    /// Whether a trim is being saved, while the save buttons wait.
    pub fn is_saving_trim(&self) -> bool {
        self.trim
            .as_ref()
            .is_some_and(|trimmer| trimmer.saving.is_some())
    }

    /// The top bar while trimming, as Photos lays it out: the file's name on the left, "Trim"
    /// in the middle, and Save as copy and Cancel before the window's buttons on the right.
    /// The save buttons wait while a save is under way.
    pub(super) fn trim_title(&mut self, ui: &mut egui::Ui, palette: &Palette, rect: Rect) {
        let ctx = ui.ctx().clone();
        let inner = rect.shrink2(vec2(8.0, 0.0));
        let idle = !self.is_saving_trim();
        let mut pressed = (false, false);
        let right = ui
            .scope_builder(
                UiBuilder::new()
                    .max_rect(inner)
                    .layout(Layout::right_to_left(Align::Center)),
                |ui| {
                    self.window_buttons(ui, palette);
                    ui.add_space(TITLE_BUTTON_GAP);
                    ui.spacing_mut().item_spacing.x = 8.0;
                    pressed.1 = ui
                        .add(ActionButton::new(palette, Kind::Secondary, "Cancel").small())
                        .on_hover_text("Leave the trimmer (Escape)")
                        .clicked();
                    pressed.0 = ui
                        .add_enabled(
                            idle,
                            ActionButton::new(palette, Kind::Primary, "Save as copy").small(),
                        )
                        .on_hover_text("Save the trimmed clip beside the original")
                        .clicked();
                },
            )
            .response
            .rect;

        // "Trim" in the middle of the window, marked as the mode the player is in.
        let font = FontId::proportional(14.0);
        let word = ui
            .painter()
            .layout_no_wrap("Trim".to_owned(), font, palette.text);
        let width = TRIM_ICON_SIZE + TRIM_ICON_GAP + word.size().x;
        let left = rect.center().x - width / 2.0;
        let icon = icons::square_at(
            pos2(left + TRIM_ICON_SIZE / 2.0, rect.center().y),
            TRIM_ICON_SIZE,
        );
        icons::paint(ui.painter(), icon, &icons::SCISSORS, palette.text);
        let text_at = pos2(
            left + TRIM_ICON_SIZE + TRIM_ICON_GAP,
            rect.center().y - word.size().y / 2.0,
        );
        let underline_y = text_at.y + word.size().y + 4.0;
        let underline_middle = text_at.x + word.size().x / 2.0;
        ui.painter().galley(text_at, word, palette.text);
        ui.painter().line_segment(
            [
                pos2(underline_middle - TRIM_MARK_WIDTH / 2.0, underline_y),
                pos2(underline_middle + TRIM_MARK_WIDTH / 2.0, underline_y),
            ],
            Stroke::new(3.0, palette.accent),
        );

        // The name on the left, as far as the middle allows.
        let name_room = Rect::from_min_max(
            pos2(inner.left() + 8.0, rect.top()),
            pos2((left - 16.0).min(right.left() - 16.0), rect.bottom()),
        );
        if name_room.width() > 24.0 {
            ui.scope_builder(
                UiBuilder::new()
                    .max_rect(name_room)
                    .layout(Layout::left_to_right(Align::Center)),
                |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(self.name())
                                .font(FontId::proportional(components::NAME_BAR_TEXT - 1.0))
                                .color(palette.text),
                        )
                        .truncate()
                        .selectable(false),
                    );
                },
            );
        }
        if pressed.0 {
            self.save_trim(&ctx, None);
        }
        if pressed.1 {
            self.close_trimmer();
        }
    }

    /// The bottom while trimming: the time at the playhead in a bubble, or a save's progress,
    /// over a pill holding play, the volume, the start time, the line with its handles, the
    /// end time and "…". The pill is as wide as the player's, for the picture's width `across`,
    /// and centred under the picture.
    pub(super) fn trim_bar(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        rect: Rect,
        across: Rangef,
    ) {
        let Some(range) = self.trim_range() else {
            return;
        };
        let pill = Rect::from_center_size(
            pos2(
                across.center(),
                rect.bottom() - PILL_BOTTOM_ROOM - PILL_HEIGHT / 2.0,
            ),
            vec2(pill::pill_width(across.span()), PILL_HEIGHT),
        );
        let bubble_middle = pos2(
            pill.center().x,
            pill.top() - BUBBLE_GAP - BUBBLE_HEIGHT / 2.0,
        );
        let saving = self
            .trim
            .as_ref()
            .and_then(|trimmer| trimmer.saving.as_ref())
            .map(|saving| saving.progress.load(Ordering::Relaxed) as f32 / 1000.0);
        if let Some(share) = saving {
            self.save_progress(ui, palette, bubble_middle, share);
        } else {
            raised_card(
                ui,
                palette,
                Rect::from_center_size(bubble_middle, vec2(BUBBLE_WIDTH, BUBBLE_HEIGHT)),
                BUBBLE_HEIGHT / 2.0,
            );
            ui.painter().text(
                bubble_middle,
                Align2::CENTER_CENTER,
                self.clock(self.position()),
                fonts::strong(14.0),
                palette.text,
            );
        }

        raised_card(ui, palette, pill, PILL_RADIUS);
        let content = pill.shrink2(vec2(8.0, 0.0));
        let duration = self.duration();
        let times = [self.clock(range.start), self.clock(range.end)];
        let time_box = pill::time_width(ui, &self.clock(duration)) + 4.0;
        let mut line = Rect::NOTHING;
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(content)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (icon, name) = if self.is_playing() {
                    (icons::PAUSE, "Pause")
                } else {
                    (icons::PLAY, "Play")
                };
                if ui
                    .add(IconButton::new(palette, Kind::Ghost, icon, name).solid())
                    .on_hover_text("Play or pause (Space)")
                    .clicked()
                {
                    self.toggle_playback();
                }
                self.volume_button(ui, palette);
                ui.add_space(pill::TIME_SIDE_GAP);
                trim_time(ui, palette, &times[0], time_box, Align::Max);
                // The line takes what the end time and More leave.
                let room = ui.available_width()
                    - time_box
                    - pill::TIME_SIDE_GAP
                    - IconButton::default_size()
                    - ui.spacing().item_spacing.x * 3.0;
                let (rect, _) =
                    ui.allocate_exact_size(vec2(room.max(40.0), PILL_HEIGHT), Sense::hover());
                line = rect;
                trim_time(ui, palette, &times[1], time_box, Align::Min);
                ui.add_space(pill::TIME_SIDE_GAP);
                self.more_button(ui, palette);
            },
        );
        self.trim_line(ui, palette, line);
    }

    /// A save's progress, with a way to stop it, where the time's bubble goes.
    fn save_progress(&mut self, ui: &mut egui::Ui, palette: &Palette, middle: Pos2, share: f32) {
        let card = Rect::from_center_size(middle, vec2(SAVING_CARD_WIDTH, BUBBLE_HEIGHT + 8.0));
        raised_card(ui, palette, card, (BUBBLE_HEIGHT + 8.0) / 2.0);
        let mut cancel_save = false;
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(card.shrink2(vec2(12.0, 0.0)))
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                ui.add(
                    egui::Label::new(
                        RichText::new("Saving the trim")
                            .size(12.5)
                            .color(palette.text),
                    )
                    .selectable(false),
                );
                ui.allocate_ui(vec2(SAVING_BAR_WIDTH, 24.0), |ui| {
                    components::progress_bar(ui, palette, share);
                });
                cancel_save = ui
                    .add(ActionButton::new(palette, Kind::Ghost, "Cancel saving").small())
                    .clicked();
            },
        );
        if cancel_save
            && let Some(saving) = self
                .trim
                .as_ref()
                .and_then(|trimmer| trimmer.saving.as_ref())
        {
            saving.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// The pill's "…": the start or end at the playhead, back to the whole file, looping the
    /// kept stretch, and saving it without its sound.
    fn more_button(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let kind = super::bars::flyout_kind(self.flyout.open, Flyout::More);
        let response = ui
            .add(IconButton::new(
                palette,
                kind,
                icons::ELLIPSIS,
                "More options",
            ))
            .on_hover_text("More options");
        let sound = self.file.as_ref().is_some_and(super::OpenFile::is_sound);
        let (looping, without_sound) = self.trim.as_ref().map_or((false, false), |trimmer| {
            (trimmer.looping, trimmer.without_sound)
        });
        let idle = !self.is_saving_trim();
        let chosen = super::bars::flyout(&response, &mut self.flyout, Flyout::More, true, |ui| {
            let start = ui
                .add(
                    MenuItem::new(
                        palette,
                        icons::ALIGN_START_VERTICAL,
                        "Start at playhead",
                        "[",
                    )
                    .tick_room(),
                )
                .on_hover_text("Keep from the frame showing")
                .clicked();
            let end = ui
                .add(
                    MenuItem::new(palette, icons::ALIGN_END_VERTICAL, "End at playhead", "]")
                        .tick_room(),
                )
                .on_hover_text("Keep up to the frame showing")
                .clicked();
            let reset = ui
                .add(
                    MenuItem::new(palette, icons::ROTATE_CCW, "Reset to whole clip", "")
                        .tick_room(),
                )
                .clicked();
            ui.separator();
            let looping = ui
                .add(MenuItem::ticked(
                    palette,
                    icons::REPEAT,
                    "Loop the kept part",
                    looping,
                ))
                .on_hover_text("Play the kept part over and over")
                .clicked();
            let without_sound = !sound
                && ui
                    .add_enabled(
                        idle,
                        MenuItem::ticked(
                            palette,
                            icons::VOLUME_X,
                            "Save without sound",
                            without_sound,
                        ),
                    )
                    .on_hover_text("The saved clip has no sound")
                    .clicked();
            TrimMore {
                start,
                end,
                reset,
                looping,
                without_sound,
            }
        });
        let pressed = chosen.unwrap_or_default();
        if pressed.start {
            self.set_trim_start(self.position());
        }
        if pressed.end {
            self.set_trim_end(self.position());
        }
        if pressed.reset {
            self.reset_trim();
        }
        if let Some(trimmer) = &mut self.trim {
            trimmer.looping ^= pressed.looping;
            trimmer.without_sound ^= pressed.without_sound;
        }
    }

    /// The line along the whole file: the kept stretch in the accent between its two handles,
    /// and the playhead as a pin standing over it. A click or drag on the line moves the
    /// playhead; the handles drag, landing on frames.
    fn trim_line(&mut self, ui: &mut egui::Ui, palette: &Palette, rect: Rect) {
        let Some(trimmer) = &self.trim else {
            return;
        };
        let duration = self.duration();
        let track = Rect::from_center_size(
            rect.center(),
            vec2(rect.width() - HANDLE_WIDTH, LINE_THICKNESS),
        );
        let x_of = |time: Ticks| {
            track.left() + track.width() * (time.0 as f64 / duration.0.max(1) as f64) as f32
        };
        let time_at = |x: f32| {
            Ticks(
                (((x - track.left()) / track.width()).clamp(0.0, 1.0) as f64 * duration.0 as f64)
                    as i64,
            )
        };
        let (start_x, end_x) = (x_of(trimmer.start), x_of(trimmer.end));
        let painter = ui.painter();
        painter.rect_filled(track, LINE_THICKNESS / 2.0, palette.border_strong);
        painter.rect_filled(
            Rect::from_min_max(pos2(start_x, track.top()), pos2(end_x, track.bottom())),
            0.0,
            palette.accent,
        );

        // A click or drag along the line, or on the thumb, moves the playhead.
        let seek_area = Rect::from_center_size(
            rect.center(),
            vec2(track.width() + THUMB_HOVER * 2.0, SEEK_REACH),
        );
        let line_response = ui.interact(
            seek_area,
            egui::Id::unique("trim_line"),
            Sense::click_and_drag(),
        );
        line_response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, true, "Seek"));
        if (line_response.clicked() || line_response.dragged())
            && let Some(pointer) = line_response.interact_pointer_pos()
        {
            self.seek(time_at(pointer.x));
        }

        // The playhead, drawn as the seek line's thumb under the handles, so a handle at the
        // same spot still shows and is what a drag there takes.
        let active = line_response.hovered() || line_response.dragged();
        let swell =
            ui.ctx()
                .animate_bool_with_time(line_response.id.with("swell"), active, SWELL_TIME);
        let middle = pos2(x_of(self.position()), track.center().y);
        let thumb = egui::lerp(THUMB..=THUMB_HOVER, swell);
        ui.painter()
            .circle_filled(middle, thumb, crate::theme::DARK.text);
        ui.painter()
            .circle_stroke(middle, thumb, Stroke::new(2.0, palette.accent));

        for (handle, x, name) in [
            (Handle::Start, start_x, "Trim start"),
            (Handle::End, end_x, "Trim end"),
        ] {
            let grip = Rect::from_center_size(
                pos2(x, track.center().y),
                vec2(HANDLE_WIDTH, HANDLE_HEIGHT),
            );
            let response = ui.interact(
                grip.expand2(vec2(4.0, 2.0)),
                egui::Id::unique(name),
                Sense::drag(),
            );
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, true, name));
            let fill = if response.hovered() || response.dragged() {
                palette.accent_hover
            } else {
                palette.accent
            };
            ui.painter().rect_filled(grip, HANDLE_WIDTH / 2.0, fill);
            ui.painter().vline(
                grip.center().x,
                (grip.center().y - GRIP_LENGTH / 2.0)..=(grip.center().y + GRIP_LENGTH / 2.0),
                Stroke::new(1.5, palette.accent_text),
            );
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
    }

    /// What the last save did, as a note over the picture: the file saved, with Play and Show
    /// in folder, or why it couldn't be. Each stays until closed, or the trimmer closes.
    pub(super) fn trim_notes(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        area: Rect,
        top: &mut f32,
    ) {
        let ctx = ui.ctx().clone();
        let Some(trimmer) = &self.trim else {
            return;
        };
        let saved = trimmer.saved.clone();
        let problem = trimmer.problem.clone();
        if let Some((path, summary)) = saved {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let words = format!("Saved {name}");
            let summary = summary.join("\n");
            match Note::new(&icons::CIRCLE_CHECK, palette.success, &words)
                .hover(&summary)
                .actions(&["Play it", "Show in folder"])
                .closable()
                .show(ui, palette, area, top)
            {
                NotePress::Action(0) => {
                    self.close_trimmer();
                    self.open(&ctx, &path);
                    return;
                }
                NotePress::Action(_) => super::show_in_folder(&path),
                NotePress::Close => {
                    if let Some(trimmer) = &mut self.trim {
                        trimmer.saved = None;
                    }
                }
                NotePress::Nothing => {}
            }
        }
        if let Some(problem) = problem
            && Note::new(&icons::CIRCLE_ALERT, palette.danger, &problem)
                .closable()
                .show(ui, palette, area, top)
                == NotePress::Close
            && let Some(trimmer) = &mut self.trim
        {
            trimmer.problem = None;
        }
    }
}

impl Viewer {
    /// Edit in BareClip: sends the whole file to the editor used last, and pauses. With no
    /// editor open, this window becomes the editor.
    pub fn edit_in_bareclip(&mut self) {
        let Some(file) = &self.file else {
            return;
        };
        let range = TimeRange {
            start: Ticks(0),
            end: file.info.duration,
        };
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

/// A start or end time in the pill, in a box of one width so the line beside it never moves.
fn trim_time(ui: &mut egui::Ui, palette: &Palette, time: &str, width: f32, align: Align) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, PILL_HEIGHT), Sense::hover());
    let (anchor, at) = if align == Align::Max {
        (Align2::RIGHT_CENTER, rect.right_center())
    } else {
        (Align2::LEFT_CENTER, rect.left_center())
    };
    ui.painter()
        .text(at, anchor, time, FontId::proportional(13.0), palette.text);
}

/// What was chosen in the trimmer's "…".
#[derive(Clone, Copy, Debug, Default)]
struct TrimMore {
    start: bool,
    end: bool,
    reset: bool,
    looping: bool,
    without_sound: bool,
}
