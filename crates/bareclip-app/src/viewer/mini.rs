//! Mini view: the player's window made small and kept above other windows, showing only the
//! picture, with its controls when the mouse is over it.

use eframe::egui::{self, Align, Layout, Rect, Sense, UiBuilder, pos2, vec2};

use super::Viewer;
use crate::{
    components::{IconButton, Kind},
    icons,
    storage::WindowPlace,
    theme::Palette,
    window_place,
};

/// Mini view's size, in points.
pub const MINI_SIZE: egui::Vec2 = vec2(400.0, 225.0);
/// How far from the screen's corner mini view first opens.
const CORNER_MARGIN: f32 = 24.0;
const CONTROLS_HEIGHT: f32 = 44.0;

/// What mini view needs to put the window back as it was.
pub struct Mini {
    /// Where the window was, as Windows keeps it, maximised or not.
    before: Option<WindowPlace>,
    /// The size it was in points, for when Windows can't be asked.
    before_size: egui::Vec2,
    before_position: Option<egui::Pos2>,
}

impl Viewer {
    #[cfg(test)]
    pub fn is_mini(&self) -> bool {
        self.mini.is_some()
    }

    /// Makes the window small and keeps it above every other window.
    pub fn enter_mini_view(&mut self, ctx: &egui::Context) {
        if self.mini.is_some() {
            return;
        }
        self.flyout.close();
        if self.full_screen {
            self.set_full_screen(ctx, false);
        }
        let (size, position, monitor) = ctx.input(|input| {
            let viewport = input.viewport();
            (
                viewport
                    .inner_rect
                    .map_or(input.content_rect().size(), |rect| rect.size()),
                viewport.outer_rect.map(|rect| rect.min),
                viewport.monitor_size,
            )
        });
        self.mini = Some(Mini {
            before: self.window.and_then(window_place::read),
            before_size: size,
            before_position: position,
        });
        self.fit();
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
            egui::WindowLevel::AlwaysOnTop,
        ));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(MINI_SIZE));
        if let (Some(window), Some(place)) = (self.window, self.settings.mini_window)
            && window_place::put_back(
                window,
                WindowPlace {
                    maximized: false,
                    ..place
                },
            )
        {
            return;
        }
        let corner = self
            .settings
            .mini_window
            .map(|place| pos2(place.left as f32, place.top as f32) / ctx.pixels_per_point())
            .or_else(|| {
                monitor.map(|monitor| {
                    (monitor - MINI_SIZE - vec2(CORNER_MARGIN, CORNER_MARGIN * 3.0)).to_pos2()
                })
            });
        if let Some(corner) = corner {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(corner));
        }
    }

    /// Puts the window back where it was before mini view, and remembers mini view's corner.
    pub fn leave_mini_view(&mut self, ctx: &egui::Context) {
        let Some(mini) = self.mini.take() else {
            return;
        };
        if let Some(place) = self.window.and_then(window_place::read) {
            self.settings.mini_window = Some(place);
            self.save_settings();
        } else if let Some(rect) = ctx.input(|input| input.viewport().outer_rect) {
            let scale = ctx.pixels_per_point();
            self.settings.mini_window = Some(WindowPlace {
                left: (rect.left() * scale) as i32,
                top: (rect.top() * scale) as i32,
                right: (rect.right() * scale) as i32,
                bottom: (rect.bottom() * scale) as i32,
                maximized: false,
            });
            self.save_settings();
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
            egui::WindowLevel::Normal,
        ));
        match (self.window, mini.before) {
            (Some(window), Some(place)) => {
                window_place::put_back(window, place);
                if place.maximized {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
                }
            }
            _ => {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(mini.before_size));
                if let Some(position) = mini.before_position {
                    ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(position));
                }
            }
        }
    }

    /// Play or pause, the seek line and the way back, over the picture while the mouse is on it.
    pub(super) fn mini_controls(&mut self, ui: &mut egui::Ui, palette: &Palette, area: Rect) {
        let hovered = ui.rect_contains_pointer(area);
        if !hovered && !self.seek_bar_held {
            return;
        }
        let strip =
            Rect::from_min_max(pos2(area.left(), area.bottom() - CONTROLS_HEIGHT), area.max);
        ui.painter()
            .rect_filled(strip, 0.0, palette.bg_app.gamma_multiply(0.85));
        let _ = ui.interact(strip, egui::Id::unique("mini_strip"), Sense::click());
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(strip.shrink2(vec2(6.0, 0.0)))
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (icon, name) = if self.is_playing() {
                    (icons::PAUSE, "Pause")
                } else {
                    (icons::PLAY, "Play")
                };
                if ui
                    .add(IconButton::new(palette, Kind::Ghost, icon, name))
                    .clicked()
                {
                    self.toggle_playback();
                }
                let room = (ui.available_width() - 40.0).max(40.0);
                let (position, duration) = (self.position(), self.duration());
                if let Some(time) = super::bars::seek_line(
                    ui,
                    palette,
                    room,
                    position,
                    duration,
                    &mut self.seek_bar_held,
                ) {
                    self.seek(time);
                }
                if ui
                    .add(IconButton::new(
                        palette,
                        Kind::Ghost,
                        icons::MAXIMIZE_2,
                        "Leave mini view",
                    ))
                    .on_hover_text("Back to the full window (Escape)")
                    .clicked()
                {
                    let ctx = ui.ctx().clone();
                    self.leave_mini_view(&ctx);
                }
            },
        );
    }
}
