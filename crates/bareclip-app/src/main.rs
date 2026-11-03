#![cfg_attr(not(test), windows_subsystem = "windows")]

mod components;
mod editor;
mod fonts;
mod icons;
mod player;
mod preview;
mod storage;
mod text;
mod theme;
mod timeline;

use std::path::PathBuf;

use eframe::egui;

const WINDOW_ICON: &[u8] = include_bytes!("../../../assets/brand/png/app-icon-64.png");

fn main() -> eframe::Result {
    let opened_with: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("BareClip")
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([960.0, 600.0])
        .with_decorations(false)
        .with_drag_and_drop(true);
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
        Box::new(move |creation_context| {
            fonts::install(&creation_context.egui_ctx);
            theme::install(&creation_context.egui_ctx);
            let mut editor = editor::Editor::new(&creation_context.egui_ctx, true);
            if let Some(folder) = storage::app_folder() {
                editor.use_storage(&creation_context.egui_ctx, folder);
            }
            if !opened_with.is_empty() {
                editor.import(&creation_context.egui_ctx, &opened_with);
            }
            Ok(Box::new(BareClipApp { editor }))
        }),
    )
}

struct BareClipApp {
    editor: editor::Editor,
}

impl eframe::App for BareClipApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.editor.show(ui);
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_icon_is_a_valid_image() {
        let icon = eframe::icon_data::from_png_bytes(WINDOW_ICON).unwrap();

        assert_eq!((icon.width, icon.height), (64, 64));
    }
}
