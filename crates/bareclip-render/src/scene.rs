//! What goes on a canvas, and whether that takes the GPU at all.

use bareclip_core::picture::{ColorFormat, Yuv420};
pub use bareclip_core::placement::{Rect, fit_inside};

/// One picture on the canvas. Later layers are drawn over earlier ones.
#[derive(Clone, Copy, Debug)]
pub struct Layer<'a> {
    pub picture: Yuv420<'a>,
    pub rect: Rect,
    /// 0 is invisible, and 1 hides what is underneath.
    pub opacity: f32,
}

/// Whether these layers take the GPU. A single picture that already fills the canvas exactly,
/// in the colours the output is labelled with, can go straight to the encoder. Nothing at all
/// is just black. Keeping both of these off the GPU is what makes simple work stay fast.
pub fn needs_compositor(layers: &[Layer], canvas: (u32, u32)) -> bool {
    match layers {
        [] => false,
        [only] => {
            let full_canvas = Rect {
                x: 0.0,
                y: 0.0,
                width: canvas.0 as f32,
                height: canvas.1 as f32,
            };
            only.opacity < 1.0
                || only.rect != full_canvas
                || (only.picture.width, only.picture.height) != canvas
                || only.picture.color != ColorFormat::OUTPUT
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareclip_core::picture::{BlackPicture, ColorMatrix};

    fn layer(black: &BlackPicture, canvas: (u32, u32)) -> Layer<'_> {
        let picture = black.view();
        Layer {
            picture,
            rect: fit_inside((picture.width, picture.height), canvas),
            opacity: 1.0,
        }
    }

    #[test]
    fn a_wide_picture_on_a_tall_canvas_gets_bars_above_and_below() {
        assert_eq!(
            fit_inside((1920, 1080), (1080, 1920)),
            Rect {
                x: 0.0,
                y: 656.25,
                width: 1080.0,
                height: 607.5,
            }
        );
    }

    #[test]
    fn a_tall_picture_on_a_wide_canvas_gets_bars_at_the_sides() {
        let rect = fit_inside((1080, 1920), (1920, 1080));

        assert_eq!(rect.height, 1080.0);
        assert_eq!(rect.width, 607.5);
        assert_eq!(rect.x, 656.25);
    }

    #[test]
    fn a_smaller_picture_of_the_same_shape_grows_to_fill_the_canvas() {
        assert_eq!(
            fit_inside((1920, 1080), (2560, 1440)),
            Rect {
                x: 0.0,
                y: 0.0,
                width: 2560.0,
                height: 1440.0,
            }
        );
    }

    #[test]
    fn nothing_on_the_canvas_needs_no_gpu() {
        assert!(!needs_compositor(&[], (64, 36)));
    }

    #[test]
    fn a_picture_that_already_fills_the_canvas_needs_no_gpu() {
        let black = BlackPicture::new(64, 36);

        assert!(!needs_compositor(&[layer(&black, (64, 36))], (64, 36)));
    }

    #[test]
    fn a_picture_of_another_size_needs_the_gpu() {
        let black = BlackPicture::new(32, 18);

        assert!(needs_compositor(&[layer(&black, (64, 36))], (64, 36)));
    }

    #[test]
    fn two_pictures_at_once_need_the_gpu() {
        let black = BlackPicture::new(64, 36);
        let one = layer(&black, (64, 36));

        assert!(needs_compositor(&[one, one], (64, 36)));
    }

    #[test]
    fn a_see_through_picture_needs_the_gpu() {
        let black = BlackPicture::new(64, 36);
        let mut one = layer(&black, (64, 36));
        one.opacity = 0.5;

        assert!(needs_compositor(&[one], (64, 36)));
    }

    #[test]
    fn a_picture_in_other_colours_needs_the_gpu_or_it_would_look_wrong() {
        let black = BlackPicture::new(64, 36);
        let mut other = layer(&black, (64, 36));
        other.picture.color.matrix = ColorMatrix::Bt601;
        assert!(needs_compositor(&[other], (64, 36)));

        let mut full = layer(&black, (64, 36));
        full.picture.color.full_range = true;
        assert!(needs_compositor(&[full], (64, 36)));
    }
}
