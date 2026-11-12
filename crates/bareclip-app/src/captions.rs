//! The Captions panel: every Subtitle title in the project in order of time, whatever is selected.

use bareclip_core::{
    srt::{Cue, write_srt},
    time::format_timecode,
};

use super::*;

/// Every row is this tall, so a project with thousands of captions only lays out what shows.
const ROW_HEIGHT: f32 = 78.0;

/// One caption in the list. Its words stay in the project, so only what is on screen copies them.
#[derive(Clone, Copy, Debug)]
pub(super) struct CaptionRow {
    pub(super) clip: ClipId,
    pub(super) media: MediaId,
    pub(super) start: Ticks,
    pub(super) end: Ticks,
}

impl Editor {
    /// Every caption on the timeline, on any row, earliest first.
    pub(super) fn caption_rows(&self) -> Vec<CaptionRow> {
        let is_caption = |id: &MediaId| {
            self.project
                .titles
                .get(id)
                .is_some_and(|title| title.style == TitleStyle::Subtitle)
        };
        let mut rows: Vec<CaptionRow> = self
            .project
            .tracks
            .tracks()
            .iter()
            .flat_map(|row| row.clips().iter())
            .filter(|clip| is_caption(&clip.media))
            .map(|clip| CaptionRow {
                clip: clip.id,
                media: clip.media,
                start: clip.start,
                end: clip.end(),
            })
            .collect();
        rows.sort_by_key(|row| (row.start, row.end));
        rows
    }

    /// The project's captions as a subtitle file, at their times on the timeline.
    pub(super) fn captions_file(&self) -> String {
        let cues: Vec<Cue> = self
            .caption_rows()
            .into_iter()
            .filter_map(|row| {
                Some(Cue {
                    start: row.start,
                    end: row.end,
                    text: self.project.titles.get(&row.media)?.text.clone(),
                })
            })
            .collect();
        write_srt(&cues)
    }

    /// Asks where to save the captions, and saves them there.
    pub(super) fn save_captions(&mut self) {
        let file = self.captions_file();
        if file.is_empty() {
            self.notice = Some("None of the captions have any words to save.".to_owned());
            return;
        }
        let picked = rfd::FileDialog::new()
            .set_title("Save captions")
            .set_file_name(format!(
                "{}.{}",
                text::safe_file_name(&self.project_name),
                text::CAPTIONS_EXTENSION
            ))
            .add_filter("Subtitle files", &[text::CAPTIONS_EXTENSION])
            .save_file();
        if let Some(path) = picked
            && let Err(error) = std::fs::write(&path, file)
        {
            self.notice = Some(format!("The captions could not be saved. {error}"));
        }
    }

    /// Selects a caption and moves the playhead to it, so its picture shows.
    fn go_to_caption(&mut self, row: CaptionRow) {
        self.finish_title_edit();
        self.selected = Some(row.clip);
        self.selected_gap = None;
        self.selected_join = None;
        self.seek(row.start);
    }

    /// The panel itself, the same width as a clip's tools so moving between them moves nothing.
    pub(super) fn captions_panel(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let frame = panel_card(palette.surface, Margin::same(14))
            .corner_radius(CornerRadius::same(PANEL_RADIUS));
        self.right_side_panel(ui, "captions_panel", frame, |this, ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Captions").font(fonts::strong(15.0)));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let close = IconButton::new(palette, Kind::Ghost, icons::X, "Close captions")
                        .size(PANEL_CLOSE_SIZE, 16.0);
                    if ui.add(close).clicked() {
                        this.right_panel = None;
                    }
                });
            });
            ui.add_space(10.0);
            let rows = this.caption_rows();
            ui.horizontal(|ui| {
                let import = ActionButton::new(palette, Kind::Secondary, "Import SRT file").small();
                if ui.add(import).clicked() {
                    this.pick_captions_requested = true;
                }
                let save = ActionButton::new(palette, Kind::Secondary, "Save SRT file").small();
                if ui.add_enabled(!rows.is_empty(), save).clicked() {
                    this.save_captions_requested = true;
                }
            });
            ui.add_space(10.0);
            if rows.is_empty() {
                ui.colored_label(
                    palette.text_muted,
                    RichText::new(
                        "No captions yet. Import an SRT file, or add a Caption from Text on \
                             the left.",
                    )
                    .size(12.0),
                );
                return;
            }
            this.caption_list(ui, palette, &rows);
        });
    }

    fn caption_list(&mut self, ui: &mut egui::Ui, palette: &Palette, rows: &[CaptionRow]) {
        let focus = self.captions_focus.take();
        let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
        if let Some((_, index)) = focus {
            // Leaves the caption before it showing, so it is clear where this one came from.
            let step = ROW_HEIGHT + ui.spacing().item_spacing.y;
            area = area.vertical_scroll_offset(index.saturating_sub(1) as f32 * step);
        }
        area.show_rows(ui, ROW_HEIGHT, rows.len(), |ui, shown| {
            for index in shown {
                let row = rows[index];
                let take_focus = focus.is_some_and(|(clip, _)| clip == row.clip);
                let pressed_enter = self.caption_row(ui, palette, row, take_focus);
                if pressed_enter && let Some(next) = rows.get(index + 1) {
                    self.captions_focus = Some((next.clip, index + 1));
                }
            }
        });
    }

    /// One caption: its times and its words. Says whether Enter was pressed in its box.
    fn caption_row(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        row: CaptionRow,
        take_focus: bool,
    ) -> bool {
        let size = vec2(ui.available_width(), ROW_HEIGHT);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let start = format_timecode(row.start);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Go to caption {start}"))
        });
        if self.selected == Some(row.clip) || response.hovered() {
            ui.painter().rect_filled(rect, 6.0, palette.raised);
        }
        let position = self.position();
        let at_playhead = position >= row.start && position < row.end;
        ui.painter().text(
            rect.left_top() + vec2(8.0, 6.0),
            egui::Align2::LEFT_TOP,
            format!("{start} – {}", format_timecode(row.end)),
            FontId::new(12.0, FontFamily::Proportional),
            if at_playhead {
                palette.accent
            } else {
                palette.text_muted
            },
        );
        if response.clicked() {
            self.go_to_caption(row);
        }
        let Some(title) = self.project.titles.get(&row.media) else {
            return false;
        };
        let mut words = title.text.clone();
        // A caption of three lines or more scrolls out of its box, because rows are all one height.
        let area = egui::Rect::from_min_max(rect.min + vec2(6.0, 24.0), rect.max - vec2(6.0, 4.0));
        let mut inner = ui.new_child(UiBuilder::new().max_rect(area));
        inner.set_clip_rect(area.intersect(ui.clip_rect()));
        let field = inner.add(
            egui::TextEdit::multiline(&mut words)
                .id(egui::Id::unique(("caption-words", row.clip)))
                .desired_rows(2)
                .desired_width(area.width())
                .return_key(Some(egui::KeyboardShortcut::new(
                    Modifiers::SHIFT,
                    Key::Enter,
                ))),
        );
        field.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::TextInput, true, format!("Caption {start}"))
        });
        if take_focus {
            field.request_focus();
        }
        if field.gained_focus() {
            self.go_to_caption(row);
        }
        if field.changed() {
            self.change_title(row.media, |title| title.text = words.clone());
        }
        field.has_focus()
            && ui.input(|input| input.key_pressed(Key::Enter) && input.modifiers.is_none())
    }
}
