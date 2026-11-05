//! What a clip looks like beyond where it sits: its filter, colour adjustments and picture fades, and
//! the transition from it into the clip after it.

use serde::{Deserialize, Serialize};

use crate::time::Ticks;

/// How far each colour adjustment can go either way.
pub const ADJUST_RANGE: i32 = 1000;
/// How long a new transition lasts.
pub const DEFAULT_TRANSITION_SECONDS: f64 = 2.0 / 3.0;
pub const SHORTEST_TRANSITION_SECONDS: f64 = 0.1;
pub const LONGEST_TRANSITION_SECONDS: f64 = 3.0;

/// One of the ways of getting from one clip to the next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TransitionKind {
    #[default]
    CrossFade,
    CrossBlur,
    Burn,
    FadeThroughBlack,
    FadeThroughWhite,
    HorizontalBanding,
    Tiles,
    HardWipeDown,
    HardWipeUp,
    HardWipeLeft,
    HardWipeRight,
    SoftWipeDown,
    SoftWipeUp,
    SoftWipeLeft,
    SoftWipeRight,
    DiagonalSoftWipe,
    Blinds,
    BarnDoorsVertical,
    BarnDoorsHorizontal,
    CircularWipe,
    Close,
    DiamondHorizontal,
    DiamondVertical,
    Thirds,
    Bloom,
    Collage,
    Spin,
    ZoomIn,
    ZoomOut,
    PushDown,
    PushUp,
    PushLeft,
    PushRight,
    Beams,
    Heart,
    Ink,
    LiquidStreaks,
    GlitchReveal,
    MorphGlitch,
    Pixelate,
    Cube,
    Exchange,
    PageTurn,
    Reveal,
}

/// The transitions the panel lists, in groups, with their headings.
pub const TRANSITION_GROUPS: [(&str, &[TransitionKind]); 7] = {
    use TransitionKind as T;
    [
        (
            "Fades & blurs",
            &[
                T::CrossFade,
                T::CrossBlur,
                T::Burn,
                T::FadeThroughBlack,
                T::FadeThroughWhite,
                T::HorizontalBanding,
                T::Tiles,
            ],
        ),
        (
            "Wipes",
            &[
                T::HardWipeDown,
                T::HardWipeUp,
                T::HardWipeLeft,
                T::HardWipeRight,
                T::SoftWipeDown,
                T::SoftWipeUp,
                T::SoftWipeLeft,
                T::SoftWipeRight,
                T::DiagonalSoftWipe,
                T::Blinds,
                T::BarnDoorsVertical,
                T::BarnDoorsHorizontal,
                T::CircularWipe,
                T::Close,
                T::DiamondHorizontal,
                T::DiamondVertical,
                T::Thirds,
            ],
        ),
        (
            "Zooms & spins",
            &[T::Bloom, T::Collage, T::Spin, T::ZoomIn, T::ZoomOut],
        ),
        (
            "Pushes",
            &[T::PushDown, T::PushUp, T::PushLeft, T::PushRight],
        ),
        ("Cartoon", &[T::Beams, T::Heart, T::Ink, T::LiquidStreaks]),
        ("Glitches", &[T::GlitchReveal, T::MorphGlitch, T::Pixelate]),
        ("3D", &[T::Cube, T::Exchange, T::PageTurn, T::Reveal]),
    ]
};

impl TransitionKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::CrossFade => "Cross fade",
            Self::CrossBlur => "Cross blur",
            Self::Burn => "Burn",
            Self::FadeThroughBlack => "Fade through black",
            Self::FadeThroughWhite => "Fade through white",
            Self::HorizontalBanding => "Horizontal banding",
            Self::Tiles => "Tiles",
            Self::HardWipeDown => "Hard wipe down",
            Self::HardWipeUp => "Hard wipe up",
            Self::HardWipeLeft => "Hard wipe left",
            Self::HardWipeRight => "Hard wipe right",
            Self::SoftWipeDown => "Soft wipe down",
            Self::SoftWipeUp => "Soft wipe up",
            Self::SoftWipeLeft => "Soft wipe left",
            Self::SoftWipeRight => "Soft wipe right",
            Self::DiagonalSoftWipe => "Diagonal soft wipe",
            Self::Blinds => "Blinds",
            Self::BarnDoorsVertical => "Barn doors - vertical",
            Self::BarnDoorsHorizontal => "Barn doors - horizontal",
            Self::CircularWipe => "Circular wipe",
            Self::Close => "Close",
            Self::DiamondHorizontal => "Diamond - horizontal",
            Self::DiamondVertical => "Diamond - vertical",
            Self::Thirds => "Thirds",
            Self::Bloom => "Bloom",
            Self::Collage => "Collage",
            Self::Spin => "Spin",
            Self::ZoomIn => "Zoom in",
            Self::ZoomOut => "Zoom out",
            Self::PushDown => "Push down",
            Self::PushUp => "Push up",
            Self::PushLeft => "Push left",
            Self::PushRight => "Push right",
            Self::Beams => "Beams",
            Self::Heart => "Heart",
            Self::Ink => "Ink",
            Self::LiquidStreaks => "Liquid streaks",
            Self::GlitchReveal => "Glitch reveal",
            Self::MorphGlitch => "Morph glitch",
            Self::Pixelate => "Pixelate",
            Self::Cube => "Cube",
            Self::Exchange => "Exchange",
            Self::PageTurn => "Page turn",
            Self::Reveal => "Reveal",
        }
    }

    /// The number the shader knows this one by.
    pub fn code(self) -> u32 {
        self as u32
    }
}

/// The transition from a clip into the clip straight after it on its row. It is centred on the cut
/// and moves nothing, so half of it plays over the end of the first clip and half over the start of
/// the second.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Transition {
    pub kind: TransitionKind,
    /// How long it takes, in ticks.
    pub duration: i64,
}

impl Transition {
    pub fn new(kind: TransitionKind) -> Self {
        Self {
            kind,
            duration: Ticks::from_seconds(DEFAULT_TRANSITION_SECONDS).0,
        }
    }

    pub fn half(self) -> Ticks {
        Ticks(self.duration / 2)
    }
}

/// How far through a transition a moment is, from 0 to 1, given where its cut is.
pub fn join_progress(cut: Ticks, duration: i64, now: Ticks) -> f32 {
    let start = cut.0 - duration / 2;
    (((now.0 - start) as f64 / duration.max(1) as f64) as f32).clamp(0.0, 1.0)
}

/// A picture being drawn together with the next one up, as the two sides of a transition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerJoin {
    pub kind: TransitionKind,
    pub progress: f32,
    /// Whether this is the clip going out, rather than the one coming in.
    pub goes_out: bool,
}

/// One of the colour looks a clip can have.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Filter {
    #[default]
    None,
    Retro,
    OrangeAndTeal,
    BoldAndBlue,
    GoldenHour,
    VibrantVlogger,
    PurpleUndertone,
    WinterSunset,
    Film35mm,
    Contrast,
    Autumn,
    Winter,
    OldWestern,
    WarmCoastline,
    CoolCoastline,
    WarmCountryside,
    CoolCountryside,
    Golden,
    Dreamscape,
    Sunrise,
    WarmToneFilm,
    CoolTone,
    PastelDreams,
    Increased,
    Scenery,
    Portrait,
    Indoors,
    Outdoors,
    Muted,
    BlackAndWhite1,
    BlackAndWhite2,
    SoftBlackAndWhite,
    MutedBlackAndWhite,
    Gloomy,
    DeepFried,
    Euphoric,
    YellowOrangeDuotone,
    PinkPurpleDuotone,
    BluePinkDuotone,
    GreenBlueDuotone,
    WhiteOverlay,
    BlackOverlay,
    YellowOverlay,
    OrangeOverlay,
    RedOverlay,
    PinkOverlay,
    PurpleOverlay,
    BlueOverlay,
    TurquoiseOverlay,
    GreenOverlay,
}

/// Every filter there is to choose, in the panel's order. `None` isn't listed here.
pub const FILTERS: [Filter; 49] = {
    use Filter as F;
    [
        F::Retro,
        F::OrangeAndTeal,
        F::BoldAndBlue,
        F::GoldenHour,
        F::VibrantVlogger,
        F::PurpleUndertone,
        F::WinterSunset,
        F::Film35mm,
        F::Contrast,
        F::Autumn,
        F::Winter,
        F::OldWestern,
        F::WarmCoastline,
        F::CoolCoastline,
        F::WarmCountryside,
        F::CoolCountryside,
        F::Golden,
        F::Dreamscape,
        F::Sunrise,
        F::WarmToneFilm,
        F::CoolTone,
        F::PastelDreams,
        F::Increased,
        F::Scenery,
        F::Portrait,
        F::Indoors,
        F::Outdoors,
        F::Muted,
        F::BlackAndWhite1,
        F::BlackAndWhite2,
        F::SoftBlackAndWhite,
        F::MutedBlackAndWhite,
        F::Gloomy,
        F::DeepFried,
        F::Euphoric,
        F::YellowOrangeDuotone,
        F::PinkPurpleDuotone,
        F::BluePinkDuotone,
        F::GreenBlueDuotone,
        F::WhiteOverlay,
        F::BlackOverlay,
        F::YellowOverlay,
        F::OrangeOverlay,
        F::RedOverlay,
        F::PinkOverlay,
        F::PurpleOverlay,
        F::BlueOverlay,
        F::TurquoiseOverlay,
        F::GreenOverlay,
    ]
};

/// What a filter does to colours, as numbers the shader can use. Worked out in BareClip's own terms,
/// not copied from anywhere, so each only aims at what its name and thumbnail suggest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grade {
    /// Exposure, contrast, saturation and temperature, each 0 for no change.
    pub basic: [f32; 4],
    /// What the darkest parts are multiplied by, and how much of it to use in the last place.
    pub shadows: [f32; 3],
    /// What the brightest parts are multiplied by.
    pub highlights: [f32; 3],
    /// The colours dark and light parts become, and how far they go there.
    pub duotone: ([f32; 3], [f32; 3], f32),
    /// A colour to wash over everything, and how much.
    pub wash: ([f32; 3], f32),
    /// The clip's own Exposure, Contrast, Saturation and Temperature, on top of all that.
    pub adjust: [f32; 4],
}

impl Grade {
    pub const NONE: Self = Self {
        basic: [0.0; 4],
        shadows: [1.0; 3],
        highlights: [1.0; 3],
        duotone: ([0.0; 3], [1.0; 3], 0.0),
        wash: ([0.0; 3], 0.0),
        adjust: [0.0; 4],
    };

    fn basic(exposure: f32, contrast: f32, saturation: f32, temperature: f32) -> Self {
        Self {
            basic: [exposure, contrast, saturation, temperature],
            ..Self::NONE
        }
    }

    fn tinted(self, shadows: [f32; 3], highlights: [f32; 3]) -> Self {
        Self {
            shadows,
            highlights,
            ..self
        }
    }

    fn duotone(dark: [f32; 3], light: [f32; 3]) -> Self {
        Self {
            basic: [0.0, 0.15, 0.0, 0.0],
            duotone: (dark, light, 1.0),
            ..Self::NONE
        }
    }

    fn wash(colour: [f32; 3], amount: f32) -> Self {
        Self {
            wash: (colour, amount),
            ..Self::NONE
        }
    }
}

impl Filter {
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Retro => "Retro",
            Self::OrangeAndTeal => "Orange and teal",
            Self::BoldAndBlue => "Bold and blue",
            Self::GoldenHour => "Golden hour",
            Self::VibrantVlogger => "Vibrant vlogger",
            Self::PurpleUndertone => "Purple undertone",
            Self::WinterSunset => "Winter sunset",
            Self::Film35mm => "35mm",
            Self::Contrast => "Contrast",
            Self::Autumn => "Autumn",
            Self::Winter => "Winter",
            Self::OldWestern => "Old Western",
            Self::WarmCoastline => "Warm coastline",
            Self::CoolCoastline => "Cool coastline",
            Self::WarmCountryside => "Warm countryside",
            Self::CoolCountryside => "Cool countryside",
            Self::Golden => "Golden",
            Self::Dreamscape => "Dreamscape",
            Self::Sunrise => "Sunrise",
            Self::WarmToneFilm => "Warm tone film",
            Self::CoolTone => "Cool tone",
            Self::PastelDreams => "Pastel dreams",
            Self::Increased => "Increased",
            Self::Scenery => "Scenery",
            Self::Portrait => "Portrait",
            Self::Indoors => "Indoors",
            Self::Outdoors => "Outdoors",
            Self::Muted => "Muted",
            Self::BlackAndWhite1 => "Black & white 1",
            Self::BlackAndWhite2 => "Black & white 2",
            Self::SoftBlackAndWhite => "Soft B&W",
            Self::MutedBlackAndWhite => "Muted B&W",
            Self::Gloomy => "Gloomy",
            Self::DeepFried => "Deep fried",
            Self::Euphoric => "Euphoric",
            Self::YellowOrangeDuotone => "Yellow and orange duotone",
            Self::PinkPurpleDuotone => "Pink and purple duotone",
            Self::BluePinkDuotone => "Blue and pink duotone",
            Self::GreenBlueDuotone => "Green and blue duotone",
            Self::WhiteOverlay => "White overlay",
            Self::BlackOverlay => "Black overlay",
            Self::YellowOverlay => "Yellow overlay",
            Self::OrangeOverlay => "Orange overlay",
            Self::RedOverlay => "Red overlay",
            Self::PinkOverlay => "Pink overlay",
            Self::PurpleOverlay => "Purple overlay",
            Self::BlueOverlay => "Blue overlay",
            Self::TurquoiseOverlay => "Turquoise overlay",
            Self::GreenOverlay => "Green overlay",
        }
    }

    #[allow(clippy::too_many_lines)] // One line for each filter reads best as a single table.
    pub fn grade(self) -> Grade {
        let basic = Grade::basic;
        let warm_shadows = [1.05, 0.98, 0.9];
        let cool_shadows = [0.9, 0.98, 1.08];
        match self {
            Self::None => Grade::NONE,
            Self::Retro => {
                basic(0.05, -0.1, -0.2, 0.25).tinted([1.1, 1.0, 0.85], [1.02, 0.98, 0.85])
            }
            Self::OrangeAndTeal => {
                basic(0.0, 0.15, 0.15, 0.0).tinted([0.8, 1.0, 1.15], [1.15, 1.0, 0.82])
            }
            Self::BoldAndBlue => {
                basic(0.0, 0.2, 0.2, -0.4).tinted([0.9, 0.95, 1.2], [0.98, 1.0, 1.1])
            }
            Self::GoldenHour => {
                basic(0.08, 0.05, 0.1, 0.45).tinted(warm_shadows, [1.08, 1.0, 0.85])
            }
            Self::VibrantVlogger => basic(0.08, 0.12, 0.4, 0.05),
            Self::PurpleUndertone => {
                basic(0.0, 0.08, 0.05, -0.05).tinted([1.1, 0.85, 1.2], [1.05, 0.96, 1.05])
            }
            Self::WinterSunset => {
                basic(0.0, 0.08, 0.05, 0.1).tinted([0.9, 0.9, 1.2], [1.12, 0.98, 0.9])
            }
            Self::Film35mm => {
                basic(0.0, -0.12, -0.1, 0.08).tinted([1.05, 1.0, 0.95], [1.0, 0.98, 0.92])
            }
            Self::Contrast => basic(0.0, 0.4, 0.05, 0.0),
            Self::Autumn => {
                basic(0.0, 0.1, 0.05, 0.35).tinted([1.05, 0.95, 0.85], [1.08, 0.98, 0.82])
            }
            Self::Winter => basic(0.03, 0.08, -0.15, -0.45),
            Self::OldWestern => {
                basic(0.0, 0.05, -0.55, 0.5).tinted([1.05, 0.92, 0.78], [1.05, 0.98, 0.85])
            }
            Self::WarmCoastline => basic(0.05, 0.02, 0.05, 0.25),
            Self::CoolCoastline => basic(0.05, 0.02, 0.05, -0.28),
            Self::WarmCountryside => {
                basic(0.03, 0.05, 0.15, 0.2).tinted(warm_shadows, [1.03, 1.02, 0.92])
            }
            Self::CoolCountryside => {
                basic(0.03, 0.05, 0.12, -0.2).tinted(cool_shadows, [0.97, 1.02, 1.03])
            }
            Self::Golden => {
                basic(0.05, 0.05, 0.05, 0.6).tinted([1.05, 1.0, 0.82], [1.08, 1.02, 0.8])
            }
            Self::Dreamscape => {
                basic(0.1, -0.2, 0.15, -0.05).tinted([1.05, 0.92, 1.15], [1.03, 1.0, 1.05])
            }
            Self::Sunrise => {
                basic(0.1, 0.0, 0.12, 0.3).tinted([1.05, 0.92, 0.95], [1.1, 1.0, 0.86])
            }
            Self::WarmToneFilm => {
                basic(0.0, 0.1, 0.05, 0.3).tinted([1.1, 0.95, 0.85], [1.05, 0.98, 0.85])
            }
            Self::CoolTone => basic(0.0, 0.05, -0.05, -0.32),
            Self::PastelDreams => {
                basic(0.14, -0.3, -0.15, 0.05).tinted([1.12, 1.02, 1.12], [1.02, 1.0, 1.02])
            }
            Self::Increased => basic(0.05, 0.2, 0.25, 0.0),
            Self::Scenery => basic(0.0, 0.15, 0.3, -0.05),
            Self::Portrait => basic(0.06, -0.05, -0.05, 0.12),
            Self::Indoors => basic(0.1, 0.03, 0.05, -0.15),
            Self::Outdoors => basic(0.04, 0.1, 0.12, 0.08),
            Self::Muted => basic(0.0, -0.15, -0.4, 0.0),
            Self::BlackAndWhite1 => basic(0.0, 0.12, -1.0, 0.0),
            Self::BlackAndWhite2 => basic(0.0, 0.35, -1.0, 0.0),
            Self::SoftBlackAndWhite => basic(0.08, -0.2, -1.0, 0.0),
            Self::MutedBlackAndWhite => basic(0.0, -0.35, -1.0, 0.0),
            Self::Gloomy => basic(-0.3, 0.45, -1.0, 0.0),
            Self::DeepFried => basic(0.25, 0.9, 2.2, 0.35),
            Self::Euphoric => {
                basic(0.1, 0.45, 1.5, -0.3).tinted([0.9, 0.85, 1.3], [1.1, 0.95, 1.1])
            }
            Self::YellowOrangeDuotone => Grade::duotone([0.86, 0.18, 0.0], [1.0, 0.86, 0.1]),
            Self::PinkPurpleDuotone => Grade::duotone([0.3, 0.0, 0.75], [0.95, 0.1, 0.9]),
            Self::BluePinkDuotone => Grade::duotone([0.12, 0.1, 0.9], [1.0, 0.25, 0.9]),
            Self::GreenBlueDuotone => Grade::duotone([0.0, 0.2, 0.75], [0.0, 0.95, 0.65]),
            Self::WhiteOverlay => Grade::wash([1.0, 1.0, 1.0], 0.45),
            Self::BlackOverlay => Grade::wash([0.0, 0.0, 0.0], 0.55),
            Self::YellowOverlay => Grade::wash([0.95, 0.85, 0.05], 0.6),
            Self::OrangeOverlay => Grade::wash([0.95, 0.5, 0.05], 0.6),
            Self::RedOverlay => Grade::wash([0.95, 0.08, 0.08], 0.6),
            Self::PinkOverlay => Grade::wash([0.95, 0.15, 0.6], 0.6),
            Self::PurpleOverlay => Grade::wash([0.55, 0.12, 0.85], 0.6),
            Self::BlueOverlay => Grade::wash([0.1, 0.45, 0.95], 0.6),
            Self::TurquoiseOverlay => Grade::wash([0.1, 0.85, 0.8], 0.6),
            Self::GreenOverlay => Grade::wash([0.1, 0.85, 0.3], 0.6),
        }
    }
}

/// What a clip's picture looks like, beyond where it sits. It belongs to the whole clip.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipLook {
    pub filter: Filter,
    /// Each from `-ADJUST_RANGE` to `ADJUST_RANGE`, with 0 for no change.
    pub exposure: i32,
    pub contrast: i32,
    pub saturation: i32,
    pub temperature: i32,
    /// How long the picture takes to appear, in ticks. Sound has fades of its own.
    pub fade_in: i64,
    /// How long the picture takes to go, in ticks.
    pub fade_out: i64,
}

impl ClipLook {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Filter and adjustments only, leaving fades out.
    pub fn changes_colour(&self) -> bool {
        self.filter != Filter::None
            || self.exposure != 0
            || self.contrast != 0
            || self.saturation != 0
            || self.temperature != 0
    }

    pub fn has_fade(&self) -> bool {
        self.fade_in > 0 || self.fade_out > 0
    }

    /// What to do to the picture's colours.
    pub fn grade(&self) -> Grade {
        if !self.changes_colour() {
            return Grade::NONE;
        }
        let part = |value: i32| value as f32 / ADJUST_RANGE as f32;
        Grade {
            adjust: [
                part(self.exposure),
                part(self.contrast),
                part(self.saturation),
                part(self.temperature),
            ],
            ..self.filter.grade()
        }
    }

    /// How much the fades let through, `into` a clip that lasts `length`. They reach at most half
    /// the clip each, so both can be used at once.
    pub fn fade_at(&self, into: Ticks, length: Ticks) -> f32 {
        let limit = length.0 / 2;
        let share = |fade: i64, away: i64| {
            let fade = fade.min(limit);
            if fade <= 0 {
                1.0
            } else {
                ((away as f64 / fade as f64) as f32).clamp(0.0, 1.0)
            }
        };
        share(self.fade_in, into.0) * share(self.fade_out, length.0 - into.0)
    }

    /// The same look with anything out of range brought back in, in case a file was edited by hand.
    pub fn checked(self) -> Self {
        let adjust = |value: i32| value.clamp(-ADJUST_RANGE, ADJUST_RANGE);
        Self {
            exposure: adjust(self.exposure),
            contrast: adjust(self.contrast),
            saturation: adjust(self.saturation),
            temperature: adjust(self.temperature),
            fade_in: self.fade_in.max(0),
            fade_out: self.fade_out.max(0),
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_transition_is_listed_once_and_there_are_44() {
        let listed: Vec<TransitionKind> = TRANSITION_GROUPS
            .iter()
            .flat_map(|(_, kinds)| kinds.iter().copied())
            .collect();
        assert_eq!(listed.len(), 44);
        for kind in &listed {
            assert_eq!(listed.iter().filter(|other| *other == kind).count(), 1);
        }
    }

    #[test]
    fn transitions_are_listed_in_the_order_the_shader_numbers_them() {
        let listed = TRANSITION_GROUPS.iter().flat_map(|(_, kinds)| kinds.iter());
        for (index, kind) in listed.enumerate() {
            assert_eq!(kind.code() as usize, index, "{kind:?}");
        }
    }

    #[test]
    fn filters_all_have_their_own_names_and_change_something() {
        for filter in FILTERS {
            assert_ne!(filter, Filter::None);
            assert_ne!(filter.grade(), Grade::NONE, "{filter:?}");
            assert_eq!(
                FILTERS.iter().filter(|f| f.name() == filter.name()).count(),
                1,
                "{filter:?}"
            );
        }
    }

    #[test]
    fn a_new_look_changes_nothing() {
        let look = ClipLook::default();
        assert!(look.is_default());
        assert!(!look.changes_colour());
        assert_eq!(look.grade(), Grade::NONE);
        assert_eq!(look.fade_at(Ticks(0), Ticks(100)), 1.0);
    }

    #[test]
    fn fades_rise_at_the_start_and_fall_at_the_end() {
        let second = Ticks::from_seconds(1.0).0;
        let look = ClipLook {
            fade_in: second,
            fade_out: second,
            ..ClipLook::default()
        };
        let length = Ticks(second * 10);
        assert_eq!(look.fade_at(Ticks(0), length), 0.0);
        assert!((look.fade_at(Ticks(second / 2), length) - 0.5).abs() < 0.01);
        assert_eq!(look.fade_at(Ticks(second * 5), length), 1.0);
        assert!((look.fade_at(Ticks(second * 19 / 2), length) - 0.5).abs() < 0.01);
        assert_eq!(look.fade_at(length, length), 0.0);
    }

    #[test]
    fn a_fade_longer_than_half_the_clip_stops_at_half() {
        let second = Ticks::from_seconds(1.0).0;
        let look = ClipLook {
            fade_in: second * 100,
            ..ClipLook::default()
        };
        assert_eq!(look.fade_at(Ticks(second), Ticks(second * 2)), 1.0);
    }

    #[test]
    fn adjustments_are_carried_along_with_the_filter() {
        let look = ClipLook {
            filter: Filter::Winter,
            exposure: 500,
            ..ClipLook::default()
        };
        let grade = look.grade();
        assert_eq!(grade.adjust[0], 0.5);
        assert_eq!(grade.basic, Filter::Winter.grade().basic);
    }

    #[test]
    fn progress_runs_from_half_before_the_cut_to_half_after() {
        let cut = Ticks(1000);
        assert_eq!(join_progress(cut, 200, Ticks(900)), 0.0);
        assert_eq!(join_progress(cut, 200, cut), 0.5);
        assert_eq!(join_progress(cut, 200, Ticks(1100)), 1.0);
    }

    #[test]
    fn a_look_read_from_a_file_comes_back_in_range() {
        let look = ClipLook {
            exposure: 99_999,
            fade_in: -5,
            ..ClipLook::default()
        }
        .checked();
        assert_eq!(look.exposure, ADJUST_RANGE);
        assert_eq!(look.fade_in, 0);
    }
}
