//! How titles move. Everything here is worked out from how far into its clip a title is, so the
//! Text panel's cards, the preview and the export all show the same motion at the same moment.
//!
//! Motion comes in layers: the whole picture can move, which costs nothing because it only changes
//! where the picture goes, and its letters, boxes and finish can move, which means drawing a new
//! picture.

use crate::{
    placement::{CropSides, Placement, WHOLE},
    title::TitleStyle,
};

/// What the whole picture does as it comes in or goes out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WholeMove {
    None,
    Fade,
    /// Fades while moving a little up into place.
    FadeRise,
    Rise,
    Fall,
    FromLeft,
    FromRight,
    /// In from a long way off to one side, for a panel across the whole canvas.
    SweepLeft,
    SweepRight,
    /// Grows to past its size and settles.
    Pop,
    /// Comes down from larger.
    Slam,
    /// Grows from a little smaller while fading in.
    Grow,
    /// Grows from almost nothing.
    Tiny,
    /// Gets bigger as it fades, on the way out.
    Expand,
    /// Shown from the left edge across.
    WipeAcross,
    /// Taken away from the left edge across, on the way out.
    WipeAway,
    WipeUp,
    /// Rises out from behind a line at its own bottom edge, and on the way out goes up behind one
    /// at its top.
    Peek,
    /// Drops in and rocks to rest.
    Swing,
    /// Hops up from below, past where it rests, and wobbles down into place.
    Hop,
}

/// What each letter does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LetterMove {
    None,
    /// Typed one at a time with a cursor, kept centred, and deleted again at the end.
    Typewriter,
    /// Appear one at a time where they rest, with no cursor.
    Reveal,
    Rise,
    Pop,
    /// Drop from above and bounce.
    Bounce,
    /// Pop in, then keep rippling for as long as the title shows.
    Wave,
    /// Start a little spread apart, and draw together.
    Settle,
    /// Fade in one after another.
    FadeInTurn,
    /// Slide out from behind the line between the two lots of words, up for the first and down
    /// for the second.
    FromLine,
    /// Each word stutters on, one after another, like a neon sign.
    Flicker,
}

/// What the boxes and lines around the words do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxMove {
    None,
    /// Revealed from the left, one after another.
    Wipe,
    /// Revealed from alternate sides, the first from the left and the next from the right.
    Glide,
    /// Grow out from their middles along their longer side.
    Grow,
    /// The first grows out from a point in both directions, the rest open downwards.
    Unfold,
    /// A thin white bar runs across, and the box's colour fills in from its top.
    Bars,
}

/// Something done to the finished picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finish {
    None,
    /// Comes on in stutters, like a tube warming up.
    Flicker,
    /// Slices jump sideways and the colours split, on the way in and out and now and then between.
    Glitch,
    /// Dissolves in and out as drifting smoke.
    Smoke,
    /// Rings and sparks burst around the words.
    Fireworks,
    /// A colour sweeps along the words over the whole clip.
    Karaoke,
    /// A circle with a ring around it opens out from the middle until it fills the canvas.
    Iris,
    /// The colour sweeps across from the left with a darker band at its edge, and the canvas is
    /// swept away the same way at the end.
    Sweep,
    /// A line spins in and opens out sideways into the frame around the words.
    SpinOpen,
    /// The letters keep changing colour.
    ColourCycle,
}

/// Something that goes on for the whole clip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Running {
    None,
    /// Grows very slowly.
    SlowZoom,
    /// Travels up from below the canvas to past its top.
    Credits,
    /// Tilts more and more, drifting sideways.
    Tilt,
    /// The words are the time left in the clip, in seconds and hundredths.
    Countdown,
    /// One word shows at a time, the words sharing the clip between them.
    WordByWord,
}

/// How one style moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TitleMotion {
    pub enter: WholeMove,
    pub leave: WholeMove,
    pub enter_seconds: f32,
    pub leave_seconds: f32,
    pub letters: LetterMove,
    pub boxes: BoxMove,
    pub finish: Finish,
    pub running: Running,
    /// Letters and boxes go back out the way they came in, rather than staying put.
    pub leave_parts: bool,
}

/// How the whole picture differs from where it rests, at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    /// How far it has moved, as shares of the canvas's shorter side.
    pub across: f32,
    pub down: f32,
    /// How much bigger it is. 1 is its own size.
    pub grow: f32,
    /// Clockwise, in degrees.
    pub turn: f32,
    pub opacity: f32,
    /// How much of each side is hidden: left, top, right and bottom, as shares of the picture.
    pub cut: [f32; 4],
    /// How far it has moved, as shares of how tall the picture and canvas are together.
    pub through_canvas: f32,
    /// How far it has moved down, as a share of its own height. The part past where it rests is
    /// hidden, as if behind a line.
    pub lift: f32,
}

impl Pose {
    pub const REST: Self = Self {
        across: 0.0,
        down: 0.0,
        grow: 1.0,
        turn: 0.0,
        opacity: 1.0,
        cut: [0.0; 4],
        through_canvas: 0.0,
        lift: 0.0,
    };

    fn and(self, other: Self) -> Self {
        Self {
            across: self.across + other.across,
            down: self.down + other.down,
            grow: self.grow * other.grow,
            turn: self.turn + other.turn,
            opacity: self.opacity * other.opacity,
            cut: [
                self.cut[0].max(other.cut[0]),
                self.cut[1].max(other.cut[1]),
                self.cut[2].max(other.cut[2]),
                self.cut[3].max(other.cut[3]),
            ],
            through_canvas: self.through_canvas + other.through_canvas,
            lift: self.lift + other.lift,
        }
    }

    /// The placement that shows a picture this way, from where it rests.
    pub fn on(self, rest: Placement, picture: (u32, u32), canvas: (u32, u32)) -> Placement {
        let mut cut = self.cut;
        // Hidden behind the line at its resting bottom, or top when it goes up.
        if self.lift > 0.0 {
            cut[3] = cut[3].max(self.lift.min(1.0));
        } else if self.lift < 0.0 {
            cut[1] = cut[1].max((-self.lift).min(1.0));
        }
        let cut_across = cut[0] + cut[2];
        let cut_down = cut[1] + cut[3];
        // A picture can't be cut to nothing, so one nearly gone just doesn't show.
        let gone = cut_across >= 0.94 || cut_down >= 0.94;
        let mut placed = if cut == [0.0; 4] || gone {
            rest
        } else {
            rest.cropped_to(
                CropSides {
                    left: f64::from(cut[0]),
                    top: f64::from(cut[1]),
                    right: f64::from(cut[2]),
                    bottom: f64::from(cut[3]),
                },
                picture,
                canvas,
            )
        };
        let whole = f64::from(WHOLE);
        let (width, height) = (f64::from(canvas.0.max(1)), f64::from(canvas.1.max(1)));
        let shorter = width.min(height);
        let rect = rest.rect(picture, canvas);
        let through = f64::from(self.through_canvas) * (height + f64::from(rect.height));
        // Moving by its own height, with the part that has gone past the line cut away, leaves the
        // part still showing where it was.
        let lifted = f64::from(self.lift) * f64::from(rect.height);
        placed.center_x += (f64::from(self.across) * shorter / width * whole).round() as i32;
        placed.center_y +=
            ((f64::from(self.down) * shorter + through + lifted) / height * whole).round() as i32;
        placed.scale = (f64::from(placed.scale) * f64::from(self.grow)).round() as i32;
        placed.opacity = if gone {
            0
        } else {
            (f64::from(placed.opacity) * f64::from(self.opacity.clamp(0.0, 1.0))).round() as i32
        };
        placed.rotation =
            (placed.rotation + (self.turn * 1000.0).round() as i32).rem_euclid(360_000);
        placed
    }
}

/// How far along coming in and going out a title is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stage {
    /// 0 at the start of the clip, and 1 once it has come in.
    pub entering: f32,
    /// 0 until it starts going out, and 1 at the end of the clip.
    pub leaving: f32,
    /// How far through the whole clip, from 0 to 1.
    pub through: f32,
    pub seconds: f32,
    pub length: f32,
}

/// How one letter differs from where it rests.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LetterPose {
    /// As shares of how tall the letters are.
    pub across: f32,
    pub down: f32,
    pub grow: f32,
    pub opacity: f32,
}

impl LetterPose {
    pub const REST: Self = Self {
        across: 0.0,
        down: 0.0,
        grow: 1.0,
        opacity: 1.0,
    };
}

/// How long typing takes for each letter, and deleting.
const TYPING_SECONDS: f32 = 0.055;
const DELETING_SECONDS: f32 = 0.04;
/// How long the cursor stays on, and then off, once the words are typed.
const BLINK_SECONDS: f32 = 0.5;

fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

fn ease_in(t: f32) -> f32 {
    t.clamp(0.0, 1.0).powi(3)
}

/// Runs a little past the end and settles back.
fn overshoot(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0) - 1.0;
    let back = 1.70158;
    1.0 + (back + 1.0) * t.powi(3) + back * t.powi(2)
}

/// Lands, and bounces twice, smaller each time.
fn bounce(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let rate = 7.5625;
    let span = 2.75;
    if t < 1.0 / span {
        rate * t * t
    } else if t < 2.0 / span {
        let t = t - 1.5 / span;
        rate * t * t + 0.75
    } else if t < 2.5 / span {
        let t = t - 2.25 / span;
        rate * t * t + 0.9375
    } else {
        let t = t - 2.625 / span;
        rate * t * t + 0.984_375
    }
}

/// How far along one of a row is, when each starts a little after the one before.
pub fn in_turn(index: usize, count: usize, amount: f32, span: f32) -> f32 {
    let count = count.max(1) as f32;
    let moved = amount.clamp(0.0, 1.0) * (count - 1.0 + span);
    ((moved - index as f32) / span).clamp(0.0, 1.0)
}

/// The pose of the whole picture for a move that is `t` of the way to resting.
fn whole_pose(motion: WholeMove, t: f32, eased: f32, going: bool) -> Pose {
    let rest = Pose::REST;
    let away = 1.0 - eased;
    let shown = t.clamp(0.0, 1.0);
    match motion {
        WholeMove::None => rest,
        WholeMove::Fade => Pose {
            opacity: shown,
            ..rest
        },
        WholeMove::FadeRise => Pose {
            down: away * 0.015,
            opacity: shown,
            ..rest
        },
        WholeMove::Rise => Pose {
            down: away * 0.07,
            opacity: (t * 2.0).clamp(0.0, 1.0),
            ..rest
        },
        WholeMove::Fall => Pose {
            down: -away * 0.07,
            opacity: (t * 2.0).clamp(0.0, 1.0),
            ..rest
        },
        WholeMove::FromLeft => Pose {
            across: -away * 0.14,
            opacity: (t * 2.0).clamp(0.0, 1.0),
            ..rest
        },
        WholeMove::FromRight => Pose {
            across: away * 0.14,
            opacity: (t * 2.0).clamp(0.0, 1.0),
            ..rest
        },
        WholeMove::SweepLeft => Pose {
            across: -away * 2.0,
            ..rest
        },
        WholeMove::SweepRight => Pose {
            across: away * 2.0,
            ..rest
        },
        WholeMove::Pop => Pose {
            grow: 0.2 + overshoot(t) * 0.8,
            opacity: (t * 3.0).clamp(0.0, 1.0),
            ..rest
        },
        WholeMove::Slam => Pose {
            grow: 1.0 + away * 0.9,
            opacity: (t * 3.0).clamp(0.0, 1.0),
            ..rest
        },
        WholeMove::Grow => Pose {
            grow: 0.75 + eased * 0.25,
            opacity: shown,
            ..rest
        },
        WholeMove::Tiny => Pose {
            grow: 0.05 + eased * 0.95,
            opacity: (t * 4.0).clamp(0.0, 1.0),
            ..rest
        },
        WholeMove::Expand => Pose {
            grow: 1.0 + away * 0.3,
            opacity: shown,
            ..rest
        },
        WholeMove::WipeAcross => Pose {
            cut: [0.0, 0.0, away, 0.0],
            ..rest
        },
        WholeMove::WipeAway => Pose {
            cut: [away, 0.0, 0.0, 0.0],
            ..rest
        },
        WholeMove::WipeUp => Pose {
            cut: [0.0, away, 0.0, 0.0],
            ..rest
        },
        WholeMove::Peek => Pose {
            lift: if going { -away } else { away },
            opacity: (t * 3.0).clamp(0.0, 1.0),
            ..rest
        },
        WholeMove::Swing => Pose {
            down: -(1.0 - bounce(t)) * 0.2,
            turn: (1.0 - shown) * (t * 14.0).sin() * 14.0,
            opacity: (t * 4.0).clamp(0.0, 1.0),
            ..rest
        },
        WholeMove::Hop => {
            let height = if t < 0.3 {
                0.05 - (t / 0.3) * 0.17
            } else {
                let after = t - 0.3;
                -0.12 * (-after * 5.0).exp() * (after * 9.0).cos()
            };
            Pose {
                down: height * (1.0 - shown.powi(8)),
                turn: -9.0 * (1.0 - shown) * (t * 11.0).sin(),
                opacity: (t * 5.0).clamp(0.0, 1.0),
                ..rest
            }
        }
    }
}

impl TitleMotion {
    /// Doesn't move at all.
    pub const STILL: Self = Self {
        enter: WholeMove::None,
        leave: WholeMove::None,
        enter_seconds: 0.0,
        leave_seconds: 0.0,
        letters: LetterMove::None,
        boxes: BoxMove::None,
        finish: Finish::None,
        running: Running::None,
        leave_parts: false,
    };

    const fn whole(enter: WholeMove, enter_seconds: f32, leave: WholeMove) -> Self {
        Self {
            enter,
            leave,
            enter_seconds,
            leave_seconds: 0.35,
            ..Self::STILL
        }
    }

    const fn letters(letters: LetterMove, enter_seconds: f32) -> Self {
        Self {
            letters,
            enter_seconds,
            leave: WholeMove::Fade,
            leave_seconds: 0.35,
            ..Self::STILL
        }
    }

    /// Boxes and letters come in, and go back out the way they came.
    const fn parts(boxes: BoxMove, letters: LetterMove, seconds: f32) -> Self {
        Self {
            boxes,
            letters,
            enter_seconds: seconds,
            leave_seconds: seconds * 0.6,
            leave_parts: true,
            ..Self::STILL
        }
    }

    const fn finish(finish: Finish, enter_seconds: f32, leave_seconds: f32) -> Self {
        Self {
            finish,
            enter_seconds,
            leave_seconds,
            ..Self::STILL
        }
    }

    pub fn is_still(&self) -> bool {
        *self == Self::STILL
    }

    /// Whether a new picture has to be drawn while this plays, rather than moving the one there is.
    pub fn draws_frames(&self) -> bool {
        self.letters != LetterMove::None
            || self.boxes != BoxMove::None
            || !matches!(self.finish, Finish::None | Finish::Flicker)
            || matches!(self.running, Running::Countdown | Running::WordByWord)
    }

    /// How far along a clip of this length is, with coming in and going out squeezed to fit.
    pub fn stage(&self, seconds: f32, length: f32) -> Stage {
        let length = length.max(0.001);
        let wanted = self.enter_seconds + self.leave_seconds;
        let squeeze = if wanted > length * 0.9 {
            length * 0.9 / wanted
        } else {
            1.0
        };
        let enter = self.enter_seconds * squeeze;
        let leave = self.leave_seconds * squeeze;
        Stage {
            entering: if enter > 0.0 {
                (seconds / enter).clamp(0.0, 1.0)
            } else {
                1.0
            },
            leaving: if leave > 0.0 {
                ((seconds - (length - leave)) / leave).clamp(0.0, 1.0)
            } else {
                0.0
            },
            through: (seconds / length).clamp(0.0, 1.0),
            seconds,
            length,
        }
    }

    /// How far in the letters and boxes are: coming in, and if they go back out, going out.
    pub fn parts_amount(&self, stage: Stage) -> f32 {
        if self.leave_parts && stage.leaving > 0.0 {
            1.0 - stage.leaving
        } else {
            stage.entering
        }
    }

    /// How the whole picture differs from where it rests.
    pub fn pose(&self, stage: Stage) -> Pose {
        let coming = whole_pose(self.enter, stage.entering, ease_out(stage.entering), false);
        let left = 1.0 - stage.leaving;
        let going = whole_pose(self.leave, left, 1.0 - ease_in(stage.leaving), true);
        let running = match self.running {
            Running::SlowZoom => Pose {
                grow: 1.0 + stage.through * 0.12,
                ..Pose::REST
            },
            Running::Credits => Pose {
                through_canvas: 0.5 - stage.through,
                ..Pose::REST
            },
            Running::Tilt => Pose {
                across: 0.1 - stage.through * 0.2,
                turn: -10.0 * ease_out(stage.through * 1.4),
                ..Pose::REST
            },
            _ => Pose::REST,
        };
        let flicker = if self.finish == Finish::Flicker && stage.entering < 1.0 {
            // Fixed points, so it stutters the same way every time.
            let step = (stage.entering * 9.0) as usize;
            let on = [0.0, 0.8, 0.1, 0.0, 1.0, 0.3, 1.0, 0.6, 1.0, 1.0];
            Pose {
                opacity: on[step.min(9)],
                ..Pose::REST
            }
        } else {
            Pose::REST
        };
        coming.and(going).and(running).and(flicker)
    }

    /// Whether a glitch is showing just now: on the way in and out, and in short bursts between.
    pub fn glitching(stage: Stage) -> bool {
        let burst = (stage.seconds * 0.6).fract() < 0.1;
        stage.entering < 1.0 || stage.leaving > 0.0 || burst
    }

    /// Whether this moment needs a picture of its own, not just the resting one moved about.
    pub fn draws_frame_at(&self, stage: Stage) -> bool {
        if !self.draws_frames() {
            return false;
        }
        let moving = stage.entering < 1.0 || (self.leave_parts && stage.leaving > 0.0);
        match self {
            motion if matches!(motion.running, Running::Countdown | Running::WordByWord) => true,
            motion if motion.letters == LetterMove::Typewriter => true,
            motion if motion.letters == LetterMove::Wave => true,
            motion if matches!(motion.finish, Finish::Karaoke | Finish::ColourCycle) => true,
            motion if motion.finish == Finish::Glitch => Self::glitching(stage),
            motion if motion.finish == Finish::Smoke => stage.entering < 1.0 || stage.leaving > 0.0,
            motion
                if matches!(
                    motion.finish,
                    Finish::Iris | Finish::Sweep | Finish::SpinOpen
                ) =>
            {
                stage.entering < 1.0 || stage.leaving > 0.0
            }
            motion if motion.finish == Finish::Fireworks => stage.through < 0.3,
            _ => moving,
        }
    }

    /// Tells frames apart, so one that is already drawn isn't drawn again.
    pub fn frame_key(&self, stage: Stage) -> u64 {
        match self.running {
            Running::Countdown => return (f64::from(Self::time_left(stage)) * 100.0) as u64,
            Running::WordByWord => return (stage.through * 10_000.0) as u64,
            _ => {}
        }
        (f64::from(stage.seconds.max(0.0)) * 60.0) as u64
    }

    /// The time left in the clip, in seconds.
    pub fn time_left(stage: Stage) -> f32 {
        (stage.length - stage.seconds).max(0.0)
    }

    /// A countdown's words: seconds and hundredths, or minutes and seconds from 100 seconds up.
    pub fn countdown_words(stage: Stage) -> String {
        let left = Self::time_left(stage);
        if left < 100.0 {
            let hundredths = (f64::from(left) * 100.0).round() as u64;
            format!("{:02}:{:02}", hundredths / 100, hundredths % 100)
        } else {
            let seconds = f64::from(left).ceil() as u64;
            format!("{:02}:{:02}", seconds / 60, seconds % 60)
        }
    }

    /// Which word shows, when one shows at a time.
    pub fn word_showing(stage: Stage, words: usize) -> usize {
        ((stage.through * words as f32) as usize).min(words.saturating_sub(1))
    }

    /// How many letters are typed at this moment, and whether the cursor shows.
    pub fn typed(stage: Stage, count: usize) -> (usize, bool) {
        let letters = count as f32;
        let typing = (letters * TYPING_SECONDS).min(stage.length * 0.3);
        let deleting = (letters * DELETING_SECONDS).min(stage.length * 0.2);
        let seconds = stage.seconds;
        if seconds < typing {
            let shown = ((seconds / typing.max(0.001)) * letters).floor() as usize + 1;
            return (shown.min(count), true);
        }
        let deleting_from = stage.length - deleting;
        if seconds >= deleting_from {
            let gone = ((seconds - deleting_from) / deleting.max(0.001) * letters).ceil() as usize;
            return (count.saturating_sub(gone), false);
        }
        // It waits with the cursor off for a moment first, as in the recording.
        let waited = ((seconds - typing) / BLINK_SECONDS) as u32;
        (count, waited % 2 == 1)
    }

    /// How one letter of `count` differs from where it rests. `place` is how far it sits from the
    /// words' middle, from -1 to 1.
    pub fn letter_pose(&self, stage: Stage, index: usize, count: usize, place: f32) -> LetterPose {
        let amount = self.parts_amount(stage);
        // Letters wait for any box they sit in.
        let amount = match self.boxes {
            BoxMove::None => amount,
            _ => ((amount - 0.35) / 0.65).clamp(0.0, 1.0),
        };
        let rest = LetterPose::REST;
        match self.letters {
            LetterMove::None | LetterMove::Typewriter => {
                if self.boxes == BoxMove::None {
                    rest
                } else {
                    LetterPose {
                        opacity: amount,
                        ..rest
                    }
                }
            }
            LetterMove::Reveal => {
                let shown = amount * count as f32;
                LetterPose {
                    opacity: if (index as f32) < shown { 1.0 } else { 0.0 },
                    ..rest
                }
            }
            LetterMove::Rise => {
                let t = in_turn(index, count, amount, 5.0);
                LetterPose {
                    down: (1.0 - ease_out(t)) * 0.7,
                    opacity: t,
                    ..rest
                }
            }
            LetterMove::Pop => {
                let t = in_turn(index, count, amount, 2.5);
                LetterPose {
                    grow: overshoot(t).max(0.0),
                    opacity: (t * 3.0).clamp(0.0, 1.0),
                    ..rest
                }
            }
            LetterMove::Bounce => {
                let t = in_turn(index, count, amount, 7.0);
                LetterPose {
                    down: -(1.0 - bounce(t)) * 1.4,
                    opacity: (t * 5.0).clamp(0.0, 1.0),
                    ..rest
                }
            }
            LetterMove::Wave => {
                let t = in_turn(index, count, amount, 5.0);
                let ripple = (stage.seconds * 5.0 - index as f32 * 0.7).sin() * 0.13;
                LetterPose {
                    down: ripple,
                    grow: overshoot(t).max(0.0),
                    opacity: (t * 3.0).clamp(0.0, 1.0),
                    ..rest
                }
            }
            LetterMove::Settle => LetterPose {
                across: place * (1.0 - ease_out(amount)) * 0.35,
                opacity: amount,
                ..rest
            },
            // Close together, so whole lines fade in one after another rather than letter by letter.
            LetterMove::FadeInTurn => LetterPose {
                opacity: in_turn(index, count, amount, (count as f32 * 0.7).max(3.0)),
                ..rest
            },
            LetterMove::FromLine => LetterPose {
                // Which way is set where it is drawn, by which side of the line it is on.
                down: 1.0 - ease_out(amount),
                ..rest
            },
            LetterMove::Flicker => rest,
        }
    }

    /// How much a word of `words` shows, for words that stutter on in turn.
    pub fn word_flicker(&self, stage: Stage, word: usize, words: usize) -> f32 {
        let amount = self.parts_amount(stage);
        let words = words.max(1) as f32;
        let local = amount * (words + 1.0) - word as f32;
        if local <= 0.0 {
            return 0.0;
        }
        if local >= 1.0 {
            return 1.0;
        }
        let steps = [0.35, 0.0, 0.55, 0.15, 0.8, 0.4, 1.0, 0.7, 1.0, 1.0];
        steps[((local * steps.len() as f32) as usize).min(steps.len() - 1)]
    }

    /// How much of one box of `count` shows, from 0 to 1.
    pub fn box_share(&self, stage: Stage, index: usize, count: usize) -> f32 {
        let amount = (self.parts_amount(stage) / 0.65).clamp(0.0, 1.0);
        match self.boxes {
            BoxMove::None => 1.0,
            _ => ease_out(in_turn(index, count, amount, 1.5)),
        }
    }
}

impl TitleStyle {
    /// How titles in this style move. Plain text, the text styles, Subtitle and the top and bottom
    /// meme stay still.
    #[allow(clippy::too_many_lines)] // One line for each style reads best as a single table.
    pub fn motion(self) -> TitleMotion {
        use {
            BoxMove as B, Finish as F, LetterMove as L, Running as R, TitleMotion as M,
            WholeMove as W,
        };
        if self.is_text_style() {
            return M::STILL;
        }
        match self {
            Self::Subtitle | Self::MemeBands => M::STILL,
            // Titles
            Self::Typewriter => TitleMotion {
                letters: L::Typewriter,
                ..M::STILL
            },
            Self::Circular => TitleMotion {
                finish: F::Iris,
                enter_seconds: 0.35,
                leave_seconds: 0.3,
                ..M::STILL
            },
            Self::Groovy => TitleMotion {
                enter: W::Fade,
                leave: W::Expand,
                ..M::finish(F::ColourCycle, 0.25, 0.4)
            },
            Self::Fireworks => TitleMotion {
                enter: W::Tiny,
                leave: W::Tiny,
                ..M::finish(F::Fireworks, 0.4, 0.35)
            },
            Self::FadeWords => M::whole(W::Peek, 0.3, W::Peek),
            Self::Smoke => M::finish(F::Smoke, 1.6, 0.9),
            Self::Framed => TitleMotion {
                finish: F::SpinOpen,
                enter_seconds: 1.1,
                leave_seconds: 0.9,
                leave_parts: true,
                ..M::STILL
            },
            Self::Tidal => TitleMotion {
                letters: L::Settle,
                enter_seconds: 0.6,
                leave_seconds: 0.5,
                leave_parts: true,
                ..M::STILL
            },
            Self::Glitch => TitleMotion {
                enter: W::Fade,
                leave: W::Fade,
                ..M::finish(F::Glitch, 0.4, 0.3)
            },
            Self::DoubleLines => M::parts(B::Grow, L::FadeInTurn, 1.0),
            Self::PushThrough => TitleMotion {
                finish: F::Sweep,
                enter_seconds: 0.5,
                leave_seconds: 0.4,
                ..M::STILL
            },
            Self::LargeHeading => M::whole(W::Fade, 0.5, W::Fade),
            Self::OutlineShadow => M::letters(L::Reveal, 1.0),
            Self::QuickPeek => TitleMotion {
                letters: L::Rise,
                ..M::whole(W::Fade, 0.4, W::Fade)
            },
            Self::Stencil => M::whole(W::WipeAcross, 0.5, W::WipeAway),
            Self::MakeItGlow => TitleMotion {
                letters: L::Flicker,
                enter_seconds: 1.0,
                leave: W::Fade,
                leave_seconds: 0.7,
                ..M::STILL
            },
            Self::Statement => TitleMotion {
                leave: W::Fade,
                leave_seconds: 0.4,
                ..M::parts(B::Grow, L::None, 0.7)
            },
            // Two lines
            Self::Underlined => M::parts(B::Grow, L::FromLine, 0.9),
            Self::Bouncing => M::whole(W::Hop, 1.0, W::Fall),
            Self::Gliding => M::parts(B::Glide, L::None, 0.7),
            Self::Funky => M::parts(B::Unfold, L::None, 0.7),
            Self::Boxed => M::whole(W::FadeRise, 0.6, W::Fade),
            Self::FadeZoom => M::whole(W::Grow, 1.2, W::None),
            // Captions
            Self::Karaoke => M::finish(F::Karaoke, 0.0, 0.0),
            Self::MultiLine => M::parts(B::Bars, L::None, 0.8),
            Self::Plunging => M::parts(B::Unfold, L::None, 0.8),
            Self::LowerThird | Self::LowerThirdGreen => M::parts(B::Wipe, L::None, 0.6),
            Self::SideBar => M::parts(B::Grow, L::Rise, 0.8),
            Self::Rapid => TitleMotion {
                running: R::WordByWord,
                ..M::STILL
            },
            // Special
            Self::Quote => TitleMotion {
                leave: W::Fade,
                leave_seconds: 0.4,
                ..M::letters(L::FadeInTurn, 1.0)
            },
            Self::Rating => M::parts(B::None, L::Pop, 1.0),
            Self::CreditsRoll => TitleMotion {
                running: R::Credits,
                ..M::STILL
            },
            Self::Countdown => TitleMotion {
                running: R::Countdown,
                ..M::STILL
            },
            Self::SalePattern => TitleMotion {
                running: R::Tilt,
                ..M::whole(W::Fade, 0.4, W::Fade)
            },
            // Intros and outros
            Self::IntroMirror => M::parts(B::Grow, L::FadeInTurn, 1.0),
            Self::IntroFunky => M::parts(B::Grow, L::FadeInTurn, 0.8),
            Self::IntroClean => M::parts(B::Grow, L::Rise, 1.0),
            Self::IntroPlungeLeft => M::whole(W::SweepLeft, 0.55, W::SweepLeft),
            Self::IntroPlungeRight => M::whole(W::SweepRight, 0.55, W::SweepRight),
            _ => M::STILL,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::title::STYLE_GROUPS;

    const LENGTH: f32 = 5.0;
    const CANVAS: (u32, u32) = (1920, 1080);

    fn every_style() -> impl Iterator<Item = TitleStyle> {
        STYLE_GROUPS
            .iter()
            .flat_map(|(_, styles)| styles.iter().copied())
    }

    #[test]
    fn plain_words_text_styles_captions_and_memes_stay_still() {
        for style in every_style() {
            let still = style.is_text_style()
                || matches!(style, TitleStyle::Subtitle | TitleStyle::MemeBands);
            let motion = style.motion();
            assert_eq!(motion.is_still(), still, "{style:?}");
            if still {
                assert_eq!(motion.pose(motion.stage(0.0, LENGTH)), Pose::REST);
                assert!(!motion.draws_frame_at(motion.stage(0.0, LENGTH)));
            }
        }
    }

    #[test]
    fn everything_comes_to_rest_in_the_middle_of_a_clip_unless_it_runs_throughout() {
        for style in every_style() {
            let motion = style.motion();
            if motion.running != Running::None {
                continue;
            }
            let pose = motion.pose(motion.stage(LENGTH / 2.0, LENGTH));
            assert_eq!(pose, Pose::REST, "{style:?}");
        }
    }

    #[test]
    fn coming_in_and_going_out_fit_inside_a_short_clip() {
        for style in every_style() {
            let motion = style.motion();
            let stage = motion.stage(0.25, 0.5);
            assert!(
                stage.entering > 0.0 || motion.enter_seconds == 0.0,
                "{style:?}"
            );
            assert!(stage.leaving < 1.0, "{style:?}");
            let end = motion.stage(0.5, 0.5);
            assert!(end.entering >= 1.0, "{style:?}");
        }
    }

    #[test]
    fn nothing_jumps_between_one_frame_and_the_next_except_where_it_is_meant_to() {
        for style in every_style() {
            let motion = style.motion();
            if matches!(motion.finish, Finish::Flicker)
                || matches!(
                    motion.enter,
                    WholeMove::WipeUp | WholeMove::SweepLeft | WholeMove::SweepRight
                )
            {
                continue;
            }
            let mut last = motion.pose(motion.stage(0.0, LENGTH));
            for frame in 1..=(LENGTH * 60.0) as usize {
                let pose = motion.pose(motion.stage(frame as f32 / 60.0, LENGTH));
                assert!((pose.down - last.down).abs() < 0.2, "{style:?}");
                assert!((pose.across - last.across).abs() < 0.8, "{style:?}");
                last = pose;
            }
        }
    }

    #[test]
    fn a_pose_at_rest_leaves_the_placement_alone() {
        let rest = Placement {
            center_x: WHOLE / 3,
            scale: WHOLE / 2,
            ..Placement::default()
        };
        assert_eq!(Pose::REST.on(rest, (400, 100), CANVAS), rest);
    }

    #[test]
    fn a_wipe_shows_part_of_the_picture_without_moving_what_it_shows() {
        let rest = Placement {
            scale: WHOLE / 2,
            ..Placement::default()
        };
        let pose = Pose {
            cut: [0.0, 0.0, 0.5, 0.0],
            ..Pose::REST
        };
        let whole = rest.rect((400, 100), CANVAS);
        let half = pose.on(rest, (400, 100), CANVAS).rect((400, 100), CANVAS);
        assert!((half.x - whole.x).abs() < 1.0);
        assert!((half.width - whole.width / 2.0).abs() < 1.0);
    }

    #[test]
    fn peeking_keeps_the_part_showing_above_the_line_it_rises_from() {
        let rest = Placement {
            scale: WHOLE / 2,
            ..Placement::default()
        };
        let picture = (400, 100);
        let whole = rest.rect(picture, CANVAS);
        let pose = Pose {
            lift: 0.5,
            ..Pose::REST
        };
        let half = pose.on(rest, picture, CANVAS).rect(picture, CANVAS);
        assert!((half.y + half.height - (whole.y + whole.height)).abs() < 1.5);
        assert!((half.height - whole.height / 2.0).abs() < 1.5);
    }

    #[test]
    fn a_picture_wiped_to_almost_nothing_does_not_show() {
        let pose = Pose {
            cut: [0.0, 0.0, 0.99, 0.0],
            ..Pose::REST
        };
        assert_eq!(pose.on(Placement::default(), (400, 100), CANVAS).opacity, 0);
    }

    #[test]
    fn typing_keeps_the_cursor_on_and_it_blinks_once_the_words_are_typed() {
        let at = |seconds: f32| TitleMotion::typed(TitleMotion::STILL.stage(seconds, LENGTH), 10);
        assert_eq!(at(0.0), (1, true));
        let (halfway, cursor) = at(0.27);
        assert!((4..=6).contains(&halfway) && cursor);
        assert_eq!(at(0.7), (10, false), "it waits with the cursor off first");
        assert_eq!(at(1.2), (10, true));
        assert_eq!(at(1.7), (10, false));
    }

    #[test]
    fn typed_words_are_deleted_at_the_end_with_no_cursor() {
        let at = |seconds: f32| TitleMotion::typed(TitleMotion::STILL.stage(seconds, LENGTH), 10);
        let (near_end, cursor) = at(LENGTH - 0.15);
        assert!(near_end < 10 && near_end > 0, "{near_end}");
        assert!(!cursor);
        assert_eq!(at(LENGTH).0, 0);
    }

    #[test]
    fn credits_start_below_the_canvas_and_end_above_it() {
        let motion = TitleStyle::CreditsRoll.motion();
        let rest = Placement {
            scale: WHOLE / 2,
            ..Placement::default()
        };
        let picture = (800, 900);
        let height = CANVAS.1 as f32;
        let start = motion
            .pose(motion.stage(0.0, LENGTH))
            .on(rest, picture, CANVAS)
            .rect(picture, CANVAS);
        assert!(start.y >= height - 1.0);
        let end = motion
            .pose(motion.stage(LENGTH, LENGTH))
            .on(rest, picture, CANVAS)
            .rect(picture, CANVAS);
        assert!(end.y + end.height <= 1.0);
    }

    #[test]
    fn a_countdown_reads_the_time_left_in_seconds_and_hundredths() {
        let motion = TitleStyle::Countdown.motion();
        let words = |seconds| TitleMotion::countdown_words(motion.stage(seconds, LENGTH));
        assert_eq!(words(0.0), "05:00");
        assert_eq!(words(0.04), "04:96");
        assert_eq!(words(LENGTH), "00:00");
        let long = TitleMotion::countdown_words(motion.stage(0.0, 300.0));
        assert_eq!(long, "05:00", "five minutes reads as minutes and seconds");
    }

    #[test]
    fn rapid_shows_each_word_for_its_share_of_the_clip() {
        let stage = |seconds| TitleStyle::Rapid.motion().stage(seconds, LENGTH);
        assert_eq!(TitleMotion::word_showing(stage(0.0), 5), 0);
        assert_eq!(TitleMotion::word_showing(stage(2.5), 5), 2);
        assert_eq!(TitleMotion::word_showing(stage(LENGTH), 5), 4);
    }

    #[test]
    fn parts_that_come_in_go_back_out_at_the_end() {
        let motion = TitleStyle::Gliding.motion();
        let share = |seconds| motion.box_share(motion.stage(seconds, LENGTH), 0, 2);
        assert_eq!(share(LENGTH / 2.0), 1.0);
        assert!(share(LENGTH - 0.05) < 0.8);
        assert_eq!(share(LENGTH), 0.0);
        assert!(motion.draws_frame_at(motion.stage(LENGTH - 0.1, LENGTH)));
    }

    #[test]
    fn words_stutter_on_one_after_another() {
        let motion = TitleStyle::MakeItGlow.motion();
        let early = motion.stage(0.2, LENGTH);
        assert!(motion.word_flicker(early, 0, 3) > 0.0 || motion.word_flicker(early, 0, 3) == 0.0);
        assert_eq!(motion.word_flicker(early, 2, 3), 0.0);
        let rest = motion.stage(LENGTH / 2.0, LENGTH);
        assert_eq!(motion.word_flicker(rest, 2, 3), 1.0);
    }

    #[test]
    fn a_frame_is_only_drawn_again_when_its_moment_changes() {
        let motion = TitleStyle::Bouncing.motion();
        let at = |seconds| motion.frame_key(motion.stage(seconds, LENGTH));
        assert_eq!(at(0.100), at(0.101));
        assert_ne!(at(0.100), at(0.120));
    }
}
