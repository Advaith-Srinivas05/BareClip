//! Wording shown to the user.

use std::path::{Path, PathBuf};

use bareclip_core::{splice::CutMode, time::Ticks};
use bareclip_media::{AudioInfo, ExportReport, MediaInfo};

/// The file a video's export is saved as unless the user picks another name.
pub fn default_output_path(input: &Path) -> PathBuf {
    let stem = input
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("video");
    let extension = input
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("mp4");
    input.with_file_name(format!("{stem}-trimmed.{extension}"))
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
    format!(
        "{}, {} x {}, {:.2} fps",
        codec_name(&info.video.codec),
        info.video.width,
        info.video.height,
        info.video.frame_rate
    )
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

/// What an export is about to do, in terms of what matters to the user.
pub fn plan_summary(copied: usize, encoded: usize) -> String {
    if encoded == 0 {
        format!("All {copied} frames are copied untouched, so this will be very quick.")
    } else if copied == 0 {
        format!(
            "All {encoded} frames are re-encoded, because this section sits between two keyframes."
        )
    } else {
        format!(
            "{copied} frames are copied untouched and {encoded} around the cuts are re-encoded."
        )
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

/// What happened in a finished export.
pub fn export_summary(report: &ExportReport) -> Vec<String> {
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
    if report.mode_requested == CutMode::FrameExact && report.mode_used == CutMode::Keyframe {
        lines.push(
            "Frame-exact cuts are not available for this video, so the cuts moved outward to \
             the nearest keyframes."
                .to_owned(),
        );
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn report() -> ExportReport {
        ExportReport {
            mode_requested: CutMode::FrameExact,
            mode_used: CutMode::FrameExact,
            sections: 1,
            copied_frames: 100,
            encoded_frames: 20,
            encoder: Some("h264_nvenc".to_owned()),
            audio_tracks: 1,
            elapsed: Duration::from_millis(2500),
        }
    }

    #[test]
    fn the_export_name_stays_beside_the_original_with_its_type() {
        assert_eq!(
            default_output_path(Path::new("C:/videos/match.mkv")),
            Path::new("C:/videos/match-trimmed.mkv")
        );
    }

    #[test]
    fn a_file_without_a_type_becomes_mp4() {
        assert_eq!(
            default_output_path(Path::new("C:/videos/match")),
            Path::new("C:/videos/match-trimmed.mp4")
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
    fn the_report_says_what_did_the_work() {
        let lines = export_summary(&report());

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
            export_summary(&clean)[1],
            "All 100 frames copied untouched."
        );
    }

    #[test]
    fn falling_back_to_keyframe_cuts_is_never_silent() {
        let mut fallback = report();
        fallback.mode_used = CutMode::Keyframe;
        fallback.encoded_frames = 0;
        fallback.encoder = None;

        let lines = export_summary(&fallback);

        assert!(lines[2].contains("moved outward"));
    }

    #[test]
    fn software_encoding_is_named_plainly() {
        assert_eq!(encoder_name("libopenh264"), "the processor");
        assert_eq!(encoder_name("hevc_amf"), "your AMD GPU");
    }
}
