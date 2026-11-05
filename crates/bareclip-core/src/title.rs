//! Titles: words drawn over the picture. A title's clip behaves like an image's, so every rule for
//! trimming, splitting and moving clips applies as it stands. What the title says and how it looks
//! lives here, under the clip's `MediaId`, and each clip has a title of its own.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{
    canvas::Colour,
    placement::{LARGEST_SCALE, Placement, SMALLEST_SCALE, WHOLE, fit_inside},
    timeline::MediaId,
};

/// How big a title can be, as parts of the canvas's shorter side.
pub const SMALLEST_SIZE: i32 = WHOLE / 50;
pub const LARGEST_SIZE: i32 = WHOLE / 2;
const DEFAULT_LOGO_SIZE: i32 = WHOLE / 5;
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
    // Text styles.
    Outlined,
    TextBox,
    Pride,
    Pill,
    Bright,
    Retro,
    LightBox,
    Neon,
    Tag,
    Expanded,
    Subscribe,
    Censor,
    Basic,
    Classic,
    Mono,
    Rounded,
    CaptionHighlight,
    SaleOutline,
    SwipeUp,
    Vintage,
    Badge,
    Like,
    OutlineBox,
    ColourShadow,
    Plush,
    MemeWords,
    ShadowWords,
    Cute,
    Boho,
    // Titles.
    Typewriter,
    Circular,
    Groovy,
    Fireworks,
    FadeWords,
    Smoke,
    Framed,
    Tidal,
    Glitch,
    DoubleLines,
    PushThrough,
    LargeHeading,
    OutlineShadow,
    QuickPeek,
    Statement,
    // Two lines.
    Underlined,
    Bouncing,
    Gliding,
    Funky,
    Boxed,
    FadeZoom,
    // Captions.
    Subtitle,
    Karaoke,
    MultiLine,
    Plunging,
    LowerThird,
    LowerThirdGreen,
    SideBar,
    Rapid,
    // Special.
    Quote,
    Rating,
    CreditsRoll,
    Countdown,
    SalePattern,
    MemeBands,
    // Intros and outros.
    IntroMirror,
    IntroFunky,
    IntroClean,
    IntroPlungeLeft,
    IntroPlungeRight,
    // Added after the rest, between Quick peek and Statement in the panel.
    Stencil,
    MakeItGlow,
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
    /// A band of the accent colour along the top, with a line around, on the background colour.
    TagBand,
    /// A short line under the words, in the accent colour.
    ShortLineUnder,
    /// A box of the accent colour behind each line of words.
    EachLineInBox,
    /// The background colour behind the first line, and the accent colour behind the second.
    BothInBoxes,
    /// A circle of the accent colour behind the words, with a wider one around it.
    Disc,
    /// A band of the accent colour across the canvas's whole width.
    Band,
    /// A band of the background colour along the canvas's top and bottom, with words in each.
    MemeBands,
    /// Big quote marks at the top left and bottom right, in the accent colour.
    QuoteMarks,
    /// A row of stars between the two lines of words.
    Stars,
    /// The words over and over, covering more than the canvas.
    Pattern,
    /// Covers the canvas, with a line across and a logo above the words.
    IntroLine,
    /// Covers the canvas in two colours, with the logo above the words.
    IntroTwoColours,
    /// Covers the canvas, with a thick line around its edge.
    IntroFrame,
    /// A panel down the left of the canvas, with the logo above the words.
    PanelLeft,
    /// A panel down the right of the canvas, with the logo above the words.
    PanelRight,
    /// The background colour over the whole canvas, with the words in the middle.
    FullFill,
}

impl TitleShape {
    /// Whether it is laid out on the whole canvas, whatever size the words are.
    pub fn covers_the_canvas(self) -> bool {
        matches!(
            self,
            Self::MemeBands
                | Self::IntroLine
                | Self::IntroTwoColours
                | Self::IntroFrame
                | Self::PanelLeft
                | Self::PanelRight
                | Self::Pattern
                | Self::FullFill
        )
    }
}

/// Copies of the words stepped down and to the right, behind them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WordStack {
    None,
    /// Solid black, this many steps.
    Black(u32),
    /// The accent colour, this many steps.
    Accent(u32),
    /// The colours of the rainbow, one after another.
    Rainbow,
    /// Bands of yellow, orange, red and blue running straight down.
    Bands,
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
    pub italic: bool,
    /// The words are written in capitals, whatever was typed.
    pub capitals: bool,
    /// Extra room between letters, in thousandths of how tall they are.
    pub spacing: i32,
    /// The words are the outline's colour around a dark middle, rather than filled in.
    pub hollow: bool,
    /// A soft light around the words, in their colour.
    pub glow: bool,
    /// How wide the outline is, in percent of how tall the letters are.
    pub outline_width: u32,
    /// The outline is the accent colour rather than black.
    pub outline_in_accent: bool,
    pub stack: WordStack,
    /// How round a box's corners are, in percent of how tall the letters are.
    pub round: u32,
    /// The first line's words are the accent colour.
    pub first_in_accent: bool,
    /// The second line's words are the accent colour.
    pub second_in_accent: bool,
    /// A solid black copy of the box behind it, stepped down and to the right.
    pub box_shadow: bool,
    /// The glow is the accent colour rather than the words' own.
    pub glow_in_accent: bool,
}

/// How many text styles show until "See all" is pressed.
pub const TEXT_STYLES_SHOWN: usize = 6;
/// The heading of the group that "See all" opens.
pub const TEXT_STYLES_HEADING: &str = "Text styles";

/// One of the groups the Text panel lists its styles in, with its heading.
pub const STYLE_GROUPS: [(&str, &[TitleStyle]); 7] = [
    ("Plain text", &[TitleStyle::Plain]),
    (
        TEXT_STYLES_HEADING,
        &[
            TitleStyle::Outlined,
            TitleStyle::TextBox,
            TitleStyle::Pride,
            TitleStyle::Pill,
            TitleStyle::Bright,
            TitleStyle::Retro,
            TitleStyle::LightBox,
            TitleStyle::Neon,
            TitleStyle::Tag,
            TitleStyle::Expanded,
            TitleStyle::Subscribe,
            TitleStyle::Censor,
            TitleStyle::Basic,
            TitleStyle::Classic,
            TitleStyle::Mono,
            TitleStyle::Rounded,
            TitleStyle::CaptionHighlight,
            TitleStyle::SaleOutline,
            TitleStyle::SwipeUp,
            TitleStyle::Vintage,
            TitleStyle::Badge,
            TitleStyle::Like,
            TitleStyle::OutlineBox,
            TitleStyle::ColourShadow,
            TitleStyle::Plush,
            TitleStyle::MemeWords,
            TitleStyle::ShadowWords,
            TitleStyle::Cute,
            TitleStyle::Boho,
        ],
    ),
    (
        "Title",
        &[
            TitleStyle::Typewriter,
            TitleStyle::Circular,
            TitleStyle::Groovy,
            TitleStyle::Fireworks,
            TitleStyle::FadeWords,
            TitleStyle::Smoke,
            TitleStyle::Framed,
            TitleStyle::Tidal,
            TitleStyle::Glitch,
            TitleStyle::DoubleLines,
            TitleStyle::PushThrough,
            TitleStyle::LargeHeading,
            TitleStyle::OutlineShadow,
            TitleStyle::QuickPeek,
            TitleStyle::Stencil,
            TitleStyle::MakeItGlow,
            TitleStyle::Statement,
        ],
    ),
    (
        "Two line",
        &[
            TitleStyle::Underlined,
            TitleStyle::Bouncing,
            TitleStyle::Gliding,
            TitleStyle::Funky,
            TitleStyle::Boxed,
            TitleStyle::FadeZoom,
        ],
    ),
    (
        "Caption",
        &[
            TitleStyle::Subtitle,
            TitleStyle::Karaoke,
            TitleStyle::MultiLine,
            TitleStyle::Plunging,
            TitleStyle::LowerThird,
            TitleStyle::LowerThirdGreen,
            TitleStyle::SideBar,
            TitleStyle::Rapid,
        ],
    ),
    (
        "Special",
        &[
            TitleStyle::Quote,
            TitleStyle::Rating,
            TitleStyle::CreditsRoll,
            TitleStyle::Countdown,
            TitleStyle::SalePattern,
            TitleStyle::MemeBands,
        ],
    ),
    (
        "Intro / Outro",
        &[
            TitleStyle::IntroMirror,
            TitleStyle::IntroFunky,
            TitleStyle::IntroClean,
            TitleStyle::IntroPlungeLeft,
            TitleStyle::IntroPlungeRight,
        ],
    ),
];

const PLAIN_LOOK: TitleLook = TitleLook {
    bold: false,
    outline: false,
    shadow: false,
    shape: TitleShape::Plain,
    second_line: None,
    left_aligned: false,
    italic: false,
    capitals: false,
    spacing: 0,
    hollow: false,
    glow: false,
    outline_width: 5,
    outline_in_accent: false,
    stack: WordStack::None,
    round: 6,
    first_in_accent: false,
    second_in_accent: false,
    box_shadow: false,
    glow_in_accent: false,
};

impl TitleStyle {
    pub fn name(self) -> &'static str {
        match self {
            Self::Plain => "Text",
            Self::Outlined => "Outlined",
            Self::TextBox => "Text box",
            Self::Pride => "Pride",
            Self::Pill => "Pill",
            Self::Bright => "Bright",
            Self::Retro => "Retro",
            Self::LightBox => "Light box",
            Self::Neon => "Neon",
            Self::Tag => "Tag",
            Self::Expanded => "Expanded",
            Self::Subscribe => "Subscribe",
            Self::Censor => "C*nsor",
            Self::Basic => "Basic",
            Self::Classic => "Classic",
            Self::Mono => "Mono",
            Self::Rounded => "Rounded",
            Self::CaptionHighlight => "Caption highlight",
            Self::SaleOutline => "Sale",
            Self::SwipeUp => "Swipe up",
            Self::Vintage => "Vintage",
            Self::Badge => "Badge",
            Self::Like => "Like",
            Self::OutlineBox => "Outline box",
            Self::ColourShadow => "Colour shadow",
            Self::Plush => "Plush",
            Self::MemeWords => "Meme",
            Self::ShadowWords => "Shadow",
            Self::Cute => "Cute",
            Self::Boho => "Boho",
            Self::Typewriter => "Typewriter",
            Self::Circular => "Circular",
            Self::Groovy => "Groovy",
            Self::Fireworks => "Fireworks",
            Self::FadeWords => "Fade",
            Self::Smoke => "Smoke",
            Self::Framed => "Framed",
            Self::Tidal => "Tidal",
            Self::Glitch => "Glitch",
            Self::DoubleLines => "Double lines",
            Self::PushThrough => "Push through",
            Self::LargeHeading => "Large heading",
            Self::OutlineShadow => "Outline shadow",
            Self::QuickPeek => "Quick peek",
            Self::Statement => "Statement",
            Self::Underlined => "Underlined",
            Self::Bouncing => "Bouncing",
            Self::Gliding => "Gliding",
            Self::Funky => "Funky",
            Self::Boxed => "Boxed",
            Self::FadeZoom => "Fade and zoom",
            Self::Subtitle => "Subtitle",
            Self::Karaoke => "Karaoke",
            Self::MultiLine => "Multi-line",
            Self::Plunging => "Plunging",
            Self::LowerThird => "Lower third",
            Self::LowerThirdGreen => "Lower third box",
            Self::SideBar => "Side bar",
            Self::Rapid => "Rapid",
            Self::Quote => "Quote",
            Self::Rating => "Rating",
            Self::CreditsRoll => "Credits roll",
            Self::Countdown => "Countdown",
            Self::SalePattern => "Sale pattern",
            Self::MemeBands => "Top and bottom",
            Self::IntroMirror => "Line intro",
            Self::IntroFunky => "Two colour intro",
            Self::IntroClean => "Frame intro",
            Self::IntroPlungeLeft => "Left panel intro",
            Self::IntroPlungeRight => "Right panel intro",
            Self::Stencil => "Stencil",
            Self::MakeItGlow => "Make it glow",
        }
    }

    /// Whether it runs across the canvas's whole width, so its card in the Text panel shows the
    /// middle of the canvas filling the card, instead of the whole canvas
    /// shrunk into a strip.
    pub fn fills_its_card(self) -> bool {
        let shape = self.look().shape;
        shape.covers_the_canvas() || shape == TitleShape::Band
    }

    /// Whether this is plain text or one of the text styles, which stay still.
    pub fn is_text_style(self) -> bool {
        STYLE_GROUPS[..2]
            .iter()
            .any(|(_, styles)| styles.contains(&self))
    }

    /// The words a new title in this style starts with.
    pub fn starting_words(self) -> (&'static str, &'static str) {
        let second = if self.look().second_line.is_some() {
            "Second line"
        } else {
            ""
        };
        match self {
            Self::Censor => ("C*NSOR", second),
            Self::Quote => ("Quote", "- Author"),
            Self::Rating => ("Rating", "Title"),
            Self::CreditsRoll => ("Credits Roll", "Enter text\nEnter text\nEnter text"),
            Self::Countdown => ("05:00", second),
            Self::Rapid => ("Rapid fire subtitles show one word at a time.", second),
            Self::MakeItGlow => ("MAKE IT\nGLOW", second),
            Self::SalePattern => ("SALE", second),
            Self::MemeBands => ("Top words", "Bottom words"),
            Self::IntroMirror
            | Self::IntroFunky
            | Self::IntroClean
            | Self::IntroPlungeLeft
            | Self::IntroPlungeRight => ("Your Headline", "Your-example.com"),
            _ => (self.name(), second),
        }
    }

    /// Whether this style has a place for a logo.
    pub fn has_logo(self) -> bool {
        matches!(
            self,
            Self::IntroMirror
                | Self::IntroFunky
                | Self::IntroClean
                | Self::IntroPlungeLeft
                | Self::IntroPlungeRight
        )
    }

    pub fn look(self) -> TitleLook {
        let plain = PLAIN_LOOK;
        let bold = TitleLook {
            bold: true,
            ..plain
        };
        match self {
            Self::Plain
            | Self::TextBox
            | Self::FadeWords
            | Self::Typewriter
            | Self::Smoke
            | Self::Glitch
            | Self::Countdown => plain,
            Self::Outlined | Self::Bright => TitleLook {
                outline: true,
                ..bold
            },
            Self::Pride => TitleLook {
                stack: WordStack::Rainbow,
                ..bold
            },
            Self::Pill | Self::SwipeUp => TitleLook {
                shape: TitleShape::Pill,
                ..bold
            },
            Self::Retro => TitleLook {
                stack: WordStack::Bands,
                capitals: true,
                ..bold
            },
            Self::LightBox | Self::Censor | Self::Mono => TitleLook {
                round: 2,
                capitals: self == Self::Censor,
                ..bold
            },
            Self::Neon => TitleLook {
                hollow: true,
                glow: true,
                outline: true,
                outline_width: 4,
                capitals: true,
                ..plain
            },
            Self::Tag => TitleLook {
                shape: TitleShape::TagBand,
                round: 2,
                ..bold
            },
            Self::Expanded => TitleLook {
                shape: TitleShape::Frame,
                spacing: 260,
                ..plain
            },
            Self::Subscribe => TitleLook {
                capitals: true,
                box_shadow: true,
                round: 8,
                ..bold
            },
            Self::Basic => TitleLook {
                shadow: true,
                ..bold
            },
            Self::Classic | Self::Vintage => TitleLook {
                italic: true,
                outline: self == Self::Classic,
                stack: if self == Self::Vintage {
                    WordStack::Accent(4)
                } else {
                    WordStack::None
                },
                ..bold
            },
            Self::Rounded => TitleLook { round: 26, ..bold },
            Self::CaptionHighlight | Self::MultiLine => TitleLook {
                shape: TitleShape::EachLineInBox,
                round: 2,
                ..bold
            },
            Self::SaleOutline => TitleLook {
                hollow: true,
                outline: true,
                outline_width: 4,
                capitals: true,
                ..bold
            },
            Self::Badge => TitleLook {
                shape: TitleShape::Pill,
                ..plain
            },
            Self::Like => TitleLook {
                box_shadow: true,
                round: 14,
                ..bold
            },
            Self::OutlineBox | Self::Framed => TitleLook {
                shape: TitleShape::Frame,
                ..plain
            },
            Self::ColourShadow => TitleLook {
                outline: true,
                outline_in_accent: true,
                outline_width: 3,
                stack: WordStack::Accent(3),
                ..bold
            },
            Self::Plush | Self::ShadowWords => TitleLook {
                stack: WordStack::Black(3),
                ..bold
            },
            Self::MemeWords => TitleLook {
                outline: true,
                ..plain
            },
            Self::Cute => TitleLook {
                outline: true,
                outline_in_accent: true,
                outline_width: 9,
                ..bold
            },
            Self::Boho => TitleLook {
                stack: WordStack::Black(2),
                ..plain
            },
            Self::Circular | Self::QuickPeek => TitleLook {
                shape: TitleShape::FullFill,
                ..plain
            },
            Self::Stencil => TitleLook {
                shape: TitleShape::FullFill,
                ..bold
            },
            Self::MakeItGlow => TitleLook {
                hollow: true,
                glow: true,
                glow_in_accent: true,
                outline: true,
                outline_width: 4,
                capitals: true,
                ..plain
            },
            Self::Groovy => TitleLook {
                hollow: true,
                outline: true,
                outline_width: 2,
                capitals: true,
                ..plain
            },
            Self::Fireworks => TitleLook {
                shadow: true,
                ..bold
            },
            Self::Tidal => TitleLook {
                spacing: 200,
                capitals: true,
                ..plain
            },
            Self::DoubleLines => TitleLook {
                shape: TitleShape::LinesAround,
                ..plain
            },
            Self::PushThrough => TitleLook {
                shape: TitleShape::FullFill,
                capitals: true,
                ..bold
            },
            Self::LargeHeading => TitleLook {
                shadow: true,
                ..bold
            },
            Self::OutlineShadow => plain,
            Self::Statement => TitleLook {
                shape: TitleShape::ShortLineUnder,
                ..bold
            },
            Self::Underlined => TitleLook {
                shape: TitleShape::LineBetween,
                second_line: Some(55),
                ..bold
            },
            Self::Bouncing => TitleLook {
                second_line: Some(100),
                second_in_accent: true,
                ..bold
            },
            Self::Gliding => TitleLook {
                shape: TitleShape::BothInBoxes,
                second_line: Some(100),
                round: 0,
                ..bold
            },
            Self::Funky => TitleLook {
                shape: TitleShape::BothInBoxes,
                second_line: Some(120),
                first_in_accent: true,
                round: 0,
                ..bold
            },
            Self::Boxed => TitleLook {
                shape: TitleShape::SecondInBox,
                second_line: Some(70),
                ..bold
            },
            Self::FadeZoom => TitleLook {
                second_line: Some(240),
                ..plain
            },
            Self::Subtitle | Self::Karaoke => TitleLook {
                shadow: true,
                ..plain
            },
            Self::Plunging | Self::SideBar => TitleLook {
                shape: TitleShape::SideBar,
                second_line: Some(55),
                left_aligned: true,
                ..bold
            },
            Self::LowerThird => TitleLook {
                shape: TitleShape::FirstInBox,
                second_line: Some(60),
                left_aligned: true,
                ..bold
            },
            Self::LowerThirdGreen => TitleLook {
                second_line: Some(70),
                left_aligned: true,
                round: 2,
                ..bold
            },
            Self::Rapid => TitleLook {
                shape: TitleShape::Band,
                ..bold
            },
            Self::Quote => TitleLook {
                shape: TitleShape::QuoteMarks,
                second_line: Some(60),
                ..bold
            },
            Self::Rating => TitleLook {
                shape: TitleShape::Stars,
                second_line: Some(50),
                ..bold
            },
            Self::CreditsRoll => TitleLook {
                second_line: Some(55),
                ..bold
            },
            Self::SalePattern => TitleLook {
                shape: TitleShape::Pattern,
                capitals: true,
                ..bold
            },
            Self::MemeBands => TitleLook {
                shape: TitleShape::MemeBands,
                second_line: Some(100),
                ..bold
            },
            Self::IntroMirror => TitleLook {
                shape: TitleShape::IntroLine,
                second_line: Some(55),
                ..bold
            },
            Self::IntroFunky => TitleLook {
                shape: TitleShape::IntroTwoColours,
                second_line: Some(55),
                second_in_accent: true,
                ..bold
            },
            Self::IntroClean => TitleLook {
                shape: TitleShape::IntroFrame,
                second_line: Some(50),
                ..bold
            },
            Self::IntroPlungeLeft => TitleLook {
                shape: TitleShape::PanelLeft,
                second_line: Some(50),
                left_aligned: true,
                ..bold
            },
            Self::IntroPlungeRight => TitleLook {
                shape: TitleShape::PanelRight,
                second_line: Some(50),
                left_aligned: true,
                ..bold
            },
        }
    }

    /// Whether this style has a colour of its own to choose.
    pub fn has_accent(self) -> bool {
        let look = self.look();
        !matches!(
            look.shape,
            TitleShape::Plain
                | TitleShape::Pill
                | TitleShape::Pattern
                | TitleShape::MemeBands
                | TitleShape::FullFill
        ) || look.outline_in_accent
            || look.glow_in_accent
            || look.first_in_accent
            || look.second_in_accent
            || matches!(look.stack, WordStack::Accent(_))
            || self == Self::Karaoke
    }

    /// Where a new title of this style goes.
    pub fn spot(self) -> TitleSpot {
        match self {
            Self::Subtitle | Self::Karaoke | Self::Rapid => TitleSpot::BottomMiddle,
            Self::Countdown => TitleSpot::TopRight,
            Self::LowerThird
            | Self::LowerThirdGreen
            | Self::SideBar
            | Self::Plunging
            | Self::MultiLine => TitleSpot::BottomLeft,
            _ => TitleSpot::Middle,
        }
    }

    /// How big a new title of this style is.
    pub fn starting_size(self) -> i32 {
        match self {
            Self::LargeHeading | Self::Plush | Self::MemeWords | Self::Boho => WHOLE * 16 / 100,
            Self::Subtitle | Self::Karaoke | Self::FadeZoom => WHOLE * 5 / 100,
            Self::LowerThird
            | Self::LowerThirdGreen
            | Self::SideBar
            | Self::Plunging
            | Self::MultiLine
            | Self::Rapid
            | Self::Expanded
            | Self::OutlineBox => WHOLE * 7 / 100,
            Self::CreditsRoll => WHOLE * 9 / 100,
            _ => WHOLE / 10,
        }
    }

    /// The font a new title of this style starts in. All of these ship with BareClip, so the style
    /// looks the same on every PC.
    pub fn starting_font(self) -> &'static str {
        match self {
            Self::Retro | Self::Plush => "Alfa Slab One",
            Self::Boho
            | Self::Underlined
            | Self::IntroMirror
            | Self::Statement
            | Self::FadeWords => "Abril Fatface",
            Self::Neon | Self::MemeWords | Self::SaleOutline | Self::MakeItGlow => "Vina Sans",
            Self::Groovy => "Raleway",
            Self::Subscribe | Self::Tidal | Self::Expanded => "Staatliches",
            Self::Mono | Self::Typewriter | Self::Countdown | Self::Censor => "Space Mono",
            Self::Bright
            | Self::Cute
            | Self::Pride
            | Self::Rounded
            | Self::Like
            | Self::Stencil => "Rubik",
            Self::Classic | Self::Vintage => "Vampiro One",
            _ => DEFAULT_FONT,
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
    /// A picture to show above the words, on styles with a place for one. Like a video, it stays a
    /// file on disk that the project refers to.
    pub logo: Option<PathBuf>,
    /// How big the logo is, as parts of the canvas's shorter side.
    pub logo_size: i32,
    /// How many of the five stars are lit, on a Rating.
    pub stars: u8,
}

impl Default for Title {
    fn default() -> Self {
        Self::in_style(TitleStyle::Plain)
    }
}

impl Title {
    pub fn in_style(style: TitleStyle) -> Self {
        let (text, second_text) = style.starting_words();
        let mut title = Self {
            text: text.to_owned(),
            second_text: second_text.to_owned(),
            font: style.starting_font().to_owned(),
            size: style.starting_size(),
            colour: WHITE,
            background: None,
            accent: [255, 176, 0],
            style,
            spot: Some(style.spot()),
            logo: None,
            logo_size: DEFAULT_LOGO_SIZE,
            stars: 4,
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
            TitleStyle::Retro => title.colour = [28, 24, 40],
            TitleStyle::LightBox => {
                title.colour = BLACK;
                title.background = Some(WHITE);
            }
            TitleStyle::Neon => title.colour = [217, 70, 239],
            TitleStyle::Tag => {
                title.colour = BLACK;
                title.background = Some(WHITE);
                title.accent = [233, 69, 96];
            }
            TitleStyle::Expanded => title.accent = WHITE,
            TitleStyle::Subscribe => title.background = Some([229, 57, 53]),
            TitleStyle::Censor => title.background = Some(BLACK),
            TitleStyle::Mono => {
                title.colour = [255, 235, 0];
                title.background = Some([24, 24, 24]);
            }
            TitleStyle::Classic => title.colour = [255, 235, 0],
            TitleStyle::Rounded => title.background = Some([168, 42, 240]),
            TitleStyle::CaptionHighlight => {
                title.colour = BLACK;
                title.accent = [255, 235, 0];
            }
            TitleStyle::SaleOutline => title.colour = [190, 214, 80],
            TitleStyle::SwipeUp => title.background = Some([0, 186, 90]),
            TitleStyle::Vintage => title.accent = [224, 74, 40],
            TitleStyle::Badge => title.background = Some([67, 160, 71]),
            TitleStyle::Like => title.background = Some([41, 121, 255]),
            TitleStyle::OutlineBox => title.accent = [235, 225, 60],
            TitleStyle::ColourShadow => title.accent = [40, 190, 70],
            TitleStyle::Plush => title.colour = [250, 225, 70],
            TitleStyle::Cute => title.accent = [225, 20, 80],
            TitleStyle::Boho => title.colour = [252, 245, 225],
            TitleStyle::Circular | TitleStyle::PushThrough => {
                title.background = Some([74, 140, 225]);
            }
            TitleStyle::Rapid => title.accent = [66, 133, 230],
            TitleStyle::Stencil => {
                title.background = Some([80, 52, 188]);
                title.colour = [16, 12, 30];
            }
            TitleStyle::MakeItGlow => title.accent = [255, 70, 70],
            TitleStyle::Smoke => title.colour = [150, 150, 150],
            TitleStyle::OutlineShadow => title.colour = [170, 170, 255],
            TitleStyle::Fireworks => title.accent = [150, 90, 235],
            TitleStyle::QuickPeek => title.background = Some([20, 14, 46]),
            TitleStyle::Statement => title.accent = [240, 128, 128],
            TitleStyle::Bouncing | TitleStyle::Quote => title.accent = [245, 166, 35],
            TitleStyle::Gliding => {
                title.background = Some([30, 30, 44]);
                title.accent = [30, 30, 44];
            }
            TitleStyle::Funky => {
                title.background = Some(WHITE);
                title.accent = [88, 70, 214];
            }
            TitleStyle::Karaoke => title.accent = [240, 128, 128],
            TitleStyle::MultiLine => title.accent = [66, 133, 230],
            TitleStyle::Plunging => {
                title.background = Some([30, 30, 44]);
                title.accent = [66, 133, 230];
            }
            TitleStyle::LowerThirdGreen => title.background = Some([38, 150, 120]),
            TitleStyle::SideBar => title.accent = [245, 166, 35],
            TitleStyle::Rating => title.accent = [255, 190, 0],
            TitleStyle::SalePattern => title.background = Some(BLACK),
            TitleStyle::IntroMirror | TitleStyle::IntroClean => {
                title.background = Some(BLACK);
                title.accent = WHITE;
            }
            TitleStyle::MemeBands => title.background = Some([16, 16, 16]),
            TitleStyle::IntroFunky => {
                title.background = Some(WHITE);
                title.accent = [66, 106, 214];
            }
            TitleStyle::IntroPlungeLeft => {
                title.background = Some([122, 112, 98]);
                title.accent = [200, 190, 170];
            }
            TitleStyle::IntroPlungeRight => {
                title.background = Some([26, 22, 48]);
                title.accent = [70, 60, 130];
            }
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
        self.logo_size = self.logo_size.clamp(SMALLEST_SIZE, LARGEST_SIZE);
        self.stars = self.stars.min(5);
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
        assert_eq!(listed.len(), 72);
        for style in &listed {
            assert_eq!(listed.iter().filter(|other| *other == style).count(), 1);
        }
    }

    #[test]
    fn stencil_starts_with_its_own_name() {
        // Its own name reads better here than "Your text / Goes here".
        assert_eq!(TitleStyle::Stencil.starting_words(), ("Stencil", ""));
    }

    #[test]
    fn only_styles_across_the_whole_canvas_fill_their_card() {
        let filling: Vec<TitleStyle> = STYLE_GROUPS
            .iter()
            .flat_map(|(_, styles)| styles.iter().copied())
            .filter(|style| style.fills_its_card())
            .collect();
        assert_eq!(
            filling,
            [
                TitleStyle::Circular,
                TitleStyle::PushThrough,
                TitleStyle::QuickPeek,
                TitleStyle::Stencil,
                TitleStyle::Rapid,
                TitleStyle::SalePattern,
                TitleStyle::MemeBands,
                TitleStyle::IntroMirror,
                TitleStyle::IntroFunky,
                TitleStyle::IntroClean,
                TitleStyle::IntroPlungeLeft,
                TitleStyle::IntroPlungeRight,
            ]
        );
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
                assert_eq!(title.text, style.starting_words().0);
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
