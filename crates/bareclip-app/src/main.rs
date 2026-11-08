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

use std::{cell::RefCell, rc::Rc, sync::Arc};

use bareclip_render::GpuId;
use eframe::egui;

const WINDOW_ICON: &[u8] = include_bytes!("../../../assets/brand/png/app-icon-64.png");
/// Picks a chip the way Settings does, such as `1002:1638`, for measuring without touching the
/// real settings file.
const GPU_VARIABLE: &str = "BARECLIP_GPU";

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

    let wanted = wanted_gpu();
    bareclip_render::choose_gpu(wanted);
    let waiting = Rc::new(RefCell::new(Some((launch, inbox))));
    let result = open_window(wanted, waiting.clone());
    // A chip that can't open the window must not stop the app starting, because Settings is the
    // only way to change it. Once the window has been up, it isn't the chip's fault it closed.
    if wanted.is_some() && result.is_err() && waiting.borrow().is_some() {
        bareclip_render::choose_gpu(None);
        return open_window(None, waiting);
    }
    result
}

/// What opens the app with, which the first window to open takes.
type WaitingLaunch = Rc<RefCell<Option<(handoff::Launch, handoff::Inbox)>>>;

/// The chip picked in Settings, or the one the hidden switch names for a test.
fn wanted_gpu() -> Option<GpuId> {
    std::env::var(GPU_VARIABLE)
        .ok()
        .and_then(|text| GpuId::parse(&text))
        .or_else(|| {
            storage::app_folder()
                .and_then(|folder| storage::Settings::load(&folder).gpu)
                .map(GpuId::from)
        })
}

/// The picked chip if it can show the window, and otherwise the one Automatic would use.
fn pick_window_chip(
    adapters: &[eframe::wgpu::Adapter],
    surface: Option<&eframe::wgpu::Surface<'_>>,
    wanted: GpuId,
) -> Result<eframe::wgpu::Adapter, String> {
    let able: Vec<&eframe::wgpu::Adapter> = adapters
        .iter()
        .filter(|adapter| surface.is_none_or(|surface| adapter.is_surface_supported(surface)))
        .collect();
    let infos: Vec<eframe::wgpu::AdapterInfo> =
        able.iter().map(|adapter| adapter.get_info()).collect();
    bareclip_render::find_gpu(&infos, wanted)
        .or_else(|| bareclip_render::automatic_gpu(&infos))
        .map(|index| able[index].clone())
        .ok_or_else(|| "No graphics chip here can show the window.".to_owned())
}

fn open_window(wanted: Option<GpuId>, waiting: WaitingLaunch) -> eframe::Result {
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
        wgpu_options: window_gpu_options(wanted),
        ..Default::default()
    };

    eframe::run_native(
        "BareClip",
        options,
        Box::new(move |creation_context| {
            let (launch, inbox) = waiting
                .borrow_mut()
                .take()
                .ok_or("The app can only start once.")?;
            fonts::install(&creation_context.egui_ctx);
            theme::install(&creation_context.egui_ctx);
            let mut editor = editor::Editor::new(&creation_context.egui_ctx, true);
            if let Some(gpu) = &creation_context.wgpu_render_state {
                editor.use_gpu(layers::LayerDisplay::on_window(gpu));
                let running = gpu.adapter.get_info();
                // Only the chip the window really got decides the encoders, so one that has
                // gone missing behaves like Automatic.
                if wanted.is_some() {
                    bareclip_media::follow_chip(bareclip_media::ChipMaker::from_vendor(
                        running.vendor,
                    ));
                }
            }
            if let Some(folder) = storage::app_folder() {
                editor.use_storage(&creation_context.egui_ctx, folder);
            }
            if let Some(gpu) = &creation_context.wgpu_render_state {
                let chips = gpu
                    .available_adapters
                    .iter()
                    .map(eframe::wgpu::Adapter::get_info)
                    .filter(bareclip_render::is_real_chip)
                    .map(|info| editor::GpuEntry::of(&info))
                    .collect();
                editor.list_gpus(editor::GpuEntry::of(&gpu.adapter.get_info()), chips);
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
fn window_gpu_options(wanted: Option<GpuId>) -> eframe::egui_wgpu::WgpuConfiguration {
    let mut setup = eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup.instance_descriptor.backends = eframe::wgpu::Backends::DX12;
    if let Some(wanted) = wanted {
        setup.native_adapter_selector = Some(Arc::new(move |adapters, surface| {
            pick_window_chip(adapters, surface, wanted)
        }));
    }
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
