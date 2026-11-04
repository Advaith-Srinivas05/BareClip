//! Which way a project gets exported, and what a full export will produce.

pub use bareclip_core::placement::final_size;
use bareclip_core::splice::FrameIndex;
use bareclip_media::{MediaInfo, SoundLabel, VideoSettings, can_write_gaps, join_problem};

/// How an export will work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// Copies the video, re-encoding only what it must.
    Fast,
    /// Redraws every frame, for this reason.
    Full(String),
}

/// How far apart two videos' average rates can be and still count as the same. Recordings with
/// uneven timing average below what they aimed for, and shouldn't lose the fast route over it.
const SAME_RATE_RATIO: f64 = 1.15;

/// Why two videos' frame rates differ enough to matter. Copying them into one file makes it change
/// pace partway through, which some software handles badly.
fn rate_difference(first: &MediaInfo, other: &MediaInfo) -> Option<String> {
    let (a, b) = (first.video.frame_rate, other.video.frame_rate);
    if a <= 0.0 || b <= 0.0 || a.max(b) / a.min(b) <= SAME_RATE_RATIO {
        return None;
    }
    let name = other.path.file_name().map_or_else(
        || "A video".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    Some(format!("{name} runs at {b:.0} fps instead of {a:.0} fps."))
}

/// Why these videos' frame rates make joining them a choice between copying and redrawing.
pub fn mixed_rates(videos: &[MediaInfo]) -> Option<String> {
    let (first, others) = videos.split_first()?;
    others
        .iter()
        .find_map(|other| rate_difference(first, other))
}

/// Whether copying is the better start for videos whose rates differ. Videos already uneven inside
/// are the same kind of file once joined, but copying steady ones adds a change of pace that
/// neither had.
pub fn copying_suits<'a>(mut frames: impl Iterator<Item = &'a FrameIndex>) -> bool {
    frames.all(|frames| !frames.is_steady())
}

/// Why a project with clips resized, moved or showing together can't be copied.
pub const LAYERED_REASON: &str =
    "Clips are resized, moved, see-through or showing together, so every frame is redrawn.";

/// Picks the route: fast whenever that is possible, because nothing beats copying.
pub fn route_for(videos: &[MediaInfo], has_gaps: bool) -> Route {
    let Some((first, others)) = videos.split_first() else {
        return Route::Fast;
    };
    if let Some(reason) = others.iter().find_map(|other| join_problem(first, other)) {
        return Route::Full(reason);
    }
    if has_gaps && !can_write_gaps(&first.video.codec) {
        return Route::Full(format!(
            "Dead space can't be added to {} video by copying it.",
            first.video.codec
        ));
    }
    Route::Fast
}

const USUAL_RATES: [(i32, i32); 16] = [
    (24000, 1001),
    (24, 1),
    (25, 1),
    (30000, 1001),
    (30, 1),
    (48, 1),
    (50, 1),
    (60000, 1001),
    (60, 1),
    (72, 1),
    (90, 1),
    (100, 1),
    (120000, 1001),
    (120, 1),
    (144, 1),
    (240, 1),
];
/// Recordings with uneven timing average just under their real rate, such as 59.87 for 60.
const CLOSE_ENOUGH: f64 = 0.01;

/// The constant rate for a finished video: the fastest video's, so nothing loses smoothness.
/// A rate close to a usual one becomes it, and any other goes up to the next whole number.
pub fn frame_rate_for(rates: &[f64]) -> (i32, i32) {
    let fastest = rates.iter().copied().fold(0.0, f64::max);
    if fastest <= 0.0 {
        return (30, 1);
    }
    let closest = USUAL_RATES.iter().copied().min_by(|a, b| {
        let off = |rate: (i32, i32)| (f64::from(rate.0) / f64::from(rate.1) - fastest).abs();
        off(*a).total_cmp(&off(*b))
    });
    if let Some(rate) = closest {
        let value = f64::from(rate.0) / f64::from(rate.1);
        if (value - fastest).abs() / value <= CLOSE_ENOUGH {
            return rate;
        }
    }
    (fastest.ceil() as i32, 1)
}

/// The sound tracks videos need kept apart, as many as the video with the most. A track's name
/// comes from the first video that gives it one.
pub fn sound_labels(videos: &[MediaInfo]) -> Vec<SoundLabel> {
    let track_count = videos
        .iter()
        .map(|info| info.audio.len())
        .max()
        .unwrap_or(0);
    (0..track_count)
        .map(|track| {
            let tracks = || videos.iter().filter_map(|info| info.audio.get(track));
            SoundLabel {
                title: tracks().find_map(|audio| audio.title.clone()),
                language: tracks().find_map(|audio| audio.language.clone()),
            }
        })
        .collect()
}

/// What a full export will write, chosen so that it matches the sources with no questions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FullPlan {
    pub settings: VideoSettings,
}

impl FullPlan {
    /// A plan at the largest video's size.
    pub fn for_videos(videos: &[MediaInfo]) -> Self {
        let sizes: Vec<(u32, u32)> = videos
            .iter()
            .map(|info| (info.video.width, info.video.height))
            .collect();
        Self::on_canvas(videos, final_size(&sizes).unwrap_or((2, 2)))
    }

    /// A plan at a size the project has chosen.
    pub fn on_canvas(videos: &[MediaInfo], (width, height): (u32, u32)) -> Self {
        let rates: Vec<f64> = videos.iter().map(|info| info.video.frame_rate).collect();

        let sound_tracks = sound_labels(videos);

        // Only for encoders with no quality setting: as much as the busiest video uses for
        // each pixel, with a margin, at the finished size.
        let pixels = f64::from(width) * f64::from(height);
        let bit_rate = videos
            .iter()
            .map(|info| {
                let seconds = info.duration.as_seconds().max(0.001);
                let own_pixels = f64::from(info.video.width.max(1) * info.video.height.max(1));
                info.size_bytes as f64 * 8.0 / seconds * pixels / own_pixels
            })
            .fold(2_000_000.0, f64::max)
            * 1.5;

        Self {
            settings: VideoSettings {
                width,
                height,
                frame_rate: frame_rate_for(&rates),
                bit_rate: bit_rate as usize,
                sound_tracks,
            },
        }
    }

    pub fn frame_rate(&self) -> f64 {
        let (num, den) = self.settings.frame_rate;
        f64::from(num) / f64::from(den)
    }
}

/// Why a video that could otherwise be copied can't be, because the project is another size.
pub fn size_problem(canvas: (u32, u32), video: &MediaInfo) -> Option<String> {
    let size = (video.video.width, video.video.height);
    if final_size(&[size]) == Some(canvas) {
        return None;
    }
    let name = video.path.file_name().map_or_else(
        || "A video".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    Some(format!(
        "{name} is {} x {} and the project is {} x {}, so it is fitted to the project.",
        size.0, size.1, canvas.0, canvas.1
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use bareclip_core::{
        splice::FrameEntry,
        time::{Ticks, TimeBase},
    };
    use bareclip_media::{AudioInfo, VideoInfo};

    fn audio(codec: &str, title: Option<&str>) -> AudioInfo {
        AudioInfo {
            stream_index: 1,
            codec: codec.to_owned(),
            channels: 2,
            sample_rate: 48000,
            title: title.map(str::to_owned),
            language: None,
        }
    }

    fn video(width: u32, height: u32, rate: f64, sound: Vec<AudioInfo>) -> MediaInfo {
        MediaInfo {
            path: PathBuf::from("a.mp4"),
            container: "mp4".to_owned(),
            size_bytes: 1_000_000,
            duration: Ticks::from_seconds(10.0),
            video: VideoInfo {
                stream_index: 0,
                codec: "h264".to_owned(),
                width,
                height,
                pixel_format: "yuv420p".to_owned(),
                time_base: TimeBase { num: 1, den: 90000 },
                frame_rate: rate,
                is_hdr: false,
            },
            audio: sound,
        }
    }

    #[test]
    fn videos_that_match_take_the_fast_route() {
        let videos = [
            video(1920, 1080, 60.0, vec![]),
            video(1920, 1080, 60.0, vec![]),
        ];

        assert_eq!(route_for(&videos, false), Route::Fast);
    }

    #[test]
    fn nothing_at_all_takes_the_fast_route() {
        assert_eq!(route_for(&[], false), Route::Fast);
    }

    #[test]
    fn a_different_size_takes_the_full_route_and_says_why() {
        let videos = [
            video(1920, 1080, 60.0, vec![]),
            video(1280, 720, 60.0, vec![]),
        ];

        let Route::Full(reason) = route_for(&videos, false) else {
            panic!("it can't be copied");
        };

        assert!(reason.contains("1280 x 720"), "{reason}");
    }

    #[test]
    fn clearly_different_frame_rates_are_a_choice_rather_than_a_route() {
        let videos = [
            video(1920, 1080, 30.0, vec![]),
            video(1920, 1080, 60.0, vec![]),
        ];

        assert_eq!(route_for(&videos, false), Route::Fast);
        let reason = mixed_rates(&videos).expect("the rates differ");
        assert!(reason.contains("60 fps instead of 30"), "{reason}");
    }

    fn frames_at(times: &[i64]) -> FrameIndex {
        FrameIndex::new(
            times
                .iter()
                .map(|time| FrameEntry {
                    pts: *time,
                    dts: *time,
                    duration: 1,
                    keyframe: false,
                })
                .collect(),
        )
    }

    #[test]
    fn copying_suits_videos_that_are_all_uneven_already() {
        let steady: Vec<i64> = (0..100).map(|frame| frame * 40).collect();
        let uneven: Vec<i64> = (0..100).map(|frame| frame * 40 + frame % 3 * 15).collect();
        let (steady, uneven) = (frames_at(&steady), frames_at(&uneven));

        assert!(copying_suits([&uneven, &uneven].into_iter()));
        assert!(!copying_suits([&steady, &steady].into_iter()));
        assert!(!copying_suits([&uneven, &steady].into_iter()));
    }

    #[test]
    fn a_recording_that_dipped_a_little_doesnt_count_as_a_different_rate() {
        let videos = [
            video(1920, 1080, 60.0, vec![]),
            video(1920, 1080, 56.3, vec![]),
        ];

        assert_eq!(route_for(&videos, false), Route::Fast);
        assert_eq!(mixed_rates(&videos), None);
    }

    #[test]
    fn a_different_number_of_sound_tracks_takes_the_full_route() {
        let videos = [
            video(1920, 1080, 60.0, vec![audio("aac", None)]),
            video(
                1920,
                1080,
                60.0,
                vec![audio("aac", None), audio("aac", None)],
            ),
        ];

        assert!(matches!(route_for(&videos, false), Route::Full(_)));
    }

    #[test]
    fn dead_space_in_a_format_that_cant_be_copied_takes_the_full_route() {
        bareclip_media::init().expect("FFmpeg starts");
        let mut old = video(640, 360, 30.0, vec![]);
        old.video.codec = "mpeg4".to_owned();
        let videos = [old];

        assert_eq!(route_for(&videos, false), Route::Fast);
        assert!(matches!(route_for(&videos, true), Route::Full(_)));
    }

    #[test]
    fn dead_space_in_an_ordinary_format_stays_on_the_fast_route() {
        bareclip_media::init().expect("FFmpeg starts");

        assert_eq!(
            route_for(&[video(1920, 1080, 60.0, vec![])], true),
            Route::Fast
        );
    }

    #[test]
    fn the_finished_video_takes_the_largest_size() {
        assert_eq!(
            final_size(&[(1280, 720), (2560, 1440), (1920, 1080)]),
            Some((2560, 1440))
        );
    }

    #[test]
    fn the_largest_is_judged_by_area_so_a_phone_video_counts() {
        assert_eq!(final_size(&[(1280, 720), (1080, 1920)]), Some((1080, 1920)));
    }

    #[test]
    fn the_first_wins_when_two_are_equally_large() {
        assert_eq!(
            final_size(&[(1920, 1080), (1080, 1920)]),
            Some((1920, 1080))
        );
    }

    #[test]
    fn odd_sizes_round_up_because_encoders_need_even_ones() {
        assert_eq!(final_size(&[(853, 481)]), Some((854, 482)));
    }

    #[test]
    fn no_videos_have_no_size() {
        assert_eq!(final_size(&[]), None);
    }

    #[test]
    fn the_finished_video_takes_the_fastest_rate() {
        assert_eq!(frame_rate_for(&[30.0, 60.0, 24.0]), (60, 1));
    }

    #[test]
    fn rates_near_a_usual_one_become_it() {
        assert_eq!(frame_rate_for(&[59.94]), (60000, 1001));
        assert_eq!(frame_rate_for(&[29.97]), (30000, 1001));
        assert_eq!(frame_rate_for(&[23.976]), (24000, 1001));
    }

    #[test]
    fn a_recording_with_uneven_timing_gets_the_rate_it_was_aiming_for() {
        assert_eq!(frame_rate_for(&[59.61]), (60000, 1001));
        assert_eq!(frame_rate_for(&[143.2]), (144, 1));
    }

    #[test]
    fn an_unusual_rate_goes_up_so_no_frames_are_lost() {
        assert_eq!(frame_rate_for(&[37.3]), (38, 1));
    }

    #[test]
    fn a_video_with_no_known_rate_gets_thirty() {
        assert_eq!(frame_rate_for(&[0.0]), (30, 1));
        assert_eq!(frame_rate_for(&[]), (30, 1));
    }

    #[test]
    fn the_plan_keeps_every_sound_track_and_its_name() {
        let videos = [
            video(1280, 720, 30.0, vec![audio("aac", None)]),
            video(
                1920,
                1080,
                60.0,
                vec![audio("aac", Some("Game")), audio("aac", Some("Mic"))],
            ),
        ];

        let settings = FullPlan::for_videos(&videos).settings;

        assert_eq!((settings.width, settings.height), (1920, 1080));
        assert_eq!(settings.frame_rate, (60, 1));
        assert_eq!(settings.sound_tracks.len(), 2);
        assert_eq!(settings.sound_tracks[0].title.as_deref(), Some("Game"));
        assert_eq!(settings.sound_tracks[1].title.as_deref(), Some("Mic"));
    }

    #[test]
    fn a_video_with_no_sound_has_no_sound_tracks() {
        let plan = FullPlan::for_videos(&[video(1280, 720, 30.0, vec![])]);

        assert!(plan.settings.sound_tracks.is_empty());
    }
}
