//! Draws titles into pictures, on the processor. The preview and the export both show what comes
//! out of here through the compositor, so what is on screen is what gets saved.

mod draw;
mod fonts;

pub use draw::{TitleDrawing, draw_font_name, draw_title};
pub use fonts::{FontList, font_list, has_font};
