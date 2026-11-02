//! Plays real video files through the preview decoders.

mod common;

use std::path::Path;

use bareclip_media::{AudioChunk, AudioPreview, AudioStep, DecodeStep, PreviewFrame, VideoPreview};
use common::*;

/// Reads a frame number back from a preview picture.
fn number_of(frame: &PreviewFrame) -> u32 {
    let width = frame.width as usize;
    let row = width * 4 * (frame.height as usize / 2);
    let mut number = 0;
    for bit in 0..16 {
        let x = bit * 20 + 10;
        if frame.rgba[row + x * 4] > 128 {
            number |= 1 << bit;
        }
    }
    number
}

fn open(path: &Path) -> VideoPreview {
    VideoPreview::open(path, 1280, 720).expect("the preview opens")
}

/// Moves to an exact frame the way the player does.
fn frame_at(preview: &mut VideoPreview, pts: i64) -> PreviewFrame {
    preview.seek(pts).expect("seeking works");
    loop {
        match preview.next_step(Some(pts)).expect("decoding works") {
            DecodeStep::Frame(frame) => return frame,
            DecodeStep::Skipped { .. } => {}
            DecodeStep::End => panic!("the video ended before {pts}"),
        }
    }
}

#[test]
fn playback_shows_every_frame_in_order() {
    let mut preview = open(&h264_mp4());

    let mut shown = Vec::new();
    loop {
        match preview.next_step(None).expect("decoding works") {
            DecodeStep::Frame(frame) => shown.push(number_of(&frame)),
            DecodeStep::Skipped { .. } => panic!("nothing should be skipped"),
            DecodeStep::End => break,
        }
    }

    assert_eq!(shown, (0..300).collect::<Vec<u32>>());
}

#[test]
fn seeking_lands_on_the_exact_frame() {
    let path = h264_mp4();
    let index = index_of(&path);
    let mut preview = open(&path);
    let mut displayed: Vec<i64> = index.frames().iter().map(|frame| frame.pts).collect();
    displayed.sort_unstable();

    // Forwards, backwards, on keyframes and between them.
    for number in [0_usize, 45, 30, 299, 10, 151, 150, 149, 1] {
        let frame = frame_at(&mut preview, displayed[number]);
        assert_eq!(
            number_of(&frame) as usize,
            number,
            "seeking to frame {number}"
        );
    }
}

#[test]
fn seeking_works_with_b_frames() {
    let path = b_frames_two_tracks_mkv();
    let index = index_of(&path);
    let mut preview = open(&path);
    let mut displayed: Vec<i64> = index.frames().iter().map(|frame| frame.pts).collect();
    displayed.sort_unstable();

    for number in [100_usize, 3, 201, 64, 65, 66, 0, 299] {
        let frame = frame_at(&mut preview, displayed[number]);
        assert_eq!(
            number_of(&frame) as usize,
            number,
            "seeking to frame {number}"
        );
    }
}

#[test]
fn seeking_works_with_variable_frame_rate() {
    let path = variable_rate_mkv();
    let index = index_of(&path);
    let mut preview = open(&path);
    let mut displayed: Vec<i64> = index.frames().iter().map(|frame| frame.pts).collect();
    displayed.sort_unstable();

    for number in [77_usize, 5, 200, 201, 202] {
        let frame = frame_at(&mut preview, displayed[number]);
        assert_eq!(
            number_of(&frame) as usize,
            number,
            "seeking to frame {number}"
        );
    }
}

#[test]
fn playback_continues_correctly_after_a_seek() {
    let path = h264_mp4();
    let index = index_of(&path);
    let mut preview = open(&path);
    let mut displayed: Vec<i64> = index.frames().iter().map(|frame| frame.pts).collect();
    displayed.sort_unstable();

    assert_eq!(number_of(&frame_at(&mut preview, displayed[100])), 100);
    for wanted in 101..130 {
        match preview.next_step(None).expect("decoding works") {
            DecodeStep::Frame(frame) => assert_eq!(number_of(&frame), wanted),
            other => panic!("expected frame {wanted}, got {other:?}"),
        }
    }
}

#[test]
fn seeking_works_again_after_the_video_ends() {
    let path = h264_mp4();
    let index = index_of(&path);
    let mut preview = open(&path);
    let mut displayed: Vec<i64> = index.frames().iter().map(|frame| frame.pts).collect();
    displayed.sort_unstable();

    assert_eq!(number_of(&frame_at(&mut preview, displayed[299])), 299);
    while !matches!(
        preview.next_step(None).expect("decoding works"),
        DecodeStep::End
    ) {}

    assert_eq!(number_of(&frame_at(&mut preview, displayed[12])), 12);
}

#[test]
fn large_video_is_reduced_for_preview() {
    let mut preview = VideoPreview::open(&h264_mp4(), 160, 90).expect("the preview opens");

    match preview.next_step(None).expect("decoding works") {
        DecodeStep::Frame(frame) => {
            assert_eq!((frame.width, frame.height), (160, 90));
            assert_eq!(frame.rgba.len(), 160 * 90 * 4);
        }
        other => panic!("expected a frame, got {other:?}"),
    }
}

#[test]
fn colours_are_decoded_with_the_right_standard() {
    let path = bt709_color_clip();
    let mut preview = open(&path);

    let DecodeStep::Frame(frame) = preview.next_step(None).expect("decoding works") else {
        panic!("expected a frame");
    };

    let pixel = frame.width as usize * 4 * (frame.height as usize / 2) + 40;
    let [r, g, b] = [
        frame.rgba[pixel],
        frame.rgba[pixel + 1],
        frame.rgba[pixel + 2],
    ];
    let close = |value: u8, wanted: u8| value.abs_diff(wanted) <= 8;
    assert!(
        close(r, 0x30) && close(g, 0x80) && close(b, 0xC0),
        "expected about #3080C0, got #{r:02X}{g:02X}{b:02X}"
    );
}

fn all_chunks(preview: &mut AudioPreview) -> Vec<AudioChunk> {
    let mut chunks = Vec::new();
    loop {
        match preview.next_step().expect("decoding works") {
            AudioStep::Chunk(chunk) => chunks.push(chunk),
            AudioStep::Pending => {}
            AudioStep::End => return chunks,
        }
    }
}

#[test]
fn audio_decodes_every_track_to_stereo_at_the_requested_rate() {
    let mut preview =
        AudioPreview::open(&b_frames_two_tracks_mkv(), 44_100).expect("the audio opens");
    assert_eq!(preview.track_count(), 2);

    let chunks = all_chunks(&mut preview);

    for track in 0..2 {
        let samples: usize = chunks
            .iter()
            .filter(|chunk| chunk.track == track)
            .map(|chunk| chunk.samples.len())
            .sum();
        let seconds = samples as f64 / 2.0 / 44_100.0;
        assert!(
            (seconds - 10.0).abs() < 0.1,
            "track {track} lasted {seconds}s"
        );
    }
    assert_eq!(preview.finished_tracks(), [0, 1]);
}

#[test]
fn audio_seeking_lands_at_the_right_time() {
    let mut preview = AudioPreview::open(&h264_mp4(), 48_000).expect("the audio opens");

    preview.seek(5.0).expect("seeking works");
    let first = loop {
        match preview.next_step().expect("decoding works") {
            AudioStep::Chunk(chunk) => break chunk,
            AudioStep::Pending => {}
            AudioStep::End => panic!("the audio ended early"),
        }
    };

    let seconds = first.start_frame as f64 / 48_000.0;
    assert!((4.0..=5.1).contains(&seconds), "landed at {seconds}s");
}

#[test]
fn a_video_with_no_audio_reports_no_tracks() {
    let mut preview = AudioPreview::open(&variable_rate_mkv(), 48_000).expect("the audio opens");

    assert_eq!(preview.track_count(), 0);
    assert!(matches!(
        preview.next_step().expect("decoding works"),
        AudioStep::End
    ));
    preview.seek(1.0).expect("seeking does nothing");
}
