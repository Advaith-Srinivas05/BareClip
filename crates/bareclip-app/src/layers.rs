//! Shows several clips at once. The main `Preview` still plays the top clip's pictures. Everything
//! under it gets a player of its own that follows the same clock.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use bareclip_core::{
    picture::StillPicture,
    speed::{NORMAL, Speed},
    time::{TICKS_PER_SECOND, Ticks},
    timeline::{Clip, ClipId, MediaId},
};
use bareclip_media::{MediaInfo, PreviewFrame, fit_preview_size};
use bareclip_render::{Compositor, Layer, LayerPicture, RenderError};
use eframe::{
    egui::{self, mutex::RwLock},
    egui_wgpu, wgpu,
};

use crate::player::{Notify, PREVIEW_HEIGHT, PREVIEW_WIDTH, Player};

/// How far a clip's picture can fall behind the clock before it jumps to catch up. Below this a
/// slow machine just shows fewer pictures.
const LATE_LIMIT: Duration = Duration::from_millis(500);
/// How long it gets to recover from one jump before it can jump again.
const CATCH_UP_EVERY: Duration = Duration::from_secs(2);
/// The same for a clip playing faster than it was recorded.
const FAST_CATCH_UP_EVERY: Duration = Duration::from_millis(400);
/// How close a piece's footage has to line up with a clip's to be part of it. Different speeds
/// can round a little.
const SAME_PLACE_IN_FOOTAGE: i64 = TICKS_PER_SECOND / 1000;

/// The videos in a stack that need a player of their own, which is all but the top one, because
/// the main `Preview` plays that. An image has nothing to play.
fn followed_videos(stack: &[Clip]) -> Vec<Clip> {
    let mut videos: Vec<Clip> = stack
        .iter()
        .filter(|clip| !clip.is_still())
        .copied()
        .collect();
    videos.pop();
    videos
}

/// Where a moment on the timeline falls in a clip's video.
fn source_time(clip: &Clip, now: Ticks) -> Ticks {
    clip.source_at(now).min(clip.source.end)
}

/// Whether a picture has fallen so far behind that it should stop trying to decode its way there.
/// `now` and `shown` are places in its footage. How late is late goes by the time it takes on
/// screen, and a clip playing fast gets to jump more often, so it goes in steps and doesn't sit
/// frozen.
pub fn should_catch_up(
    now: Ticks,
    shown: Option<Ticks>,
    last_jump: Duration,
    speed: Speed,
) -> bool {
    let late = speed.in_footage(Ticks::from_seconds(LATE_LIMIT.as_secs_f64()));
    let every = if speed.rate > NORMAL {
        FAST_CATCH_UP_EVERY
    } else {
        CATCH_UP_EVERY
    };
    shown.is_some_and(|shown| now.0 - shown.0 > late.0) && last_jump >= every
}

/// The piece of the stack a picture from the main player belongs to. Pieces of a clip have their
/// own ids, but they line up with the clip's footage in the same way.
fn clip_of_piece(stack: &[Clip], piece: &Clip) -> Option<ClipId> {
    stack
        .iter()
        .rev()
        .find(|clip| {
            clip.media == piece.media
                && clip.speed == piece.speed
                && (clip.source_at(piece.start).0 - piece.source.start.0).abs()
                    <= SAME_PLACE_IN_FOOTAGE
        })
        .map(|clip| clip.id)
}

struct Follower {
    clip: ClipId,
    /// Still getting ready for when this clip first goes under another.
    waiting: bool,
    player: Player,
    shown: Option<Ticks>,
    last_jump: Instant,
}

/// What to do this frame, as `Preview` sees it.
#[derive(Clone, Debug)]
pub struct Moment {
    pub now: Ticks,
    pub playing: bool,
    /// The playhead was moved by hand, or playback started or stopped.
    pub jumped: bool,
    /// What shows from the next change, if that is close enough to get ready for.
    pub coming: Option<(Ticks, Vec<Clip>)>,
}

#[derive(Default)]
pub struct LayerPictures {
    followers: Vec<Follower>,
    pictures: HashMap<ClipId, PreviewFrame>,
    /// Something has changed since the canvas was last drawn.
    changed: bool,
    /// The clips and where they were when the canvas was last drawn.
    drawn: Vec<Clip>,
}

impl LayerPictures {
    /// Brings every layer up to date for this moment. `stack` is what shows, lowest first, and
    /// `top` is the picture the main player just produced, if any, and where it came from.
    pub fn update(
        &mut self,
        stack: &[Clip],
        moment: Moment,
        top: Option<(PreviewFrame, Clip)>,
        lookup: &dyn Fn(MediaId) -> Option<MediaInfo>,
        notify: &Notify,
    ) -> Option<String> {
        let mut problem = None;
        let under = followed_videos(stack);
        let (coming_at, coming) = match &moment.coming {
            Some((at, stack)) if moment.playing => (*at, followed_videos(stack)),
            _ => (moment.now, Vec::new()),
        };

        self.followers.retain(|follower| {
            let wanted = under
                .iter()
                .chain(&coming)
                .any(|clip| clip.id == follower.clip);
            // One that was getting ready for a moment that was jumped away from starts over.
            wanted && !(follower.waiting && moment.jumped)
        });
        for clip in &coming {
            if under.iter().any(|shown| shown.id == clip.id)
                || self.followers.iter().any(|f| f.clip == clip.id)
            {
                continue;
            }
            let Some(info) = lookup(clip.media) else {
                continue;
            };
            match Player::open_as(&info, true, notify.clone()) {
                Ok(mut player) => {
                    player.prepare(source_time(clip, coming_at));
                    self.followers.push(Follower {
                        clip: clip.id,
                        waiting: true,
                        player,
                        shown: None,
                        last_jump: Instant::now(),
                    });
                }
                Err(error) => problem = Some(error),
            }
        }
        for clip in &under {
            let known = self.followers.iter().any(|f| f.clip == clip.id);
            if known {
                continue;
            }
            let Some(info) = lookup(clip.media) else {
                continue;
            };
            match Player::open_as(&info, true, notify.clone()) {
                Ok(mut player) => {
                    player.seek(source_time(clip, moment.now));
                    if moment.playing {
                        player.play();
                    }
                    self.followers.push(Follower {
                        clip: clip.id,
                        waiting: false,
                        player,
                        shown: None,
                        last_jump: Instant::now(),
                    });
                }
                Err(error) => problem = Some(error),
            }
        }

        for follower in &mut self.followers {
            let Some(clip) = under.iter().find(|clip| clip.id == follower.clip) else {
                // Not on show yet, so it only fills up with pictures.
                follower.player.update();
                continue;
            };
            if std::mem::take(&mut follower.waiting) {
                follower.player.begin();
                follower.last_jump = Instant::now();
            }
            let wanted = source_time(clip, moment.now);
            if moment.jumped {
                follower.player.seek(wanted);
                if moment.playing {
                    follower.player.play();
                } else {
                    follower.player.pause();
                }
                follower.shown = None;
            } else if moment.playing
                && should_catch_up(
                    wanted,
                    follower.shown,
                    follower.last_jump.elapsed(),
                    clip.speed,
                )
            {
                follower.player.seek(wanted);
                follower.last_jump = Instant::now();
                follower.shown = None;
            }

            let frame = if moment.playing {
                follower.player.update_following(wanted)
            } else {
                follower.player.update()
            };
            if let Some(error) = follower.player.take_error() {
                problem = Some(error);
            }
            if let Some(frame) = frame.filter(|frame| frame.time < clip.source.end) {
                follower.shown = Some(frame.time);
                self.pictures.insert(clip.id, frame);
                self.changed = true;
            }
        }

        if let Some((frame, piece)) = top
            && let Some(id) = clip_of_piece(stack, &piece)
        {
            self.pictures.insert(id, frame);
            self.changed = true;
        }

        self.pictures
            .retain(|id, _| stack.iter().any(|clip| clip.id == *id));
        problem
    }

    /// Says an image has just finished loading, so the canvas is out of date.
    pub fn picture_arrived(&mut self) {
        self.changed = true;
    }

    /// Whether the canvas needs drawing again for this stack.
    pub fn needs_drawing(&self, stack: &[Clip]) -> bool {
        self.changed || self.drawn != stack
    }

    /// Whether every clip in the stack has a picture to show.
    pub fn is_ready(&self, stack: &[Clip]) -> bool {
        stack
            .iter()
            .all(|clip| clip.is_still() || self.pictures.contains_key(&clip.id))
    }

    /// Draws the stack. `size_of` gives each video's real size, because the size of the picture
    /// being drawn was rounded for the preview and would shift it slightly from the export.
    pub fn draw(
        &mut self,
        compositor: &mut Compositor,
        stack: &[Clip],
        now: Ticks,
        canvas: (u32, u32),
        size_of: &dyn Fn(MediaId) -> Option<(u32, u32)>,
        still_of: &dyn Fn(MediaId) -> Option<Arc<StillPicture>>,
    ) -> Result<(), RenderError> {
        let target = compositor.size();
        let stills: Vec<(MediaId, Arc<StillPicture>)> = stack
            .iter()
            .filter(|clip| clip.is_still())
            .filter_map(|clip| Some((clip.media, still_of(clip.media)?)))
            .collect();
        let layers: Vec<Layer> = stack
            .iter()
            .filter_map(|clip| {
                let picture: LayerPicture = if clip.is_still() {
                    let (_, still) = stills.iter().find(|(media, _)| *media == clip.media)?;
                    (&**still).into()
                } else {
                    self.pictures.get(&clip.id)?.view()?.into()
                };
                let placement = clip.placement.at(Ticks(now.0 - clip.start.0));
                let size = size_of(clip.media)?;
                Some(Layer {
                    picture,
                    rect: scaled_rect(placement.rect(size, canvas), canvas, target),
                    opacity: placement.opacity as f32 / bareclip_core::placement::WHOLE as f32,
                    look: placement.look(),
                })
            })
            .collect();
        compositor.draw_for_display(&layers)?;
        self.changed = false;
        self.drawn = stack.to_vec();
        Ok(())
    }
}

/// Draws layers on the window's GPU, and hands the result to the window as a picture it can show
/// without anything coming back through memory.
pub struct LayerDisplay {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Arc<RwLock<egui_wgpu::Renderer>>,
    compositor: Option<Compositor>,
    texture: Option<egui::TextureId>,
    /// The picture is up to date for the project as it now is, so it can be shown.
    drawn: bool,
    drawn_on: Option<((u32, u32), [u8; 3])>,
}

impl LayerDisplay {
    pub fn on_window(gpu: &egui_wgpu::RenderState) -> Self {
        Self::new(gpu.device.clone(), gpu.queue.clone(), gpu.renderer.clone())
    }

    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        renderer: Arc<RwLock<egui_wgpu::Renderer>>,
    ) -> Self {
        Self {
            device,
            queue,
            renderer,
            compositor: None,
            texture: None,
            drawn: false,
            drawn_on: None,
        }
    }

    /// Starts over, because the project has just begun or stopped taking layers.
    pub fn forget(&mut self) {
        self.drawn = false;
    }

    /// The picture to show, once there is one.
    pub fn picture(&self) -> Option<egui::TextureId> {
        self.texture.filter(|_| self.drawn)
    }

    /// Makes sure there is a canvas of the right shape for this project's finished video.
    fn compositor_for(&mut self, canvas: (u32, u32)) -> Result<&mut Compositor, RenderError> {
        let size = fit_preview_size(canvas.0, canvas.1, PREVIEW_WIDTH, PREVIEW_HEIGHT);
        let stale = self
            .compositor
            .as_ref()
            .is_none_or(|compositor| compositor.size() != size);
        if stale {
            let compositor =
                Compositor::on_device(self.device.clone(), self.queue.clone(), size.0, size.1)?;
            let mut renderer = self.renderer.write();
            let view = compositor.display_view();
            match self.texture {
                Some(id) => renderer.update_egui_texture_from_wgpu_texture(
                    &self.device,
                    view,
                    wgpu::FilterMode::Linear,
                    id,
                ),
                None => {
                    self.texture = Some(renderer.register_native_texture(
                        &self.device,
                        view,
                        wgpu::FilterMode::Linear,
                    ));
                }
            }
            drop(renderer);
            self.compositor = Some(compositor);
            self.drawn = false;
        }
        self.compositor
            .as_mut()
            .ok_or_else(|| RenderError::Gpu("The canvas was not made.".to_owned()))
    }

    /// Draws the stack if it has changed. The first drawing waits until every layer has a
    /// picture, because until then the last plain picture is a better thing to show.
    pub fn draw(
        &mut self,
        pictures: &mut LayerPictures,
        stack: &[Clip],
        now: Ticks,
        (canvas, background): ((u32, u32), [u8; 3]),
        size_of: &dyn Fn(MediaId) -> Option<(u32, u32)>,
        still_of: &dyn Fn(MediaId) -> Option<Arc<StillPicture>>,
    ) -> Result<(), RenderError> {
        self.compositor_for(canvas)?.set_background(background);
        // Places are worked out from the canvas, so a new one means drawing them again.
        let new_canvas = self.drawn_on != Some((canvas, background));
        let wanted = pictures.needs_drawing(stack) || new_canvas || !self.drawn;
        if !wanted || (!self.drawn && !pictures.is_ready(stack)) {
            return Ok(());
        }
        pictures.draw(
            self.compositor_for(canvas)?,
            stack,
            now,
            canvas,
            size_of,
            still_of,
        )?;
        self.drawn = true;
        self.drawn_on = Some((canvas, background));
        Ok(())
    }
}

/// A place on the finished video's canvas, moved to the preview's smaller canvas.
fn scaled_rect(
    rect: bareclip_render::Rect,
    canvas: (u32, u32),
    target: (u32, u32),
) -> bareclip_render::Rect {
    let across = target.0 as f32 / canvas.0.max(1) as f32;
    let down = target.1 as f32 / canvas.1.max(1) as f32;
    bareclip_render::Rect {
        x: rect.x * across,
        y: rect.y * down,
        width: rect.width * across,
        height: rect.height * down,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{path::PathBuf, process::Command, sync::Arc, thread};

    use bareclip_core::{
        placement::{Placement, WHOLE},
        timeline::TimeRange,
        tracks::{TrackTarget, Tracks},
    };
    use bareclip_media::probe;

    use crate::preview::Preview;

    fn seconds(value: f64) -> Ticks {
        Ticks::from_seconds(value)
    }

    fn clip(start: f64, source_start: f64, length: f64) -> Clip {
        Clip {
            id: ClipId(1),
            media: MediaId(1),
            start: seconds(start),
            source: TimeRange {
                start: seconds(source_start),
                end: seconds(source_start + length),
            },
            placement: Placement::default(),
            sound: Default::default(),
            speed: Default::default(),
        }
    }

    #[test]
    fn a_moment_on_the_timeline_lands_on_the_right_footage() {
        assert_eq!(
            source_time(&clip(10.0, 3.0, 5.0), seconds(12.0)),
            seconds(5.0)
        );
    }

    #[test]
    fn a_moment_past_a_clip_stops_at_its_end() {
        assert_eq!(
            source_time(&clip(10.0, 3.0, 5.0), seconds(99.0)),
            seconds(8.0)
        );
    }

    #[test]
    fn a_picture_a_little_behind_is_left_to_recover_by_itself() {
        assert!(!should_catch_up(
            seconds(5.0),
            Some(seconds(4.8)),
            Duration::from_secs(60),
            Speed::default(),
        ));
    }

    #[test]
    fn a_picture_far_behind_jumps_but_not_again_straight_away() {
        let behind = Some(seconds(3.0));

        assert!(should_catch_up(
            seconds(5.0),
            behind,
            Duration::from_secs(3),
            Speed::default(),
        ));
        assert!(!should_catch_up(
            seconds(5.0),
            behind,
            Duration::from_millis(100),
            Speed::default(),
        ));
    }

    #[test]
    fn a_picture_not_shown_yet_is_still_starting_rather_than_late() {
        assert!(!should_catch_up(
            seconds(5.0),
            None,
            Duration::from_secs(60),
            Speed::default(),
        ));
    }

    #[test]
    fn a_piece_of_a_clip_belongs_to_that_clip() {
        let whole = clip(0.0, 10.0, 100.0);
        let piece = Clip {
            id: ClipId(99),
            start: seconds(50.0),
            source: TimeRange {
                start: seconds(60.0),
                end: seconds(110.0),
            },
            ..whole
        };

        assert_eq!(clip_of_piece(&[whole], &piece), Some(whole.id));
    }

    #[test]
    fn a_picture_from_a_clip_that_no_longer_shows_belongs_to_nothing() {
        let whole = clip(0.0, 10.0, 100.0);
        let elsewhere = Clip {
            media: MediaId(2),
            ..whole
        };

        assert_eq!(clip_of_piece(&[whole], &elsewhere), None);
    }

    #[test]
    fn a_place_keeps_its_share_of_a_smaller_canvas() {
        let rect = scaled_rect(
            bareclip_render::Rect {
                x: 960.0,
                y: 540.0,
                width: 480.0,
                height: 270.0,
            },
            (1920, 1080),
            (1280, 720),
        );

        assert_eq!((rect.x, rect.y), (640.0, 360.0));
        assert_eq!((rect.width, rect.height), (320.0, 180.0));
    }

    /// A colourful moving picture, with enough detail for a wrong place or colour to show.
    fn detailed_clip(name: &str, pattern: &str) -> PathBuf {
        let folder = std::env::temp_dir().join("bareclip-app-tests");
        std::fs::create_dir_all(&folder).expect("the folder can be created");
        let path = folder.join(format!("layers-{name}-{}.mp4", std::process::id()));
        if path.exists() {
            return path;
        }
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
            ])
            .arg(format!("{pattern}=s=640x360:r=30:d=3,format=yuv420p"))
            .args(["-c:v", "libopenh264", "-b:v", "8M", "-g", "30"])
            .args(["-colorspace", "bt709"])
            .arg(&path)
            .status()
            .expect("ffmpeg is on PATH");
        assert!(status.success());
        path
    }

    /// The average difference between two pictures of the same size, out of 255.
    fn average_difference(first: &[u8], second: &[u8]) -> f64 {
        assert_eq!(first.len(), second.len());
        let total: u64 = first
            .iter()
            .zip(second)
            .map(|(a, b)| u64::from(a.abs_diff(*b)))
            .sum();
        total as f64 / first.len() as f64
    }

    /// A compositor, or nothing on a machine with no GPU at all, such as a build server.
    fn compositor_of(size: (u32, u32)) -> Option<Compositor> {
        match Compositor::new(size.0, size.1) {
            Ok(compositor) => Some(compositor),
            Err(RenderError::NoGpu(reason)) => {
                eprintln!("skipped, because there is no GPU here: {reason}");
                None
            }
            Err(error) => panic!("{error}"),
        }
    }

    /// What the window would show for a project, paused at a moment.
    fn drawn_by_the_preview(
        tracks: &Tracks,
        infos: &[MediaInfo],
        at: Ticks,
        compositor: &mut Compositor,
    ) -> Vec<u8> {
        let lookup = |id: MediaId| infos.get(id.0 as usize).cloned();
        let size_of = |id: MediaId| {
            infos
                .get(id.0 as usize)
                .map(|info| (info.video.width, info.video.height))
        };
        let canvas = tracks.canvas(&size_of).expect("there is a canvas");
        let notify: Notify = Arc::new(|| {});
        let mut preview = Preview::new(false, notify.clone());
        preview.set_planes(true, tracks.played_videos(), &lookup);
        preview.seek(tracks.played_videos(), &lookup, at);
        let mut pictures = LayerPictures::default();
        let stack = tracks.stack_at(at, canvas, &size_of);
        let stills: HashMap<MediaId, Arc<StillPicture>> = stack
            .iter()
            .filter(|clip| clip.is_still())
            .map(|clip| {
                let info = lookup(clip.media).expect("the image is known");
                let still = bareclip_media::read_still(&info.path).expect("the image can be read");
                (clip.media, Arc::new(still))
            })
            .collect();
        let exact = |pictures: &LayerPictures| {
            stack.iter().all(|clip| {
                clip.is_still()
                    || pictures
                        .pictures
                        .get(&clip.id)
                        .is_some_and(|picture| picture.time >= clip.source_at(at))
            })
        };
        let started = Instant::now();
        while !exact(&pictures) {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "the exact pictures never came"
            );
            let top = preview.update(tracks.played_videos(), &lookup);
            let moment = Moment {
                now: at,
                playing: false,
                jumped: preview.take_jump(),
                coming: None,
            };
            let top = top.zip(preview.last_picture_from());
            pictures.update(&stack, moment, top, &lookup, &notify);
            thread::sleep(Duration::from_millis(2));
        }
        pictures
            .draw(compositor, &stack, at, canvas, &size_of, &|id| {
                stills.get(&id).cloned()
            })
            .expect("it draws");
        compositor.read_display().expect("it reads back")
    }

    /// What playing real recordings fast looks like, against a real clock. Run with
    /// `$env:BARECLIP_LAYER_FILES = 'a.mp4'; ./scripts/cargo.ps1 test --release -p bareclip-app fast_playback_on_real_recordings '--' --ignored --nocapture`
    #[test]
    #[ignore = "needs real recordings, and reports numbers rather than checking them"]
    fn fast_playback_on_real_recordings() {
        let Some(paths) = real_files() else {
            eprintln!("set BARECLIP_LAYER_FILES");
            return;
        };
        for path in paths {
            let info = probe(&path).expect("the video can be read");
            println!(
                "{}: {:.1} s at {:.1} fps",
                path.display(),
                info.duration.as_seconds(),
                info.video.frame_rate
            );
            for factor in [0.5, 1.0, 2.0, 4.0, 8.0, 16.0] {
                let infos = [info.clone()];
                let lookup = |id: MediaId| infos.get(id.0 as usize).cloned();
                let mut tracks = Tracks::default();
                let id = tracks.push(
                    MediaId(0),
                    TimeRange {
                        start: Ticks(0),
                        end: info.duration,
                    },
                );
                tracks.set_speed(id, Speed::default().with_factor(factor));
                let notify: Notify = Arc::new(|| {});
                let mut preview = Preview::new(false, notify);
                preview.set_planes(true, tracks.played_videos(), &lookup);
                preview.play(tracks.played_videos(), &lookup);
                let started = Instant::now();
                // At most this long, so slow motion doesn't take all day.
                let longest = Duration::from_secs(8);
                let (mut pictures, mut last, mut longest_wait) = (0_u32, Instant::now(), 0.0_f64);
                let mut furthest_behind = 0.0_f64;
                while preview.is_playing() && started.elapsed() < longest {
                    if let Some(frame) = preview.update(tracks.played_videos(), &lookup) {
                        pictures += 1;
                        longest_wait = longest_wait.max(last.elapsed().as_secs_f64());
                        last = Instant::now();
                        let wanted = tracks
                            .clip(id)
                            .expect("it is there")
                            .source_at(preview.position(tracks.played_videos()));
                        let behind = (wanted.0 - frame.time.0) as f64 / 705_600_000.0;
                        furthest_behind = furthest_behind.max(behind);
                    }
                    thread::sleep(Duration::from_millis(2));
                }
                let seconds_played = started.elapsed().as_secs_f64();
                println!(
                    "  {factor}x: {:.1} pictures a second, longest wait {:.0} ms, at most {:.2} s of footage behind, over {seconds_played:.1} s",
                    f64::from(pictures) / seconds_played,
                    longest_wait * 1000.0,
                    furthest_behind,
                );
            }
        }
    }

    #[test]
    fn a_clip_playing_fast_can_catch_up_sooner_and_is_late_by_its_own_measure() {
        let fast = Speed::default().with_factor(8.0);
        let (now, shown) = (seconds(10.0), Some(seconds(8.0)));

        assert!(!should_catch_up(
            now,
            shown,
            Duration::from_millis(500),
            fast
        ));
        let far_behind = Some(seconds(5.0));
        assert!(should_catch_up(
            now,
            far_behind,
            Duration::from_millis(500),
            fast
        ));
        assert!(!should_catch_up(
            now,
            far_behind,
            Duration::from_millis(100),
            fast
        ));
    }

    #[test]
    fn a_faster_clip_looks_the_same_in_the_preview_and_the_export() {
        let path = detailed_clip("at-speed", "testsrc2");
        let info = probe(&path).expect("the video can be read");
        let infos = vec![info.clone()];
        let mut tracks = Tracks::default();
        let id = tracks.place(
            MediaId(0),
            TimeRange {
                start: Ticks(0),
                end: seconds(3.0),
            },
            Ticks(0),
            TrackTarget::New(0),
        );
        tracks.set_speed(id, Speed::default().with_factor(2.0));
        let canvas = (info.video.width, info.video.height);
        let size_of = |_: MediaId| Some(canvas);
        let Some(mut compositor) = compositor_of(canvas) else {
            return;
        };
        let at = seconds(1.0);
        let previewed = drawn_by_the_preview(&tracks, &infos, at, &mut compositor);

        let output = std::env::temp_dir().join(format!(
            "bareclip-app-tests/layers-speed-matches-{}.mp4",
            std::process::id()
        ));
        let stretches = tracks
            .stretches(canvas, &size_of)
            .into_iter()
            .map(|stretch| bareclip_export::LayeredStretch {
                start: stretch.start,
                end: stretch.end,
                stack: stretch
                    .stack
                    .iter()
                    .map(|clip| bareclip_export::LayeredClip {
                        key: clip.id.0,
                        input: path.clone(),
                        start: clip.start,
                        source: clip.source,
                        placement: clip.placement,
                        speed: clip.speed,
                    })
                    .collect(),
            })
            .collect();
        let request = bareclip_export::LayeredRequest {
            output: output.clone(),
            stretches,
            sound: bareclip_export::SoundPlan::default(),
            canvas: Some(canvas),
            background: [0, 0, 0],
            quality: Default::default(),
            rate: Default::default(),
        };
        bareclip_export::export_layered(
            &request,
            &std::sync::atomic::AtomicBool::new(false),
            &|_| {},
        )
        .expect("the export succeeds");

        let mut reader = bareclip_media::PictureReader::open(&output).expect("the export opens");
        reader.seek(at).expect("it seeks");
        let exported = loop {
            let picture = reader
                .next_picture()
                .expect("it decodes")
                .expect("the moment is in the video");
            if picture.time() >= at {
                break picture;
            }
        };
        let layer = Layer {
            picture: exported.view().into(),
            rect: bareclip_render::fit_inside(exported.size(), canvas),
            opacity: 1.0,
            look: bareclip_render::PictureLook::PLAIN,
        };
        compositor.draw_for_display(&[layer]).expect("it draws");
        let shown = compositor.read_display().expect("it reads back");

        let difference = average_difference(&previewed, &shown);
        assert!(
            difference < 6.0,
            "the export differs from the preview by {difference:.2} out of 255"
        );
        // A clip at the wrong place in its footage makes this large, so it means something.
        let mut normal = tracks.clone();
        normal.set_speed(id, Speed::default());
        let elsewhere = drawn_by_the_preview(&normal, &infos, at, &mut compositor);
        assert!(
            average_difference(&previewed, &elsewhere) > difference + 1.0,
            "the speed made no difference to the picture"
        );
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn a_see_through_image_looks_the_same_in_the_preview_and_the_export() {
        let video = detailed_clip("under-image", "testsrc2");
        let folder = std::env::temp_dir().join("bareclip-app-tests");
        let image = folder.join(format!("layers-image-{}.png", std::process::id()));
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
            ])
            .arg("color=orange@0.5:s=320x180,format=rgba")
            .args(["-frames:v", "1"])
            .arg(&image)
            .status()
            .expect("ffmpeg is on PATH");
        assert!(status.success());
        let infos: Vec<MediaInfo> = [&video, &image]
            .iter()
            .map(|path| probe(path).expect("it can be read"))
            .collect();
        let source = TimeRange {
            start: Ticks(0),
            end: seconds(3.0),
        };
        let mut tracks = Tracks::default();
        tracks.place(MediaId(0), source, Ticks(0), TrackTarget::New(0));
        let still = tracks.place(
            MediaId(1),
            TimeRange::new_still(),
            Ticks(0),
            TrackTarget::New(0),
        );
        tracks.set_placement(
            still,
            Placement {
                center_x: WHOLE * 7 / 10,
                center_y: WHOLE * 4 / 10,
                scale: WHOLE / 2,
                rotation: 20_000,
                ..Placement::default()
            },
        );
        let size_of = |id: MediaId| {
            infos
                .get(id.0 as usize)
                .map(|info| (info.video.width, info.video.height))
        };
        let canvas = (infos[0].video.width, infos[0].video.height);
        let Some(mut compositor) = compositor_of(canvas) else {
            return;
        };
        let at = seconds(1.0);
        let previewed = drawn_by_the_preview(&tracks, &infos, at, &mut compositor);

        let output = folder.join(format!("layers-image-matches-{}.mp4", std::process::id()));
        let paths = [video.clone(), image.clone()];
        let stretches = tracks
            .stretches(canvas, &size_of)
            .into_iter()
            .map(|stretch| bareclip_export::LayeredStretch {
                start: stretch.start,
                end: stretch.end,
                stack: stretch
                    .stack
                    .iter()
                    .map(|clip| bareclip_export::LayeredClip {
                        key: clip.id.0,
                        input: paths[clip.media.0 as usize].clone(),
                        start: clip.start,
                        source: clip.source,
                        placement: clip.placement,
                        speed: clip.speed,
                    })
                    .collect(),
            })
            .collect();
        let request = bareclip_export::LayeredRequest {
            output: output.clone(),
            stretches,
            sound: bareclip_export::SoundPlan::default(),
            canvas: Some(canvas),
            background: [0, 0, 0],
            quality: Default::default(),
            rate: Default::default(),
        };
        bareclip_export::export_layered(
            &request,
            &std::sync::atomic::AtomicBool::new(false),
            &|_| {},
        )
        .expect("the export succeeds");

        let mut reader = bareclip_media::PictureReader::open(&output).expect("the export opens");
        reader.seek(at).expect("it seeks");
        let exported = loop {
            let picture = reader
                .next_picture()
                .expect("it decodes")
                .expect("the moment is in the video");
            if picture.time() >= at {
                break picture;
            }
        };
        let layer = Layer {
            picture: exported.view().into(),
            rect: bareclip_render::fit_inside(exported.size(), canvas),
            opacity: 1.0,
            look: bareclip_render::PictureLook::PLAIN,
        };
        compositor.draw_for_display(&[layer]).expect("it draws");
        let shown = compositor.read_display().expect("it reads back");

        let difference = average_difference(&previewed, &shown);
        assert!(
            difference < 6.0,
            "the export differs from the preview by {difference:.2} out of 255"
        );
        // Without the image the picture is clearly different, so the comparison means something.
        let mut without = tracks.clone();
        without.remove(still);
        let plain = drawn_by_the_preview(&without, &infos, at, &mut compositor);
        assert!(
            average_difference(&previewed, &plain) > difference + 2.0,
            "the image made no difference"
        );
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn the_preview_and_the_export_show_the_same_picture() {
        let paths = [
            detailed_clip("below", "testsrc2"),
            detailed_clip("above", "testsrc"),
        ];
        let infos: Vec<MediaInfo> = paths
            .iter()
            .map(|path| probe(path).expect("the video can be read"))
            .collect();
        let source = TimeRange {
            start: Ticks(0),
            end: seconds(3.0),
        };
        let mut tracks = Tracks::default();
        tracks.place(MediaId(0), source, Ticks(0), TrackTarget::New(0));
        let above = tracks.place(MediaId(1), source, Ticks(0), TrackTarget::New(0));
        tracks.set_placement(
            above,
            Placement {
                center_x: WHOLE * 7 / 10,
                center_y: WHOLE * 4 / 10,
                scale: WHOLE / 2,
                opacity: WHOLE * 3 / 4,
                rotation: 20_000,
                flip_x: true,
                crop_left: WHOLE / 10,
                crop_bottom: WHOLE / 5,
                ..Placement::default()
            },
        );
        let size_of = |id: MediaId| {
            infos
                .get(id.0 as usize)
                .map(|info| (info.video.width, info.video.height))
        };
        let canvas = tracks.canvas(&size_of).expect("there is a canvas");
        let Some(mut compositor) = compositor_of(canvas) else {
            return;
        };

        // The frame at a moment that falls exactly on one in both videos.
        let at = seconds(1.0);
        let previewed = drawn_by_the_preview(&tracks, &infos, at, &mut compositor);

        let output = std::env::temp_dir().join(format!(
            "bareclip-app-tests/layers-matches-{}.mp4",
            std::process::id()
        ));
        let stretches = tracks
            .stretches(canvas, &size_of)
            .into_iter()
            .map(|stretch| bareclip_export::LayeredStretch {
                start: stretch.start,
                end: stretch.end,
                stack: stretch
                    .stack
                    .iter()
                    .map(|clip| bareclip_export::LayeredClip {
                        key: clip.id.0,
                        input: paths[clip.media.0 as usize].clone(),
                        start: clip.start,
                        source: clip.source,
                        placement: clip.placement,
                        speed: clip.speed,
                    })
                    .collect(),
            })
            .collect();
        let request = bareclip_export::LayeredRequest {
            output: output.clone(),
            stretches,
            sound: bareclip_export::SoundPlan::default(),
            canvas: None,
            background: [0, 0, 0],
            quality: Default::default(),
            rate: Default::default(),
        };
        bareclip_export::export_layered(
            &request,
            &std::sync::atomic::AtomicBool::new(false),
            &|_| {},
        )
        .expect("the export succeeds");

        // The exported frame, shown the way the window would show it.
        let mut reader = bareclip_media::PictureReader::open(&output).expect("the export opens");
        reader.seek(at).expect("it seeks");
        let exported = loop {
            let picture = reader
                .next_picture()
                .expect("it decodes")
                .expect("the moment is in the video");
            if picture.time() >= at {
                break picture;
            }
        };
        let layer = Layer {
            picture: exported.view().into(),
            rect: bareclip_render::fit_inside(exported.size(), canvas),
            opacity: 1.0,
            look: bareclip_render::PictureLook::PLAIN,
        };
        compositor.draw_for_display(&[layer]).expect("it draws");
        let shown = compositor.read_display().expect("it reads back");

        let difference = average_difference(&previewed, &shown);
        assert!(
            difference < 6.0,
            "the export differs from the preview by {difference:.2} out of 255"
        );
        // A clip out of place makes this large, so the comparison means something.
        let mut moved = tracks.clone();
        moved.set_placement(
            above,
            Placement {
                center_x: WHOLE * 3 / 10,
                center_y: WHOLE * 6 / 10,
                scale: WHOLE / 2,
                opacity: WHOLE * 3 / 4,
                rotation: 20_000,
                flip_x: true,
                crop_left: WHOLE / 10,
                crop_bottom: WHOLE / 5,
                ..Placement::default()
            },
        );
        let elsewhere = drawn_by_the_preview(&moved, &infos, at, &mut compositor);
        assert!(average_difference(&elsewhere, &shown) > difference * 3.0);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn colours_dont_jump_between_a_plain_preview_and_a_layered_one() {
        let path = detailed_clip("colours", "testsrc2");
        let size = (640, 360);
        let Some(mut compositor) = compositor_of(size) else {
            return;
        };
        let first_frame = |preview: &mut bareclip_media::VideoPreview| loop {
            if let bareclip_media::DecodeStep::Frame(frame) =
                preview.next_step(None).expect("it decodes")
            {
                break frame;
            }
        };
        let mut plain =
            bareclip_media::VideoPreview::open(&path, 640, 360).expect("the video opens");
        let mut planes = bareclip_media::VideoPreview::open(&path, 640, 360)
            .expect("the video opens")
            .with_planes();

        let plain = first_frame(&mut plain);
        let planes = first_frame(&mut planes);
        let layer = Layer {
            picture: planes.view().expect("it has planes").into(),
            rect: bareclip_render::fit_inside(size, size),
            opacity: 1.0,
            look: bareclip_render::PictureLook::PLAIN,
        };
        compositor.draw_for_display(&[layer]).expect("it draws");
        let drawn = compositor.read_display().expect("it reads back");

        let difference = average_difference(&plain.rgba, &drawn);
        assert!(
            difference < 3.0,
            "the two ways of showing a picture differ by {difference:.2} out of 255"
        );
    }

    #[test]
    fn a_picture_for_the_gpu_is_tightly_packed_and_labelled() {
        let path = detailed_clip("packed", "testsrc2");
        let mut preview = bareclip_media::VideoPreview::open(&path, 320, 180)
            .expect("the video opens")
            .with_planes();

        let frame = loop {
            if let bareclip_media::DecodeStep::Frame(frame) =
                preview.next_step(None).expect("it decodes")
            {
                break frame;
            }
        };

        assert_eq!((frame.width, frame.height), (320, 180));
        assert!(frame.rgba.is_empty());
        let view = frame.view().expect("it has planes");
        assert_eq!(view.y.len(), 320 * 180);
        assert_eq!(view.u.len(), 160 * 90);
        assert_eq!(view.v.len(), 160 * 90);
    }

    /// A busy test video with B-frames, like a gameplay recording.
    fn busy_video(name: &str, size: (u32, u32), pattern: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "bareclip-layer-gate-{name}-{}x{}.mp4",
            size.0, size.1
        ));
        if path.exists() {
            return path;
        }
        let filter = format!(
            "{pattern}=s={}x{}:r=60,noise=alls=10:allf=t,format=yuv420p",
            size.0, size.1
        );
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
            ])
            .arg(filter)
            .args([
                "-t",
                "30",
                "-c:v",
                "h264_nvenc",
                "-preset",
                "p4",
                "-rc",
                "vbr",
                "-cq",
                "20",
            ])
            .args(["-bf", "3", "-g", "120", "-colorspace", "bt709"])
            .arg(&path)
            .status()
            .expect("ffmpeg is on PATH");
        assert!(status.success(), "the test video could not be made");
        path
    }

    /// What one run of the speed test found.
    struct Run {
        seconds: f64,
        /// Different pictures shown, and how many the video has in that stretch, for each layer
        /// from the bottom. Recordings have no one frame rate, so this counts the real frames.
        shown: Vec<(u64, u64)>,
        /// The longest wait between new pictures on any layer.
        longest_gap: f64,
        /// The longest wait the videos themselves have between frames over the same stretch.
        longest_gap_in_source: f64,
        draws_per_second: f64,
        slowest_draw_ms: f64,
    }

    /// One full-size video with the others small over it, arriving at a moment on the timeline.
    fn gate_layout(infos: &[MediaInfo], overlays_from: Ticks) -> Tracks {
        let mut tracks = Tracks::default();
        let length = infos
            .iter()
            .map(|info| info.duration)
            .min()
            .expect("there are videos");
        for (index, _) in infos.iter().enumerate() {
            let id = tracks.place(
                MediaId(index as u64),
                TimeRange {
                    start: Ticks(0),
                    end: length,
                },
                if index > 0 { overlays_from } else { Ticks(0) },
                TrackTarget::New(0),
            );
            if index > 0 {
                tracks.set_placement(
                    id,
                    Placement {
                        center_x: if index == 1 { WHOLE / 4 } else { WHOLE * 3 / 4 },
                        center_y: if index == 1 { WHOLE / 4 } else { WHOLE * 3 / 4 },
                        scale: WHOLE * 2 / 5,
                        ..Placement::default()
                    },
                );
            }
        }
        tracks
    }

    /// When each frame of a video appears, in order.
    fn frame_times(info: &MediaInfo) -> Vec<Ticks> {
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let index =
            bareclip_media::scan_frames(&info.path, &cancel, |_| {}).expect("it can be scanned");
        let mut times: Vec<Ticks> = index
            .frames()
            .iter()
            .map(|frame| Ticks::from_pts(frame.pts, info.video.time_base))
            .collect();
        times.sort();
        times
    }

    fn real_time_run(sources: &[PathBuf], run_for: Duration, overlays_from: Ticks) -> Run {
        run_against(sources, run_for, overlays_from, false)
    }

    /// With `on_sound_card`, every clip is decoded and mixed and goes to the real sound output, at
    /// no volume so nothing is heard, and how much of it has played is the clock.
    fn run_against(
        sources: &[PathBuf],
        run_for: Duration,
        overlays_from: Ticks,
        on_sound_card: bool,
    ) -> Run {
        let infos: Vec<MediaInfo> = sources
            .iter()
            .map(|path| probe(path).expect("the video can be read"))
            .collect();
        let lookup = |id: MediaId| infos.get(id.0 as usize).cloned();
        let size_of = |id: MediaId| {
            infos
                .get(id.0 as usize)
                .map(|info| (info.video.width, info.video.height))
        };
        let tracks = gate_layout(&infos, overlays_from);
        let canvas = tracks.canvas(&size_of).expect("there is a canvas");
        assert!(tracks.needs_layers(canvas, &size_of));

        let notify: Notify = Arc::new(|| {});
        let mut preview = Preview::new(on_sound_card, notify.clone());
        preview.set_planes(true, tracks.played(), &lookup);
        if on_sound_card {
            let spans = tracks
                .tracks()
                .iter()
                .flat_map(|track| track.clips())
                .map(|clip| bareclip_audio::MixSpan {
                    path: infos[clip.media.0 as usize].path.clone(),
                    start: clip.start,
                    source: clip.source,
                    sound: bareclip_core::sound::ClipSound {
                        volume: 0,
                        ..Default::default()
                    },
                    speed: clip.speed,
                    bus: bareclip_audio::Bus::All(0),
                })
                .collect();
            preview.set_sound(spans, tracks.duration());
        }
        let mut pictures = LayerPictures::default();
        let target = bareclip_media::fit_preview_size(canvas.0, canvas.1, 1280, 720);
        let mut compositor = Compositor::new(target.0, target.1).expect("the GPU opens");

        preview.play(tracks.played(), &lookup);
        let started = Instant::now();
        let mut last_seen: HashMap<ClipId, (Ticks, Instant)> = HashMap::new();
        let mut counts: HashMap<ClipId, u64> = HashMap::new();
        let mut longest_gap = Duration::ZERO;
        let mut draws = 0_u64;
        let mut slowest_draw = Duration::ZERO;
        let mut stack_seen = Vec::new();
        let mut reached = Ticks(0);
        while started.elapsed() < run_for && preview.is_playing() {
            let top = preview.update(tracks.played(), &lookup);
            let piece = preview.last_picture_from();
            let now = preview.position(tracks.played());
            reached = now;
            let stack = tracks.stack_at(now, canvas, &size_of);
            let moment = Moment {
                now,
                playing: preview.is_playing(),
                jumped: preview.take_jump(),
                coming: tracks.next_change(
                    now,
                    Ticks::from_seconds(crate::preview::PREPARE_AHEAD_SECONDS),
                    canvas,
                    &size_of,
                ),
            };
            let top = top.zip(piece);
            if let Some(problem) = pictures.update(&stack, moment, top, &lookup, &notify) {
                panic!("{problem}");
            }
            for clip in &stack {
                if let Some(picture) = pictures.pictures.get(&clip.id) {
                    let seen = last_seen.get(&clip.id).copied();
                    if seen.is_none_or(|(time, _)| time != picture.time) {
                        // Only once the clock runs does a wait mean anything.
                        if let Some((_, at)) = seen
                            && started.elapsed() > Duration::from_secs(1)
                        {
                            longest_gap = longest_gap.max(at.elapsed());
                        }
                        last_seen.insert(clip.id, (picture.time, Instant::now()));
                        *counts.entry(clip.id).or_default() += 1;
                    }
                }
            }
            if pictures.needs_drawing(&stack) {
                let before = Instant::now();
                pictures
                    .draw(&mut compositor, &stack, now, canvas, &size_of, &|_| None)
                    .expect("it draws");
                slowest_draw = slowest_draw.max(before.elapsed());
                draws += 1;
            }
            if !stack.is_empty() {
                stack_seen = stack;
            }
            // About what a 144 Hz window does, so the loop isn't the limit.
            thread::sleep(Duration::from_millis(6));
        }
        let seconds = started.elapsed().as_secs_f64();
        if on_sound_card {
            assert!(
                preview.sound_works(),
                "there is no sound output to test with"
            );
            println!(
                "    the clock reached {:.2} s in {seconds:.2} s of real time",
                reached.as_seconds()
            );
        }
        // What each layer should have shown between where it started and where playback got to.
        let in_the_stretch = |clip: &Clip| -> Vec<Ticks> {
            let info = &infos[clip.media.0 as usize];
            let until = source_time(clip, reached);
            frame_times(info)
                .into_iter()
                .filter(|time| *time >= clip.source.start && *time <= until)
                .collect()
        };
        let mut shown = Vec::new();
        let mut longest_gap_in_source = 0.0_f64;
        for clip in &stack_seen {
            let times = in_the_stretch(clip);
            for pair in times.windows(2) {
                longest_gap_in_source =
                    longest_gap_in_source.max((pair[1].0 - pair[0].0) as f64 / 705_600_000.0);
            }
            shown.push((
                counts.get(&clip.id).copied().unwrap_or(0),
                times.len() as u64,
            ));
        }
        Run {
            seconds,
            shown,
            longest_gap: longest_gap.as_secs_f64(),
            longest_gap_in_source,
            draws_per_second: draws as f64 / seconds,
            slowest_draw_ms: slowest_draw.as_secs_f64() * 1000.0,
        }
    }

    /// Prints a run and says whether its layers kept up.
    fn report(name: &str, run: &Run) -> bool {
        let counts: Vec<String> = run
            .shown
            .iter()
            .map(|(shown, total)| format!("{shown}/{total}"))
            .collect();
        // Allows for the start, when nothing shows yet, and for the odd late picture.
        let smooth = run
            .shown
            .iter()
            .all(|(shown, total)| *shown as f64 >= *total as f64 * 0.95);
        println!(
            "{name:<34} pictures shown {}, longest wait {:.0} ms (the videos' own: {:.0} ms), {:.0} draws/s, slowest draw {:.1} ms, over {:.1} s: {}",
            counts.join(" / "),
            run.longest_gap * 1000.0,
            run.longest_gap_in_source * 1000.0,
            run.draws_per_second,
            run.slowest_draw_ms,
            run.seconds,
            if smooth { "PASS" } else { "MISS" },
        );
        smooth
    }

    /// Exports real recordings in layers, says how fast, and compares the export with the preview.
    /// `cargo test --release -p bareclip-app real_recordings_export_like_the_preview -- --ignored --nocapture`
    #[test]
    #[ignore = "needs BARECLIP_LAYER_FILES and a GPU"]
    fn real_recordings_export_like_the_preview() {
        let Some(paths) = real_files() else {
            eprintln!("set BARECLIP_LAYER_FILES");
            return;
        };
        let infos: Vec<MediaInfo> = paths
            .iter()
            .map(|path| probe(path).expect("the video can be read"))
            .collect();
        let tracks = gate_layout(&infos, Ticks(0));
        let size_of = |id: MediaId| {
            infos
                .get(id.0 as usize)
                .map(|info| (info.video.width, info.video.height))
        };
        let canvas = tracks.canvas(&size_of).expect("there is a canvas");
        let mut compositor = Compositor::new(canvas.0, canvas.1).expect("the GPU opens");
        let output =
            std::env::temp_dir().join(format!("bareclip-real-layers-{}.mp4", std::process::id()));
        let stretches = tracks
            .stretches(canvas, &size_of)
            .into_iter()
            .map(|stretch| bareclip_export::LayeredStretch {
                start: stretch.start,
                end: stretch.end,
                stack: stretch
                    .stack
                    .iter()
                    .map(|clip| bareclip_export::LayeredClip {
                        key: clip.id.0,
                        input: paths[clip.media.0 as usize].clone(),
                        start: clip.start,
                        source: clip.source,
                        placement: clip.placement,
                        speed: clip.speed,
                    })
                    .collect(),
            })
            .collect();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let sound: Vec<bareclip_media::ExportPart> = tracks
            .played()
            .clips()
            .iter()
            .map(|clip| bareclip_media::ExportPart {
                input: paths[clip.media.0 as usize].clone(),
                frames: Arc::new(
                    bareclip_media::scan_frames(&paths[clip.media.0 as usize], &cancel, |_| {})
                        .expect("it can be scanned"),
                ),
                range: clip.source,
                gap_before: Ticks(0),
            })
            .collect();
        let request = bareclip_export::LayeredRequest {
            output: output.clone(),
            stretches,
            sound: bareclip_export::SoundPlan::as_recorded(&sound, &infos),
            canvas: None,
            background: [0, 0, 0],
            quality: Default::default(),
            rate: Default::default(),
        };
        let report = bareclip_export::export_layered(&request, &cancel, &|_| {})
            .expect("the export succeeds");
        let length = tracks.duration().as_seconds();
        println!(
            "Exported {length:.1} s at {}x{} {:.2} fps in {:.1} s ({:.1}x real time), {} drawn, {} direct, {} repeated, encoder {}",
            report.width,
            report.height,
            report.frame_rate,
            report.elapsed.as_secs_f64(),
            length / report.elapsed.as_secs_f64(),
            report.composited_frames,
            report.direct_frames,
            report.repeated_frames,
            report.encoder,
        );
        let out = probe(&output).expect("the export can be read");
        println!(
            "The file: {:.2} s, {} sound tracks, {:.1} MB",
            out.duration.as_seconds(),
            out.audio.len(),
            out.size_bytes as f64 / 1_000_000.0,
        );

        for moment in [0.2, 0.4, 0.6, 0.8] {
            let at = seconds(length * moment);
            let previewed = drawn_by_the_preview(&tracks, &infos, at, &mut compositor);
            // Recordings don't have a frame at every moment, so a frame next to this one can be the
            // true match. A wrong place or colour would differ from all of them.
            let mut reader =
                bareclip_media::PictureReader::open(&output).expect("the export opens");
            reader
                .seek(Ticks((at.0 - seconds(1.0).0).max(0)))
                .expect("it seeks");
            let mut nearby = Vec::new();
            while let Some(picture) = reader.next_picture().expect("it decodes") {
                let off = picture.time().as_seconds() - at.as_seconds();
                if off > 0.07 {
                    break;
                }
                if off < -0.07 {
                    continue;
                }
                let layer = Layer {
                    picture: picture.view().into(),
                    rect: bareclip_render::fit_inside(picture.size(), canvas),
                    opacity: 1.0,
                    look: bareclip_render::PictureLook::PLAIN,
                };
                compositor.draw_for_display(&[layer]).expect("it draws");
                let shown = compositor.read_display().expect("it reads back");
                nearby.push((off, average_difference(&previewed, &shown)));
            }
            let found: Vec<String> = nearby
                .iter()
                .map(|(off, difference)| format!("{:+.0} ms: {difference:.2}", off * 1000.0))
                .collect();
            println!(
                "At {:.1} s, difference from the preview by export frame: {}",
                at.as_seconds(),
                found.join(", ")
            );
        }
        let mut reader = bareclip_media::PictureReader::open(&output).expect("the export opens");
        let mut pictures = 0;
        while reader.next_picture().expect("it decodes").is_some() {
            pictures += 1;
        }
        println!(
            "It holds {pictures} pictures, and {} were written",
            report.frames
        );
        assert_eq!(pictures, report.frames);
        let _ = std::fs::remove_file(&output);
    }

    fn real_files() -> Option<Vec<PathBuf>> {
        let list = std::env::var("BARECLIP_LAYER_FILES").ok()?;
        Some(list.split(';').map(PathBuf::from).collect())
    }

    /// The gate for layers: several videos playing against a real clock, with one full-size and
    /// two smaller. Set `BARECLIP_LAYER_FILES` to `a.mkv;b.mp4;c.mp4` to try real recordings.
    /// Layers against the real sound card's clock, with every clip's sound being decoded and mixed.
    /// Nothing is heard, because everything is at no volume. Only meaningful in a release build:
    /// `$env:BARECLIP_LAYER_FILES = 'a.mkv;b.mp4;c.mp4'; cargo test --release -p bareclip-app layers_keep_up_on_the_sound_cards_clock -- --ignored --nocapture`
    #[test]
    #[ignore = "needs BARECLIP_LAYER_FILES, a GPU and a sound output"]
    fn layers_keep_up_on_the_sound_cards_clock() {
        let Some(files) = real_files() else {
            eprintln!("set BARECLIP_LAYER_FILES");
            return;
        };
        let run_for = Duration::from_secs(20);
        let run = run_against(&files, run_for, Ticks(0), true);
        let smooth = report("Real recordings, sound card clock", &run);
        let late = run_against(&files, run_for, seconds(5.0), true);
        report("The same, overlays arriving at 5 s", &late);
        assert!(smooth);
    }

    /// Only meaningful in a release build:
    /// `cargo test --release -p bareclip-app layers_hold_their_frame_rate -- --ignored --nocapture`
    #[test]
    #[ignore = "needs a real GPU and an NVENC-capable ffmpeg, and takes about a minute"]
    fn layers_hold_their_frame_rate() {
        let run_for = Duration::from_secs(20);
        if let Some(files) = real_files() {
            let run = real_time_run(&files, run_for, Ticks(0));
            let smooth = report("Real recordings", &run);
            let late = real_time_run(&files, run_for, seconds(5.0));
            report("Real, overlays arriving at 5 s", &late);
            assert!(smooth);
            return;
        }
        let sources: Vec<PathBuf> = [("a", "testsrc2"), ("b", "testsrc"), ("c", "smptebars")]
            .iter()
            .map(|(name, pattern)| busy_video(name, (1920, 1080), pattern))
            .collect();
        let at_1080 = report("Three 1080p60", &real_time_run(&sources, run_for, Ticks(0)));
        report(
            "1080p60, overlays arriving at 5 s",
            &real_time_run(&sources, run_for, seconds(5.0)),
        );
        let sources: Vec<PathBuf> = [("a", "testsrc2"), ("b", "testsrc"), ("c", "smptebars")]
            .iter()
            .map(|(name, pattern)| busy_video(name, (2560, 1440), pattern))
            .collect();
        // Not a target of the plan, but what the footage is really like.
        report(
            "Three 1440p60 (for information)",
            &real_time_run(&sources, run_for, Ticks(0)),
        );
        assert!(at_1080);
    }
}
