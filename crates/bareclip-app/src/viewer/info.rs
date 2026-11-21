//! The info panel on the right: almost everything a file says about itself, with its name,
//! Windows' dates, the artist and the creation time inside the file editable. Delete is here
//! too, because it lets go of the file the same way.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, Receiver},
    },
    thread::{self, JoinHandle},
    time::SystemTime,
};

use bareclip_media::{MediaDetails, StreamDetails, TagChanges};
use eframe::egui::{self, Align, Key, Layout, Rect, RichText, Stroke, UiBuilder, vec2};

use super::{Released, Viewer};
use crate::{
    components::{self, ActionButton, DateTime, IconButton, Kind},
    file_ops, fonts, icons, text,
    theme::Palette,
};

/// The details being read, and the thread reading them.
type DetailsRead = (Receiver<Result<MediaDetails, String>>, JoinHandle<()>);

/// How wide the panel is.
pub const PANEL_WIDTH: f32 = 330.0;
/// Files at least this large show the rewrite's progress.
const SHOW_PROGRESS_FROM: u64 = 50 << 20;

/// A copy of the file being written with new tags.
struct Rewrite {
    receiver: Receiver<Result<(), String>>,
    /// Thousandths of the file copied.
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    released: Released,
    big: bool,
}

/// Which field is being typed into.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Editing {
    Name(String),
    Created(String),
    Modified(String),
    Artist(String),
    Made(String),
}

#[derive(Default)]
pub struct InfoPanel {
    details: Option<Result<MediaDetails, String>>,
    reading: Option<DetailsRead>,
    /// Which file the details are of.
    read_for: Option<PathBuf>,
    times: Option<(SystemTime, SystemTime)>,
    editing: Option<Editing>,
    pub(super) problem: Option<String>,
    rewrite: Option<Rewrite>,
}

impl InfoPanel {
    /// Stops reading the file's details, waiting for the reader to let go of it.
    pub fn stop_reading(&mut self) {
        if let Some((_, reader)) = self.reading.take() {
            let _ = reader.join();
            self.read_for = None;
        }
    }

    #[cfg(test)]
    pub fn is_rewriting(&self) -> bool {
        self.rewrite.is_some()
    }

    /// Whether a box in the panel is being typed into.
    pub fn is_typing(&self) -> bool {
        self.editing.is_some()
    }

    #[cfg(test)]
    pub fn has_details(&self) -> bool {
        self.details.is_some()
    }
}

/// The creation time inside a file, as FFmpeg writes it, from a moment.
fn creation_time_text(moment: SystemTime) -> String {
    let utc = file_ops::utc_parts(moment);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.000000Z",
        utc.year, utc.month, utc.day, utc.hour, utc.minute, utc.second
    )
}

/// The moment a file's own creation time names, read as FFmpeg writes it.
pub fn read_creation_time(text: &str) -> Option<SystemTime> {
    let trimmed = text.trim().trim_end_matches('Z');
    let whole = trimmed.split('.').next()?;
    file_ops::from_utc(DateTime::parse(whole)?)
}

impl Viewer {
    #[cfg(test)]
    pub fn is_info_open(&self) -> bool {
        self.info.is_some()
    }

    pub fn toggle_info(&mut self) {
        if self.info.is_some() {
            if let Some(mut info) = self.info.take() {
                info.stop_reading();
            }
        } else {
            self.info = Some(InfoPanel::default());
        }
    }

    /// Reads the open file's details and dates in the background, once for each file.
    fn read_info(&mut self, ctx: &egui::Context) {
        let Some(path) = self.file.as_ref().map(|file| file.info.path.clone()) else {
            return;
        };
        let Some(info) = &mut self.info else {
            return;
        };
        if let Some((receiver, _)) = &info.reading
            && let Ok(details) = receiver.try_recv()
        {
            info.details = Some(details);
            if let Some((_, reader)) = info.reading.take() {
                let _ = reader.join();
            }
        }
        if info.read_for.as_deref() == Some(path.as_path()) || info.rewrite.is_some() {
            return;
        }
        info.read_for = Some(path.clone());
        info.details = None;
        info.editing = None;
        info.times = file_ops::file_times(&path).ok();
        let (sender, receiver) = mpsc::channel();
        let repaint = ctx.clone();
        let reader = thread::spawn(move || {
            let details = bareclip_media::read_details(&path).map_err(|error| error.to_string());
            let _ = sender.send(details);
            repaint.request_repaint();
        });
        info.reading = Some((receiver, reader));
    }

    /// Renames the file, keeping its extension, and carries on playing it under its new name.
    pub fn rename_file(&mut self, ctx: &egui::Context, new_stem: &str) {
        let Some(released) = self.release_file() else {
            return;
        };
        let path = match file_ops::rename(&released.path, new_stem) {
            Ok(path) => path,
            Err(problem) => {
                self.set_info_problem(problem);
                released.path
            }
        };
        self.reopen(ctx, &path, released.at, released.playing);
    }

    /// Sets Windows' Created or Modified date, leaving `None` as it is.
    pub fn set_dates(
        &mut self,
        ctx: &egui::Context,
        created: Option<SystemTime>,
        modified: Option<SystemTime>,
    ) {
        let Some(released) = self.release_file() else {
            return;
        };
        if let Err(error) = file_ops::set_file_times(&released.path, created, modified) {
            self.set_info_problem(format!("Windows wouldn't change the date. {error}"));
        }
        self.reopen(ctx, &released.path, released.at, released.playing);
    }

    /// Writes a copy of the file with changed tags in the background, then swaps it in, keeping
    /// Windows' Created and Modified dates.
    pub fn change_tags(&mut self, ctx: &egui::Context, changes: TagChanges) {
        let Some(path) = self.file.as_ref().map(|file| file.info.path.clone()) else {
            return;
        };
        let size = std::fs::metadata(&path).map_or(0, |metadata| metadata.len());
        let folder = path.parent().unwrap_or(Path::new("."));
        if file_ops::free_space(folder).is_some_and(|free| free < size + (1 << 20)) {
            self.set_info_problem(format!(
                "Changing this needs {} free on this drive, to write a copy first.",
                text::file_size(size)
            ));
            return;
        }
        let Some(released) = self.release_file() else {
            return;
        };
        let modified = file_ops::file_times(&released.path)
            .ok()
            .map(|times| times.1);
        let copy = file_ops::copy_path_for(&released.path);
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel();
        let (source, shown, stop, repaint) = (
            released.path.clone(),
            progress.clone(),
            cancel.clone(),
            ctx.clone(),
        );
        thread::spawn(move || {
            let result =
                bareclip_media::write_with_tags(&source, &copy, &changes, &stop, |share| {
                    shown.store((share * 1000.0) as u32, Ordering::Relaxed);
                    repaint.request_repaint();
                })
                .map_err(|error| match error {
                    bareclip_media::MediaError::Cancelled => "Cancelled.".to_owned(),
                    error => format!("The file couldn't be changed. {error}"),
                })
                .and_then(|()| {
                    file_ops::swap_in(&source, &copy).map_err(|error| {
                        let _ = std::fs::remove_file(&copy);
                        format!("The changed copy couldn't take the file's place. {error}")
                    })
                })
                .and_then(|()| {
                    // The rewrite is not a change anyone made to the video, so it keeps its date.
                    file_ops::set_file_times(&source, None, modified)
                        .map_err(|error| format!("The Modified date couldn't be put back. {error}"))
                });
            let _ = sender.send(result);
            repaint.request_repaint();
        });
        if let Some(info) = &mut self.info {
            info.rewrite = Some(Rewrite {
                receiver,
                progress,
                cancel,
                released,
                big: size >= SHOW_PROGRESS_FROM,
            });
        }
    }

    /// Takes in a finished rewrite and opens the file again.
    fn finish_rewrite(&mut self, ctx: &egui::Context) {
        let Some(info) = &mut self.info else {
            return;
        };
        let Some(rewrite) = &info.rewrite else {
            return;
        };
        let Ok(result) = rewrite.receiver.try_recv() else {
            return;
        };
        let Some(rewrite) = info.rewrite.take() else {
            return;
        };
        info.read_for = None;
        if let Err(problem) = result {
            info.problem = Some(problem);
        }
        let released = rewrite.released;
        self.reopen(ctx, &released.path, released.at, released.playing);
    }

    #[cfg(test)]
    pub fn is_rewriting(&self) -> bool {
        self.info.as_ref().is_some_and(InfoPanel::is_rewriting)
    }

    fn set_info_problem(&mut self, problem: String) {
        match &mut self.info {
            Some(info) => info.problem = Some(problem),
            None => self.notice = Some(problem),
        }
    }

    /// Asks before moving the file to the Recycle Bin.
    pub fn ask_to_delete(&mut self) {
        if self.file.is_some() {
            self.confirm_delete = true;
        }
    }

    /// Moves the file to the Recycle Bin, then shows the next file in the folder, or the one
    /// before if it was the last, or closes the window if none is left.
    pub fn delete_file(&mut self, ctx: &egui::Context) {
        self.confirm_delete = false;
        let next = self.next_file().or_else(|| self.previous_file());
        let Some(released) = self.release_file() else {
            return;
        };
        if let Err(problem) = (self.trash_file)(&released.path) {
            self.reopen(ctx, &released.path, released.at, released.playing);
            self.notice = Some(format!("It couldn't go to the Recycle Bin. {problem}"));
            return;
        }
        self.folder.retain(|file| file != &released.path);
        match next {
            Some(next) => self.open(ctx, &next),
            None => {
                self.picture = None;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    /// The question before deleting.
    pub(super) fn delete_dialog(&mut self, ctx: &egui::Context, palette: &Palette) {
        if !self.confirm_delete {
            return;
        }
        let name = self.name();
        let mut delete = false;
        let mut cancel = false;
        egui::Modal::new(egui::Id::unique("viewer_delete"))
            .frame(crate::editor::dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width(440.0);
                components::dialog_title(
                    ui,
                    &icons::TRASH_2,
                    palette.danger,
                    &format!("Move {name} to the Recycle Bin?"),
                );
                ui.add_space(10.0);
                ui.colored_label(
                    palette.text_muted,
                    "Projects that use it will show it as missing.",
                );
                ui.add_space(18.0);
                components::dialog_buttons(ui, |ui| {
                    delete = ui
                        .add(ActionButton::new(
                            palette,
                            Kind::Danger,
                            "Move to Recycle Bin",
                        ))
                        .clicked();
                    cancel = ui
                        .add(ActionButton::new(palette, Kind::Secondary, "Cancel"))
                        .clicked();
                });
            });
        if cancel || ctx.input(|input| input.key_pressed(Key::Escape)) {
            self.confirm_delete = false;
        } else if delete {
            self.delete_file(ctx);
        }
    }

    /// The panel itself.
    pub(super) fn info_panel(&mut self, ui: &mut egui::Ui, palette: &Palette, rect: Rect) {
        let ctx = ui.ctx().clone();
        self.finish_rewrite(&ctx);
        self.read_info(&ctx);
        ui.painter().rect_filled(rect, 0.0, palette.surface);
        ui.painter().vline(
            rect.left(),
            rect.y_range(),
            Stroke::new(1.0, palette.border),
        );
        let mut close = false;
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(rect.shrink2(vec2(18.0, 14.0)))
                .layout(Layout::top_down(Align::Min)),
            |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Info")
                            .font(fonts::strong(18.0))
                            .color(palette.text),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        close = ui
                            .add(IconButton::new(
                                palette,
                                Kind::Ghost,
                                icons::X,
                                "Close info",
                            ))
                            .clicked();
                    });
                });
                ui.add_space(8.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.info_contents(ui, palette));
            },
        );
        if close {
            self.toggle_info();
        }
    }

    fn info_contents(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let ctx = ui.ctx().clone();
        let path = self.current_path().map(Path::to_owned);
        if let Some(rewrite) = self.info.as_ref().and_then(|info| info.rewrite.as_ref()) {
            ui.label(RichText::new("Saving the change…").color(palette.text));
            if rewrite.big {
                let share = rewrite.progress.load(Ordering::Relaxed) as f32 / 1000.0;
                components::progress_bar(ui, palette, share);
            }
            if ui
                .add(ActionButton::new(palette, Kind::Secondary, "Cancel").small())
                .clicked()
            {
                rewrite.cancel.store(true, Ordering::Relaxed);
            }
            return;
        }
        let Some(path) = path else {
            return;
        };
        ui.horizontal(|ui| {
            if ui
                .add(
                    ActionButton::new(palette, Kind::Secondary, "Copy path")
                        .icon(icons::COPY)
                        .small(),
                )
                .clicked()
            {
                ctx.copy_text(path.display().to_string());
            }
            if ui
                .add(
                    ActionButton::new(palette, Kind::Secondary, "Open location")
                        .icon(icons::FOLDER_OPEN)
                        .small(),
                )
                .clicked()
            {
                super::show_in_folder(&path);
            }
        });
        ui.add_space(12.0);
        if let Some(problem) = self.info.as_ref().and_then(|info| info.problem.clone()) {
            ui.colored_label(palette.danger, problem);
            ui.add_space(8.0);
        }

        // The name, without its extension, which stays as it is.
        heading(ui, palette, "File name");
        let stem = path
            .file_stem()
            .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
        let extension = path
            .extension()
            .map(|extension| format!(".{}", extension.to_string_lossy()))
            .unwrap_or_default();
        ui.horizontal(|ui| {
            if let Some(typed) = self.edit_field(ui, "File name", &stem, Editing::Name, |editing| {
                matches!(editing, Editing::Name(_))
            }) && typed.trim() != stem
            {
                self.rename_file(&ctx, &typed);
            }
            ui.colored_label(palette.text_muted, extension);
        });
        ui.add_space(10.0);

        let times = self.info.as_ref().and_then(|info| info.times);
        heading(ui, palette, "Date and time");
        if let Some((created, modified)) = times {
            for (label, moment, made) in [
                (
                    "Created",
                    created,
                    Editing::Created as fn(String) -> Editing,
                ),
                ("Modified", modified, Editing::Modified),
            ] {
                ui.colored_label(palette.text_muted, label);
                let shown = file_ops::local_parts(moment).to_typed();
                let is_this = |editing: &Editing| {
                    std::mem::discriminant(editing) == std::mem::discriminant(&made(String::new()))
                };
                if let Some(typed) = self.edit_field(ui, label, &shown, made, is_this) {
                    match DateTime::parse(&typed).and_then(file_ops::from_local) {
                        Some(moment) if label == "Created" => {
                            self.set_dates(&ctx, Some(moment), None);
                        }
                        Some(moment) => self.set_dates(&ctx, None, Some(moment)),
                        None => self.set_info_problem(date_hint()),
                    }
                }
            }
        }
        ui.add_space(10.0);

        let details = self.info.as_ref().and_then(|info| info.details.clone());
        let Some(details) = details else {
            ui.colored_label(palette.text_muted, "Reading…");
            return;
        };
        let details = match details {
            Ok(details) => details,
            Err(problem) => {
                ui.colored_label(palette.danger, problem);
                return;
            }
        };
        let support = bareclip_media::tag_support(&path);
        heading(ui, palette, "Artist");
        let artist = details.tag("artist").unwrap_or("").to_owned();
        if support.artist {
            if let Some(typed) =
                self.edit_field(ui, "Artist", &artist, Editing::Artist, |editing| {
                    matches!(editing, Editing::Artist(_))
                })
                && typed.trim() != artist
            {
                self.change_tags(
                    &ctx,
                    TagChanges {
                        artist: Some(typed.trim().to_owned()),
                        creation_time: None,
                    },
                );
            }
        } else {
            value(
                ui,
                palette,
                if artist.is_empty() { "None" } else { &artist },
            );
            note(
                ui,
                palette,
                "This kind of file's tags can't be changed here.",
            );
        }
        ui.add_space(10.0);

        heading(ui, palette, "Made, as the file says");
        let made = details
            .tag("creation_time")
            .and_then(read_creation_time)
            .map(|moment| file_ops::local_parts(moment).to_typed())
            .unwrap_or_default();
        if support.creation_time {
            if let Some(typed) = self.edit_field(ui, "Made", &made, Editing::Made, |editing| {
                matches!(editing, Editing::Made(_))
            }) && typed.trim() != made
            {
                let changes = if typed.trim().is_empty() {
                    Some(String::new())
                } else {
                    DateTime::parse(&typed)
                        .and_then(file_ops::from_local)
                        .map(creation_time_text)
                };
                match changes {
                    Some(creation_time) => self.change_tags(
                        &ctx,
                        TagChanges {
                            artist: None,
                            creation_time: Some(creation_time),
                        },
                    ),
                    None => self.set_info_problem(date_hint()),
                }
            }
        } else {
            value(
                ui,
                palette,
                if made.is_empty() { "Not given" } else { &made },
            );
        }
        ui.add_space(10.0);

        heading(ui, palette, "Size info");
        let mut size_line = vec![text::file_size(details.size_bytes)];
        size_line.push(text::clip_length(details.duration));
        if let Some(rate) = details.bit_rate {
            size_line.push(bit_rate(rate));
        }
        value(ui, palette, &size_line.join("   "));
        value(ui, palette, &details.container);
        ui.add_space(10.0);

        heading(ui, palette, "Location");
        if let Some(folder) = path.parent() {
            value(ui, palette, &folder.display().to_string());
        }
        ui.add_space(10.0);

        let mut sound_number = 0;
        for stream in &details.streams {
            match stream {
                StreamDetails::Picture {
                    codec,
                    profile,
                    width,
                    height,
                    frame_rate,
                    variable_rate,
                    bit_depth,
                    colours,
                    hdr,
                    bit_rate: rate,
                } => {
                    heading(ui, palette, "Picture");
                    value(ui, palette, &format!("{width} x {height}"));
                    let mut rate_line = format!("{frame_rate:.2} frames a second");
                    if *variable_rate {
                        rate_line.push_str(", varying");
                    }
                    value(ui, palette, &rate_line);
                    let mut codec_line = text::codec_name(codec);
                    if let Some(profile) = profile {
                        codec_line.push_str(&format!(", {profile}"));
                    }
                    value(ui, palette, &codec_line);
                    let mut colour_line = Vec::new();
                    if let Some(depth) = bit_depth {
                        colour_line.push(format!("{depth}-bit"));
                    }
                    if let Some(colours) = colours {
                        colour_line.push(colours.clone());
                    }
                    colour_line.push(hdr.unwrap_or("Not HDR").to_owned());
                    value(ui, palette, &colour_line.join(", "));
                    if let Some(rate) = rate {
                        value(ui, palette, &bit_rate(*rate));
                    }
                    ui.add_space(10.0);
                }
                StreamDetails::Sound {
                    codec,
                    profile,
                    channels,
                    layout,
                    sample_rate,
                    bit_rate: rate,
                    name,
                    language,
                } => {
                    sound_number += 1;
                    heading(ui, palette, &format!("Sound {sound_number}"));
                    if let Some(name) = name {
                        value(ui, palette, name);
                    }
                    let mut codec_line = text::codec_name(codec);
                    if let Some(profile) = profile {
                        codec_line.push_str(&format!(", {profile}"));
                    }
                    value(ui, palette, &codec_line);
                    let shape = layout
                        .clone()
                        .unwrap_or_else(|| format!("{channels} channels"));
                    value(
                        ui,
                        palette,
                        &format!("{shape}, {:.1} kHz", f64::from(*sample_rate) / 1000.0),
                    );
                    if let Some(rate) = rate {
                        value(ui, palette, &bit_rate(*rate));
                    }
                    if let Some(language) = language {
                        value(ui, palette, &format!("Language: {language}"));
                    }
                    ui.add_space(10.0);
                }
            }
        }
        if details.subtitles > 0 || details.chapters > 0 {
            heading(ui, palette, "Subtitles and chapters");
            value(
                ui,
                palette,
                &format!(
                    "{} subtitle tracks, {} chapters",
                    details.subtitles, details.chapters
                ),
            );
            ui.add_space(10.0);
        }
        let others: Vec<&(String, String)> = details
            .tags
            .iter()
            .filter(|(key, _)| {
                !key.eq_ignore_ascii_case("artist") && !key.eq_ignore_ascii_case("creation_time")
            })
            .collect();
        if !others.is_empty() {
            heading(ui, palette, "Other tags");
            for (key, tag) in others {
                value(ui, palette, &format!("{key}: {tag}"));
            }
        }
    }

    /// A box that shows a value and takes a new one, given back on Enter or clicking away.
    /// Escape puts the old value back.
    fn edit_field(
        &mut self,
        ui: &mut egui::Ui,
        name: &str,
        current: &str,
        make: impl Fn(String) -> Editing,
        is_this: impl Fn(&Editing) -> bool,
    ) -> Option<String> {
        let info = self.info.as_mut()?;
        let editing_this = info.editing.as_ref().is_some_and(&is_this);
        let mut typed = match &info.editing {
            Some(
                Editing::Name(typed)
                | Editing::Created(typed)
                | Editing::Modified(typed)
                | Editing::Artist(typed)
                | Editing::Made(typed),
            ) if editing_this => typed.clone(),
            _ => current.to_owned(),
        };
        // The box takes the Escape key for itself, so it is read first.
        let escaped = ui.input(|input| input.key_pressed(Key::Escape));
        let response = ui.add(
            egui::TextEdit::singleline(&mut typed)
                .id(egui::Id::unique(("viewer_info_field", name)))
                .desired_width(PANEL_WIDTH - 90.0),
        );
        response.widget_info(|| {
            let mut info = egui::WidgetInfo::text_edit(true, current, &typed, "");
            info.label = Some(name.to_owned());
            info
        });
        // Escape puts the old value back. The box may only report losing focus a frame later,
        // by when nothing is being typed, so nothing is changed then.
        if escaped && (editing_this || response.has_focus() || response.lost_focus()) {
            info.editing = None;
            response.surrender_focus();
            return None;
        }
        if response.gained_focus() || response.changed() {
            info.problem = None;
            info.editing = Some(make(typed.clone()));
        }
        if response.lost_focus() && editing_this {
            info.editing = None;
            return Some(typed);
        }
        None
    }
}

fn date_hint() -> String {
    "Type a date and time like 2025-09-14 01:35.".to_owned()
}

fn bit_rate(rate: u64) -> String {
    if rate >= 1_000_000 {
        format!("{:.1} Mb/s", rate as f64 / 1_000_000.0)
    } else {
        format!("{} kb/s", rate / 1000)
    }
}

fn heading(ui: &mut egui::Ui, palette: &Palette, text: &str) {
    ui.label(
        RichText::new(text)
            .font(fonts::strong(13.0))
            .color(palette.text),
    );
}

fn value(ui: &mut egui::Ui, palette: &Palette, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).size(12.5).color(palette.text_muted)).wrap());
}

fn note(ui: &mut egui::Ui, palette: &Palette, text: &str) {
    ui.add(
        egui::Label::new(
            RichText::new(text)
                .size(11.0)
                .italics()
                .color(palette.text_muted),
        )
        .wrap(),
    );
}
