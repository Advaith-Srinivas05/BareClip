//! Which way up a picture is meant to be shown, and turning it that way. Phones and cameras
//! usually save the sensor's picture as it came, and note which way up they were held, for photos
//! and videos alike.

use ffmpeg_next::{codec::packet, format::stream::Stream, frame};

/// How a picture has to be turned to show the right way up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Orientation {
    #[default]
    Upright,
    Mirrored,
    HalfTurn,
    Flipped,
    /// Mirrored across the line from the top left to the bottom right.
    Transposed,
    QuarterClockwise,
    /// Mirrored across the line from the top right to the bottom left.
    Transversed,
    QuarterAnticlockwise,
}

impl Orientation {
    /// Reads FFmpeg's matrix for a picture, of nine numbers row by row. Only the top left four say
    /// anything about a turn. Anything but a whole quarter turn or mirror is treated as upright,
    /// because no photo format can store one and phones only ever turn by quarters.
    pub(crate) fn from_matrix(matrix: &[i32; 9]) -> Self {
        match [matrix[0], matrix[1], matrix[3], matrix[4]].map(i32::signum) {
            [-1, 0, 0, 1] => Self::Mirrored,
            [-1, 0, 0, -1] => Self::HalfTurn,
            [1, 0, 0, -1] => Self::Flipped,
            [0, 1, 1, 0] => Self::Transposed,
            [0, 1, -1, 0] => Self::QuarterClockwise,
            [0, -1, -1, 0] => Self::Transversed,
            [0, -1, 1, 0] => Self::QuarterAnticlockwise,
            _ => Self::Upright,
        }
    }

    fn from_bytes(bytes: &[u8]) -> Self {
        let (numbers, _) = bytes.as_chunks::<4>();
        if numbers.len() < 9 {
            return Self::Upright;
        }
        let mut matrix = [0; 9];
        for (number, bytes) in matrix.iter_mut().zip(numbers) {
            *number = i32::from_ne_bytes(*bytes);
        }
        Self::from_matrix(&matrix)
    }

    /// The way up a decoded picture carries with it, which is how a photo says it.
    pub(crate) fn of(picture: &frame::Video) -> Self {
        picture
            .side_data(frame::side_data::Type::DisplayMatrix)
            .map_or(Self::Upright, |side_data| {
                Self::from_bytes(side_data.data())
            })
    }

    /// The way up a video's stream says, which is how a phone's video says it.
    pub(crate) fn of_stream(stream: &Stream) -> Self {
        stream
            .side_data()
            .find(|side_data| side_data.kind() == packet::side_data::Type::DisplayMatrix)
            .map_or(Self::Upright, |side_data| {
                Self::from_bytes(side_data.data())
            })
    }

    pub fn is_upright(self) -> bool {
        self == Self::Upright
    }

    /// The size of a picture once it is turned. Turning that size back gives the stored one.
    pub fn turned_size(self, width: u32, height: u32) -> (u32, u32) {
        match self {
            Self::Upright | Self::Mirrored | Self::HalfTurn | Self::Flipped => (width, height),
            Self::Transposed
            | Self::QuarterClockwise
            | Self::Transversed
            | Self::QuarterAnticlockwise => (height, width),
        }
    }

    /// A picture of `width` by `height` samples of `bytes` each, with rows `stride` bytes apart,
    /// turned and packed tightly.
    pub(crate) fn turn_samples(
        self,
        data: &[u8],
        size: (usize, usize),
        stride: usize,
        bytes: usize,
    ) -> Vec<u8> {
        match bytes {
            1 => self.turn_fixed::<1>(data, size, stride),
            2 => self.turn_fixed::<2>(data, size, stride),
            4 => self.turn_fixed::<4>(data, size, stride),
            _ => unreachable!("samples are 1, 2 or 4 bytes"),
        }
    }

    /// The same for samples of `N` bytes. Which way to turn is settled once for the whole picture
    /// rather than for every pixel, which took a 1080p picture from 8.5 ms to 3.4 ms. Working in
    /// small squares to help the processor's cache was slower, at 11 ms.
    fn turn_fixed<const N: usize>(
        self,
        data: &[u8],
        (width, height): (usize, usize),
        stride: usize,
    ) -> Vec<u8> {
        let (turned_width, _) = self.turned_size(width as u32, height as u32);
        let mut turned = vec![0; width * height * N];
        let (last_x, last_y) = (width.saturating_sub(1), height.saturating_sub(1));
        let into = Placing {
            data,
            width,
            height,
            stride,
            turned: &mut turned,
            turned_width: turned_width as usize,
        };
        match self {
            Self::Upright => into.place::<N>(|x, y| (x, y)),
            Self::Mirrored => into.place::<N>(|x, y| (last_x - x, y)),
            Self::HalfTurn => into.place::<N>(|x, y| (last_x - x, last_y - y)),
            Self::Flipped => into.place::<N>(|x, y| (x, last_y - y)),
            Self::Transposed => into.place::<N>(|x, y| (y, x)),
            Self::QuarterClockwise => into.place::<N>(|x, y| (last_y - y, x)),
            Self::Transversed => into.place::<N>(|x, y| (last_y - y, last_x - x)),
            Self::QuarterAnticlockwise => into.place::<N>(|x, y| (y, last_x - x)),
        }
        turned
    }

    /// Tightly packed RGBA pixels of a picture of this size, turned.
    pub(crate) fn turn(self, rgba: Vec<u8>, width: u32, height: u32) -> Vec<u8> {
        if self.is_upright() {
            return rgba;
        }
        let size = (width as usize, height as usize);
        self.turn_samples(&rgba, size, size.0 * 4, 4)
    }

    /// A picture in the usual 4:2:0 layout, with eight bits a sample, turned.
    pub(crate) fn turn_yuv420(self, picture: &frame::Video) -> frame::Video {
        let (width, height) = self.turned_size(picture.width(), picture.height());
        let mut turned = frame::Video::new(picture.format(), width, height);
        for plane in 0..3 {
            let (from_width, from_height) = if plane == 0 {
                (picture.width() as usize, picture.height() as usize)
            } else {
                (
                    picture.width().div_ceil(2) as usize,
                    picture.height().div_ceil(2) as usize,
                )
            };
            let samples = self.turn_samples(
                picture.data(plane),
                (from_width, from_height),
                picture.stride(plane),
                1,
            );
            let (to_width, _) = self.turned_size(from_width as u32, from_height as u32);
            let to_width = to_width as usize;
            let stride = turned.stride(plane);
            let into = turned.data_mut(plane);
            for (row, values) in samples.chunks_exact(to_width).enumerate() {
                into[row * stride..][..to_width].copy_from_slice(values);
            }
        }
        turned.set_pts(picture.pts());
        turned.set_color_space(picture.color_space());
        turned.set_color_range(picture.color_range());
        turned
    }
}

/// A picture being copied into its turned place.
struct Placing<'a> {
    data: &'a [u8],
    width: usize,
    height: usize,
    stride: usize,
    turned: &'a mut [u8],
    turned_width: usize,
}

impl Placing<'_> {
    /// Copies every sample to where `to` says the one at `x`, `y` lands.
    fn place<const N: usize>(self, to: impl Fn(usize, usize) -> (usize, usize)) {
        for y in 0..self.height {
            let row = &self.data[y * self.stride..][..self.width * N];
            let (samples, _) = row.as_chunks::<N>();
            for (x, sample) in samples.iter().enumerate() {
                let (to_x, to_y) = to(x, y);
                let at = (to_y * self.turned_width + to_x) * N;
                self.turned[at..at + N].copy_from_slice(sample);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffmpeg_next::format::Pixel;

    const ONE: i32 = 1 << 16;
    const LAST: i32 = 1 << 30;

    #[test]
    fn each_way_up_is_read_from_the_matrix_ffmpeg_gives() {
        for (a, b, c, d, expected) in [
            (ONE, 0, 0, ONE, Orientation::Upright),
            (-ONE, 0, 0, ONE, Orientation::Mirrored),
            (-ONE, 0, 0, -ONE, Orientation::HalfTurn),
            (ONE, 0, 0, -ONE, Orientation::Flipped),
            (0, ONE, ONE, 0, Orientation::Transposed),
            (0, ONE, -ONE, 0, Orientation::QuarterClockwise),
            (0, -ONE, -ONE, 0, Orientation::Transversed),
            (0, -ONE, ONE, 0, Orientation::QuarterAnticlockwise),
        ] {
            let matrix = [a, b, 0, c, d, 0, 0, 0, LAST];
            assert_eq!(Orientation::from_matrix(&matrix), expected);
        }
    }

    #[test]
    fn a_matrix_that_is_no_whole_turn_counts_as_upright() {
        let matrix = [46341, 46341, 0, -46341, 46341, 0, 0, 0, 1 << 30];
        assert_eq!(Orientation::from_matrix(&matrix), Orientation::Upright);
    }

    #[test]
    fn rows_with_padding_turn_without_it() {
        // 3 x 2, each row padded to 4 bytes: 1 2 3 / 4 5 6.
        let data = [1, 2, 3, 0, 4, 5, 6, 0];

        let turned = Orientation::QuarterClockwise.turn_samples(&data, (3, 2), 4, 1);

        // Turned a quarter clockwise, the bottom left comes to the top left.
        assert_eq!(turned, [4, 1, 5, 2, 6, 3]);
    }

    #[test]
    fn samples_of_several_bytes_move_whole() {
        let data = [1, 1, 2, 2, 3, 3, 4, 4];

        let turned = Orientation::HalfTurn.turn_samples(&data, (2, 2), 4, 2);

        assert_eq!(turned, [4, 4, 3, 3, 2, 2, 1, 1]);
    }

    #[test]
    fn a_video_picture_turns_every_plane_and_swaps_its_size() {
        crate::init().expect("FFmpeg starts");
        let mut picture = frame::Video::new(Pixel::YUV420P, 4, 2);
        for plane in 0..3 {
            let stride = picture.stride(plane);
            let width = if plane == 0 { 4 } else { 2 };
            let rows = if plane == 0 { 2 } else { 1 };
            let data = picture.data_mut(plane);
            for row in 0..rows {
                for column in 0..width {
                    data[row * stride + column] = (plane * 100 + row * 10 + column) as u8;
                }
            }
        }

        let turned = Orientation::QuarterAnticlockwise.turn_yuv420(&picture);

        assert_eq!((turned.width(), turned.height()), (2, 4));
        let row = |plane: usize, row: usize, width: usize| {
            turned.data(plane)[row * turned.stride(plane)..][..width].to_vec()
        };
        // A quarter anticlockwise brings the top right to the top left.
        assert_eq!(row(0, 0, 2), [3, 13]);
        assert_eq!(row(0, 3, 2), [0, 10]);
        assert_eq!(row(1, 0, 1), [101]);
        assert_eq!(row(1, 1, 1), [100]);
        assert_eq!(row(2, 0, 1), [201]);
    }

    #[test]
    #[ignore = "a measurement, best run with --release"]
    fn how_long_turning_a_1080p_picture_takes() {
        crate::init().expect("FFmpeg starts");
        let picture = frame::Video::new(Pixel::YUV420P, 1920, 1080);
        let rounds = 60;
        let started = std::time::Instant::now();
        for _ in 0..rounds {
            std::hint::black_box(Orientation::QuarterClockwise.turn_yuv420(&picture));
        }
        println!(
            "{:.2} ms a picture",
            started.elapsed().as_secs_f64() * 1000.0 / f64::from(rounds)
        );
    }

    #[test]
    fn turning_a_size_twice_gives_it_back() {
        for orientation in [
            Orientation::Upright,
            Orientation::HalfTurn,
            Orientation::QuarterClockwise,
            Orientation::Transversed,
        ] {
            let (width, height) = orientation.turned_size(1920, 1080);
            assert_eq!(orientation.turned_size(width, height), (1920, 1080));
        }
    }
}
