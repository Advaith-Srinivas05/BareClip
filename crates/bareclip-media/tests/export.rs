//! Exports real video files and proves exactly what came out.

mod common;

use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use bareclip_core::splice::CutMode;
use bareclip_media::{ExportRequest, MediaError, export, probe};
use common::*;

fn expected(frames: std::ops::Range<u32>) -> Vec<u32> {
    frames.collect()
}

/// Copied frames must match the source exactly, which proves the stream around
/// them did not disturb how they decode.
fn assert_all_identical(source: &Path, output: &Path, frames: std::ops::Range<u32>) {
    let source = decode_video(source);
    let output = decode_video(output);
    for number in frames {
        assert_identical(&source, &output, number);
    }
}

fn copied_frames(input: &Path, start: u32, end: u32) -> std::ops::Range<u32> {
    let time_base = probe(input).expect("the clip can be read").video.time_base;
    frames_copied_within(
        &index_of(input),
        f64::from(time_base.num) / f64::from(time_base.den),
        start,
        end,
    )
}

#[test]
fn keyframe_aligned_cut_copies_every_frame_untouched() {
    let input = h264_mp4();
    let output = output_path("aligned", "mp4");

    let report = run_export(&input, &output, vec![range(1.0, 3.0)], CutMode::FrameExact);

    assert_eq!(report.encoded_frames, 0);
    assert_eq!(report.encoder, None);
    assert_eq!(numbers(&decode_video(&output)), expected(30..90));
    assert_all_identical(&input, &output, 30..90);
}

#[test]
fn frame_exact_cut_lands_on_the_exact_frames() {
    let input = h264_mp4();
    let output = output_path("exact", "mp4");

    let report = run_export(&input, &output, vec![range(0.5, 2.5)], CutMode::FrameExact);

    assert_eq!(report.mode_used, CutMode::FrameExact);
    assert_eq!(report.encoded_frames, 30);
    assert_eq!(report.copied_frames, 30);
    assert!(report.encoder.is_some());
    assert_eq!(numbers(&decode_video(&output)), expected(15..75));
}

#[test]
fn frames_copied_after_a_re_encoded_part_stay_identical() {
    let input = h264_mp4();
    let output = output_path("identical", "mp4");

    run_export(&input, &output, vec![range(0.5, 2.5)], CutMode::FrameExact);

    assert_all_identical(&input, &output, 30..60);
}

#[test]
fn a_cut_inside_one_keyframe_interval_works() {
    let input = h264_mp4();
    let output = output_path("inside", "mp4");

    let report = run_export(&input, &output, vec![range(1.2, 1.5)], CutMode::FrameExact);

    assert_eq!(report.copied_frames, 0);
    assert_eq!(numbers(&decode_video(&output)), expected(36..45));
}

#[test]
fn a_cut_reaching_the_end_keeps_the_last_frame() {
    let input = h264_mp4();
    let output = output_path("to-end", "mp4");

    run_export(&input, &output, vec![range(8.5, 10.0)], CutMode::FrameExact);

    assert_eq!(numbers(&decode_video(&output)), expected(255..300));
}

#[test]
fn several_sections_join_in_order_with_steady_timing() {
    let input = h264_mp4();
    let output = output_path("sections", "mp4");

    let report = run_export(
        &input,
        &output,
        vec![range(0.5, 1.5), range(5.2, 6.1)],
        CutMode::FrameExact,
    );

    let frames = decode_video(&output);
    let mut wanted = expected(15..45);
    wanted.extend(156..183);
    assert_eq!(report.sections, 2);
    assert_eq!(numbers(&frames), wanted);

    let frame_time = 1.0 / FPS as f64;
    for pair in frames.windows(2) {
        let gap = pair[1].seconds - pair[0].seconds;
        assert!(
            (gap - frame_time).abs() < 0.002,
            "gap of {gap}s between frames {} and {}",
            pair[0].number,
            pair[1].number
        );
    }
}

#[test]
fn keyframe_mode_grows_outward_and_encodes_nothing() {
    let input = h264_mp4();
    let output = output_path("keyframe", "mp4");

    let report = run_export(&input, &output, vec![range(0.5, 2.5)], CutMode::Keyframe);

    assert_eq!(report.mode_used, CutMode::Keyframe);
    assert_eq!(report.encoded_frames, 0);
    assert_eq!(report.encoder, None);
    assert_eq!(numbers(&decode_video(&output)), expected(0..90));
    assert_all_identical(&input, &output, 0..90);
}

#[test]
fn b_frames_cut_exactly_and_copied_frames_stay_identical() {
    let input = b_frames_two_tracks_mkv();
    let output = output_path("bframes", "mkv");
    assert!(
        index_of(&input)
            .frames()
            .iter()
            .any(|frame| frame.pts != frame.dts),
        "the test clip should contain B-frames"
    );

    let report = run_export(&input, &output, vec![range(0.5, 5.5)], CutMode::FrameExact);

    assert!(report.copied_frames > 0, "{report:?}");
    assert_eq!(numbers(&decode_video(&output)), expected(15..165));
    let copied = copied_frames(&input, 15, 165);
    assert!(!copied.is_empty());
    assert_all_identical(&input, &output, copied);
}

#[test]
fn other_codecs_fall_back_to_keyframe_cuts_and_say_so() {
    let input = mpeg4_mkv();
    let output = output_path("mpeg4", "mkv");

    let report = run_export(&input, &output, vec![range(2.0, 5.0)], CutMode::FrameExact);

    assert_eq!(report.mode_requested, CutMode::FrameExact);
    assert_eq!(report.mode_used, CutMode::Keyframe);
    assert_eq!(report.encoded_frames, 0);
    let frames = numbers(&decode_video(&output));
    assert!(
        frames.windows(2).all(|pair| pair[1] == pair[0] + 1),
        "{frames:?}"
    );
    assert!(
        frames[0] <= 60 && *frames.last().expect("frames exist") >= 149,
        "{frames:?}"
    );
}

#[test]
fn av1_cuts_exactly_and_copied_frames_stay_identical() {
    let input = av1_mkv();
    let output = output_path("av1", "mkv");

    let report = run_export(&input, &output, vec![range(0.5, 4.5)], CutMode::FrameExact);

    assert_eq!(report.mode_used, CutMode::FrameExact, "{report:?}");
    assert!(report.copied_frames > 0, "{report:?}");
    assert_eq!(numbers(&decode_video(&output)), expected(15..135));
    assert_all_identical(&input, &output, copied_frames(&input, 15, 135));
}

#[test]
fn b_frame_sections_join_cleanly() {
    let input = b_frames_two_tracks_mkv();
    let output = output_path("bframes-sections", "mkv");

    run_export(
        &input,
        &output,
        vec![range(0.5, 2.5), range(4.1, 6.3)],
        CutMode::FrameExact,
    );

    let mut wanted = expected(15..75);
    wanted.extend(123..189);
    assert_eq!(numbers(&decode_video(&output)), wanted);
}

#[test]
fn every_audio_track_is_kept_with_its_name_and_sound() {
    let input = b_frames_two_tracks_mkv();
    let output = output_path("tracks", "mkv");

    let report = run_export(&input, &output, vec![range(2.0, 6.0)], CutMode::FrameExact);

    assert_eq!(report.audio_tracks, 2);
    let tracks = decode_audio(&output);
    assert_eq!(tracks.len(), 2);
    assert_eq!(tracks[0].title.as_deref(), Some("Game"));
    assert_eq!(tracks[0].language.as_deref(), Some("eng"));
    assert_eq!(tracks[1].title.as_deref(), Some("Microphone"));

    for (track, wanted_pitch) in tracks.iter().zip([440.0, 1760.0]) {
        assert!((track.seconds - 4.0).abs() < 0.1, "{}", track.seconds);
        let pitch = pitch_of(&track.samples, track.rate);
        assert!(
            (pitch - wanted_pitch).abs() < 20.0,
            "track {:?} has pitch {pitch}",
            track.title
        );
    }
}

#[test]
fn audio_tracks_keep_their_length_across_several_sections() {
    let input = b_frames_two_tracks_mkv();
    let output = output_path("tracks-sections", "mkv");

    run_export(
        &input,
        &output,
        vec![range(1.0, 3.0), range(5.5, 7.0)],
        CutMode::FrameExact,
    );

    for track in decode_audio(&output) {
        assert!((track.seconds - 3.5).abs() < 0.1, "{}", track.seconds);
    }
}

/// Beeps start on whole seconds in the source, so each one has to line up with the
/// picture of the frame shown at that second.
fn assert_beeps_match_picture(output: &Path) {
    let frames = decode_video(output);
    let tracks = decode_audio(output);
    let beeps = beep_times(&tracks[0]);
    assert!(!beeps.is_empty(), "no beeps found");

    let mut compared = 0;
    for beep in beeps {
        // The frame on screen when this beep starts.
        let shown = frames
            .iter()
            .min_by(|a, b| {
                (a.seconds - beep)
                    .abs()
                    .total_cmp(&(b.seconds - beep).abs())
            })
            .expect("the video has frames");
        let source_time = f64::from(shown.number) / FPS as f64;
        let error = (source_time - source_time.round()).abs();
        // Allows one audio packet of about 21 ms, plus half a frame of rounding.
        assert!(
            error < 0.045,
            "the beep at {beep:.3}s shows frame {}, which is {error:.3}s from a whole second",
            shown.number
        );
        compared += 1;
    }
    assert!(compared >= 2, "only {compared} beeps compared");
}

#[test]
fn audio_stays_in_sync_after_a_frame_exact_cut() {
    let input = h264_mp4();
    let output = output_path("sync", "mp4");

    run_export(&input, &output, vec![range(2.5, 7.5)], CutMode::FrameExact);

    assert_beeps_match_picture(&output);
}

#[test]
fn audio_stays_in_sync_in_every_section_after_joining() {
    let input = h264_mp4();
    let output = output_path("sync-sections", "mp4");

    run_export(
        &input,
        &output,
        vec![range(0.5, 2.5), range(3.7, 5.3), range(7.2, 9.4)],
        CutMode::FrameExact,
    );

    assert_beeps_match_picture(&output);
}

#[test]
fn audio_stays_in_sync_after_a_keyframe_cut() {
    let input = h264_mp4();
    let output = output_path("sync-keyframe", "mp4");

    run_export(&input, &output, vec![range(2.5, 7.5)], CutMode::Keyframe);

    assert_beeps_match_picture(&output);
}

#[test]
fn variable_frame_rate_keeps_the_exact_frames_and_their_timing() {
    let input = variable_rate_mkv();
    let output = output_path("vfr", "mkv");
    let source = decode_video(&input);

    let start = source[47].seconds;
    let end = source[170].seconds;
    run_export(
        &input,
        &output,
        vec![range(start, end)],
        CutMode::FrameExact,
    );

    let frames = decode_video(&output);
    assert_eq!(numbers(&frames), expected(47..170));

    // Uneven gaps must survive. Matroska stores whole milliseconds, hence the margin.
    for (index, pair) in frames.windows(2).enumerate() {
        let original = source[47 + index + 1].seconds - source[47 + index].seconds;
        let gap = pair[1].seconds - pair[0].seconds;
        assert!(
            (gap - original).abs() < 0.0025,
            "frame {}: gap {gap} should be {original}",
            pair[0].number
        );
    }
}

#[test]
fn variable_frame_rate_does_not_drift_over_several_sections() {
    let input = variable_rate_mkv();
    let output = output_path("vfr-sections", "mkv");
    let source = decode_video(&input);

    let first = (source[20].seconds, source[80].seconds);
    let second = (source[150].seconds, source[260].seconds);
    run_export(
        &input,
        &output,
        vec![range(first.0, first.1), range(second.0, second.1)],
        CutMode::FrameExact,
    );

    let frames = decode_video(&output);
    let mut wanted = expected(20..80);
    wanted.extend(150..260);
    assert_eq!(numbers(&frames), wanted);

    let total = (first.1 - first.0) + (second.1 - second.0);
    let last_gap = source[260].seconds - source[259].seconds;
    let measured = frames.last().expect("frames exist").seconds - frames[0].seconds + last_gap;
    assert!((measured - total).abs() < 0.005, "{measured} vs {total}");
}

#[test]
fn hevc_cuts_exactly_with_software_encoding_available() {
    let input = hevc_mkv();
    let output = output_path("hevc", "mkv");

    let report = run_export(&input, &output, vec![range(0.5, 4.5)], CutMode::FrameExact);

    assert_eq!(report.mode_used, CutMode::FrameExact);
    assert_eq!(numbers(&decode_video(&output)), expected(15..135));
    let copied = copied_frames(&input, 15, 135);
    assert!(!copied.is_empty());
    assert_all_identical(&input, &output, copied);
}

#[test]
fn a_different_container_can_be_chosen() {
    let input = h264_mp4();
    let output = output_path("to-mkv", "mkv");

    run_export(&input, &output, vec![range(0.5, 2.5)], CutMode::FrameExact);

    assert_eq!(numbers(&decode_video(&output)), expected(15..75));
    assert_eq!(decode_audio(&output).len(), 1);
}

fn request(input: &Path, output: &Path) -> ExportRequest {
    ExportRequest {
        input: input.to_owned(),
        output: output.to_owned(),
        ranges: vec![range(1.0, 3.0)],
        mode: CutMode::FrameExact,
    }
}

#[test]
fn cancelling_leaves_no_files_behind() {
    let input = h264_mp4();
    let output = output_path("cancelled", "mp4");
    let frames = index_of(&input);
    let cancel = AtomicBool::new(false);
    cancel.store(true, Ordering::Relaxed);

    let result = export(&request(&input, &output), &frames, &cancel, &|_| {});

    assert!(matches!(result, Err(MediaError::Cancelled)), "{result:?}");
    assert!(!output.exists());
    assert!(!output.with_file_name("out-cancelled.partial.mp4").exists());
}

#[test]
fn the_original_file_cannot_be_overwritten() {
    let input = h264_mp4();
    let frames = index_of(&input);

    let result = export(
        &request(&input, &input),
        &frames,
        &AtomicBool::new(false),
        &|_| {},
    );

    assert!(
        matches!(result, Err(MediaError::SameInputAndOutput)),
        "{result:?}"
    );
    assert!(input.exists());
}

#[test]
fn exporting_nothing_is_refused() {
    let input = h264_mp4();
    let output = output_path("nothing", "mp4");
    let frames = index_of(&input);
    let mut empty = request(&input, &output);
    empty.ranges.clear();

    let result = export(&empty, &frames, &AtomicBool::new(false), &|_| {});

    assert!(
        matches!(result, Err(MediaError::NothingToExport)),
        "{result:?}"
    );
    assert!(!output.exists());
}

#[test]
fn exported_files_report_correct_details() {
    let input = h264_mp4();
    let output = output_path("details", "mp4");

    run_export(&input, &output, vec![range(1.0, 3.0)], CutMode::FrameExact);

    let info = probe(&output).expect("the output can be read");
    assert_eq!(info.video.codec, "h264");
    assert_eq!((info.video.width, info.video.height), (320, 180));
    assert_eq!(info.audio.len(), 1);
    assert_eq!(info.audio[0].codec, "aac");
    assert!(info.size_bytes > 0);
}

#[test]
fn probing_reads_the_source_details() {
    let info = probe(&b_frames_two_tracks_mkv()).expect("the file can be read");

    assert_eq!(info.video.codec, "hevc");
    assert!((info.video.frame_rate - 30.0).abs() < 0.01);
    assert!(!info.video.is_hdr);
    assert_eq!(info.audio.len(), 2);
    assert_eq!(info.audio[1].title.as_deref(), Some("Microphone"));
    assert_eq!(info.audio[0].sample_rate, 48_000);
}

#[test]
fn a_file_without_video_is_reported_clearly() {
    let path = output_dir().join("not-a-video.txt");
    std::fs::write(&path, "this is not a video").expect("the file can be written");

    assert!(probe(&path).is_err());
}

#[test]
fn the_frame_table_lists_every_frame() {
    let index = index_of(&h264_mp4());

    assert_eq!(index.frames().len(), 300);
    assert_eq!(index.clean_points().len(), 10);
}

// These need an NVIDIA GPU, so GitHub Actions cannot run them. Run them on the
// laptop with: ./scripts/cargo.ps1 test -p bareclip-media '--' --ignored

fn nvenc_case(encoder: &str, extension: &str) {
    let input = nvenc_clip(encoder, extension);
    let output = output_path(&format!("gpu-{encoder}"), extension);
    let source = index_of(&input);
    let has_reordering = source.frames().iter().any(|frame| frame.pts != frame.dts);
    assert!(has_reordering, "the test clip should contain B-frames");

    let report = run_export(
        &input,
        &output,
        vec![range(0.7, 5.3), range(6.1, 8.9)],
        CutMode::FrameExact,
    );

    assert_eq!(report.mode_used, CutMode::FrameExact);
    assert!(
        report
            .encoder
            .as_deref()
            .is_some_and(|name| name.ends_with("_nvenc")),
        "{report:?}"
    );
    assert!(report.copied_frames > 0, "{report:?}");
    let mut wanted = expected(21..159);
    wanted.extend(183..267);
    assert_eq!(numbers(&decode_video(&output)), wanted);
    assert_all_identical(&input, &output, 60..120);
    assert_beeps_match_picture(&output);
}

#[test]
#[ignore = "needs an NVIDIA GPU"]
fn nvenc_h264_with_b_frames_in_mp4() {
    nvenc_case("h264_nvenc", "mp4");
}

#[test]
#[ignore = "needs an NVIDIA GPU"]
fn nvenc_h264_with_b_frames_in_mkv() {
    nvenc_case("h264_nvenc", "mkv");
}

#[test]
#[ignore = "needs an NVIDIA GPU"]
fn nvenc_hevc_with_b_frames_in_mp4() {
    nvenc_case("hevc_nvenc", "mp4");
}

#[test]
#[ignore = "needs an NVIDIA GPU"]
fn nvenc_hevc_with_b_frames_in_mkv() {
    nvenc_case("hevc_nvenc", "mkv");
}
