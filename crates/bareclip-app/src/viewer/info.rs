//! The info panel: a card that slides in from the right, as the Photos app's does, holding
//! almost everything a file says about itself. Every fact is one line, a name and a value; the
//! file's name, Windows' dates, the artist and the creation time inside the file show a pencil
//! and turn into a box on the same line when clicked. Delete is here too, because it lets go
//! of the file the same way.

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
use eframe::egui::{
    self, Align, FontId, Key, Layout, Rect, RichText, Sense, Stroke, UiBuilder, pos2, vec2,
};

use super::{Released, Viewer};
use crate::{
    components::{self, ActionButton, DateTime, IconButton, Kind},
    file_ops, fonts, icons, text,
    theme::Palette,
};

/// The details being read, and the thread reading them.
type DetailsRead = (Receiver<Result<MediaDetails, String>>, JoinHandle<()>);

/// How wide the card is, and the room it keeps from the bars, the window's edge and the picture.
pub const CARD_WIDTH: f32 = 340.0;
pub const CARD_MARGIN: f32 = 8.0;
const CARD_RADIUS: f32 = 8.0;
/// The room above and below the line that starts a part of the panel.
const SECTION_GAP: f32 = 12.0;
/// The room under a group of facts.
const FIELD_GAP: f32 = 8.0;
/// How wide the names of facts are, so what they say lines up. "Creation time in file" fits.
const FACT_NAME_WIDTH: f32 = 120.0;
/// The size of facts' names and values.
const FACT_SIZE: f32 = 12.5;
/// The pencil at the end of a fact that can be changed, and the room kept for it.
const PENCIL_SIZE: f32 = 13.0;
const PENCIL_ROOM: f32 = 22.0;
/// How tall a fact that can be changed is, at rest or as a box, so nothing moves when its box
/// opens, and how far its words sit down from its top, where the box's words go.
const EDIT_ROW_HEIGHT: f32 = 19.0;
const EDIT_INSET: f32 = 1.0;
/// The name of the time the file says it was made, which is kept inside it.
pub const MADE_LABEL: &str = "Creation time in file";
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
    /// Closed, and sliding away. It goes once it is out of sight.
    closing: bool,
    /// A box just opened, which takes the keyboard on its first frame.
    focus_pending: bool,
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

impl Editing {
    fn text(&self) -> &str {
        match self {
            Self::Name(typed)
            | Self::Created(typed)
            | Self::Modified(typed)
            | Self::Artist(typed)
            | Self::Made(typed) => typed,
        }
    }
}

impl Viewer {
    /// Whether the info panel is open, not counting one sliding away.
    pub fn is_info_open(&self) -> bool {
        self.info.as_ref().is_some_and(|info| !info.closing)
    }

    /// Opens the panel, or closes it, sliding away. Opening one that is sliding away brings it
    /// back.
    pub fn toggle_info(&mut self) {
        match &mut self.info {
            Some(info) if !info.closing => {
                info.closing = true;
                info.editing = None;
                info.stop_reading();
            }
            Some(info) => info.closing = false,
            None => self.info = Some(InfoPanel::default()),
        }
    }

    /// Lets go of a closed panel once it has slid away, unless a change to the file is still
    /// being written, which it holds.
    pub(super) fn forget_closed_info(&mut self) {
        if self
            .info
            .as_ref()
            .is_some_and(|info| info.closing && info.rewrite.is_none())
        {
            self.info = None;
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

    /// Takes in a finished rewrite and opens the file again. This runs every frame, whether or
    /// not the panel shows, so closing it or going full screen partway can't lose the file.
    pub(super) fn finish_rewrite(&mut self, ctx: &egui::Context) {
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
            Some(info) if !info.closing => info.problem = Some(problem),
            _ => self.notice = Some(problem),
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

    /// The card itself. While it slides away it still shows what it held, and takes no clicks.
    pub(super) fn info_panel(&mut self, ui: &mut egui::Ui, palette: &Palette, rect: Rect) {
        let ctx = ui.ctx().clone();
        let closing = !self.is_info_open();
        if !closing {
            self.read_info(&ctx);
        }
        ui.painter().rect_filled(rect, CARD_RADIUS, palette.surface);
        ui.painter().rect_stroke(
            rect,
            CARD_RADIUS,
            Stroke::new(1.0, palette.border),
            egui::StrokeKind::Inside,
        );
        // The card keeps clicks and the wheel off what is under it.
        let _ = ui.interact(rect, egui::Id::unique("viewer_info_card"), Sense::click());
        let path = self.current_path().map(Path::to_owned);
        let mut pressed = HeaderPress::default();
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(rect.shrink2(vec2(16.0, 12.0)))
                .layout(Layout::top_down(Align::Min)),
            |ui| {
                // One height for the line, so the heading and the buttons share a middle.
                let line = vec2(ui.available_width(), IconButton::default_size());
                ui.allocate_ui_with_layout(line, Layout::left_to_right(Align::Center), |ui| {
                    ui.label(
                        RichText::new("Info")
                            .font(fonts::strong(18.0))
                            .color(palette.text),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let button = |icon, name| IconButton::new(palette, Kind::Ghost, icon, name);
                        pressed.close = ui
                            .add(button(icons::X, "Close info"))
                            .on_hover_text("Close")
                            .clicked();
                        ui.add_space(4.0);
                        pressed.open = ui
                            .add_enabled(
                                path.is_some(),
                                button(icons::FOLDER_OPEN, "Open location"),
                            )
                            .on_hover_text("Open the file's location")
                            .clicked();
                        pressed.copy = ui
                            .add_enabled(path.is_some(), button(icons::COPY, "Copy path"))
                            .on_hover_text("Copy the file's path")
                            .clicked();
                    });
                });
                ui.add_space(8.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.info_contents(ui, palette));
            },
        );
        if pressed.close {
            // A click on × while it slides away brings it back, as Info does.
            self.toggle_info();
        } else if closing {
            return;
        }
        if let Some(path) = path {
            if pressed.copy {
                ctx.copy_text(path.display().to_string());
            }
            if pressed.open {
                super::show_in_folder(&path);
            }
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
        if let Some(problem) = self.info.as_ref().and_then(|info| info.problem.clone()) {
            ui.add(egui::Label::new(RichText::new(problem).color(palette.danger)).wrap());
            ui.add_space(8.0);
        }

        // The name, whose extension stays as it is, and where the file is.
        let file_name = path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let stem = path
            .file_stem()
            .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
        let extension = path
            .extension()
            .map(|extension| format!(".{}", extension.to_string_lossy()))
            .unwrap_or_default();
        let name_row = EditRow {
            name: "File name",
            shown: &file_name,
            starts_as: &stem,
            after_box: &extension,
            make: Editing::Name,
        };
        if let Some(typed) = self.edit_row(ui, palette, &name_row)
            && typed.trim() != stem
        {
            self.rename_file(&ctx, &typed);
        }
        if let Some(folder) = path.parent() {
            fact(ui, palette, "Folder", &folder.display().to_string());
        }

        let details = self.info.as_ref().and_then(|info| info.details.clone());
        let support = bareclip_media::tag_support(&path);
        let times = self.info.as_ref().and_then(|info| info.times);
        section(ui, palette, "Dates");
        if let Some((created, modified)) = times {
            for (name, moment, make) in [
                (
                    "Created",
                    created,
                    Editing::Created as fn(String) -> Editing,
                ),
                ("Modified", modified, Editing::Modified),
            ] {
                let shown = file_ops::local_parts(moment).to_typed();
                let row = EditRow {
                    name,
                    shown: &shown,
                    starts_as: &shown,
                    after_box: "",
                    make,
                };
                if let Some(typed) = self.edit_row(ui, palette, &row)
                    && typed.trim() != shown
                {
                    match DateTime::parse(&typed).and_then(file_ops::from_local) {
                        Some(moment) if name == "Created" => {
                            self.set_dates(&ctx, Some(moment), None);
                        }
                        Some(moment) => self.set_dates(&ctx, None, Some(moment)),
                        None => self.set_info_problem(date_hint()),
                    }
                }
            }
        }

        let details = match details {
            None => {
                ui.colored_label(palette.text_muted, "Reading…");
                return;
            }
            Some(Err(problem)) => {
                ui.colored_label(palette.danger, problem);
                return;
            }
            Some(Ok(details)) => details,
        };

        // The time the file says it was made, which lives inside it rather than in Windows.
        let made = details
            .tag("creation_time")
            .and_then(read_creation_time)
            .map(|moment| file_ops::local_parts(moment).to_typed())
            .unwrap_or_default();
        let made_shown = if made.is_empty() { "Not given" } else { &made };
        if support.creation_time {
            let row = EditRow {
                name: MADE_LABEL,
                shown: made_shown,
                starts_as: &made,
                after_box: "",
                make: Editing::Made,
            };
            if let Some(typed) = self.edit_row(ui, palette, &row)
                && typed.trim() != made
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
            fact(ui, palette, MADE_LABEL, made_shown);
        }

        section(ui, palette, "Tags");
        let artist = details.tag("artist").unwrap_or("").to_owned();
        let artist_shown = if artist.is_empty() { "None" } else { &artist };
        if support.artist {
            let row = EditRow {
                name: "Artist",
                shown: artist_shown,
                starts_as: &artist,
                after_box: "",
                make: Editing::Artist,
            };
            if let Some(typed) = self.edit_row(ui, palette, &row)
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
            fact(ui, palette, "Artist", artist_shown);
            note(
                ui,
                palette,
                "This kind of file's tags can't be changed here.",
            );
        }
        let others: Vec<&(String, String)> = details
            .tags
            .iter()
            .filter(|(key, _)| {
                !key.eq_ignore_ascii_case("artist") && !key.eq_ignore_ascii_case("creation_time")
            })
            .collect();
        for (key, tag) in others {
            fact(ui, palette, key, tag);
        }

        section(ui, palette, "File");
        fact(ui, palette, "Size", &text::file_size(details.size_bytes));
        fact(ui, palette, "Length", &text::clip_length(details.duration));
        if let Some(rate) = details.bit_rate {
            fact(ui, palette, "Bit rate", &bit_rate(rate));
        }
        fact(ui, palette, "Format", &details.container);

        let sounds = details
            .streams
            .iter()
            .filter(|stream| matches!(stream, StreamDetails::Sound { .. }))
            .count();
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
                    section(ui, palette, "Picture");
                    fact(ui, palette, "Size", &format!("{width} \u{d7} {height}"));
                    let mut rate_line = format!("{frame_rate:.2} a second");
                    if *variable_rate {
                        rate_line.push_str(", varying");
                    }
                    fact(ui, palette, "Frame rate", &rate_line);
                    let mut codec_line = text::codec_name(codec);
                    if let Some(profile) = profile {
                        codec_line.push_str(&format!(", {profile}"));
                    }
                    fact(ui, palette, "Codec", &codec_line);
                    let mut colour_line = Vec::new();
                    if let Some(depth) = bit_depth {
                        colour_line.push(format!("{depth}-bit"));
                    }
                    if let Some(colours) = colours {
                        colour_line.push(colours.clone());
                    }
                    colour_line.push(hdr.unwrap_or("Not HDR").to_owned());
                    fact(ui, palette, "Colour", &colour_line.join(", "));
                    if let Some(rate) = rate {
                        fact(ui, palette, "Bit rate", &bit_rate(*rate));
                    }
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
                    if sounds > 1 {
                        section(ui, palette, &format!("Sound {sound_number}"));
                    } else {
                        section(ui, palette, "Sound");
                    }
                    if let Some(name) = name {
                        fact(ui, palette, "Name", name);
                    }
                    let mut codec_line = text::codec_name(codec);
                    if let Some(profile) = profile {
                        codec_line.push_str(&format!(", {profile}"));
                    }
                    fact(ui, palette, "Codec", &codec_line);
                    let shape = layout
                        .clone()
                        .unwrap_or_else(|| format!("{channels} channels"));
                    fact(
                        ui,
                        palette,
                        "Channels",
                        &format!("{shape}, {:.1} kHz", f64::from(*sample_rate) / 1000.0),
                    );
                    if let Some(rate) = rate {
                        fact(ui, palette, "Bit rate", &bit_rate(*rate));
                    }
                    if let Some(language) = language {
                        fact(ui, palette, "Language", language);
                    }
                }
            }
        }
        if details.subtitles > 0 || details.chapters > 0 {
            section(ui, palette, "Subtitles and chapters");
            fact(ui, palette, "Subtitles", &details.subtitles.to_string());
            fact(ui, palette, "Chapters", &details.chapters.to_string());
        }
        ui.add_space(FIELD_GAP);
    }

    /// A fact that can be changed. At rest it looks like every other fact, with a faint pencil
    /// at its end; a click, or Enter or Space on it, turns its value into a box on the same
    /// line. What was typed is given back on Enter or clicking away, and Escape puts the old
    /// value back.
    fn edit_row(&mut self, ui: &mut egui::Ui, palette: &Palette, row: &EditRow) -> Option<String> {
        let closing = !self.is_info_open();
        let info = self.info.as_mut()?;
        let this = std::mem::discriminant(&(row.make)(String::new()));
        let editing = info
            .editing
            .as_ref()
            .filter(|editing| std::mem::discriminant(*editing) == this)
            .map(|editing| editing.text().to_owned());
        let Some(mut typed) = editing else {
            let response = changeable_fact(ui, palette, row.name, row.shown);
            if response.clicked() && !closing {
                info.problem = None;
                info.editing = Some((row.make)(row.starts_as.to_owned()));
                info.focus_pending = true;
            }
            return None;
        };
        // The box takes the Escape key for itself, so it is read first.
        let escaped = ui.input(|input| input.key_pressed(Key::Escape));
        let mut result = None;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.spacing_mut().interact_size.y = EDIT_ROW_HEIGHT;
            ui.vertical(|ui| {
                ui.add_space(EDIT_INSET);
                fact_name(ui, palette, row.name);
            });
            let after_width = if row.after_box.is_empty() {
                0.0
            } else {
                ui.painter()
                    .layout_no_wrap(
                        row.after_box.to_owned(),
                        FontId::proportional(FACT_SIZE),
                        palette.text_muted,
                    )
                    .size()
                    .x
                    + ui.spacing().item_spacing.x
            };
            let response = ui.add(
                egui::TextEdit::singleline(&mut typed)
                    .id(egui::Id::unique(("viewer_info_field", row.name)))
                    .font(FontId::proportional(FACT_SIZE))
                    .margin(vec2(4.0, EDIT_INSET))
                    .desired_width((ui.available_width() - after_width).max(40.0)),
            );
            response.widget_info(|| {
                let mut widget = egui::WidgetInfo::text_edit(true, row.starts_as, &typed, "");
                widget.label = Some(row.name.to_owned());
                widget
            });
            if !row.after_box.is_empty() {
                ui.colored_label(palette.text_muted, row.after_box);
            }
            if std::mem::take(&mut info.focus_pending) {
                response.request_focus();
            }
            if escaped {
                info.editing = None;
                response.surrender_focus();
                return;
            }
            if response.changed() {
                info.problem = None;
                info.editing = Some((row.make)(typed.clone()));
            }
            if response.lost_focus() {
                info.editing = None;
                result = Some(typed);
            }
        });
        ui.add_space(3.0);
        result
    }
}

/// A fact that can be changed: its name, what it shows at rest, what its box starts with, and
/// anything kept after the box, such as the file's extension.
struct EditRow<'a> {
    name: &'a str,
    shown: &'a str,
    starts_as: &'a str,
    after_box: &'a str,
    make: fn(String) -> Editing,
}

/// What was pressed on the card's heading line.
#[derive(Clone, Copy, Debug, Default)]
struct HeaderPress {
    copy: bool,
    open: bool,
    close: bool,
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

/// The start of a part of the panel: a line across it and a small heading.
fn section(ui: &mut egui::Ui, palette: &Palette, text: &str) {
    ui.add_space(SECTION_GAP);
    let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().hline(
        line.x_range(),
        line.center().y,
        Stroke::new(1.0, palette.border),
    );
    ui.add_space(SECTION_GAP);
    ui.label(
        RichText::new(text.to_uppercase())
            .font(fonts::strong(11.0))
            .color(palette.text_muted),
    );
    ui.add_space(6.0);
}

/// A fact's name, in a column of one width so what the facts say lines up.
fn fact_name(ui: &mut egui::Ui, palette: &Palette, name: &str) {
    ui.allocate_ui_with_layout(
        vec2(FACT_NAME_WIDTH, 18.0),
        Layout::left_to_right(Align::Min),
        |ui| {
            ui.set_min_width(FACT_NAME_WIDTH);
            ui.add(
                egui::Label::new(
                    RichText::new(name)
                        .size(FACT_SIZE)
                        .color(palette.text_muted),
                )
                .truncate(),
            );
        },
    );
}

/// One fact about the file: its name on the left and what it is beside it, going onto more
/// lines when long.
fn fact(ui: &mut egui::Ui, palette: &Palette, name: &str, text: &str) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        fact_name(ui, palette, name);
        ui.add(egui::Label::new(RichText::new(text).size(FACT_SIZE).color(palette.text)).wrap());
    });
    ui.add_space(3.0);
}

/// A fact that can be changed, at rest: drawn just as [`fact`] draws one, with a faint pencil
/// at its end, and the whole line to press. Under the mouse the line lights up a little.
fn changeable_fact(ui: &mut egui::Ui, palette: &Palette, name: &str, text: &str) -> egui::Response {
    let width = ui.available_width();
    let value_left = FACT_NAME_WIDTH + 8.0;
    let room = (width - value_left - PENCIL_ROOM).max(20.0);
    let value = ui.painter().layout(
        text.to_owned(),
        FontId::proportional(FACT_SIZE),
        palette.text,
        room,
    );
    let mut job = egui::text::LayoutJob::simple_singleline(
        name.to_owned(),
        FontId::proportional(FACT_SIZE),
        palette.text_muted,
    );
    job.wrap.max_width = FACT_NAME_WIDTH;
    job.wrap.max_rows = 1;
    job.wrap.overflow_character = Some('…');
    let label = ui.painter().layout_job(job);
    let height = (value.size().y.max(label.size().y) + EDIT_INSET * 2.0).max(EDIT_ROW_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
    let lit = response.hovered() || response.has_focus();
    if lit {
        ui.painter().rect_filled(
            rect.expand2(vec2(6.0, 2.0)),
            4.0,
            palette.text.gamma_multiply(0.06),
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
    }
    let top = rect.top() + EDIT_INSET;
    ui.painter()
        .galley(pos2(rect.left(), top), label, palette.text_muted);
    ui.painter()
        .galley(pos2(rect.left() + value_left, top), value, palette.text);
    let pencil = icons::square_at(
        pos2(
            rect.right() - PENCIL_SIZE / 2.0 - 2.0,
            top + FACT_SIZE * 0.65,
        ),
        PENCIL_SIZE,
    );
    let colour = if lit {
        palette.text
    } else {
        palette.text_muted.gamma_multiply(0.75)
    };
    icons::paint(ui.painter(), pencil, &icons::PENCIL, colour);
    ui.add_space(3.0);
    response.on_hover_text("Click to change")
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
