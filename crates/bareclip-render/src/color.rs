//! The numbers that turn a picture's stored values into red, green and blue.

use bareclip_core::picture::{ColorFormat, ColorMatrix};

/// How much of each of red and blue goes into brightness.
fn weights(matrix: ColorMatrix) -> (f32, f32) {
    match matrix {
        ColorMatrix::Bt601 => (0.299, 0.114),
        ColorMatrix::Bt709 => (0.2126, 0.0722),
        ColorMatrix::Bt2020 => (0.2627, 0.0593),
    }
}

/// Rows of a matrix turning stored values, from 0 to 1, into red, green and blue. The fourth
/// number of each row is added afterwards.
pub fn to_rgb(format: ColorFormat) -> [[f32; 4]; 3] {
    let (kr, kb) = weights(format.matrix);
    let kg = 1.0 - kr - kb;
    let (y_scale, y_offset, c_scale) = if format.full_range {
        (1.0, 0.0, 1.0)
    } else {
        (255.0 / 219.0, 16.0 / 255.0, 255.0 / 224.0)
    };
    let c_offset = 128.0 / 255.0;
    let r_v = 2.0 * (1.0 - kr) * c_scale;
    let b_u = 2.0 * (1.0 - kb) * c_scale;
    let g_u = -b_u * kb / kg;
    let g_v = -r_v * kr / kg;
    [
        [y_scale, 0.0, r_v, -y_scale * y_offset - r_v * c_offset],
        [
            y_scale,
            g_u,
            g_v,
            -y_scale * y_offset - g_u * c_offset - g_v * c_offset,
        ],
        [y_scale, b_u, 0.0, -y_scale * y_offset - b_u * c_offset],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const HD: ColorFormat = ColorFormat::OUTPUT;

    fn convert(format: ColorFormat, y: f32, u: f32, v: f32) -> [f32; 3] {
        to_rgb(format).map(|row| row[0] * y + row[1] * u + row[2] * v + row[3])
    }

    fn assert_close(found: [f32; 3], wanted: [f32; 3]) {
        for (found, wanted) in found.iter().zip(wanted) {
            assert!((found - wanted).abs() < 0.01, "{found} instead of {wanted}");
        }
    }

    #[test]
    fn the_lowest_value_of_the_usual_range_is_black() {
        assert_close(convert(HD, 16.0 / 255.0, 0.502, 0.502), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn the_highest_value_of_the_usual_range_is_white() {
        assert_close(convert(HD, 235.0 / 255.0, 0.502, 0.502), [1.0, 1.0, 1.0]);
    }

    #[test]
    fn full_range_white_is_at_the_very_top() {
        let full = ColorFormat {
            matrix: ColorMatrix::Bt709,
            full_range: true,
        };

        assert_close(convert(full, 1.0, 0.502, 0.502), [1.0, 1.0, 1.0]);
        assert_close(convert(full, 0.0, 0.502, 0.502), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn pure_red_in_hd_video_comes_out_red() {
        // BT.709 red in the usual range is 63, 102, 240.
        assert_close(
            convert(HD, 63.0 / 255.0, 102.0 / 255.0, 240.0 / 255.0),
            [1.0, 0.0, 0.0],
        );
    }

    #[test]
    fn the_same_values_mean_a_different_colour_in_old_video() {
        let old = ColorFormat {
            matrix: ColorMatrix::Bt601,
            full_range: false,
        };

        // BT.601 red in the usual range is 81, 90, 240.
        assert_close(
            convert(old, 81.0 / 255.0, 90.0 / 255.0, 240.0 / 255.0),
            [1.0, 0.0, 0.0],
        );
    }
}
