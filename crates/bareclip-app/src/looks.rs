//! The Transitions panel, and the Filters, Adjust colours and Fade controls for a selected clip.

use std::sync::mpsc::{Receiver, channel};

use bareclip_core::{
    effects::{
        ADJUST_RANGE, ClipLook, FILTERS, Filter, LONGEST_TRANSITION_SECONDS,
        SHORTEST_TRANSITION_SECONDS, TRANSITION_GROUPS, Transition, TransitionKind,
    },
    timeline::Clip,
};
use bareclip_media::DecodedPicture;

use super::*;
use crate::{layers::CardDrawer, timeline::TransitionDrag};

const CARD_COLUMNS: usize = 2;
const CARD_GAP: f32 = 8.0;
const LOOK_PANEL_WIDTH: f32 = 250.0;
/// How long the pointer rests on a card before it starts moving, the same as a title's.
const CARD_HOVER_SECONDS: f64 = 1.0;
/// How long a transition card takes to play, and the pause before it plays again.
const CARD_PLAY_SECONDS: f32 = 1.2;
const CARD_PAUSE_SECONDS: f32 = 0.5;
/// How many cards get a picture each frame, so opening a panel stays smooth.
const CARDS_DRAWN_PER_FRAME: usize = 3;
/// A card's picture, and how much of the card's width its height is.
const CARD_SHAPE: f32 = 0.625;
/// The sliders reach this far, and a longer fade can be typed in.
const LONGEST_FADE_SLIDER_SECONDS: f64 = 2.0;
const START_TAB_NAMES: [(LookTab, &str); 3] = [
    (LookTab::Filters, "Filters"),
    (LookTab::Colours, "Adjust colours"),
    (LookTab::Fade, "Fade"),
];

/// Which of a clip's controls is showing in the right-hand panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum LookTab {
    #[default]
    Filters,
    Colours,
    Fade,
}

/// Which of a title's controls is showing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum TitleTab {
    #[default]
    Text,
    Colours,
    Fade,
}

/// A transition card the pointer is resting on.
#[derive(Clone, Copy, Debug)]
pub(super) struct TransitionHover {
    kind: TransitionKind,
    since: f64,
    seen: bool,
}

/// The picture filter cards show, read from the selected clip off to one side.
pub(super) struct FilterSource {
    media: MediaId,
    coming: Option<Receiver<Option<DecodedPicture>>>,
    picture: Option<DecodedPicture>,
    /// Whether it has been asked for already and an image has no need of it.
    still: Option<Arc<StillPicture>>,
}

/// The pictures that the panels have made.
#[derive(Default)]
pub(super) struct LookPictures {
    pub(super) drawer: Option<CardDrawer>,
    drawer_failed: bool,
    transitions: HashMap<TransitionKind, TextureHandle>,
    filters: HashMap<(MediaId, Filter), TextureHandle>,
    source: Option<FilterSource>,
    hover: Option<TransitionHover>,
}

impl Editor {
    /// Opens the panel where a transition is changed, and selects it.
    pub(super) fn select_join(&mut self, id: ClipId) {
        self.selected_join = Some(id);
        self.selected = None;
        self.selected_gap = None;
        self.left_panel = LeftPanel::Transitions;
        self.media_panel_open = true;
    }

    /// Forgets the selected transition if it has gone, such as after an undo.
    pub(super) fn settle_selected_join(&mut self) {
        if let Some(id) = self.selected_join
            && self
                .project
                .tracks
                .clip(id)
                .is_none_or(|clip| clip.transition.is_none())
        {
            self.selected_join = None;
        }
    }

    pub(super) fn delete_selected_join(&mut self) -> bool {
        let Some(id) = self.selected_join.take() else {
            return false;
        };
        self.record();
        self.project.tracks.set_transition(id, None);
        true
    }

    /// Puts a new transition at a join, or swaps the one that is there, keeping how long it lasts.
    pub(super) fn add_transition(&mut self, id: ClipId, kind: TransitionKind) {
        let old = self
            .project
            .tracks
            .clip(id)
            .and_then(|clip| clip.transition);
        let new = old.map_or_else(|| Transition::new(kind), |old| Transition { kind, ..old });
        self.record();
        self.project.tracks.set_transition(id, Some(new));
        self.select_join(id);
    }

    /// One hold of a control is one undo step, in the same way as a title's.
    fn before_change(&mut self) {
        if self.title_before.is_none() {
            self.title_before = Some(self.project.clone());
        }
    }

    fn change_look(&mut self, id: ClipId, change: impl Fn(&mut ClipLook)) {
        let Some(clip) = self.project.tracks.clip(id) else {
            return;
        };
        let mut look = clip.look;
        change(&mut look);
        if look != clip.look {
            self.before_change();
            self.project.tracks.set_look(id, look);
        }
    }

    fn change_transition_length(&mut self, id: ClipId, seconds: f64) {
        let Some(transition) = self
            .project
            .tracks
            .clip(id)
            .and_then(|clip| clip.transition)
        else {
            return;
        };
        let duration = Ticks::from_seconds(seconds).0;
        if duration != transition.duration {
            self.before_change();
            self.project.tracks.set_transition(
                id,
                Some(Transition {
                    duration,
                    ..transition
                }),
            );
        }
    }

    /// Gets ready what draws the cards, on the window's GPU. Without one the cards go without.
    fn card_drawer(&mut self) -> Option<&mut CardDrawer> {
        if self.look_pictures.drawer.is_none() && !self.look_pictures.drawer_failed {
            let made = self.layer_display.as_ref().map(CardDrawer::new);
            match made {
                Some(Ok(drawer)) => self.look_pictures.drawer = Some(drawer),
                Some(Err(_)) => self.look_pictures.drawer_failed = true,
                None => {}
            }
        }
        self.look_pictures.drawer.as_mut()
    }

    /// The Transitions panel. The search box filters it, and the selected transition's length and a
    /// None card sit at the top, because there is no second panel for those.
    pub(super) fn transitions_panel(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let search = ui.add(
            egui::TextEdit::singleline(&mut self.transition_search)
                .hint_text("Search transitions")
                .desired_width(f32::INFINITY),
        );
        search.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Search transitions")
        });
        ui.add_space(8.0);
        let selected = self.selected_join.and_then(|id| {
            let transition = self.project.tracks.clip(id)?.transition?;
            Some((id, transition))
        });
        if let Some(hover) = &mut self.look_pictures.hover {
            hover.seen = false;
        }
        let wanted = self.transition_search.trim().to_lowercase();
        let mut left_to_draw = CARDS_DRAWN_PER_FRAME;
        let mut picked = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let width = ui.available_width();
                let card = (width - CARD_GAP) / CARD_COLUMNS as f32;
                match selected {
                    Some((id, transition)) => {
                        self.transition_length(ui, palette, id, transition);
                        ui.add_space(8.0);
                        if ui
                            .add(
                                ActionButton::new(palette, Kind::Secondary, "None")
                                    .icon(icons::X)
                                    .width(width),
                            )
                            .on_hover_text("Take this transition away")
                            .clicked()
                        {
                            self.delete_selected_join();
                        }
                        ui.add_space(10.0);
                    }
                    None => {
                        ui.colored_label(
                            palette.text_muted,
                            RichText::new("Drag a transition between two clips on the timeline.")
                                .size(12.0),
                        );
                        ui.add_space(10.0);
                    }
                }
                let chosen = selected.map(|(_, transition)| transition.kind);
                for (heading, kinds) in TRANSITION_GROUPS {
                    let matching: Vec<TransitionKind> = kinds
                        .iter()
                        .copied()
                        .filter(|kind| kind.name().to_lowercase().contains(&wanted))
                        .collect();
                    if matching.is_empty() {
                        continue;
                    }
                    ui.label(RichText::new(heading).font(fonts::strong(13.0)));
                    ui.add_space(6.0);
                    for row in matching.chunks(CARD_COLUMNS) {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = CARD_GAP;
                            for kind in row {
                                let is_chosen = chosen == Some(*kind);
                                if self.transition_card(
                                    ui,
                                    palette,
                                    *kind,
                                    card,
                                    is_chosen,
                                    &mut left_to_draw,
                                ) {
                                    picked = Some(*kind);
                                }
                            }
                        });
                        ui.add_space(CARD_GAP);
                    }
                    ui.add_space(6.0);
                }
            });
        if self.look_pictures.hover.is_some_and(|hover| !hover.seen) {
            self.look_pictures.hover = None;
        }
        if left_to_draw == 0 {
            ui.ctx().request_repaint();
        }
        if let (Some(kind), Some((id, _))) = (picked, selected) {
            self.add_transition(id, kind);
        }
    }

    fn transition_length(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        id: ClipId,
        transition: Transition,
    ) {
        ui.label(RichText::new("Duration").font(fonts::strong(13.0)));
        ui.add_space(4.0);
        let longest = self
            .project
            .tracks
            .longest_join(id)
            .map_or(LONGEST_TRANSITION_SECONDS, Ticks::as_seconds)
            .clamp(SHORTEST_TRANSITION_SECONDS, LONGEST_TRANSITION_SECONDS);
        let mut seconds = Ticks(transition.duration).as_seconds();
        ui.horizontal(|ui| {
            let number_width = 62.0;
            let room = ui.available_width() - number_width - ui.spacing().item_spacing.x;
            set_slider_look(ui, palette);
            ui.spacing_mut().slider_width = room;
            let slider = ui.add(
                egui::Slider::new(&mut seconds, SHORTEST_TRANSITION_SECONDS..=longest)
                    .show_value(false)
                    .trailing_fill(true),
            );
            slider.widget_info(|| {
                egui::WidgetInfo::slider(slider.enabled(), seconds, "Transition duration")
            });
            if slider.drag_stopped() || slider.clicked() {
                slider.surrender_focus();
            }
            ui.add_sized(
                vec2(number_width, 22.0),
                egui::DragValue::new(&mut seconds)
                    .speed(0.01)
                    .range(SHORTEST_TRANSITION_SECONDS..=longest)
                    .max_decimals(2)
                    .suffix(" s"),
            );
        });
        self.change_transition_length(id, seconds);
    }

    /// A transition to drag to the timeline. With one selected, pressing it swaps to this one.
    fn transition_card(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        kind: TransitionKind,
        width: f32,
        chosen: bool,
        left_to_draw: &mut usize,
    ) -> bool {
        let name_height = 18.0;
        let picture_height = width * CARD_SHAPE;
        let (whole, response) = ui.allocate_exact_size(
            vec2(width, picture_height + name_height),
            Sense::click_and_drag(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Add {}", kind.name()))
        });
        if response.drag_started() {
            response.dnd_set_drag_payload(TransitionDrag(kind));
        }
        if !ui.is_rect_visible(whole) {
            return response.clicked();
        }
        let picture = egui::Rect::from_min_size(whole.min, vec2(width, picture_height));
        let painter = ui.painter_at(whole);
        painter.rect_filled(picture, 6.0, palette.raised);
        let full = egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));

        let live = if response.hovered() {
            self.live_card(ui.ctx(), kind)
        } else {
            None
        };
        if let Some(live) = live {
            painter.image(live, picture, full, Color32::WHITE);
        } else {
            if !self.look_pictures.transitions.contains_key(&kind)
                && *left_to_draw > 0
                && let Some(drawn) = self
                    .card_drawer()
                    .and_then(|drawer| drawer.transition(kind))
            {
                *left_to_draw -= 1;
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [drawn.width as usize, drawn.height as usize],
                    &drawn.rgba,
                );
                let texture = ui.ctx().load_texture(
                    format!("transition-{kind:?}"),
                    image,
                    TextureOptions::LINEAR,
                );
                self.look_pictures.transitions.insert(kind, texture);
            }
            if let Some(texture) = self.look_pictures.transitions.get(&kind) {
                painter.image(texture.id(), picture, full, Color32::WHITE);
            }
        }
        let outline = if chosen {
            Some(palette.accent)
        } else if response.hovered() {
            Some(palette.text_muted)
        } else {
            None
        };
        if let Some(colour) = outline {
            painter.rect_stroke(
                picture,
                6.0,
                Stroke::new(2.0, colour),
                egui::StrokeKind::Inside,
            );
        }
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            response
                .clone()
                .on_hover_text(format!("{}: drag between two clips", kind.name()));
        }
        painter.text(
            pos2(whole.left() + 2.0, picture.bottom() + 3.0),
            egui::Align2::LEFT_TOP,
            kind.name(),
            egui::FontId::proportional(12.0),
            palette.text_muted,
        );
        response.clicked()
    }

    /// Plays a card once the pointer has rested on it, on the GPU, and asks for the next frame.
    fn live_card(&mut self, ctx: &egui::Context, kind: TransitionKind) -> Option<egui::TextureId> {
        let now = ctx.input(|input| input.time);
        let hover = &mut self.look_pictures.hover;
        match hover {
            Some(hover) if hover.kind == kind => hover.seen = true,
            _ => {
                *hover = Some(TransitionHover {
                    kind,
                    since: now,
                    seen: true,
                });
            }
        }
        let since = hover.map_or(now, |hover| hover.since);
        let waited = now - since;
        if waited < CARD_HOVER_SECONDS {
            ctx.request_repaint_after(Duration::from_secs_f64(CARD_HOVER_SECONDS - waited));
            return None;
        }
        ctx.request_repaint();
        let looped =
            (waited - CARD_HOVER_SECONDS) as f32 % (CARD_PLAY_SECONDS + CARD_PAUSE_SECONDS);
        let progress = (looped / CARD_PLAY_SECONDS).min(1.0);
        self.card_drawer()?.live_transition(kind, progress)
    }

    /// Whether the right-hand panel has a clip's look to show.
    pub(super) fn looking_at(&self) -> Option<Clip> {
        if !self.look_panel_open {
            return None;
        }
        let clip = self.selected.and_then(|id| self.project.tracks.clip(id))?;
        let on_pictures = self
            .project
            .tracks
            .track_of(clip.id)
            .is_some_and(|row| row < self.project.tracks.picture_tracks().len());
        (on_pictures && !clip.is_title()).then_some(clip)
    }

    /// The panel with a selected clip's look on it.
    pub(super) fn look_panel(&mut self, ui: &mut egui::Ui, palette: &Palette, clip: Clip) {
        let frame = panel_card(palette.surface, Margin::same(14))
            .corner_radius(CornerRadius::same(PANEL_RADIUS));
        egui::Panel::right("look_panel")
            .exact_size(LOOK_PANEL_WIDTH)
            .show_separator_line(false)
            .frame(frame)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let name = START_TAB_NAMES
                        .iter()
                        .find(|(tab, _)| *tab == self.look_tab)
                        .map_or("", |(_, name)| name);
                    ui.label(RichText::new(name).font(fonts::strong(15.0)));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let close = IconButton::new(palette, Kind::Ghost, icons::X, "Close look")
                            .size(PANEL_CLOSE_SIZE, 16.0);
                        if ui.add(close).clicked() {
                            self.look_panel_open = false;
                        }
                    });
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (tab, name, short) in [
                        (LookTab::Filters, "Filters", "Filters"),
                        (LookTab::Colours, "Adjust colours", "Colours"),
                        (LookTab::Fade, "Fade", "Fade"),
                    ] {
                        let kind = if self.look_tab == tab {
                            Kind::Secondary
                        } else {
                            Kind::Ghost
                        };
                        let button = ActionButton::new(palette, kind, short).small();
                        if ui.add(button).on_hover_text(name).clicked() {
                            self.look_tab = tab;
                        }
                    }
                });
                ui.add_space(10.0);
                match self.look_tab {
                    LookTab::Filters => self.filter_cards(ui, palette, clip),
                    LookTab::Colours => {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| self.colour_controls(ui, palette, clip));
                    }
                    LookTab::Fade => self.fade_controls(ui, palette, clip),
                }
            });
    }

    /// A title's Adjust colours and Fade, which sit in its own panel beside its words.
    pub(super) fn title_look(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let Some(clip) = self.selected.and_then(|id| self.project.tracks.clip(id)) else {
            return;
        };
        match self.title_tab {
            TitleTab::Text => {}
            TitleTab::Colours => self.colour_controls(ui, palette, clip),
            TitleTab::Fade => self.fade_controls(ui, palette, clip),
        }
    }

    /// Gets the selected clip's picture for the filter cards, without holding up the window.
    fn source_for_filters(&mut self, ctx: &egui::Context, clip: Clip) {
        let same = self
            .look_pictures
            .source
            .as_ref()
            .is_some_and(|source| source.media == clip.media);
        if !same {
            let still = self.entry(clip.media).and_then(|entry| entry.still.clone());
            let path = self.entry(clip.media).map(|entry| entry.info.path.clone());
            let coming = match (&still, path) {
                (None, Some(path)) => {
                    let (sender, receiver) = channel();
                    let moment = clip.source_at(self.position().clamp(clip.start, clip.end()));
                    let repaint = ctx.clone();
                    let _ = thread::Builder::new()
                        .name("bareclip-filter-picture".to_owned())
                        .spawn(move || {
                            let picture = bareclip_media::PictureReader::open(&path)
                                .and_then(|mut reader| {
                                    reader.seek(moment)?;
                                    reader.next_picture()
                                })
                                .ok()
                                .flatten();
                            let _ = sender.send(picture);
                            repaint.request_repaint();
                        });
                    Some(receiver)
                }
                _ => None,
            };
            self.look_pictures.source = Some(FilterSource {
                media: clip.media,
                coming,
                picture: None,
                still,
            });
        }
        if let Some(source) = &mut self.look_pictures.source
            && let Some(picture) = source
                .coming
                .as_ref()
                .and_then(|coming| coming.try_recv().ok())
        {
            source.picture = picture;
            source.coming = None;
        }
    }

    fn filter_cards(&mut self, ui: &mut egui::Ui, palette: &Palette, clip: Clip) {
        self.source_for_filters(ui.ctx(), clip);
        let search = ui.add(
            egui::TextEdit::singleline(&mut self.filter_search)
                .hint_text("Search filters")
                .desired_width(f32::INFINITY),
        );
        search.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Search filters")
        });
        ui.add_space(8.0);
        let wanted = self.filter_search.trim().to_lowercase();
        let mut listed = vec![Filter::None];
        listed.extend(FILTERS);
        listed.retain(|filter| filter.name().to_lowercase().contains(&wanted));
        let mut left_to_draw = CARDS_DRAWN_PER_FRAME;
        let mut picked = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let width = ui.available_width();
                let card = (width - CARD_GAP) / CARD_COLUMNS as f32;
                for row in listed.chunks(CARD_COLUMNS) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = CARD_GAP;
                        for filter in row {
                            if self.filter_card(ui, palette, clip, *filter, card, &mut left_to_draw)
                            {
                                picked = Some(*filter);
                            }
                        }
                    });
                    ui.add_space(CARD_GAP);
                }
            });
        if left_to_draw == 0 {
            ui.ctx().request_repaint();
        }
        if let Some(filter) = picked {
            self.change_look(clip.id, |look| look.filter = filter);
        }
    }

    fn filter_card(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        clip: Clip,
        filter: Filter,
        width: f32,
        left_to_draw: &mut usize,
    ) -> bool {
        let name_height = 30.0;
        let picture_height = width * CARD_SHAPE;
        let (whole, response) =
            ui.allocate_exact_size(vec2(width, picture_height + name_height), Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Button,
                true,
                format!("Filter {}", filter.name()),
            )
        });
        if !ui.is_rect_visible(whole) {
            return response.clicked();
        }
        let picture = egui::Rect::from_min_size(whole.min, vec2(width, picture_height));
        let painter = ui.painter_at(whole);
        painter.rect_filled(picture, 6.0, palette.raised);
        let key = (clip.media, filter);
        if !self.look_pictures.filters.contains_key(&key) && *left_to_draw > 0 {
            *left_to_draw -= 1;
            self.draw_filter_card(ui.ctx(), key);
        }
        if filter == Filter::None {
            icons::paint(
                &painter,
                icons::square_at(picture.center(), 22.0),
                &icons::X,
                palette.text_muted,
            );
        } else if let Some(texture) = self.look_pictures.filters.get(&key) {
            painter.image(
                texture.id(),
                picture,
                egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        let outline = if clip.look.filter == filter {
            Some(palette.accent)
        } else if response.hovered() {
            Some(palette.text_muted)
        } else {
            None
        };
        if let Some(colour) = outline {
            painter.rect_stroke(
                picture,
                6.0,
                Stroke::new(2.0, colour),
                egui::StrokeKind::Inside,
            );
        }
        let name = painter.layout(
            filter.name().to_owned(),
            egui::FontId::proportional(12.0),
            palette.text_muted,
            width - 4.0,
        );
        painter.galley(
            pos2(whole.left() + 2.0, picture.bottom() + 3.0),
            name,
            palette.text_muted,
        );
        response.clicked()
    }

    fn draw_filter_card(&mut self, ctx: &egui::Context, (media, filter): (MediaId, Filter)) {
        if filter == Filter::None {
            return;
        }
        self.card_drawer();
        let LookPictures {
            drawer: Some(drawer),
            source: Some(source),
            filters,
            ..
        } = &mut self.look_pictures
        else {
            return;
        };
        let drawn = match (&source.still, &source.picture) {
            (Some(still), _) => drawer.filter_on_still(still, filter),
            (None, Some(picture)) => drawer.filter_on_video(picture, filter),
            (None, None) => return,
        };
        if let Some(drawn) = drawn {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [drawn.width as usize, drawn.height as usize],
                &drawn.rgba,
            );
            let name = format!("filter-{media:?}-{filter:?}");
            filters.insert(
                (media, filter),
                ctx.load_texture(name, image, TextureOptions::LINEAR),
            );
        }
    }

    fn colour_controls(&mut self, ui: &mut egui::Ui, palette: &Palette, clip: Clip) {
        let id = clip.id;
        let look = clip.look;
        let row = |editor: &mut Self,
                   ui: &mut egui::Ui,
                   name: &str,
                   value: i32,
                   apply: fn(&mut ClipLook, i32)| {
            if let Some(new) = adjust_slider(ui, palette, name, value) {
                editor.change_look(id, |look| apply(look, new));
            }
        };
        row(self, ui, "Exposure", look.exposure, |look, value| {
            look.exposure = value;
        });
        row(self, ui, "Contrast", look.contrast, |look, value| {
            look.contrast = value;
        });
        row(self, ui, "Saturation", look.saturation, |look, value| {
            look.saturation = value;
        });
        row(self, ui, "Temperature", look.temperature, |look, value| {
            look.temperature = value;
        });

        ui.add_space(6.0);
        ui.label(RichText::new("Transparency").font(fonts::strong(13.0)));
        ui.add_space(4.0);
        set_slider_look(ui, palette);
        ui.spacing_mut().slider_width = ui.available_width();
        let whole = f64::from(bareclip_core::placement::WHOLE);
        let mut see_through = 1.0 - f64::from(clip.placement.opacity) / whole;
        let slider = ui.add(
            egui::Slider::new(&mut see_through, 0.0..=1.0)
                .show_value(false)
                .trailing_fill(true),
        );
        slider.widget_info(|| {
            egui::WidgetInfo::slider(slider.enabled(), see_through * 100.0, "Transparency")
        });
        if slider.changed() {
            self.before_change();
            let opacity = ((1.0 - see_through) * whole).round() as i32;
            self.project.tracks.set_placement(
                id,
                Placement {
                    opacity,
                    ..clip.placement
                },
            );
        }
        if slider.drag_stopped() || slider.clicked() {
            slider.surrender_focus();
        }

        ui.add_space(12.0);
        let reset = ActionButton::new(palette, Kind::Secondary, "Reset")
            .icon(icons::ROTATE_CCW)
            .width(ui.available_width());
        if ui.add(reset).clicked() {
            self.change_look(id, |look| {
                *look = ClipLook {
                    filter: look.filter,
                    fade_in: look.fade_in,
                    fade_out: look.fade_out,
                    ..ClipLook::default()
                };
            });
            self.before_change();
            self.project.tracks.set_placement(
                id,
                Placement {
                    opacity: bareclip_core::placement::WHOLE,
                    ..clip.placement
                },
            );
        }
    }

    fn fade_controls(&mut self, ui: &mut egui::Ui, palette: &Palette, clip: Clip) {
        let longest = clip.duration().as_seconds() / 2.0;
        let id = clip.id;
        if let Some(seconds) = fade_row(ui, palette, "Fade in", clip.look.fade_in, longest) {
            let ticks = Ticks::from_seconds(seconds).0;
            self.change_look(id, |look| look.fade_in = ticks);
        }
        ui.add_space(8.0);
        if let Some(seconds) = fade_row(ui, palette, "Fade out", clip.look.fade_out, longest) {
            let ticks = Ticks::from_seconds(seconds).0;
            self.change_look(id, |look| look.fade_out = ticks);
        }
    }
}

/// Gets a slider ready, in BareClip's colours.
fn set_slider_look(ui: &mut egui::Ui, palette: &Palette) {
    let visuals = &mut ui.style_mut().visuals;
    visuals.widgets.inactive.bg_fill = palette.border_strong;
    visuals.widgets.hovered.bg_fill = palette.border_strong;
    visuals.widgets.active.bg_fill = palette.border_strong;
    visuals.selection.bg_fill = palette.accent;
}

/// A named slider running from one end to the other with no change in the middle.
fn adjust_slider(ui: &mut egui::Ui, palette: &Palette, name: &str, value: i32) -> Option<i32> {
    ui.add_space(4.0);
    ui.label(RichText::new(name).font(fonts::strong(13.0)));
    ui.add_space(4.0);
    set_slider_look(ui, palette);
    ui.spacing_mut().slider_width = ui.available_width();
    let mut shown = f64::from(value) / f64::from(ADJUST_RANGE);
    let slider = ui.add(egui::Slider::new(&mut shown, -1.0..=1.0).show_value(false));
    slider.widget_info(|| egui::WidgetInfo::slider(slider.enabled(), shown, name));
    if slider.drag_stopped() || slider.clicked() {
        slider.surrender_focus();
    }
    ui.add_space(6.0);
    slider
        .changed()
        .then(|| (shown * f64::from(ADJUST_RANGE)).round() as i32)
}

/// A fade's name, its length as a number that can be typed over, and a slider.
fn fade_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    name: &str,
    ticks: i64,
    longest: f64,
) -> Option<f64> {
    let mut seconds = Ticks(ticks).as_seconds();
    let was = seconds;
    ui.horizontal(|ui| {
        ui.label(RichText::new(name).font(fonts::strong(13.0)));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add(
                egui::DragValue::new(&mut seconds)
                    .speed(0.05)
                    .range(0.0..=longest)
                    .max_decimals(2)
                    .suffix(" s"),
            );
        });
    });
    ui.add_space(4.0);
    set_slider_look(ui, palette);
    ui.spacing_mut().slider_width = ui.available_width();
    let top = longest.clamp(0.01, LONGEST_FADE_SLIDER_SECONDS.max(0.01));
    let mut on_slider = seconds.min(top);
    let slider = ui.add(
        egui::Slider::new(&mut on_slider, 0.0..=top)
            .show_value(false)
            .trailing_fill(true),
    );
    slider.widget_info(|| egui::WidgetInfo::slider(slider.enabled(), on_slider, name));
    if slider.changed() {
        seconds = on_slider;
    }
    if slider.drag_stopped() || slider.clicked() {
        slider.surrender_focus();
    }
    ((seconds - was).abs() > f64::EPSILON).then_some(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panels_have_something_to_list() {
        assert_eq!(FILTERS.len(), 49);
        let kinds: usize = TRANSITION_GROUPS.iter().map(|(_, kinds)| kinds.len()).sum();
        assert_eq!(kinds, 44);
    }
}
