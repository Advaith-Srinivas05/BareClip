//! A decoded picture, as plain memory that any crate can hand to any other.

use std::sync::atomic::{AtomicU64, Ordering};

/// Which formula turns a video's stored brightness and colour values into red, green and blue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMatrix {
    Bt601,
    Bt709,
    Bt2020,
}

/// How a picture's values are to be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorFormat {
    pub matrix: ColorMatrix,
    /// Values use all of 0 to 255, rather than the usual 16 to 235.
    pub full_range: bool,
}

impl ColorFormat {
    /// What every exported video is labelled as.
    pub const OUTPUT: Self = Self {
        matrix: ColorMatrix::Bt709,
        full_range: false,
    };
}

/// A picture with a brightness plane and two quarter-size colour planes.
#[derive(Clone, Copy, Debug)]
pub struct Yuv420<'a> {
    pub width: u32,
    pub height: u32,
    pub y: &'a [u8],
    pub y_stride: usize,
    pub u: &'a [u8],
    pub v: &'a [u8],
    pub uv_stride: usize,
    pub color: ColorFormat,
}

impl Yuv420<'_> {
    /// The size of the colour planes, rounded up so an odd-sized picture is fully covered.
    pub fn chroma_size(&self) -> (u32, u32) {
        (self.width.div_ceil(2), self.height.div_ceil(2))
    }
}

/// An image, ready to draw. Its colours have already been scaled by how see-through each pixel is.
#[derive(Clone, PartialEq, Eq)]
pub struct StillPicture {
    /// Tells images apart, so one already on the GPU isn't sent there again.
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl StillPicture {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            width,
            height,
            rgba,
        }
    }
}

impl std::fmt::Debug for StillPicture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StillPicture({} x {})", self.width, self.height)
    }
}

/// A plain black picture, kept in memory so dead space costs nothing to draw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlackPicture {
    width: u32,
    height: u32,
    y: Vec<u8>,
    chroma: Vec<u8>,
}

impl BlackPicture {
    pub fn new(width: u32, height: u32) -> Self {
        let chroma_width = width.div_ceil(2) as usize;
        let chroma_height = height.div_ceil(2) as usize;
        Self {
            width,
            height,
            y: vec![16; width as usize * height as usize],
            chroma: vec![128; chroma_width * chroma_height],
        }
    }

    pub fn view(&self) -> Yuv420<'_> {
        Yuv420 {
            width: self.width,
            height: self.height,
            y: &self.y,
            y_stride: self.width as usize,
            u: &self.chroma,
            v: &self.chroma,
            uv_stride: self.width.div_ceil(2) as usize,
            color: ColorFormat::OUTPUT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_odd_sized_picture_rounds_its_colour_planes_up() {
        let black = BlackPicture::new(5, 3);

        assert_eq!(black.view().chroma_size(), (3, 2));
    }

    #[test]
    fn black_is_the_lowest_value_of_the_usual_range_with_no_colour() {
        let black = BlackPicture::new(4, 2);
        let view = black.view();

        assert!(view.y.iter().all(|value| *value == 16));
        assert!(view.u.iter().chain(view.v).all(|value| *value == 128));
        assert_eq!(view.y.len(), 8);
        assert_eq!(view.u.len(), 2);
    }
}
