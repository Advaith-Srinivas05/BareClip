//! Moves titles in the preview. The project's clips keep where each title rests, and the stack about
//! to be drawn gets where it is at this moment, along with a new picture if its parts are moving.

use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::Arc,
};

use bareclip_core::{
    motion::{Pose, Running},
    picture::StillPicture,
    placement::Placement,
    time::Ticks,
    timeline::{Clip, ClipId, MediaId},
    title::Title,
};
use bareclip_text::MovingTitle;

/// A title whose parts are moving, ready to draw any moment.
struct Holder {
    title: Title,
    canvas: (u32, u32),
    moving: MovingTitle,
    drawn: Option<(u64, Arc<StillPicture>)>,
}

/// How far into a clip a moment is, and how long the clip is, in seconds.
pub(crate) fn seconds_in(clip: &Clip, now: Ticks) -> (f32, f32) {
    (
        Ticks(now.0 - clip.start.0).as_seconds() as f32,
        clip.duration().as_seconds() as f32,
    )
}

#[derive(Default)]
pub struct TitleFrames {
    holders: HashMap<MediaId, Holder>,
    /// The pictures showing now that aren't a title's resting one.
    shown: HashMap<MediaId, Arc<StillPicture>>,
    logos: HashMap<PathBuf, Option<Arc<StillPicture>>>,
}

impl TitleFrames {
    /// The picture for a title's logo, read the first time it is wanted.
    pub fn logo(&mut self, title: &Title) -> Option<Arc<StillPicture>> {
        let path = title.logo.as_ref()?;
        self.logos
            .entry(path.clone())
            .or_insert_with(|| bareclip_media::read_still(path).ok().map(Arc::new))
            .clone()
    }

    /// Moves each title in the stack to where it is at this moment, leaving `held` at rest. Returns
    /// whether any new picture was drawn, because the stack alone wouldn't show that.
    pub fn pose_stack(
        &mut self,
        stack: &mut [Clip],
        now: Ticks,
        canvas: (u32, u32),
        titles: &BTreeMap<MediaId, Title>,
        rest_size_of: &dyn Fn(MediaId) -> Option<(u32, u32)>,
        held: Option<ClipId>,
    ) -> bool {
        self.shown.clear();
        self.holders
            .retain(|media, _| stack.iter().any(|clip| clip.media == *media));
        let mut drew = false;
        for clip in stack.iter_mut() {
            let Some(title) = titles.get(&clip.media) else {
                continue;
            };
            let motion = title.style.motion();
            if motion.is_still() {
                continue;
            }
            let is_held = held == Some(clip.id);
            // A countdown reads the time at any moment, so holding it still only stops it fading.
            if is_held && motion.running != Running::Countdown {
                continue;
            }
            let (seconds, length) = seconds_in(clip, now);
            let stage = motion.stage(seconds, length);
            let pose = if is_held {
                Pose::REST
            } else {
                motion.pose(stage)
            };
            if motion.draws_frames() && motion.draws_frame_at(stage) {
                let key = motion.frame_key(stage);
                let logo = self.logo(title);
                let stale = self
                    .holders
                    .get(&clip.media)
                    .is_none_or(|holder| holder.title != *title || holder.canvas != canvas);
                if stale {
                    self.holders.insert(
                        clip.media,
                        Holder {
                            title: title.clone(),
                            canvas,
                            moving: MovingTitle::prepare(title, canvas, logo.as_deref()),
                            drawn: None,
                        },
                    );
                }
                let Some(holder) = self.holders.get_mut(&clip.media) else {
                    continue;
                };
                let picture = match &holder.drawn {
                    Some((drawn_key, picture)) if *drawn_key == key => picture.clone(),
                    _ => {
                        let picture = Arc::new(holder.moving.frame(stage));
                        holder.drawn = Some((key, picture.clone()));
                        drew = true;
                        picture
                    }
                };
                let size = (picture.width, picture.height);
                let rest = Placement {
                    scale: holder.moving.scale_for(size),
                    ..clip.placement
                };
                clip.placement = pose.on(rest, size, canvas);
                self.shown.insert(clip.media, picture);
            } else if let Some(size) = rest_size_of(clip.media) {
                clip.placement = pose.on(clip.placement, size, canvas);
            }
        }
        drew
    }

    /// The size of the picture showing for a title, if it isn't its resting one.
    pub fn shown_size(&self, media: MediaId) -> Option<(u32, u32)> {
        self.shown
            .get(&media)
            .map(|picture| (picture.width, picture.height))
    }

    /// The picture showing for a title, if it isn't its resting one.
    pub fn shown_picture(&self, media: MediaId) -> Option<Arc<StillPicture>> {
        self.shown.get(&media).cloned()
    }
}

#[cfg(test)]
mod tests {
    use bareclip_core::{placement::WHOLE, timeline::TimeRange, title::TitleStyle, tracks::Tracks};

    use super::*;

    const CANVAS: (u32, u32) = (1280, 720);

    fn stack_with(style: TitleStyle) -> (Vec<Clip>, BTreeMap<MediaId, Title>) {
        let media = MediaId::for_title(1);
        let mut tracks = Tracks::default();
        tracks.place_title(media, TimeRange::new_title(), Ticks(0));
        let clips: Vec<Clip> = tracks.tracks()[0].clips().to_vec();
        (clips, BTreeMap::from([(media, Title::in_style(style))]))
    }

    fn rest_size(_: MediaId) -> Option<(u32, u32)> {
        Some((400, 100))
    }

    #[test]
    fn a_title_that_only_moves_as_a_whole_changes_place_but_draws_nothing() {
        let (mut stack, titles) = stack_with(TitleStyle::Bouncing);
        let rest = stack[0].placement;
        let mut frames = TitleFrames::default();
        let drew = frames.pose_stack(
            &mut stack,
            Ticks::from_seconds(0.1),
            CANVAS,
            &titles,
            &rest_size,
            None,
        );
        assert!(!drew);
        assert_ne!(stack[0].placement, rest);
        assert!(frames.shown_picture(stack[0].media).is_none());
    }

    #[test]
    fn a_title_whose_letters_move_gets_a_picture_of_its_own_only_while_they_do() {
        let (mut stack, titles) = stack_with(TitleStyle::Rating);
        let media = stack[0].media;
        let mut frames = TitleFrames::default();
        let drew = frames.pose_stack(
            &mut stack,
            Ticks::from_seconds(0.5),
            CANVAS,
            &titles,
            &rest_size,
            None,
        );
        assert!(drew);
        assert!(frames.shown_picture(media).is_some());
        let drew_again = frames.pose_stack(
            &mut stack,
            Ticks::from_seconds(0.5),
            CANVAS,
            &titles,
            &rest_size,
            None,
        );
        assert!(!drew_again, "the same moment doesn't need drawing twice");
        frames.pose_stack(
            &mut stack,
            Ticks::from_seconds(2.5),
            CANVAS,
            &titles,
            &rest_size,
            None,
        );
        assert!(frames.shown_picture(media).is_none());
    }

    #[test]
    fn a_held_title_stays_at_rest_so_it_can_be_seen_and_changed() {
        let (mut stack, titles) = stack_with(TitleStyle::Typewriter);
        let rest = stack[0].placement;
        let id = stack[0].id;
        let mut frames = TitleFrames::default();
        let drew = frames.pose_stack(&mut stack, Ticks(0), CANVAS, &titles, &rest_size, Some(id));
        assert!(!drew);
        assert_eq!(stack[0].placement, rest);
    }

    #[test]
    fn captions_cost_nothing() {
        let (mut stack, titles) = stack_with(TitleStyle::Subtitle);
        let rest = stack[0].placement;
        let mut frames = TitleFrames::default();
        let drew = frames.pose_stack(&mut stack, Ticks(0), CANVAS, &titles, &rest_size, None);
        assert!(!drew);
        assert_eq!(stack[0].placement, rest);
        assert!(frames.holders.is_empty());
    }

    #[test]
    fn a_title_no_longer_showing_is_forgotten() {
        let (mut stack, titles) = stack_with(TitleStyle::Typewriter);
        let mut frames = TitleFrames::default();
        frames.pose_stack(
            &mut stack,
            Ticks::from_seconds(0.5),
            CANVAS,
            &titles,
            &rest_size,
            None,
        );
        assert_eq!(frames.holders.len(), 1);
        frames.pose_stack(&mut [], Ticks(0), CANVAS, &titles, &rest_size, None);
        assert!(frames.holders.is_empty());
    }

    #[test]
    fn a_frame_stays_centred_on_where_the_title_rests() {
        let (mut stack, titles) = stack_with(TitleStyle::Bright);
        stack[0].placement.center_x = WHOLE / 3;
        let mut frames = TitleFrames::default();
        frames.pose_stack(
            &mut stack,
            Ticks::from_seconds(0.1),
            CANVAS,
            &titles,
            &rest_size,
            None,
        );
        assert_eq!(stack[0].placement.center_x, WHOLE / 3);
    }
}
