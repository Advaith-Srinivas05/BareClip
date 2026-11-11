//! The Content library: shapes, annotations, frames, overlays and backgrounds that BareClip draws
//! itself. Each one added becomes a title's clip whose `art` says what to draw, so it is trimmed,
//! split, moved, saved and exported the way titles are.

use bareclip_core::art::{
    Animate, Art, ArtItem, ArtKind, CATALOGUE, OUTLINE_STEPS, THICKEST_OUTLINE, collections,
    item_named, items_in, matches_search,
};
use bareclip_core::{placement::WHOLE, timeline::Clip};
use bareclip_text::{MovingTitle, draw_title};

use super::{
    titles::{leave_room_for_number, style_slider, texture_of},
    *,
};

/// How many items sit across the panel, for things placed on the picture and for things that cover
/// it.
const PLACED_COLUMNS: usize = 3;
const COVERING_COLUMNS: usize = 2;
const LIBRARY_GAP: f32 = 6.0;
const LIBRARY_CHECK: f32 = 8.0;
const LIBRARY_RADIUS: f32 = 6.0;
/// What an item's card draws it for. A shape takes two fifths of the shorter side, so these give
/// pictures of about a hundred pixels, sharp on any display at the size they show.
const PLACED_CANVAS: (u32, u32) = (240, 240);
const COVERING_CANVAS: (u32, u32) = (256, 144);
/// How long a moving card plays before it starts again, and the pause before it does.
const CARD_LOOP_SECONDS: f32 = 3.0;
const CARD_LOOP_PAUSE: f32 = 0.5;
/// A category or collection's card: its name and count above a row of what is in it.
const GROUP_HEADER: f32 = 44.0;
const GROUP_PADDING: f32 = 10.0;
/// How many items a group's card shows: two rows of four shapes, or two of anything else.
const GROUP_SHAPES: usize = 8;
const GROUP_OTHERS: usize = 2;
/// A new frame or overlay in an empty project lasts this long, since there is no length to match.
const EMPTY_PROJECT_SECONDS: f64 = 5.0;

/// Where the library is, and what has been typed into its search box.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct LibraryView {
    kind: Option<ArtKind>,
    collection: Option<&'static str>,
    pub(super) search: String,
}

/// An item card the pointer is on, which plays its motion if it has one.
pub(super) struct LibraryHover {
    key: &'static str,
    since: f64,
    seen: bool,
    moving: Option<MovingTitle>,
    frame: Option<TextureHandle>,
}

/// What a library card in My media can be asked to do.
#[derive(Default)]
struct LibraryCardActions {
    add: Option<usize>,
    remove: Option<usize>,
}

/// Whether an item covers the canvas, and so is shown wide rather than square.
fn covers(item: &ArtItem) -> bool {
    item.kind.fills_canvas()
}

/// The canvas an item's card draws it for.
fn card_canvas_of(item: &ArtItem) -> (u32, u32) {
    if covers(item) {
        COVERING_CANVAS
    } else {
        PLACED_CANVAS
    }
}

/// Fits a picture of `size` in the middle of `rect`, leaving a little room.
fn fitted_in(rect: egui::Rect, size: egui::Vec2) -> egui::Rect {
    let room = rect.shrink(4.0);
    let scale = (room.width() / size.x.max(1.0)).min(room.height() / size.y.max(1.0));
    egui::Rect::from_center_size(rect.center(), size * scale)
}

/// The see-through squares behind an item.
fn library_checks(painter: &egui::Painter, palette: &Palette, rect: egui::Rect) {
    painter.rect_filled(rect, LIBRARY_RADIUS, palette.raised);
    let columns = (rect.width() / LIBRARY_CHECK).ceil() as usize;
    let rows = (rect.height() / LIBRARY_CHECK).ceil() as usize;
    for row in 0..rows {
        for column in (row % 2..columns).step_by(2) {
            let corner = rect.left_top() + vec2(column as f32, row as f32) * LIBRARY_CHECK;
            let check = egui::Rect::from_min_size(corner, vec2(LIBRARY_CHECK, LIBRARY_CHECK));
            painter.rect_filled(check.intersect(rect), 0.0, palette.surface);
        }
    }
}

/// What goes behind an item on its card: the see-through squares, or for something that would
/// vanish into them, a plain backdrop. Overlays are mostly light, and some items mostly dark.
fn paint_backdrop(painter: &egui::Painter, palette: &Palette, rect: egui::Rect, item: &ArtItem) {
    let colour = item.fill.unwrap_or(item.second);
    let brightness = colour.iter().map(|part| u32::from(*part)).sum::<u32>() / 3;
    if item.kind == ArtKind::Overlay {
        painter.rect_filled(rect, LIBRARY_RADIUS, palette.stage);
    } else if brightness < 60 {
        // A mid grey in both themes, which a dark outline such as a phone's shows up on.
        painter.rect_filled(rect, LIBRARY_RADIUS, palette.border_strong);
    } else {
        library_checks(painter, palette, rect);
    }
}

/// How long a new item of this kind lasts, given how long the project is.
pub(super) fn length_of(kind: ArtKind, project: Ticks) -> Ticks {
    if kind.lasts_whole_project() && project > Ticks(0) {
        project
    } else if kind.lasts_whole_project() {
        Ticks::from_seconds(EMPTY_PROJECT_SECONDS)
    } else {
        Ticks::from_seconds(kind.seconds() as f64)
    }
}

/// What each Animate choice does, to go under its name.
fn animate_description(animate: Animate) -> &'static str {
    match animate {
        Animate::None => "Stays still",
        Animate::Fade => "Fades in at the start and out at the end",
        Animate::SmoothSlide => "Slides in from the left and out to the right",
        Animate::BounceSlide => "Slides in, runs a little past and settles",
        Animate::SmoothScale => "Grows from its middle, and shrinks away",
        Animate::BounceScale => "Grows past its size, dips and settles",
        Animate::Natural => "A motion that suits the shape: flowers bloom, arrows slide",
    }
}

fn animate_icon(animate: Animate) -> icons::Icon {
    match animate {
        Animate::None => icons::X,
        Animate::Fade => icons::CONTRAST,
        Animate::SmoothSlide | Animate::BounceSlide => icons::MOVE_HORIZONTAL,
        Animate::SmoothScale | Animate::BounceScale => icons::SCALING,
        Animate::Natural => icons::WAND_SPARKLES,
    }
}

impl Editor {
    /// The library item a title's clip draws, if it is one.
    pub(super) fn art_of(&self, clip: Clip) -> Option<&Art> {
        self.title_of(clip).and_then(|title| title.art.as_ref())
    }

    /// The selected clip's library item, with its title's id.
    pub(super) fn selected_art(&self) -> Option<(MediaId, Art)> {
        let (id, title) = self.selected_title()?;
        Some((id, title.art?))
    }

    /// Adds something from the library, and selects it. It goes on a new row above everything and
    /// moves nothing. A frame or overlay covers the whole project unless it was dropped somewhere
    /// on the timeline, when it runs from there to the end. `middle` puts a placed item there, as
    /// shares of the canvas, for one dropped on the picture.
    pub(super) fn add_art(
        &mut self,
        art: Art,
        time: Ticks,
        target: Option<TrackTarget>,
        middle: Option<(f64, f64)>,
    ) {
        let kind = art.kind();
        self.record();
        let id = self.new_title_id();
        let project = self.project.tracks.duration();
        let start = if kind.lasts_whole_project() && target.is_none() {
            Ticks(0)
        } else {
            time
        };
        let length = if kind.lasts_whole_project() && start > Ticks(0) {
            let left = Ticks(project.0 - start.0);
            if left > Ticks(0) {
                left
            } else {
                length_of(kind, Ticks(0))
            }
        } else {
            length_of(kind, project)
        };
        let mut title = Title::of_art(art.clone());
        if middle.is_some() && !kind.fills_canvas() {
            title.spot = None;
        }
        self.project.titles.insert(id, title);
        let range = TimeRange::new_still_lasting(length);
        let target = target.unwrap_or(TrackTarget::New(0));
        let placed = self.project.tracks.place(id, range, start, target);
        if let (Some((across, down)), false) = (middle, kind.fills_canvas())
            && let Some(clip) = self.project.tracks.clip(placed)
        {
            let whole = f64::from(WHOLE);
            let placement = Placement {
                center_x: (across.clamp(0.0, 1.0) * whole).round() as i32,
                center_y: (down.clamp(0.0, 1.0) * whole).round() as i32,
                ..clip.placement
            };
            self.project.tracks.set_placement(placed, placement);
        }
        if kind.gets_media_card()
            && !self
                .project
                .library
                .iter()
                .any(|card| card.item == art.item)
        {
            self.project.library.push(Art::of(art.entry()));
        }
        self.settle_canvas();
        self.selected = Some(placed);
        self.sync_titles();
        self.after_clip_added();
    }

    /// Changes the selected clip's library item, as titles' changes are: one undo step for as long
    /// as a control is held.
    pub(super) fn change_art(&mut self, id: MediaId, change: impl Fn(&mut Art)) {
        self.change_title(id, |title| {
            if let Some(art) = &mut title.art {
                change(art);
            }
        });
    }

    /// A card's picture of an item at rest, drawn the first time it is wanted.
    fn library_thumbnail(&mut self, ctx: &egui::Context, item: &'static ArtItem) -> TextureHandle {
        self.library_thumbnails
            .entry(item.key)
            .or_insert_with(|| {
                let drawing = draw_title(&Title::of_art(Art::of(item)), card_canvas_of(item));
                texture_of(ctx, format!("library-{}", item.key), &drawing.picture)
            })
            .clone()
    }

    /// The Content library, in the left panel.
    pub(super) fn library_panel(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let hint = match self.library_view.kind {
            Some(kind) => format!("Search {}", kind.name().to_lowercase()),
            None => "Search".to_owned(),
        };
        let search = ui.add(
            egui::TextEdit::singleline(&mut self.library_view.search)
                .hint_text(hint.as_str())
                .desired_width(f32::INFINITY),
        );
        search.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Search the library")
        });
        ui.add_space(6.0);
        self.library_chips(ui, palette);
        ui.add_space(8.0);
        if let Some(hover) = &mut self.library_hover {
            hover.seen = false;
        }
        let mut add = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let searching = !self.library_view.search.trim().is_empty();
                let view = self.library_view.clone();
                match (view.kind, view.collection) {
                    _ if searching => {
                        let found: Vec<&'static ArtItem> = CATALOGUE
                            .iter()
                            .filter(|item| view.kind.is_none_or(|kind| item.kind == kind))
                            .filter(|item| {
                                view.collection.is_none_or(|name| item.collection == name)
                            })
                            .filter(|item| matches_search(item, &view.search))
                            .collect();
                        add = self.item_grid(ui, palette, &found);
                    }
                    (None, _) => self.category_cards(ui, palette),
                    (Some(kind), None) => self.collection_cards(ui, palette, kind),
                    (Some(kind), Some(collection)) => {
                        let items = items_in(kind, collection);
                        add = self.item_grid(ui, palette, &items);
                    }
                }
            });
        if self.library_hover.as_ref().is_some_and(|hover| !hover.seen) {
            self.library_hover = None;
        }
        if let Some(item) = add {
            let time = self.position();
            self.add_art(Art::of(item), time, None, None);
        }
    }

    /// The chips under the search box: All, then the category and collection open, each of which
    /// steps back when pressed.
    fn library_chips(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let view = self.library_view.clone();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            let at_top = view.kind.is_none();
            let kind = if at_top { Kind::Primary } else { Kind::Ghost };
            if ui
                .add(ActionButton::new(palette, kind, "All").small())
                .on_hover_text("Everything in the library")
                .clicked()
            {
                self.library_view = LibraryView {
                    search: view.search.clone(),
                    ..LibraryView::default()
                };
            }
            if let Some(kind) = view.kind {
                let chip = ActionButton::new(palette, Kind::Primary, kind.name())
                    .icon(icons::X)
                    .small();
                if ui
                    .add(chip)
                    .on_hover_text(format!("Leave {}", kind.name()))
                    .clicked()
                {
                    self.library_view.kind = None;
                    self.library_view.collection = None;
                }
            }
            if let Some(collection) = view.collection {
                let chip = ActionButton::new(palette, Kind::Primary, collection)
                    .icon(icons::X)
                    .small();
                if ui
                    .add(chip)
                    .on_hover_text(format!("Leave {collection}"))
                    .clicked()
                {
                    self.library_view.collection = None;
                }
            }
        });
        if let (Some(kind), Some(collection)) = (view.kind, view.collection) {
            let count = items_in(kind, collection).len();
            ui.add_space(4.0);
            ui.colored_label(palette.text_muted, format!("{count} items"));
        }
    }

    /// One card per category, at the top of the library.
    fn category_cards(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        for kind in ArtKind::ALL {
            let items: Vec<&'static ArtItem> =
                CATALOGUE.iter().filter(|item| item.kind == kind).collect();
            if self.group_card(ui, palette, kind.name(), &items) {
                self.library_view.kind = Some(kind);
                self.library_view.collection = None;
            }
            ui.add_space(LIBRARY_GAP + 2.0);
        }
    }

    /// One card per collection, inside a category.
    fn collection_cards(&mut self, ui: &mut egui::Ui, palette: &Palette, kind: ArtKind) {
        ui.label(RichText::new("Collections").font(fonts::strong(14.0)));
        ui.add_space(6.0);
        for collection in collections(kind) {
            let items = items_in(kind, collection);
            if self.group_card(ui, palette, collection, &items) {
                self.library_view.collection = Some(collection);
            }
            ui.add_space(LIBRARY_GAP + 2.0);
        }
    }

    /// A card for a category or collection: its name, how many items, and a few of them. Returns
    /// whether it was pressed.
    fn group_card(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        name: &str,
        items: &[&'static ArtItem],
    ) -> bool {
        let width = ui.available_width();
        let placed = items.first().is_some_and(|item| !covers(item));
        let shown = if placed { GROUP_SHAPES } else { GROUP_OTHERS }.min(items.len());
        let across = if placed { 4 } else { 2 };
        let inner = width - GROUP_PADDING * 2.0;
        let cell_width = (inner - LIBRARY_GAP * (across as f32 - 1.0)) / across as f32;
        let cell_height = if placed {
            cell_width
        } else {
            cell_width * 9.0 / 16.0
        };
        let rows = shown.div_ceil(across);
        let height = GROUP_HEADER
            + rows as f32 * cell_height
            + (rows.saturating_sub(1)) as f32 * LIBRARY_GAP
            + GROUP_PADDING;
        let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click());
        let spoken = format!("Open {name}, {} items", items.len());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &spoken));
        if !ui.is_rect_visible(rect) {
            return response.clicked();
        }
        let ctx = ui.ctx().clone();
        let painter = ui.painter_at(rect);
        let border = if response.hovered() {
            palette.border_strong
        } else {
            palette.border
        };
        painter.rect(
            rect,
            LIBRARY_RADIUS + 2.0,
            palette.surface,
            Stroke::new(1.0, border),
            egui::StrokeKind::Inside,
        );
        painter.text(
            rect.left_top() + vec2(GROUP_PADDING, 8.0),
            egui::Align2::LEFT_TOP,
            name,
            fonts::strong(14.0),
            palette.text,
        );
        painter.text(
            rect.left_top() + vec2(GROUP_PADDING, 26.0),
            egui::Align2::LEFT_TOP,
            format!("{} items", items.len()),
            egui::FontId::proportional(12.0),
            palette.text_muted,
        );
        icons::paint(
            &painter,
            icons::square_at(
                pos2(rect.right() - GROUP_PADDING - 8.0, rect.top() + 18.0),
                16.0,
            ),
            &icons::CHEVRON_RIGHT,
            palette.text_muted,
        );
        for (index, item) in items.iter().take(shown).enumerate() {
            let (column, row) = (index % across, index / across);
            let cell = egui::Rect::from_min_size(
                rect.left_top()
                    + vec2(
                        GROUP_PADDING + column as f32 * (cell_width + LIBRARY_GAP),
                        GROUP_HEADER + row as f32 * (cell_height + LIBRARY_GAP),
                    ),
                vec2(cell_width, cell_height),
            );
            if !placed {
                paint_backdrop(&painter, palette, cell, item);
            }
            let texture = self.library_thumbnail(&ctx, item);
            painter.image(
                texture.id(),
                fitted_in(cell, texture.size_vec2()),
                egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        response.clicked()
    }

    /// The items of a collection or a search, to press or drag. Returns the one pressed.
    fn item_grid(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        items: &[&'static ArtItem],
    ) -> Option<&'static ArtItem> {
        let mut pressed = None;
        if items.is_empty() {
            ui.colored_label(palette.text_muted, "Nothing in the library matches that");
            return None;
        }
        let width = ui.available_width();
        // Things that cover the canvas go two across and wide, the rest three across.
        let mut index = 0;
        while index < items.len() {
            let wide = covers(items[index]);
            let across = if wide {
                COVERING_COLUMNS
            } else {
                PLACED_COLUMNS
            };
            let row: Vec<&'static ArtItem> = items[index..]
                .iter()
                .take(across)
                .take_while(|item| covers(item) == wide)
                .copied()
                .collect();
            index += row.len();
            let cell = (width - LIBRARY_GAP * (across as f32 - 1.0)) / across as f32;
            let height = if wide { cell * 9.0 / 16.0 } else { cell };
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = LIBRARY_GAP;
                for item in row {
                    if self.item_card(ui, palette, item, vec2(cell, height)) {
                        pressed = Some(item);
                    }
                }
            });
            ui.add_space(LIBRARY_GAP);
        }
        ui.add_space(8.0);
        ui.vertical_centered(|ui| ui.colored_label(palette.text_muted, "End of results"));
        pressed
    }

    /// One item, to press to add at the playhead or drag to the timeline or the picture. Returns
    /// whether it was pressed.
    fn item_card(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        item: &'static ArtItem,
        size: egui::Vec2,
    ) -> bool {
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let name = format!("Add {}", item.name);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
        if response.drag_started() {
            response.dnd_set_drag_payload(MediaDrag {
                media: MediaId::for_title(0),
                length: length_of(item.kind, self.project.tracks.duration()),
                sound_only: false,
                title: None,
                art: Some(item.key),
            });
        }
        if !ui.is_rect_visible(rect) {
            return response.clicked();
        }
        let ctx = ui.ctx().clone();
        let painter = ui.painter_at(rect);
        paint_backdrop(&painter, palette, rect, item);
        let rest = self.library_thumbnail(&ctx, item);
        let moving = response.hovered() && self.library_card_frame(&ctx, item);
        let texture = match (&self.library_hover, moving) {
            (Some(hover), true) => hover.frame.clone().unwrap_or(rest),
            _ => rest,
        };
        painter.image(
            texture.id(),
            fitted_in(rect, texture.size_vec2()),
            egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        if response.hovered() {
            painter.rect_stroke(
                rect,
                LIBRARY_RADIUS,
                Stroke::new(2.0, palette.accent),
                egui::StrokeKind::Inside,
            );
            let badge =
                egui::Rect::from_min_size(rect.right_bottom() - vec2(26.0, 26.0), vec2(22.0, 22.0));
            painter.circle_filled(badge.center(), 11.0, palette.accent);
            icons::paint(
                &painter,
                icons::square_at(badge.center(), 14.0),
                &icons::PLUS,
                palette.accent_text,
            );
            let hint = format!(
                "{}: press to add at the playhead, or drag to the timeline or the picture",
                item.name
            );
            response.clone().on_hover_text(hint);
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        response.clicked()
    }

    /// Plays an item's own motion on its card while the pointer is on it: annotations drawing in
    /// and overlays moving. Returns whether there is a moving picture to show.
    fn library_card_frame(&mut self, ctx: &egui::Context, item: &'static ArtItem) -> bool {
        let title = Title::of_art(Art::of(item));
        let motion = title.motion();
        if motion.is_still() || !motion.draws_frames() {
            return false;
        }
        let now = ctx.input(|input| input.time);
        let same = self
            .library_hover
            .as_ref()
            .is_some_and(|hover| hover.key == item.key);
        if !same {
            self.library_hover = Some(LibraryHover {
                key: item.key,
                since: now,
                seen: true,
                moving: None,
                frame: None,
            });
        }
        let Some(hover) = &mut self.library_hover else {
            return false;
        };
        hover.seen = true;
        ctx.request_repaint();
        let looped = (now - hover.since) as f32 % (CARD_LOOP_SECONDS + CARD_LOOP_PAUSE);
        let stage = motion.stage(looped.min(CARD_LOOP_SECONDS), CARD_LOOP_SECONDS);
        let canvas = card_canvas_of(item);
        let moving = hover
            .moving
            .get_or_insert_with(|| MovingTitle::prepare(&title, canvas, None));
        let picture = moving.frame(stage);
        let image = egui::ColorImage::from_rgba_premultiplied(
            [picture.width as usize, picture.height as usize],
            &picture.rgba,
        );
        match &mut hover.frame {
            Some(texture) => texture.set(image, TextureOptions::LINEAR),
            None => {
                hover.frame =
                    Some(ctx.load_texture("library-moving", image, TextureOptions::LINEAR));
            }
        }
        true
    }

    /// Whether any clip on the timeline draws this item.
    fn item_on_timeline(&self, key: &str) -> bool {
        let tracks = &self.project.tracks;
        self.project
            .titles
            .iter()
            .filter(|(_, title)| title.art.as_ref().is_some_and(|art| art.item == key))
            .any(|(id, _)| tracks.clips_using(*id) > 0)
    }

    /// The library's items that have been added to this project, under the files in My media,
    /// each to add again. Returns whether there were any.
    pub(super) fn library_media_cards(&mut self, ui: &mut egui::Ui, palette: &Palette) -> bool {
        if self.project.library.is_empty() {
            return false;
        }
        let ctx = ui.ctx().clone();
        let mut actions = LibraryCardActions::default();
        let width = ui.available_width();
        let cell = (width - LIBRARY_GAP) / 2.0;
        let cards: Vec<(usize, &'static ArtItem)> = self
            .project
            .library
            .iter()
            .enumerate()
            .filter_map(|(index, art)| Some((index, item_named(&art.item)?)))
            .collect();
        for row in cards.chunks(2) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = LIBRARY_GAP;
                for (index, item) in row {
                    ui.vertical(|ui| {
                        ui.set_width(cell);
                        let picture = vec2(cell, cell * 9.0 / 16.0);
                        let (rect, response) =
                            ui.allocate_exact_size(picture, Sense::click_and_drag());
                        let spoken = format!("{} from the library", item.name);
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::Button, true, &spoken)
                        });
                        if response.drag_started() {
                            response.dnd_set_drag_payload(MediaDrag {
                                media: MediaId::for_title(0),
                                length: length_of(item.kind, self.project.tracks.duration()),
                                sound_only: false,
                                title: None,
                                art: Some(item.key),
                            });
                        }
                        let painter = ui.painter_at(rect);
                        paint_backdrop(&painter, palette, rect, item);
                        let texture = self.library_thumbnail(&ctx, item);
                        painter.image(
                            texture.id(),
                            fitted_in(rect, texture.size_vec2()),
                            egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                        let corner = |at: egui::Pos2| {
                            egui::Rect::from_min_size(at, vec2(CARD_BUTTON_SIZE, CARD_BUTTON_SIZE))
                        };
                        let hovered = ui.rect_contains_pointer(rect);
                        if hovered {
                            let add = corner(
                                rect.right_bottom()
                                    - vec2(
                                        CARD_BUTTON_SIZE + CARD_BUTTON_MARGIN,
                                        CARD_BUTTON_SIZE + CARD_BUTTON_MARGIN,
                                    ),
                            );
                            let name = format!("Add {} to the timeline", item.name);
                            let add = IconButton::new(palette, Kind::Primary, icons::PLUS, &name)
                                .size(CARD_BUTTON_SIZE, 16.0)
                                .show_at(ui, ui.scope_id().with(("library-add", *index)), add);
                            if add.on_hover_text("Add at the playhead").clicked() {
                                actions.add = Some(*index);
                            }
                            let remove = corner(
                                rect.right_top()
                                    + vec2(
                                        -(CARD_BUTTON_SIZE + CARD_BUTTON_MARGIN),
                                        CARD_BUTTON_MARGIN,
                                    ),
                            );
                            let name = format!("Remove {} from My media", item.name);
                            let remove = IconButton::new(palette, Kind::Secondary, icons::X, &name)
                                .size(CARD_BUTTON_SIZE, 14.0)
                                .show_at(
                                    ui,
                                    ui.scope_id().with(("library-remove", *index)),
                                    remove,
                                );
                            if remove
                                .on_hover_text("Take this card away. The timeline isn't changed.")
                                .clicked()
                            {
                                actions.remove = Some(*index);
                            }
                        }
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(item.name).size(12.0).color(palette.text),
                                )
                                .truncate(),
                            );
                            if self.item_on_timeline(item.key) {
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    components::symbol(
                                        ui,
                                        &icons::CIRCLE_CHECK,
                                        CARD_TICK_SIZE,
                                        palette.accent,
                                    );
                                });
                            }
                        });
                    });
                }
            });
            ui.add_space(MEDIA_CARD_SPACING);
        }
        if let Some(index) = actions.add
            && let Some(art) = self.project.library.get(index).cloned()
        {
            let time = self.position();
            self.add_art(art, time, None, None);
        }
        if let Some(index) = actions.remove
            && index < self.project.library.len()
        {
            self.record();
            self.project.library.remove(index);
        }
        true
    }

    /// The Shape panel: a shape's colours and outline, or a background's two colours.
    pub(super) fn shape_controls(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let Some((id, art)) = self.selected_art() else {
            return;
        };
        if art.kind() == ArtKind::Background {
            if let Some(colour) = self.colour_row(ui, palette, "Colour", art.fill, false) {
                self.change_art(id, |art| art.fill = colour.or(art.fill));
            }
            if let Some(colour) =
                self.colour_row(ui, palette, "Second colour", Some(art.second), false)
            {
                self.change_art(id, |art| art.second = colour.unwrap_or(art.second));
            }
            return;
        }
        if let Some(colour) = self.colour_row(ui, palette, "Colour", art.fill, true) {
            self.change_art(id, |art| art.fill = colour);
        }
        ui.add_space(6.0);
        if let Some(colour) = self.colour_row(ui, palette, "Outline", Some(art.second), false) {
            self.change_art(id, |art| art.second = colour.unwrap_or(art.second));
        }
        self.outline_choices(ui, palette, id, &art);
    }

    /// The four outline buttons and the slider for an exact thickness.
    fn outline_choices(&mut self, ui: &mut egui::Ui, palette: &Palette, id: MediaId, art: &Art) {
        let width = ui.available_width();
        // Two by two, because four across leaves too little room for "Medium" in a panel this
        // narrow, and a row wider than the panel pushes the whole panel sideways.
        let button = (width - LIBRARY_GAP) / 2.0;
        let mut picked = None;
        for pair in OUTLINE_STEPS.chunks(2) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = LIBRARY_GAP;
                for (thickness, name) in pair {
                    let kind = if art.outline == *thickness {
                        Kind::Primary
                    } else {
                        Kind::Secondary
                    };
                    let hint = if *thickness == 0 {
                        "No outline".to_owned()
                    } else {
                        format!("{name} outline")
                    };
                    if ui
                        .add(ActionButton::new(palette, kind, name).small().width(button))
                        .on_hover_text(hint)
                        .clicked()
                    {
                        picked = Some(*thickness);
                    }
                }
            });
            ui.add_space(LIBRARY_GAP);
        }
        ui.add_space(6.0);
        style_slider(ui, palette);
        leave_room_for_number(ui);
        let mut thickness = u32::from(art.outline);
        let slider = ui.add(
            egui::Slider::new(&mut thickness, 0..=u32::from(THICKEST_OUTLINE))
                .show_value(true)
                .trailing_fill(true),
        );
        slider.widget_info(|| {
            egui::WidgetInfo::slider(slider.enabled(), f64::from(thickness), "Outline thickness")
        });
        if slider.changed() {
            picked = Some(thickness as u8);
        }
        if slider.drag_stopped() || slider.clicked() {
            slider.surrender_focus();
        }
        if let Some(thickness) = picked {
            self.change_art(id, |art| art.outline = thickness);
        }
    }

    /// The Animate panel: how a shape comes in and goes out.
    pub(super) fn animate_choices(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let Some((id, art)) = self.selected_art() else {
            return;
        };
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (animate, name) in Animate::ALL {
                    let card = components::choice_card(
                        ui,
                        palette,
                        &animate_icon(animate),
                        name,
                        animate_description(animate),
                        art.animate == animate,
                    );
                    if card.clicked() && art.animate != animate {
                        self.change_art(id, |art| art.animate = animate);
                        self.finish_title_edit();
                        // Shows it from the start, so what was chosen can be seen.
                        if let Some(clip) =
                            self.selected.and_then(|id| self.project.tracks.clip(id))
                        {
                            self.seek(clip.start);
                        }
                    }
                    ui.add_space(6.0);
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_and_overlays_last_the_project_and_the_rest_a_set_time() {
        let project = Ticks::from_seconds(12.0);
        assert_eq!(length_of(ArtKind::Frame, project), project);
        assert_eq!(length_of(ArtKind::Overlay, project), project);
        assert_eq!(length_of(ArtKind::Shape, project), Ticks::from_seconds(5.0));
        assert_eq!(
            length_of(ArtKind::Annotation, project),
            Ticks::from_seconds(4.0)
        );
        assert_eq!(
            length_of(ArtKind::Background, project),
            Ticks::from_seconds(5.0)
        );
        assert_eq!(
            length_of(ArtKind::Frame, Ticks(0)),
            Ticks::from_seconds(EMPTY_PROJECT_SECONDS)
        );
    }

    #[test]
    fn every_animate_choice_says_what_it_does() {
        for (animate, _) in Animate::ALL {
            assert!(!animate_description(animate).is_empty());
        }
    }
}
