//! Moving through the files in the open file's folder, and picking a file up where it was
//! last stopped.

use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use bareclip_core::time::{Ticks, format_timecode};
use eframe::egui::{self, Align, Layout, Rect, RichText, UiBuilder, pos2, vec2};

use super::Viewer;
use crate::{
    components::{ActionButton, IconButton, Kind},
    folder_files, icons,
    theme::Palette,
};

/// How long Start over stays after a file picks up where it was stopped.
pub const START_OVER_FOR: Duration = Duration::from_secs(5);
/// How often the spot of a file that is playing is kept, so a crash loses little.
const KEEP_SPOT_EVERY: Duration = Duration::from_secs(5);
const EDGE_BUTTON: f32 = 44.0;

impl Viewer {
    /// The file on screen, or the one that couldn't be played.
    pub fn current_path(&self) -> Option<&Path> {
        self.file
            .as_ref()
            .map(|file| file.info.path.as_path())
            .or(self.failed.as_ref().map(|failed| failed.path.as_path()))
    }

    /// Starts reading the playable files in the folder of the file on screen.
    pub(super) fn read_folder(&mut self, ctx: &egui::Context) {
        let Some(folder) = self.current_path().and_then(Path::parent) else {
            return;
        };
        let folder = folder.to_owned();
        let (sender, receiver) = mpsc::channel();
        let repaint = ctx.clone();
        thread::spawn(move || {
            let _ = sender.send(folder_files::list(&folder));
            repaint.request_repaint();
        });
        self.folder_reading = Some(receiver);
    }

    pub(super) fn take_folder(&mut self) {
        if let Some(receiver) = &self.folder_reading
            && let Ok(files) = receiver.try_recv()
        {
            self.folder = files;
            self.folder_reading = None;
        }
    }

    /// The file after the one on screen in its folder, if there is one.
    pub fn next_file(&self) -> Option<PathBuf> {
        folder_files::next_after(&self.folder, self.current_path()?)
    }

    pub fn previous_file(&self) -> Option<PathBuf> {
        folder_files::previous_before(&self.folder, self.current_path()?)
    }

    /// Opens the next or previous file in the folder, if there is one.
    pub fn go_to_neighbour(&mut self, ctx: &egui::Context, forward: bool) {
        let neighbour = if forward {
            self.next_file()
        } else {
            self.previous_file()
        };
        if let Some(path) = neighbour {
            self.open(ctx, &path);
        }
    }

    /// Keeps where the file on screen was stopped, in memory.
    pub(super) fn remember_spot(&mut self) {
        if !self.settings.resume {
            return;
        }
        let Some(file) = &self.file else {
            return;
        };
        let (path, duration) = (file.info.path.clone(), file.info.duration);
        let at = self.position();
        self.resume.remember(&path, at, duration);
    }

    /// Writes the remembered spots to disk.
    pub(super) fn save_spots(&mut self) {
        if let Some(folder) = &self.app_folder
            && let Err(error) = self.resume.save(folder)
        {
            self.notice = Some(format!("Where you stopped couldn't be kept. {error}"));
        }
        self.spot_kept = Instant::now();
    }

    /// While playing, keeps the spot every few seconds, so a crash keeps it too.
    pub(super) fn keep_spot_while_playing(&mut self) {
        if self.is_playing() && self.spot_kept.elapsed() >= KEEP_SPOT_EVERY {
            self.remember_spot();
            self.save_spots();
        }
    }

    /// Jumps to where this file was last stopped, if resume is on and it was.
    pub(super) fn resume_if_stopped_partway(&mut self) {
        if !self.settings.resume {
            return;
        }
        let Some(path) = self.current_path().map(Path::to_owned) else {
            return;
        };
        if let Some(at) = self.resume.spot_for(&path) {
            self.seek(at);
            self.resumed = Some((at, Instant::now()));
        }
    }

    /// Goes back to the start after picking up partway.
    pub fn start_over(&mut self) {
        self.resumed = None;
        self.seek(Ticks(0));
    }

    pub fn set_resume(&mut self, on: bool) {
        self.settings.resume = on;
        self.save_settings();
    }

    /// The arrows at the picture's edges, while the mouse is over it.
    pub(super) fn edge_arrows(&mut self, ui: &mut egui::Ui, palette: &Palette, area: Rect) {
        if !ui.rect_contains_pointer(area) {
            return;
        }
        let ctx = ui.ctx().clone();
        let middle = area.center().y;
        let previous = self.previous_file().is_some();
        let next = self.next_file().is_some();
        let left = Rect::from_center_size(
            pos2(area.left() + 12.0 + EDGE_BUTTON / 2.0, middle),
            vec2(EDGE_BUTTON, EDGE_BUTTON),
        );
        let right = Rect::from_center_size(
            pos2(area.right() - 12.0 - EDGE_BUTTON / 2.0, middle),
            vec2(EDGE_BUTTON, EDGE_BUTTON),
        );
        if previous
            && IconButton::new(
                palette,
                Kind::Secondary,
                icons::CHEVRON_LEFT,
                "Previous file",
            )
            .size(EDGE_BUTTON, 20.0)
            .round()
            .show_at(ui, egui::Id::unique("viewer_previous"), left)
            .on_hover_text("Previous file (Page Up)")
            .clicked()
        {
            self.go_to_neighbour(&ctx, false);
        }
        if next
            && IconButton::new(palette, Kind::Secondary, icons::CHEVRON_RIGHT, "Next file")
                .size(EDGE_BUTTON, 20.0)
                .round()
                .show_at(ui, egui::Id::unique("viewer_next"), right)
                .on_hover_text("Next file (Page Down)")
                .clicked()
        {
            self.go_to_neighbour(&ctx, true);
        }
    }

    /// Says where a file picked up, with Start over, for a few seconds.
    pub(super) fn start_over_note(&mut self, ui: &mut egui::Ui, palette: &Palette, area: Rect) {
        let Some((at, since)) = self.resumed else {
            return;
        };
        if since.elapsed() >= START_OVER_FOR {
            self.resumed = None;
            return;
        }
        ui.ctx()
            .request_repaint_after(START_OVER_FOR - since.elapsed());
        let rect = Rect::from_min_size(
            pos2(area.left() + 16.0, area.bottom() - 16.0 - 40.0),
            vec2(260.0, 40.0),
        );
        ui.painter().rect_filled(rect, 8.0, palette.raised);
        let mut start_over = false;
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(rect.shrink2(vec2(12.0, 0.0)))
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("Picked up at {}", format_timecode(at)))
                            .size(13.0)
                            .color(palette.text),
                    )
                    .selectable(false),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    start_over = ui
                        .add(ActionButton::new(palette, Kind::Secondary, "Start over").small())
                        .clicked();
                });
            },
        );
        if start_over {
            self.start_over();
        }
    }
}
