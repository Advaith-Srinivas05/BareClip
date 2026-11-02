#![cfg_attr(not(test), windows_subsystem = "windows")]

mod fonts;
mod theme;

use eframe::egui;

const WINDOW_ICON: &[u8] = include_bytes!("../../../assets/brand/png/app-icon-64.png");

fn main() -> eframe::Result {
    let ffmpeg_license = match bareclip_media::init() {
        Ok(()) => bareclip_media::ffmpeg_info().license,
        Err(error) => format!("failed to start: {error}"),
    };

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("BareClip")
        .with_inner_size([1280.0, 720.0]);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(WINDOW_ICON) {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "BareClip",
        options,
        Box::new(|creation_context| {
            fonts::install(&creation_context.egui_ctx);
            theme::install(&creation_context.egui_ctx);
            let gpu = creation_context
                .wgpu_render_state
                .as_ref()
                .map(|state| state.adapter.get_info().name);
            Ok(Box::new(BareClipApp {
                gpu_label: gpu_label(gpu),
                ffmpeg_license,
            }))
        }),
    )
}

struct BareClipApp {
    gpu_label: String,
    ffmpeg_license: String,
}

impl BareClipApp {
    fn content(&self, ui: &mut egui::Ui) -> Vec<(egui::ThemePreference, egui::Rect)> {
        // The root Ui has no background, so the panel is what paints the theme.
        egui::CentralPanel::default()
            .show(ui, |ui| {
                ui.heading("BareClip");
                ui.label(format!("GPU: {}", self.gpu_label));
                ui.label(format!("FFmpeg: {}", self.ffmpeg_license));
                ui.add_space(8.0);
                theme::theme_picker(ui)
            })
            .inner
    }
}

impl eframe::App for BareClipApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.content(ui);
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }
}

fn gpu_label(name: Option<String>) -> String {
    name.unwrap_or_else(|| "unknown".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton, Pos2, Rect, Shape, Theme, ThemePreference, vec2};

    fn test_app() -> BareClipApp {
        BareClipApp {
            gpu_label: "Test GPU".to_owned(),
            ffmpeg_license: "LGPL".to_owned(),
        }
    }

    fn run_frame(
        ctx: &egui::Context,
        app: &BareClipApp,
        events: Vec<Event>,
    ) -> (egui::FullOutput, Vec<(ThemePreference, Rect)>) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut buttons = Vec::new();
        let mut output = ctx.run_ui(input, |ui| buttons = app.content(ui));
        // There is no GPU in these tests to upload font textures to.
        output.textures_delta.clear();
        (output, buttons)
    }

    fn themed_context() -> egui::Context {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        ctx
    }

    fn click(ctx: &egui::Context, app: &BareClipApp, target: Rect) {
        let pos = target.center();
        let button = |pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        run_frame(ctx, app, vec![Event::PointerMoved(pos)]);
        run_frame(ctx, app, vec![button(true)]);
        run_frame(ctx, app, vec![button(false)]);
        run_frame(ctx, app, vec![]);
    }

    fn button_for(buttons: &[(ThemePreference, Rect)], choice: ThemePreference) -> Rect {
        buttons
            .iter()
            .find(|(preference, _)| *preference == choice)
            .expect("the theme choice has a button")
            .1
    }

    fn panel_is_painted(output: &egui::FullOutput, color: egui::Color32) -> bool {
        output.shapes.iter().any(|clipped| match &clipped.shape {
            Shape::Rect(rect) => {
                rect.fill == color && rect.rect.width() >= 800.0 && rect.rect.height() >= 600.0
            }
            _ => false,
        })
    }

    #[test]
    fn gpu_label_uses_the_adapter_name() {
        assert_eq!(gpu_label(Some("Test GPU".to_owned())), "Test GPU");
    }

    #[test]
    fn gpu_label_handles_a_missing_adapter() {
        assert_eq!(gpu_label(None), "unknown");
    }

    #[test]
    fn window_icon_is_a_valid_image() {
        let icon = eframe::icon_data::from_png_bytes(WINDOW_ICON).unwrap();

        assert_eq!((icon.width, icon.height), (64, 64));
    }

    #[test]
    fn window_background_is_painted_in_each_theme() {
        let app = test_app();
        for (theme, palette) in [(Theme::Dark, &theme::DARK), (Theme::Light, &theme::LIGHT)] {
            let ctx = themed_context();
            ctx.set_theme(theme);
            run_frame(&ctx, &app, vec![]);
            let (output, _) = run_frame(&ctx, &app, vec![]);

            assert!(panel_is_painted(&output, palette.surface), "{theme:?}");
        }
    }

    #[test]
    fn clicking_the_theme_buttons_switches_theme() {
        let app = test_app();
        let ctx = themed_context();
        ctx.set_theme(Theme::Dark);
        let (_, buttons) = run_frame(&ctx, &app, vec![]);

        click(&ctx, &app, button_for(&buttons, ThemePreference::Light));
        assert_eq!(ctx.theme(), Theme::Light);
        let (output, buttons) = run_frame(&ctx, &app, vec![]);
        assert!(panel_is_painted(&output, theme::LIGHT.surface));

        click(&ctx, &app, button_for(&buttons, ThemePreference::Dark));
        assert_eq!(ctx.theme(), Theme::Dark);
        let (output, _) = run_frame(&ctx, &app, vec![]);
        assert!(panel_is_painted(&output, theme::DARK.surface));
    }

    #[test]
    fn system_choice_follows_the_system_theme() {
        let app = test_app();
        let ctx = themed_context();
        ctx.set_theme(Theme::Dark);
        let (_, buttons) = run_frame(&ctx, &app, vec![]);

        click(&ctx, &app, button_for(&buttons, ThemePreference::System));

        assert_eq!(
            ctx.options(|options| options.theme_preference),
            ThemePreference::System
        );
    }
}
