//! Exports real video files and proves exactly what came out.

mod common;

use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use bareclip_core::{
    splice::CutMode,
    time::Ticks,
    timeline::MediaId,
    tracks::{TrackTarget, Tracks},
};
use bareclip_media::{ExportPart, ExportRequest, MediaError, export, probe};
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
fn sections_marked_mid_frame_still_join_with_steady_timing() {
    let input = h264_mp4();
    let output = output_path("mid-frame", "mp4");
    let frame_time = 1.0 / FPS as f64;

    // Every mark here falls partway through a frame.
    run_export(
        &input,
        &output,
        vec![
            range(0.5 + frame_time / 3.0, 1.5 + frame_time / 3.0),
            range(5.2 + frame_time / 2.0, 6.1 + frame_time / 2.0),
        ],
        CutMode::FrameExact,
    );

    let frames = decode_video(&output);
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
        output: output.to_owned(),
        parts: parts_of(input, vec![range(1.0, 3.0)]),
        mode: CutMode::FrameExact,
    }
}

#[test]
fn cancelling_leaves_no_files_behind() {
    let input = h264_mp4();
    let output = output_path("cancelled", "mp4");
    let cancel = AtomicBool::new(false);
    cancel.store(true, Ordering::Relaxed);

    let result = export(&request(&input, &output), &cancel, &|_| {});

    assert!(matches!(result, Err(MediaError::Cancelled)), "{result:?}");
    assert!(!output.exists());
    assert!(!output.with_file_name("out-cancelled.partial.mp4").exists());
}

#[test]
fn the_original_file_cannot_be_overwritten() {
    let input = h264_mp4();

    let result = export(&request(&input, &input), &AtomicBool::new(false), &|_| {});

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
    let mut empty = request(&input, &output);
    empty.parts.clear();

    let result = export(&empty, &AtomicBool::new(false), &|_| {});

    assert!(
        matches!(result, Err(MediaError::NothingToExport)),
        "{result:?}"
    );
    assert!(!output.exists());
}

#[test]
fn clips_from_two_files_join_in_order() {
    let first = h264_mp4();
    let second = second_h264_mp4();
    let output = output_path("two-files", "mp4");
    let mut parts = parts_of(&first, vec![range(0.5, 2.5)]);
    parts.extend(parts_of(&second, vec![range(1.0, 3.0)]));

    let report = run_parts(&output, parts, CutMode::FrameExact);

    let mut wanted = expected(15..75);
    wanted.extend(1030..1090);
    let frames = decode_video(&output);
    assert_eq!(numbers(&frames), wanted, "{report:?}");
    assert_all_identical(&first, &output, 30..60);
    assert_all_identical(&second, &output, 1030..1090);
    let frame_time = 1.0 / FPS as f64;
    for pair in frames.windows(2) {
        let gap = pair[1].seconds - pair[0].seconds;
        assert!(
            (gap - frame_time).abs() < 0.002,
            "gap of {gap}s at {}",
            pair[0].number
        );
    }
}

#[test]
fn audio_stays_in_sync_after_joining_two_files() {
    let output = output_path("two-files-sync", "mp4");
    let mut parts = parts_of(&h264_mp4(), vec![range(2.5, 5.5)]);
    parts.extend(parts_of(&second_h264_mp4(), vec![range(1.5, 4.5)]));

    run_parts(&output, parts, CutMode::FrameExact);

    let frames = decode_video(&output);
    let beeps = beep_times(&decode_audio(&output)[0]);
    assert!(beeps.len() >= 4, "{beeps:?}");
    for beep in beeps {
        let shown = frames
            .iter()
            .min_by(|a, b| {
                (a.seconds - beep)
                    .abs()
                    .total_cmp(&(b.seconds - beep).abs())
            })
            .expect("the video has frames");
        // The second source numbers its frames from 1000, and beeps on its own seconds.
        let source_frame = f64::from(shown.number % 1000);
        let error = (source_frame / FPS as f64 - (source_frame / FPS as f64).round()).abs();
        assert!(
            error < 0.045,
            "the beep at {beep:.3}s shows frame {}",
            shown.number
        );
    }
}

#[test]
fn clips_can_play_in_a_different_order_than_the_source() {
    let input = h264_mp4();
    let output = output_path("reordered", "mp4");

    run_export(
        &input,
        &output,
        vec![range(5.0, 6.0), range(1.0, 2.0)],
        CutMode::FrameExact,
    );

    let mut wanted = expected(150..180);
    wanted.extend(30..60);
    assert_eq!(numbers(&decode_video(&output)), wanted);
}

#[test]
fn the_same_footage_can_appear_twice() {
    let input = h264_mp4();
    let output = output_path("repeated", "mp4");

    run_export(
        &input,
        &output,
        vec![range(1.0, 2.0), range(1.0, 2.0)],
        CutMode::FrameExact,
    );

    let mut wanted = expected(30..60);
    wanted.extend(30..60);
    assert_eq!(numbers(&decode_video(&output)), wanted);
}

#[test]
fn keyframe_cuts_can_join_two_files_too() {
    let output = output_path("two-files-keyframe", "mp4");
    let mut parts = parts_of(&h264_mp4(), vec![range(1.0, 2.0)]);
    parts.extend(parts_of(&second_h264_mp4(), vec![range(1.0, 2.0)]));

    let report = run_parts(&output, parts, CutMode::Keyframe);

    assert_eq!(report.encoded_frames, 0);
    let mut wanted = expected(30..60);
    wanted.extend(1030..1060);
    assert_eq!(numbers(&decode_video(&output)), wanted);
}

/// Two parts of one video with dead space between them.
fn parts_with_gap(input: &Path, gap: f64) -> Vec<bareclip_media::ExportPart> {
    let mut parts = parts_of(input, vec![range(1.0, 2.0), range(3.0, 4.0)]);
    parts[1].gap_before = seconds(gap);
    parts
}

/// How loud a stretch of an audio track is.
fn loudness(track: &AudioTrackInfo, from: f64, to: f64) -> f32 {
    let rate = f64::from(track.rate);
    let first = (((from - track.first_sample_seconds) * rate) as usize).min(track.samples.len());
    let last = (((to - track.first_sample_seconds) * rate) as usize).min(track.samples.len());
    let part = &track.samples[first..last];
    assert!(!part.is_empty(), "no audio between {from}s and {to}s");
    (part.iter().map(|sample| sample * sample).sum::<f32>() / part.len() as f32).sqrt()
}

#[test]
fn dead_space_exports_as_black_between_untouched_frames() {
    let input = h264_mp4();
    let output = output_path("gap", "mp4");

    let report = run_parts(&output, parts_with_gap(&input, 1.0), CutMode::FrameExact);

    let frames = decode_video(&output);
    let mut wanted = expected(30..60);
    wanted.extend(std::iter::repeat_n(0, 30));
    wanted.extend(90..120);
    assert_eq!(numbers(&frames), wanted);
    assert_eq!(report.black_frames, 30);
    assert_eq!(report.encoded_frames, 0);
    assert_all_identical(&input, &output, 30..60);
    assert_all_identical(&input, &output, 90..120);

    let frame_time = 1.0 / FPS as f64;
    for pair in frames.windows(2) {
        let gap = pair[1].seconds - pair[0].seconds;
        assert!((gap - frame_time).abs() < 0.002, "gap of {gap}s");
    }
    assert!((probe(&output).expect("it opens").duration.as_seconds() - 3.0).abs() < 0.1);
}

#[test]
fn every_black_frame_is_the_same_dark_picture() {
    let input = h264_mp4();
    let output = output_path("gap-black", "mp4");

    run_parts(&output, parts_with_gap(&input, 0.5), CutMode::FrameExact);

    let frames = decode_video(&output);
    let black: Vec<_> = frames.iter().filter(|frame| frame.number == 0).collect();
    assert_eq!(black.len(), 15);
    assert!(black.iter().all(|frame| frame.hash == black[0].hash));
}

#[test]
fn dead_space_is_silent_and_audio_stays_in_sync_after_it() {
    let input = h264_mp4();
    let output = output_path("gap-audio", "mp4");

    run_parts(&output, parts_with_gap(&input, 1.0), CutMode::FrameExact);

    assert_beeps_match_picture(&output);
    let tracks = decode_audio(&output);
    assert!(
        (tracks[0].seconds - 3.0).abs() < 0.1,
        "{}",
        tracks[0].seconds
    );
    assert!(loudness(&tracks[0], 1.1, 1.9) < 0.001);
    // Each second of the source starts with a beep, so the part after the gap does too.
    assert!(loudness(&tracks[0], 2.0, 2.1) > 0.1);
}

#[test]
fn dead_space_can_come_first() {
    let input = h264_mp4();
    let output = output_path("gap-first", "mp4");
    let mut parts = parts_of(&input, vec![range(2.0, 3.0)]);
    parts[0].gap_before = seconds(0.5);

    run_parts(&output, parts, CutMode::FrameExact);

    let mut wanted = vec![0; 15];
    wanted.extend(60..90);
    assert_eq!(numbers(&decode_video(&output)), wanted);
    assert_all_identical(&input, &output, 60..90);
    let tracks = decode_audio(&output);
    assert!(loudness(&tracks[0], 0.0, 0.45) < 0.001);
    assert!(loudness(&tracks[0], 0.5, 0.6) > 0.1);
}

#[test]
fn dead_space_works_with_fastest_cuts_and_nothing_else_is_encoded() {
    let input = h264_mp4();
    let output = output_path("gap-keyframe", "mp4");

    let report = run_parts(&output, parts_with_gap(&input, 1.0), CutMode::Keyframe);

    assert_eq!(report.encoded_frames, 0);
    assert_eq!(report.black_frames, 30);
    assert_eq!(report.mode_used, CutMode::Keyframe);
    assert_eq!(decode_video(&output).len(), 90);
}

#[test]
fn dead_space_fills_every_audio_track() {
    let input = b_frames_two_tracks_mkv();
    let output = output_path("gap-tracks", "mkv");

    run_parts(&output, parts_with_gap(&input, 1.0), CutMode::FrameExact);

    let tracks = decode_audio(&output);
    assert_eq!(tracks.len(), 2);
    for track in &tracks {
        assert!((track.seconds - 3.0).abs() < 0.1, "{}", track.seconds);
        assert!(loudness(track, 1.1, 1.9) < 0.001);
        // FFmpeg's sine source has an amplitude of 1/8, a level of about 0.088.
        assert!(loudness(track, 0.2, 0.8) > 0.05);
        assert!(loudness(track, 2.2, 2.8) > 0.05);
    }
    let frames = decode_video(&output);
    let mut wanted = expected(30..60);
    wanted.extend(std::iter::repeat_n(0, 30));
    wanted.extend(90..120);
    assert_eq!(numbers(&frames), wanted);
}

#[test]
fn dead_space_works_in_a_video_with_no_sound() {
    let input = hevc_mkv();
    let output = output_path("gap-hevc", "mkv");

    run_parts(&output, parts_with_gap(&input, 1.0), CutMode::FrameExact);

    assert_eq!(decode_video(&output).len(), 90);
}

#[test]
fn dead_space_works_in_av1() {
    let input = av1_mkv();
    let output = output_path("gap-av1", "mkv");

    run_parts(&output, parts_with_gap(&input, 1.0), CutMode::FrameExact);

    let numbers = numbers(&decode_video(&output));
    assert_eq!(numbers.len(), 90);
    assert_eq!(numbers[45], 0);
    assert_eq!(numbers[60], 90);
}

#[test]
fn a_sliver_of_dead_space_is_not_written() {
    let input = h264_mp4();
    let output = output_path("gap-sliver", "mp4");

    let report = run_parts(&output, parts_with_gap(&input, 0.005), CutMode::FrameExact);

    assert_eq!(report.black_frames, 0);
    assert_eq!(decode_video(&output).len(), 60);
}

#[test]
fn dead_space_in_a_format_without_an_encoder_is_refused_with_the_reason() {
    let input = mpeg4_mkv();
    let output = output_path("gap-mpeg4", "mkv");
    let request = ExportRequest {
        output: output.clone(),
        parts: parts_with_gap(&input, 1.0),
        mode: CutMode::FrameExact,
    };

    let result = export(&request, &AtomicBool::new(false), &|_| {});

    let error = result.expect_err("there is nothing to write black frames with");
    assert!(matches!(error, MediaError::GapNeedsEncoder));
    assert!(error.to_string().contains("Remove the gaps"));
    assert!(!output.exists());
}

#[test]
fn cancelling_during_dead_space_leaves_no_files_behind() {
    let input = h264_mp4();
    let output = output_path("gap-cancel", "mp4");
    let mut parts = parts_of(&input, vec![range(0.0, 1.0)]);
    parts[0].gap_before = Ticks::from_seconds(60.0);
    let request = ExportRequest {
        output: output.clone(),
        parts,
        mode: CutMode::FrameExact,
    };

    let result = export(&request, &AtomicBool::new(true), &|_| {});

    assert!(matches!(result, Err(MediaError::Cancelled)), "{result:?}");
    assert!(!output.exists());
    assert!(!output.with_file_name("out-gap-cancel.partial.mp4").exists());
}

#[test]
fn videos_in_different_formats_are_refused_with_the_reason() {
    let output = output_path("mismatch", "mp4");
    let mut parts = parts_of(&h264_mp4(), vec![range(0.0, 1.0)]);
    parts.extend(parts_of(&different_size_mp4(), vec![range(0.0, 1.0)]));

    let result = export(
        &ExportRequest {
            output: output.clone(),
            parts,
            mode: CutMode::FrameExact,
        },
        &AtomicBool::new(false),
        &|_| {},
    );

    let Err(MediaError::CannotJoin(reason)) = result else {
        panic!("expected a refusal, got {result:?}");
    };
    assert!(reason.contains("160 x 90"), "{reason}");
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

/// The parts a stack of tracks exports as, all from one video.
fn parts_of_tracks(input: &Path, tracks: &Tracks) -> Vec<ExportPart> {
    let played = tracks.played();
    let mut previous_end = Ticks(0);
    played
        .clips()
        .iter()
        .map(|clip| {
            let gap_before = Ticks(clip.start.0 - previous_end.0);
            previous_end = clip.end();
            let mut part = parts_of(input, vec![clip.source]).remove(0);
            part.gap_before = gap_before;
            part
        })
        .collect()
}

#[test]
fn a_clip_on_a_higher_track_replaces_the_frames_it_covers() {
    let input = h264_mp4();
    let output = output_path("tracks-covered", "mp4");
    let video = MediaId(1);
    let mut tracks = Tracks::default();
    // Frames 0 to 90 below, with frames 150 to 180 on top from one second in.
    tracks.place(video, range(0.0, 3.0), seconds(0.0), TrackTarget::New(0));
    tracks.place(video, range(5.0, 6.0), seconds(1.0), TrackTarget::New(0));

    run_parts(
        &output,
        parts_of_tracks(&input, &tracks),
        CutMode::FrameExact,
    );

    let mut wanted = expected(0..30);
    wanted.extend(150..180);
    wanted.extend(60..90);
    assert_eq!(numbers(&decode_video(&output)), wanted);
}

#[test]
fn a_clip_that_is_completely_covered_is_not_exported() {
    let input = h264_mp4();
    let output = output_path("tracks-hidden", "mp4");
    let video = MediaId(1);
    let mut tracks = Tracks::default();
    tracks.place(video, range(5.0, 6.0), seconds(0.5), TrackTarget::New(0));
    tracks.place(video, range(0.0, 3.0), seconds(0.0), TrackTarget::New(0));

    run_parts(
        &output,
        parts_of_tracks(&input, &tracks),
        CutMode::FrameExact,
    );

    assert_eq!(numbers(&decode_video(&output)), expected(0..90));
}

#[test]
fn dead_space_with_nothing_on_any_track_exports_as_black_at_its_length() {
    let input = h264_mp4();
    let output = output_path("tracks-gap", "mp4");
    let video = MediaId(1);
    let mut tracks = Tracks::default();
    tracks.place(video, range(0.0, 1.0), seconds(0.0), TrackTarget::New(0));
    tracks.place(video, range(2.0, 3.0), seconds(2.0), TrackTarget::New(0));

    let report = run_parts(
        &output,
        parts_of_tracks(&input, &tracks),
        CutMode::FrameExact,
    );

    assert_eq!(report.black_frames, 30);
    assert_eq!(decode_video(&output).len(), 90);
}

#[test]
fn sound_follows_the_top_clip_across_tracks() {
    let input = h264_mp4();
    let output = output_path("tracks-sound", "mp4");
    let video = MediaId(1);
    let mut tracks = Tracks::default();
    tracks.place(video, range(0.0, 3.0), seconds(0.0), TrackTarget::New(0));
    tracks.place(video, range(5.0, 6.0), seconds(1.0), TrackTarget::New(0));

    run_parts(
        &output,
        parts_of_tracks(&input, &tracks),
        CutMode::FrameExact,
    );

    let picture = decode_video(&output).len() as f64 / 30.0;
    for track in decode_audio(&output) {
        let sound = track.samples.len() as f64 / f64::from(track.rate);
        assert!((sound - picture).abs() < 0.05, "{sound} and {picture}");
    }
}
