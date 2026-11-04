//! What goes on a canvas, and whether that takes the GPU at all.

use bareclip_core::picture::{ColorFormat, StillPicture, Yuv420};
pub use bareclip_core::placement::{PictureLook, Rect, fit_inside};

/// What a layer shows.
#[derive(Clone, Copy, Debug)]
pub enum LayerPicture<'a> {
    Video(Yuv420<'a>),
    /// An image, which can be see-through in places.
    Still(&'a StillPicture),
}

impl<'a> From<Yuv420<'a>> for LayerPicture<'a> {
    fn from(picture: Yuv420<'a>) -> Self {
        Self::Video(picture)
    }
}

impl<'a> From<&'a StillPicture> for LayerPicture<'a> {
    fn from(picture: &'a StillPicture) -> Self {
        Self::Still(picture)
    }
}

impl LayerPicture<'_> {
    pub fn size(&self) -> (u32, u32) {
        match self {
            Self::Video(picture) => (picture.width, picture.height),
            Self::Still(picture) => (picture.width, picture.height),
        }
    }
}

/// One picture on the canvas. Later layers are drawn over earlier ones.
#[derive(Clone, Copy, Debug)]
pub struct Layer<'a> {
    pub picture: LayerPicture<'a>,
    pub rect: Rect,
    /// 0 is invisible, and 1 hides what is underneath.
    pub opacity: f32,
    pub look: PictureLook,
}

/// Whether these layers take the GPU. A single picture that already fills the canvas exactly,
/// in the colours the output is labelled with, can go straight to the encoder. Nothing at all
/// is just black. Keeping both of these off the GPU is what makes simple work stay fast.
pub fn needs_compositor(layers: &[Layer], canvas: (u32, u32)) -> bool {
    match layers {
        [] => false,
        [only] => {
            let LayerPicture::Video(picture) = only.picture else {
                return true;
            };
            let full_canvas = Rect {
                x: 0.0,
                y: 0.0,
                width: canvas.0 as f32,
                height: canvas.1 as f32,
            };
            only.opacity < 1.0
                || !only.look.is_plain()
                || only.rect != full_canvas
                || (picture.width, picture.height) != canvas
                || picture.color != ColorFormat::OUTPUT
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
            picture: picture.into(),
            rect: fit_inside((picture.width, picture.height), canvas),
            opacity: 1.0,
            look: PictureLook::PLAIN,
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
    fn a_turned_or_flipped_picture_needs_the_gpu() {
        let black = BlackPicture::new(64, 36);
        let mut turned = layer(&black, (64, 36));
        turned.look.degrees = 180.0;
        assert!(needs_compositor(&[turned], (64, 36)));

        let mut flipped = layer(&black, (64, 36));
        flipped.look.flip_x = true;
        assert!(needs_compositor(&[flipped], (64, 36)));
    }

    #[test]
    fn a_picture_in_other_colours_needs_the_gpu_or_it_would_look_wrong() {
        let black = BlackPicture::new(64, 36);
        let recoloured = |change: fn(&mut ColorFormat)| {
            let mut picture = black.view();
            change(&mut picture.color);
            Layer {
                picture: picture.into(),
                ..layer(&black, (64, 36))
            }
        };

        let other = recoloured(|color| color.matrix = ColorMatrix::Bt601);
        assert!(needs_compositor(&[other], (64, 36)));

        let full = recoloured(|color| color.full_range = true);
        assert!(needs_compositor(&[full], (64, 36)));
    }

    #[test]
    fn an_image_always_needs_the_gpu_even_at_the_canvass_exact_size() {
        let black = BlackPicture::new(64, 36);
        let still = StillPicture::new(64, 36, vec![0; 64 * 36 * 4]);
        let layer = Layer {
            picture: (&still).into(),
            ..layer(&black, (64, 36))
        };

        assert!(needs_compositor(&[layer], (64, 36)));
    }
}
