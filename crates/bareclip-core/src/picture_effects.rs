//! The effects a clip's picture can have, several at once, each with its own settings. Their
//! motion was measured from a sample recording of
//! each one playing.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::time::Ticks;

/// A slider runs from 0 to this.
pub const SLIDER_TOP: i32 = 1000;
/// The most controls any one effect has.
pub const MOST_CONTROLS: usize = 4;
/// The spots on a 3x3 grid, read across and then down, so 4 is the middle.
pub const GRID_SPOTS: i32 = 9;

/// One of the effects, in the order of the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EffectKind {
    Flash,
    Pulse,
    Spin,
    Vhs,
    Rotate,
    Vaporwave,
    ChromaticAberration,
    CrashZoom,
    SlowZoom,
    SlowZoomRandom,
    GreenScreen,
    BlackWhiteRemoval,
    Blur,
    BlurFill,
    Filmic,
    Glitch,
    Disco,
    ColourShift,
    Glass,
    Comic,
    RetroGraphics,
    Vertical,
    Radial,
    Smoke,
    Kaleidoscope,
    Glow,
    Diffusion,
}

/// Every effect, in the panel's order. `None` isn't one of them.
pub const EFFECTS: [EffectKind; 27] = {
    use EffectKind as E;
    [
        E::Flash,
        E::Pulse,
        E::Spin,
        E::Vhs,
        E::Rotate,
        E::Vaporwave,
        E::ChromaticAberration,
        E::CrashZoom,
        E::SlowZoom,
        E::SlowZoomRandom,
        E::GreenScreen,
        E::BlackWhiteRemoval,
        E::Blur,
        E::BlurFill,
        E::Filmic,
        E::Glitch,
        E::Disco,
        E::ColourShift,
        E::Glass,
        E::Comic,
        E::RetroGraphics,
        E::Vertical,
        E::Radial,
        E::Smoke,
        E::Kaleidoscope,
        E::Glow,
        E::Diffusion,
    ]
};

/// One of an effect's settings, with where it starts. Starting points are read off reference
/// screenshots, which show each effect as it is first turned on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    /// From 0 to `SLIDER_TOP`.
    Slider { name: &'static str, start: i32 },
    /// A tick box.
    Switch { name: &'static str, start: bool },
    /// A list to choose from, starting on one of them.
    Choice {
        name: &'static str,
        options: &'static [&'static str],
        start: i32,
    },
    /// The 3x3 grid of where a zoom aims.
    Grid { name: &'static str, start: i32 },
}

impl Control {
    pub fn name(self) -> &'static str {
        match self {
            Self::Slider { name, .. }
            | Self::Switch { name, .. }
            | Self::Choice { name, .. }
            | Self::Grid { name, .. } => name,
        }
    }

    pub fn start(self) -> i32 {
        match self {
            Self::Slider { start, .. } | Self::Choice { start, .. } | Self::Grid { start, .. } => {
                start
            }
            Self::Switch { start, .. } => i32::from(start),
        }
    }

    /// The same value, brought into range.
    pub fn checked(self, value: i32) -> i32 {
        match self {
            Self::Slider { .. } => value.clamp(0, SLIDER_TOP),
            Self::Switch { .. } => value.clamp(0, 1),
            Self::Choice { options, .. } => value.clamp(0, options.len() as i32 - 1),
            Self::Grid { .. } => value.clamp(0, GRID_SPOTS - 1),
        }
    }
}

const MIDDLE: i32 = 4;
/// How many times Crash zoom jumps in. Only the first two are measured, so how
/// far the list goes is BareClip's.
pub const CRASH_ZOOM_STEPS: [&str; 5] = ["1", "2", "3", "4", "5"];
pub const SCREEN_COLOURS: [&str; 3] = ["Red", "Green", "Blue"];
pub const REMOVAL_COLOURS: [&str; 2] = ["Black", "White"];
pub const RETRO_PALETTES: [&str; 7] = [
    "Original",
    "1-bit",
    "4-bit",
    "8-bit",
    "16-bit",
    "Paint 1995",
    "Retro Handheld",
];

impl EffectKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Flash => "Flash",
            Self::Pulse => "Pulse",
            Self::Spin => "Spin",
            Self::Vhs => "VHS",
            Self::Rotate => "Rotate",
            Self::Vaporwave => "Vaporwave",
            Self::ChromaticAberration => "Chromatic aberration",
            Self::CrashZoom => "Crash zoom",
            Self::SlowZoom => "Slow zoom",
            Self::SlowZoomRandom => "Slow zoom random",
            Self::GreenScreen => "Green screen",
            Self::BlackWhiteRemoval => "Black/white removal",
            Self::Blur => "Blur",
            Self::BlurFill => "Blur fill",
            Self::Filmic => "Filmic",
            Self::Glitch => "Glitch",
            Self::Disco => "Disco",
            Self::ColourShift => "Colour shift",
            Self::Glass => "Glass",
            Self::Comic => "Comic",
            Self::RetroGraphics => "Retro graphics",
            Self::Vertical => "Vertical",
            Self::Radial => "Radial",
            Self::Smoke => "Smoke",
            Self::Kaleidoscope => "Kaleidoscope",
            Self::Glow => "Glow",
            Self::Diffusion => "Diffusion",
        }
    }

    /// Its place in `EFFECTS`, which is also the number the shader knows it by.
    pub fn index(self) -> usize {
        self as usize
    }

    /// Its settings, in the order the panel shows them.
    pub fn controls(self) -> &'static [Control] {
        match self {
            Self::Flash => &[
                Control::Slider {
                    name: "Speed",
                    start: 230,
                },
                Control::Switch {
                    name: "Fade",
                    start: false,
                },
            ],
            Self::Pulse => &[
                Control::Slider {
                    name: "Speed",
                    start: 330,
                },
                Control::Slider {
                    name: "Amount",
                    start: 290,
                },
                Control::Switch {
                    name: "Loop",
                    start: true,
                },
            ],
            Self::Spin => &[
                Control::Switch {
                    name: "Bounce",
                    start: true,
                },
                Control::Switch {
                    name: "Loop",
                    start: false,
                },
                Control::Switch {
                    name: "Reverse",
                    start: false,
                },
            ],
            Self::Vhs => &[
                Control::Slider {
                    name: "Grain",
                    start: 780,
                },
                Control::Slider {
                    name: "Intensity",
                    start: 500,
                },
            ],
            Self::Rotate => &[
                Control::Slider {
                    name: "Speed",
                    start: 0,
                },
                Control::Switch {
                    name: "Reverse",
                    start: false,
                },
            ],
            Self::Vaporwave => &[
                Control::Slider {
                    name: "Effect intensity",
                    start: 290,
                },
                Control::Slider {
                    name: "Grain",
                    start: 500,
                },
                Control::Slider {
                    name: "Intensity",
                    start: 500,
                },
            ],
            Self::ChromaticAberration => &[Control::Slider {
                name: "Effect intensity",
                start: 200,
            }],
            Self::CrashZoom => &[
                Control::Grid {
                    name: "Position",
                    start: MIDDLE,
                },
                Control::Slider {
                    name: "Zoom",
                    start: 400,
                },
                Control::Choice {
                    name: "Steps",
                    options: &CRASH_ZOOM_STEPS,
                    start: 0,
                },
                Control::Slider {
                    name: "Step delay",
                    start: 0,
                },
            ],
            Self::SlowZoom => &[
                Control::Grid {
                    name: "Position",
                    start: MIDDLE,
                },
                Control::Slider {
                    name: "Speed",
                    start: 0,
                },
                Control::Switch {
                    name: "Reverse",
                    start: false,
                },
            ],
            Self::SlowZoomRandom => &[
                Control::Slider {
                    name: "Speed",
                    start: 0,
                },
                Control::Switch {
                    name: "Reverse",
                    start: false,
                },
            ],
            Self::GreenScreen => &[
                Control::Slider {
                    name: "Screen threshold",
                    start: 290,
                },
                Control::Choice {
                    name: "Screen colour",
                    options: &SCREEN_COLOURS,
                    start: 1,
                },
            ],
            Self::BlackWhiteRemoval => &[
                Control::Slider {
                    name: "Colour threshold",
                    start: 290,
                },
                Control::Choice {
                    name: "Removal colour",
                    options: &REMOVAL_COLOURS,
                    start: 0,
                },
            ],
            Self::Blur => &[Control::Slider {
                name: "Blur",
                start: 110,
            }],
            Self::Filmic => &[
                Control::Slider {
                    name: "Grain",
                    start: 500,
                },
                Control::Slider {
                    name: "Intensity",
                    start: 500,
                },
            ],
            Self::Glitch => &[
                Control::Slider {
                    name: "Glitch intensity",
                    start: 200,
                },
                Control::Slider {
                    name: "Glitch speed",
                    start: 200,
                },
            ],
            Self::Comic => &[Control::Slider {
                name: "Halftone dot opacity",
                start: 290,
            }],
            Self::RetroGraphics => &[
                Control::Slider {
                    name: "Pixelation",
                    start: 420,
                },
                Control::Choice {
                    name: "Colour palette",
                    options: &RETRO_PALETTES,
                    start: 4,
                },
            ],
            Self::Kaleidoscope => &[
                Control::Slider {
                    name: "Speed",
                    start: 210,
                },
                Control::Slider {
                    name: "Segments",
                    start: 190,
                },
                Control::Switch {
                    name: "Colour shift",
                    start: false,
                },
            ],
            Self::Glow => &[Control::Slider {
                name: "Strength",
                start: 610,
            }],
            Self::Diffusion => &[Control::Slider {
                name: "Strength",
                start: 720,
            }],
            Self::BlurFill
            | Self::Disco
            | Self::ColourShift
            | Self::Glass
            | Self::Vertical
            | Self::Radial
            | Self::Smoke => &[],
        }
    }

    /// Whether it looks different from one moment to the next, even on a still picture.
    pub fn moves(self) -> bool {
        !matches!(
            self,
            Self::ChromaticAberration
                | Self::GreenScreen
                | Self::BlackWhiteRemoval
                | Self::Blur
                | Self::BlurFill
                | Self::Comic
                | Self::RetroGraphics
                | Self::Glow
                | Self::Diffusion
        )
    }

    /// Whether what is under the clip can show through it, around a turned or tiled picture or
    /// where a colour is taken out.
    pub fn may_show_through(self) -> bool {
        matches!(
            self,
            Self::Spin
                | Self::Rotate
                | Self::GreenScreen
                | Self::BlackWhiteRemoval
                | Self::Vertical
                | Self::Radial
                | Self::Kaleidoscope
        )
    }

    /// Its settings as they start.
    pub fn starting_values(self) -> [i32; MOST_CONTROLS] {
        let mut values = [0; MOST_CONTROLS];
        for (value, control) in values.iter_mut().zip(self.controls()) {
            *value = control.start();
        }
        values
    }
}

/// The effects a clip has on, with their settings. An effect that is off keeps its starting
/// settings, so turning it on again starts afresh, and saving and loading change nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Effects {
    on: u32,
    values: [[i32; MOST_CONTROLS]; EFFECTS.len()],
}

impl Default for Effects {
    fn default() -> Self {
        let mut values = [[0; MOST_CONTROLS]; EFFECTS.len()];
        for kind in EFFECTS {
            values[kind.index()] = kind.starting_values();
        }
        Self { on: 0, values }
    }
}

impl Effects {
    pub fn is_on(&self, kind: EffectKind) -> bool {
        self.on & (1 << kind.index()) != 0
    }

    pub fn any(&self) -> bool {
        self.on != 0
    }

    /// Whether the picture changes over time because of an effect.
    pub fn move_over_time(&self) -> bool {
        self.list().any(EffectKind::moves)
    }

    /// Whether what is under the clip might show through it because of an effect.
    pub fn may_show_through(&self) -> bool {
        self.list().any(EffectKind::may_show_through)
    }

    /// Those that are on, in the panel's order.
    pub fn list(&self) -> impl Iterator<Item = EffectKind> + '_ {
        EFFECTS.into_iter().filter(|kind| self.is_on(*kind))
    }

    /// Turns one on or off. Off puts its settings back to how they start.
    pub fn set(&mut self, kind: EffectKind, on: bool) {
        if on {
            self.on |= 1 << kind.index();
        } else {
            self.on &= !(1 << kind.index());
            self.values[kind.index()] = kind.starting_values();
        }
    }

    pub fn value(&self, kind: EffectKind, control: usize) -> i32 {
        self.values[kind.index()][control]
    }

    /// One setting's value, as a share of its whole range, from 0 to 1.
    pub fn share(&self, kind: EffectKind, control: usize) -> f32 {
        let value = self.value(kind, control);
        match kind.controls().get(control) {
            Some(Control::Slider { .. }) => value as f32 / SLIDER_TOP as f32,
            _ => value as f32,
        }
    }

    pub fn set_value(&mut self, kind: EffectKind, control: usize, value: i32) {
        if let Some(found) = kind.controls().get(control) {
            self.values[kind.index()][control] = found.checked(value);
        }
    }
}

/// What drawing a clip's effects needs at one moment: which are on, their settings as shares of
/// each range, and how far into the clip it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EffectsAt {
    pub on: u32,
    pub settings: [[f32; MOST_CONTROLS]; EFFECTS.len()],
    pub seconds: f32,
    /// How long the clip lasts, in seconds.
    pub length: f32,
    /// How far the effects turn the picture, clockwise in degrees, around its middle.
    pub turn: f32,
    /// How far they zoom into it, 1 being not at all, keeping the place it is drawn in.
    pub zoom: f32,
    /// The spot on the picture the zoom heads for, from 0 to 1 each way.
    pub toward: [f32; 2],
}

impl EffectsAt {
    pub const NONE: Self = Self {
        on: 0,
        settings: [[0.0; MOST_CONTROLS]; EFFECTS.len()],
        seconds: 0.0,
        length: 0.0,
        turn: 0.0,
        zoom: 1.0,
        toward: [0.5, 0.5],
    };

    pub fn is_on(&self, kind: EffectKind) -> bool {
        self.on & (1 << kind.index()) != 0
    }

    pub fn setting(&self, kind: EffectKind, control: usize) -> f32 {
        self.settings[kind.index()][control]
    }
}

/// Where on the picture a spot of the 3x3 grid is: a corner, an edge's middle or the middle.
pub fn grid_point(spot: i32) -> [f32; 2] {
    let spot = spot.clamp(0, GRID_SPOTS - 1);
    [(spot % 3) as f32 / 2.0, (spot / 3) as f32 / 2.0]
}

/// From 0 to 1, easing at both ends.
fn ease(share: f32) -> f32 {
    let share = share.clamp(0.0, 1.0);
    share * share * (3.0 - 2.0 * share)
}

/// Spin with Bounce, as measured from a sample recording a fifteenth of a second apart: how far
/// round it is, from 0 to 1, by seconds in. It winds back 17 degrees, goes round faster and faster,
/// overshoots by 17 degrees and settles.
const SPIN_WITH_BOUNCE: [(f32, f32); 43] = [
    (0.00, 0.0000),
    (0.07, -0.0158),
    (0.13, -0.0297),
    (0.20, -0.0406),
    (0.27, -0.0469),
    (0.33, -0.0481),
    (0.40, -0.0444),
    (0.47, -0.0347),
    (0.53, -0.0167),
    (0.60, 0.0114),
    (0.67, 0.0528),
    (0.73, 0.1092),
    (0.80, 0.1822),
    (0.87, 0.2736),
    (0.93, 0.3836),
    (1.00, 0.5100),
    (1.07, 0.6486),
    (1.13, 0.7922),
    (1.20, 0.9283),
    (1.27, 1.0231),
    (1.33, 1.0442),
    (1.40, 1.0469),
    (1.47, 1.0367),
    (1.53, 1.0200),
    (1.60, 1.0039),
    (1.67, 0.9919),
    (1.73, 0.9861),
    (1.80, 0.9858),
    (1.87, 0.9892),
    (1.93, 0.9944),
    (2.00, 0.9994),
    (2.07, 1.0028),
    (2.13, 1.0044),
    (2.20, 1.0042),
    (2.27, 1.0031),
    (2.33, 1.0017),
    (2.40, 1.0000),
    (2.47, 0.9992),
    (2.53, 0.9986),
    (2.60, 0.9986),
    (2.67, 0.9992),
    (2.73, 0.9997),
    (2.80, 1.0000),
];

/// How far through its one turn Spin is, from 0 to 1, at `seconds` into a go.
fn spin_share(seconds: f32, bounce: bool) -> f32 {
    if !bounce {
        return ease(seconds / 1.07);
    }
    let after = SPIN_WITH_BOUNCE.partition_point(|(time, _)| *time <= seconds);
    match (
        SPIN_WITH_BOUNCE.get(after.wrapping_sub(1)),
        SPIN_WITH_BOUNCE.get(after),
    ) {
        (Some(&(from, was)), Some(&(to, next))) => {
            was + (next - was) * (seconds - from) / (to - from)
        }
        (Some(_), None) => 1.0,
        _ => 0.0,
    }
}

/// How the effects turn and zoom the picture, measured from a sample recording at the starting
/// settings and scaled from there.
fn motion(effects: &Effects, seconds: f32, length: f32, seed: i64) -> (f32, f32, [f32; 2]) {
    let mut turn = 0.0;
    let mut zoom = 1.0;
    let mut toward = [0.5, 0.5];
    let share = |kind: EffectKind, control: usize| effects.share(kind, control);
    let ticked = |kind: EffectKind, control: usize| effects.value(kind, control) != 0;
    if effects.is_on(EffectKind::Pulse) {
        // In to 115% and back once a second, at the starting settings.
        let rate = 2.0_f32.powf(3.0 * (share(EffectKind::Pulse, 0) - 0.33));
        let peak = 0.15 * share(EffectKind::Pulse, 1) / 0.29;
        let beats = seconds * rate;
        if ticked(EffectKind::Pulse, 2) || beats < 1.0 {
            zoom *= 1.0 + peak / 2.0 * (1.0 - (std::f32::consts::TAU * beats).cos());
        }
    }
    if effects.is_on(EffectKind::Spin) {
        let bounce = ticked(EffectKind::Spin, 0);
        // A go lasts long enough to settle before Loop starts the next.
        let go = if bounce { 3.0 } else { 1.5 };
        let into = if ticked(EffectKind::Spin, 1) {
            seconds % go
        } else {
            seconds
        };
        let way = if ticked(EffectKind::Spin, 2) {
            1.0
        } else {
            -1.0
        };
        turn += way * 360.0 * spin_share(into, bounce);
    }
    if effects.is_on(EffectKind::Rotate) {
        // 54 degrees a second anticlockwise at the slowest, which is where it starts.
        let rate = 54.0 * (1.0 + 9.0 * share(EffectKind::Rotate, 0));
        let way = if ticked(EffectKind::Rotate, 1) {
            1.0
        } else {
            -1.0
        };
        turn += way * rate * seconds;
    }
    if effects.is_on(EffectKind::CrashZoom) {
        // Still until 0.2 seconds, then in to 3 times over 0.3 seconds, at the starting settings.
        // More steps share the way in, with the delay between each.
        let most = 1.0 + 5.0 * share(EffectKind::CrashZoom, 1);
        let steps = effects.value(EffectKind::CrashZoom, 2) + 1;
        let delay = 2.0 * share(EffectKind::CrashZoom, 3);
        let mut reached = 0.0;
        for step in 0..steps {
            let begins = 0.2 + step as f32 * (0.3 + delay);
            reached += ease((seconds - begins) / 0.3) / steps as f32;
        }
        zoom *= 1.0 + (most - 1.0) * reached;
        toward = grid_point(effects.value(EffectKind::CrashZoom, 0));
    }
    let slowly = |speed: f32, reverse: bool| {
        // 5% a second at the slowest, which is where it starts.
        let rate = 0.05 * (1.0 + 9.0 * speed);
        let into = if reverse { length - seconds } else { seconds };
        1.0 + rate * into.max(0.0)
    };
    if effects.is_on(EffectKind::SlowZoom) {
        zoom *= slowly(
            share(EffectKind::SlowZoom, 1),
            ticked(EffectKind::SlowZoom, 2),
        );
        toward = grid_point(effects.value(EffectKind::SlowZoom, 0));
    }
    if effects.is_on(EffectKind::SlowZoomRandom) {
        zoom *= slowly(
            share(EffectKind::SlowZoomRandom, 0),
            ticked(EffectKind::SlowZoomRandom, 1),
        );
        // A spot somewhere on the picture that stays the same for the same footage.
        let mixed = (seed as u64)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .rotate_left(29);
        let part = |shift: u32| ((mixed >> shift) & 0xFFFF) as f32 / 65535.0;
        toward = [0.15 + 0.7 * part(0), 0.15 + 0.7 * part(16)];
    }
    (turn, zoom, toward)
}

impl Effects {
    /// The effects `into` a clip that lasts `length`. With none on, this is always the same, so a
    /// plain clip still counts as plain. `seed` makes anything random the same each time for the
    /// same clip, such as where Slow zoom random heads.
    pub fn at(&self, into: Ticks, length: Ticks, seed: i64) -> EffectsAt {
        if !self.any() {
            return EffectsAt::NONE;
        }
        let mut settings = [[0.0; MOST_CONTROLS]; EFFECTS.len()];
        for kind in self.list() {
            for (control, setting) in settings[kind.index()].iter_mut().enumerate() {
                *setting = self.share(kind, control);
            }
        }
        let seconds = into.as_seconds() as f32;
        let length = length.as_seconds() as f32;
        let (turn, zoom, toward) = motion(self, seconds, length, seed);
        EffectsAt {
            on: self.on,
            settings,
            seconds,
            length,
            turn,
            zoom,
            toward,
        }
    }

    /// Whether an effect zooms into the picture, so the preview needs it at full size to stay
    /// sharp.
    pub fn zooms_in(&self) -> bool {
        [
            EffectKind::Pulse,
            EffectKind::CrashZoom,
            EffectKind::SlowZoom,
            EffectKind::SlowZoomRandom,
        ]
        .into_iter()
        .any(|kind| self.is_on(kind))
    }
}

/// One effect as a project file holds it, with its settings by name so a file stays readable and
/// a setting added later can't be confused with another.
#[derive(Serialize, Deserialize)]
struct SavedEffect {
    effect: EffectKind,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    settings: BTreeMap<String, i32>,
}

impl Serialize for Effects {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let saved: Vec<SavedEffect> = self
            .list()
            .map(|kind| SavedEffect {
                effect: kind,
                settings: kind
                    .controls()
                    .iter()
                    .enumerate()
                    .map(|(index, control)| (control.name().to_owned(), self.value(kind, index)))
                    .collect(),
            })
            .collect();
        saved.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Effects {
    /// Anything missing starts as it would, and anything out of range is brought back in, in case
    /// a file was edited by hand.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let saved = Vec::<SavedEffect>::deserialize(deserializer)?;
        let mut effects = Self::default();
        for one in saved {
            effects.set(one.effect, true);
            for (index, control) in one.effect.controls().iter().enumerate() {
                if let Some(value) = one.settings.get(control.name()) {
                    effects.set_value(one.effect, index, *value);
                }
            }
        }
        Ok(effects)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_are_27_effects_each_with_its_own_name_in_the_order_they_are_numbered() {
        assert_eq!(EFFECTS.len(), 27);
        for (index, kind) in EFFECTS.into_iter().enumerate() {
            assert_eq!(kind.index(), index);
            assert_eq!(
                EFFECTS.iter().filter(|k| k.name() == kind.name()).count(),
                1
            );
        }
    }

    #[test]
    fn no_effect_has_more_controls_than_there_is_room_for() {
        for kind in EFFECTS {
            assert!(kind.controls().len() <= MOST_CONTROLS, "{kind:?}");
            for control in kind.controls() {
                assert_eq!(
                    control.checked(control.start()),
                    control.start(),
                    "{kind:?}"
                );
            }
        }
    }

    #[test]
    fn settings_start_where_the_screenshots_show_them() {
        let effects = Effects::default();
        assert_eq!(effects.value(EffectKind::CrashZoom, 0), 4);
        assert_eq!(effects.value(EffectKind::CrashZoom, 2), 0);
        assert_eq!(effects.value(EffectKind::Pulse, 2), 1);
        assert_eq!(effects.value(EffectKind::GreenScreen, 1), 1);
        assert_eq!(effects.value(EffectKind::RetroGraphics, 1), 4);
    }

    #[test]
    fn several_can_be_on_at_once_and_list_in_the_panels_order() {
        let mut effects = Effects::default();
        assert!(!effects.any());
        effects.set(EffectKind::Glow, true);
        effects.set(EffectKind::Flash, true);
        assert!(effects.any());
        let listed: Vec<EffectKind> = effects.list().collect();
        assert_eq!(listed, [EffectKind::Flash, EffectKind::Glow]);
    }

    #[test]
    fn turning_one_off_puts_its_settings_back() {
        let mut effects = Effects::default();
        effects.set(EffectKind::Blur, true);
        effects.set_value(EffectKind::Blur, 0, 900);
        effects.set(EffectKind::Blur, false);
        assert_eq!(effects, Effects::default());
    }

    #[test]
    fn settings_out_of_range_are_brought_back_in() {
        let mut effects = Effects::default();
        effects.set_value(EffectKind::Blur, 0, 5000);
        effects.set_value(EffectKind::CrashZoom, 0, 12);
        effects.set_value(EffectKind::Flash, 1, -3);
        assert_eq!(effects.value(EffectKind::Blur, 0), SLIDER_TOP);
        assert_eq!(effects.value(EffectKind::CrashZoom, 0), 8);
        assert_eq!(effects.value(EffectKind::Flash, 1), 0);
    }

    #[test]
    fn effects_save_by_name_and_come_back_the_same() {
        let mut effects = Effects::default();
        effects.set(EffectKind::CrashZoom, true);
        effects.set_value(EffectKind::CrashZoom, 0, 2);
        effects.set(EffectKind::Vhs, true);
        let text = serde_json::to_string(&effects).expect("it saves");
        assert!(text.contains("\"Position\":2"), "{text}");
        let back: Effects = serde_json::from_str(&text).expect("it loads");
        assert_eq!(back, effects);
    }

    fn at(kind: EffectKind, seconds: f64, change: impl Fn(&mut Effects)) -> EffectsAt {
        let mut effects = Effects::default();
        effects.set(kind, true);
        change(&mut effects);
        effects.at(Ticks::from_seconds(seconds), Ticks::from_seconds(5.0), 0)
    }

    #[test]
    fn pulse_zooms_in_to_115_percent_and_out_once_a_second() {
        let close = |a: f32, b: f32| (a - b).abs() < 0.01;
        assert!(close(at(EffectKind::Pulse, 0.0, |_| {}).zoom, 1.0));
        assert!(close(at(EffectKind::Pulse, 0.5, |_| {}).zoom, 1.15));
        assert!(close(at(EffectKind::Pulse, 1.0, |_| {}).zoom, 1.0));
        assert!(close(at(EffectKind::Pulse, 2.5, |_| {}).zoom, 1.15));
        let once = at(EffectKind::Pulse, 2.5, |effects| {
            effects.set_value(EffectKind::Pulse, 2, 0);
        });
        assert!(close(once.zoom, 1.0), "without Loop it pulses once");
    }

    #[test]
    fn spin_winds_back_turns_once_anticlockwise_overshoots_and_settles() {
        assert!((at(EffectKind::Spin, 0.33, |_| {}).turn - 17.0).abs() < 0.5);
        let past = at(EffectKind::Spin, 1.4, |_| {}).turn;
        assert!((past + 377.0).abs() < 0.5, "{past}");
        let settled = at(EffectKind::Spin, 3.5, |_| {}).turn;
        assert!((settled + 360.0).abs() < 0.5, "{settled}");
        let reversed = at(EffectKind::Spin, 3.5, |effects| {
            effects.set_value(EffectKind::Spin, 2, 1);
        });
        assert!((reversed.turn - 360.0).abs() < 0.5);
    }

    #[test]
    fn rotate_turns_54_degrees_a_second_anticlockwise_at_the_start() {
        assert!((at(EffectKind::Rotate, 2.0, |_| {}).turn + 108.0).abs() < 0.01);
        let reversed = at(EffectKind::Rotate, 2.0, |effects| {
            effects.set_value(EffectKind::Rotate, 1, 1);
        });
        assert!((reversed.turn - 108.0).abs() < 0.01);
    }

    #[test]
    fn crash_zoom_jumps_to_three_times_between_a_fifth_and_half_a_second() {
        assert_eq!(at(EffectKind::CrashZoom, 0.1, |_| {}).zoom, 1.0);
        let done = at(EffectKind::CrashZoom, 0.6, |_| {});
        assert!((done.zoom - 3.0).abs() < 0.01, "{}", done.zoom);
        assert_eq!(done.toward, [0.5, 0.5]);
        let corner = at(EffectKind::CrashZoom, 0.6, |effects| {
            effects.set_value(EffectKind::CrashZoom, 0, 0);
        });
        assert_eq!(corner.toward, [0.0, 0.0]);
        // With two steps and no delay, the second starts as the first ends.
        let two = at(EffectKind::CrashZoom, 0.5, |effects| {
            effects.set_value(EffectKind::CrashZoom, 2, 1);
        });
        assert!(
            (two.zoom - 2.0).abs() < 0.01,
            "half way after one of two steps"
        );
    }

    #[test]
    fn slow_zoom_grows_five_percent_a_second_and_reverse_runs_backwards() {
        let close = |a: f32, b: f32| (a - b).abs() < 0.001;
        assert!(close(at(EffectKind::SlowZoom, 4.0, |_| {}).zoom, 1.2));
        let back = at(EffectKind::SlowZoom, 4.0, |effects| {
            effects.set_value(EffectKind::SlowZoom, 2, 1);
        });
        assert!(close(back.zoom, 1.05));
    }

    #[test]
    fn slow_zoom_random_heads_for_the_same_spot_for_the_same_footage() {
        let mut effects = Effects::default();
        effects.set(EffectKind::SlowZoomRandom, true);
        let one = effects.at(Ticks(0), Ticks(1), 12345).toward;
        assert_eq!(one, effects.at(Ticks(0), Ticks(1), 12345).toward);
        assert_ne!(one, effects.at(Ticks(0), Ticks(1), 999).toward);
    }

    #[test]
    fn a_hand_edited_file_with_odd_settings_still_loads() {
        let text =
            r#"[{"effect":"Blur","settings":{"Blur":99999,"Nonsense":3}},{"effect":"Smoke"}]"#;
        let effects: Effects = serde_json::from_str(text).expect("it loads");
        assert!(effects.is_on(EffectKind::Blur) && effects.is_on(EffectKind::Smoke));
        assert_eq!(effects.value(EffectKind::Blur, 0), SLIDER_TOP);
    }
}
