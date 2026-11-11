//! Titles in the editor: adding them, the Text panel, the controls beside the picture, and copying
//! clips. A title lives in the project's `titles`, and its clip refers to it by `MediaId`. It also
//! gets a `MediaEntry` that nothing lists, so the rest of the editor treats it as an image.

use bareclip_core::{
    placement::WHOLE,
    srt::{decode_subtitles, parse_srt},
    time::TimeBase,
    timeline::Clip,
    title::{
        LARGEST_SIZE, SMALLEST_SIZE, STYLE_GROUPS, TEXT_STYLES_HEADING, TEXT_STYLES_SHOWN,
        TitleSpot,
    },
};
use bareclip_media::VideoInfo;
use bareclip_text::{MovingTitle, draw_font_name, draw_title, draw_title_with_logo, font_list};

use super::*;

/// As wide as the media panel.
/// The canvas the Text panel's cards draw their styles for. Only the shape matters, and these are
/// shown smaller than they are drawn so they stay sharp on any display.
const CARD_CANVAS: (u32, u32) = (640, 360);
/// The canvas for styles that run across the whole canvas. Their cards are laid out on a
/// square, so a frame goes around all four sides and a panel keeps its words, where cropping the
/// middle of a wide canvas would cut them.
const FULL_CARD_CANVAS: (u32, u32) = (360, 360);
const CARD_COLUMNS: usize = 2;
const CARD_GAP: f32 = 8.0;
const CARD_ROOM: f32 = 8.0;
const CARD_CHECK: f32 = 10.0;
const CARD_RADIUS: f32 = 6.0;
const FONT_ROW_HEIGHT: f32 = 32.0;
const FONT_ROW_TEXT: f32 = 18.0;
const FONT_LIST_WIDTH: f32 = 240.0;
const FONT_LIST_HEIGHT: f32 = 300.0;
/// How many new fonts get drawn for the list each frame, so a fast scroll stays smooth.
const FONTS_DRAWN_PER_FRAME: usize = 8;
const SPOT_GAP: f32 = 2.0;
/// Next to each other on a row counts as this close, as captions from a file often leave a sliver.
const CAPTIONS_TOUCHING_SECONDS: f64 = 0.05;
const SHIPPED_HEADING: &str = "Fonts with BareClip";
const ON_THIS_PC_HEADING: &str = "On this PC";
/// How long the pointer rests on a card before it starts moving.
const CARD_HOVER_SECONDS: f64 = 1.0;
/// How long a clip a moving card pretends to be, and the pause before it starts again.
const CARD_CLIP_SECONDS: f32 = 3.0;
const CARD_PAUSE_SECONDS: f32 = 0.5;

/// A clip copied to be pasted, with what it needs to stand on its own.
#[derive(Clone, Debug)]
pub(super) struct CopiedClip {
    clip: Clip,
    title: Option<Title>,
    row: usize,
    sound_only: bool,
}

impl CopiedClip {
    /// Whether it is a clip of one of these videos, rather than a title of its own.
    pub(super) fn is_from_any(&self, media: &[MediaId]) -> bool {
        self.title.is_none() && media.contains(&self.clip.media)
    }
}

/// A corner of a title being dragged. The title is redrawn at its new size when it is let go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TitleResize {
    media: MediaId,
    size: i32,
    scale: i32,
}

/// A style's card in the Text panel, at rest.
pub(super) struct TitleCard {
    texture: TextureHandle,
    /// The scale that shows its picture at its own pixels on the card's canvas.
    scale: i32,
    /// Where a logo goes, as shares of the picture, on styles with a place for one.
    logo_spot: Option<[f32; 4]>,
}

/// The card the pointer is on, and what it needs to move.
pub(super) struct CardHover {
    style: TitleStyle,
    since: f64,
    /// Whether the pointer was on it this frame.
    seen: bool,
    moving: Option<MovingTitle>,
    frame: Option<TextureHandle>,
}

#[cfg(test)]
impl CardHover {
    pub(super) fn moving_yet(&self) -> Option<&MovingTitle> {
        self.moving.as_ref()
    }

    pub(super) fn wait_over(&mut self) {
        self.since -= CARD_HOVER_SECONDS;
    }

    /// Jumps this far into the card's motion, as if the pointer had rested on it that much longer.
    pub(super) fn play_to(&mut self, seconds: f64, now: f64) {
        self.since = now - CARD_HOVER_SECONDS - seconds;
    }
}

/// The Edit duration window, with what has been typed so far.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DurationEdit {
    clip: ClipId,
    pub(super) typed: String,
    just_opened: bool,
}

/// What the left panel is showing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum LeftPanel {
    #[default]
    Media,
    /// Ways of recording straight into the project.
    Record,
    /// Things BareClip draws itself to drop into a video.
    Library,
    Text,
    Transitions,
}

/// A line of the font list.
#[derive(Clone, Debug, PartialEq, Eq)]
enum FontRow {
    Heading(&'static str),
    Font(String),
}

/// The lines of the font list that match what was typed.
fn font_rows(list: &FontList, search: &str) -> Vec<FontRow> {
    let wanted = search.trim().to_lowercase();
    let matching = |fonts: &[String]| -> Vec<FontRow> {
        fonts
            .iter()
            .filter(|font| font.to_lowercase().contains(&wanted))
            .map(|font| FontRow::Font(font.clone()))
            .collect()
    };
    let mut rows = Vec::new();
    for (heading, fonts) in [
        (SHIPPED_HEADING, &list.shipped),
        (ON_THIS_PC_HEADING, &list.on_this_pc),
    ] {
        let found = matching(fonts);
        if !found.is_empty() {
            rows.push(FontRow::Heading(heading));
            rows.extend(found);
        }
    }
    rows
}

/// A picture ready for the screen, whose colours are already scaled by how see-through they are.
pub(super) fn texture_of(
    ctx: &egui::Context,
    name: String,
    picture: &StillPicture,
) -> TextureHandle {
    let image = egui::ColorImage::from_rgba_premultiplied(
        [picture.width as usize, picture.height as usize],
        &picture.rgba,
    );
    ctx.load_texture(name, image, TextureOptions::LINEAR)
}

/// The squares that mean "see-through" behind a style, so that a black outline or box shows up.
fn paint_checks(painter: &egui::Painter, palette: &Palette, rect: egui::Rect) {
    painter.rect_filled(rect, CARD_RADIUS, palette.raised);
    let columns = (rect.width() / CARD_CHECK).ceil() as usize;
    let rows = (rect.height() / CARD_CHECK).ceil() as usize;
    for row in 0..rows {
        for column in (row % 2..columns).step_by(2) {
            let corner = rect.left_top() + vec2(column as f32, row as f32) * CARD_CHECK;
            let check = egui::Rect::from_min_size(corner, vec2(CARD_CHECK, CARD_CHECK));
            painter.rect_filled(check.intersect(rect), 0.0, palette.surface);
        }
    }
}

/// The canvas a style's card draws it for.
fn card_canvas(style: TitleStyle) -> (u32, u32) {
    if style.fills_its_card() {
        FULL_CARD_CANVAS
    } else {
        CARD_CANVAS
    }
}

/// Where a style's picture goes on its card, and how many points each pixel of the card's canvas
/// takes up there.
fn card_view(style: TitleStyle, card: egui::Rect, picture: egui::Vec2) -> (egui::Rect, f32) {
    if style.fills_its_card() {
        // The canvas, edge to edge. Shrinking a wide canvas into
        // the square left a thin strip with words too small to read.
        let canvas = FULL_CARD_CANVAS;
        let across = card.width() / canvas.0 as f32;
        let down = card.height() / canvas.1 as f32;
        (card, across.max(down))
    } else {
        let room = card.shrink(CARD_ROOM);
        let fit = (room.width() / picture.x).min(room.height() / picture.y);
        (room, fit)
    }
}

/// Paints over a card's square corners outside its rounding, in the colour behind the card, for a
/// picture that runs to the card's edges.
fn round_off_corners(painter: &egui::Painter, rect: egui::Rect, radius: f32, colour: Color32) {
    const STEPS: u32 = 8;
    use std::f32::consts::{FRAC_PI_2, PI};
    let mut mesh = egui::Mesh::default();
    // Each corner, the middle of its rounding, and the angle its curve starts at, going clockwise
    // on the screen, where down is positive.
    for (corner, middle, start) in [
        (rect.left_top(), rect.left_top() + vec2(radius, radius), PI),
        (
            rect.right_top(),
            rect.right_top() + vec2(-radius, radius),
            PI + FRAC_PI_2,
        ),
        (
            rect.right_bottom(),
            rect.right_bottom() + vec2(-radius, -radius),
            0.0,
        ),
        (
            rect.left_bottom(),
            rect.left_bottom() + vec2(radius, -radius),
            FRAC_PI_2,
        ),
    ] {
        let first = mesh.vertices.len() as u32;
        mesh.colored_vertex(corner, colour);
        for step in 0..=STEPS {
            let angle = start + step as f32 / STEPS as f32 * FRAC_PI_2;
            mesh.colored_vertex(middle + radius * vec2(angle.cos(), angle.sin()), colour);
        }
        for step in 0..STEPS {
            mesh.add_triangle(first, first + 1 + step, first + 2 + step);
        }
    }
    painter.add(mesh);
}

/// Gets a slider ready to fill the panel, in BareClip's colours.
pub(super) fn style_slider(ui: &mut egui::Ui, palette: &Palette) {
    let visuals = &mut ui.style_mut().visuals;
    visuals.widgets.inactive.bg_fill = palette.border_strong;
    visuals.widgets.hovered.bg_fill = palette.border_strong;
    visuals.widgets.active.bg_fill = palette.border_strong;
    visuals.selection.bg_fill = palette.accent;
    ui.spacing_mut().slider_width = ui.available_width();
}

/// How much of a row the number beside a slider takes.
const SLIDER_NUMBER_ROOM: f32 = 56.0;

/// Shortens a slider that shows its number beside it, so the two fit the row. A slider as wide as
/// the row pushed its number past the panel's edge, which pushed the whole panel sideways.
pub(super) fn leave_room_for_number(ui: &mut egui::Ui) {
    ui.spacing_mut().slider_width = (ui.available_width() - SLIDER_NUMBER_ROOM).max(40.0);
}

/// Paints part of a picture, turned about its middle and partly see-through.
fn paint_turned(
    painter: &egui::Painter,
    texture: egui::TextureId,
    (centre, size): (egui::Pos2, egui::Vec2),
    degrees: f32,
    source: [f32; 4],
    opacity: f32,
) {
    let (sin, cos) = degrees.to_radians().sin_cos();
    let colour = Color32::WHITE.gamma_multiply(opacity);
    let corner = |across: f32, down: f32, u: f32, v: f32| {
        let (x, y) = (across * size.x / 2.0, down * size.y / 2.0);
        egui::epaint::Vertex {
            pos: centre + vec2(x * cos - y * sin, x * sin + y * cos),
            uv: pos2(u, v),
            color: colour,
        }
    };
    let [left, top, right, bottom] = source;
    let mut mesh = egui::epaint::Mesh::with_texture(texture);
    mesh.vertices.push(corner(-1.0, -1.0, left, top));
    mesh.vertices.push(corner(1.0, -1.0, right, top));
    mesh.vertices.push(corner(1.0, 1.0, right, bottom));
    mesh.vertices.push(corner(-1.0, 1.0, left, bottom));
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

fn colour32(colour: Colour) -> Color32 {
    Color32::from_rgb(colour[0], colour[1], colour[2])
}

impl Editor {
    /// The title a clip shows, if that is what it shows.
    pub(super) fn title_of(&self, clip: Clip) -> Option<&Title> {
        self.project.titles.get(&clip.media)
    }

    /// The words a title's clip has on it on the timeline.
    pub(super) fn title_label(title: &Title) -> String {
        if let Some(art) = &title.art {
            return art.entry().name.to_owned();
        }
        title
            .text
            .lines()
            .find(|line| !line.trim().is_empty())
            .map_or_else(|| title.style.name().to_owned(), str::to_owned)
    }

    pub(super) fn new_title_id(&mut self) -> MediaId {
        self.next_title += 1;
        MediaId::for_title(self.next_title)
    }

    /// Adds a title, and selects it. Without a target it goes on the top row of titles if that has
    /// room, or a new row above everything, and nothing else moves.
    pub(super) fn add_title(
        &mut self,
        style: TitleStyle,
        time: Ticks,
        target: Option<TrackTarget>,
    ) {
        self.record();
        let id = self.new_title_id();
        self.project.titles.insert(id, Title::in_style(style));
        let range = TimeRange::new_title();
        let tracks = &mut self.project.tracks;
        let placed = match target {
            Some(target) => tracks.place(id, range, time, target),
            None => tracks.place_title(id, range, time),
        };
        self.settle_canvas();
        self.selected = Some(placed);
        // The one time a panel opens by itself: a new title is there to be typed in.
        self.right_panel = Some(RightPanel::Tool(LookTab::Text));
        self.sync_titles();
        self.after_clip_added();
    }

    /// Gives a clip that has just been split or copied a title of its own, so changing one doesn't
    /// change the other.
    pub(super) fn give_own_title(&mut self, id: ClipId) {
        let Some(clip) = self.project.tracks.clip(id) else {
            return;
        };
        let Some(title) = self.title_of(clip).cloned() else {
            return;
        };
        let own = self.new_title_id();
        self.project.titles.insert(own, title);
        self.project.tracks.set_media(id, own);
        self.sync_titles();
    }

    /// The titles that need a picture now: near the playhead, and whichever is selected. Others
    /// have none, because a project can hold thousands of captions.
    fn titles_needed(&self) -> Vec<Clip> {
        let tracks = &self.project.tracks;
        let now = last_moment_by(self.position(), tracks.duration());
        let until = Ticks(now.0 + Ticks::from_seconds(PREPARE_AHEAD_SECONDS).0);
        tracks
            .picture_tracks()
            .iter()
            .filter(|track| track.clips().first().is_some_and(|clip| clip.is_title()))
            .flat_map(Timeline::clips)
            .filter(|clip| {
                (clip.start <= until && now < clip.end()) || self.selected == Some(clip.id)
            })
            .copied()
            .collect()
    }

    /// Makes sure the titles that show are drawn from what they now say, and that their clips fit
    /// those pictures. A title is only drawn when something about it changed.
    pub(super) fn sync_titles(&mut self) {
        let canvas = self.project.canvas.size();
        let held = self.title_resize.map(|resize| resize.media);
        let needed = self.titles_needed();
        self.media.retain(|entry| {
            !entry.id.is_title()
                || Some(entry.id) == held
                || needed.iter().any(|clip| clip.media == entry.id)
        });
        for clip in needed {
            if Some(clip.media) == held {
                continue;
            }
            let Some(title) = self.project.titles.get(&clip.media).cloned() else {
                continue;
            };
            let index = self
                .media
                .iter()
                .position(|entry| entry.id == clip.media)
                .unwrap_or_else(|| {
                    self.media.push(Self::title_entry(clip.media, &title));
                    self.media.len() - 1
                });
            let drawn = self.media[index]
                .title_drawn
                .as_ref()
                .filter(|drawn| drawn.0 == title && drawn.1 == canvas)
                .map(|drawn| drawn.2);
            let scale = match drawn {
                Some(scale) => scale,
                None => self.draw_title_into(index, &title, canvas),
            };
            let video = &self.media[index].info.video;
            let size = (video.width, video.height);
            let placed = title.placed(clip.placement, size, scale, canvas);
            if placed != clip.placement {
                self.project.tracks.set_placement(clip.id, placed);
            }
        }
    }

    /// Draws a title into its entry, and returns the scale that shows it right.
    fn draw_title_into(&mut self, index: usize, title: &Title, canvas: (u32, u32)) -> i32 {
        let logo = self.title_frames.logo(title);
        let drawing = draw_title_with_logo(title, canvas, logo.as_deref());
        if drawing.font_missing && !self.fonts_reported.contains(&title.font) {
            self.fonts_reported.push(title.font.clone());
            self.notice = Some(format!(
                "The font {} isn't on this PC, so {} is standing in for it.",
                title.font,
                bareclip_core::title::DEFAULT_FONT
            ));
        }
        let entry = &mut self.media[index];
        entry.info.video.width = drawing.picture.width;
        entry.info.video.height = drawing.picture.height;
        entry.still = Some(Arc::new(drawing.picture));
        entry.title_drawn = Some((title.clone(), canvas, drawing.scale));
        self.layer_pictures.picture_arrived();
        drawing.scale
    }

    /// How much footage a clip of this has to run through. A title may have no entry just now.
    pub(super) fn footage_of(&self, media: MediaId) -> Option<Ticks> {
        if media.is_title() {
            return Some(STILL_LENGTH);
        }
        self.entry(media).map(|entry| entry.info.duration)
    }

    /// What stands in for a file, for a title. Its size is filled in when it is drawn.
    fn title_entry(id: MediaId, title: &Title) -> MediaEntry {
        MediaEntry {
            id,
            info: MediaInfo {
                path: PathBuf::new(),
                container: "title".to_owned(),
                size_bytes: 0,
                duration: STILL_LENGTH,
                video: VideoInfo {
                    stream_index: 0,
                    codec: "title".to_owned(),
                    width: 1,
                    height: 1,
                    pixel_format: "rgba".to_owned(),
                    time_base: TimeBase { num: 1, den: 1000 },
                    frame_rate: 0.0,
                    is_hdr: false,
                    orientation: Default::default(),
                },
                audio: Vec::new(),
            },
            file_name: String::new(),
            name: Self::title_label(title),
            created: None,
            index: None,
            scan: None,
            cancel_scan: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(AtomicU32::new(0)),
            thumbnail: None,
            strip_pictures: Vec::new(),
            strip_pictures_done: true,
            still: None,
            sound_shape: None,
            strip_reader: None,
            title_drawn: None,
        }
    }

    /// Adds the titles a clip still shows to a project as it is written down. A title's scale, and
    /// where it is if it is in a position, come from drawing it, and most titles haven't been yet.
    /// So those are left out, or a project would seem changed just from being played.
    pub(super) fn write_titles_to(&self, file: &mut ProjectFile) {
        // Asking about each title in turn would go through every clip thousands of times.
        let used: std::collections::HashSet<u64> = file
            .tracks
            .iter()
            .flatten()
            .map(|clip| clip.media)
            .collect();
        file.titles = self
            .project
            .titles
            .iter()
            .filter(|(id, _)| used.contains(&id.0))
            .map(|(id, title)| project::SavedTitle {
                id: id.0,
                title: title.clone(),
            })
            .collect();
        for clip in file.tracks.iter_mut().flatten() {
            if let Some(title) = self.project.titles.get(&MediaId(clip.media)) {
                let start = Placement::default();
                clip.placement.scale = start.scale;
                if title.spot.is_some() {
                    clip.placement.center_x = start.center_x;
                    clip.placement.center_y = start.center_y;
                }
            }
        }
    }

    /// Puts saved titles back, keeping their ids so their clips find them.
    pub(super) fn restore_titles(&mut self, saved: &[project::SavedTitle]) {
        for saved in saved {
            let id = MediaId(saved.id);
            if id.is_title() {
                self.next_title = self.next_title.max(id.title_number());
                self.project
                    .titles
                    .insert(id, saved.title.clone().checked());
            }
        }
    }

    /// What a change on the stage means for a title. Its size is what a corner drag changes, and
    /// Reset goes back to how the style started.
    pub(super) fn title_placement(&mut self, id: ClipId, placement: Placement) -> Placement {
        let Some(clip) = self.project.tracks.clip(id).filter(|clip| clip.is_title()) else {
            return placement;
        };
        let Some(title) = self.project.titles.get_mut(&clip.media) else {
            return placement;
        };
        if placement.is_default() {
            title.size = title.starting_size();
            title.spot = Some(title.starting_spot());
            self.title_resize = None;
            // The next look at the title works out its scale and place from these.
            return placement;
        }
        if placement.center_x != clip.placement.center_x
            || placement.center_y != clip.placement.center_y
        {
            title.spot = None;
        }
        if placement.scale != clip.placement.scale {
            let resize = *self.title_resize.get_or_insert(TitleResize {
                media: clip.media,
                size: title.size,
                scale: clip.placement.scale,
            });
            let grown = f64::from(placement.scale) / f64::from(resize.scale.max(1));
            let (smallest, largest) = title.size_limits();
            title.size = ((f64::from(resize.size) * grown).round() as i32).clamp(smallest, largest);
        }
        placement
    }

    /// Changes the selected title, keeping every change one undo step until the box or slider
    /// being used is let go.
    pub(super) fn change_title(&mut self, id: MediaId, change: impl Fn(&mut Title)) {
        let Some(title) = self.project.titles.get(&id) else {
            return;
        };
        let mut changed = title.clone();
        change(&mut changed);
        if changed == *title {
            return;
        }
        if self.title_before.is_none() {
            self.title_before = Some(self.project.clone());
        }
        self.project.titles.insert(id, changed);
        self.show_selected_title();
    }

    /// The other captions a change to the selected one's look goes to, if any.
    pub(super) fn caption_mates(&self) -> Vec<MediaId> {
        let tracks = &self.project.tracks;
        let Some((clip, row)) = self
            .selected
            .and_then(|id| Some((tracks.clip(id)?, tracks.track_of(id)?)))
        else {
            return Vec::new();
        };
        let is_caption = |id: MediaId| {
            self.project
                .titles
                .get(&id)
                .is_some_and(|t| t.style == TitleStyle::Subtitle)
        };
        if !is_caption(clip.media) {
            return Vec::new();
        }
        tracks.tracks()[row]
            .clips()
            .iter()
            .filter(|other| other.id != clip.id && is_caption(other.media))
            .map(|other| other.media)
            .collect()
    }

    /// Changes how a title looks, and every caption on its row too if that is switched on.
    pub(super) fn change_title_look(&mut self, id: MediaId, change: impl Fn(&mut Title)) {
        if self.style_whole_row {
            for mate in self.caption_mates() {
                self.change_title(mate, &change);
            }
        }
        self.change_title(id, &change);
    }

    /// Enter in a caption's text box: goes on to the caption straight after this one on the row, or
    /// starts a new one there, as long as this one and only as long as there is room for.
    pub(super) fn next_caption(&mut self) {
        let tracks = &self.project.tracks;
        let Some((clip, row)) = self
            .selected
            .and_then(|id| Some((tracks.clip(id)?, tracks.track_of(id)?)))
        else {
            return;
        };
        let Some(title) = self.title_of(clip).cloned() else {
            return;
        };
        let next = tracks.tracks()[row]
            .clips()
            .iter()
            .find(|other| other.start >= clip.end())
            .copied();
        let room = next.map(|next| Ticks(next.start.0 - clip.end().0));
        self.finish_title_edit();
        self.focus_title_text = true;
        let touching = Ticks::from_seconds(CAPTIONS_TOUCHING_SECONDS);
        if let (Some(next), Some(room)) = (next, room)
            && room <= touching
        {
            self.selected = Some(next.id);
            self.seek(next.start);
            return;
        }
        self.record();
        let id = self.new_title_id();
        self.project.titles.insert(
            id,
            Title {
                text: String::new(),
                second_text: String::new(),
                ..title
            },
        );
        let length = room.map_or(clip.duration(), |room| room.min(clip.duration()));
        let source = TimeRange {
            start: clip.source.start,
            end: Ticks(clip.source.start.0 + length.0),
        };
        let copy = Clip { source, ..clip };
        let added =
            self.project
                .tracks
                .place_copy(copy, id, clip.end(), TrackTarget::Existing(row), false);
        self.selected = Some(added);
        self.sync_titles();
        self.seek(clip.end());
    }

    /// Asks for a subtitle file to import.
    pub(super) fn pick_captions(&mut self) {
        let picked = rfd::FileDialog::new()
            .set_title("Import captions")
            .add_filter("Subtitle files", &[text::CAPTIONS_EXTENSION])
            .add_filter("All files", &["*"])
            .pick_file();
        if let Some(path) = picked {
            self.import_captions(&path);
        }
    }

    /// Adds every caption in a subtitle file at its own time, as one undo step. Nothing else moves,
    /// and they go on a new row above everything.
    pub(super) fn import_captions(&mut self, path: &Path) {
        let name = path.file_name().map_or_else(
            || "That file".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.notice = Some(format!("{name} could not be opened. {error}"));
                return;
            }
        };
        let cues = parse_srt(&decode_subtitles(&bytes));
        if cues.is_empty() {
            self.notice = Some(format!("{name} has no captions BareClip can read."));
            return;
        }
        self.record();
        let source = TimeRange::new_title();
        let placed: Vec<(MediaId, TimeRange, Ticks)> = cues
            .iter()
            .map(|cue| {
                let id = self.new_title_id();
                let title = Title {
                    text: cue.text.clone(),
                    ..Title::in_style(TitleStyle::Subtitle)
                };
                self.project.titles.insert(id, title);
                let end = Ticks(source.start.0 + cue.end.0 - cue.start.0);
                let lasting = TimeRange {
                    start: source.start,
                    end,
                };
                (id, lasting, cue.start)
            })
            .collect();
        self.project.tracks.place_captions(&placed);
        self.selected = None;
        self.settle_canvas();
        self.after_clip_added();
    }

    /// A change to a title that isn't on screen would look like nothing happened, so the playhead
    /// goes to the start of it.
    fn show_selected_title(&mut self) {
        let Some(clip) = self.selected.and_then(|id| self.project.tracks.clip(id)) else {
            return;
        };
        let position = self.position();
        if position < clip.start || position >= clip.end() {
            self.seek(clip.start);
        }
    }

    /// Finishes the undo step for a change to a title, once nothing is being held or typed in.
    pub(super) fn settle_title_edit(&mut self, ctx: &egui::Context) {
        let busy = ctx.input(|input| input.pointer.any_down()) || ctx.egui_wants_keyboard_input();
        if !busy {
            self.finish_title_edit();
        }
    }

    /// Closes off the changes made to a title so far as one undo step.
    pub(super) fn finish_title_edit(&mut self) {
        if let Some(before) = self.title_before.take()
            && before != self.project
        {
            self.history.record(before);
        }
    }

    pub(super) fn copy_clip(&mut self, id: ClipId) {
        let tracks = &self.project.tracks;
        let (Some(clip), Some(row)) = (tracks.clip(id), tracks.track_of(id)) else {
            return;
        };
        self.clipboard = Some(CopiedClip {
            clip,
            title: self.title_of(clip).cloned(),
            row,
            sound_only: tracks.is_sound_row(row),
        });
    }

    /// Adds a copy of the copied clip at the playhead, on the row it came from if it can.
    pub(super) fn paste_clip(&mut self) {
        let Some(copied) = self.clipboard.clone() else {
            return;
        };
        self.record();
        let media = match copied.title {
            Some(title) => {
                let own = self.new_title_id();
                self.project.titles.insert(own, title);
                own
            }
            None => copied.clip.media,
        };
        let time = self.position();
        let pasted = self.project.tracks.place_copy(
            copied.clip,
            media,
            time,
            TrackTarget::Existing(copied.row),
            copied.sound_only,
        );
        self.sync_titles();
        self.selected = Some(pasted);
        self.settle_canvas();
        self.after_clip_added();
    }

    pub(super) fn open_edit_duration(&mut self, id: ClipId) {
        if let Some(clip) = self.project.tracks.clip(id) {
            self.duration_edit = Some(DurationEdit {
                clip: id,
                typed: text::duration_to_type(clip.duration()),
                just_opened: true,
            });
        }
    }

    /// Applies a typed length to a clip. Returns whether what was typed could be read.
    pub(super) fn apply_duration(&mut self, id: ClipId, typed: &str) -> bool {
        let Some(seconds) = text::typed_duration(typed) else {
            return false;
        };
        let Some(clip) = self.project.tracks.clip(id) else {
            return true;
        };
        let Some(footage) = self.footage_of(clip.media) else {
            return true;
        };
        let wanted = Ticks::from_seconds(seconds.max(MIN_CLIP_SECONDS));
        let before = self.project.clone();
        if self.project.tracks.set_duration(id, wanted, footage) {
            self.history.record(before);
            self.after_timeline_change();
        }
        let got = self.project.tracks.clip(id).map(|clip| clip.duration());
        if got.is_some_and(|got| got.0 + Ticks::from_seconds(0.01).0 < wanted.0) {
            self.notice = Some("That is as long as this clip's footage lasts.".to_owned());
        }
        true
    }

    pub(super) fn edit_duration_dialog(&mut self, ctx: &egui::Context, palette: &Palette) {
        let Some(mut edit) = self.duration_edit.clone() else {
            return;
        };
        let mut close = false;
        let mut unreadable = false;
        egui::Modal::new(egui::Id::unique("edit_duration_dialog"))
            .frame(dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width(300.0);
                components::dialog_title(ui, &icons::CLOCK, palette.text, "Edit duration");
                ui.add_space(8.0);
                ui.colored_label(
                    palette.text_muted,
                    "How long this clip lasts, such as 5 or 1:30.",
                );
                ui.add_space(8.0);
                let field = ui.add(
                    egui::TextEdit::singleline(&mut edit.typed).desired_width(ui.available_width()),
                );
                field.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Duration")
                });
                if edit.just_opened {
                    field.request_focus();
                    edit.just_opened = false;
                }
                let entered =
                    field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                ui.add_space(12.0);
                let mut apply = entered;
                components::dialog_buttons(ui, |ui| {
                    if ui.add(dialog_button(palette, "Cancel")).clicked() {
                        close = true;
                    }
                    if ui
                        .add(ActionButton::new(palette, Kind::Primary, "Apply"))
                        .clicked()
                    {
                        apply = true;
                    }
                });
                if apply {
                    if self.apply_duration(edit.clip, &edit.typed) {
                        close = true;
                    } else {
                        unreadable = true;
                    }
                }
            });
        if unreadable {
            self.notice = Some("Type a length such as 5 or 1:30.".to_owned());
        }
        self.duration_edit = (!close).then_some(edit);
    }

    /// The Text panel's list of styles.
    pub(super) fn text_panel(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let mut add = None;
        if let Some(hover) = &mut self.card_hover {
            hover.seen = false;
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let width = ui.available_width();
                let columns = CARD_COLUMNS as f32;
                let card = (width - CARD_GAP * (columns - 1.0)) / columns;
                if self.all_text_styles {
                    let back = IconButton::new(
                        palette,
                        Kind::Ghost,
                        icons::ARROW_LEFT,
                        "Back to all the text",
                    );
                    ui.horizontal(|ui| {
                        if ui.add(back).clicked() {
                            self.all_text_styles = false;
                        }
                        ui.label(RichText::new(TEXT_STYLES_HEADING).font(fonts::strong(13.0)));
                    });
                    ui.add_space(6.0);
                }
                for (heading, styles) in STYLE_GROUPS {
                    let is_text_styles = heading == TEXT_STYLES_HEADING;
                    if self.all_text_styles && !is_text_styles {
                        continue;
                    }
                    let shown = if is_text_styles && !self.all_text_styles {
                        &styles[..TEXT_STYLES_SHOWN]
                    } else {
                        styles
                    };
                    if !self.all_text_styles {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(heading).font(fonts::strong(13.0)));
                            if is_text_styles {
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    let see_all =
                                        ActionButton::new(palette, Kind::Ghost, "See all")
                                            .icon(icons::CHEVRON_RIGHT);
                                    if ui.add(see_all).clicked() {
                                        self.all_text_styles = true;
                                    }
                                });
                            }
                        });
                        ui.add_space(6.0);
                    }
                    if styles.contains(&TitleStyle::Subtitle) {
                        let button = ActionButton::new(palette, Kind::Secondary, "Import SRT file")
                            .icon(icons::FILE_PLUS)
                            .width(width);
                        if ui
                            .add(button)
                            .on_hover_text("Adds each caption in a subtitle file at its own time")
                            .clicked()
                        {
                            self.pick_captions_requested = true;
                        }
                        ui.add_space(CARD_GAP);
                    }
                    for row in shown.chunks(CARD_COLUMNS) {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = CARD_GAP;
                            for style in row {
                                if self.style_card(ui, palette, *style, card) {
                                    add = Some(*style);
                                }
                            }
                        });
                        ui.add_space(CARD_GAP);
                    }
                    ui.add_space(6.0);
                }
            });
        if self.card_hover.as_ref().is_some_and(|hover| !hover.seen) {
            self.card_hover = None;
        }
        if let Some(style) = add {
            let time = self.position();
            self.add_title(style, time, None);
        }
    }

    /// A style to press or drag onto the timeline. Returns whether it was pressed.
    fn style_card(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        style: TitleStyle,
        width: f32,
    ) -> bool {
        let (rect, response) = ui.allocate_exact_size(vec2(width, width), Sense::click_and_drag());
        let name = format!("Add {}", style.name());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
        if response.drag_started() {
            response.dnd_set_drag_payload(MediaDrag {
                media: MediaId::for_title(0),
                length: TimeRange::new_title().duration(),
                sound_only: false,
                title: Some(style),
                art: None,
            });
        }
        if !ui.is_rect_visible(rect) {
            return response.clicked();
        }
        let ctx = ui.ctx().clone();
        let card = self.title_cards.entry(style).or_insert_with(|| {
            let drawing = draw_title(&Title::in_style(style), card_canvas(style));
            TitleCard {
                texture: texture_of(&ctx, format!("title-card-{style:?}"), &drawing.picture),
                scale: drawing.scale,
                logo_spot: drawing.logo_spot,
            }
        });
        let (rest_texture, rest_scale, logo_spot) =
            (card.texture.clone(), card.scale, card.logo_spot);
        let painter = ui.painter_at(rect);
        paint_checks(&painter, palette, rect);
        let picture = rest_texture.size_vec2();
        let fills = style.fills_its_card();
        let (room, fit) = card_view(style, rect, picture);
        let hovering = response.hovered() && !style.motion().is_still();
        let moved = hovering
            && self.paint_moving_card(&ctx, &painter, style, room, picture, (fit, rest_scale));
        if !moved {
            painter.image(
                rest_texture.id(),
                egui::Rect::from_center_size(room.center(), picture * fit),
                egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            if let Some([left, top, wide, tall]) = logo_spot {
                // Only a card shows this. Nothing shows in the video until a logo is chosen.
                let shown = egui::Rect::from_center_size(room.center(), picture * fit);
                let middle = shown.left_top()
                    + vec2(
                        (left + wide / 2.0) * shown.width(),
                        (top + tall / 2.0) * shown.height(),
                    );
                let reach = (wide * shown.width()).min(tall * shown.height()) / 2.0;
                painter.circle(
                    middle,
                    reach,
                    palette.raised,
                    Stroke::new(1.0, palette.text_muted),
                );
            }
        }
        if fills {
            round_off_corners(&painter, rect, CARD_RADIUS, palette.surface);
        }
        if response.hovered() {
            painter.rect_stroke(
                rect,
                CARD_RADIUS,
                Stroke::new(2.0, palette.accent),
                egui::StrokeKind::Inside,
            );
            let badge =
                egui::Rect::from_min_size(rect.left_top() + vec2(6.0, 6.0), vec2(22.0, 22.0));
            painter.circle_filled(badge.center(), 11.0, palette.accent);
            icons::paint(
                &painter,
                icons::square_at(badge.center(), 14.0),
                &icons::PLUS,
                palette.accent_text,
            );
            let hint = format!(
                "{}: press to add at the playhead, or drag to the timeline",
                style.name()
            );
            response.clone().on_hover_text(hint);
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        response.clicked()
    }

    /// Plays a card's motion, once the pointer has rested on it long enough. Returns whether it
    /// painted anything, which it doesn't until then.
    fn paint_moving_card(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        style: TitleStyle,
        room: egui::Rect,
        picture: egui::Vec2,
        (fit, rest_scale): (f32, i32),
    ) -> bool {
        let now = ctx.input(|input| input.time);
        let same = self
            .card_hover
            .as_ref()
            .is_some_and(|hover| hover.style == style);
        if !same {
            self.card_hover = Some(CardHover {
                style,
                since: now,
                seen: true,
                moving: None,
                frame: None,
            });
        }
        let Some(hover) = &mut self.card_hover else {
            return false;
        };
        hover.seen = true;
        let waited = now - hover.since;
        if waited < CARD_HOVER_SECONDS {
            // Asks for one frame when the wait ends, so an idle window stays idle until then.
            ctx.request_repaint_after(Duration::from_secs_f64(CARD_HOVER_SECONDS - waited));
            return false;
        }
        ctx.request_repaint();
        let canvas = card_canvas(style);
        let title = Title {
            spot: Some(TitleSpot::Middle),
            ..Title::in_style(style)
        };
        let motion = style.motion();
        let looped =
            (waited - CARD_HOVER_SECONDS) as f32 % (CARD_CLIP_SECONDS + CARD_PAUSE_SECONDS);
        let seconds = looped.min(CARD_CLIP_SECONDS);
        let stage = motion.stage(seconds, CARD_CLIP_SECONDS);
        // The pause at the end of a loop leaves the card empty for a moment, like a clip ending.
        let pose = motion.pose(stage);
        let rest_size = (picture.x as u32, picture.y as u32);
        let frame_size = if motion.draws_frames() && motion.draws_frame_at(stage) {
            let moving = hover
                .moving
                .get_or_insert_with(|| MovingTitle::prepare(&title, canvas, None));
            let frame = moving.frame(stage);
            let size = (frame.width, frame.height);
            let image = egui::ColorImage::from_rgba_premultiplied(
                [frame.width as usize, frame.height as usize],
                &frame.rgba,
            );
            match &mut hover.frame {
                Some(texture) => texture.set(image, TextureOptions::LINEAR),
                None => {
                    hover.frame =
                        Some(ctx.load_texture("title-card-moving", image, TextureOptions::LINEAR));
                }
            }
            Some((size, moving.scale_for(size)))
        } else {
            None
        };
        let (size, scale, texture) = match (frame_size, &hover.frame) {
            (Some((size, scale)), Some(texture)) => (size, scale, texture.id()),
            _ => {
                let rest = self.title_cards.get(&style).map(|card| card.texture.id());
                let Some(rest) = rest else {
                    return false;
                };
                (rest_size, rest_scale, rest)
            }
        };
        let rest = Placement {
            scale,
            ..Placement::default()
        };
        let placed = pose.on(rest, size, canvas);
        let rect = placed.rect(size, canvas);
        let look = placed.look();
        let opacity = placed.opacity as f32 / WHOLE as f32;
        let canvas_middle = vec2(canvas.0 as f32, canvas.1 as f32) / 2.0;
        let middle = vec2(rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
        let centre = room.center() + (middle - canvas_middle) * fit;
        paint_turned(
            painter,
            texture,
            (centre, vec2(rect.width, rect.height) * fit),
            look.degrees,
            look.source,
            opacity,
        );
        true
    }

    /// The selected clip, if it is a title.
    pub(super) fn selected_title(&self) -> Option<(MediaId, Title)> {
        let clip = self.selected.and_then(|id| self.project.tracks.clip(id))?;
        Some((clip.media, self.title_of(clip)?.clone()))
    }

    pub(super) fn title_controls(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        id: MediaId,
        title: &Title,
    ) {
        let width = ui.available_width();
        let heading = |ui: &mut egui::Ui, words: &str| {
            ui.add_space(6.0);
            ui.label(RichText::new(words).font(fonts::strong(13.0)));
            ui.add_space(4.0);
        };

        let is_caption = title.style == TitleStyle::Subtitle;
        let counts_down = title.style == TitleStyle::Countdown;
        if counts_down {
            ui.colored_label(
                palette.text_muted,
                RichText::new("Counts down the length of this clip.").size(12.0),
            );
        }
        let mut words = title.text.clone();
        let mut text_box = egui::TextEdit::multiline(&mut words)
            .id(egui::Id::unique("title-text"))
            .desired_rows(2)
            .desired_width(width);
        if is_caption {
            text_box = text_box.return_key(Some(egui::KeyboardShortcut::new(
                Modifiers::SHIFT,
                Key::Enter,
            )));
        }
        let field = ui.add_enabled(!counts_down, text_box);
        field.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Title text"));
        if std::mem::take(&mut self.focus_title_text) {
            field.request_focus();
        }
        if field.changed() {
            self.change_title(id, |title| title.text = words.clone());
        }
        if is_caption {
            let pressed_enter =
                ui.input(|input| input.key_pressed(Key::Enter) && input.modifiers.is_none());
            if field.has_focus() && pressed_enter {
                self.next_caption();
            }
            ui.add_space(2.0);
            ui.colored_label(
                palette.text_muted,
                RichText::new("Enter goes to the next caption, and Shift+Enter starts a new line.")
                    .size(12.0),
            );
            if !self.caption_mates().is_empty() {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let name = "Change every caption on this row";
                    if ui
                        .add(CheckBox::new(palette, self.style_whole_row, name))
                        .on_hover_text("Font, size, colours and position go to all of them")
                        .clicked()
                    {
                        self.style_whole_row = !self.style_whole_row;
                    }
                    ui.colored_label(
                        palette.text_muted,
                        RichText::new("Change all on this row").size(12.0),
                    );
                });
            }
        }
        if title.style.look().second_line.is_some() {
            heading(ui, "Second line");
            let mut words = title.second_text.clone();
            // Credits run to many lines, where every other style's second line is just the one.
            let second_box = if title.style == TitleStyle::CreditsRoll {
                egui::TextEdit::multiline(&mut words).desired_rows(4)
            } else {
                egui::TextEdit::singleline(&mut words)
            };
            let field = ui.add(second_box.desired_width(width));
            field.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Second line text")
            });
            if field.changed() {
                self.change_title(id, |title| title.second_text = words.clone());
            }
        }

        heading(ui, "Font");
        self.font_picker(ui, palette, id, title, width);

        heading(ui, "Size");
        self.size_slider(ui, palette, id, title);

        ui.add_space(6.0);
        if let Some(colour) = self.colour_row(ui, palette, "Text colour", Some(title.colour), false)
        {
            self.change_title_look(id, |title| title.colour = colour.unwrap_or(title.colour));
        }
        if let Some(colour) =
            self.colour_row(ui, palette, "Background colour", title.background, true)
        {
            self.change_title_look(id, |title| title.background = colour);
        }
        if title.style.has_accent()
            && let Some(colour) =
                self.colour_row(ui, palette, "Accent colour", Some(title.accent), false)
        {
            self.change_title_look(id, |title| title.accent = colour.unwrap_or(title.accent));
        }

        if title.style == TitleStyle::Rating {
            heading(ui, "Stars");
            self.stars_slider(ui, palette, id, title);
        }
        if title.style.has_logo() {
            self.logo_controls(ui, palette, id, title, width);
        }

        heading(ui, "Position");
        self.position_grid(ui, palette, id, title);
    }

    fn stars_slider(&mut self, ui: &mut egui::Ui, palette: &Palette, id: MediaId, title: &Title) {
        style_slider(ui, palette);
        leave_room_for_number(ui);
        let mut stars = u32::from(title.stars);
        let slider = ui.add(
            egui::Slider::new(&mut stars, 0..=5)
                .show_value(true)
                .trailing_fill(true),
        );
        slider.widget_info(|| {
            egui::WidgetInfo::slider(slider.enabled(), f64::from(stars), "Stars lit")
        });
        if slider.changed() {
            self.change_title_look(id, |title| title.stars = stars as u8);
        }
        if slider.drag_stopped() || slider.clicked() {
            slider.surrender_focus();
        }
    }

    /// Choosing a logo.
    fn logo_controls(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        id: MediaId,
        title: &Title,
        width: f32,
    ) {
        ui.add_space(6.0);
        ui.label(RichText::new("Logo size").font(fonts::strong(13.0)));
        ui.add_space(4.0);
        style_slider(ui, palette);
        let whole = f64::from(WHOLE);
        let mut size = f64::from(title.logo_size) / whole;
        let range = f64::from(SMALLEST_SIZE) / whole..=f64::from(LARGEST_SIZE) / whole;
        let slider = ui.add(
            egui::Slider::new(&mut size, range)
                .show_value(false)
                .trailing_fill(true),
        );
        slider
            .widget_info(|| egui::WidgetInfo::slider(slider.enabled(), size * 100.0, "Logo size"));
        if slider.changed() {
            self.change_title_look(id, |title| title.logo_size = (size * whole).round() as i32);
        }
        if slider.drag_stopped() || slider.clicked() {
            slider.surrender_focus();
        }
        ui.add_space(6.0);
        let import = ActionButton::new(palette, Kind::Secondary, "Import logo")
            .icon(icons::IMAGE)
            .width(width);
        if ui.add(import).clicked() {
            self.pick_logo_for = Some(id);
        }
        if title.logo.is_some() {
            ui.add_space(4.0);
            let none = ActionButton::new(palette, Kind::Ghost, "No logo")
                .icon(icons::X)
                .width(width);
            if ui.add(none).clicked() {
                self.change_title_look(id, |title| title.logo = None);
            }
        }
    }

    /// Asks which picture to use as a title's logo.
    pub(super) fn pick_logo(&mut self, id: MediaId) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Import logo")
            .add_filter("Images", &["png", "jpg", "jpeg", "webp", "bmp"])
            .pick_file()
        else {
            return;
        };
        self.set_title_logo(id, path);
    }

    /// Gives a title a logo, if the file can be read as a picture.
    pub(super) fn set_title_logo(&mut self, id: MediaId, path: PathBuf) {
        if bareclip_media::read_still(&path).is_err() {
            self.notice = Some(format!("{} can't be read as a picture.", path.display()));
            return;
        }
        self.change_title_look(id, |title| title.logo = Some(path.clone()));
    }

    fn size_slider(&mut self, ui: &mut egui::Ui, palette: &Palette, id: MediaId, title: &Title) {
        style_slider(ui, palette);
        let whole = f64::from(WHOLE);
        let mut size = f64::from(title.size) / whole;
        let range = f64::from(SMALLEST_SIZE) / whole..=f64::from(LARGEST_SIZE) / whole;
        let slider = ui.add(
            egui::Slider::new(&mut size, range)
                .show_value(false)
                .trailing_fill(true),
        );
        slider
            .widget_info(|| egui::WidgetInfo::slider(slider.enabled(), size * 100.0, "Title size"));
        if slider.changed() {
            self.change_title_look(id, |title| title.size = (size * whole).round() as i32);
        }
        // Holding on to the keyboard would stop the arrow keys moving the playhead.
        if slider.drag_stopped() || slider.clicked() {
            slider.surrender_focus();
        }
    }

    /// A named colour with a swatch that opens the choices. The outer `Option` says whether a
    /// colour was picked, and the inner one is `None` for no colour at all.
    pub(super) fn colour_row(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        name: &str,
        current: Option<Colour>,
        can_be_none: bool,
    ) -> Option<Option<Colour>> {
        let mut picked = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new(name).font(fonts::strong(13.0)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let shown = current.map_or(Color32::TRANSPARENT, colour32);
                let button = ui.add(ColourSwatch::new(palette, shown, name, false));
                if current.is_none() {
                    ui.painter().line_segment(
                        [
                            button.rect.left_bottom() + vec2(4.0, -4.0),
                            button.rect.right_top() + vec2(-4.0, 4.0),
                        ],
                        Stroke::new(1.5, palette.text_muted),
                    );
                }
                egui::Popup::menu(&button)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .show(|ui| {
                        if can_be_none
                            && ui
                                .add(MenuItem::new(palette, icons::X, "No colour", ""))
                                .clicked()
                        {
                            picked = Some(None);
                        }
                        let start = current.unwrap_or([0, 0, 0]);
                        if let Some(colour) = components::colour_choices(
                            ui,
                            palette,
                            start,
                            &mut self.title_colour_text,
                        ) {
                            picked = Some(Some(colour));
                        }
                    });
            });
        });
        ui.add_space(4.0);
        picked
    }

    /// The nine positions, with the one the title is in lit up.
    fn position_grid(&mut self, ui: &mut egui::Ui, palette: &Palette, id: MediaId, title: &Title) {
        let mut picked = None;
        ui.spacing_mut().item_spacing = vec2(SPOT_GAP, SPOT_GAP);
        for row in TitleSpot::ALL.chunks(3) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = vec2(SPOT_GAP, SPOT_GAP);
                for (spot, name) in row {
                    let button =
                        components::SpotButton::new(palette, name, title.spot == Some(*spot));
                    if ui.add(button).on_hover_text(*name).clicked() {
                        picked = Some(*spot);
                    }
                }
            });
        }
        if let Some(spot) = picked {
            self.change_title_look(id, |title| title.spot = Some(spot));
        }
    }

    /// The font box, and the list it opens.
    fn font_picker(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        id: MediaId,
        title: &Title,
        width: f32,
    ) {
        let name = format!("Font: {}", title.font);
        let button = ui.add(components::Dropdown::new(
            palette,
            &name,
            &title.font,
            width,
        ));
        let list = self.font_list.get_or_insert_with(font_list).clone();
        let mut picked = None;
        egui::Popup::menu(&button)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.set_width(FONT_LIST_WIDTH);
                let search = ui.add(
                    egui::TextEdit::singleline(&mut self.font_search)
                        .hint_text("Search fonts")
                        .desired_width(FONT_LIST_WIDTH),
                );
                search.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Search fonts")
                });
                ui.add_space(6.0);
                let rows = font_rows(&list, &self.font_search);
                if rows.is_empty() {
                    ui.colored_label(palette.text_muted, "No fonts match that.");
                    return;
                }
                let mut left_to_draw = FONTS_DRAWN_PER_FRAME;
                egui::ScrollArea::vertical()
                    .max_height(FONT_LIST_HEIGHT)
                    .auto_shrink([false, false])
                    .show_rows(ui, FONT_ROW_HEIGHT, rows.len(), |ui, range| {
                        for row in &rows[range] {
                            match row {
                                FontRow::Heading(heading) => {
                                    let (rect, _) = ui.allocate_exact_size(
                                        vec2(FONT_LIST_WIDTH, FONT_ROW_HEIGHT),
                                        Sense::hover(),
                                    );
                                    ui.painter().text(
                                        rect.left_center() + vec2(6.0, 0.0),
                                        egui::Align2::LEFT_CENTER,
                                        *heading,
                                        fonts::strong(13.0),
                                        palette.text_muted,
                                    );
                                }
                                FontRow::Font(font) => {
                                    if self.font_row(
                                        ui,
                                        palette,
                                        font,
                                        *font == title.font,
                                        &mut left_to_draw,
                                    ) {
                                        picked = Some(font.clone());
                                    }
                                }
                            }
                        }
                    });
                if left_to_draw == 0 {
                    ui.ctx().request_repaint();
                }
            });
        if let Some(font) = picked {
            self.change_title_look(id, |title| title.font = font.clone());
            egui::Popup::close_all(ui.ctx());
        }
    }

    /// One font in the list, shown in itself. Returns whether it was pressed.
    fn font_row(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        font: &str,
        chosen: bool,
        left_to_draw: &mut usize,
    ) -> bool {
        let (rect, response) =
            ui.allocate_exact_size(vec2(FONT_LIST_WIDTH, FONT_ROW_HEIGHT), Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, font));
        if !ui.is_rect_visible(rect) {
            return response.clicked();
        }
        if response.hovered() {
            ui.painter().rect_filled(rect, 4.0, palette.raised);
        }
        let text_left = rect.left() + 26.0;
        if chosen {
            icons::paint(
                ui.painter(),
                icons::square_at(rect.left_center() + vec2(13.0, 0.0), 14.0),
                &icons::CHECK,
                palette.text,
            );
        }
        let scale = ui.ctx().pixels_per_point();
        if !self.font_faces.contains_key(font) && *left_to_draw > 0 {
            *left_to_draw -= 1;
            let face = draw_font_name(font, FONT_ROW_TEXT * scale, [255, 255, 255])
                .map(|picture| texture_of(ui.ctx(), format!("font-{font}"), &picture));
            self.font_faces.insert(font.to_owned(), face);
        }
        match self.font_faces.get(font) {
            Some(Some(texture)) => {
                let size = texture.size_vec2() / scale;
                let room = rect.right() - text_left - 6.0;
                let shrink = (room / size.x)
                    .min((FONT_ROW_HEIGHT - 2.0) / size.y)
                    .min(1.0);
                let shown = size * shrink;
                let top = rect.center().y - shown.y / 2.0;
                ui.painter().image(
                    texture.id(),
                    egui::Rect::from_min_size(pos2(text_left, top), shown),
                    egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    palette.text,
                );
            }
            // Not drawn yet, or a font with no way to show its own name.
            _ => {
                ui.painter().text(
                    pos2(text_left, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    font,
                    egui::FontId::proportional(14.0),
                    palette.text,
                );
            }
        }
        response.clicked()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> FontList {
        FontList {
            shipped: vec!["Abel".to_owned(), "DM Sans".to_owned()],
            on_this_pc: vec!["Arial".to_owned(), "Calibri".to_owned()],
        }
    }

    #[test]
    fn a_style_across_the_whole_canvas_fills_its_card_edge_to_edge() {
        let card = egui::Rect::from_min_size(pos2(10.0, 20.0), vec2(103.0, 103.0));
        for style in [
            TitleStyle::Circular,
            TitleStyle::IntroClean,
            TitleStyle::Rapid,
        ] {
            let canvas = card_canvas(style);
            let drawing = draw_title(&Title::in_style(style), canvas);
            let picture = vec2(drawing.picture.width as f32, drawing.picture.height as f32);
            let (room, fit) = card_view(style, card, picture);
            assert_eq!(room, card, "{style:?} uses the whole card");
            let shown = vec2(canvas.0 as f32, canvas.1 as f32) * fit;
            assert!(
                (shown.x - card.width()).abs() < 0.01 && (shown.y - card.height()).abs() < 0.01,
                "{style:?}'s canvas is the card's shape and covers it, not {shown:?}"
            );
        }
        // Laid out on a square, a frame goes around all four sides.
        let frame = draw_title(&Title::in_style(TitleStyle::IntroClean), FULL_CARD_CANVAS);
        assert_eq!(
            (frame.picture.width, frame.picture.height),
            FULL_CARD_CANVAS
        );
    }

    #[test]
    fn other_styles_fit_inside_their_card_with_room_around() {
        let card = egui::Rect::from_min_size(pos2(10.0, 20.0), vec2(103.0, 103.0));
        let style = TitleStyle::Typewriter;
        assert_eq!(card_canvas(style), CARD_CANVAS);
        let drawing = draw_title(&Title::in_style(style), CARD_CANVAS);
        let picture = vec2(drawing.picture.width as f32, drawing.picture.height as f32);
        let (room, fit) = card_view(style, card, picture);
        assert_eq!(room, card.shrink(CARD_ROOM));
        let shown = picture * fit;
        assert!(shown.x <= room.width() + 0.01 && shown.y <= room.height() + 0.01);
    }

    #[test]
    fn the_font_list_goes_under_two_headings() {
        let rows = font_rows(&list(), "");
        assert_eq!(rows.len(), 6);
        assert_eq!(rows[0], FontRow::Heading(SHIPPED_HEADING));
        assert_eq!(rows[3], FontRow::Heading(ON_THIS_PC_HEADING));
    }

    #[test]
    fn searching_ignores_case_and_a_heading_with_nothing_under_it_goes() {
        let rows = font_rows(&list(), " ari");
        assert_eq!(
            rows,
            vec![
                FontRow::Heading(ON_THIS_PC_HEADING),
                FontRow::Font("Arial".to_owned())
            ]
        );
        assert!(font_rows(&list(), "nothing like this").is_empty());
    }
}
