//! Titles: words drawn over the picture. A title's clip behaves like an image's, so every rule for
//! trimming, splitting and moving clips applies as it stands. What the title says and how it looks
//! lives here, under the clip's `MediaId`, and each clip has a title of its own.

use serde::{Deserialize, Serialize};

use crate::{
    canvas::Colour,
    placement::{LARGEST_SCALE, Placement, SMALLEST_SCALE, WHOLE, fit_inside},
    timeline::MediaId,
};

/// How big a title can be, as parts of the canvas's shorter side.
pub const SMALLEST_SIZE: i32 = WHOLE / 50;
pub const LARGEST_SIZE: i32 = WHOLE / 2;
/// The font a title starts in, which ships with BareClip.
pub const DEFAULT_FONT: &str = "Hanken Grotesk";

const WHITE: Colour = [255, 255, 255];
const BLACK: Colour = [0, 0, 0];
/// How far a title sits from the canvas's edges in a position, as a share of its shorter side.
const SPOT_MARGIN: f64 = 0.04;
/// How close a title's centre has to be to a position to count as being in it.
const CLOSE_TO_SPOT: f64 = 0.002;

/// What makes a media id a title's rather than a file's.
const TITLE_ID_BIT: u64 = 1 << 62;

impl MediaId {
    /// The id for a title, from a number no other title uses.
    pub fn for_title(number: u64) -> Self {
        Self(number | TITLE_ID_BIT)
    }

    /// Whether this is a title rather than a file.
    pub fn is_title(self) -> bool {
        self.0 & TITLE_ID_BIT != 0
    }

    /// The number a title's id came from.
    pub fn title_number(self) -> u64 {
        self.0 & !TITLE_ID_BIT
    }
}

/// What a title looks like before anyone changes it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TitleStyle {
    #[default]
    Plain,
    Outlined,
    TextBox,
    Pill,
    Bright,
    Framed,
    DoubleLines,
    LargeHeading,
    Underlined,
    Boxed,
    Subtitle,
    LowerThird,
    SideBar,
}

/// What a style draws around its words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleShape {
    /// Nothing, or a box of the background colour if there is one.
    Plain,
    /// The background colour, in a shape with fully round ends.
    Pill,
    /// A line all the way around, in the accent colour.
    Frame,
    /// A line above and one below, in the accent colour.
    LinesAround,
    /// A line between the first line and the second, in the accent colour.
    LineBetween,
    /// A box of the accent colour behind the second line.
    SecondInBox,
    /// A box of the accent colour behind the first line.
    FirstInBox,
    /// A bar down the left side, in the accent colour.
    SideBar,
}

/// The parts of a style that never change on a title.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TitleLook {
    pub bold: bool,
    pub outline: bool,
    pub shadow: bool,
    pub shape: TitleShape,
    /// The second line's size compared with the first, in percent. `None` means there isn't one.
    pub second_line: Option<u32>,
    pub left_aligned: bool,
}

/// One of the groups the Text panel lists its styles in, with its heading.
pub const STYLE_GROUPS: [(&str, &[TitleStyle]); 5] = [
    ("Plain text", &[TitleStyle::Plain]),
    (
        "Text styles",
        &[
            TitleStyle::Outlined,
            TitleStyle::TextBox,
            TitleStyle::Pill,
            TitleStyle::Bright,
        ],
    ),
    (
        "Title",
        &[
            TitleStyle::Framed,
            TitleStyle::DoubleLines,
            TitleStyle::LargeHeading,
        ],
    ),
    ("Two line", &[TitleStyle::Underlined, TitleStyle::Boxed]),
    (
        "Caption",
        &[
            TitleStyle::Subtitle,
            TitleStyle::LowerThird,
            TitleStyle::SideBar,
        ],
    ),
];

impl TitleStyle {
    pub fn name(self) -> &'static str {
        match self {
            Self::Plain => "Text",
            Self::Outlined => "Outlined",
            Self::TextBox => "Text box",
            Self::Pill => "Pill",
            Self::Bright => "Bright",
            Self::Framed => "Framed",
            Self::DoubleLines => "Double lines",
            Self::LargeHeading => "Large heading",
            Self::Underlined => "Underlined",
            Self::Boxed => "Boxed",
            Self::Subtitle => "Subtitle",
            Self::LowerThird => "Lower third",
            Self::SideBar => "Side bar",
        }
    }

    pub fn look(self) -> TitleLook {
        let plain = TitleLook {
            bold: false,
            outline: false,
            shadow: false,
            shape: TitleShape::Plain,
            second_line: None,
            left_aligned: false,
        };
        match self {
            Self::Plain | Self::TextBox => plain,
            Self::Outlined | Self::Bright => TitleLook {
                bold: true,
                outline: true,
                ..plain
            },
            Self::Pill => TitleLook {
                bold: true,
                shape: TitleShape::Pill,
                ..plain
            },
            Self::Framed => TitleLook {
                shape: TitleShape::Frame,
                ..plain
            },
            Self::DoubleLines => TitleLook {
                shape: TitleShape::LinesAround,
                ..plain
            },
            Self::LargeHeading => TitleLook {
                bold: true,
                shadow: true,
                ..plain
            },
            Self::Underlined => TitleLook {
                bold: true,
                shape: TitleShape::LineBetween,
                second_line: Some(55),
                ..plain
            },
            Self::Boxed => TitleLook {
                bold: true,
                shape: TitleShape::SecondInBox,
                second_line: Some(70),
                ..plain
            },
            Self::Subtitle => TitleLook {
                shadow: true,
                ..plain
            },
            Self::LowerThird => TitleLook {
                bold: true,
                shape: TitleShape::FirstInBox,
                second_line: Some(60),
                left_aligned: true,
                ..plain
            },
            Self::SideBar => TitleLook {
                bold: true,
                shape: TitleShape::SideBar,
                second_line: Some(55),
                left_aligned: true,
                ..plain
            },
        }
    }

    /// Whether this style has a colour of its own to choose.
    pub fn has_accent(self) -> bool {
        !matches!(self.look().shape, TitleShape::Plain | TitleShape::Pill)
    }

    /// Where a new title of this style goes.
    pub fn spot(self) -> TitleSpot {
        match self {
            Self::Subtitle => TitleSpot::BottomMiddle,
            Self::LowerThird | Self::SideBar => TitleSpot::BottomLeft,
            _ => TitleSpot::Middle,
        }
    }

    /// How big a new title of this style is.
    pub fn starting_size(self) -> i32 {
        match self {
            Self::LargeHeading => WHOLE * 16 / 100,
            Self::Subtitle => WHOLE * 5 / 100,
            Self::LowerThird | Self::SideBar => WHOLE * 7 / 100,
            _ => WHOLE / 10,
        }
    }
}

/// One of the nine places the Position control offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TitleSpot {
    TopLeft,
    TopMiddle,
    TopRight,
    MiddleLeft,
    Middle,
    MiddleRight,
    BottomLeft,
    BottomMiddle,
    BottomRight,
}

impl TitleSpot {
    /// Left to right and then down, with what screen readers call each one.
    pub const ALL: [(Self, &'static str); 9] = [
        (Self::TopLeft, "Top left"),
        (Self::TopMiddle, "Top middle"),
        (Self::TopRight, "Top right"),
        (Self::MiddleLeft, "Middle left"),
        (Self::Middle, "Middle"),
        (Self::MiddleRight, "Middle right"),
        (Self::BottomLeft, "Bottom left"),
        (Self::BottomMiddle, "Bottom middle"),
        (Self::BottomRight, "Bottom right"),
    ];

    /// How far along each way: 0 is the start, 1 the middle and 2 the end.
    fn along(self) -> (u8, u8) {
        match self {
            Self::TopLeft => (0, 0),
            Self::TopMiddle => (1, 0),
            Self::TopRight => (2, 0),
            Self::MiddleLeft => (0, 1),
            Self::Middle => (1, 1),
            Self::MiddleRight => (2, 1),
            Self::BottomLeft => (0, 2),
            Self::BottomMiddle => (1, 2),
            Self::BottomRight => (2, 2),
        }
    }

    /// The placement that puts a picture here, leaving everything else about it alone. However
    /// it is turned, all of it stays on the canvas if it can.
    pub fn placed(
        self,
        placement: Placement,
        picture: (u32, u32),
        canvas: (u32, u32),
    ) -> Placement {
        let rect = placement.rect(picture, canvas);
        let radians = placement.degrees().to_radians();
        let (width, height) = (f64::from(rect.width), f64::from(rect.height));
        let half_across = (width * radians.cos().abs() + height * radians.sin().abs()) / 2.0;
        let half_down = (width * radians.sin().abs() + height * radians.cos().abs()) / 2.0;
        let (canvas_width, canvas_height) = (f64::from(canvas.0), f64::from(canvas.1));
        let margin = canvas_width.min(canvas_height) * SPOT_MARGIN;
        let place = |along: u8, size: f64, half: f64| match along {
            0 => margin + half,
            1 => size / 2.0,
            _ => size - margin - half,
        };
        let (across, down) = self.along();
        let whole = f64::from(WHOLE);
        Placement {
            center_x: (place(across, canvas_width, half_across) / canvas_width * whole).round()
                as i32,
            center_y: (place(down, canvas_height, half_down) / canvas_height * whole).round()
                as i32,
            ..placement
        }
    }

    /// Which position a picture is in, if it is in one.
    pub fn of(placement: Placement, picture: (u32, u32), canvas: (u32, u32)) -> Option<Self> {
        Self::ALL.into_iter().map(|(spot, _)| spot).find(|spot| {
            let there = spot.placed(placement, picture, canvas);
            let close = |a: i32, b: i32| {
                (f64::from(a) - f64::from(b)).abs() / f64::from(WHOLE) <= CLOSE_TO_SPOT
            };
            close(placement.center_x, there.center_x) && close(placement.center_y, there.center_y)
        })
    }
}

/// What a title says and how it looks.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Title {
    /// Can run over several lines.
    pub text: String,
    /// The smaller words, on styles that have them.
    pub second_text: String,
    pub font: String,
    /// How tall its letters are, as parts of the canvas's shorter side.
    pub size: i32,
    pub colour: Colour,
    pub background: Option<Colour>,
    /// The colour of the style's lines and boxes, on styles that have them.
    pub accent: Colour,
    pub style: TitleStyle,
    /// The position it stays in as its size changes. `None` once it has been moved by hand, when
    /// it stays where it was put.
    pub spot: Option<TitleSpot>,
}

impl Default for Title {
    fn default() -> Self {
        Self::in_style(TitleStyle::Plain)
    }
}

impl Title {
    pub fn in_style(style: TitleStyle) -> Self {
        let mut title = Self {
            text: style.name().to_owned(),
            second_text: if style.look().second_line.is_some() {
                "Second line".to_owned()
            } else {
                String::new()
            },
            font: DEFAULT_FONT.to_owned(),
            size: style.starting_size(),
            colour: WHITE,
            background: None,
            accent: [255, 176, 0],
            style,
            spot: Some(style.spot()),
        };
        match style {
            TitleStyle::TextBox => title.background = Some(BLACK),
            TitleStyle::Pill => {
                title.colour = BLACK;
                title.background = Some(WHITE);
            }
            TitleStyle::Bright => title.colour = [173, 255, 47],
            TitleStyle::Framed => title.accent = [240, 128, 128],
            TitleStyle::DoubleLines => title.accent = [186, 104, 200],
            TitleStyle::Boxed => title.accent = [63, 81, 181],
            TitleStyle::LowerThird => title.accent = [0, 122, 255],
            _ => {}
        }
        title
    }

    /// Where a clip of this title goes once it is drawn at this size, from where the clip was.
    pub fn placed(
        &self,
        placement: Placement,
        picture: (u32, u32),
        scale: i32,
        canvas: (u32, u32),
    ) -> Placement {
        let scaled = Placement { scale, ..placement };
        match self.spot {
            Some(spot) => spot.placed(scaled, picture, canvas),
            None => scaled,
        }
    }

    /// The same title with anything out of range brought back in, in case a file was edited by
    /// hand.
    pub fn checked(mut self) -> Self {
        self.size = self.size.clamp(SMALLEST_SIZE, LARGEST_SIZE);
        if self.font.is_empty() {
            self.font = DEFAULT_FONT.to_owned();
        }
        self
    }
}

/// The scale at which a picture of this size shows `shrink` times its own pixels. A title is drawn
/// at the size it shows at, so it stays sharp, which makes this 1 unless it was too big to draw.
pub fn scale_for_pixels(picture: (u32, u32), canvas: (u32, u32), shrink: f64) -> i32 {
    let fitted = fit_inside(picture, canvas);
    let scale = f64::from(picture.0.max(1)) / f64::from(fitted.width.max(1.0)) / shrink;
    ((scale * f64::from(WHOLE)).round() as i32).clamp(SMALLEST_SCALE, LARGEST_SCALE)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANVAS: (u32, u32) = (1920, 1080);

    #[test]
    fn a_title_id_is_told_apart_from_a_files() {
        assert!(MediaId::for_title(7).is_title());
        assert!(!MediaId(7).is_title());
        assert_ne!(MediaId::for_title(7), MediaId::for_title(8));
    }

    #[test]
    fn every_style_is_listed_once() {
        let listed: Vec<TitleStyle> = STYLE_GROUPS
            .iter()
            .flat_map(|(_, styles)| styles.iter().copied())
            .collect();
        assert_eq!(listed.len(), 13);
        for style in &listed {
            assert_eq!(listed.iter().filter(|other| *other == style).count(), 1);
        }
    }

    #[test]
    fn only_styles_with_a_second_line_start_with_second_words() {
        for (_, styles) in STYLE_GROUPS {
            for style in styles {
                let title = Title::in_style(*style);
                assert_eq!(
                    title.second_text.is_empty(),
                    style.look().second_line.is_none(),
                    "{style:?}"
                );
                assert_eq!(title.text, style.name());
            }
        }
    }

    #[test]
    fn a_title_in_a_position_is_seen_to_be_in_it() {
        let picture = (400, 100);
        let start = Placement {
            scale: scale_for_pixels(picture, CANVAS, 1.0),
            ..Placement::default()
        };
        for (spot, name) in TitleSpot::ALL {
            let placed = spot.placed(start, picture, CANVAS);
            assert_eq!(TitleSpot::of(placed, picture, CANVAS), Some(spot), "{name}");
        }
    }

    #[test]
    fn a_title_elsewhere_is_in_no_position() {
        let picture = (400, 100);
        let placement = Placement {
            center_x: WHOLE / 3,
            center_y: WHOLE / 5,
            scale: scale_for_pixels(picture, CANVAS, 1.0),
            ..Placement::default()
        };
        assert_eq!(TitleSpot::of(placement, picture, CANVAS), None);
    }

    #[test]
    fn positions_keep_a_margin_and_leave_the_rest_of_the_placement_alone() {
        let picture = (400, 100);
        let start = Placement {
            scale: scale_for_pixels(picture, CANVAS, 1.0),
            opacity: WHOLE / 2,
            ..Placement::default()
        };
        let placed = TitleSpot::TopLeft.placed(start, picture, CANVAS);
        let rect = placed.rect(picture, CANVAS);
        let margin = 1080.0 * SPOT_MARGIN as f32;
        assert!((rect.x - margin).abs() < 0.5, "{}", rect.x);
        assert!((rect.y - margin).abs() < 0.5, "{}", rect.y);
        assert_eq!(placed.opacity, start.opacity);
        assert_eq!(placed.scale, start.scale);
    }

    #[test]
    fn at_this_scale_a_picture_shows_its_own_pixels() {
        for picture in [(400, 100), (100, 400), (1920, 1080), (3000, 200)] {
            let placement = Placement {
                center_x: WHOLE / 3,
                scale: scale_for_pixels(picture, CANVAS, 1.0),
                ..Placement::default()
            };
            let rect = placement.rect(picture, CANVAS);
            assert!((rect.width - picture.0 as f32).abs() < 0.5, "{picture:?}");
            assert!((rect.height - picture.1 as f32).abs() < 0.5, "{picture:?}");
        }
    }

    #[test]
    fn a_title_read_back_is_the_same_title() {
        let title = Title {
            text: "Two\nlines".to_owned(),
            background: Some([1, 2, 3]),
            ..Title::in_style(TitleStyle::Boxed)
        };
        let text = serde_json::to_string(&title).expect("it can be written");
        let back: Title = serde_json::from_str(&text).expect("it can be read");
        assert_eq!(back, title);
    }

    #[test]
    fn a_title_in_a_position_goes_there_and_one_moved_by_hand_stays_put() {
        let picture = (400, 100);
        let start = Placement {
            center_x: WHOLE / 3,
            ..Placement::default()
        };
        let scale = scale_for_pixels(picture, CANVAS, 1.0);
        let in_spot = Title {
            spot: Some(TitleSpot::TopLeft),
            ..Title::default()
        };
        let placed = in_spot.placed(start, picture, scale, CANVAS);
        assert_eq!(
            TitleSpot::of(placed, picture, CANVAS),
            Some(TitleSpot::TopLeft)
        );
        let by_hand = Title {
            spot: None,
            ..Title::default()
        };
        let stayed = by_hand.placed(start, picture, scale, CANVAS);
        assert_eq!(stayed.center_x, start.center_x);
        assert_eq!(stayed.scale, scale);
    }

    #[test]
    fn a_hand_edited_size_is_brought_back_in_range() {
        let title = Title {
            size: i32::MAX,
            font: String::new(),
            ..Title::default()
        }
        .checked();
        assert_eq!(title.size, LARGEST_SIZE);
        assert_eq!(title.font, DEFAULT_FONT);
    }
}
