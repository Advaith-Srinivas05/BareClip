//! The notes that show over the top of the picture: a picture saved, a trim saved, where a file
//! picked up, and problems. They all look the same and stack down from the top, so two at once
//! never cover each other.

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Layout, Rect, RichText, Stroke, UiBuilder, pos2,
    vec2,
};

use crate::{
    components::{ActionButton, IconButton, Kind},
    icons::{self, Icon},
    theme::Palette,
};

/// How tall a note is.
pub const NOTE_HEIGHT: f32 = 44.0;
/// The room above the first note, and between notes.
pub const NOTE_GAP: f32 = 12.0;
/// The widest a note gets.
const NOTE_MOST_WIDTH: f32 = 560.0;
const NOTE_RADIUS: u8 = 10;
const NOTE_PADDING: f32 = 14.0;
const WORDS_SIZE: f32 = 13.0;
const SYMBOL_SIZE: f32 = 16.0;
const ITEM_GAP: f32 = 8.0;
/// The close button's side.
const CLOSE_SIZE: f32 = 28.0;

/// What was pressed on a note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotePress {
    Nothing,
    /// One of its buttons, counted from the first given.
    Action(usize),
    Close,
}

/// A note: a symbol, a line of words, and buttons on its right.
pub struct Note<'a> {
    icon: &'a Icon,
    tint: Color32,
    words: &'a str,
    hover: Option<&'a str>,
    actions: &'a [&'a str],
    closable: bool,
}

impl<'a> Note<'a> {
    pub fn new(icon: &'a Icon, tint: Color32, words: &'a str) -> Self {
        Self {
            icon,
            tint,
            words,
            hover: None,
            actions: &[],
            closable: false,
        }
    }

    /// More about it, shown while the mouse rests on the words.
    pub fn hover(mut self, hover: &'a str) -> Self {
        self.hover = Some(hover);
        self
    }

    pub fn actions(mut self, actions: &'a [&'a str]) -> Self {
        self.actions = actions;
        self
    }

    pub fn closable(mut self) -> Self {
        self.closable = true;
        self
    }

    /// How wide everything but the words is.
    fn fixed_width(&self, ui: &egui::Ui) -> f32 {
        let actions: f32 = self
            .actions
            .iter()
            // A small button is its words plus 8 points either side.
            .map(|action| measure(ui, action, 12.0, f32::INFINITY).x + 16.0 + ITEM_GAP)
            .sum();
        let close = if self.closable {
            CLOSE_SIZE + ITEM_GAP
        } else {
            0.0
        };
        NOTE_PADDING * 2.0 + SYMBOL_SIZE + ITEM_GAP * 2.0 + actions + close
    }

    /// Shows the note in the middle of `area`, at `top`, and moves `top` down past it. Words
    /// that don't fit on one line go onto more, and the note grows to hold them.
    pub fn show(
        self,
        ui: &mut egui::Ui,
        palette: &Palette,
        area: Rect,
        top: &mut f32,
    ) -> NotePress {
        let fixed = self.fixed_width(ui);
        let one_line = measure(ui, self.words, WORDS_SIZE, f32::INFINITY).x;
        let width = (fixed + one_line)
            .min(NOTE_MOST_WIDTH)
            .min(area.width() - NOTE_GAP * 2.0)
            .max(120.0);
        let words_height = measure(ui, self.words, WORDS_SIZE, (width - fixed).max(40.0)).y;
        let height = NOTE_HEIGHT.max(words_height + NOTE_PADDING * 2.0);
        let rect = Rect::from_min_size(
            pos2(area.center().x - width / 2.0, *top),
            vec2(width, height),
        );
        *top = rect.bottom() + NOTE_GAP;
        card(ui, palette, rect);
        let mut pressed = NotePress::Nothing;
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(rect.shrink2(vec2(NOTE_PADDING, 0.0)))
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = ITEM_GAP;
                let (symbol, _) =
                    ui.allocate_exact_size(vec2(SYMBOL_SIZE, SYMBOL_SIZE), egui::Sense::hover());
                icons::paint(ui.painter(), symbol, self.icon, self.tint);
                // The buttons go in from the right first, so the words take what is left.
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = ITEM_GAP;
                    if self.closable
                        && ui
                            .add(
                                IconButton::new(palette, Kind::Ghost, icons::X, "Close note")
                                    .size(CLOSE_SIZE, 14.0),
                            )
                            .clicked()
                    {
                        pressed = NotePress::Close;
                    }
                    for (index, action) in self.actions.iter().enumerate().rev() {
                        if ui
                            .add(ActionButton::new(palette, Kind::Secondary, action).small())
                            .clicked()
                        {
                            pressed = NotePress::Action(index);
                        }
                    }
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        let words = ui.add(
                            egui::Label::new(
                                RichText::new(self.words)
                                    .size(WORDS_SIZE)
                                    .color(palette.text),
                            )
                            .wrap()
                            .selectable(false),
                        );
                        if let Some(hover) = self.hover {
                            words.on_hover_text(hover);
                        }
                    });
                });
            },
        );
        pressed
    }
}

/// The size of some words, wrapped at `wrap` points.
fn measure(ui: &egui::Ui, text: &str, size: f32, wrap: f32) -> egui::Vec2 {
    ui.painter()
        .layout(
            text.to_owned(),
            FontId::proportional(size),
            Color32::WHITE,
            wrap,
        )
        .size()
}

/// The raised card under a note, lifted off the picture by a soft shadow and an outline.
fn card(ui: &egui::Ui, palette: &Palette, rect: Rect) {
    let radius = CornerRadius::same(NOTE_RADIUS);
    let shadow = ui.visuals().popup_shadow;
    ui.painter().add(shadow.as_shape(rect, radius));
    ui.painter().rect_filled(rect, radius, palette.raised);
    ui.painter().rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, palette.border),
        egui::StrokeKind::Inside,
    );
}
