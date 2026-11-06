//! The GPU compositor: draws pictures onto one canvas, for anything that can't simply be copied
//! or encoded as it is. It knows nothing about video files.

mod color;
mod compositor;
mod scene;

pub use compositor::{Compositor, RenderError};
pub use scene::{Layer, LayerPicture, PictureLook, Rect, fit_inside, needs_compositor};

#[cfg(test)]
mod shader_tests {
    /// Reads and checks every shader the way the GPU's driver would, which needs no GPU, so a
    /// mistake in one shows its line even where there is no graphics chip.
    #[test]
    fn every_shader_is_valid() {
        for (name, source) in [
            ("layer.wgsl", include_str!("layer.wgsl")),
            ("output.wgsl", include_str!("output.wgsl")),
            ("transition.wgsl", include_str!("transition.wgsl")),
            ("blur.wgsl", include_str!("blur.wgsl")),
        ] {
            let module = naga::front::wgsl::parse_str(source)
                .unwrap_or_else(|error| panic!("{name}: {}", error.emit_to_string(source)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::empty(),
            )
            .validate(&module)
            .unwrap_or_else(|error| panic!("{name}: {}", error.emit_to_string(source)));
        }
    }
}
