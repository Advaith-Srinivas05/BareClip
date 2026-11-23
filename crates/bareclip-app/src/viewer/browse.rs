//! Moving through the files in the open file's folder.

use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
};

use eframe::egui::{self, Rect, pos2, vec2};

use super::Viewer;
use crate::{
    components::{IconButton, Kind},
    folder_files, icons,
    theme::Palette,
};

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

    /// Opens the next or previous file in the folder, if there is one. Not while trimming, which
    /// belongs to the file on screen.
    pub fn go_to_neighbour(&mut self, ctx: &egui::Context, forward: bool) {
        if self.trim.is_some() {
            return;
        }
        let neighbour = if forward {
            self.next_file()
        } else {
            self.previous_file()
        };
        if let Some(path) = neighbour {
            self.open(ctx, &path);
        }
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
}
