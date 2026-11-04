//! The size and shape of the finished video.
//!
//! The first video on the timeline sets it, and picking a ratio by hand overrides that. With
//! nothing on the timeline and nothing picked, a project is 16:9.

use serde::{Deserialize, Serialize};

use crate::{placement::final_size, timeline::MediaId, tracks::Tracks};

/// An empty project's size, and what a picked ratio is worked out from until a video arrives.
pub const EMPTY_SIZE: (u32, u32) = (1920, 1080);
/// The ratios on offer, and their names.
pub const RATIOS: [((u32, u32), &str); 6] = [
    ((16, 9), "16:9"),
    ((9, 16), "9:16"),
    ((1, 1), "1:1"),
    ((4, 5), "4:5"),
    ((4, 3), "4:3"),
    ((21, 9), "21:9"),
];
/// What the first video's own size is called in the picker.
pub const ORIGINAL_NAME: &str = "Original";
/// Larger than any encoder here takes, so a hand-edited file can't ask for something absurd.
const LARGEST_SIDE: u32 = 16_384;

/// A colour's red, green and blue, as stored values.
pub type Colour = [u8; 3];
pub const BLACK: Colour = [0, 0, 0];
/// The colours offered for the background, and what screen readers call them.
pub const BACKGROUND_SWATCHES: [(Colour, &str); 16] = [
    ([255, 255, 255], "White"),
    ([255, 0, 0], "Red"),
    ([255, 148, 34], "Orange"),
    ([255, 255, 0], "Yellow"),
    ([173, 255, 47], "Lime"),
    ([77, 238, 255], "Light blue"),
    ([255, 105, 240], "Pink"),
    ([110, 32, 255], "Purple"),
    ([0, 0, 0], "Black"),
    ([176, 0, 0], "Dark red"),
    ([255, 106, 0], "Dark orange"),
    ([255, 213, 0], "Gold"),
    ([76, 175, 80], "Green"),
    ([0, 122, 255], "Blue"),
    ([186, 0, 240], "Magenta"),
    ([0, 26, 122], "Navy"),
];

/// A colour written the usual way, such as #FF9422.
pub fn colour_to_hex(colour: Colour) -> String {
    format!("#{:02X}{:02X}{:02X}", colour[0], colour[1], colour[2])
}

/// Reads a colour written the usual way, with or without its #.
pub fn colour_from_hex(text: &str) -> Option<Colour> {
    let digits = text.trim().trim_start_matches('#');
    if digits.len() != 6 || !digits.is_ascii() {
        return None;
    }
    let part = |range: std::ops::Range<usize>| u8::from_str_radix(&digits[range], 16).ok();
    Some([part(0..2)?, part(2..4)?, part(4..6)?])
}

fn is_black(colour: &Colour) -> bool {
    *colour == BLACK
}

/// A size in this shape that keeps what a video of `base` size has: its longer side, or for a
/// square its shorter one, because a square at the longer side would only add pixels nothing
/// fills. Encoders need even sizes.
pub fn size_for_ratio(ratio: (u32, u32), base: (u32, u32)) -> (u32, u32) {
    let (ratio_width, ratio_height) = (f64::from(ratio.0.max(1)), f64::from(ratio.1.max(1)));
    let long = f64::from(base.0.max(base.1));
    let short = f64::from(base.0.min(base.1));
    let (width, height) = if ratio_width > ratio_height {
        (long, long * ratio_height / ratio_width)
    } else if ratio_height > ratio_width {
        (long * ratio_width / ratio_height, long)
    } else {
        (short, short)
    };
    let even = |value: f64| ((value / 2.0).round() as u32 * 2).max(2);
    (even(width), even(height))
}

/// What sets the finished video's size. It is part of what undo brings back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Canvas {
    /// The size of the first video to go on the timeline, kept even if that clip later goes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original: Option<(u32, u32)>,
    /// A ratio picked by hand, which stays until another is picked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ratio: Option<(u32, u32)>,
    /// What shows wherever no picture covers the canvas, including dead space.
    #[serde(default, skip_serializing_if = "is_black")]
    pub background: Colour,
}

impl Canvas {
    /// The finished video's size in pixels.
    pub fn size(&self) -> (u32, u32) {
        let base = self.original.unwrap_or(EMPTY_SIZE);
        match self.ratio {
            Some(ratio) => size_for_ratio(ratio, base),
            None => base,
        }
    }

    /// What the picker shows as chosen.
    pub fn name(&self) -> &'static str {
        let named = |ratio: (u32, u32)| {
            RATIOS
                .iter()
                .find(|(known, _)| *known == ratio)
                .map(|(_, name)| *name)
        };
        match self.ratio {
            Some(ratio) => named(ratio).unwrap_or(ORIGINAL_NAME),
            None if self.original.is_some() => ORIGINAL_NAME,
            None => "16:9",
        }
    }

    /// Whether a given choice is the current one, with `None` meaning Original.
    pub fn is_chosen(&self, ratio: Option<(u32, u32)>) -> bool {
        match (self.ratio, ratio) {
            (Some(chosen), Some(ratio)) => chosen == ratio,
            (None, None) => self.original.is_some(),
            // An empty project that nobody has touched is 16:9.
            (None, Some(ratio)) => self.original.is_none() && ratio == (16, 9),
            (Some(_), None) => false,
        }
    }

    /// Keeps this in step with the timeline. The first video to arrive sets the size, and an
    /// empty timeline forgets it, so the next one can. A ratio picked by hand stays.
    pub fn settle(&mut self, tracks: &Tracks, size_of: &dyn Fn(MediaId) -> Option<(u32, u32)>) {
        if tracks.is_empty() {
            self.original = None;
            return;
        }
        if self.original.is_none() {
            // The earliest clip is the first one, however many arrived together.
            let first = tracks
                .picture_tracks()
                .iter()
                .flat_map(|track| track.clips())
                .min_by_key(|clip| clip.start)
                .and_then(|clip| size_of(clip.media));
            self.original = first.and_then(|size| final_size(&[size]));
        }
    }

    /// The same with anything out of range dropped, for what comes from a file.
    pub fn checked(self) -> Self {
        let sensible = |size: &(u32, u32)| {
            (2..=LARGEST_SIDE).contains(&size.0) && (2..=LARGEST_SIDE).contains(&size.1)
        };
        Self {
            original: self
                .original
                .filter(sensible)
                .and_then(|size| final_size(&[size])),
            ratio: self
                .ratio
                .filter(|ratio| RATIOS.iter().any(|(known, _)| known == ratio)),
            background: self.background,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        time::Ticks,
        timeline::TimeRange,
        tracks::{TrackTarget, Tracks},
    };

    const SMALL: MediaId = MediaId(1);
    const LARGE: MediaId = MediaId(2);

    fn sizes(id: MediaId) -> Option<(u32, u32)> {
        Some(if id == SMALL {
            (1280, 720)
        } else {
            (2560, 1440)
        })
    }

    fn source() -> TimeRange {
        TimeRange {
            start: Ticks(0),
            end: Ticks(100),
        }
    }

    #[test]
    fn an_empty_project_is_16_by_9() {
        let canvas = Canvas::default();

        assert_eq!(canvas.size(), (1920, 1080));
        assert_eq!(canvas.name(), "16:9");
        assert!(canvas.is_chosen(Some((16, 9))));
        assert!(!canvas.is_chosen(None));
    }

    #[test]
    fn the_first_video_sets_the_exact_size_and_later_ones_change_nothing() {
        let mut tracks = Tracks::default();
        let mut canvas = Canvas::default();

        tracks.push(SMALL, source());
        canvas.settle(&tracks, &sizes);
        assert_eq!(canvas.size(), (1280, 720));
        assert_eq!(canvas.name(), ORIGINAL_NAME);
        assert!(canvas.is_chosen(None));

        tracks.push(LARGE, source());
        canvas.settle(&tracks, &sizes);
        assert_eq!(canvas.size(), (1280, 720), "a larger one doesn't take over");
    }

    #[test]
    fn a_size_not_quite_any_ratio_is_kept_as_it_is() {
        let mut tracks = Tracks::default();
        tracks.push(SMALL, source());
        let mut canvas = Canvas::default();

        canvas.settle(&tracks, &|_| Some((1920, 992)));

        assert_eq!(canvas.size(), (1920, 992));
    }

    #[test]
    fn the_size_stays_when_the_first_clip_goes_but_others_remain() {
        let mut tracks = Tracks::default();
        let first = tracks.push(SMALL, source());
        tracks.push(LARGE, source());
        let mut canvas = Canvas::default();
        canvas.settle(&tracks, &sizes);

        tracks.remove(first);
        canvas.settle(&tracks, &sizes);

        assert_eq!(canvas.size(), (1280, 720));
    }

    #[test]
    fn emptying_the_timeline_goes_back_to_16_by_9_and_the_next_video_sets_it() {
        let mut tracks = Tracks::default();
        let clip = tracks.push(SMALL, source());
        let mut canvas = Canvas::default();
        canvas.settle(&tracks, &sizes);

        tracks.remove(clip);
        canvas.settle(&tracks, &sizes);
        assert_eq!(canvas.size(), EMPTY_SIZE);

        tracks.push(LARGE, source());
        canvas.settle(&tracks, &sizes);
        assert_eq!(canvas.size(), (2560, 1440));
    }

    #[test]
    fn a_ratio_picked_on_an_empty_project_stays_and_takes_its_size_from_the_first_video() {
        let mut canvas = Canvas {
            ratio: Some((9, 16)),
            ..Canvas::default()
        };
        assert_eq!(canvas.size(), (1080, 1920));

        let mut tracks = Tracks::default();
        tracks.push(LARGE, source());
        canvas.settle(&tracks, &sizes);

        assert_eq!(canvas.size(), (1440, 2560));
        assert_eq!(canvas.name(), "9:16");
    }

    #[test]
    fn a_ratio_picked_by_hand_survives_emptying_the_timeline() {
        let mut tracks = Tracks::default();
        let clip = tracks.push(SMALL, source());
        let mut canvas = Canvas {
            ratio: Some((1, 1)),
            ..Canvas::default()
        };
        canvas.settle(&tracks, &sizes);

        tracks.remove(clip);
        canvas.settle(&tracks, &sizes);

        assert_eq!(canvas.name(), "1:1");
        assert_eq!(canvas.size(), (1080, 1080));
    }

    #[test]
    fn the_earliest_clip_counts_as_first_when_several_arrive_together() {
        let mut tracks = Tracks::default();
        tracks.place(LARGE, source(), Ticks(500), TrackTarget::New(0));
        tracks.place(SMALL, source(), Ticks(0), TrackTarget::New(0));
        let mut canvas = Canvas::default();

        canvas.settle(&tracks, &sizes);

        assert_eq!(canvas.size(), (1280, 720));
    }

    #[test]
    fn each_ratio_keeps_the_longer_side_of_the_video() {
        let base = (1920, 1080);

        assert_eq!(size_for_ratio((16, 9), base), (1920, 1080));
        assert_eq!(size_for_ratio((9, 16), base), (1080, 1920));
        assert_eq!(size_for_ratio((4, 5), base), (1536, 1920));
        assert_eq!(size_for_ratio((4, 3), base), (1920, 1440));
        assert_eq!(size_for_ratio((21, 9), base), (1920, 822));
    }

    #[test]
    fn a_square_keeps_the_shorter_side() {
        assert_eq!(size_for_ratio((1, 1), (1920, 1080)), (1080, 1080));
        assert_eq!(size_for_ratio((1, 1), (1080, 1920)), (1080, 1080));
    }

    #[test]
    fn a_tall_video_gives_the_same_sizes_as_a_wide_one() {
        let tall = (1080, 1920);

        assert_eq!(size_for_ratio((16, 9), tall), (1920, 1080));
        assert_eq!(size_for_ratio((9, 16), tall), (1080, 1920));
    }

    #[test]
    fn every_ratio_comes_out_even() {
        for (ratio, name) in RATIOS {
            for base in [(1920, 992), (853, 481), (3840, 2160), (2, 2)] {
                let (width, height) = size_for_ratio(ratio, base);
                assert!(
                    width % 2 == 0 && height % 2 == 0 && width > 0 && height > 0,
                    "{name} from {base:?} gave {width}x{height}"
                );
            }
        }
    }

    #[test]
    fn an_odd_first_video_rounds_up_to_an_even_size() {
        let mut tracks = Tracks::default();
        tracks.push(SMALL, source());
        let mut canvas = Canvas::default();

        canvas.settle(&tracks, &|_| Some((853, 481)));

        assert_eq!(canvas.size(), (854, 482));
    }

    #[test]
    fn a_colour_can_be_written_and_read_back() {
        assert_eq!(colour_to_hex([255, 148, 34]), "#FF9422");
        assert_eq!(colour_from_hex("#FF9422"), Some([255, 148, 34]));
        assert_eq!(colour_from_hex("ff9422"), Some([255, 148, 34]));
        assert_eq!(colour_from_hex("  #ff9422 "), Some([255, 148, 34]));
    }

    #[test]
    fn something_that_isnt_a_colour_is_refused() {
        for text in ["", "#FFF", "#FF94222", "#GG9422", "#ÿÿÿ", "red"] {
            assert_eq!(colour_from_hex(text), None, "{text}");
        }
    }

    #[test]
    fn a_black_background_writes_nothing_and_a_file_without_one_is_black() {
        let plain = serde_json::to_string(&Canvas::default()).expect("it writes");
        assert!(!plain.contains("background"), "{plain}");

        let read: Canvas = serde_json::from_str("{}").expect("it reads");
        assert_eq!(read.background, BLACK);
    }

    #[test]
    fn nonsense_from_a_file_is_dropped() {
        let canvas = Canvas {
            original: Some((0, 99_999)),
            ratio: Some((7, 0)),
            background: BLACK,
        }
        .checked();

        assert_eq!(canvas, Canvas::default());
    }

    #[test]
    fn sensible_values_from_a_file_are_kept() {
        let canvas = Canvas {
            original: Some((1920, 992)),
            ratio: Some((4, 5)),
            background: [1, 2, 3],
        };

        assert_eq!(canvas.checked(), canvas);
    }
}
