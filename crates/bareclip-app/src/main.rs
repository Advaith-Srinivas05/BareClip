#![cfg_attr(not(test), windows_subsystem = "windows")]

use eframe::egui;

fn main() -> eframe::Result {
    let ffmpeg_license = match bareclip_media::init() {
        Ok(()) => bareclip_media::ffmpeg_info().license,
        Err(error) => format!("failed to start: {error}"),
    };

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_title("BareClip")
            .with_inner_size([1280.0, 720.0]),
        ..Default::default()
    };

    eframe::run_native(
        "BareClip",
        options,
        Box::new(|creation_context| {
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

impl eframe::App for BareClipApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.heading("BareClip");
        ui.label(format!("GPU: {}", self.gpu_label));
        ui.label(format!("FFmpeg: {}", self.ffmpeg_license));
    }
}

fn gpu_label(name: Option<String>) -> String {
    name.unwrap_or_else(|| "unknown".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_label_uses_the_adapter_name() {
        assert_eq!(gpu_label(Some("Test GPU".to_owned())), "Test GPU");
    }

    #[test]
    fn gpu_label_handles_a_missing_adapter() {
        assert_eq!(gpu_label(None), "unknown");
    }
}
