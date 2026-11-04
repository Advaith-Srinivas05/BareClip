//! Wording shown to the user.

use std::{
    cmp::Ordering,
    path::{Path, PathBuf},
};

use bareclip_core::{project, splice::CutMode, time::Ticks};
use bareclip_export::{FullPlan, FullReport, ProjectReport};
use bareclip_media::{AudioInfo, ExportReport, MediaInfo, Quality};

pub const VIDEO_EXTENSIONS: [&str; 6] = ["mp4", "mkv", "mov", "webm", "avi", "ts"];
/// Sound with no picture.
pub const AUDIO_EXTENSIONS: [&str; 7] = ["mp3", "wav", "flac", "m4a", "aac", "ogg", "opus"];
/// Everything that can be added to a project: videos, sound on its own, and images.
pub const MEDIA_EXTENSIONS: [&str; 19] = [
    "mp4", "mkv", "mov", "webm", "avi", "ts", "mp3", "wav", "flac", "m4a", "aac", "ogg", "opus",
    "jpg", "jpeg", "png", "webp", "bmp", "gif",
];

/// A project's name as Windows will accept it for a file.
pub fn safe_file_name(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '-'
            } else {
                c
            }
        })
        .collect();
    let cleaned = replaced.trim().trim_end_matches(['.', ' ']);
    if cleaned.is_empty() {
        return project::DEFAULT_NAME.to_owned();
    }
    // These stand for devices on Windows, even with a type after them.
    let stem = cleaned.split('.').next().unwrap_or(cleaned).trim_end();
    let reserved = ["CON", "PRN", "AUX", "NUL"]
        .iter()
        .any(|device| stem.eq_ignore_ascii_case(device))
        || ["COM", "LPT"].iter().any(|device| {
            stem.len() == 4
                && stem.is_char_boundary(3)
                && stem[..3].eq_ignore_ascii_case(device)
                && matches!(stem.as_bytes()[3], b'1'..=b'9')
        });
    if reserved {
        format!("{cleaned} video")
    } else {
        cleaned.to_owned()
    }
}

/// The file an export is saved as unless the user picks another: named after the project,
/// beside the first video and of the same type. A video type typed into the name isn't doubled.
pub fn default_output_path(project_name: &str, first_video: &Path) -> PathBuf {
    let name = safe_file_name(project_name);
    let typed = name.rsplit_once('.').filter(|(_, typed)| {
        VIDEO_EXTENSIONS
            .iter()
            .any(|known| known.eq_ignore_ascii_case(typed))
    });
    let stem = typed.map_or(name.as_str(), |(stem, _)| stem);
    let stem = if stem.trim().is_empty() {
        project::DEFAULT_NAME
    } else {
        stem
    };
    let extension = first_video
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("mp4");
    first_video.with_file_name(format!("{stem}.{extension}"))
}

pub fn file_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    if value < 1024.0 {
        return format!("{bytes} bytes");
    }
    let mut unit = 0;
    value /= 1024.0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// A length of time for humans, such as `1 min 24 s`.
pub fn short_duration(time: Ticks) -> String {
    let total = time.as_seconds().max(0.0);
    if total < 60.0 {
        return format!("{total:.1} s");
    }
    let whole = total.round() as u64;
    let (hours, minutes, seconds) = (whole / 3600, whole / 60 % 60, whole % 60);
    if hours > 0 {
        format!("{hours} h {minutes} min")
    } else {
        format!("{minutes} min {seconds} s")
    }
}

/// How long something took, for showing after it finishes.
pub fn elapsed(seconds: f64) -> String {
    if seconds < 60.0 {
        format!("{seconds:.1} seconds")
    } else {
        short_duration(Ticks::from_seconds(seconds))
    }
}

pub fn codec_name(codec: &str) -> String {
    match codec {
        "h264" => "H.264".to_owned(),
        "hevc" => "HEVC".to_owned(),
        "av1" => "AV1".to_owned(),
        "aac" => "AAC".to_owned(),
        "opus" => "Opus".to_owned(),
        "mp3" => "MP3".to_owned(),
        other => other.to_uppercase(),
    }
}

pub fn video_summary(info: &MediaInfo) -> String {
    if info.is_still() {
        return format!(
            "{}, {} x {}",
            image_kind(&info.video.codec),
            info.video.width,
            info.video.height
        );
    }
    format!(
        "{}, {} x {}, {:.2} fps",
        codec_name(&info.video.codec),
        info.video.width,
        info.video.height,
        info.video.frame_rate
    )
}

/// The usual name for a kind of image.
fn image_kind(codec: &str) -> String {
    match codec {
        "mjpeg" => "JPEG".to_owned(),
        other => other.to_uppercase(),
    }
}

pub fn audio_track_summary(position: usize, track: &AudioInfo) -> String {
    let name = track
        .title
        .clone()
        .unwrap_or_else(|| format!("Track {}", position + 1));
    let channels = match track.channels {
        1 => "mono".to_owned(),
        2 => "stereo".to_owned(),
        other => format!("{other} channels"),
    };
    format!("{name}: {}, {channels}", codec_name(&track.codec))
}

/// A clip's length while it is being adjusted, precise enough to see each change.
pub fn clip_length(time: Ticks) -> String {
    if time.as_seconds() < 60.0 {
        format!("{:.2} s", time.as_seconds().max(0.0))
    } else {
        short_duration(time)
    }
}

/// What an export is about to do, in terms of what matters to the user.
pub fn plan_summary(copied: usize, encoded: usize) -> String {
    if encoded == 0 {
        format!("All {copied} frames are copied untouched, so this will be very quick.")
    } else if copied == 0 {
        format!("All {encoded} frames are re-encoded, because they sit between two keyframes.")
    } else {
        format!(
            "{copied} frames are copied untouched and {encoded} around the cuts are re-encoded."
        )
    }
}

/// The warning shown before a video and its clips leave the project.
pub fn remove_warning(name: &str, clips: usize) -> String {
    match clips {
        1 => format!("{name} has 1 clip on the timeline, which will be removed too."),
        count => format!("{name} has {count} clips on the timeline, which will be removed too."),
    }
}

/// The warning shown before several videos and their clips leave the project.
pub fn remove_many_warning(videos: usize, clips: usize) -> String {
    match clips {
        1 => {
            format!("These {videos} videos have 1 clip on the timeline, which will be removed too.")
        }
        count => format!(
            "These {videos} videos have {count} clips on the timeline, which will be removed too."
        ),
    }
}

/// The warning shown before exporting a timeline with dead space on it.
pub fn gap_warning(gaps: usize, total: Ticks) -> String {
    let length = clip_length(total);
    match gaps {
        1 => format!("1 gap of {length} will export as black picture and silence."),
        count => {
            format!("{count} gaps, {length} in total, will export as black picture and silence.")
        }
    }
}

pub fn encoder_name(encoder: &str) -> &str {
    match encoder {
        name if name.ends_with("_nvenc") => "your NVIDIA GPU",
        name if name.ends_with("_amf") => "your AMD GPU",
        name if name.ends_with("_qsv") => "your Intel GPU",
        name if name.ends_with("_mf") => "Windows",
        _ => "the processor",
    }
}

/// What a full export is going to write.
pub fn full_export_plan(plan: &FullPlan) -> String {
    let settings = &plan.settings;
    format!(
        "Every frame is redrawn and saved as an H.264 MP4 at {} x {} and {} fps. This takes longer than copying. {}",
        settings.width,
        settings.height,
        frame_rate(plan.frame_rate()),
        quality_note(settings.quality),
    )
}

/// What a quality is called in the export window.
pub fn quality_name(quality: Quality) -> &'static str {
    match quality {
        Quality::MatchSource => "Match source",
        Quality::Balanced => "Balanced",
        Quality::Smaller => "Smaller file",
    }
}

/// What a quality will do, in a sentence.
pub fn quality_note(quality: Quality) -> &'static str {
    match quality {
        Quality::MatchSource => {
            "Looks the same as the source. The file can come out larger than it."
        }
        Quality::Balanced => "Hard to tell from the source, at about its size or smaller.",
        Quality::Smaller => "About half the size of Balanced. The loss can show on some footage.",
    }
}

/// A frame rate with no needless decimals, such as `60` or `59.94`.
fn frame_rate(rate: f64) -> String {
    if (rate - rate.round()).abs() < 0.001 {
        format!("{rate:.0}")
    } else {
        format!("{rate:.2}")
    }
}

/// What happened in a finished export that redrew every frame.
fn full_export_summary(report: &FullReport) -> Vec<String> {
    let mut lines = vec![
        format!("Finished in {}.", elapsed(report.elapsed.as_secs_f64())),
        format!(
            "{} frames written at {} x {} and {} fps, encoded using {}.",
            report.frames,
            report.width,
            report.height,
            frame_rate(report.frame_rate),
            encoder_name(&report.encoder)
        ),
    ];
    if report.composited_frames > 0 {
        lines.push(format!(
            "{} frames were fitted to that size on your graphics chip.",
            report.composited_frames
        ));
    }
    if report.black_frames > 0 {
        lines.push(format!(
            "Dead space written as {} black frames.",
            report.black_frames
        ));
    }
    lines
}

/// What happened in a finished export, whichever route it took.
pub fn export_summary(report: &ProjectReport) -> Vec<String> {
    match report {
        ProjectReport::Fast(report) => fast_export_summary(report),
        ProjectReport::Full(report) => full_export_summary(report),
        ProjectReport::Sound(report) => vec![
            format!("Finished in {}.", elapsed(report.elapsed.as_secs_f64())),
            format!(
                "{} of sound saved, with no picture.",
                elapsed(report.length.as_seconds())
            ),
        ],
    }
}

/// What happened in a finished export that copied the video.
fn fast_export_summary(report: &ExportReport) -> Vec<String> {
    let mut lines = vec![format!(
        "Finished in {}.",
        elapsed(report.elapsed.as_secs_f64())
    )];
    lines.push(match (&report.encoder, report.encoded_frames) {
        (Some(encoder), encoded) if encoded > 0 => format!(
            "{} frames copied untouched, {encoded} re-encoded using {}.",
            report.copied_frames,
            encoder_name(encoder)
        ),
        _ => format!("All {} frames copied untouched.", report.copied_frames),
    });
    if report.black_frames > 0 {
        lines.push(format!(
            "Dead space written as {} black frames.",
            report.black_frames
        ));
    }
    if report.mode_requested == CutMode::FrameExact && report.mode_used == CutMode::Keyframe {
        lines.push(
            "Frame-exact cuts are not available for this video, so the cuts moved outward to \
             the nearest keyframes."
                .to_owned(),
        );
    }
    lines
}

/// A project's name as the top strip shows it, marked if it has unsaved changes.
pub fn project_title(name: &str, unsaved: bool) -> String {
    if unsaved {
        format!("{name} *")
    } else {
        name.to_owned()
    }
}

/// Orders names the way File Explorer does: ignoring case, with runs of digits compared
/// as numbers, so "clip 2" comes before "clip 10".
pub fn natural_order(a: &str, b: &str) -> Ordering {
    let mut left = a.chars().peekable();
    let mut right = b.chars().peekable();
    loop {
        match (left.peek().copied(), right.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(l), Some(r)) if l.is_ascii_digit() && r.is_ascii_digit() => {
                let order = compare_digit_runs(&digit_run(&mut left), &digit_run(&mut right));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(l), Some(r)) => {
                let order = l.to_lowercase().cmp(r.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                left.next();
                right.next();
            }
        }
    }
}

fn digit_run(chars: &mut std::iter::Peekable<std::str::Chars>) -> String {
    let mut run = String::new();
    while let Some(digit) = chars.next_if(char::is_ascii_digit) {
        run.push(digit);
    }
    run
}

/// Compares digits by value without parsing them, so any length of number works.
fn compare_digit_runs(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.trim_start_matches('0'), b.trim_start_matches('0'));
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_sort_the_way_file_explorer_sorts_them() {
        let mut names = [
            "clip 10.mp4",
            "Clip 2.mp4",
            "clip 1.mp4",
            "another.mp4",
            "clip.mp4",
        ];

        names.sort_by(|a, b| natural_order(a, b));

        assert_eq!(
            names,
            [
                "another.mp4",
                "clip 1.mp4",
                "Clip 2.mp4",
                "clip 10.mp4",
                "clip.mp4"
            ]
        );
    }

    #[test]
    fn numbers_compare_by_value_at_any_length() {
        assert_eq!(natural_order("a007", "a7b"), Ordering::Less);
        assert_eq!(
            natural_order("a99999999999999999999999", "a100000000000000000000000"),
            Ordering::Less
        );
        assert_eq!(natural_order("same", "same"), Ordering::Equal);
    }
    use std::time::Duration;

    #[test]
    fn the_warning_for_several_videos_counts_their_clips() {
        assert_eq!(
            remove_many_warning(2, 1),
            "These 2 videos have 1 clip on the timeline, which will be removed too."
        );
        assert_eq!(
            remove_many_warning(3, 5),
            "These 3 videos have 5 clips on the timeline, which will be removed too."
        );
    }

    #[test]
    fn the_gap_warning_says_how_much_dead_space_there_is() {
        assert_eq!(
            gap_warning(1, Ticks::from_seconds(2.5)),
            "1 gap of 2.50 s will export as black picture and silence."
        );
        assert_eq!(
            gap_warning(3, Ticks::from_seconds(10.0)),
            "3 gaps, 10.00 s in total, will export as black picture and silence."
        );
    }

    fn full_report() -> FullReport {
        FullReport {
            width: 1920,
            height: 1080,
            frame_rate: 60000.0 / 1001.0,
            frames: 600,
            composited_frames: 240,
            direct_frames: 300,
            repeated_frames: 0,
            black_frames: 60,
            encoder: "h264_nvenc".to_owned(),
            gpu: Some("NVIDIA GeForce RTX 3060".to_owned()),
            audio_tracks: 2,
            elapsed: Duration::from_millis(4000),
        }
    }

    #[test]
    fn a_full_export_reports_what_it_wrote() {
        let lines = export_summary(&ProjectReport::Full(full_report()));

        assert!(lines[1].contains("600 frames"), "{lines:?}");
        assert!(lines[1].contains("1920 x 1080"), "{lines:?}");
        assert!(lines[1].contains("59.94 fps"), "{lines:?}");
        assert!(lines[1].contains("your NVIDIA GPU"), "{lines:?}");
        assert!(lines[2].contains("240 frames"), "{lines:?}");
        assert!(lines[3].contains("60 black frames"), "{lines:?}");
    }

    #[test]
    fn a_full_export_that_needed_no_fitting_doesnt_mention_it() {
        let mut report = full_report();
        report.composited_frames = 0;
        report.black_frames = 0;

        let lines = export_summary(&ProjectReport::Full(report));

        assert_eq!(lines.len(), 2, "{lines:?}");
    }

    #[test]
    fn whole_frame_rates_have_no_decimals() {
        assert_eq!(frame_rate(60.0), "60");
        assert_eq!(frame_rate(30000.0 / 1001.0), "29.97");
    }

    fn report() -> ExportReport {
        ExportReport {
            mode_requested: CutMode::FrameExact,
            mode_used: CutMode::FrameExact,
            sections: 1,
            copied_frames: 100,
            black_frames: 0,
            encoded_frames: 20,
            encoder: Some("h264_nvenc".to_owned()),
            audio_tracks: 1,
            elapsed: Duration::from_millis(2500),
        }
    }

    #[test]
    fn clip_lengths_stay_precise_until_they_get_long() {
        assert_eq!(clip_length(Ticks::from_seconds(5.9)), "5.90 s");
        assert_eq!(clip_length(Ticks::from_seconds(84.0)), "1 min 24 s");
    }

    #[test]
    fn dead_space_is_mentioned_after_an_export_that_had_some() {
        let mut with_gap = report();
        with_gap.black_frames = 30;

        assert!(
            fast_export_summary(&with_gap)
                .iter()
                .any(|line| line.contains("30 black frames"))
        );
        assert!(
            !fast_export_summary(&report())
                .iter()
                .any(|line| line.contains("black"))
        );
    }

    #[test]
    fn the_export_takes_the_project_name_beside_the_first_video_with_its_type() {
        assert_eq!(
            default_output_path("untitled", Path::new("C:/videos/match.mkv")),
            Path::new("C:/videos/untitled.mkv")
        );
        assert_eq!(
            default_output_path("Final round", Path::new("C:/videos/match.mp4")),
            Path::new("C:/videos/Final round.mp4")
        );
    }

    #[test]
    fn a_first_video_without_a_type_exports_as_mp4() {
        assert_eq!(
            default_output_path("untitled", Path::new("C:/videos/match")),
            Path::new("C:/videos/untitled.mp4")
        );
    }

    #[test]
    fn a_video_type_typed_into_the_name_is_not_doubled() {
        let first = Path::new("C:/videos/match.mkv");
        assert_eq!(
            default_output_path("clip.mkv", first),
            Path::new("C:/videos/clip.mkv")
        );
        assert_eq!(
            default_output_path("clip.MP4", first),
            Path::new("C:/videos/clip.mkv")
        );
        assert_eq!(
            default_output_path(".mp4", first),
            Path::new("C:/videos/untitled.mkv")
        );
    }

    #[test]
    fn dots_that_are_part_of_the_name_stay() {
        assert_eq!(
            default_output_path("v1.2 final", Path::new("C:/videos/match.mp4")),
            Path::new("C:/videos/v1.2 final.mp4")
        );
    }

    #[test]
    fn characters_windows_refuses_are_replaced() {
        assert_eq!(safe_file_name("a/b:c*d?"), "a-b-c-d-");
        assert_eq!(safe_file_name("line\nbreak"), "line-break");
        assert_eq!(safe_file_name("  spaced  "), "spaced");
        assert_eq!(safe_file_name("ends with dots..."), "ends with dots");
    }

    #[test]
    fn a_name_with_nothing_usable_falls_back_to_the_default() {
        assert_eq!(safe_file_name("..."), project::DEFAULT_NAME);
        assert_eq!(safe_file_name("   "), project::DEFAULT_NAME);
    }

    #[test]
    fn names_of_windows_devices_are_kept_from_being_devices() {
        for device in ["CON", "nul", "Com1", "LPT9", "aux.final"] {
            assert_eq!(safe_file_name(device), format!("{device} video"));
        }
        for fine in ["console", "com0", "com10", "lpt", "nullable", "c\u{f6}n1"] {
            assert_eq!(safe_file_name(fine), fine);
        }
    }

    #[test]
    fn non_english_names_are_kept() {
        assert_eq!(
            safe_file_name("\u{c95}\u{ca8}\u{ccd}\u{ca8}\u{ca1} \u{52d5}\u{753b}"),
            "\u{c95}\u{ca8}\u{ccd}\u{ca8}\u{ca1} \u{52d5}\u{753b}"
        );
    }

    #[test]
    fn sizes_use_a_readable_unit() {
        assert_eq!(file_size(512), "512 bytes");
        assert_eq!(file_size(2048), "2.0 KB");
        assert_eq!(file_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(file_size(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn durations_use_a_readable_form() {
        assert_eq!(short_duration(Ticks::from_seconds(12.34)), "12.3 s");
        assert_eq!(short_duration(Ticks::from_seconds(84.0)), "1 min 24 s");
        assert_eq!(short_duration(Ticks::from_seconds(7500.0)), "2 h 5 min");
    }

    #[test]
    fn elapsed_time_is_written_out() {
        assert_eq!(elapsed(2.5), "2.5 seconds");
        assert_eq!(elapsed(90.0), "1 min 30 s");
    }

    #[test]
    fn known_codecs_get_their_usual_names() {
        assert_eq!(codec_name("h264"), "H.264");
        assert_eq!(codec_name("vp9"), "VP9");
    }

    #[test]
    fn audio_tracks_use_their_title_or_position() {
        let mut track = AudioInfo {
            stream_index: 1,
            codec: "aac".to_owned(),
            channels: 2,
            sample_rate: 48_000,
            title: Some("Microphone".to_owned()),
            language: None,
        };
        assert_eq!(audio_track_summary(0, &track), "Microphone: AAC, stereo");

        track.title = None;
        track.channels = 6;
        assert_eq!(audio_track_summary(1, &track), "Track 2: AAC, 6 channels");
    }

    #[test]
    fn the_plan_describes_copying_and_encoding() {
        assert!(plan_summary(100, 0).contains("All 100 frames are copied"));
        assert!(plan_summary(0, 12).contains("All 12 frames are re-encoded"));
        assert!(plan_summary(100, 12).contains("100 frames are copied"));
    }

    #[test]
    fn the_remove_warning_counts_the_clips() {
        assert!(remove_warning("a.mp4", 1).contains("1 clip on"));
        assert!(remove_warning("a.mp4", 3).contains("3 clips on"));
    }

    #[test]
    fn the_report_says_what_did_the_work() {
        let lines = fast_export_summary(&report());

        assert_eq!(lines[0], "Finished in 2.5 seconds.");
        assert!(lines[1].contains("20 re-encoded using your NVIDIA GPU"));
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn a_clean_copy_does_not_mention_encoding() {
        let mut clean = report();
        clean.encoded_frames = 0;
        clean.encoder = None;

        assert_eq!(
            fast_export_summary(&clean)[1],
            "All 100 frames copied untouched."
        );
    }

    #[test]
    fn falling_back_to_keyframe_cuts_is_never_silent() {
        let mut fallback = report();
        fallback.mode_used = CutMode::Keyframe;
        fallback.encoded_frames = 0;
        fallback.encoder = None;

        let lines = fast_export_summary(&fallback);

        assert!(lines[2].contains("moved outward"));
    }

    #[test]
    fn software_encoding_is_named_plainly() {
        assert_eq!(encoder_name("libopenh264"), "the processor");
        assert_eq!(encoder_name("hevc_amf"), "your AMD GPU");
    }
}
