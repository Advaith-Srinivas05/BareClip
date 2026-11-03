//! Reads pictures and sound shapes from real video files.

mod common;

use std::sync::atomic::{AtomicBool, Ordering};

use bareclip_core::time::Ticks;
use bareclip_media::{MediaError, StripPicture, probe, read_sound_shape, read_strip_pictures};
use common::*;

fn pictures_of(path: &std::path::Path, spacing: f64) -> Vec<StripPicture> {
    let mut pictures = Vec::new();
    let duration = probe(path).expect("the video can be read").duration;
    read_strip_pictures(
        path,
        duration,
        seconds(spacing),
        (128, 72),
        &AtomicBool::new(false),
        |picture| pictures.push(picture),
    )
    .expect("the pictures can be read");
    pictures
}

#[test]
fn pictures_come_from_along_the_video_at_about_the_spacing_asked_for() {
    // Ten seconds, with a keyframe every second.
    let pictures = pictures_of(&h264_mp4(), 2.0);

    let times: Vec<f64> = pictures.iter().map(|p| p.time.as_seconds()).collect();
    assert_eq!(times, vec![0.0, 2.0, 4.0, 6.0, 8.0], "{times:?}");
}

#[test]
fn pictures_are_the_size_asked_for_and_hold_every_pixel() {
    let pictures = pictures_of(&h264_mp4(), 5.0);

    for picture in &pictures {
        assert!(picture.width <= 128 && picture.height <= 72);
        let expected = picture.width as usize * picture.height as usize * 4;
        assert_eq!(picture.rgba.len(), expected);
    }
}

#[test]
fn a_picture_shows_the_frame_at_its_time() {
    let input = h264_mp4();
    let frames = decode_video(&input);
    let pictures = pictures_of(&input, 4.0);

    // Burned-in frames differ across the video, so pictures from different moments differ.
    assert_ne!(pictures[0].rgba, pictures[1].rgba);
    assert_eq!(pictures[1].time, seconds(frames[120].seconds));
}

#[test]
fn spacing_finer_than_the_keyframes_gives_each_keyframe_once() {
    // Keyframes are a second apart here.
    let pictures = pictures_of(&h264_mp4(), 0.25);

    let times: Vec<f64> = pictures.iter().map(|p| p.time.as_seconds()).collect();
    assert_eq!(times.len(), 10, "{times:?}");
    for pair in pictures.windows(2) {
        assert!(pair[0].time < pair[1].time);
    }
}

#[test]
fn reading_finishes_when_the_last_keyframe_is_well_before_the_end() {
    // Spacing longer than the rest of the video, so the next picture wanted is past the end.
    let pictures = pictures_of(&h264_mp4(), 9.5);

    let times: Vec<f64> = pictures.iter().map(|p| p.time.as_seconds()).collect();
    assert_eq!(times, vec![0.0, 9.0], "{times:?}");
}

#[test]
fn pictures_can_be_read_from_every_kind_of_video_the_app_supports() {
    for input in [
        b_frames_two_tracks_mkv(),
        hevc_mkv(),
        av1_mkv(),
        variable_rate_mkv(),
    ] {
        let pictures = pictures_of(&input, 1.0);

        assert!(pictures.len() >= 2, "{input:?}");
        for pair in pictures.windows(2) {
            assert!(pair[0].time < pair[1].time, "{input:?}");
        }
    }
}

#[test]
fn reading_pictures_stops_when_cancelled() {
    let cancel = AtomicBool::new(false);
    let mut count = 0;

    read_strip_pictures(
        &h264_mp4(),
        seconds(10.0),
        seconds(1.0),
        (128, 72),
        &cancel,
        |_| {
            count += 1;
            cancel.store(true, Ordering::Relaxed);
        },
    )
    .expect("stopping is not a failure");

    assert_eq!(count, 1);
}

#[test]
fn a_file_with_no_video_has_nothing_to_read_pictures_from() {
    let folder = output_path("strip-not-a-video", "txt");
    std::fs::write(&folder, "not a video").expect("it can be written");

    let result = read_strip_pictures(
        &folder,
        seconds(1.0),
        seconds(1.0),
        (128, 72),
        &AtomicBool::new(false),
        |_| {},
    );

    assert!(matches!(
        result,
        Err(MediaError::Ffmpeg(_) | MediaError::NoVideoStream)
    ));
}

#[test]
fn the_sound_shape_covers_the_whole_video_and_peaks_where_it_beeps() {
    let input = h264_mp4();
    let beeps = beep_times(&decode_audio(&input)[0]);
    assert!(!beeps.is_empty());

    let shape = read_sound_shape(&input, &AtomicBool::new(false), |_| {})
        .expect("the sound can be read")
        .expect("it was not cancelled");

    let seconds_covered = shape.peaks.len() as f64 / f64::from(shape.peaks_per_second);
    assert!((seconds_covered - 10.0).abs() < 0.2, "{seconds_covered}");
    let around = |time: f64, width: f64| {
        shape.peak_between(
            Ticks::from_seconds((time - width).max(0.0)),
            Ticks::from_seconds(time + width),
        )
    };
    for beep in &beeps {
        assert!(around(*beep, 0.06) > 0.1, "nothing at the beep at {beep}");
    }
    let quiet = beeps[0] + 0.5;
    assert!(
        around(quiet, 0.02) < around(beeps[0], 0.06) / 2.0,
        "no quiet between beeps"
    );
}

#[test]
fn every_audio_track_counts_toward_the_shape() {
    let input = b_frames_two_tracks_mkv();

    let shape = read_sound_shape(&input, &AtomicBool::new(false), |_| {})
        .expect("the sound can be read")
        .expect("it was not cancelled");

    assert_eq!(shape.peaks.iter().copied().max(), Some(255));
}

#[test]
fn a_video_with_no_sound_has_an_empty_shape() {
    let shape = read_sound_shape(&silent_mp4(), &AtomicBool::new(false), |_| {})
        .expect("the file can be read")
        .expect("it was not cancelled");

    assert!(shape.peaks.is_empty());
}

#[test]
fn reading_the_sound_stops_when_cancelled() {
    let cancel = AtomicBool::new(true);

    let shape = read_sound_shape(&h264_mp4(), &cancel, |_| {}).expect("stopping is not a failure");

    assert!(shape.is_none());
}
