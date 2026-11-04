//! The GPU compositor: draws pictures onto one canvas, for anything that can't simply be copied
//! or encoded as it is. It knows nothing about video files.

mod color;
mod compositor;
mod scene;

pub use compositor::{Compositor, RenderError};
pub use scene::{Layer, LayerPicture, PictureLook, Rect, fit_inside, needs_compositor};
