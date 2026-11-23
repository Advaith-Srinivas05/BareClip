//! The pill of playing controls that floats over the bottom of the picture, laid out as the
//! Photos app's: play, jump back, jump forward, the volume, the time, the seek line, the
//! length, loop and save frame. In a narrow window the jumps, loop and save frame go into a
//! "…" at its end and the times hide, as Photos does, so the seek line keeps its room.
//!
//! Its size was measured from Photos at three window sizes and with Info open: always 56% as
//! wide as the room the picture has, 56 px tall and about 32 px above the status bar at 125%.
//! The trimmer's pill follows the same rule.

use eframe::egui::{
    self, Align, Align2, FontId, Layout, Rect, Sense, Stroke, UiBuilder, pos2, vec2,
};

use super::{
    Viewer,
    bars::{self, Flyout},
};
use crate::{
    components::{IconButton, Kind, MenuItem},
    icons,
    theme::Palette,
};

/// How much of the picture's width the pill takes: 1072 of 1917, 534 of 960 and 820 of 1467
/// in Photos.
pub const PILL_SHARE: f32 = 0.557;
/// How tall it is, 56 px at 125%.
pub const PILL_HEIGHT: f32 = 45.0;
/// How far its bottom is from the bottom of the picture's room, about 32 px at 125%.
pub const PILL_GAP: f32 = 26.0;
/// How round its corners are.
pub const PILL_RADIUS: f32 = 6.0;
/// The least room either side of it, in a window too small for its share.
const PILL_SIDE_ROOM: f32 = 8.0;
/// The room inside it before the first button and after the last.
const PILL_PADDING: f32 = 8.0;
/// The room between its controls.
const PILL_SPACING: f32 = 6.0;
/// The extra room between each time and the button on its far side, Volume before the first and
/// Loop after the second, so the times sit with the seek line rather than the buttons.
pub const TIME_SIDE_GAP: f32 = 10.0;
/// The shortest the seek line gets before the rest goes into "…".
const LEAST_SEEK: f32 = 140.0;
/// How far it sinks as it goes.
pub const PILL_SINK: f32 = 10.0;
/// The times' size.
const TIME_SIZE: f32 = 12.5;

/// How wide a pill is in this much room.
pub fn pill_width(room: f32) -> f32 {
    (room * PILL_SHARE)
        .min(room - PILL_SIDE_ROOM * 2.0)
        .max(0.0)
}

/// Where the pill goes in the picture's room: centred across it, near its bottom.
pub fn pill_rect(room: Rect) -> Rect {
    Rect::from_center_size(
        pos2(
            room.center().x,
            room.bottom() - PILL_GAP - PILL_HEIGHT / 2.0,
        ),
        vec2(pill_width(room.width()), PILL_HEIGHT),
    )
}

/// A raised card with a soft shadow and an outline, under a pill or the trimmer's bubble.
pub fn raised_card(ui: &egui::Ui, palette: &Palette, rect: Rect, radius: f32) {
    let shadow = ui.ctx().global_style().visuals.popup_shadow;
    ui.painter().add(shadow.as_shape(rect, radius));
    ui.painter().rect_filled(rect, radius, palette.raised);
    ui.painter().rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, palette.border),
        egui::StrokeKind::Inside,
    );
}

/// A card floating over the picture that lets it show through. It has no shadow, which would
/// show through it as a smudge.
pub fn floating_card(ui: &egui::Ui, palette: &Palette, rect: Rect, radius: f32) {
    ui.painter().rect_filled(
        rect,
        radius,
        palette.raised.gamma_multiply(FLOATING_OPACITY),
    );
    ui.painter().rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, palette.border.gamma_multiply(FLOATING_OPACITY)),
        egui::StrokeKind::Inside,
    );
}

/// How solid a card floating over the picture is.
pub const FLOATING_OPACITY: f32 = 0.7;

/// A time in a box `width` wide, against its left or right side, so the line beside it never
/// moves as it changes.
pub fn time_in_box(
    ui: &mut egui::Ui,
    palette: &Palette,
    time: &str,
    (width, height): (f32, f32),
    align: Align,
) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let (anchor, at) = if align == Align::Max {
        (Align2::RIGHT_CENTER, rect.right_center())
    } else {
        (Align2::LEFT_CENTER, rect.left_center())
    };
    ui.painter().text(
        at,
        anchor,
        time,
        FontId::proportional(TIME_SIZE),
        palette.text,
    );
}

/// How wide a time is drawn in a pill.
pub fn time_width(ui: &egui::Ui, time: &str) -> f32 {
    let galley = ui.painter().layout_no_wrap(
        time.to_owned(),
        FontId::proportional(TIME_SIZE),
        egui::Color32::WHITE,
    );
    galley.size().x.ceil()
}

/// Whether everything fits in a pill this wide, rather than some of it going into "…".
pub fn fits_everything(inner_width: f32, time_width: f32, picture: bool) -> bool {
    let buttons = if picture { 5.0 } else { 4.0 };
    let fixed = IconButton::default_size() * buttons
        + bars::SKIP_SIZE * 2.0
        + time_width * 2.0
        + TIME_SIDE_GAP * 2.0
        + PILL_SPACING * (buttons + 4.0);
    inner_width - fixed >= LEAST_SEEK
}

impl Viewer {
    /// The pill itself. It keeps clicks, drags and the wheel off the picture under it.
    pub(super) fn play_pill(&mut self, ui: &mut egui::Ui, palette: &Palette, rect: Rect) {
        let _ = ui.interact(
            rect,
            egui::Id::unique("viewer_pill"),
            Sense::click_and_drag(),
        );
        raised_card(ui, palette, rect, PILL_RADIUS);
        let has_file = self.file.is_some();
        let picture = self.video_size().is_some();
        let (position, duration) = (self.position(), self.duration());
        let times = [self.clock(position), self.clock(duration)];
        let time_box = time_width(ui, &times[0]).max(time_width(ui, &times[1]));
        let inner = rect.shrink2(vec2(PILL_PADDING, 0.0));
        let wide = fits_everything(inner.width(), time_box, picture);
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(inner)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = PILL_SPACING;
                let (icon, name) = if self.is_playing() {
                    (icons::PAUSE, "Pause")
                } else {
                    (icons::PLAY, "Play")
                };
                if ui
                    .add_enabled(
                        has_file,
                        IconButton::new(palette, Kind::Ghost, icon, name).solid(),
                    )
                    .on_hover_text(name)
                    .clicked()
                {
                    self.toggle_playback();
                }
                if wide {
                    self.skip_button(ui, palette, false);
                    self.skip_button(ui, palette, true);
                }
                self.volume_button(ui, palette);
                if wide {
                    ui.add_space(TIME_SIDE_GAP);
                    time_in_box(ui, palette, &times[0], (time_box, PILL_HEIGHT), Align::Min);
                }
                let after = if wide {
                    let buttons = if picture { 2.0 } else { 1.0 };
                    time_box
                        + TIME_SIDE_GAP
                        + IconButton::default_size() * buttons
                        + PILL_SPACING * (buttons + 1.0)
                } else {
                    IconButton::default_size() + PILL_SPACING * 2.0
                };
                let room = (ui.available_width() - after).max(40.0);
                if let Some(time) = bars::seek_line(
                    ui,
                    palette,
                    room,
                    position,
                    duration,
                    &mut self.seek_bar_held,
                ) {
                    self.seek(time);
                }
                if wide {
                    time_in_box(ui, palette, &times[1], (time_box, PILL_HEIGHT), Align::Max);
                    ui.add_space(TIME_SIDE_GAP);
                    self.loop_button(ui, palette);
                    if picture {
                        self.save_picture_button(ui, palette);
                    }
                } else {
                    self.pill_more(ui, palette, picture);
                }
            },
        );
    }

    fn loop_button(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let looping = self.looping;
        let kind = if looping { Kind::Chosen } else { Kind::Ghost };
        if ui
            .add_enabled(
                self.file.is_some(),
                IconButton::new(palette, kind, icons::REPEAT, "Loop"),
            )
            .on_hover_text(if looping { "Loop is on" } else { "Loop" })
            .clicked()
        {
            self.looping = !looping;
        }
    }

    fn save_picture_button(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        if ui
            .add_enabled(
                self.saving_picture.is_none(),
                IconButton::new(palette, Kind::Ghost, icons::CAMERA, "Save frame"),
            )
            .on_hover_text("Save frame")
            .clicked()
        {
            self.save_picture(ui.ctx());
        }
    }

    /// The narrow pill's "…": the jumps, loop and save frame.
    fn pill_more(&mut self, ui: &mut egui::Ui, palette: &Palette, picture: bool) {
        let kind = bars::flyout_kind(self.flyout.open, Flyout::More);
        let response = ui
            .add_enabled(
                self.file.is_some(),
                IconButton::new(palette, kind, icons::ELLIPSIS, "More options"),
            )
            .on_hover_text("More options");
        let (back_hint, forward_hint) = (
            format!("Back {}s", self.jump(false)),
            format!("Forward {}s", self.jump(true)),
        );
        let looping = self.looping;
        let saving = self.saving_picture.is_some();
        let chosen = bars::flyout(&response, &mut self.flyout, Flyout::More, true, |ui| {
            let back = ui
                .add(MenuItem::new(palette, icons::UNDO, "Jump back", "Left").tick_room())
                .on_hover_text(back_hint)
                .clicked();
            let forward = ui
                .add(MenuItem::new(palette, icons::REDO, "Jump forward", "Right").tick_room())
                .on_hover_text(forward_hint)
                .clicked();
            let looping = ui
                .add(MenuItem::ticked(palette, icons::REPEAT, "Loop", looping))
                .clicked();
            let picture = picture
                && ui
                    .add_enabled(
                        !saving,
                        MenuItem::new(palette, icons::CAMERA, "Save frame", "").tick_room(),
                    )
                    .clicked();
            PillMore {
                back,
                forward,
                looping,
                picture,
            }
        });
        let pressed = chosen.unwrap_or_default();
        if pressed.back {
            self.skip(false);
        }
        if pressed.forward {
            self.skip(true);
        }
        if pressed.looping {
            self.looping = !looping;
        }
        if pressed.picture {
            self.save_picture(ui.ctx());
        }
    }
}

/// What was chosen in the narrow pill's "…".
#[derive(Clone, Copy, Debug, Default)]
struct PillMore {
    back: bool,
    forward: bool,
    looping: bool,
    picture: bool,
}
