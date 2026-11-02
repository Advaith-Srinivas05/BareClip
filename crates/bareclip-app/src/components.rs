//! BareClip's shared controls, so every button looks and reacts the same way.
//! Symbols alone are used where their meaning is universal, such as playback and zoom.
//! Actions where a wrong guess costs something also carry their name.

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontFamily, FontId, Layout, Rect, Response, Sense,
    Stroke, StrokeKind, Ui, Widget, pos2, vec2,
};

use crate::{
    fonts,
    icons::{self, Icon},
    theme::Palette,
};

const RADIUS: u8 = 6;
const BUTTON_HEIGHT: f32 = 32.0;
const SMALL_BUTTON_HEIGHT: f32 = 26.0;
const BUTTON_PADDING: f32 = 12.0;
const SMALL_BUTTON_PADDING: f32 = 8.0;
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
const DIVIDER_HEIGHT: f32 = 20.0;
const DIVIDER_ROOM: f32 = 13.0;

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
    round: bool,
    solid: bool,
    hidden: bool,
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
            round: false,
            solid: false,
            hidden: false,
        }
    }

    pub fn size(mut self, size: f32, icon_size: f32) -> Self {
        self.size = size;
        self.icon_size = icon_size;
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

    /// Draws nothing unless focused, while the button can still be clicked.
    pub fn hidden(mut self, hidden: bool) -> Self {
        self.hidden = hidden;
        self
    }

    /// Places the button at an exact spot, without taking up room in the layout.
    pub fn show_at(self, ui: &mut Ui, id: egui::Id, rect: Rect) -> Response {
        let response = ui.interact(rect, id, Sense::click());
        self.finish(ui, rect, response)
    }

    fn finish(self, ui: &Ui, rect: Rect, response: Response) -> Response {
        let enabled = response.enabled();
        let name = self.name;
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, name));
        let shown = !self.hidden || response.has_focus();
        if shown && ui.is_rect_visible(rect) {
            let look = look(self.palette, self.kind, state_of(&response));
            let radius = if self.round {
                rect.height() / 2.0
            } else {
                f32::from(RADIUS)
            };
            paint_box(ui, rect, radius, &look, &response, self.palette);
            let square = icons::square_at(rect.center(), self.icon_size);
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

/// A symbol that sits among text and does nothing.
pub fn symbol(ui: &mut Ui, icon: &Icon, size: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    icons::paint(ui.painter(), rect, icon, color);
}

/// A thin upright line between groups of controls in a toolbar.
pub fn divider(ui: &mut Ui, palette: &Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(DIVIDER_ROOM, DIVIDER_HEIGHT), Sense::hover());
    ui.painter().vline(
        rect.center().x,
        rect.y_range(),
        Stroke::new(1.0, palette.border),
    );
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
