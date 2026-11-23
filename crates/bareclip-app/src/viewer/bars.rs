//! The player's two bars, laid out as the Photos app's. Along the top: Info, Trim, Edit in
//! BareClip and Delete, the file's name in the middle, and Settings and the window's buttons.
//! Along the bottom, a status bar: the file's details, and zoom, speed, mini view and full
//! screen. The playing controls are on the pill over the picture (`pill.rs`).
//!
//! In a window they stay, and the picture sits between them. In full screen each slides in
//! over the picture only while the mouse is over its strip of the screen.

use std::time::{Duration, Instant};

use bareclip_core::time::{Ticks, clock_needs_hours, format_clock};
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Layout, Rect, RectAlign, RichText, Sense, Stroke,
    UiBuilder, emath::TSTransform, pos2, vec2,
};

use super::{NORMAL_SPEED, SPEEDS, VOLUME_STEP, Viewer, file_details, wheel_notches, zoom};
use crate::{
    components::{self, IconButton, Kind, MenuItem},
    fonts, icons, text,
    theme::{self, Palette},
};

pub const TITLE_HEIGHT: f32 = 48.0;
/// The status bar along the bottom: one row of buttons with a little room above and below.
pub const BOTTOM_HEIGHT: f32 = 40.0;
const WINDOW_BUTTON_SIZE: f32 = 40.0;
const CONTROL_SPACING: f32 = 4.0;
/// What goes between the file's details in the bottom bar.
const DETAIL_GAP: &str = "  \u{b7}  ";
/// How much of the bars' colour covers the picture under them while they float over it.
const BAR_OPACITY: f32 = 0.86;
const VOLUME_SLIDER_WIDTH: f32 = 160.0;
/// The zoom's percentage between − and +, kept one width so they don't move.
const ZOOM_LABEL_WIDTH: f32 = 50.0;
/// The small − and + beside the zoom.
const SMALL_BUTTON: f32 = 26.0;
/// The room before the zoom's reset button, setting it apart from − and +.
const ZOOM_RESET_GAP: f32 = 10.0;
/// The zoom flyout's row of −, the percentage, + and reset, which its slider matches.
const ZOOM_ROW_WIDTH: f32 =
    SMALL_BUTTON * 3.0 + ZOOM_LABEL_WIDTH + CONTROL_SPACING * 3.0 + ZOOM_RESET_GAP;
/// − and + take the zoom to the next multiple of this many percent.
pub const ZOOM_BUTTON_STEP: f32 = 10.0;
/// The jump buttons beside play: an arrow over the number of seconds.
pub const SKIP_SIZE: f32 = 36.0;
const SKIP_ARROW_SIZE: f32 = 20.0;
const SKIP_ARROW_RAISE: f32 = 4.0;
const SKIP_NUMBER_SIZE: f32 = 10.5;
const SKIP_NUMBER_DROP: f32 = 8.0;
/// The room between a flyout and its button, and how far it rises into place as it opens.
const FLYOUT_GAP: f32 = 6.0;
/// How small a flyout starts as it grows out of its button.
const FLYOUT_START_SCALE: f32 = 0.4;
/// How long the mouse rests on a button before its flyout opens.
const HOVER_WAIT: Duration = Duration::from_millis(300);
/// How long the mouse can be away from a flyout and its button before it closes.
const FLYOUT_AWAY: Duration = Duration::from_secs(1);
/// The speed menu, wide enough for its tick and the longest speed, 0.25x.
const SPEED_MENU_WIDTH: f32 = 110.0;
/// The seek line: how much room it takes, its track at rest and under the mouse, and its thumb.
const SEEK_LINE_HEIGHT: f32 = 22.0;
const TRACK: f32 = 4.0;
const TRACK_HOVER: f32 = 6.0;
const THUMB: f32 = 6.0;
const THUMB_HOVER: f32 = 8.0;
/// How long the seek line takes to swell and settle.
const SWELL_TIME: f32 = 0.12;

/// The bars' fill: solid, or a little see-through while they float over the picture.
fn bar_fill(palette: &Palette, floating: bool) -> Color32 {
    if floating {
        palette.bg_app.gamma_multiply(BAR_OPACITY)
    } else {
        palette.bg_app
    }
}

/// Under a bar: its fill, a line along its edge with the picture while it doesn't float, and
/// something that keeps clicks, drags and the wheel off the picture under it. It has to sense
/// drags too, or the picture, which does, still counts as under the mouse.
pub(super) fn bar_backdrop(
    ui: &mut egui::Ui,
    palette: &Palette,
    rect: Rect,
    floating: bool,
    name: &str,
) {
    let _ = ui.interact(rect, egui::Id::unique(name), Sense::click_and_drag());
    ui.painter()
        .rect_filled(rect, 0.0, bar_fill(palette, floating));
    if !floating {
        ui.painter()
            .hline(rect.x_range(), rect.top(), Stroke::new(1.0, palette.border));
    }
}

/// The volume's symbol, which shows roughly how loud it is.
pub fn volume_icon(level: u32) -> icons::Icon {
    match level {
        0 => icons::VOLUME_X,
        1..=33 => icons::VOLUME,
        34..=66 => icons::VOLUME_1,
        _ => icons::VOLUME_2,
    }
}

impl Viewer {
    /// Maximises or restores the window. In full screen it leaves full screen instead, because a
    /// window maximised under full screen would stop short of the taskbar.
    fn toggle_maximized(&mut self, ctx: &egui::Context, maximized: bool) {
        if self.full_screen {
            self.set_full_screen(ctx, false);
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
        }
    }

    /// Close, maximise or restore, and minimise, laid out from the right.
    pub(super) fn window_buttons(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let ctx = ui.ctx().clone();
        let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
        ui.spacing_mut().item_spacing.x = CONTROL_SPACING;
        let button = |icon, name| {
            IconButton::new(palette, Kind::Ghost, icon, name).size(WINDOW_BUTTON_SIZE, 16.0)
        };
        if ui
            .add(button(icons::X, "Close window"))
            .on_hover_text("Close")
            .clicked()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let (icon, name, hint) = if maximized {
            (icons::COPY, "Restore window", "Restore")
        } else {
            (icons::SQUARE, "Maximise window", "Maximise")
        };
        if ui.add(button(icon, name)).on_hover_text(hint).clicked() {
            self.toggle_maximized(&ctx, maximized);
        }
        if ui
            .add(button(icons::MINUS, "Minimise window"))
            .on_hover_text("Minimise")
            .clicked()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
    }

    /// The strip across the top, which replaces the Windows title bar and does its jobs.
    pub(super) fn title_strip(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        rect: Rect,
        floating: bool,
    ) {
        let ctx = ui.ctx().clone();
        ui.painter()
            .rect_filled(rect, 0.0, bar_fill(palette, floating));
        let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
        let strip = ui.interact(
            rect,
            egui::Id::unique("viewer_title_drag"),
            Sense::click_and_drag(),
        );
        // In full screen the strip neither moves the window nor maximises it, as in other
        // programs' full screen.
        if self.full_screen {
        } else if strip.double_clicked() {
            self.toggle_maximized(&ctx, maximized);
        } else if strip.drag_started_by(egui::PointerButton::Primary) {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if self.trim.is_some() {
            self.trim_title(ui, palette, rect);
            return;
        }
        let has_file = self.file.is_some();
        let inner = rect.shrink2(vec2(8.0, 0.0));
        let left = ui
            .scope_builder(
                UiBuilder::new()
                    .max_rect(inner)
                    .layout(Layout::left_to_right(Align::Center)),
                |ui| {
                    ui.spacing_mut().item_spacing.x = CONTROL_SPACING;
                    let info_open = self.is_info_open();
                    let kind = if info_open { Kind::Chosen } else { Kind::Ghost };
                    if ui
                        .add_enabled(
                            has_file || info_open,
                            IconButton::new(palette, kind, icons::INFO, "Info"),
                        )
                        .on_hover_text("Info")
                        .clicked()
                    {
                        self.toggle_info();
                    }
                    let trimming = self.trim.is_some();
                    let kind = if trimming { Kind::Chosen } else { Kind::Ghost };
                    if ui
                        .add_enabled(
                            has_file,
                            IconButton::new(palette, kind, icons::SCISSORS, "Trim"),
                        )
                        .on_hover_text("Trim")
                        .clicked()
                    {
                        if trimming {
                            self.close_trimmer();
                        } else {
                            let ctx = ui.ctx().clone();
                            self.open_trimmer(&ctx);
                        }
                    }
                    if ui
                        .add_enabled(
                            has_file,
                            IconButton::mark(palette, Kind::Ghost, "Edit in BareClip"),
                        )
                        .on_hover_text("Edit in BareClip")
                        .clicked()
                    {
                        self.edit_in_bareclip();
                    }
                    if ui
                        .add_enabled(
                            has_file,
                            IconButton::new(palette, Kind::Ghost, icons::TRASH_2, "Delete"),
                        )
                        .on_hover_text("Delete")
                        .clicked()
                    {
                        self.ask_to_delete();
                    }
                },
            )
            .response
            .rect;
        let right = ui
            .scope_builder(
                UiBuilder::new()
                    .max_rect(inner)
                    .layout(Layout::right_to_left(Align::Center)),
                |ui| {
                    ui.spacing_mut().item_spacing.x = CONTROL_SPACING;
                    self.window_buttons(ui, palette);
                    ui.add_space(8.0);
                    if ui
                        .add(IconButton::new(
                            palette,
                            Kind::Ghost,
                            icons::SETTINGS,
                            "Settings",
                        ))
                        .on_hover_text("Settings")
                        .clicked()
                    {
                        self.settings_open = true;
                    }
                },
            )
            .response
            .rect;
        let name_rect = name_room(rect, left, right);
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(name_rect)
                .layout(Layout::centered_and_justified(egui::Direction::LeftToRight)),
            |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(self.name())
                            .font(FontId::proportional(crate::components::NAME_BAR_TEXT - 1.0))
                            .color(palette.text),
                    )
                    .truncate()
                    .selectable(false),
                );
            },
        );
    }

    /// The status bar along the bottom: the file's details on the left, and zoom, speed, mini
    /// view and full screen on the right. The playing controls are on the pill instead.
    pub(super) fn bottom_bar(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        rect: Rect,
        floating: bool,
    ) {
        bar_backdrop(ui, palette, rect, floating, "viewer_bottom_bar");
        let has_file = self.file.is_some();
        let picture = self.video_size().is_some();

        // The file's details on the left, as one quiet line.
        let details = self.file.as_ref().map(file_details).unwrap_or_default();
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(rect.shrink2(vec2(16.0, 0.0)))
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                if !details.is_empty() {
                    ui.add(
                        egui::Label::new(
                            RichText::new(details.join(DETAIL_GAP))
                                .size(12.0)
                                .color(palette.text_muted),
                        )
                        .truncate()
                        .selectable(false),
                    );
                }
            },
        );

        // On the right: zoom, speed, mini view and full screen, laid out from the far edge in.
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(rect.shrink2(vec2(12.0, 0.0)))
                .layout(Layout::right_to_left(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = CONTROL_SPACING;
                // F11 works too, but the hint only names F.
                let (icon, name, hint) = if self.full_screen {
                    (icons::MINIMIZE, "Leave full screen", "Exit fullscreen (F)")
                } else {
                    (icons::MAXIMIZE, "Full screen", "Fullscreen (F)")
                };
                if ui
                    .add(IconButton::new(palette, Kind::Ghost, icon, name))
                    .on_hover_text(hint)
                    .clicked()
                {
                    let full = !self.full_screen;
                    self.set_full_screen(ui.ctx(), full);
                }
                if ui
                    .add_enabled(
                        has_file,
                        IconButton::new(
                            palette,
                            Kind::Ghost,
                            icons::PICTURE_IN_PICTURE_2,
                            "Mini view",
                        ),
                    )
                    .on_hover_text("Mini view")
                    .clicked()
                {
                    self.enter_mini_view(ui.ctx());
                }
                self.speed_button(ui, palette);
                if picture {
                    self.zoom_button(ui, palette);
                }
            },
        );
    }

    /// Back or forward as far as Settings says, drawn as an arrow over the number of seconds.
    pub(super) fn skip_button(&mut self, ui: &mut egui::Ui, palette: &Palette, forward: bool) {
        let step = self.jump(forward);
        let (icon, name, hint) = if forward {
            (icons::REDO, "Jump forward", format!("Forward {step}s"))
        } else {
            (icons::UNDO, "Jump back", format!("Back {step}s"))
        };
        let enabled = self.file.is_some();
        let button = IconButton::new(palette, Kind::Ghost, icon, name)
            .size(SKIP_SIZE, SKIP_ARROW_SIZE)
            .icon_raised(SKIP_ARROW_RAISE)
            .icon_squashed(components::SKIP_ARCH);
        let response = ui.add_enabled(enabled, button).on_hover_text(hint);
        ui.painter().text(
            response.rect.center() + vec2(0.0, SKIP_NUMBER_DROP),
            Align2::CENTER_CENTER,
            step.to_string(),
            fonts::strong(SKIP_NUMBER_SIZE),
            if enabled {
                palette.text
            } else {
                palette.text_muted
            },
        );
        if response.clicked() {
            self.skip(forward);
        }
    }

    /// The volume's button, whose symbol shows how loud it is. A click mutes and unmutes,
    /// resting the mouse on it opens a slider above it, and the wheel over it turns the volume
    /// up or down.
    pub(super) fn volume_button(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let shown = if self.is_muted() { 0 } else { self.volume() };
        let kind = flyout_kind(self.flyout.open, Flyout::Volume);
        let response = ui.add(IconButton::new(palette, kind, volume_icon(shown), "Volume"));
        if response.clicked() {
            self.toggle_mute();
            self.save_settings();
        }
        if response.hovered() {
            let notches = ui.input(|input| wheel_notches(&input.events));
            if notches != 0.0 {
                let wanted =
                    (self.volume() as f32 + notches * VOLUME_STEP as f32).clamp(0.0, 100.0);
                self.set_volume(wanted.round() as u32);
                self.save_settings();
                self.flyout.show_for_a_moment(Flyout::Volume);
            }
        }
        let mut held = self.volume_held;
        let chosen = flyout(&response, &mut self.flyout, Flyout::Volume, false, |ui| {
            let mut toggle_mute = false;
            let mut level = None;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let name = if shown == 0 { "Unmute" } else { "Mute" };
                toggle_mute = ui
                    .add(IconButton::new(
                        palette,
                        Kind::Ghost,
                        volume_icon(shown),
                        name,
                    ))
                    .on_hover_text(format!("{name} (M)"))
                    .clicked();
                level = level_bar(
                    ui,
                    palette,
                    VOLUME_SLIDER_WIDTH,
                    shown,
                    &mut held,
                    "Volume level",
                );
                number(ui, palette, &shown.to_string(), 28.0);
            });
            (toggle_mute, level)
        });
        let (toggle_mute, level) = chosen.unwrap_or_default();
        if toggle_mute {
            self.toggle_mute();
            self.save_settings();
        }
        if let Some(volume) = level {
            self.set_volume(volume);
        }
        if self.volume_held && !held {
            self.save_settings();
        }
        self.volume_held = held;
    }

    /// The speed's button, a gauge the same size as the buttons beside it, and its menu, which
    /// opens while the mouse rests on it. A click goes back to normal speed. It shows as chosen
    /// while the speed isn't normal.
    fn speed_button(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let speed = self.speed;
        let open = self.flyout.open == Some(Flyout::Speed);
        let kind = if open || speed != NORMAL_SPEED {
            Kind::Chosen
        } else {
            Kind::Ghost
        };
        let response = ui.add_enabled(
            self.file.is_some(),
            IconButton::new(palette, kind, icons::GAUGE, "Watching speed"),
        );
        if response.clicked() {
            self.set_speed(NORMAL_SPEED);
        }
        let chosen = flyout(&response, &mut self.flyout, Flyout::Speed, true, |ui| {
            let mut chosen = None;
            for (index, choice) in SPEEDS.iter().enumerate().rev() {
                let name = text::speed_name(*choice);
                if ui
                    .add(MenuItem::choice(palette, &name, index == speed).short(SPEED_MENU_WIDTH))
                    .clicked()
                {
                    chosen = Some(index);
                }
            }
            chosen
        });
        if let Some(index) = chosen.flatten() {
            self.set_speed(index);
            self.flyout.close();
        }
    }

    /// The zoom's button, the same size as the others. A click goes back to fit, and resting
    /// the mouse on it opens a flyout with a slider with fit in the middle, over −, the
    /// percentage, + and a button back to fit.
    fn zoom_button(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let ctx = ui.ctx().clone();
        let percent = self.zoom_percent(&ctx);
        let label = format!("{percent:.0}%");
        let kind = flyout_kind(self.flyout.open, Flyout::Zoom);
        let response = ui.add(IconButton::new(palette, kind, icons::ZOOM_IN, "Zoom"));
        if response.clicked() {
            self.fit();
        }
        let (fit_percent, most) = self.video_size().map_or((100.0, 800.0), |video| {
            let fit = zoom::fit_percent(self.view, video, ctx.pixels_per_point());
            (fit, (zoom::MOST_ZOOM * 100.0).max(fit))
        });
        let share = zoom::slider_share(percent, fit_percent, most);
        let chosen = flyout(&response, &mut self.flyout, Flyout::Zoom, false, |ui| {
            let mut held = false;
            let level = level_bar(
                ui,
                palette,
                ZOOM_ROW_WIDTH,
                (share * 100.0).round() as u32,
                &mut held,
                "Zoom level",
            );
            ui.add_space(2.0);
            let mut pressed = ZoomPress::default();
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = CONTROL_SPACING;
                let small = |icon, name| {
                    IconButton::new(palette, Kind::Ghost, icon, name).size(SMALL_BUTTON, 14.0)
                };
                pressed.out = ui
                    .add(small(icons::MINUS, "Zoom out"))
                    .on_hover_text("Zoom out (Ctrl+-)")
                    .clicked();
                centred_number(ui, palette, &label, ZOOM_LABEL_WIDTH);
                pressed.into = ui
                    .add(small(icons::PLUS, "Zoom in"))
                    .on_hover_text("Zoom in (Ctrl+=)")
                    .clicked();
                ui.add_space(ZOOM_RESET_GAP);
                pressed.reset = ui
                    .add(small(icons::ROTATE_CCW, "Reset zoom"))
                    .on_hover_text("Back to fit (0)")
                    .clicked();
            });
            (level, pressed)
        });
        let (level, pressed) = chosen.unwrap_or_default();
        if let Some(level) = level {
            let wanted = zoom::percent_at_share(level as f32 / 100.0, fit_percent, most);
            self.zoom_to(&ctx, wanted);
        }
        if pressed.out {
            self.zoom_step(&ctx, false);
        }
        if pressed.into {
            self.zoom_step(&ctx, true);
        }
        if pressed.reset {
            self.fit();
        }
    }
}

/// What was pressed in the zoom's flyout.
#[derive(Clone, Copy, Debug, Default)]
struct ZoomPress {
    out: bool,
    into: bool,
    reset: bool,
}

/// The flyouts that open above the pill's and bottom bar's buttons. One at a time is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flyout {
    Volume,
    Zoom,
    Speed,
    /// A pill's "…": the trimmer's tools, or what a narrow pill puts away.
    More,
}

impl Flyout {
    /// Whether it opens while the mouse rests on its button and closes once the mouse has been
    /// away a moment, leaving a click on the button to do something else. "…" opens on a click.
    pub fn opens_on_hover(self) -> bool {
        self != Self::More
    }
}

/// The button the mouse rests on, since when, and whether its flyout has opened for this rest,
/// so one closed by Escape or a choice stays closed until the mouse leaves and comes back.
#[derive(Clone, Copy, Debug)]
struct Rest {
    which: Flyout,
    since: Instant,
    opened: bool,
}

/// Which flyout is open, and when one that opens on hover opens or closes.
#[derive(Debug, Default)]
pub struct Flyouts {
    pub open: Option<Flyout>,
    /// Where the open flyout and its button are, so the mouse on either keeps it.
    rect: Option<Rect>,
    button: Option<Rect>,
    resting: Option<Rest>,
    /// Since when the mouse has been off a flyout that opened on hover and off its button.
    away_since: Option<Instant>,
}

impl Flyouts {
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn close(&mut self) {
        self.open = None;
        self.rect = None;
        self.button = None;
        self.away_since = None;
    }

    /// Opens one now, as the wheel does. It goes once the mouse has been away from it a moment.
    pub fn show_for_a_moment(&mut self, which: Flyout) {
        if self.open != Some(which) {
            self.open = Some(which);
            self.rect = None;
        }
        self.away_since = None;
    }

    /// Makes the mouse seem to have rested on its button long enough to open its flyout.
    #[cfg(test)]
    pub fn rest_is_up(&mut self) {
        if let Some(rest) = &mut self.resting {
            rest.since -= HOVER_WAIT;
        }
    }

    /// Makes the mouse seem to have been away from the open flyout long enough to close it.
    #[cfg(test)]
    pub fn away_is_up(&mut self) {
        if let Some(since) = &mut self.away_since {
            *since -= FLYOUT_AWAY;
        }
    }

    /// Whether the open flyout is counting down to close, the mouse being away from it.
    #[cfg(test)]
    pub fn is_closing(&self) -> bool {
        self.away_since.is_some()
    }

    fn toggle(&mut self, which: Flyout) {
        if self.open == Some(which) {
            self.close();
        } else {
            self.open = Some(which);
            self.rect = None;
            self.away_since = None;
        }
    }

    /// Opens a flyout once the mouse has rested on its button for a moment, so passing over it
    /// on the way somewhere else opens nothing.
    fn rest_on(&mut self, button: &egui::Response, which: Flyout) {
        let over = button.hovered() && button.enabled();
        if self.open == Some(which) {
            self.button = Some(button.rect);
        }
        let now = Instant::now();
        match &mut self.resting {
            Some(rest) if rest.which == which => {
                if !over {
                    self.resting = None;
                } else if !rest.opened {
                    let ready = rest.since + HOVER_WAIT;
                    if now >= ready {
                        rest.opened = true;
                        self.show_for_a_moment(which);
                    } else {
                        button.ctx.request_repaint_after(ready - now);
                    }
                }
            }
            _ if over => {
                self.resting = Some(Rest {
                    which,
                    since: now,
                    opened: false,
                });
                button.ctx.request_repaint_after(HOVER_WAIT);
            }
            _ => {}
        }
    }

    /// Closes a flyout that opens on hover once the mouse has been off it and its button for a
    /// moment. Not while a mouse button is held, as while dragging its slider past its edge.
    pub fn tick(&mut self, ctx: &egui::Context) {
        let Some(which) = self.open else {
            self.away_since = None;
            return;
        };
        if !which.opens_on_hover() {
            return;
        }
        let (pointer, held) =
            ctx.input(|input| (input.pointer.hover_pos(), input.pointer.any_down()));
        let near = pointer.is_some_and(|pointer| {
            [self.rect, self.button]
                .into_iter()
                .flatten()
                .any(|rect| rect.contains(pointer))
        });
        if near || held {
            self.away_since = None;
            return;
        }
        let now = Instant::now();
        let at = *self.away_since.get_or_insert(now) + FLYOUT_AWAY;
        if now >= at {
            self.close();
        } else {
            ctx.request_repaint_after(at - now);
        }
    }
}

/// A flyout's button looks chosen while its flyout is open, as if held in.
pub(super) fn flyout_kind(open: Option<Flyout>, which: Flyout) -> Kind {
    if open == Some(which) {
        Kind::Chosen
    } else {
        Kind::Ghost
    }
}

/// A flyout above its button, with its middle over its button's middle. Volume, zoom and speed
/// open while the mouse rests on their button and close once it has been away a moment, so a
/// click on their button is left for something else. "…" opens and closes on a click on its
/// button. Any of them closes on a click elsewhere or Escape. It grows out of the button and
/// fades in as it opens, and shrinks back into it as it closes, when it can no longer be used.
/// Gives what `add` gave while it could be used.
pub(super) fn flyout<R>(
    button: &egui::Response,
    flyouts: &mut Flyouts,
    which: Flyout,
    close_on_click_inside: bool,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<R> {
    let ctx = button.ctx.clone();
    if which.opens_on_hover() {
        flyouts.rest_on(button, which);
    } else if button.clicked() {
        flyouts.toggle(which);
    }
    let is_open = flyouts.open == Some(which);
    let shown = theme::appear(&ctx, button.id.with("flyout"), is_open);
    let id = egui::Popup::default_response_id(button);
    let layer = egui::LayerId::new(egui::Order::Foreground, id);
    // Scaled about the middle of the button's top edge, so it seems to come out of it.
    let pivot = pos2(button.rect.center().x, button.rect.top() - FLYOUT_GAP);
    let scale = egui::lerp(FLYOUT_START_SCALE..=1.0, shown);
    ctx.set_transform_layer(
        layer,
        TSTransform::new(pivot.to_vec2() * (1.0 - scale), scale),
    );
    if shown <= 0.0 {
        return None;
    }
    let mut frame = egui::Frame::popup(&ctx.global_style());
    frame.fill = frame.fill.gamma_multiply(shown);
    frame.stroke.color = frame.stroke.color.gamma_multiply(shown);
    frame.shadow.color = frame.shadow.color.gamma_multiply(shown);
    let close = if close_on_click_inside {
        egui::PopupCloseBehavior::CloseOnClick
    } else {
        egui::PopupCloseBehavior::CloseOnClickOutside
    };
    let shown_popup = egui::Popup::from_response(button)
        .kind(egui::PopupKind::Menu)
        .layout(Layout::top_down_justified(Align::Min))
        .align(RectAlign::TOP)
        .align_alternatives(&[RectAlign::TOP])
        .gap(FLYOUT_GAP)
        .close_behavior(close)
        .interactable(is_open)
        .frame(frame)
        .show(|ui| {
            ui.multiply_opacity(shown);
            add(ui)
        })?;
    if is_open {
        flyouts.rect = Some(shown_popup.response.rect);
    }
    // A click on the button isn't inside, but it opened it or did the button's own job.
    if is_open && shown_popup.response.should_close() && !button.clicked() {
        flyouts.close();
    }
    is_open.then_some(shown_popup.inner)
}

/// Where the file's name goes: centred on the window, as wide as the buttons either side allow,
/// or in the room between them if that is too narrow to read.
pub fn name_room(rect: Rect, left: Rect, right: Rect) -> Rect {
    const MARGIN: f32 = 16.0;
    let middle = rect.center().x;
    let half = (middle - left.right()).min(right.left() - middle) - MARGIN;
    if half * 2.0 >= 160.0 {
        return Rect::from_center_size(rect.center(), vec2(half * 2.0, rect.height()));
    }
    Rect::from_min_max(
        pos2(left.right() + MARGIN, rect.top()),
        pos2(
            (right.left() - MARGIN).max(left.right() + MARGIN),
            rect.bottom(),
        ),
    )
}

/// The player's seek line: a rounded track that swells under the mouse, gold up to the playhead,
/// a light thumb ringed in gold, and the time under the pointer in a bubble above it.
pub fn seek_line(
    ui: &mut egui::Ui,
    palette: &Palette,
    width: f32,
    position: Ticks,
    total: Ticks,
    held: &mut bool,
) -> Option<Ticks> {
    let (rect, response) =
        ui.allocate_exact_size(vec2(width, SEEK_LINE_HEIGHT), Sense::click_and_drag());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, true, "Seek"));
    let active = response.hovered() || response.dragged();
    let swell = ui
        .ctx()
        .animate_bool_with_time(response.id.with("swell"), active, SWELL_TIME);
    let track_height = egui::lerp(TRACK..=TRACK_HOVER, swell);
    let thumb = egui::lerp(THUMB..=THUMB_HOVER, swell);
    let left = rect.left() + THUMB_HOVER;
    let room = (rect.width() - THUMB_HOVER * 2.0).max(1.0);
    let track = Rect::from_min_size(
        pos2(left, rect.center().y - track_height / 2.0),
        vec2(room, track_height),
    );
    let radius = track_height / 2.0;
    let painter = ui.painter();
    painter.rect_filled(track, radius, palette.border_strong);
    let x_of =
        |time: Ticks| left + room * (time.0 as f64 / total.0.max(1) as f64).clamp(0.0, 1.0) as f32;
    let time_at = |x: f32| {
        Ticks((((x - left) / room).clamp(0.0, 1.0) as f64 * total.0 as f64).round() as i64)
    };
    if total.0 > 0 {
        let x = x_of(position.min(total));
        painter.rect_filled(
            track.with_max_x(x.max(track.left() + track_height)),
            radius,
            palette.accent,
        );
        let middle = pos2(x, rect.center().y);
        painter.circle_filled(middle, thumb, theme::DARK.text);
        painter.circle_stroke(middle, thumb, Stroke::new(2.0, palette.accent));
        if active && let Some(pointer) = response.hover_pos().or(response.interact_pointer_pos()) {
            let shown = if response.dragged() {
                position
            } else {
                time_at(pointer.x)
            };
            time_bubble(
                ui.ctx(),
                palette,
                pos2(pointer.x, rect.top() - 4.0),
                &format_clock(shown, clock_needs_hours(total)),
            );
        }
    }
    *held = response.dragged();
    // Only when it moves, because seeking again would stop the exact frame from ever being
    // reached while the pointer rests.
    let moved = response.dragged() && response.drag_delta() != vec2(0.0, 0.0);
    if total.0 <= 0 || !(moved || response.clicked()) {
        return None;
    }
    let pointer = response.interact_pointer_pos()?;
    Some(time_at(pointer.x))
}

/// A small card with a time in it, whose bottom middle sits at `anchor`, kept on screen.
fn time_bubble(ctx: &egui::Context, palette: &Palette, anchor: egui::Pos2, time: &str) {
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::unique("viewer_seek_time"),
    ));
    let galley = painter.layout_no_wrap(time.to_owned(), fonts::strong(12.0), palette.text);
    let size = galley.size() + vec2(16.0, 8.0);
    let screen = ctx.content_rect();
    let x = anchor.x.clamp(
        screen.left() + size.x / 2.0 + 4.0,
        screen.right() - size.x / 2.0 - 4.0,
    );
    let rect = Rect::from_min_size(pos2(x - size.x / 2.0, anchor.y - size.y), size);
    let shadow = ctx.global_style().visuals.popup_shadow;
    painter.add(shadow.as_shape(rect, 6.0));
    painter.rect_filled(rect, 6.0, palette.raised);
    painter.rect_stroke(
        rect,
        6.0,
        Stroke::new(1.0, palette.border),
        egui::StrokeKind::Inside,
    );
    painter.galley(rect.center() - galley.size() / 2.0, galley, palette.text);
}

/// A number in the middle of a fixed width, such as the zoom between − and +.
fn centred_number(ui: &mut egui::Ui, palette: &Palette, value: &str, width: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, SMALL_BUTTON), Sense::hover());
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        value,
        FontId::proportional(12.5),
        palette.text,
    );
}

/// A number beside a slider, in a fixed width so the slider doesn't move as it changes.
fn number(ui: &mut egui::Ui, palette: &Palette, value: &str, width: f32) {
    ui.allocate_ui_with_layout(
        vec2(width, 24.0),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.add(
                egui::Label::new(RichText::new(value).size(12.5).color(palette.text))
                    .selectable(false),
            );
        },
    );
}

/// A bar for a level out of 100, such as the volume, which a click or drag sets.
pub fn level_bar(
    ui: &mut egui::Ui,
    palette: &Palette,
    width: f32,
    level: u32,
    held: &mut bool,
    name: &str,
) -> Option<u32> {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 24.0), Sense::click_and_drag());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, true, name));
    let active = response.hovered() || response.dragged();
    let knob = if active { THUMB_HOVER - 1.0 } else { THUMB };
    let left = rect.left() + THUMB_HOVER;
    let room = (rect.width() - THUMB_HOVER * 2.0).max(1.0);
    let line = Rect::from_min_size(pos2(left, rect.center().y - TRACK / 2.0), vec2(room, TRACK));
    ui.painter()
        .rect_filled(line, TRACK / 2.0, palette.border_strong);
    let x = left + room * level.min(100) as f32 / 100.0;
    ui.painter().rect_filled(
        line.with_max_x(x.max(line.left() + TRACK)),
        TRACK / 2.0,
        palette.accent,
    );
    let middle = pos2(x, rect.center().y);
    ui.painter().circle_filled(middle, knob, theme::DARK.text);
    ui.painter()
        .circle_stroke(middle, knob, Stroke::new(2.0, palette.accent));
    *held = response.dragged();
    if !(response.dragged() || response.clicked()) {
        return None;
    }
    let pointer = response.interact_pointer_pos()?;
    let share = ((pointer.x - left) / room).clamp(0.0, 1.0);
    Some((share * 100.0).round() as u32)
}
