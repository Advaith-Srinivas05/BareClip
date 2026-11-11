//! Turns pictures of the screen, which Windows gives as blue, green, red and alpha bytes, into
//! the video colours a recording is written in.

use bareclip_core::picture::{ColorFormat, ColorMatrix};
use ffmpeg_next::{ffi, format::Pixel, frame, software::scaling};

use crate::{MediaError, full::coefficients_of};

/// A picture in video colours, owned, ready for a `VideoWriter`.
pub struct ScreenPicture {
    frame: frame::Video,
}

impl ScreenPicture {
    pub fn size(&self) -> (u32, u32) {
        (self.frame.width(), self.frame.height())
    }

    pub fn view(&self) -> bareclip_core::picture::Yuv420<'_> {
        bareclip_core::picture::Yuv420 {
            width: self.frame.width(),
            height: self.frame.height(),
            y: self.frame.data(0),
            y_stride: self.frame.stride(0),
            u: self.frame.data(1),
            v: self.frame.data(2),
            uv_stride: self.frame.stride(1),
            color: ColorFormat {
                matrix: ColorMatrix::Bt709,
                full_range: false,
            },
        }
    }
}

/// Converts BGRA pictures of one size into BT.709 video colours of another, scaling on the way if
/// the sizes differ.
pub struct BgraConverter {
    context: scaling::Context,
    input: frame::Video,
}

impl BgraConverter {
    pub fn new(
        (width, height): (u32, u32),
        (out_width, out_height): (u32, u32),
    ) -> Result<Self, MediaError> {
        crate::init()?;
        let mut context = scaling::Context::get(
            Pixel::BGRA,
            width,
            height,
            Pixel::YUV420P,
            out_width,
            out_height,
            scaling::Flags::BILINEAR,
        )?;
        // SAFETY: the context is live, and these calls only read the constant coefficient tables
        // and copy their values into it. The screen is full range, the video the usual range.
        unsafe {
            let table = ffi::sws_getCoefficients(coefficients_of(ColorMatrix::Bt709));
            ffi::sws_setColorspaceDetails(
                context.as_mut_ptr(),
                table,
                1,
                table,
                0,
                0,
                1 << 16,
                1 << 16,
            );
        }
        Ok(Self {
            context,
            input: frame::Video::new(Pixel::BGRA, width, height),
        })
    }

    /// Converts one picture, given as rows of `stride` bytes.
    pub fn convert(&mut self, bgra: &[u8], stride: usize) -> Result<ScreenPicture, MediaError> {
        let (width, height) = (self.input.width() as usize, self.input.height() as usize);
        let into_stride = self.input.stride(0);
        let into = self.input.data_mut(0);
        for row in 0..height {
            let from = &bgra[row * stride..row * stride + width * 4];
            into[row * into_stride..row * into_stride + width * 4].copy_from_slice(from);
        }
        let mut converted = frame::Video::empty();
        self.context.run(&self.input, &mut converted)?;
        Ok(ScreenPicture { frame: converted })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: usize, height: usize, [b, g, r]: [u8; 3]) -> Vec<u8> {
        [b, g, r, 255].repeat(width * height)
    }

    #[test]
    fn white_and_black_become_the_usual_range() {
        let mut converter = BgraConverter::new((64, 32), (64, 32)).expect("it opens");
        let white = converter
            .convert(&solid(64, 32, [255, 255, 255]), 64 * 4)
            .expect("it converts");
        let view = white.view();
        assert!((i32::from(view.y[0]) - 235).abs() <= 1, "{}", view.y[0]);
        assert!((i32::from(view.u[0]) - 128).abs() <= 1);
        let black = converter
            .convert(&solid(64, 32, [0, 0, 0]), 64 * 4)
            .expect("it converts");
        assert!((i32::from(black.view().y[0]) - 16).abs() <= 1);
    }

    #[test]
    fn red_uses_the_bt709_matrix() {
        let mut converter = BgraConverter::new((16, 16), (16, 16)).expect("it opens");
        let red = converter
            .convert(&solid(16, 16, [0, 0, 255]), 16 * 4)
            .expect("it converts");
        let view = red.view();
        // BT.709 puts pure red at 63, 102, 240. BT.601 would give 81, 90, 240.
        assert!((i32::from(view.y[0]) - 63).abs() <= 2, "{}", view.y[0]);
        assert!((i32::from(view.u[0]) - 102).abs() <= 2, "{}", view.u[0]);
        assert!((i32::from(view.v[0]) - 240).abs() <= 2, "{}", view.v[0]);
    }

    #[test]
    fn rows_with_room_on_the_end_are_read_by_their_stride() {
        let (width, height, stride) = (8, 4, 64);
        let mut bgra = vec![0_u8; stride * height];
        for row in 0..height {
            bgra[row * stride..row * stride + width * 4].copy_from_slice(&solid(
                width,
                1,
                [255, 255, 255],
            ));
        }
        let mut converter = BgraConverter::new((8, 4), (8, 4)).expect("it opens");
        let picture = converter.convert(&bgra, stride).expect("it converts");
        let view = picture.view();
        assert!(view.y[..width].iter().all(|&y| y >= 233));
    }

    #[test]
    fn a_picture_can_be_scaled_on_the_way() {
        let mut converter = BgraConverter::new((64, 32), (32, 16)).expect("it opens");
        let picture = converter
            .convert(&solid(64, 32, [10, 20, 30]), 64 * 4)
            .expect("it converts");
        assert_eq!(picture.size(), (32, 16));
    }
}
