//! BareClip's shared controls, so every button looks and reacts the same way.
//! Symbols alone are used where their meaning is universal, such as playback and zoom.
//! Actions where a wrong guess costs something also carry their name.

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontFamily, FontId, Layout, Rect, Response, Sense,
    Stroke, StrokeKind, Ui, Widget, pos2, vec2,
};

use bareclip_core::canvas::{BACKGROUND_SWATCHES, Colour, colour_from_hex, colour_to_hex};

use crate::{
    fonts,
    icons::{self, Icon},
    theme::Palette,
};

const RADIUS: u8 = 6;
const SWATCH_SIZE: f32 = 26.0;
const BUTTON_HEIGHT: f32 = 32.0;
const SMALL_BUTTON_HEIGHT: f32 = 26.0;
const BUTTON_PADDING: f32 = 12.0;
const SMALL_BUTTON_PADDING: f32 = 8.0;
pub const NAME_BAR_PADDING: f32 = 8.0;
pub const NAME_BAR_HEIGHT: f32 = 32.0;
/// The same size as the app's name beside it.
pub const NAME_BAR_TEXT: f32 = 15.0;
const ICON_GAP: f32 = 6.0;
const FOCUS_RING_GAP: f32 = 2.0;
const DIALOG_BUTTON_SPACING: f32 = 8.0;
const GHOST_HOVER: f32 = 0.09;
const GHOST_PRESSED: f32 = 0.16;
const PRESSED_SHIFT: f32 = 0.16;
const HOVER_SHIFT: f32 = 0.1;
const CARD_PADDING: f32 = 12.0;
const CARD_ICON_SIZE: f32 = 18.0;
const CARD_ICON_GAP: f32 = 10.0;
const PROGRESS_HEIGHT: f32 = 8.0;
const RAIL_TAB_HEIGHT: f32 = 58.0;
/// How large a rail tab's symbol is, and the room between it and its name.
const RAIL_SYMBOL: f32 = 20.0;
const RAIL_SYMBOL_GAP: f32 = 5.0;
/// The room kept clear inside a rail tab's edges.
const RAIL_LABEL_INSET: f32 = 3.0;
const CHECK_BOX_SIZE: f32 = 20.0;
const MENU_ITEM_HEIGHT: f32 = 30.0;
const MENU_WIDTH: f32 = 210.0;
const MENU_ITEM_PADDING: f32 = 8.0;
/// How many lines a long name in a menu takes before it is cut short.
const MENU_ITEM_ROWS: usize = 2;
const MENU_ICON_SIZE: f32 = 16.0;
const MENU_ARROW_SIZE: f32 = 14.0;
/// The room between a menu line's name and its shortcut or arrow.
const MENU_SHORTCUT_GAP: f32 = 12.0;
/// The room for the drawing of a shape on a line of a menu.
const MENU_SHAPE_ROOM: egui::Vec2 = vec2(18.0, 16.0);
/// How long the pointer rests on something before its note shows, and how the note arrives.
const NOTE_REST_SECONDS: f64 = 0.5;
const NOTE_FADE_SECONDS: f32 = 0.15;
const NOTE_RISE: f32 = 3.0;
/// How far a note sits from the line it explains, clear of the menu's frame.
const NOTE_OFFSET: f32 = 14.0;
const NOTE_PADDING: egui::Vec2 = vec2(8.0, 5.0);
const SETTINGS_CARD_RADIUS: u8 = 8;
const SETTINGS_ROW_PADDING: f32 = 12.0;
const SETTINGS_ROW_HEIGHT: f32 = 44.0;
/// A row in a recording window, where height is short.
const COMPACT_ROW_HEIGHT: f32 = 36.0;
const SPOT_BUTTON_SIZE: f32 = 26.0;
const SETTINGS_ROW_WITH_DETAIL_HEIGHT: f32 = 54.0;
const SWITCH_SIZE: egui::Vec2 = vec2(34.0, 18.0);
/// The information symbol after a switch row's name.
const INFO_SIZE: f32 = 14.0;
const SWITCH_KNOB_RADIUS: f32 = 6.0;
const SWITCH_SECONDS: f32 = 0.12;
const SIDE_TAB_HEIGHT: f32 = 36.0;
/// How wide a level meter is when it sits beside a switch.
pub const METER_BESIDE_SWITCH: f32 = 120.0;

/// How much attention a button asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The main action of its area. Use one at a time where possible.
    Primary,
    /// An ordinary action, with its own outline.
    Secondary,
    /// A quiet action that sits among others in a toolbar.
    Ghost,
    /// An action that removes something.
    Danger,
    /// The choice currently selected in a group.
    Chosen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Rest,
    Hover,
    Pressed,
    Disabled,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Look {
    fill: Color32,
    stroke: Stroke,
    content: Color32,
}

/// A kind's look, with the symbol in `tint` when one is given and the button isn't disabled.
fn tinted_look(palette: &Palette, kind: Kind, state: State, tint: Option<Color32>) -> Look {
    let mut look = look(palette, kind, state);
    if let Some(tint) = tint
        && state != State::Disabled
    {
        look.content = tint;
    }
    look
}

fn look(palette: &Palette, kind: Kind, state: State) -> Look {
    let plain = |fill, stroke, content| Look {
        fill,
        stroke,
        content,
    };
    if state == State::Disabled {
        let outline = match kind {
            Kind::Ghost => Stroke::NONE,
            _ => Stroke::new(1.0, palette.border),
        };
        return plain(Color32::TRANSPARENT, outline, palette.border_strong);
    }
    match kind {
        Kind::Primary => plain(
            match state {
                State::Hover => palette.accent_hover,
                // Toward the theme's main text colour, which gains contrast in both themes.
                State::Pressed => palette
                    .accent_hover
                    .lerp_to_gamma(palette.text, PRESSED_SHIFT),
                _ => palette.accent,
            },
            Stroke::NONE,
            palette.accent_text,
        ),
        Kind::Secondary => plain(
            match state {
                State::Pressed => palette.surface,
                _ => palette.raised,
            },
            Stroke::new(
                1.0,
                match state {
                    State::Hover => palette.border_strong,
                    State::Pressed => palette.accent,
                    _ => palette.border,
                },
            ),
            palette.text,
        ),
        Kind::Ghost => plain(
            match state {
                State::Hover => palette.text.gamma_multiply(GHOST_HOVER),
                State::Pressed => palette.text.gamma_multiply(GHOST_PRESSED),
                _ => Color32::TRANSPARENT,
            },
            Stroke::NONE,
            palette.text,
        ),
        Kind::Danger => plain(
            match state {
                State::Hover => palette.danger.lerp_to_gamma(palette.text, HOVER_SHIFT),
                State::Pressed => palette.danger.lerp_to_gamma(palette.text, PRESSED_SHIFT),
                _ => palette.danger,
            },
            Stroke::NONE,
            palette.accent_text,
        ),
        Kind::Chosen => plain(
            match state {
                // Away from the text colour, since selection has the least contrast to spare.
                State::Hover => palette.selection.lerp_to_gamma(palette.bg_app, HOVER_SHIFT),
                State::Pressed => palette
                    .selection
                    .lerp_to_gamma(palette.bg_app, PRESSED_SHIFT),
                _ => palette.selection,
            },
            Stroke::NONE,
            palette.selection_text,
        ),
    }
}

fn state_of(response: &Response) -> State {
    if !response.enabled() {
        State::Disabled
    } else if response.is_pointer_button_down_on() {
        State::Pressed
    } else if response.hovered() {
        State::Hover
    } else {
        State::Rest
    }
}

fn paint_box(
    ui: &Ui,
    rect: Rect,
    radius: f32,
    look: &Look,
    response: &Response,
    palette: &Palette,
) {
    let painter = ui.painter();
    let corners = CornerRadius::from(radius);
    painter.rect(rect, corners, look.fill, look.stroke, StrokeKind::Inside);
    if response.has_focus() {
        painter.rect_stroke(
            rect.expand(FOCUS_RING_GAP),
            corners + CornerRadius::from(FOCUS_RING_GAP),
            Stroke::new(1.5, palette.accent),
            StrokeKind::Outside,
        );
    }
}

/// A button with a name, and a symbol if it has one.
pub struct ActionButton<'a> {
    palette: &'a Palette,
    kind: Kind,
    icon: Option<Icon>,
    label: &'a str,
    width: f32,
    small: bool,
    selected: Option<bool>,
}

impl<'a> ActionButton<'a> {
    pub fn new(palette: &'a Palette, kind: Kind, label: &'a str) -> Self {
        Self {
            palette,
            kind,
            icon: None,
            label,
            width: 0.0,
            small: false,
            selected: None,
        }
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// The button is at least this wide, with its contents in the middle.
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }

    /// Makes this one of a group of choices, showing whether it is the current one.
    pub fn selected(mut self, selected: bool) -> Self {
        self.kind = if selected { Kind::Chosen } else { Kind::Ghost };
        self.selected = Some(selected);
        self
    }
}

impl Widget for ActionButton<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (height, padding, icon_size, text_size) = if self.small {
            (SMALL_BUTTON_HEIGHT, SMALL_BUTTON_PADDING, 14.0, 12.0)
        } else {
            (BUTTON_HEIGHT, BUTTON_PADDING, 16.0, 13.0)
        };
        let font = match self.kind {
            Kind::Primary | Kind::Danger => fonts::strong(text_size),
            _ => FontId::new(text_size, FontFamily::Proportional),
        };
        let galley = ui
            .painter()
            .layout_no_wrap(self.label.to_owned(), font, Color32::PLACEHOLDER);
        let icon_room = if self.icon.is_some() {
            icon_size + ICON_GAP
        } else {
            0.0
        };
        let contents = icon_room + galley.size().x;
        let size = vec2((contents + padding * 2.0).max(self.width), height);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let enabled = response.enabled();
        let label = self.label;
        response.widget_info(|| match self.selected {
            Some(selected) => {
                egui::WidgetInfo::selected(egui::Role::Button, enabled, selected, label)
            }
            None => egui::WidgetInfo::labeled(egui::Role::Button, enabled, label),
        });

        if ui.is_rect_visible(rect) {
            let look = look(self.palette, self.kind, state_of(&response));
            paint_box(ui, rect, f32::from(RADIUS), &look, &response, self.palette);
            let left = rect.center().x - contents / 2.0;
            if let Some(icon) = &self.icon {
                let middle = pos2(left + icon_size / 2.0, rect.center().y);
                icons::paint(
                    ui.painter(),
                    icons::square_at(middle, icon_size),
                    icon,
                    look.content,
                );
            }
            let text = pos2(left + icon_room, rect.center().y - galley.size().y / 2.0);
            ui.painter().galley(text, galley, look.content);
        }
        response
    }
}

/// A button that is only a symbol. Its name is read out by screen readers, so hover text
/// should repeat it along with any shortcut.
pub struct IconButton<'a> {
    palette: &'a Palette,
    kind: Kind,
    icon: Icon,
    name: &'a str,
    size: f32,
    icon_size: f32,
    icon_raise: f32,
    round: bool,
    solid: bool,
    hidden: bool,
    tint: Option<Color32>,
}

impl<'a> IconButton<'a> {
    pub fn new(palette: &'a Palette, kind: Kind, icon: Icon, name: &'a str) -> Self {
        Self {
            palette,
            kind,
            icon,
            name,
            size: BUTTON_HEIGHT,
            icon_size: 16.0,
            icon_raise: 0.0,
            round: false,
            solid: false,
            hidden: false,
            tint: None,
        }
    }

    pub fn size(mut self, size: f32, icon_size: f32) -> Self {
        self.size = size;
        self.icon_size = icon_size;
        self
    }

    /// Moves the symbol up from the middle, leaving room for something under it.
    pub fn icon_raised(mut self, by: f32) -> Self {
        self.icon_raise = by;
        self
    }

    pub fn round(mut self) -> Self {
        self.round = true;
        self
    }

    /// Fills the symbol in. Only for symbols made of simple closed shapes.
    pub fn solid(mut self) -> Self {
        self.solid = true;
        self
    }

    /// Draws the symbol in its own colour rather than the kind's, unless disabled. Only for a
    /// colour that stays clear on the kind's fills, as [`tinted_look`] is tested for.
    pub fn tint(mut self, color: Color32) -> Self {
        self.tint = Some(color);
        self
    }

    /// Draws nothing unless focused, while the button can still be clicked.
    pub fn hidden(mut self, hidden: bool) -> Self {
        self.hidden = hidden;
        self
    }

    /// Places the button at an exact spot, without taking up room in the layout. These sit
    /// on top of things that can be dragged, so it senses drags too, to keep them for itself.
    pub fn show_at(self, ui: &mut Ui, id: egui::Id, rect: Rect) -> Response {
        let response = ui.interact(rect, id, Sense::click_and_drag());
        self.finish(ui, rect, response)
    }

    fn finish(self, ui: &Ui, rect: Rect, response: Response) -> Response {
        let enabled = response.enabled();
        let name = self.name;
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, name));
        let shown = !self.hidden || response.has_focus();
        if shown && ui.is_rect_visible(rect) {
            let look = tinted_look(self.palette, self.kind, state_of(&response), self.tint);
            let radius = if self.round {
                rect.height() / 2.0
            } else {
                f32::from(RADIUS)
            };
            paint_box(ui, rect, radius, &look, &response, self.palette);
            let middle = rect.center() - vec2(0.0, self.icon_raise);
            let square = icons::square_at(middle, self.icon_size);
            if self.solid {
                icons::paint_solid(ui.painter(), square, &self.icon, look.content);
            } else {
                icons::paint(ui.painter(), square, &self.icon, look.content);
            }
        }
        response
    }
}

impl Widget for IconButton<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, response) = ui.allocate_exact_size(vec2(self.size, self.size), Sense::click());
        self.finish(ui, rect, response)
    }
}

/// A box that can be ticked. It shows a number instead of a tick when given one, for
/// choices where the order they were picked in matters.
pub struct CheckBox<'a> {
    palette: &'a Palette,
    checked: bool,
    name: &'a str,
    number: Option<usize>,
    partly: bool,
}

impl<'a> CheckBox<'a> {
    pub fn new(palette: &'a Palette, checked: bool, name: &'a str) -> Self {
        Self {
            palette,
            checked,
            name,
            number: None,
            partly: false,
        }
    }

    /// Shows that some of what this box stands for is ticked, but not all of it.
    pub fn partly(mut self, partly: bool) -> Self {
        self.partly = partly;
        self
    }

    pub fn number(mut self, number: Option<usize>) -> Self {
        self.number = number;
        self
    }

    /// Places the box at the top left of `area`, without taking up room in the layout. It
    /// sits on top of things that can be dragged, so it senses drags too, to keep them for
    /// itself.
    pub fn show_in_corner(self, ui: &mut Ui, id: egui::Id, area: Rect, margin: f32) -> Response {
        let rect = Rect::from_min_size(
            area.min + vec2(margin, margin),
            vec2(CHECK_BOX_SIZE, CHECK_BOX_SIZE),
        );
        let response = ui.interact(rect, id, Sense::click_and_drag());
        self.finish(ui, rect, response)
    }

    fn finish(self, ui: &Ui, rect: Rect, response: Response) -> Response {
        let (checked, name) = (self.checked, self.name);
        let enabled = response.enabled();
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::CheckBox, enabled, checked, name)
        });
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let palette = self.palette;
        let look = if checked {
            Look {
                fill: palette.accent,
                stroke: Stroke::NONE,
                content: palette.accent_text,
            }
        } else {
            Look {
                fill: palette.raised,
                stroke: Stroke::new(
                    1.0,
                    if response.hovered() {
                        palette.text_muted
                    } else {
                        palette.border_strong
                    },
                ),
                content: palette.text,
            }
        };
        paint_box(ui, rect, 4.0, &look, &response, palette);
        if self.partly && !checked {
            let inner = Rect::from_center_size(rect.center(), rect.size() * 0.5);
            ui.painter().rect_filled(inner, 2.0, palette.accent);
        }
        if checked {
            match self.number {
                Some(number) => {
                    ui.painter().text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        number.to_string(),
                        fonts::strong(12.0),
                        look.content,
                    );
                }
                None => icons::paint(
                    ui.painter(),
                    icons::square_at(rect.center(), 14.0),
                    &icons::CHECK,
                    look.content,
                ),
            }
        }
        response
    }
}

impl Widget for CheckBox<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let size = vec2(CHECK_BOX_SIZE, CHECK_BOX_SIZE);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        self.finish(ui, rect, response)
    }
}

/// One line of a menu, with its symbol, its name and any shortcut.
pub struct MenuItem<'a> {
    palette: &'a Palette,
    icon: Option<Icon>,
    label: &'a str,
    shortcut: &'a str,
    opens_more: bool,
    shape: Option<(u32, u32)>,
    detail: &'a str,
    /// Leaves room for a tick before the symbol, and whether it is ticked.
    tick: Option<bool>,
    width: Option<f32>,
}

impl<'a> MenuItem<'a> {
    pub fn new(palette: &'a Palette, icon: Icon, label: &'a str, shortcut: &'a str) -> Self {
        Self {
            palette,
            icon: Some(icon),
            label,
            shortcut,
            opens_more: false,
            shape: None,
            detail: "",
            tick: None,
            width: None,
        }
    }

    /// Takes this width rather than the usual one, such as a dropdown's, from [`menu_width`].
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    /// One choice among several that has a symbol of its own, with a tick before it if it is the
    /// current one.
    pub fn ticked(palette: &'a Palette, icon: Icon, label: &'a str, chosen: bool) -> Self {
        Self {
            tick: Some(chosen),
            ..Self::new(palette, icon, label, "")
        }
    }

    /// A line that opens more beside the menu, marked with an arrow.
    pub fn opens_more(palette: &'a Palette, icon: Icon, label: &'a str) -> Self {
        Self {
            opens_more: true,
            ..Self::new(palette, icon, label, "")
        }
    }

    /// One choice among several, ticked if it is the current one.
    pub fn choice(palette: &'a Palette, label: &'a str, chosen: bool) -> Self {
        Self {
            palette,
            icon: chosen.then_some(icons::CHECK),
            label,
            shortcut: "",
            opens_more: false,
            shape: None,
            detail: "",
            tick: None,
            width: None,
        }
    }

    /// One shape among several: a drawing of it, its name and, more faintly, its numbers. It is
    /// ticked and the drawing filled in if it is the current one.
    pub fn shape_choice(
        palette: &'a Palette,
        shape: (u32, u32),
        label: &'a str,
        detail: &'a str,
        chosen: bool,
    ) -> Self {
        Self {
            shape: Some(shape),
            detail,
            ..Self::choice(palette, label, chosen)
        }
    }
}

/// How large a drawing of this shape is on a line of a menu.
fn menu_shape_size((width, height): (u32, u32)) -> egui::Vec2 {
    let (width, height) = (width.max(1) as f32, height.max(1) as f32);
    let scale = (MENU_SHAPE_ROOM.x / width).min(MENU_SHAPE_ROOM.y / height);
    vec2(width * scale, height * scale)
}

/// Where a menu line's words start, after any tick, symbol and drawing of a shape.
fn menu_text_left(tick: bool, shape: bool) -> f32 {
    let mut left = MENU_ITEM_PADDING + MENU_ICON_SIZE + ICON_GAP + 2.0;
    if tick {
        left += MENU_ICON_SIZE + ICON_GAP;
    }
    if shape {
        left += MENU_SHAPE_ROOM.x + ICON_GAP + 2.0;
    }
    left
}

/// A menu line's name and any detail after it, going onto a second line rather than past the
/// menu's edge, and cut short with an ellipsis after that.
fn menu_words(
    ui: &Ui,
    (label, detail): (&str, &str),
    detail_colour: Color32,
    room: f32,
) -> std::sync::Arc<egui::Galley> {
    let font = FontId::new(13.0, FontFamily::Proportional);
    let mut job = egui::text::LayoutJob::default();
    job.append(
        label,
        0.0,
        egui::TextFormat::simple(font.clone(), Color32::PLACEHOLDER),
    );
    if !detail.is_empty() {
        job.append(detail, 5.0, egui::TextFormat::simple(font, detail_colour));
    }
    job.wrap.max_width = room;
    job.wrap.max_rows = MENU_ITEM_ROWS;
    job.wrap.overflow_character = Some('…');
    ui.painter().layout_job(job)
}

impl Widget for MenuItem<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let palette = self.palette;
        let width = self.width.unwrap_or(MENU_WIDTH).max(MENU_WIDTH);
        let enabled = ui.is_enabled();
        let shortcut_colour = if enabled {
            palette.text_muted
        } else {
            look(palette, Kind::Ghost, State::Disabled).content
        };
        let shortcut_font = FontId::new(12.0, FontFamily::Proportional);
        let shortcut_room = if self.shortcut.is_empty() {
            0.0
        } else {
            let shortcut = ui.painter().layout_no_wrap(
                self.shortcut.to_owned(),
                shortcut_font.clone(),
                shortcut_colour,
            );
            shortcut.size().x + MENU_SHORTCUT_GAP
        };
        let arrow_room = if self.opens_more {
            MENU_ARROW_SIZE + MENU_SHORTCUT_GAP
        } else {
            0.0
        };
        let text_left = menu_text_left(self.tick.is_some(), self.shape.is_some());
        let room = (width - text_left - MENU_ITEM_PADDING - shortcut_room - arrow_room).max(24.0);
        let words = menu_words(ui, (self.label, self.detail), shortcut_colour, room);
        let height = MENU_ITEM_HEIGHT.max(words.size().y + MENU_ITEM_PADDING * 1.5);
        let (rect, mut response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
        let label = self.label;
        let (detail, chosen) = (self.detail, self.icon.is_some());
        let tick = self.tick;
        response.widget_info(|| {
            let name = if detail.is_empty() {
                label.to_owned()
            } else {
                format!("{label} {detail}")
            };
            match tick {
                Some(ticked) => {
                    egui::WidgetInfo::selected(egui::Role::MenuItem, enabled, ticked, name)
                }
                None => egui::WidgetInfo::labeled(egui::Role::MenuItem, enabled, name),
            }
        });
        if ui.is_rect_visible(rect) {
            let look = look(palette, Kind::Ghost, state_of(&response));
            paint_box(ui, rect, f32::from(RADIUS), &look, &response, palette);
            let icon_size = MENU_ICON_SIZE;
            let mut left = rect.left() + MENU_ITEM_PADDING;
            if let Some(ticked) = self.tick {
                if ticked {
                    icons::paint(
                        ui.painter(),
                        icons::square_at(pos2(left + icon_size / 2.0, rect.center().y), icon_size),
                        &icons::CHECK,
                        look.content,
                    );
                }
                left += icon_size + ICON_GAP;
            }
            if let Some(icon) = &self.icon {
                icons::paint(
                    ui.painter(),
                    icons::square_at(pos2(left + icon_size / 2.0, rect.center().y), icon_size),
                    icon,
                    look.content,
                );
            }
            if let Some(shape) = self.shape {
                let shape_left = left + icon_size + ICON_GAP + 2.0;
                let middle = pos2(shape_left + MENU_SHAPE_ROOM.x / 2.0, rect.center().y);
                let drawing = Rect::from_center_size(middle, menu_shape_size(shape));
                if chosen {
                    ui.painter().rect_filled(drawing, 2.0, look.content);
                } else {
                    ui.painter().rect_stroke(
                        drawing,
                        2.0,
                        Stroke::new(1.3, look.content),
                        StrokeKind::Inside,
                    );
                }
            }
            let words_at = pos2(
                rect.left() + text_left,
                rect.center().y - words.size().y / 2.0,
            );
            ui.painter().galley(words_at, words.clone(), look.content);
            ui.painter().text(
                pos2(rect.right() - MENU_ITEM_PADDING, rect.center().y),
                Align2::RIGHT_CENTER,
                self.shortcut,
                shortcut_font,
                shortcut_colour,
            );
            if self.opens_more {
                let arrow_size = MENU_ARROW_SIZE;
                icons::paint(
                    ui.painter(),
                    icons::square_at(
                        pos2(
                            rect.right() - MENU_ITEM_PADDING - arrow_size / 2.0,
                            rect.center().y,
                        ),
                        arrow_size,
                    ),
                    &icons::CHEVRON_RIGHT,
                    look.content,
                );
            }
        }
        if words.elided {
            response = response.on_hover_text(label);
        }
        response
    }
}

/// How wide a dropdown's menu lines are, so the menu lines up with the dropdown itself.
pub fn menu_width(dropdown: &Response) -> f32 {
    let margin = dropdown.ctx.global_style().spacing.menu_margin.sum().x;
    (dropdown.rect.width() - margin).max(MENU_WIDTH)
}

/// Explains something once the pointer has rested on it, the way a clip's name arrives: a wait,
/// then a fade and a small rise. It sits beside the thing, or on its other side if there is no
/// room, and over everything else. The wait asks for a single frame when it ends.
pub fn note_on_rest(ui: &Ui, palette: &Palette, response: &Response, note: &str) {
    let id = response.id.with("rest-note");
    let now = ui.input(|input| input.time);
    let hovered = response.hovered();
    let since = ui.data_mut(|data| {
        if hovered {
            *data.get_temp_mut_or(id, now)
        } else {
            data.remove::<f64>(id);
            now
        }
    });
    let left = NOTE_REST_SECONDS - (now - since);
    if hovered && left > 0.0 {
        ui.ctx().request_repaint_after_secs(left as f32);
    }
    let opacity =
        ui.ctx()
            .animate_bool_with_time(id.with("fade"), hovered && left <= 0.0, NOTE_FADE_SECONDS);
    if opacity <= 0.0 {
        return;
    }
    let mut painter = ui
        .ctx()
        .layer_painter(egui::LayerId::new(egui::Order::Tooltip, id));
    painter.multiply_opacity(opacity);
    let text = painter.layout_no_wrap(
        note.to_owned(),
        FontId::new(12.0, FontFamily::Proportional),
        palette.text,
    );
    let size = text.size() + NOTE_PADDING * 2.0;
    let row = response.rect;
    let beside = row.right() + NOTE_OFFSET;
    let left_edge = if beside + size.x > ui.ctx().content_rect().right() {
        row.left() - NOTE_OFFSET - size.x
    } else {
        beside
    };
    let top = row.center().y - size.y / 2.0 + (1.0 - opacity) * NOTE_RISE;
    let shape = Rect::from_min_size(pos2(left_edge, top), size);
    painter.rect_filled(shape, f32::from(RADIUS), palette.raised);
    painter.rect_stroke(
        shape,
        f32::from(RADIUS),
        Stroke::new(1.0, palette.border_strong),
        StrokeKind::Inside,
    );
    painter.galley(shape.min + NOTE_PADDING, text, palette.text);
}

/// A setting that is on or off: its name on the left and a switch on the right, with the whole
/// line to press. Made to sit inside a [`settings_group`] or a menu.
pub struct SwitchRow<'a> {
    palette: &'a Palette,
    name: &'a str,
    on: bool,
    info: bool,
    width: Option<f32>,
    height: f32,
}

impl<'a> SwitchRow<'a> {
    pub fn new(palette: &'a Palette, name: &'a str, on: bool) -> Self {
        Self {
            palette,
            name,
            on,
            info: false,
            width: None,
            height: SETTINGS_ROW_HEIGHT,
        }
    }

    /// A shorter row, for a window that needs to save height, such as a recording's.
    pub fn compact(mut self) -> Self {
        self.height = COMPACT_ROW_HEIGHT;
        self
    }

    /// Keeps to this width rather than taking all there is, which in a menu is a great deal.
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    /// Puts an information symbol after the name, saying the row has a hint to rest on.
    pub fn with_info(mut self) -> Self {
        self.info = true;
        self
    }
}

impl Widget for SwitchRow<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let size = vec2(
            self.width.unwrap_or_else(|| ui.available_width()),
            self.height,
        );
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let (name, on) = (self.name, self.on);
        let enabled = response.enabled();
        response
            .widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, enabled, on, name));
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let palette = self.palette;
        let row_look = look(palette, Kind::Ghost, state_of(&response));
        paint_box(ui, rect.shrink(3.0), 5.0, &row_look, &response, palette);
        let text_colour = if enabled {
            palette.text
        } else {
            row_look.content
        };
        let name_rect = ui.painter().text(
            pos2(rect.left() + SETTINGS_ROW_PADDING, rect.center().y),
            Align2::LEFT_CENTER,
            name,
            fonts::strong(13.0),
            text_colour,
        );
        if self.info {
            let symbol = Rect::from_center_size(
                pos2(name_rect.right() + 6.0 + INFO_SIZE / 2.0, rect.center().y),
                vec2(INFO_SIZE, INFO_SIZE),
            );
            let colour = if enabled {
                palette.text_muted
            } else {
                row_look.content
            };
            icons::paint(ui.painter(), symbol, &icons::INFO, colour);
        }
        let along = ui
            .ctx()
            .animate_bool_with_time(response.id.with("switch"), on, SWITCH_SECONDS);
        let track = Rect::from_center_size(
            pos2(
                rect.right() - SETTINGS_ROW_PADDING - SWITCH_SIZE.x / 2.0,
                rect.center().y,
            ),
            SWITCH_SIZE,
        );
        let (track_fill, knob, edge) = if enabled {
            switch_colours(palette, along)
        } else {
            (palette.surface, palette.border_strong, palette.border)
        };
        let radius = SWITCH_SIZE.y / 2.0;
        ui.painter().rect_filled(track, radius, track_fill);
        ui.painter()
            .rect_stroke(track, radius, Stroke::new(1.0, edge), StrokeKind::Inside);
        let reach = SWITCH_SIZE.x / 2.0 - radius;
        ui.painter().circle_filled(
            track.center() + vec2(reach * (along * 2.0 - 1.0), 0.0),
            SWITCH_KNOB_RADIUS,
            knob,
        );
        response
    }
}

/// A switch's track, knob and outline, on the way from off at 0 to on at 1.
fn switch_colours(palette: &Palette, along: f32) -> (Color32, Color32, Color32) {
    (
        palette.surface.lerp_to_gamma(palette.accent, along),
        palette.text_muted.lerp_to_gamma(palette.accent_text, along),
        palette.border_strong.lerp_to_gamma(palette.accent, along),
    )
}

/// A group of settings: a small heading over a card that holds its rows.
pub fn settings_group(ui: &mut Ui, palette: &Palette, heading: &str, add: impl FnOnce(&mut Ui)) {
    ui.label(
        egui::RichText::new(heading.to_uppercase())
            .font(fonts::strong(11.0))
            .color(palette.text_muted),
    );
    ui.add_space(6.0);
    egui::Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(CornerRadius::same(SETTINGS_CARD_RADIUS))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui);
        });
}

/// A line of a group of settings, with its name and any explanation on the left, and on the
/// right a control exactly `control_width` wide.
pub fn settings_row(
    ui: &mut Ui,
    palette: &Palette,
    (name, detail): (&str, Option<&str>),
    control_width: f32,
    add_control: impl FnOnce(&mut Ui),
) {
    let height = if detail.is_some() {
        SETTINGS_ROW_WITH_DETAIL_HEIGHT
    } else {
        SETTINGS_ROW_HEIGHT
    };
    row_with_control(
        ui,
        palette,
        (name, detail),
        (control_width, height),
        add_control,
    );
}

/// A [`settings_row`] as short as a [`SwitchRow::compact`], with no explanation.
pub fn compact_settings_row(
    ui: &mut Ui,
    palette: &Palette,
    name: &str,
    control_width: f32,
    add_control: impl FnOnce(&mut Ui),
) {
    row_with_control(
        ui,
        palette,
        (name, None),
        (control_width, COMPACT_ROW_HEIGHT),
        add_control,
    );
}

fn row_with_control(
    ui: &mut Ui,
    palette: &Palette,
    (name, detail): (&str, Option<&str>),
    (control_width, height): (f32, f32),
    add_control: impl FnOnce(&mut Ui),
) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let left = rect.left() + SETTINGS_ROW_PADDING;
    let name_y = rect.center().y - if detail.is_some() { 9.0 } else { 0.0 };
    ui.painter().text(
        pos2(left, name_y),
        Align2::LEFT_CENTER,
        name,
        fonts::strong(13.0),
        palette.text,
    );
    if let Some(detail) = detail {
        ui.painter().text(
            pos2(left, rect.center().y + 9.0),
            Align2::LEFT_CENTER,
            detail,
            FontId::new(12.0, FontFamily::Proportional),
            palette.text_muted,
        );
    }
    let right = rect.right() - SETTINGS_ROW_PADDING;
    let place = Rect::from_min_max(
        pos2(right - control_width, rect.top()),
        pos2(right, rect.bottom()),
    );
    let mut control = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(place)
            .layout(Layout::left_to_right(Align::Center)),
    );
    add_control(&mut control);
}

/// The line between two rows of a group of settings.
pub fn settings_divider(ui: &mut Ui, palette: &Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(1.0, palette.border),
    );
}

/// Quieter words under a row of a group of settings, lined up with its name and going onto more
/// lines rather than past the card's edge.
pub fn settings_note(ui: &mut Ui, palette: &Palette, note: &str, colour: Option<Color32>) {
    let room = egui::Margin {
        left: SETTINGS_ROW_PADDING as i8,
        right: SETTINGS_ROW_PADDING as i8,
        top: 0,
        bottom: 8,
    };
    egui::Frame::new().inner_margin(room).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add(
            egui::Label::new(
                egui::RichText::new(note)
                    .size(12.0)
                    .color(colour.unwrap_or(palette.text_muted)),
            )
            .wrap(),
        );
    });
}

/// A control that fills a row of a group of settings, inside the row's padding, such as a
/// dropdown under a switch.
pub fn settings_wide_control(ui: &mut Ui, add: impl FnOnce(&mut Ui, f32)) {
    let room = egui::Margin {
        left: SETTINGS_ROW_PADDING as i8,
        right: SETTINGS_ROW_PADDING as i8,
        top: 0,
        bottom: 10,
    };
    egui::Frame::new().inner_margin(room).show(ui, |ui| {
        let width = ui.available_width();
        add(ui, width);
    });
}

/// The room just before a [`SwitchRow`]'s switch, `width` wide and as tall as `height`, for
/// something that belongs with it, such as how loud it is.
pub fn beside_switch(row: Rect, width: f32, height: f32) -> Rect {
    let right = row.right() - SETTINGS_ROW_PADDING - SWITCH_SIZE.x - 12.0;
    Rect::from_min_max(
        pos2(right - width, row.center().y - height / 2.0),
        pos2(right, row.center().y + height / 2.0),
    )
}

/// A round colour to pick, with a ring around the chosen one. It shows the colour itself, so it
/// sits outside the palette, and its name is what screen readers and tests go by.
/// One of the nine places in a position grid.
pub struct SpotButton<'a> {
    palette: &'a Palette,
    name: &'a str,
    chosen: bool,
}

impl<'a> SpotButton<'a> {
    pub fn new(palette: &'a Palette, name: &'a str, chosen: bool) -> Self {
        Self {
            palette,
            name,
            chosen,
        }
    }
}

impl Widget for SpotButton<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, response) =
            ui.allocate_exact_size(vec2(SPOT_BUTTON_SIZE, SPOT_BUTTON_SIZE), Sense::click());
        let name = self.name;
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
        if ui.is_rect_visible(rect) {
            let palette = self.palette;
            let fill = if self.chosen {
                palette.accent
            } else if response.hovered() {
                palette.raised
            } else {
                palette.bg_app
            };
            ui.painter().rect(
                rect,
                2.0,
                fill,
                Stroke::new(1.0, palette.border_strong),
                StrokeKind::Inside,
            );
        }
        response
    }
}

/// Swatches, a colour code and a picker. Returns a colour if one was picked. `typed` keeps a code
/// partway through being typed.
pub fn colour_choices(
    ui: &mut egui::Ui,
    palette: &Palette,
    current: Colour,
    typed: &mut Option<String>,
) -> Option<Colour> {
    let mut picked = None;
    let [red, green, blue] = current;
    let shown = Color32::from_rgb(red, green, blue);
    for row in BACKGROUND_SWATCHES.chunks(8) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (colour, name) in row {
                let [red, green, blue] = *colour;
                let swatch = ColourSwatch::new(
                    palette,
                    Color32::from_rgb(red, green, blue),
                    name,
                    *colour == current,
                );
                if ui.add(swatch).clicked() {
                    picked = Some(*colour);
                }
            }
        });
    }
    ui.separator();
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Custom")
                .size(13.0)
                .color(palette.text_muted),
        );
        let mut text = typed.clone().unwrap_or_else(|| colour_to_hex(current));
        let field = ui.add(
            egui::TextEdit::singleline(&mut text)
                .desired_width(80.0)
                .char_limit(7),
        );
        field.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Colour code"));
        if field.changed() {
            if let Some(colour) = colour_from_hex(&text) {
                picked = Some(colour);
            }
            *typed = Some(text);
        } else if !field.has_focus() {
            *typed = None;
        }
    });
    let mut chosen = shown;
    egui::color_picker::color_picker_color32(ui, &mut chosen, egui::color_picker::Alpha::Opaque);
    if chosen != shown {
        picked = Some([chosen.r(), chosen.g(), chosen.b()]);
    }
    picked
}

pub struct ColourSwatch<'a> {
    palette: &'a Palette,
    colour: Color32,
    name: &'a str,
    chosen: bool,
}

impl<'a> ColourSwatch<'a> {
    pub fn new(palette: &'a Palette, colour: Color32, name: &'a str, chosen: bool) -> Self {
        Self {
            palette,
            colour,
            name,
            chosen,
        }
    }
}

impl Widget for ColourSwatch<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, response) =
            ui.allocate_exact_size(vec2(SWATCH_SIZE, SWATCH_SIZE), Sense::click());
        let name = self.name;
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
        if ui.is_rect_visible(rect) {
            let ring = if self.chosen {
                Stroke::new(2.0, self.palette.accent)
            } else if response.hovered() {
                Stroke::new(1.5, self.palette.text)
            } else {
                Stroke::new(1.0, self.palette.border_strong)
            };
            ui.painter()
                .circle(rect.center(), SWATCH_SIZE / 2.0 - 2.0, self.colour, ring);
        }
        response
    }
}

/// A box showing a name that opens for typing when clicked. It stays one width, cutting a long
/// name short, so a long name can't push the controls beside it around.
pub struct NameBar<'a> {
    palette: &'a Palette,
    label: &'a str,
    width: f32,
}

impl<'a> NameBar<'a> {
    pub fn new(palette: &'a Palette, label: &'a str, width: f32) -> Self {
        Self {
            palette,
            label,
            width,
        }
    }
}

impl Widget for NameBar<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let size = vec2(self.width, NAME_BAR_HEIGHT);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let enabled = response.enabled();
        let label = self.label;
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
        if ui.is_rect_visible(rect) {
            let palette = self.palette;
            let border = if response.hovered() || response.has_focus() {
                palette.accent
            } else {
                palette.border
            };
            ui.painter().rect(
                rect,
                f32::from(RADIUS),
                palette.bg_app,
                Stroke::new(1.0, border),
                StrokeKind::Inside,
            );
            let mut job = egui::text::LayoutJob::simple_singleline(
                label.to_owned(),
                FontId::new(NAME_BAR_TEXT, FontFamily::Proportional),
                palette.text,
            );
            job.wrap.max_width = self.width - NAME_BAR_PADDING * 2.0;
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            let galley = ui.painter().layout_job(job);
            let text = pos2(
                rect.left() + NAME_BAR_PADDING,
                rect.center().y - galley.size().y / 2.0,
            );
            ui.painter().galley(text, galley, palette.text);
        }
        response
    }
}

/// A folder shown as one box with a folder symbol, its path and "Change", which can be clicked
/// anywhere to pick another folder. A long path is cut short at its end and shown whole when
/// hovered.
pub struct FolderBox<'a> {
    palette: &'a Palette,
    folder: &'a std::path::Path,
    width: f32,
}

impl<'a> FolderBox<'a> {
    pub fn new(palette: &'a Palette, folder: &'a std::path::Path, width: f32) -> Self {
        Self {
            palette,
            folder,
            width,
        }
    }

    /// What screen readers call it.
    pub fn name(folder: &std::path::Path) -> String {
        format!("Save in {}", folder.display())
    }
}

impl Widget for FolderBox<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let size = vec2(self.width, SMALL_BUTTON_HEIGHT + 4.0);
        let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
        let enabled = response.enabled();
        let name = Self::name(self.folder);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, &name));
        let palette = self.palette;
        let icon_size = 14.0;
        let change = ui.painter().layout_no_wrap(
            "Change".to_owned(),
            FontId::new(12.0, FontFamily::Proportional),
            palette.accent,
        );
        let path = self.folder.display().to_string();
        let room =
            self.width - NAME_BAR_PADDING * 2.0 - icon_size - ICON_GAP * 2.0 - change.size().x;
        let mut job = egui::text::LayoutJob::simple_singleline(
            path.clone(),
            FontId::new(12.0, FontFamily::Proportional),
            palette.text,
        );
        job.wrap.max_width = room.max(0.0);
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        job.wrap.overflow_character = Some('…');
        let words = ui.painter().layout_job(job);
        if ui.is_rect_visible(rect) {
            let border = if response.hovered() || response.has_focus() {
                palette.accent
            } else {
                palette.border
            };
            ui.painter().rect(
                rect,
                f32::from(RADIUS),
                palette.bg_app,
                Stroke::new(1.0, border),
                StrokeKind::Inside,
            );
            let mut x = rect.left() + NAME_BAR_PADDING;
            icons::paint(
                ui.painter(),
                icons::square_at(pos2(x + icon_size / 2.0, rect.center().y), icon_size),
                &icons::FOLDER_OPEN,
                palette.text_muted,
            );
            x += icon_size + ICON_GAP;
            ui.painter().galley(
                pos2(x, rect.center().y - words.size().y / 2.0),
                words.clone(),
                palette.text,
            );
            ui.painter().galley(
                pos2(
                    rect.right() - NAME_BAR_PADDING - change.size().x,
                    rect.center().y - change.size().y / 2.0,
                ),
                change,
                palette.accent,
            );
        }
        if words.elided {
            response = response.on_hover_text(path);
        }
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    }
}

/// A button showing the current one of several choices, which opens them in a menu. Its name can
/// say more than it shows, because several can show the same choice.
pub struct Dropdown<'a> {
    palette: &'a Palette,
    name: &'a str,
    current: &'a str,
    width: f32,
}

impl<'a> Dropdown<'a> {
    pub fn new(palette: &'a Palette, name: &'a str, current: &'a str, width: f32) -> Self {
        Self {
            palette,
            name,
            current,
            width,
        }
    }
}

impl Widget for Dropdown<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let size = vec2(self.width, SMALL_BUTTON_HEIGHT + 4.0);
        let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
        let enabled = response.enabled();
        let name = self.name;
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, name));
        let icon_size = 14.0;
        // The choice is cut short before the arrow rather than running under it.
        let mut job = egui::text::LayoutJob::simple_singleline(
            self.current.to_owned(),
            FontId::new(12.0, FontFamily::Proportional),
            self.palette.text,
        );
        job.wrap.max_width = (self.width - NAME_BAR_PADDING * 2.0 - icon_size - ICON_GAP).max(0.0);
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        job.wrap.overflow_character = Some('…');
        let words = ui.painter().layout_job(job);
        if ui.is_rect_visible(rect) {
            let palette = self.palette;
            let border = if response.hovered() || response.has_focus() {
                palette.accent
            } else {
                palette.border
            };
            ui.painter().rect(
                rect,
                f32::from(RADIUS),
                palette.bg_app,
                Stroke::new(1.0, border),
                StrokeKind::Inside,
            );
            ui.painter().galley(
                pos2(
                    rect.left() + NAME_BAR_PADDING,
                    rect.center().y - words.size().y / 2.0,
                ),
                words.clone(),
                palette.text,
            );
            icons::paint(
                ui.painter(),
                icons::square_at(
                    pos2(
                        rect.right() - NAME_BAR_PADDING - icon_size / 2.0,
                        rect.center().y,
                    ),
                    icon_size,
                ),
                &icons::CHEVRON_DOWN,
                palette.text_muted,
            );
        }
        if words.elided {
            response = response.on_hover_text(self.current);
        }
        response
    }
}

/// A tab in a list down the side of a dialog, with its symbol before its label. `name` is what
/// screen readers are told, which can say more than the label.
pub struct SideTab<'a> {
    palette: &'a Palette,
    icon: Icon,
    label: &'a str,
    name: &'a str,
    selected: bool,
}

impl<'a> SideTab<'a> {
    pub fn new(
        palette: &'a Palette,
        icon: Icon,
        (label, name): (&'a str, &'a str),
        selected: bool,
    ) -> Self {
        Self {
            palette,
            icon,
            label,
            name,
            selected,
        }
    }
}

impl Widget for SideTab<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let size = vec2(ui.available_width(), SIDE_TAB_HEIGHT);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let (label, name, selected) = (self.label, self.name, self.selected);
        response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, selected, name));
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let palette = self.palette;
        let state = state_of(&response);
        // Picked like a rail tab: a raised card with the accent's bar down its edge.
        let fill = match state {
            State::Pressed => palette.text.gamma_multiply(GHOST_PRESSED),
            State::Hover if !selected => palette.text.gamma_multiply(GHOST_HOVER),
            _ if selected => palette.raised,
            _ => Color32::TRANSPARENT,
        };
        let (symbol_colour, text_colour) = if selected {
            (palette.accent, palette.text)
        } else if state == State::Rest {
            (palette.text_muted, palette.text_muted)
        } else {
            (palette.text, palette.text)
        };
        let look = Look {
            fill,
            stroke: Stroke::NONE,
            content: text_colour,
        };
        paint_box(ui, rect, f32::from(RADIUS), &look, &response, palette);
        if selected {
            ui.painter().rect_filled(
                Rect::from_min_size(
                    rect.left_top() + vec2(0.0, 8.0),
                    vec2(3.0, rect.height() - 16.0),
                ),
                1.0,
                palette.accent,
            );
        }
        let symbol = 16.0;
        let left = rect.left() + SETTINGS_ROW_PADDING;
        icons::paint(
            ui.painter(),
            icons::square_at(pos2(left + symbol / 2.0, rect.center().y), symbol),
            &self.icon,
            symbol_colour,
        );
        ui.painter().text(
            pos2(left + symbol + ICON_GAP + 2.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            if selected {
                fonts::strong(13.0)
            } else {
                FontId::new(13.0, FontFamily::Proportional)
            },
            text_colour,
        );
        response
    }
}

/// A tab on a tool rail, with its symbol above its label. `name` is what screen readers are told.
/// A rail tab's name, centred, going onto a second line rather than past the rail's edges.
fn rail_label(ui: &Ui, label: &str, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(
        label.to_owned(),
        FontId::new(11.0, FontFamily::Proportional),
        Color32::PLACEHOLDER,
        width - RAIL_LABEL_INSET * 2.0,
    );
    job.halign = Align::Center;
    ui.painter().layout_job(job)
}

/// Lets a rail's tabs scroll when the window is too short for them all, with no bar taking room
/// from the tabs.
pub fn rail_scroll(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::ScrollArea::vertical()
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .auto_shrink([false, true])
        .show(ui, add);
}

/// A tab on a tool rail. A compact one is its symbol alone, square, and says its name when
/// hovered like any symbol-only button.
pub fn rail_tab(
    ui: &mut Ui,
    palette: &Palette,
    icon: &Icon,
    label: &str,
    name: &str,
    selected: bool,
    compact: bool,
) -> Response {
    let width = ui.available_width();
    let words = rail_label(ui, if compact { "" } else { label }, width);
    let content = if compact {
        RAIL_SYMBOL
    } else {
        RAIL_SYMBOL + RAIL_SYMBOL_GAP + words.rect.height()
    };
    let size = if compact {
        vec2(width, width)
    } else {
        vec2(width, RAIL_TAB_HEIGHT.max(content + RAIL_LABEL_INSET * 4.0))
    };
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, name));
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let state = state_of(&response);
    let fill = match state {
        State::Pressed => palette.text.gamma_multiply(GHOST_PRESSED),
        State::Hover if !selected => palette.text.gamma_multiply(GHOST_HOVER),
        _ if selected => palette.raised,
        _ => Color32::TRANSPARENT,
    };
    let (symbol_color, text_color) = if selected {
        (palette.accent, palette.text)
    } else if state == State::Rest {
        (palette.text_muted, palette.text_muted)
    } else {
        (palette.text, palette.text)
    };
    let look = Look {
        fill,
        stroke: Stroke::NONE,
        content: text_color,
    };
    paint_box(ui, rect, f32::from(RADIUS), &look, &response, palette);
    if selected {
        let inset = (rect.height() / 4.0).min(12.0);
        ui.painter().rect_filled(
            Rect::from_min_size(
                rect.left_top() + vec2(0.0, inset),
                vec2(3.0, rect.height() - inset * 2.0),
            ),
            1.0,
            palette.accent,
        );
    }
    // The symbol and its name sit together in the middle, however many lines the name takes.
    let top = rect.center().y - content / 2.0;
    icons::paint(
        ui.painter(),
        icons::square_at(pos2(rect.center().x, top + RAIL_SYMBOL / 2.0), RAIL_SYMBOL),
        icon,
        symbol_color,
    );
    if !compact {
        ui.painter().galley(
            pos2(rect.center().x, top + RAIL_SYMBOL + RAIL_SYMBOL_GAP),
            words,
            text_color,
        );
    }
    response
}

/// A symbol that sits among text and does nothing.
pub fn symbol(ui: &mut Ui, icon: &Icon, size: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    icons::paint(ui.painter(), rect, icon, color);
}

/// The title of a dialog, with a symbol saying what kind of dialog it is.
pub fn dialog_title(ui: &mut Ui, icon: &Icon, color: Color32, title: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        symbol(ui, icon, 20.0, color);
        ui.label(egui::RichText::new(title).font(fonts::strong(18.0)));
    });
}

/// The row of buttons along the bottom of a dialog, with the first added at the far right.
/// It only takes the height of a button, so the dialog can shrink to fit its contents.
pub fn dialog_buttons(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let size = vec2(ui.available_width(), BUTTON_HEIGHT);
    ui.allocate_ui_with_layout(size, Layout::right_to_left(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = DIALOG_BUTTON_SPACING;
        add(ui);
    });
}

/// A dialog's bottom row: quieter words on the left and its buttons on the right, both centred
/// on the same line.
pub fn dialog_footer(ui: &mut Ui, palette: &Palette, note: &str, add: impl FnOnce(&mut Ui)) {
    let size = vec2(ui.available_width(), BUTTON_HEIGHT);
    ui.allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), |ui| {
        ui.colored_label(palette.text_muted, note);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = DIALOG_BUTTON_SPACING;
            add(ui);
        });
    });
}

/// One option of a group, with a symbol and a line explaining what it does.
pub fn choice_card(
    ui: &mut Ui,
    palette: &Palette,
    icon: &Icon,
    title: &str,
    description: &str,
    selected: bool,
) -> Response {
    let width = ui.available_width();
    let text_left = CARD_PADDING + CARD_ICON_SIZE + CARD_ICON_GAP;
    let wrap = width - text_left - CARD_PADDING;
    let title_text =
        ui.painter()
            .layout_no_wrap(title.to_owned(), fonts::strong(13.0), palette.text);
    let description_text = ui.painter().layout(
        description.to_owned(),
        FontId::new(12.0, FontFamily::Proportional),
        palette.text_muted,
        wrap,
    );
    let title_height = title_text.size().y;
    let height = CARD_PADDING * 2.0 + title_height + 3.0 + description_text.size().y;
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, selected, title));

    if ui.is_rect_visible(rect) {
        let (fill, stroke) = if selected {
            (palette.surface, Stroke::new(1.5, palette.accent))
        } else if response.hovered() {
            (palette.surface, Stroke::new(1.0, palette.border_strong))
        } else {
            (Color32::TRANSPARENT, Stroke::new(1.0, palette.border))
        };
        let look = Look {
            fill,
            stroke,
            content: palette.text,
        };
        paint_box(ui, rect, f32::from(RADIUS), &look, &response, palette);
        let color = if selected {
            palette.accent
        } else {
            palette.text_muted
        };
        let middle = pos2(
            rect.left() + CARD_PADDING + CARD_ICON_SIZE / 2.0,
            rect.top() + CARD_PADDING + title_height / 2.0,
        );
        icons::paint(
            ui.painter(),
            icons::square_at(middle, CARD_ICON_SIZE),
            icon,
            color,
        );
        let left = rect.left() + text_left;
        let top = rect.top() + CARD_PADDING;
        ui.painter()
            .galley(pos2(left, top), title_text, palette.text);
        ui.painter().galley(
            pos2(left, top + title_height + 3.0),
            description_text,
            palette.text_muted,
        );
    }
    response
}

/// How far something has got, as a bar with the percentage at its right.
pub fn progress_bar(ui: &mut Ui, palette: &Palette, fraction: f32) {
    let fraction = fraction.clamp(0.0, 1.0);
    let percent = format!("{:.0}%", fraction * 100.0);
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::ProgressIndicator, true, percent.clone())
    });
    let font = FontId::new(12.0, FontFamily::Proportional);
    let text = ui.painter().text(
        rect.right_center(),
        Align2::RIGHT_CENTER,
        percent,
        font,
        palette.text,
    );
    let track = Rect::from_center_size(
        pos2(rect.left(), rect.center().y),
        vec2(0.0, PROGRESS_HEIGHT),
    )
    .with_max_x(text.left() - 10.0);
    let radius = PROGRESS_HEIGHT / 2.0;
    ui.painter().rect_filled(track, radius, palette.border);
    if fraction > 0.0 {
        let filled = track.with_max_x(
            (track.left() + track.width() * fraction).max(track.left() + PROGRESS_HEIGHT),
        );
        ui.painter().rect_filled(filled, radius, palette.accent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{
        DARK, LIGHT,
        tests::{contrast, over},
    };
    use eframe::egui::accesskit;

    const KINDS: [Kind; 5] = [
        Kind::Primary,
        Kind::Secondary,
        Kind::Ghost,
        Kind::Danger,
        Kind::Chosen,
    ];
    const TEXT_MINIMUM: f64 = 4.5;

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        fonts::install(&ctx);
        ctx
    }

    /// Runs one frame. Nothing draws these frames, so their texture changes are dropped.
    fn run_ui(
        ctx: &egui::Context,
        input: egui::RawInput,
        show: impl FnMut(&mut Ui),
    ) -> egui::FullOutput {
        let mut output = ctx.run_ui(input, show);
        output.textures_delta.clear();
        output
    }

    fn press_at(at: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn a_rail_tab_reads_clearly_at_rest() {
        for palette in [DARK, LIGHT] {
            assert!(contrast(palette.text_muted, palette.bg_app) >= TEXT_MINIMUM);
            assert!(contrast(palette.text, palette.raised) >= TEXT_MINIMUM);
        }
    }

    #[test]
    fn a_long_rail_name_goes_onto_a_second_line_inside_the_rail() {
        // The rail is 78 wide with 8 either side.
        let width = 62.0;
        let ctx = context();
        run_ui(&ctx, egui::RawInput::default(), |ui| {
            let long = rail_label(ui, "Content library", width);
            assert_eq!(long.rows.len(), 2);
            assert!(long.rect.width() <= width - RAIL_LABEL_INSET * 2.0);
            assert!(long.rect.min.x >= -width / 2.0 && long.rect.max.x <= width / 2.0);
            assert_eq!(rail_label(ui, "Text", width).rows.len(), 1);
        });
    }

    #[test]
    fn a_long_rail_name_makes_its_tab_taller_and_a_short_one_does_not() {
        let ctx = context();
        run_ui(&ctx, egui::RawInput::default(), |ui| {
            ui.set_width(62.0);
            let short = rail_tab(ui, &DARK, &icons::TYPE, "Text", "Text", false, false);
            let long = rail_tab(
                ui,
                &DARK,
                &icons::LIBRARY_BIG,
                "Content library",
                "",
                false,
                false,
            );
            assert_eq!(short.rect.height(), RAIL_TAB_HEIGHT);
            assert!(long.rect.height() > RAIL_TAB_HEIGHT);
        });
    }

    #[test]
    fn a_compact_rail_tab_is_square_whatever_its_name() {
        let ctx = context();
        run_ui(&ctx, egui::RawInput::default(), |ui| {
            ui.set_width(40.0);
            let tab = rail_tab(
                ui,
                &DARK,
                &icons::LIBRARY_BIG,
                "Content library",
                "Content library",
                false,
                true,
            );
            assert_eq!(tab.rect.size(), vec2(40.0, 40.0));
        });
    }

    #[test]
    fn a_check_box_keeps_its_tick_readable() {
        for palette in [DARK, LIGHT] {
            assert!(contrast(palette.accent_text, palette.accent) >= TEXT_MINIMUM);
        }
    }

    #[test]
    fn a_shortcut_in_a_menu_is_readable() {
        for palette in [DARK, LIGHT] {
            assert!(contrast(palette.text_muted, palette.raised) >= TEXT_MINIMUM);
        }
    }

    #[test]
    fn a_check_box_a_rail_tab_and_a_menu_item_can_all_be_clicked() {
        assert!(clicked(|ui| ui.add(CheckBox::new(&DARK, false, "Pick"))));
        assert!(clicked(|ui| rail_tab(
            ui,
            &DARK,
            &icons::FILM,
            "Tool",
            "Tool",
            true,
            false
        )));
        assert!(clicked(|ui| ui.add(MenuItem::new(
            &DARK,
            icons::SCISSORS,
            "Split",
            "S"
        ))));
    }

    #[test]
    fn a_check_box_tells_screen_readers_whether_it_is_ticked() {
        for checked in [false, true] {
            let ctx = context();
            ctx.enable_accesskit();
            let output = run_ui(&ctx, egui::RawInput::default(), |ui| {
                ui.add(CheckBox::new(&DARK, checked, "Pick"));
            });
            let update = output
                .platform_output
                .accesskit_update
                .expect("the tree is reported");
            let node = update
                .nodes
                .iter()
                .find(|(_, node)| node.label() == Some("Pick"))
                .expect("the box is named");
            let expected = if checked {
                accesskit::Toggled::True
            } else {
                accesskit::Toggled::False
            };
            assert_eq!(node.1.toggled(), Some(expected));
        }
    }

    /// Clicks a widget that sits at the top left, and reports whether it noticed.
    fn clicked(widget: impl Fn(&mut Ui) -> Response) -> bool {
        let ctx = context();
        let at = pos2(12.0, 12.0);
        let mut noticed = false;
        for events in [
            vec![],
            vec![egui::Event::PointerMoved(at)],
            vec![press_at(at, true)],
            vec![press_at(at, false)],
        ] {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            run_ui(&ctx, input, |ui| {
                noticed |= widget(ui).clicked();
            });
        }
        noticed
    }

    fn named_nodes(widget: impl Fn(&mut Ui)) -> Vec<(accesskit::Role, String, bool)> {
        let ctx = context();
        ctx.enable_accesskit();
        let output = run_ui(&ctx, egui::RawInput::default(), |ui| widget(ui));
        let update = output
            .platform_output
            .accesskit_update
            .expect("the tree is reported");
        update
            .nodes
            .iter()
            .filter_map(|(_, node)| {
                Some((node.role(), node.label()?.to_owned(), node.is_disabled()))
            })
            .collect()
    }

    #[test]
    fn a_switch_can_be_read_on_and_off_in_both_themes() {
        const SHAPE_MINIMUM: f64 = 3.0;
        for palette in [&DARK, &LIGHT] {
            let (track, knob, _) = switch_colours(palette, 0.0);
            assert!(contrast(knob, track) >= SHAPE_MINIMUM);
            let (track, knob, _) = switch_colours(palette, 1.0);
            assert!(contrast(knob, track) >= SHAPE_MINIMUM);
            assert_eq!(track, palette.accent);
        }
    }

    #[test]
    fn a_drawn_shape_fits_its_room_and_keeps_its_proportions() {
        for ratio in [(16, 9), (9, 16), (1, 1), (21, 9), (2, 3), (1920, 992)] {
            let size = menu_shape_size(ratio);
            assert!(size.x <= MENU_SHAPE_ROOM.x + 0.01 && size.y <= MENU_SHAPE_ROOM.y + 0.01);
            let wanted = ratio.0 as f32 / ratio.1 as f32;
            assert!((size.x / size.y - wanted).abs() < 0.01, "{ratio:?}");
        }
    }

    #[test]
    fn every_kind_of_button_can_be_read_in_both_themes() {
        for (theme, palette) in [("dark", &DARK), ("light", &LIGHT)] {
            for kind in KINDS {
                for state in [State::Rest, State::Hover, State::Pressed] {
                    let look = look(palette, kind, state);
                    for background in [palette.surface, palette.raised, palette.bg_app] {
                        let fill = over(look.fill, background);
                        let ratio = contrast(look.content, fill);
                        assert!(
                            ratio >= TEXT_MINIMUM,
                            "{theme} {kind:?} {state:?} is {ratio:.2}:1"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_red_symbol_on_a_quiet_button_stands_out_in_both_themes() {
        const SHAPE_MINIMUM: f64 = 3.0;
        for (theme, palette) in [("dark", &DARK), ("light", &LIGHT)] {
            for state in [State::Rest, State::Hover, State::Pressed] {
                let look = tinted_look(palette, Kind::Ghost, state, Some(palette.danger));
                assert_eq!(look.content, palette.danger);
                for background in [palette.surface, palette.raised] {
                    let ratio = contrast(look.content, over(look.fill, background));
                    assert!(ratio >= SHAPE_MINIMUM, "{theme} {state:?} is {ratio:.2}:1");
                }
            }
            let disabled = tinted_look(palette, Kind::Ghost, State::Disabled, Some(palette.danger));
            assert_ne!(
                disabled.content, palette.danger,
                "{theme} disabled stays red"
            );
        }
    }

    #[test]
    fn every_kind_of_button_changes_as_it_is_used() {
        for palette in [&DARK, &LIGHT] {
            for kind in KINDS {
                let rest = look(palette, kind, State::Rest);
                let hover = look(palette, kind, State::Hover);
                let pressed = look(palette, kind, State::Pressed);
                let disabled = look(palette, kind, State::Disabled);
                assert_ne!(rest, hover, "{kind:?} does not react to hovering");
                assert_ne!(hover, pressed, "{kind:?} does not react to pressing");
                assert_ne!(rest, disabled, "{kind:?} does not look disabled");
                assert_ne!(
                    rest.content, disabled.content,
                    "{kind:?} symbol stays bright"
                );
            }
        }
    }

    #[test]
    fn a_disabled_button_is_no_longer_filled() {
        for palette in [&DARK, &LIGHT] {
            for kind in KINDS {
                assert_eq!(
                    look(palette, kind, State::Disabled).fill,
                    Color32::TRANSPARENT
                );
            }
        }
    }

    #[test]
    fn enabled_buttons_can_be_clicked() {
        assert!(clicked(|ui| {
            ui.add(ActionButton::new(&DARK, Kind::Primary, "Export").icon(icons::UPLOAD))
        }));
        assert!(clicked(|ui| {
            ui.add(IconButton::new(&DARK, Kind::Ghost, icons::UNDO_2, "Undo"))
        }));
    }

    #[test]
    fn disabled_buttons_cannot_be_clicked() {
        assert!(!clicked(|ui| {
            ui.add_enabled(false, ActionButton::new(&DARK, Kind::Primary, "Export"))
        }));
        assert!(!clicked(|ui| {
            ui.add_enabled(
                false,
                IconButton::new(&DARK, Kind::Ghost, icons::UNDO_2, "Undo"),
            )
        }));
    }

    #[test]
    fn a_hidden_button_can_still_be_clicked() {
        assert!(clicked(|ui| {
            let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(30.0, 30.0));
            IconButton::new(&DARK, Kind::Primary, icons::PLUS, "Add to timeline")
                .hidden(true)
                .show_at(ui, egui::Id::unique("hidden"), rect)
        }));
    }

    #[test]
    fn a_button_that_is_only_a_symbol_still_has_a_name() {
        let nodes = named_nodes(|ui| {
            ui.add(IconButton::new(&DARK, Kind::Ghost, icons::UNDO_2, "Undo"));
        });

        assert!(nodes.contains(&(accesskit::Role::Button, "Undo".to_owned(), false)));
    }

    #[test]
    fn a_disabled_button_is_reported_as_disabled() {
        let nodes = named_nodes(|ui| {
            ui.add_enabled(
                false,
                IconButton::new(&DARK, Kind::Ghost, icons::REDO_2, "Redo"),
            );
            ui.add_enabled(false, ActionButton::new(&DARK, Kind::Primary, "Export"));
        });

        assert!(nodes.contains(&(accesskit::Role::Button, "Redo".to_owned(), true)));
        assert!(nodes.contains(&(accesskit::Role::Button, "Export".to_owned(), true)));
    }

    #[test]
    fn a_choice_is_reported_with_its_role_and_selection() {
        let ctx = context();
        ctx.enable_accesskit();
        let output = run_ui(&ctx, egui::RawInput::default(), |ui| {
            choice_card(
                ui,
                &DARK,
                &icons::ZAP,
                "Fastest",
                "Nothing is re-encoded.",
                true,
            );
        });

        let update = output.platform_output.accesskit_update.expect("reported");
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, node)| node.label() == Some("Fastest"))
            .expect("the choice is named");
        assert_eq!(node.role(), accesskit::Role::RadioButton);
        assert_eq!(node.toggled(), Some(accesskit::Toggled::True));
    }

    #[test]
    fn a_button_grows_to_fit_its_name_and_to_the_requested_width() {
        let ctx = context();
        run_ui(&ctx, egui::RawInput::default(), |ui| {
            let short = ui.add(ActionButton::new(&DARK, Kind::Primary, "Go"));
            let long = ui.add(ActionButton::new(
                &DARK,
                Kind::Primary,
                "Choose file and export",
            ));
            let with_icon = ui.add(ActionButton::new(&DARK, Kind::Primary, "Go").icon(icons::PLAY));
            let wide = ui.add(ActionButton::new(&DARK, Kind::Primary, "Go").width(200.0));
            let small = ui.add(ActionButton::new(&DARK, Kind::Primary, "Go").small());

            assert!(long.rect.width() > short.rect.width());
            assert!(with_icon.rect.width() > short.rect.width());
            assert!((wide.rect.width() - 200.0).abs() < 0.5);
            assert!(small.rect.height() < short.rect.height());
        });
    }

    #[test]
    fn a_choice_grows_to_fit_its_description() {
        let ctx = context();
        run_ui(&ctx, egui::RawInput::default(), |ui| {
            ui.set_width(300.0);
            let short = choice_card(ui, &DARK, &icons::ZAP, "A", "Short.", false);
            let long = choice_card(
                ui,
                &DARK,
                &icons::ZAP,
                "A",
                &"A long line. ".repeat(20),
                false,
            );

            assert!(long.rect.height() > short.rect.height());
        });
    }

    #[test]
    fn the_progress_bar_accepts_any_fraction() {
        let ctx = context();
        run_ui(&ctx, egui::RawInput::default(), |ui| {
            for fraction in [-1.0, 0.0, 0.005, 0.5, 1.0, 2.0, f32::NAN] {
                progress_bar(ui, &DARK, fraction);
            }
        });
    }
}
