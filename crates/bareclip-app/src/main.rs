#![cfg_attr(not(test), windows_subsystem = "windows")]

mod components;
mod editor;
mod fonts;
mod handoff;
mod icons;
mod layers;
mod player;
mod preview;
mod stage;
mod storage;
mod text;
mod theme;
mod timeline;
mod title_frames;

use eframe::egui;

const WINDOW_ICON: &[u8] = include_bytes!("../../../assets/brand/png/app-icon-64.png");

fn main() -> eframe::Result {
    let launch = handoff::read_launch(std::env::args_os().skip(1));
    // "Edit with BareClip" goes to a window already open, if there is one.
    let mut claimed = None;
    if launch.edit {
        match handoff::hand_over(handoff::PIPE_PREFIX, &launch.paths) {
            handoff::HandOver::Delivered => return Ok(()),
            handoff::HandOver::Claimed(name) => claimed = Some(name),
            handoff::HandOver::Alone => {}
        }
    }
    let inbox = handoff::Inbox::listen(handoff::PIPE_PREFIX, claimed);

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
        wgpu_options: window_gpu_options(),
        ..Default::default()
    };

    eframe::run_native(
        "BareClip",
        options,
        Box::new(move |creation_context| {
            fonts::install(&creation_context.egui_ctx);
            theme::install(&creation_context.egui_ctx);
            let mut editor = editor::Editor::new(&creation_context.egui_ctx, true);
            if let Some(gpu) = &creation_context.wgpu_render_state {
                editor.use_gpu(layers::LayerDisplay::on_window(gpu));
            }
            if let Some(folder) = storage::app_folder() {
                editor.use_storage(&creation_context.egui_ctx, folder);
            }
            if launch.edit {
                editor.edit_files(&launch.paths);
            } else if !launch.paths.is_empty() {
                editor.import(&creation_context.egui_ctx, &launch.paths);
            }
            inbox.wake(&creation_context.egui_ctx);
            Ok(Box::new(BareClipApp { editor, inbox }))
        }),
    )
}

/// The window only asks for DirectX 12. Asking for every graphics system on the machine loads
/// drivers for all of them, including old OpenGL ones that have deadlocked here while loading,
/// and the compositor shares this GPU for showing layers.
fn window_gpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    let mut setup = eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup.instance_descriptor.backends = eframe::wgpu::Backends::DX12;
    eframe::egui_wgpu::WgpuConfiguration {
        wgpu_setup: setup.into(),
        ..Default::default()
    }
}

struct BareClipApp {
    editor: editor::Editor,
    /// Files from "Edit with BareClip" launches after this window opened.
    inbox: handoff::Inbox,
}

impl eframe::App for BareClipApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let arrived = self.inbox.take();
        if !arrived.is_empty() {
            self.editor.edit_files(&arrived);
        }
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
