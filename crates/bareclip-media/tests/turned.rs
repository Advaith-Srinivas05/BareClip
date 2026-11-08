//! Videos from phones, which store their pictures sideways and note which way up to show them.

mod common;

use std::sync::atomic::AtomicBool;

use bareclip_core::{splice::CutMode, time::Ticks};
use bareclip_media::{
    DecodeStep, Orientation, PictureReader, VideoPreview, join_problem, probe, read_strip_pictures,
};
use common::*;

/// Black is about 16 and white about 235, so anything past the middle is clearly one or the other.
fn assert_white_at_the_bottom((top, bottom): (f64, f64), what: &str) {
    assert!(
        top < 60.0 && bottom > 190.0,
        "{what}: top {top}, bottom {bottom}"
    );
}

#[test]
fn a_phone_video_is_its_upright_size_and_knows_its_turn() {
    let info = probe(&phone_mp4()).expect("the video can be read");

    assert_eq!((info.video.width, info.video.height), (180, 320));
    assert_eq!(info.video.orientation, Orientation::QuarterAnticlockwise);
}

#[test]
fn a_video_with_no_turn_noted_stays_as_stored() {
    let info = probe(&unturned_mp4()).expect("the video can be read");

    assert_eq!((info.video.width, info.video.height), (320, 180));
    assert_eq!(info.video.orientation, Orientation::Upright);
}

#[test]
fn redrawing_reads_every_picture_the_right_way_up() {
    let mut reader = PictureReader::open(&phone_mp4()).expect("the video opens");
    let mut pictures = 0;
    while let Some(picture) = reader.next_picture().expect("the video decodes") {
        let view = picture.view();
        assert_eq!((view.width, view.height), (180, 320));
        assert_white_at_the_bottom(
            top_and_bottom(view.y, view.y_stride, 180, 320),
            "redrawn picture",
        );
        pictures += 1;
    }
    assert_eq!(pictures, 90);
}

#[test]
fn the_preview_shows_it_the_right_way_up_for_the_gpu_and_the_screen() {
    for planes in [true, false] {
        let preview = VideoPreview::open(&phone_mp4(), 1920, 1080).expect("the video opens");
        let mut preview = if planes {
            preview.with_planes()
        } else {
            preview
        };
        let DecodeStep::Frame(frame) = preview.next_step(None).expect("it decodes") else {
            panic!("no picture came out");
        };

        assert_eq!((frame.width, frame.height), (180, 320));
        if let Some(view) = frame.view() {
            assert_white_at_the_bottom(top_and_bottom(view.y, view.y_stride, 180, 320), "planes");
            assert_eq!(view.u.len(), 90 * 160);
        } else {
            // RGBA: take the red of each pixel as its brightness.
            let red: Vec<u8> = frame
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .map(|pixel| pixel[0])
                .collect();
            assert_white_at_the_bottom(top_and_bottom(&red, 180, 180, 320), "rgba");
        }
    }
}

#[test]
fn a_small_preview_keeps_the_upright_shape() {
    let mut preview = VideoPreview::open(&phone_mp4(), 90, 90)
        .expect("the video opens")
        .with_planes();
    let DecodeStep::Frame(frame) = preview.next_step(None).expect("it decodes") else {
        panic!("no picture came out");
    };

    // 90 high, so about 50 across, kept even.
    assert_eq!(frame.height, 90);
    assert!(
        frame.width < frame.height,
        "{} x {}",
        frame.width,
        frame.height
    );
}

#[test]
fn pictures_along_the_clip_are_the_right_way_up() {
    let mut found = Vec::new();
    read_strip_pictures(
        &phone_mp4(),
        Ticks::from_seconds(3.0),
        Ticks::from_seconds(1.0),
        (90, 160),
        &AtomicBool::new(false),
        |picture| found.push(picture),
    )
    .expect("pictures are read");

    assert!(!found.is_empty());
    for picture in found {
        assert_eq!((picture.width, picture.height), (90, 160));
        let red: Vec<u8> = picture
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|pixel| pixel[0])
            .collect();
        assert_white_at_the_bottom(top_and_bottom(&red, 90, 90, 160), "strip picture");
    }
}

#[test]
fn a_copied_cut_keeps_the_turn_so_players_stand_it_up() {
    let output = output_path("phone-cut", "mp4");

    // Between keyframes at both ends, so both ends are re-encoded and the middle is copied.
    let report = run_export(
        &phone_mp4(),
        &output,
        vec![range(0.5, 2.5)],
        CutMode::FrameExact,
    );
    assert!(
        report.encoded_frames > 0 && report.copied_frames > 0,
        "{report:?}"
    );

    let info = probe(&output).expect("the export can be read");
    assert_eq!(info.video.orientation, Orientation::QuarterAnticlockwise);
    assert_eq!((info.video.width, info.video.height), (180, 320));

    // Re-encoded and copied pictures alike come out the right way up.
    let mut reader = PictureReader::open(&output).expect("the export opens");
    let mut pictures = 0;
    while let Some(picture) = reader.next_picture().expect("the export decodes") {
        let view = picture.view();
        assert_white_at_the_bottom(
            top_and_bottom(view.y, view.y_stride, 180, 320),
            &format!("picture {pictures}"),
        );
        pictures += 1;
    }
    assert_eq!(pictures, 60);
}

#[test]
fn videos_turned_different_ways_cannot_be_copied_together() {
    let phone = probe(&phone_mp4()).expect("the video can be read");
    let mut plain = phone.clone();
    plain.video.orientation = Orientation::Upright;

    let problem = join_problem(&phone, &plain).expect("they can't be joined");

    assert!(problem.contains("turned a different way"), "{problem}");
    assert_eq!(join_problem(&phone, &phone.clone()), None);
}
